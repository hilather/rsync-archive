//! Load include-from / exclude-from / filter list files with size and line caps.
//!
//! Caps (design / security): max **10 MiB** or **1_000_000 lines** per file →
//! [`Error::FilterFileTooLarge`].

use super::rules::{RuleAction, RuleSet};
use crate::error::{Error, Result};
use std::fs::File;
use std::io::Read;
use std::path::Path;

/// Maximum filter/list file size in bytes (10 MiB).
pub const MAX_FILTER_FILE_BYTES: u64 = 10 * 1024 * 1024;

/// Maximum number of lines per filter/list file.
pub const MAX_FILTER_FILE_LINES: usize = 1_000_000;

/// Read a text list file with size/line caps. Returns non-comment content lines
/// still raw (caller decides how to interpret). Blank and `#` comment lines are
/// **included** in the returned vec as empty-after-trim skips are done by
/// RuleSet push helpers; this function returns every physical line (trimmed end
/// newline only) for maximum flexibility.
///
/// Prefer [`load_exclude_from`] / [`load_include_from`] / [`load_filter_from`]
/// for rule loading. This helper is also intended for future `--files-from`.
pub fn read_capped_lines(path: &Path) -> Result<Vec<String>> {
    let meta = std::fs::metadata(path).map_err(|e| {
        Error::Selection(format!("stat filter file {}: {e}", path.display()))
    })?;
    let len = meta.len();
    if len > MAX_FILTER_FILE_BYTES {
        return Err(Error::FilterFileTooLarge {
            path: path.to_path_buf(),
            detail: format!("{len} bytes exceeds {MAX_FILTER_FILE_BYTES} byte limit"),
        });
    }

    // Read fully under the size cap (metadata can race; also cap while reading).
    let mut file = File::open(path).map_err(|e| {
        Error::Selection(format!("open filter file {}: {e}", path.display()))
    })?;
    let mut buf = Vec::new();
    file.by_ref()
        .take(MAX_FILTER_FILE_BYTES + 1)
        .read_to_end(&mut buf)
        .map_err(|e| Error::Selection(format!("read filter file {}: {e}", path.display())))?;
    if buf.len() as u64 > MAX_FILTER_FILE_BYTES {
        return Err(Error::FilterFileTooLarge {
            path: path.to_path_buf(),
            detail: format!("exceeds {MAX_FILTER_FILE_BYTES} byte limit"),
        });
    }

    let text = String::from_utf8(buf).map_err(|_| {
        Error::Selection(format!(
            "filter file is not valid UTF-8: {}",
            path.display()
        ))
    })?;

    let mut lines = Vec::new();
    for line in text.lines() {
        if lines.len() >= MAX_FILTER_FILE_LINES {
            return Err(Error::FilterFileTooLarge {
                path: path.to_path_buf(),
                detail: format!("exceeds {MAX_FILTER_FILE_LINES} line limit"),
            });
        }
        lines.push(line.to_string());
    }
    // If file ends with a trailing newline, `lines()` does not add an extra empty
    // line beyond content; that's fine.
    // Count: if there are more than MAX lines we already erred. Also handle the
    // case where lines() yields exactly the limit — OK.
    Ok(lines)
}

/// Load `--exclude-from FILE`: bare lines are excludes; `+`/`-` prefixes honored.
pub fn load_exclude_from(rules: &mut RuleSet, path: &Path) -> Result<()> {
    load_from_file(rules, path, RuleAction::Exclude)
}

/// Load `--include-from FILE`: bare lines are includes; `+`/`-` prefixes honored.
pub fn load_include_from(rules: &mut RuleSet, path: &Path) -> Result<()> {
    load_from_file(rules, path, RuleAction::Include)
}

/// Load a filter-rules file: each non-comment line must be `+`/`-`/`include`/`exclude`.
pub fn load_filter_from(rules: &mut RuleSet, path: &Path) -> Result<()> {
    let lines = read_capped_lines(path)?;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        rules.push_filter_line(line).map_err(|e| {
            Error::Selection(format!(
                "{}:{}: {e}",
                path.display(),
                idx + 1
            ))
        })?;
    }
    Ok(())
}

fn load_from_file(rules: &mut RuleSet, path: &Path, default: RuleAction) -> Result<()> {
    let lines = read_capped_lines(path)?;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        rules
            .push_from_file_line(line, default)
            .map_err(|e| {
                Error::Selection(format!(
                    "{}:{}: {e}",
                    path.display(),
                    idx + 1
                ))
            })?;
    }
    Ok(())
}

/// Stream-oriented line count check used when reading via BufRead without full buffer.
///
/// Currently `read_capped_lines` is the main entry; this remains for callers that
/// already have a reader and want the same limits.
#[allow(dead_code)]
pub fn read_capped_lines_from_reader<R: Read>(
    path_for_errors: &Path,
    reader: R,
    max_bytes: u64,
    max_lines: usize,
) -> Result<Vec<String>> {
    let mut limited = reader.take(max_bytes + 1);
    let mut buf = Vec::new();
    limited.read_to_end(&mut buf).map_err(|e| {
        Error::Selection(format!(
            "read filter file {}: {e}",
            path_for_errors.display()
        ))
    })?;
    if buf.len() as u64 > max_bytes {
        return Err(Error::FilterFileTooLarge {
            path: path_for_errors.to_path_buf(),
            detail: format!("exceeds {max_bytes} byte limit"),
        });
    }
    let text = String::from_utf8(buf).map_err(|_| {
        Error::Selection(format!(
            "filter file is not valid UTF-8: {}",
            path_for_errors.display()
        ))
    })?;
    let mut lines = Vec::new();
    for line in text.lines() {
        if lines.len() >= max_lines {
            return Err(Error::FilterFileTooLarge {
                path: path_for_errors.to_path_buf(),
                detail: format!("exceeds {max_lines} line limit"),
            });
        }
        lines.push(line.to_string());
    }
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::rules::RuleAction;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_temp(contents: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn exclude_from_bare_and_comments() {
        let f = write_temp("# header\n\n*.tmp\n*.log\n+ keep.tmp\n");
        let mut rs = RuleSet::new();
        load_exclude_from(&mut rs, f.path()).unwrap();
        assert_eq!(rs.len(), 3);
        assert_eq!(rs.rules()[0].action, RuleAction::Exclude);
        assert_eq!(rs.rules()[0].pattern, "*.tmp");
        assert_eq!(rs.rules()[1].pattern, "*.log");
        assert_eq!(rs.rules()[2].action, RuleAction::Include);
        assert_eq!(rs.rules()[2].pattern, "keep.tmp");
    }

    #[test]
    fn include_from_default_include() {
        let f = write_temp("*.c\n*.h\n");
        let mut rs = RuleSet::new();
        load_include_from(&mut rs, f.path()).unwrap();
        assert_eq!(rs.len(), 2);
        assert!(rs.rules().iter().all(|r| r.action == RuleAction::Include));
    }

    #[test]
    fn filter_from_requires_prefix() {
        let f = write_temp("+ *.c\n- *\n");
        let mut rs = RuleSet::new();
        load_filter_from(&mut rs, f.path()).unwrap();
        assert_eq!(rs.len(), 2);

        let bad = write_temp("bare_pattern\n");
        let mut rs2 = RuleSet::new();
        assert!(load_filter_from(&mut rs2, bad.path()).is_err());
    }

    #[test]
    fn size_cap_rejects_huge_file() {
        // Simulate by writing a small file but calling reader with tiny max.
        let f = write_temp("aaaa\nbbbb\n");
        let file = File::open(f.path()).unwrap();
        let err = read_capped_lines_from_reader(f.path(), file, 3, MAX_FILTER_FILE_LINES)
            .unwrap_err();
        assert!(matches!(err, Error::FilterFileTooLarge { .. }));
    }

    #[test]
    fn line_cap_rejects() {
        let f = write_temp("a\nb\nc\n");
        let file = File::open(f.path()).unwrap();
        let err = read_capped_lines_from_reader(f.path(), file, MAX_FILTER_FILE_BYTES, 2)
            .unwrap_err();
        assert!(matches!(err, Error::FilterFileTooLarge { .. }));
    }

    #[test]
    fn read_capped_ok() {
        let f = write_temp("one\ntwo\n");
        let lines = read_capped_lines(f.path()).unwrap();
        assert_eq!(lines, vec!["one", "two"]);
    }
}
