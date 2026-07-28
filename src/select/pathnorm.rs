//! Path normalization for archive member names and filter paths.
//!
//! All archive-relative paths use `/` separators, no leading `/`, and reject `..`.

use crate::error::{Error, Result};
use std::path::{Component, Path};

/// Normalize a path for use as an archive member or keep-path name.
///
/// Rules (design `normalize_keep`):
/// 1. Path must be valid UTF-8 (error otherwise).
/// 2. `\` → `/`; strip leading `/`.
/// 3. Collapse empty segments; strip `.` segments.
/// 4. **Reject** if any `..` segment remains.
/// 5. Result must be non-empty.
pub fn normalize_archive_path(path: &Path) -> Result<String> {
    let s = path_to_utf8(path)?;
    normalize_archive_str(&s)
}

/// Same as [`normalize_archive_path`] but from a string (may contain `\`).
pub fn normalize_archive_str(s: &str) -> Result<String> {
    let s = s.replace('\\', "/");
    let mut out: Vec<&str> = Vec::new();
    for part in s.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                return Err(Error::PathTraversal(format!(
                    "path contains '..': {s}"
                )));
            }
            other => out.push(other),
        }
    }
    if out.is_empty() {
        return Err(Error::InvalidMemberName(
            "normalized path is empty".into(),
        ));
    }
    Ok(out.join("/"))
}

/// Normalize an optional embed/create prefix: relative, no `..`, trailing `/`.
///
/// Empty input returns empty string (no prefix). Non-empty results end with `/`.
pub fn normalize_prefix(prefix: &str) -> Result<String> {
    let trimmed = prefix.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    // Reject absolute Unix or Windows-style roots before strip.
    if trimmed.starts_with('/') || trimmed.starts_with('\\') {
        return Err(Error::InvalidMemberName(format!(
            "prefix must be relative: {prefix}"
        )));
    }
    if trimmed.len() >= 2 && trimmed.as_bytes()[1] == b':' {
        return Err(Error::InvalidMemberName(format!(
            "prefix must be relative: {prefix}"
        )));
    }
    let mut n = normalize_archive_str(trimmed)?;
    if !n.ends_with('/') {
        n.push('/');
    }
    Ok(n)
}

/// Join prefix (may be empty or end with `/`) and base member name.
pub fn join_archive_name(prefix: &str, base: &str) -> Result<String> {
    if base.is_empty() || base == "." || base == ".." {
        return Err(Error::InvalidMemberName(format!(
            "invalid base name: {base:?}"
        )));
    }
    // Reject traversal in base even if already "normalized".
    if base.split('/').any(|p| p == "..") {
        return Err(Error::PathTraversal(format!(
            "member base contains '..': {base}"
        )));
    }
    if prefix.is_empty() {
        return Ok(base.to_string());
    }
    let p = if prefix.ends_with('/') {
        prefix.to_string()
    } else {
        format!("{prefix}/")
    };
    let full = format!("{p}{base}");
    normalize_archive_str(&full)
}

/// Basename as UTF-8 for flatten naming. Rejects `.` / `..` / empty.
pub fn basename_utf8(path: &Path) -> Result<String> {
    let name = path
        .file_name()
        .ok_or_else(|| Error::InvalidMemberName(format!(
            "path has no basename: {}",
            path.display()
        )))?
        .to_str()
        .ok_or_else(|| Error::InvalidUtf8Path(path.display().to_string()))?;
    if name.is_empty() || name == "." || name == ".." {
        return Err(Error::InvalidMemberName(format!(
            "invalid basename: {name:?}"
        )));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(Error::InvalidMemberName(format!(
            "basename contains separator: {name}"
        )));
    }
    Ok(name.to_string())
}

/// Validate a final archive member name (non-empty, no NUL, no `..`, no leading `/`).
pub fn validate_member_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(Error::InvalidMemberName("empty member name".into()));
    }
    if name.contains('\0') {
        return Err(Error::InvalidMemberName(
            "member name contains NUL".into(),
        ));
    }
    if name.starts_with('/') {
        return Err(Error::InvalidMemberName(format!(
            "member name must not start with /: {name}"
        )));
    }
    if name.split('/').any(|p| p == ".." || p.is_empty()) {
        return Err(Error::PathTraversal(format!(
            "invalid member name segments: {name}"
        )));
    }
    Ok(())
}

fn path_to_utf8(path: &Path) -> Result<String> {
    // Prefer lossless UTF-8; also accept OsStr via to_str.
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::InvalidUtf8Path(path.display().to_string()))
}

/// Whether a path string (as given by the user) ends with a path separator.
pub fn has_trailing_slash(s: &str) -> bool {
    s.ends_with('/') || s.ends_with('\\')
}

/// True if any component is `ParentDir`.
pub fn contains_parent_dir(path: &Path) -> bool {
    path.components().any(|c| matches!(c, Component::ParentDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn normalize_simple() {
        assert_eq!(normalize_archive_str("a/b").unwrap(), "a/b");
        assert_eq!(normalize_archive_str("./a/./b/").unwrap(), "a/b");
        assert_eq!(normalize_archive_str("a//b").unwrap(), "a/b");
        assert_eq!(normalize_archive_str(r"a\b").unwrap(), "a/b");
    }

    #[test]
    fn normalize_strips_leading_slash() {
        assert_eq!(normalize_archive_str("/a/b").unwrap(), "a/b");
        assert_eq!(normalize_archive_str("///a").unwrap(), "a");
    }

    #[test]
    fn normalize_rejects_dotdot() {
        assert!(matches!(
            normalize_archive_str("a/../b"),
            Err(Error::PathTraversal(_))
        ));
        assert!(matches!(
            normalize_archive_str(".."),
            Err(Error::PathTraversal(_))
        ));
        assert!(matches!(
            normalize_archive_str("../x"),
            Err(Error::PathTraversal(_))
        ));
    }

    #[test]
    fn normalize_empty_errors() {
        assert!(matches!(
            normalize_archive_str(""),
            Err(Error::InvalidMemberName(_))
        ));
        assert!(matches!(
            normalize_archive_str("./"),
            Err(Error::InvalidMemberName(_))
        ));
        assert!(matches!(
            normalize_archive_str("/"),
            Err(Error::InvalidMemberName(_))
        ));
    }

    #[test]
    fn normalize_path_buf() {
        let p = PathBuf::from("dir/file.txt");
        assert_eq!(normalize_archive_path(&p).unwrap(), "dir/file.txt");
    }

    #[test]
    fn prefix_adds_slash() {
        assert_eq!(normalize_prefix("packs").unwrap(), "packs/");
        assert_eq!(normalize_prefix("packs/").unwrap(), "packs/");
        assert_eq!(normalize_prefix("").unwrap(), "");
        assert_eq!(normalize_prefix("  ").unwrap(), "");
    }

    #[test]
    fn prefix_rejects_absolute_and_dotdot() {
        assert!(normalize_prefix("/abs").is_err());
        assert!(normalize_prefix("a/../b").is_err());
    }

    #[test]
    fn join_prefix_and_base() {
        assert_eq!(join_archive_name("packs/", "a.7z").unwrap(), "packs/a.7z");
        assert_eq!(join_archive_name("packs", "a.7z").unwrap(), "packs/a.7z");
        assert_eq!(join_archive_name("", "a.7z").unwrap(), "a.7z");
    }

    #[test]
    fn basename_ok() {
        assert_eq!(basename_utf8(Path::new("dir/x.7z")).unwrap(), "x.7z");
        assert_eq!(basename_utf8(Path::new("x.7z")).unwrap(), "x.7z");
    }

    #[test]
    fn validate_member() {
        assert!(validate_member_name("a/b").is_ok());
        assert!(validate_member_name("").is_err());
        assert!(validate_member_name("/a").is_err());
        assert!(validate_member_name("a/../b").is_err());
    }

    #[test]
    fn trailing_slash_detect() {
        assert!(has_trailing_slash("photos/"));
        assert!(has_trailing_slash(r"photos\"));
        assert!(!has_trailing_slash("photos"));
    }

    #[test]
    fn contains_parent() {
        assert!(contains_parent_dir(Path::new("a/../b")));
        assert!(!contains_parent_dir(Path::new("a/b")));
    }
}
