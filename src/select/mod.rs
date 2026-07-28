//! Filesystem selection: path specs, rsync filter rules, and walk.
//!
//! Filter semantics: [`docs/SELECTION.md`](../../docs/SELECTION.md).

pub mod from_file;
pub mod matcher;
pub mod pathnorm;
pub mod rules;
pub mod walk;

pub use from_file::{
    load_exclude_from, load_filter_from, load_include_from, read_capped_lines,
    read_capped_lines_from_reader, MAX_FILTER_FILE_BYTES, MAX_FILTER_FILE_LINES,
};
pub use rules::{parse_rule, Rule, RuleAction, RuleSet};
pub use walk::{
    collect_from_files_from, collect_from_sources, SelectedEntry, SelectionStats,
};

use crate::error::{Error, Result};
use pathnorm::has_trailing_slash;
use std::path::{Path, PathBuf};

/// Kind of a source argument after metadata inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    File,
    Dir,
}

/// One create source as given on the CLI (or equivalent).
///
/// Trailing slash on directory SRCs changes archive naming (rsync-inspired):
/// - `photos`  → members like `photos/a.jpg`
/// - `photos/` → members like `a.jpg`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    /// Path as resolved for I/O (trailing slash stripped for open/stat).
    pub path: PathBuf,
    /// User-facing path string (may include trailing slash).
    pub original: String,
    /// Whether the user path ended with `/` or `\`.
    pub trailing_slash: bool,
    /// File vs directory (from metadata).
    pub kind: SourceKind,
}

impl SourceSpec {
    /// Build a [`SourceSpec`] from a user path string.
    ///
    /// Does not follow symlinks for the kind check (`symlink_metadata`).
    pub fn from_user_path(s: impl AsRef<str>) -> Result<Self> {
        let original = s.as_ref().to_string();
        let trailing_slash = has_trailing_slash(&original);
        let path = strip_trailing_seps(PathBuf::from(&original));
        if path.as_os_str().is_empty() {
            return Err(Error::Selection("empty source path".into()));
        }
        let meta = std::fs::symlink_metadata(&path).map_err(|e| {
            Error::Selection(format!("stat {}: {e}", path.display()))
        })?;
        let kind = if meta.is_dir() {
            SourceKind::Dir
        } else if meta.is_file() {
            SourceKind::File
        } else {
            return Err(Error::NotRegularFile(path));
        };
        // Trailing slash on a file is unusual; treat as error for clarity.
        if trailing_slash && kind == SourceKind::File {
            return Err(Error::Selection(format!(
                "trailing slash on file source: {original}"
            )));
        }
        Ok(Self {
            path,
            original,
            trailing_slash,
            kind,
        })
    }

    /// Archive-name prefix for files under this source (no trailing `/`).
    ///
    /// - Dir without trailing slash: basename of the directory.
    /// - Dir with trailing slash: empty (contents at archive root).
    /// - File: empty (member is basename only).
    pub fn archive_prefix(&self) -> Result<String> {
        match self.kind {
            SourceKind::File => Ok(String::new()),
            SourceKind::Dir if self.trailing_slash => Ok(String::new()),
            SourceKind::Dir => {
                let name = pathnorm::basename_utf8(&self.path)?;
                Ok(name)
            }
        }
    }
}

fn strip_trailing_seps(mut p: PathBuf) -> PathBuf {
    // PathBuf doesn't always preserve trailing slash; handle string form too.
    let s = p.to_string_lossy();
    if has_trailing_slash(&s) {
        let trimmed = s.trim_end_matches(['/', '\\']);
        if trimmed.is_empty() {
            // "/" or "\" only — keep as root-like path for OS to resolve
            return PathBuf::from(if cfg!(windows) { "\\" } else { "/" });
        }
        p = PathBuf::from(trimmed);
    }
    p
}

/// Map a file under a source to its archive member name.
pub fn archive_name_for(source: &SourceSpec, file_abs: &Path) -> Result<String> {
    match source.kind {
        SourceKind::File => pathnorm::basename_utf8(file_abs),
        SourceKind::Dir => {
            let rel = file_abs
                .strip_prefix(&source.path)
                .map_err(|_| {
                    Error::Selection(format!(
                        "file {} is not under source {}",
                        file_abs.display(),
                        source.path.display()
                    ))
                })?;
            let rel_s = pathnorm::normalize_archive_path(rel)?;
            let prefix = source.archive_prefix()?;
            if prefix.is_empty() {
                Ok(rel_s)
            } else {
                pathnorm::join_archive_name(&format!("{prefix}/"), &rel_s)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn source_spec_dir_trailing_slash() {
        let dir = tempdir().unwrap();
        let d = dir.path().join("photos");
        fs::create_dir(&d).unwrap();
        let with = format!("{}/", d.display());
        let spec = SourceSpec::from_user_path(&with).unwrap();
        assert!(spec.trailing_slash);
        assert_eq!(spec.kind, SourceKind::Dir);
        assert_eq!(spec.archive_prefix().unwrap(), "");
    }

    #[test]
    fn source_spec_dir_no_trailing_slash() {
        let dir = tempdir().unwrap();
        let d = dir.path().join("photos");
        fs::create_dir(&d).unwrap();
        let spec = SourceSpec::from_user_path(d.to_str().unwrap()).unwrap();
        assert!(!spec.trailing_slash);
        assert_eq!(spec.archive_prefix().unwrap(), "photos");
    }

    #[test]
    fn source_spec_file() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("x.bin");
        fs::write(&f, b"hi").unwrap();
        let spec = SourceSpec::from_user_path(f.to_str().unwrap()).unwrap();
        assert_eq!(spec.kind, SourceKind::File);
        assert_eq!(spec.archive_prefix().unwrap(), "");
    }

    #[test]
    fn archive_name_dir_with_and_without_slash() {
        let dir = tempdir().unwrap();
        let photos = dir.path().join("photos");
        fs::create_dir(&photos).unwrap();
        let file = photos.join("a.jpg");
        fs::write(&file, b"x").unwrap();

        let no_slash = SourceSpec::from_user_path(photos.to_str().unwrap()).unwrap();
        assert_eq!(
            archive_name_for(&no_slash, &file).unwrap(),
            "photos/a.jpg"
        );

        let with = format!("{}/", photos.display());
        let slash = SourceSpec::from_user_path(&with).unwrap();
        assert_eq!(archive_name_for(&slash, &file).unwrap(), "a.jpg");
    }

    #[test]
    fn archive_name_single_file_is_basename() {
        let dir = tempdir().unwrap();
        let nested = dir.path().join("dir");
        fs::create_dir(&nested).unwrap();
        let f = nested.join("x.bin");
        fs::write(&f, b"x").unwrap();
        let spec = SourceSpec::from_user_path(f.to_str().unwrap()).unwrap();
        assert_eq!(archive_name_for(&spec, &f).unwrap(), "x.bin");
    }
}
