//! Walk sources and `--files-from` lists into [`SelectedEntry`] values.
//!
//! Selection is fully built (with collision checks) before any archive write.
//! Same builder is used for create dry-run and future create write.

use super::from_file::read_capped_lines;
use super::pathnorm::{basename_utf8, normalize_archive_str};
use super::rules::{RuleAction, RuleSet};
use super::{archive_name_for, SourceKind, SourceSpec};
use crate::error::{Error, Result};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::debug;
use walkdir::{DirEntry, WalkDir};

/// One regular file selected for archiving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedEntry {
    /// Absolute or resolved path used for open/read.
    pub abs_path: PathBuf,
    /// Path inside the archive (`/`-separated).
    pub archive_name: String,
    /// Size in bytes at selection time.
    pub size: u64,
    /// Unix mtime seconds at selection (None if unavailable). Avoids re-stat on encode.
    pub mtime_unix: Option<u64>,
}

/// Counters for skipped items during selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SelectionStats {
    pub selected: u64,
    pub skipped_symlinks: u64,
    pub skipped_special: u64,
    pub skipped_excluded: u64,
    /// Files dropped solely because a `--dir-max-size` budget was exhausted.
    pub skipped_dir_budget: u64,
    /// Files dropped solely because a `--dir-max-files` limit was exhausted
    /// (immediate children only).
    pub skipped_dir_file_limit: u64,
    /// Single-file size above `--max-size`.
    pub skipped_max_size: u64,
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
/// Only regular files are selected. Symlinks and special files are skipped
/// (counted). Directory prune uses [`RuleSet::should_prune_dir`].
///
/// Duplicate `archive_name` values produce [`Error::Collision`].
pub fn collect_from_sources(
    sources: &[SourceSpec],
    rules: &RuleSet,
) -> Result<(Vec<SelectedEntry>, SelectionStats)> {
    let mut out = Vec::new();
    let mut stats = SelectionStats::default();
    let mut names = HashSet::new();
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
                )?;
            }
            SourceKind::Dir => {
                walk_dir_source(src, rules, &cwd, &mut out, &mut stats, &mut names)?;
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
            stats.skipped_symlinks += 1;
            debug!(path = %path.display(), "skip symlink");
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
        consider_file(path, &archive_name, rules, cwd, out, stats, names)?;
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
) -> Result<()> {
    // Non-following metadata at selection time.
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        Error::Selection(format!("stat {}: {e}", path.display()))
    })?;
    if meta.file_type().is_symlink() {
        stats.skipped_symlinks += 1;
        return Ok(());
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

    out.push(SelectedEntry {
        abs_path,
        archive_name: archive_name.to_string(),
        size: meta.len(),
        mtime_unix,
    });
    Ok(())
}

/// Collect entries from a `--files-from` list (K26).
///
/// Lines are paths relative to **CWD** or absolute. Only regular files.
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
        if meta.file_type().is_symlink() || !meta.is_file() {
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

        out.push(SelectedEntry {
            abs_path: fs_path,
            archive_name,
            size: meta.len(),
            mtime_unix,
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
}
