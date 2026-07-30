//! Restriction list files (rsync-like path/pattern + extra fields).
//!
//! Distinct from include/exclude filter files:
//!
//! - **`--file-size-from`**: per matching archive path, max size only.
//!   Non-matching master-list entries ignore the list.
//! - **`--dir-max-size-from`**: directory budgets (`max=SIZE`) and optional
//!   file-count (`files=N`) for listed prefixes only.
//!
//! Line format (blank / `#` comments skipped):
//!
//! ```text
//! # file-size-from
//! **/*.log          max=100M
//! var/log/app.log   max=10M
//!
//! # dir-max-size-from
//! logs/             max=500M
//! logs/             files=100
//! cache/            max=1G files=50
//! logs/=100M        # legacy PATH=SIZE
//! ```

use super::dir_budget::{
    normalize_budget_prefix, DirBudget, DirFileLimit, RestrictionFile, RestrictionReport,
};
use super::from_file::read_capped_lines;
use super::matcher::path_matches_rule;
use super::rules::{parse_rule, Rule, RuleAction};
use super::walk::{SelectedEntry, SelectionStats};
use crate::error::{Error, Result};
use crate::util::parse_byte_size;
use std::path::Path;
use tracing::warn;

/// One file-size restriction: rsync-style pattern + max size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSizeRule {
    /// Pattern used for matching (include-style; first match wins).
    pub rule: Rule,
    pub max_size: u64,
}

/// Parse a single `--file-size-from` line: `PATTERN max=SIZE`.
///
/// Token order is flexible (`max=SIZE` may appear before or after the pattern).
/// Exactly one `max=` field is required. No `min=` (not supported).
/// Invalid lines are soft-skipped by loaders (see [`load_file_size_from`]).
pub fn parse_file_size_line(line: &str) -> Result<FileSizeRule> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Err(Error::Message(
            "internal: empty/comment line should be skipped before parse".into(),
        ));
    }

    let mut max_size: Option<u64> = None;
    let mut pattern_parts: Vec<&str> = Vec::new();

    for tok in line.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("max=") {
            if rest.is_empty() {
                return Err(Error::Message(format!(
                    "invalid file-size line '{line}': empty max="
                )));
            }
            if max_size.is_some() {
                return Err(Error::Message(format!(
                    "invalid file-size line '{line}': duplicate max="
                )));
            }
            max_size = Some(parse_byte_size(rest).map_err(|e| {
                Error::Message(format!("invalid file-size line '{line}': {e}"))
            })?);
        } else if tok.starts_with("min=") {
            return Err(Error::Message(format!(
                "invalid file-size line '{line}': min= is not supported (max= only)"
            )));
        } else if tok.contains('=') && !tok.starts_with("max=") {
            return Err(Error::Message(format!(
                "invalid file-size line '{line}': unknown field '{tok}' (expected max=SIZE)"
            )));
        } else {
            pattern_parts.push(tok);
        }
    }

    let max_size = max_size.ok_or_else(|| {
        Error::Message(format!(
            "invalid file-size line '{line}': need max=SIZE (e.g. '**/*.log max=100M')"
        ))
    })?;
    if pattern_parts.is_empty() {
        return Err(Error::Message(format!(
            "invalid file-size line '{line}': missing path/pattern"
        )));
    }
    let pat = pattern_parts.join(" ");
    let rule = parse_rule(RuleAction::Include, &pat)?;
    Ok(FileSizeRule { rule, max_size })
}

/// Load `--file-size-from FILE` (ordered; first match wins at apply time).
///
/// **Invalid lines are logged and ignored** (create continues). Blank/`#` lines skip.
pub fn load_file_size_from(path: &Path) -> Result<Vec<FileSizeRule>> {
    let lines = read_capped_lines(path)?;
    let mut out = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match parse_file_size_line(trimmed) {
            Ok(rule) => out.push(rule),
            Err(e) => {
                warn!(
                    file = %path.display(),
                    line = idx + 1,
                    error = %e,
                    text = %trimmed,
                    "ignoring invalid --file-size-from line"
                );
            }
        }
    }
    Ok(out)
}

/// First matching file-size rule for an archive path, if any.
pub fn match_file_size_rule<'a>(
    rules: &'a [FileSizeRule],
    archive_name: &str,
) -> Option<&'a FileSizeRule> {
    for r in rules {
        if path_matches_rule(&r.rule, archive_name, false) {
            return Some(r);
        }
    }
    None
}

/// Apply `--file-size-from` rules: only matching paths are size-capped; others pass.
///
/// First matching rule wins. Files with `size > max` are skipped.
pub fn apply_file_size_from(
    entries: Vec<SelectedEntry>,
    rules: &[FileSizeRule],
    stats: &mut SelectionStats,
    report: &mut RestrictionReport,
) -> Result<Vec<SelectedEntry>> {
    if rules.is_empty() || entries.is_empty() {
        stats.selected = entries.len() as u64;
        return Ok(entries);
    }

    let mut kept = Vec::with_capacity(entries.len());
    for e in entries {
        if let Some(r) = match_file_size_rule(rules, &e.archive_name) {
            if e.size > r.max_size {
                stats.skipped_file_size_from += 1;
                report.skipped_file_size_from.push(RestrictionFile {
                    archive_name: e.archive_name.clone(),
                    size: e.size,
                });
                continue;
            }
        }
        kept.push(e);
    }
    stats.selected = kept.len() as u64;
    Ok(kept)
}

/// Parsed fields from one directory-restriction list line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirRestrictLine {
    pub prefix: String,
    pub max_size: Option<u64>,
    pub max_files: Option<u64>,
}

/// Parse a directory restriction line.
///
/// Accepts:
/// - Extended: `logs/ max=500M`, `logs/ files=100`, `cache/ max=1G files=50`
/// - Legacy size: `logs/=100M` (PATH=SIZE)
///
/// At least one of `max=` / `files=` (or legacy `=SIZE`) is required.
pub fn parse_dir_restrict_line(line: &str) -> Result<DirRestrictLine> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Err(Error::Message(
            "internal: empty/comment line should be skipped before parse".into(),
        ));
    }

    // Legacy PATH=SIZE when no whitespace and no max=/files= tokens.
    if !line.contains(char::is_whitespace)
        && !line.contains("max=")
        && !line.contains("files=")
        && line.contains('=')
    {
        let (path, size) = line.split_once('=').unwrap();
        let prefix = normalize_budget_prefix(path)?;
        let limit = parse_byte_size(size.trim()).map_err(|e| {
            Error::Message(format!(
                "invalid dir restriction '{line}': {e} (use PATH=SIZE or 'PATH/ max=SIZE')"
            ))
        })?;
        return Ok(DirRestrictLine {
            prefix,
            max_size: Some(limit),
            max_files: None,
        });
    }

    let mut max_size: Option<u64> = None;
    let mut max_files: Option<u64> = None;
    let mut path_parts: Vec<&str> = Vec::new();

    for tok in line.split_whitespace() {
        if let Some(rest) = tok.strip_prefix("max=") {
            if rest.is_empty() {
                return Err(Error::Message(format!(
                    "invalid dir restriction '{line}': empty max="
                )));
            }
            if max_size.is_some() {
                return Err(Error::Message(format!(
                    "invalid dir restriction '{line}': duplicate max="
                )));
            }
            max_size = Some(parse_byte_size(rest).map_err(|e| {
                Error::Message(format!("invalid dir restriction '{line}': {e}"))
            })?);
        } else if let Some(rest) = tok.strip_prefix("files=") {
            if rest.is_empty() {
                return Err(Error::Message(format!(
                    "invalid dir restriction '{line}': empty files="
                )));
            }
            if max_files.is_some() {
                return Err(Error::Message(format!(
                    "invalid dir restriction '{line}': duplicate files="
                )));
            }
            let n: u64 = rest.parse().map_err(|_| {
                Error::Message(format!(
                    "invalid dir restriction '{line}': files= must be a non-negative integer"
                ))
            })?;
            max_files = Some(n);
        } else if tok.starts_with("min=") {
            return Err(Error::Message(format!(
                "invalid dir restriction '{line}': min= is not supported"
            )));
        } else if tok.contains('=') {
            return Err(Error::Message(format!(
                "invalid dir restriction '{line}': unknown field '{tok}'"
            )));
        } else {
            path_parts.push(tok);
        }
    }

    if path_parts.is_empty() {
        return Err(Error::Message(format!(
            "invalid dir restriction '{line}': missing directory path"
        )));
    }
    if max_size.is_none() && max_files.is_none() {
        return Err(Error::Message(format!(
            "invalid dir restriction '{line}': need max=SIZE and/or files=N"
        )));
    }
    let path = path_parts.join(" ");
    let prefix = normalize_budget_prefix(&path)?;
    Ok(DirRestrictLine {
        prefix,
        max_size,
        max_files,
    })
}

/// Load `--dir-max-size-from FILE` into size budgets and optional file-count limits.
///
/// **Invalid lines are logged and ignored.** Duplicate prefixes against existing
/// CLI/file entries are also logged and ignored (line skipped).
pub fn load_dir_max_size_from(
    path: &Path,
    budgets: &mut Vec<DirBudget>,
    file_limits: &mut Vec<DirFileLimit>,
) -> Result<()> {
    let lines = read_capped_lines(path)?;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let parsed = match parse_dir_restrict_line(trimmed) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    file = %path.display(),
                    line = idx + 1,
                    error = %e,
                    text = %trimmed,
                    "ignoring invalid --dir-max-size-from line"
                );
                continue;
            }
        };
        if let Some(limit) = parsed.max_size {
            if budgets.iter().any(|b| b.prefix == parsed.prefix) {
                warn!(
                    file = %path.display(),
                    line = idx + 1,
                    prefix = %parsed.prefix,
                    "ignoring duplicate dir-max-size prefix"
                );
            } else {
                budgets.push(DirBudget {
                    prefix: parsed.prefix.clone(),
                    limit,
                });
            }
        }
        if let Some(max_count) = parsed.max_files {
            if file_limits.iter().any(|f| f.prefix == parsed.prefix) {
                warn!(
                    file = %path.display(),
                    line = idx + 1,
                    prefix = %parsed.prefix,
                    "ignoring duplicate dir-max-files prefix"
                );
            } else {
                file_limits.push(DirFileLimit {
                    prefix: parsed.prefix,
                    max_count,
                });
            }
        }
    }
    Ok(())
}

/// Load `--dir-max-files-from` accepting legacy `PATH=N` **or** `PATH/ files=N`.
///
/// **Invalid lines are logged and ignored.**
pub fn load_dir_max_files_from_ext(
    path: &Path,
    existing: &mut Vec<DirFileLimit>,
) -> Result<()> {
    let lines = read_capped_lines(path)?;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let lim = match parse_dir_file_limit_line(trimmed) {
            Ok(l) => l,
            Err(e) => {
                warn!(
                    file = %path.display(),
                    line = idx + 1,
                    error = %e,
                    text = %trimmed,
                    "ignoring invalid --dir-max-files-from line"
                );
                continue;
            }
        };
        if existing.iter().any(|x| x.prefix == lim.prefix) {
            warn!(
                file = %path.display(),
                line = idx + 1,
                prefix = %lim.prefix,
                "ignoring duplicate --dir-max-files prefix"
            );
            continue;
        }
        existing.push(lim);
    }
    Ok(())
}

/// Parse `PATH=N` or `PATH/ files=N` (files-only; rejects max= on this helper).
fn parse_dir_file_limit_line(line: &str) -> Result<DirFileLimit> {
    let line = line.trim();
    // Extended form with files=
    if line.contains("files=") || line.contains("max=") {
        let p = parse_dir_restrict_line(line)?;
        let max_count = p.max_files.ok_or_else(|| {
            Error::Message(format!(
                "invalid dir-max-files line '{line}': need files=N (max= belongs in --dir-max-size-from)"
            ))
        })?;
        if p.max_size.is_some() {
            return Err(Error::Message(format!(
                "invalid dir-max-files line '{line}': use --dir-max-size-from for max=SIZE (or combine max= and files= there)"
            )));
        }
        return Ok(DirFileLimit {
            prefix: p.prefix,
            max_count,
        });
    }
    // Legacy PATH=N
    let (path, count) = line.split_once('=').ok_or_else(|| {
        Error::Message(format!(
            "invalid dir-max-files '{line}': expected PATH=N or 'PATH/ files=N'"
        ))
    })?;
    let prefix = normalize_budget_prefix(path)?;
    let max_count: u64 = count.trim().parse().map_err(|_| {
        Error::Message(format!(
            "invalid dir-max-files '{line}': N must be a non-negative integer"
        ))
    })?;
    Ok(DirFileLimit { prefix, max_count })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn parse_file_size_pattern_then_max() {
        let r = parse_file_size_line("**/*.log max=100M").unwrap();
        assert_eq!(r.max_size, 100 * 1024 * 1024);
        assert!(path_matches_rule(&r.rule, "var/a.log", false));
        assert!(!path_matches_rule(&r.rule, "var/a.txt", false));
    }

    #[test]
    fn parse_file_size_max_first() {
        let r = parse_file_size_line("max=10K core").unwrap();
        assert_eq!(r.max_size, 10 * 1024);
        assert!(path_matches_rule(&r.rule, "core", false));
        assert!(path_matches_rule(&r.rule, "sub/core", false)); // basename mode
    }

    #[test]
    fn parse_file_size_rejects_min() {
        assert!(parse_file_size_line("a max=1M min=1K").is_err());
    }

    #[test]
    fn load_file_size_from_ignores_invalid_lines() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("sizes.txt");
        fs::write(
            &f,
            "# comment\n\
             **/*.log max=10\n\
             max=8192P-1\n\
             not-a-valid-line\n\
             core max=1K\n",
        )
        .unwrap();
        let rules = load_file_size_from(&f).expect("load must not fail on bad lines");
        assert_eq!(rules.len(), 2);
        assert!(path_matches_rule(&rules[0].rule, "a.log", false));
        assert_eq!(rules[0].max_size, 10);
        assert!(path_matches_rule(&rules[1].rule, "core", false));
        assert_eq!(rules[1].max_size, 1024);
    }

    #[test]
    fn file_size_apply_only_matching() {
        let rules = vec![parse_file_size_line("*.log max=10").unwrap()];
        let dir = tempdir().unwrap();
        let small = dir.path().join("a.log");
        let big = dir.path().join("b.log");
        let other = dir.path().join("c.txt");
        fs::write(&small, vec![0u8; 5]).unwrap();
        fs::write(&big, vec![0u8; 20]).unwrap();
        fs::write(&other, vec![0u8; 100]).unwrap();
        let entries = vec![
            entry(&small, "a.log", 5),
            entry(&big, "b.log", 20),
            entry(&other, "c.txt", 100),
        ];
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_file_size_from(entries, &rules, &mut stats, &mut report).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        assert_eq!(names, vec!["a.log", "c.txt"]);
        assert_eq!(stats.skipped_file_size_from, 1);
        assert_eq!(report.skipped_file_size_from.len(), 1);
    }

    #[test]
    fn parse_dir_extended_and_legacy() {
        let a = parse_dir_restrict_line("logs/ max=500M files=10").unwrap();
        assert_eq!(a.prefix, "logs");
        assert_eq!(a.max_size, Some(500 * 1024 * 1024));
        assert_eq!(a.max_files, Some(10));

        let b = parse_dir_restrict_line("cache/=50M").unwrap();
        assert_eq!(b.prefix, "cache");
        assert_eq!(b.max_size, Some(50 * 1024 * 1024));
        assert_eq!(b.max_files, None);

        let c = parse_dir_restrict_line("tmp/ files=3").unwrap();
        assert_eq!(c.prefix, "tmp");
        assert_eq!(c.max_size, None);
        assert_eq!(c.max_files, Some(3));
    }

    #[test]
    fn load_dir_max_size_from_merges() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("d.txt");
        fs::write(
            &f,
            "# budgets\nlogs/ max=100M files=5\ncache/=10M\n",
        )
        .unwrap();
        let mut budgets = Vec::new();
        let mut files = Vec::new();
        load_dir_max_size_from(&f, &mut budgets, &mut files).unwrap();
        assert_eq!(budgets.len(), 2);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].prefix, "logs");
        assert_eq!(files[0].max_count, 5);
    }

    fn entry(abs: &Path, name: &str, size: u64) -> SelectedEntry {
        SelectedEntry {
            abs_path: abs.to_path_buf(),
            archive_name: name.into(),
            size,
            mtime_unix: Some(1),
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: super::super::walk::MemberKind::File,
        }
    }
}
