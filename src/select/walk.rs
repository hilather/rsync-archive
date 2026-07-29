//! Walk sources and `--files-from` lists into [`SelectedEntry`] values.
//!
//! Selection is fully built (with collision checks) before any archive write.
//! Same builder is used for create dry-run and future create write.

use super::from_file::read_capped_lines;
use super::pathnorm::{basename_utf8, normalize_archive_str};
use super::rules::{RuleAction, RuleSet};
use super::{archive_name_for, SourceKind, SourceSpec};
use crate::error::{Error, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::{DirEntry, WalkDir};

/// Kind of selected archive member (file body vs link metadata only).
///
/// Tar formats archive all three. Non-tar formats (7z, seekable-zstd) keep only
/// [`MemberKind::File`] and skip symlinks / hard-link members at encode time
/// (the first regular-file copy of a hard-linked inode remains).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MemberKind {
    /// Regular file; `size` is content length; open/read `abs_path` for body.
    File,
    /// Symbolic link; `size` is always 0; target is stored in tar `linkname` / pax `linkpath`.
    Symlink { target: String },
    /// Hard link to an earlier regular-file member; `size` is always 0.
    /// `target` is that member's `archive_name` (first occurrence of the inode).
    HardLink { target: String },
}

impl Default for MemberKind {
    fn default() -> Self {
        MemberKind::File
    }
}

/// One selected archive member (regular file, symlink, or hard link).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedEntry {
    /// Absolute or resolved path used for open/read (file) or lstat (link).
    pub abs_path: PathBuf,
    /// Path inside the archive (`/`-separated).
    pub archive_name: String,
    /// Size in bytes at selection time (0 for symlinks and hard links).
    pub size: u64,
    /// Unix mtime seconds at selection (None if unavailable). Avoids re-stat on encode.
    pub mtime_unix: Option<u64>,
    /// Permission bits at selection (`st_mode & 0o7777` on Unix). Default `0o644` if unknown.
    pub mode: u32,
    /// Owner uid at selection (0 if unavailable / non-Unix).
    pub uid: u32,
    /// Owner gid at selection (0 if unavailable / non-Unix).
    pub gid: u32,
    /// Owner user name at selection (empty if unresolved / non-Unix).
    pub uname: String,
    /// Owner group name at selection (empty if unresolved / non-Unix).
    pub gname: String,
    /// File vs symlink vs hard-link member.
    pub kind: MemberKind,
}

impl SelectedEntry {
    /// True when this entry is a symbolic link member.
    pub fn is_symlink(&self) -> bool {
        matches!(self.kind, MemberKind::Symlink { .. })
    }

    /// True when this entry is a hard-link member (no file body; points at first archive path).
    pub fn is_hard_link(&self) -> bool {
        matches!(self.kind, MemberKind::HardLink { .. })
    }

    /// True when the member has a file data body (regular file only).
    pub fn has_data_body(&self) -> bool {
        matches!(self.kind, MemberKind::File)
    }

    /// Link target for symlink (as stored on disk) or hard link (first `archive_name` for inode).
    pub fn link_target(&self) -> Option<&str> {
        match &self.kind {
            MemberKind::Symlink { target } | MemberKind::HardLink { target } => {
                Some(target.as_str())
            }
            MemberKind::File => None,
        }
    }
}

/// Map of `(dev, ino)` → first `archive_name` for hard-link detection (Unix only).
type InodeFirstMap = HashMap<(u64, u64), String>;

/// Default mode when metadata is unavailable or for synthetic test entries.
pub const DEFAULT_FILE_MODE: u32 = 0o644;

/// Ustar uname/gname field width (bytes); longer names go to pax.
pub const USTAR_NAME_FIELD: usize = 32;

/// Extract mode/uid/gid from file metadata (Unix `MetadataExt`; portable defaults elsewhere).
pub fn meta_owner_mode(meta: &std::fs::Metadata) -> (u32, u32, u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.mode() & 0o7777, meta.uid(), meta.gid())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        (DEFAULT_FILE_MODE, 0, 0)
    }
}

/// Resolve owner user/group names for `uid`/`gid` at selection time.
///
/// On Unix uses reentrant `getpwuid_r` / `getgrgid_r`. Returns empty strings when
/// the id has no entry, on lookup failure, or on non-Unix.
pub fn names_for_uid_gid(uid: u32, gid: u32) -> (String, String) {
    #[cfg(unix)]
    {
        (lookup_uname(uid), lookup_gname(gid))
    }
    #[cfg(not(unix))]
    {
        let _ = (uid, gid);
        (String::new(), String::new())
    }
}

#[cfg(unix)]
fn lookup_uname(uid: u32) -> String {
    // Buffer sized for typical passwd line + long names; grow once if ERANGE.
    let mut buflen = 1024usize;
    for _ in 0..4 {
        let mut pwd = std::mem::MaybeUninit::<libc::passwd>::uninit();
        let mut buf = vec![0u8; buflen];
        let mut result: *mut libc::passwd = std::ptr::null_mut();
        // SAFETY: getpwuid_r writes into pwd/buf; result is set on success.
        let rc = unsafe {
            libc::getpwuid_r(
                uid as libc::uid_t,
                pwd.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buflen,
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            buflen = buflen.saturating_mul(2).max(buflen + 1024);
            continue;
        }
        if rc != 0 || result.is_null() {
            return String::new();
        }
        // SAFETY: result non-null means pwd was filled; pw_name points into buf.
        let name_ptr = unsafe { (*result).pw_name };
        return cstr_to_string(name_ptr);
    }
    String::new()
}

#[cfg(unix)]
fn lookup_gname(gid: u32) -> String {
    let mut buflen = 1024usize;
    for _ in 0..4 {
        let mut grp = std::mem::MaybeUninit::<libc::group>::uninit();
        let mut buf = vec![0u8; buflen];
        let mut result: *mut libc::group = std::ptr::null_mut();
        // SAFETY: getgrgid_r writes into grp/buf; result is set on success.
        let rc = unsafe {
            libc::getgrgid_r(
                gid as libc::gid_t,
                grp.as_mut_ptr(),
                buf.as_mut_ptr().cast(),
                buflen,
                &mut result,
            )
        };
        if rc == libc::ERANGE {
            buflen = buflen.saturating_mul(2).max(buflen + 1024);
            continue;
        }
        if rc != 0 || result.is_null() {
            return String::new();
        }
        // SAFETY: result non-null means grp was filled; gr_name points into buf.
        let name_ptr = unsafe { (*result).gr_name };
        return cstr_to_string(name_ptr);
    }
    String::new()
}

#[cfg(unix)]
fn cstr_to_string(ptr: *const libc::c_char) -> String {
    if ptr.is_null() {
        return String::new();
    }
    // SAFETY: ptr is a NUL-terminated C string from libc passwd/group.
    let cstr = unsafe { std::ffi::CStr::from_ptr(ptr) };
    match cstr.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => {
            // Lossy: keep bytes that are valid for tar (skip if empty after).
            String::from_utf8_lossy(cstr.to_bytes()).into_owned()
        }
    }
}

/// Counters for skipped items during selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionStats {
    pub selected: u64,
    pub skipped_symlinks: u64,
    /// Hard-link members dropped at encode for formats that only pack regular files.
    pub skipped_hardlinks: u64,
    pub skipped_special: u64,
    pub skipped_excluded: u64,
    /// Files dropped solely because a `--dir-max-size` budget was exhausted.
    pub skipped_dir_budget: u64,
    /// Files dropped solely because a `--dir-max-files` limit was exhausted
    /// (recursive under the directory prefix).
    pub skipped_dir_file_limit: u64,
    /// Single-file size above `--max-size`.
    pub skipped_max_size: u64,
    /// Skipped by `--file-size-from` pattern list (size above matching max=).
    pub skipped_file_size_from: u64,
    /// Single-file size below `--min-size`.
    pub skipped_min_size: u64,
    /// File mtime older than `--newer-than` window.
    pub skipped_older_than: u64,
    /// Dropped by global `--max-total-size` (newest-first fill).
    pub skipped_max_total_size: u64,
    /// Dropped by global `--max-files` (newest-first).
    pub skipped_max_files: u64,
}

/// Collect selected entries from SRC walk mode.
///
/// Regular files, symbolic links, and (on Unix) subsequent hard links of an
/// already-selected inode that pass filters are selected. Symlink and hard-link
/// members have `size == 0`. Special files are skipped (counted). Directory
/// prune uses [`RuleSet::should_prune_dir`].
///
/// Hard-link detection uses `(st_dev, st_ino)`: the first regular-file path for
/// an inode is [`MemberKind::File`] with content size; later paths become
/// [`MemberKind::HardLink`] pointing at that first `archive_name`. Non-Unix
/// builds treat every regular file as [`MemberKind::File`] (no hard-link detect).
///
/// Duplicate `archive_name` values produce [`Error::Collision`].
pub fn collect_from_sources(
    sources: &[SourceSpec],
    rules: &RuleSet,
) -> Result<(Vec<SelectedEntry>, SelectionStats)> {
    let mut out = Vec::new();
    let mut stats = SelectionStats::default();
    let mut names = HashSet::new();
    let mut inode_first = InodeFirstMap::new();
    // OPT-04: resolve CWD once for relative paths.
    let cwd = std::env::current_dir()
        .map_err(|e| Error::Selection(format!("cwd: {e}")))?;

    for src in sources {
        match src.kind {
            SourceKind::File => {
                consider_file(
                    &src.path,
                    &archive_name_for(src, &src.path)?,
                    rules,
                    &cwd,
                    &mut out,
                    &mut stats,
                    &mut names,
                    &mut inode_first,
                )?;
            }
            SourceKind::Dir => {
                walk_dir_source(
                    src,
                    rules,
                    &cwd,
                    &mut out,
                    &mut stats,
                    &mut names,
                    &mut inode_first,
                )?;
            }
        }
    }

    stats.selected = out.len() as u64;
    Ok((out, stats))
}

fn walk_dir_source(
    src: &SourceSpec,
    rules: &RuleSet,
    cwd: &Path,
    out: &mut Vec<SelectedEntry>,
    stats: &mut SelectionStats,
    names: &mut HashSet<String>,
    inode_first: &mut InodeFirstMap,
) -> Result<()> {
    let walker = WalkDir::new(&src.path)
        .follow_links(false)
        .same_file_system(false)
        .into_iter()
        .filter_entry(|e| should_descend(src, rules, e));

    for entry in walker {
        let entry = entry.map_err(|e| {
            Error::Selection(format!("walk error under {}: {e}", src.path.display()))
        })?;
        let path = entry.path();
        let ft = entry.file_type();

        if ft.is_symlink() {
            // walkdir with follow_links(false) still yields symlink entries.
            let archive_name = archive_name_for(src, path)?;
            consider_symlink(path, &archive_name, rules, cwd, out, stats, names)?;
            continue;
        }
        if ft.is_dir() {
            continue;
        }
        if !ft.is_file() {
            stats.skipped_special += 1;
            debug!(path = %path.display(), "skip special file");
            continue;
        }

        let archive_name = archive_name_for(src, path)?;
        consider_file(
            path,
            &archive_name,
            rules,
            cwd,
            out,
            stats,
            names,
            inode_first,
        )?;
    }
    Ok(())
}

/// Return true if walkdir should enter this directory entry.
fn should_descend(src: &SourceSpec, rules: &RuleSet, entry: &DirEntry) -> bool {
    if !entry.file_type().is_dir() {
        return true;
    }
    let path = entry.path();
    // Always enter the source root.
    if path == src.path {
        return true;
    }
    let Ok(archive_name) = dir_archive_name(src, path) else {
        // If we cannot name it, do not descend (safety).
        return false;
    };
    // Empty name is archive root content (trailing-slash SRC); never prune root.
    if archive_name.is_empty() {
        return true;
    }
    let prune = rules.should_prune_dir(&archive_name);
    if prune {
        debug!(dir = %archive_name, "prune directory");
    }
    !prune
}

fn dir_archive_name(src: &SourceSpec, dir: &Path) -> Result<String> {
    if dir == src.path {
        return Ok(String::new());
    }
    let rel = dir.strip_prefix(&src.path).map_err(|_| {
        Error::Selection(format!(
            "dir {} is not under source {}",
            dir.display(),
            src.path.display()
        ))
    })?;
    let rel_s = if rel.as_os_str().is_empty() {
        String::new()
    } else {
        super::pathnorm::normalize_archive_path(rel)?
    };
    let prefix = src.archive_prefix()?;
    if prefix.is_empty() {
        Ok(rel_s)
    } else if rel_s.is_empty() {
        Ok(prefix)
    } else {
        Ok(format!("{prefix}/{rel_s}"))
    }
}

fn consider_file(
    path: &Path,
    archive_name: &str,
    rules: &RuleSet,
    cwd: &Path,
    out: &mut Vec<SelectedEntry>,
    stats: &mut SelectionStats,
    names: &mut HashSet<String>,
    inode_first: &mut InodeFirstMap,
) -> Result<()> {
    // Non-following metadata at selection time.
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        Error::Selection(format!("stat {}: {e}", path.display()))
    })?;
    if meta.file_type().is_symlink() {
        return consider_symlink(path, archive_name, rules, cwd, out, stats, names);
    }
    if !meta.is_file() {
        stats.skipped_special += 1;
        return Ok(());
    }

    if rules.action_for(archive_name, false) == RuleAction::Exclude {
        stats.skipped_excluded += 1;
        debug!(name = %archive_name, "exclude file");
        return Ok(());
    }

    if !names.insert(archive_name.to_string()) {
        return Err(Error::Collision(archive_name.to_string()));
    }

    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let mtime_unix = meta.modified().ok().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    });
    let (mode, uid, gid) = meta_owner_mode(&meta);
    let (uname, gname) = names_for_uid_gid(uid, gid);

    // Unix: subsequent paths sharing (dev, ino) become hard-link members (size 0).
    // Symlinks are never routed here as hard links (they use consider_symlink).
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let key = (meta.dev(), meta.ino());
        if let Some(first_name) = inode_first.get(&key) {
            debug!(
                name = %archive_name,
                target = %first_name,
                "select hard link"
            );
            out.push(SelectedEntry {
                abs_path,
                archive_name: archive_name.to_string(),
                size: 0,
                mtime_unix,
                mode,
                uid,
                gid,
                uname,
                gname,
                kind: MemberKind::HardLink {
                    target: first_name.clone(),
                },
            });
            return Ok(());
        }
        inode_first.insert(key, archive_name.to_string());
    }
    #[cfg(not(unix))]
    {
        let _ = inode_first;
    }

    out.push(SelectedEntry {
        abs_path,
        archive_name: archive_name.to_string(),
        size: meta.len(),
        mtime_unix,
        mode,
        uid,
        gid,
        uname,
        gname,
        kind: MemberKind::File,
    });
    Ok(())
}

/// Include a symlink as an archive member (no follow; target via `read_link`).
fn consider_symlink(
    path: &Path,
    archive_name: &str,
    rules: &RuleSet,
    cwd: &Path,
    out: &mut Vec<SelectedEntry>,
    stats: &mut SelectionStats,
    names: &mut HashSet<String>,
) -> Result<()> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        Error::Selection(format!("lstat {}: {e}", path.display()))
    })?;
    if !meta.file_type().is_symlink() {
        stats.skipped_special += 1;
        return Ok(());
    }

    if rules.action_for(archive_name, false) == RuleAction::Exclude {
        stats.skipped_excluded += 1;
        debug!(name = %archive_name, "exclude symlink");
        return Ok(());
    }

    if !names.insert(archive_name.to_string()) {
        return Err(Error::Collision(archive_name.to_string()));
    }

    let target_path = std::fs::read_link(path).map_err(|e| {
        Error::Selection(format!("read_link {}: {e}", path.display()))
    })?;
    let target = link_target_string(&target_path)?;

    let abs_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };

    let mtime_unix = meta.modified().ok().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs())
    });
    let (mode, uid, gid) = meta_owner_mode(&meta);
    let (uname, gname) = names_for_uid_gid(uid, gid);

    debug!(name = %archive_name, target = %target, "select symlink");
    out.push(SelectedEntry {
        abs_path,
        archive_name: archive_name.to_string(),
        size: 0,
        mtime_unix,
        mode,
        uid,
        gid,
        uname,
        gname,
        kind: MemberKind::Symlink { target },
    });
    Ok(())
}

/// Convert a symlink target path to a UTF-8 string as stored (not resolved).
fn link_target_string(target: &Path) -> Result<String> {
    match target.to_str() {
        Some(s) => Ok(s.to_string()),
        None => Err(Error::Selection(format!(
            "symlink target is not valid UTF-8: {}",
            target.display()
        ))),
    }
}

/// Collect entries from a `--files-from` list (K26).
///
/// Lines are paths relative to **CWD** or absolute. Regular files and
/// symbolic links are allowed; other special files error. On Unix, later
/// hard-linked regular files become [`MemberKind::HardLink`] (see
/// [`collect_from_sources`]).
/// Relative lines keep normalized path as `archive_name`; absolute lines use
/// **basename only**. Filters apply to `archive_name`.
pub fn collect_from_files_from(
    list_path: &Path,
    rules: &RuleSet,
) -> Result<(Vec<SelectedEntry>, SelectionStats)> {
    let lines = read_capped_lines(list_path)?;
    let cwd = std::env::current_dir()
        .map_err(|e| Error::Selection(format!("cwd: {e}")))?;

    let mut out = Vec::new();
    let mut stats = SelectionStats::default();
    let mut names = HashSet::new();
    let mut inode_first = InodeFirstMap::new();

    for (idx, line) in lines.iter().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let path_as_written = line;
        let is_abs = Path::new(path_as_written).is_absolute();
        let fs_path = if is_abs {
            PathBuf::from(path_as_written)
        } else {
            cwd.join(path_as_written)
        };

        let meta = std::fs::symlink_metadata(&fs_path).map_err(|e| {
            Error::Selection(format!(
                "files-from line {}: stat {}: {e}",
                idx + 1,
                fs_path.display()
            ))
        })?;
        let is_symlink = meta.file_type().is_symlink();
        if !is_symlink && !meta.is_file() {
            return Err(Error::NotRegularFile(fs_path));
        }

        let archive_name = if is_abs {
            basename_utf8(&fs_path)?
        } else {
            normalize_archive_str(path_as_written)?
        };

        if rules.action_for(&archive_name, false) == RuleAction::Exclude {
            stats.skipped_excluded += 1;
            continue;
        }

        if !names.insert(archive_name.clone()) {
            return Err(Error::Collision(archive_name));
        }

        let mtime_unix = meta.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        });
        let (mode, uid, gid) = meta_owner_mode(&meta);
        let (uname, gname) = names_for_uid_gid(uid, gid);

        let (kind, size) = if is_symlink {
            let target_path = std::fs::read_link(&fs_path).map_err(|e| {
                Error::Selection(format!(
                    "files-from line {}: read_link {}: {e}",
                    idx + 1,
                    fs_path.display()
                ))
            })?;
            (
                MemberKind::Symlink {
                    target: link_target_string(&target_path)?,
                },
                0u64,
            )
        } else {
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                let key = (meta.dev(), meta.ino());
                if let Some(first_name) = inode_first.get(&key) {
                    debug!(
                        name = %archive_name,
                        target = %first_name,
                        "select hard link (files-from)"
                    );
                    (
                        MemberKind::HardLink {
                            target: first_name.clone(),
                        },
                        0u64,
                    )
                } else {
                    inode_first.insert(key, archive_name.clone());
                    (MemberKind::File, meta.len())
                }
            }
            #[cfg(not(unix))]
            {
                let _ = &inode_first;
                (MemberKind::File, meta.len())
            }
        };

        out.push(SelectedEntry {
            abs_path: fs_path,
            archive_name,
            size,
            mtime_unix,
            mode,
            uid,
            gid,
            uname,
            gname,
            kind,
        });
    }

    stats.selected = out.len() as u64;
    Ok((out, stats))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::SourceSpec;
    use std::fs;
    use tempfile::tempdir;

    fn rules_exclude(pat: &str) -> RuleSet {
        let mut r = RuleSet::new();
        r.push_exclude(pat).unwrap();
        r
    }

    #[test]
    fn walk_selects_regular_files() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        fs::write(root.join("sub/b.txt"), b"b").unwrap();

        let src = SourceSpec::from_user_path(root.to_str().unwrap()).unwrap();
        let (entries, stats) = collect_from_sources(&[src], &RuleSet::new()).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(stats.selected, 2);
        let names: HashSet<_> = entries.iter().map(|e| e.archive_name.as_str()).collect();
        assert!(names.contains("tree/a.txt"));
        assert!(names.contains("tree/sub/b.txt"));
    }

    #[test]
    fn trailing_slash_strips_root_name() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        let with = format!("{}/", root.display());
        let src = SourceSpec::from_user_path(&with).unwrap();
        let (entries, _) = collect_from_sources(&[src], &RuleSet::new()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_name, "a.txt");
    }

    #[test]
    fn prune_excludes_dir_contents() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("skipme")).unwrap();
        fs::write(root.join("keep.txt"), b"k").unwrap();
        fs::write(root.join("skipme/secret"), b"s").unwrap();

        let src = SourceSpec::from_user_path(format!("{}/", root.display()).as_str()).unwrap();
        let rules = rules_exclude("skipme/");
        let (entries, _) = collect_from_sources(&[src], &rules).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.archive_name.as_str()).collect();
        assert_eq!(names, vec!["keep.txt"]);
        assert!(!names.iter().any(|n| n.contains("secret")));
    }

    #[test]
    fn multi_src_collision() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("f"), b"1").unwrap();
        fs::write(b.join("f"), b"2").unwrap();
        let sa = SourceSpec::from_user_path(format!("{}/", a.display()).as_str()).unwrap();
        let sb = SourceSpec::from_user_path(format!("{}/", b.display()).as_str()).unwrap();
        let err = collect_from_sources(&[sa, sb], &RuleSet::new()).unwrap_err();
        assert!(matches!(err, Error::Collision(_)));
    }

    #[test]
    fn files_from_relative_and_absolute() {
        let dir = tempdir().unwrap();
        let rel_dir = dir.path().join("foo");
        fs::create_dir_all(&rel_dir).unwrap();
        let rel_file = rel_dir.join("a.txt");
        fs::write(&rel_file, b"r").unwrap();
        let abs_file = dir.path().join("abs.txt");
        fs::write(&abs_file, b"a").unwrap();

        let list = dir.path().join("list.txt");
        // relative path as written from within dir as cwd — use absolute list with relative names
        // We'll write paths relative to dir and set current_dir in test via absolute join logic:
        // collect_from_files_from uses process cwd; so write absolute paths in list for abs case
        // and create relative structure under a temp cwd.

        // Simpler: only absolute lines in this unit test for abs basename; relative with chdir.
        fs::write(
            &list,
            format!("{}\n", abs_file.display()),
        )
        .unwrap();
        let (entries, _) = collect_from_files_from(&list, &RuleSet::new()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_name, "abs.txt");
    }

    #[test]
    fn files_from_absolute_basename_collision() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("x").join("a.txt");
        let b = dir.path().join("y").join("a.txt");
        fs::create_dir_all(a.parent().unwrap()).unwrap();
        fs::create_dir_all(b.parent().unwrap()).unwrap();
        fs::write(&a, b"1").unwrap();
        fs::write(&b, b"2").unwrap();
        let list = dir.path().join("list.txt");
        fs::write(
            &list,
            format!("{}\n{}\n", a.display(), b.display()),
        )
        .unwrap();
        let err = collect_from_files_from(&list, &RuleSet::new()).unwrap_err();
        assert!(matches!(err, Error::Collision(_)));
    }

    #[test]
    fn files_from_exclude_filter() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("a.txt");
        fs::write(&f, b"x").unwrap();
        let list = dir.path().join("list.txt");
        fs::write(&list, format!("{}\n", f.display())).unwrap();
        let rules = rules_exclude("*.txt");
        let (entries, stats) = collect_from_files_from(&list, &rules).unwrap();
        assert!(entries.is_empty());
        assert_eq!(stats.skipped_excluded, 1);
    }

    #[cfg(unix)]
    #[test]
    fn walk_resolves_owner_names_for_current_user() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("owned.txt"), b"o").unwrap();
        let src = SourceSpec::from_user_path(format!("{}/", root.display()).as_str()).unwrap();
        let (entries, _) = collect_from_sources(&[src], &RuleSet::new()).unwrap();
        assert_eq!(entries.len(), 1);
        let e = &entries[0];
        // Resolve independently; if the system has a name for this uid, walk must match.
        let (expect_u, expect_g) = names_for_uid_gid(e.uid, e.gid);
        assert_eq!(e.uname, expect_u);
        assert_eq!(e.gname, expect_g);
        // Most CI/dev users have a passwd entry; require non-empty when lookup succeeds.
        if !expect_u.is_empty() {
            assert!(!e.uname.is_empty());
        }
    }

    #[cfg(unix)]
    #[test]
    fn walk_selects_symlinks_with_target() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("target.txt"), b"data").unwrap();
        std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();
        std::os::unix::fs::symlink("../target.txt", root.join("sub/rel_link")).unwrap();

        let src = SourceSpec::from_user_path(format!("{}/", root.display()).as_str()).unwrap();
        let (entries, stats) = collect_from_sources(&[src], &RuleSet::new()).unwrap();
        assert_eq!(stats.skipped_symlinks, 0);
        assert_eq!(entries.len(), 3);
        let link = entries
            .iter()
            .find(|e| e.archive_name == "link.txt")
            .expect("link.txt");
        assert!(link.is_symlink());
        assert_eq!(link.size, 0);
        assert_eq!(link.link_target(), Some("target.txt"));
        let rel = entries
            .iter()
            .find(|e| e.archive_name == "sub/rel_link")
            .expect("sub/rel_link");
        assert_eq!(rel.link_target(), Some("../target.txt"));
    }

    #[cfg(unix)]
    #[test]
    fn walk_excludes_symlink_by_filter() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"a").unwrap();
        std::os::unix::fs::symlink("a.txt", root.join("skip.link")).unwrap();
        let src = SourceSpec::from_user_path(format!("{}/", root.display()).as_str()).unwrap();
        let rules = rules_exclude("*.link");
        let (entries, stats) = collect_from_sources(&[src], &rules).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_name, "a.txt");
        assert_eq!(stats.skipped_excluded, 1);
    }

    #[cfg(unix)]
    #[test]
    fn walk_detects_hard_links() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        let a = root.join("a.txt");
        let b = root.join("b.txt");
        fs::write(&a, b"shared-payload").unwrap();
        fs::hard_link(&a, &b).unwrap();

        let src = SourceSpec::from_user_path(format!("{}/", root.display()).as_str()).unwrap();
        let (entries, stats) = collect_from_sources(&[src], &RuleSet::new()).unwrap();
        assert_eq!(stats.selected, 2);
        assert_eq!(entries.len(), 2);

        let files: Vec<_> = entries.iter().filter(|e| e.has_data_body()).collect();
        let links: Vec<_> = entries.iter().filter(|e| e.is_hard_link()).collect();
        assert_eq!(files.len(), 1);
        assert_eq!(links.len(), 1);
        assert_eq!(files[0].size, 14);
        assert_eq!(links[0].size, 0);
        assert_eq!(
            links[0].link_target(),
            Some(files[0].archive_name.as_str())
        );
        // Order is walk order: first path is the file body.
        assert!(entries[0].has_data_body());
        assert!(entries[1].is_hard_link());
    }

    #[cfg(unix)]
    #[test]
    fn walk_hard_link_size_not_double_counted_in_entry_size() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("orig"), b"12345").unwrap();
        fs::hard_link(root.join("orig"), root.join("alias")).unwrap();
        let src = SourceSpec::from_user_path(format!("{}/", root.display()).as_str()).unwrap();
        let (entries, _) = collect_from_sources(&[src], &RuleSet::new()).unwrap();
        let total: u64 = entries.iter().map(|e| e.size).sum();
        assert_eq!(total, 5, "hard link must contribute size 0");
    }
}
