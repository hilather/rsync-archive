//! Output path helpers: overwrite checks, partial files, atomic rename.

use crate::error::{Error, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// Final and partial paths for an atomic archive write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutputPaths {
    /// User-facing final path (`-o`).
    pub final_path: PathBuf,
    /// Same-directory partial: `{final}.partial` (e.g. `out.7z.partial`).
    pub partial_path: PathBuf,
}

impl OutputPaths {
    /// Compute paths for `-o`.
    ///
    /// Naming: append `.partial` to the full final path string
    /// (`archive.7z` → `archive.7z.partial`).
    pub fn new(final_path: impl Into<PathBuf>) -> Self {
        let final_path = final_path.into();
        let partial_path = partial_path_for(&final_path);
        Self {
            final_path,
            partial_path,
        }
    }
}

/// `{path}.partial` in the same directory (does not replace extension).
pub fn partial_path_for(final_path: &Path) -> PathBuf {
    let mut os = final_path.as_os_str().to_owned();
    os.push(".partial");
    PathBuf::from(os)
}

/// Ensure we may write to `final_path`.
///
/// - If final exists and `force` is false → [`Error::OutputExists`].
/// - Stale partial is removed when `force` is true or when final does not
///   exist (safe cleanup of interrupted runs). If final exists and `force`
///   is false we error before touching partial.
pub fn prepare_output(final_path: &Path, force: bool) -> Result<OutputPaths> {
    let paths = OutputPaths::new(final_path);
    if paths.final_path.exists() {
        if !force {
            return Err(Error::OutputExists(paths.final_path.clone()));
        }
    }
    if paths.partial_path.exists() {
        fs::remove_file(&paths.partial_path)?;
    }
    if let Some(parent) = paths.final_path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(paths)
}

/// Rename partial → final after successful finish. Removes existing final if present.
pub fn commit_output(paths: &OutputPaths) -> Result<()> {
    if paths.final_path.exists() {
        fs::remove_file(&paths.final_path)?;
    }
    fs::rename(&paths.partial_path, &paths.final_path)?;
    Ok(())
}

/// Best-effort cleanup of partial on failure (does not error if missing).
pub fn cleanup_partial(paths: &OutputPaths) {
    let _ = fs::remove_file(&paths.partial_path);
}

/// Check whether the final output path already exists.
pub fn output_exists(final_path: &Path) -> bool {
    final_path.exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn partial_naming_appends_partial() {
        assert_eq!(
            partial_path_for(Path::new("out.7z")),
            PathBuf::from("out.7z.partial")
        );
        assert_eq!(
            partial_path_for(Path::new("/tmp/archive.7z")),
            PathBuf::from("/tmp/archive.7z.partial")
        );
        assert_eq!(
            partial_path_for(Path::new("noext")),
            PathBuf::from("noext.partial")
        );
        let p = OutputPaths::new("dir/a.7z");
        assert_eq!(p.partial_path, PathBuf::from("dir/a.7z.partial"));
        assert_eq!(p.final_path, PathBuf::from("dir/a.7z"));
    }

    #[test]
    fn prepare_errors_if_exists_without_force() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.7z");
        fs::write(&out, b"x").unwrap();
        let err = prepare_output(&out, false).unwrap_err();
        assert!(matches!(err, Error::OutputExists(_)));
    }

    #[test]
    fn prepare_force_allows_existing_and_clears_stale_partial() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.7z");
        let partial = partial_path_for(&out);
        fs::write(&out, b"old").unwrap();
        fs::write(&partial, b"stale").unwrap();
        let paths = prepare_output(&out, true).unwrap();
        assert_eq!(paths.final_path, out);
        assert!(!paths.partial_path.exists());
    }

    #[test]
    fn prepare_removes_stale_partial_when_final_absent() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.7z");
        let partial = partial_path_for(&out);
        fs::write(&partial, b"stale").unwrap();
        let paths = prepare_output(&out, false).unwrap();
        assert!(!paths.partial_path.exists());
        assert_eq!(paths.final_path, out);
    }

    #[test]
    fn commit_renames_partial_to_final() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.7z");
        let paths = prepare_output(&out, false).unwrap();
        fs::write(&paths.partial_path, b"archive-bytes").unwrap();
        commit_output(&paths).unwrap();
        assert!(paths.final_path.exists());
        assert!(!paths.partial_path.exists());
        assert_eq!(fs::read(&paths.final_path).unwrap(), b"archive-bytes");
    }

    #[test]
    fn cleanup_partial_idempotent() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("out.7z");
        let paths = OutputPaths::new(&out);
        fs::write(&paths.partial_path, b"x").unwrap();
        cleanup_partial(&paths);
        assert!(!paths.partial_path.exists());
        cleanup_partial(&paths); // no error
    }

    #[test]
    fn output_exists_helper() {
        let dir = tempdir().unwrap();
        let out = dir.path().join("x.7z");
        assert!(!output_exists(&out));
        fs::write(&out, b"1").unwrap();
        assert!(output_exists(&out));
    }
}
