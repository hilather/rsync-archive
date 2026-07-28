//! Directory size budgets: cap selected bytes under a directory (newest-first).
//!
//! After normal include/exclude selection, files under a budgeted archive-relative
//! directory are sorted by mtime descending (then archive_name ascending). Files
//! are kept while `running_sum + size <= limit`; further files are budget-skips.
//!
//! Nested budgets: longest matching directory prefix wins.

use super::pathnorm::normalize_archive_str;
use super::walk::{SelectedEntry, SelectionStats};
use crate::error::{Error, Result};
use crate::util::parse_byte_size;
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::warn;

/// One `--dir-max-size PATH=SIZE` budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirBudget {
    /// Archive-relative directory prefix (no trailing `/`).
    pub prefix: String,
    /// Max total selected bytes under this directory (recursive).
    pub limit: u64,
}

/// Parse a single `--dir-max-size PATH=SIZE` argument.
///
/// `PATH` is an archive-relative directory prefix (trailing `/` optional).
/// `SIZE` uses the same syntax as [`parse_byte_size`] (`100M`, `1G`, …).
pub fn parse_dir_max_size_arg(s: &str) -> Result<DirBudget> {
    let s = s.trim();
    let (path, size) = s.split_once('=').ok_or_else(|| {
        Error::Message(format!(
            "invalid --dir-max-size '{s}': expected PATH=SIZE (e.g. logs/=100M)"
        ))
    })?;
    let path = path.trim();
    let size = size.trim();
    if path.is_empty() {
        return Err(Error::Message(
            "invalid --dir-max-size: empty PATH before '='".into(),
        ));
    }
    if size.is_empty() {
        return Err(Error::Message(format!(
            "invalid --dir-max-size '{s}': empty SIZE after '='"
        )));
    }
    let prefix = normalize_budget_prefix(path)?;
    let limit = parse_byte_size(size)?;
    Ok(DirBudget { prefix, limit })
}

/// Normalize archive-relative directory prefix (strip trailing `/`, no `..`).
pub fn normalize_budget_prefix(path: &str) -> Result<String> {
    let trimmed = path.trim().trim_end_matches(['/', '\\']);
    if trimmed.is_empty() {
        return Err(Error::Message(
            "invalid --dir-max-size PATH: empty after stripping trailing slash".into(),
        ));
    }
    normalize_archive_str(trimmed)
}

/// Parse many CLI args into budgets; duplicate prefixes error.
pub fn parse_dir_budgets(args: &[String]) -> Result<Vec<DirBudget>> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let b = parse_dir_max_size_arg(a)?;
        if out.iter().any(|x: &DirBudget| x.prefix == b.prefix) {
            return Err(Error::Message(format!(
                "duplicate --dir-max-size for directory '{}'",
                b.prefix
            )));
        }
        out.push(b);
    }
    Ok(out)
}

/// Whether `archive_name` is a regular file under directory prefix `prefix`.
///
/// Matches recursive children (`prefix/…`) only, not a file whose name equals `prefix`.
pub fn is_under_budget_dir(archive_name: &str, prefix: &str) -> bool {
    archive_name.len() > prefix.len()
        && archive_name.as_bytes().get(prefix.len()) == Some(&b'/')
        && archive_name.starts_with(prefix)
}

/// Index of the budget with the longest matching prefix, if any.
fn longest_budget_index(archive_name: &str, budgets: &[DirBudget]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (index, prefix_len)
    for (i, b) in budgets.iter().enumerate() {
        if is_under_budget_dir(archive_name, &b.prefix) {
            let len = b.prefix.len();
            if best.map(|(_, l)| len > l).unwrap_or(true) {
                best = Some((i, len));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn file_mtime_secs(path: &std::path::Path) -> u64 {
    std::fs::symlink_metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t: SystemTime| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Apply directory size budgets to a post-filter selection list.
///
/// Files not under any budgeted directory pass through. Per budget group, newest
/// mtime first (tie-break: archive_name ascending); keep while sum + size ≤ limit.
/// Preserves relative order of kept entries from the input list.
pub fn apply_dir_budgets(
    entries: Vec<SelectedEntry>,
    budgets: &[DirBudget],
    stats: &mut SelectionStats,
) -> Result<Vec<SelectedEntry>> {
    if budgets.is_empty() || entries.is_empty() {
        stats.selected = entries.len() as u64;
        return Ok(entries);
    }

    // Assign each entry to a budget group (or none).
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); budgets.len()];
    let mut unbudgeted: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        match longest_budget_index(&e.archive_name, budgets) {
            Some(g) => groups[g].push(i),
            None => unbudgeted.push(i),
        }
    }

    let mut keep = vec![false; entries.len()];
    for i in unbudgeted {
        keep[i] = true;
    }

    for (g, indices) in groups.iter().enumerate() {
        if indices.is_empty() {
            continue;
        }
        let budget = &budgets[g];
        // Sort candidates: mtime desc, archive_name asc.
        let mut ranked: Vec<(u64, &str, usize)> = indices
            .iter()
            .map(|&i| {
                let e = &entries[i];
                let mtime = file_mtime_secs(&e.abs_path);
                (mtime, e.archive_name.as_str(), i)
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0) // mtime desc
                .then_with(|| a.1.cmp(b.1)) // name asc
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut sum = 0u64;
        for &(mtime, _name, i) in &ranked {
            let e = &entries[i];
            let next = sum.saturating_add(e.size);
            if next <= budget.limit {
                keep[i] = true;
                sum = next;
            } else {
                stats.skipped_dir_budget += 1;
                warn!(
                    path = %e.archive_name,
                    abs = %e.abs_path.display(),
                    size = e.size,
                    mtime_unix = mtime,
                    budget_dir = %budget.prefix,
                    budget_limit = budget.limit,
                    running_sum = sum,
                    "skipped by directory size budget"
                );
            }
        }
    }

    let out: Vec<SelectedEntry> = entries
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| if keep[i] { Some(e) } else { None })
        .collect();
    stats.selected = out.len() as u64;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn entry(dir: &std::path::Path, rel: &str, data: &[u8]) -> SelectedEntry {
        let abs = dir.join(rel);
        if let Some(p) = abs.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&abs, data).unwrap();
        SelectedEntry {
            abs_path: abs,
            archive_name: rel.replace('\\', "/"),
            size: data.len() as u64,
        }
    }

    fn set_mtime(path: &std::path::Path, secs: i64) {
        let ft = filetime::FileTime::from_unix_time(secs, 0);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    #[test]
    fn parse_path_eq_size() {
        let b = parse_dir_max_size_arg("logs/=100M").unwrap();
        assert_eq!(b.prefix, "logs");
        assert_eq!(b.limit, 100 * 1024 * 1024);

        let b = parse_dir_max_size_arg("cache=50K").unwrap();
        assert_eq!(b.prefix, "cache");
        assert_eq!(b.limit, 50 * 1024);
    }

    #[test]
    fn parse_rejects_bad() {
        assert!(parse_dir_max_size_arg("noscale").is_err());
        assert!(parse_dir_max_size_arg("=100M").is_err());
        assert!(parse_dir_max_size_arg("logs/=").is_err());
        assert!(parse_dir_max_size_arg("../x=1").is_err());
    }

    #[test]
    fn under_dir_match() {
        assert!(is_under_budget_dir("logs/a.txt", "logs"));
        assert!(is_under_budget_dir("logs/old/x", "logs"));
        assert!(!is_under_budget_dir("logs", "logs"));
        assert!(!is_under_budget_dir("logs2/a", "logs"));
        assert!(!is_under_budget_dir("other/a", "logs"));
    }

    #[test]
    fn newest_first_budget_keeps_largest_newest() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // sizes 10, 20, 30; budget 35; newest = 30 → keep only 30 (+ unbudgeted)
        let e10 = entry(root, "logs/old.bin", &vec![0u8; 10]);
        let e20 = entry(root, "logs/mid.bin", &vec![0u8; 20]);
        let e30 = entry(root, "logs/new.bin", &vec![0u8; 30]);
        let other = entry(root, "keep/me.txt", b"ok");

        set_mtime(&e10.abs_path, 100);
        set_mtime(&e20.abs_path, 200);
        set_mtime(&e30.abs_path, 300);

        let entries = vec![e10, e20, e30, other];
        let budgets = vec![DirBudget {
            prefix: "logs".into(),
            limit: 35,
        }];
        let mut stats = SelectionStats::default();
        let kept = apply_dir_budgets(entries, &budgets, &mut stats).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // newest 30 fits; mid 20 → 50 > 35; old 10 → 40 > 35
        assert_eq!(names, vec!["logs/new.bin", "keep/me.txt"]);
        assert_eq!(stats.skipped_dir_budget, 2);
        assert_eq!(stats.selected, 2);
    }

    #[test]
    fn newest_first_packs_two_smaller() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let e10 = entry(root, "logs/old.bin", &vec![0u8; 10]);
        let e20 = entry(root, "logs/mid.bin", &vec![0u8; 20]);
        let e30 = entry(root, "logs/big.bin", &vec![0u8; 30]);
        // mid newest, then old, then big oldest
        set_mtime(&e10.abs_path, 200);
        set_mtime(&e20.abs_path, 300);
        set_mtime(&e30.abs_path, 50);

        let mut stats = SelectionStats::default();
        let kept = apply_dir_budgets(
            vec![e10, e20, e30],
            &[DirBudget {
                prefix: "logs".into(),
                limit: 35,
            }],
            &mut stats,
        )
        .unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // mid(20)+old(10)=30; big(30) skipped (would be 60)
        assert_eq!(names, vec!["logs/old.bin", "logs/mid.bin"]);
        assert_eq!(stats.skipped_dir_budget, 1);
    }

    #[test]
    fn nested_longest_prefix() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        // outer budget 1000, inner 15
        let a = entry(root, "logs/a.bin", &vec![0u8; 10]);
        let b = entry(root, "logs/nested/b.bin", &vec![0u8; 10]);
        let c = entry(root, "logs/nested/c.bin", &vec![0u8; 10]);
        set_mtime(&a.abs_path, 100);
        set_mtime(&b.abs_path, 300);
        set_mtime(&c.abs_path, 200);

        let budgets = vec![
            DirBudget {
                prefix: "logs".into(),
                limit: 1000,
            },
            DirBudget {
                prefix: "logs/nested".into(),
                limit: 15,
            },
        ];
        let mut stats = SelectionStats::default();
        let kept = apply_dir_budgets(vec![a, b, c], &budgets, &mut stats).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // a under outer (always kept within 1000)
        // nested: b(10)+c would be 20 > 15 → keep newest b only
        assert_eq!(names, vec!["logs/a.bin", "logs/nested/b.bin"]);
        assert_eq!(stats.skipped_dir_budget, 1);
    }

    #[test]
    fn name_tiebreak_same_mtime() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let b = entry(root, "d/b", &vec![0u8; 5]);
        let a = entry(root, "d/a", &vec![0u8; 5]);
        set_mtime(&a.abs_path, 100);
        set_mtime(&b.abs_path, 100);
        let mut stats = SelectionStats::default();
        let kept = apply_dir_budgets(
            vec![b, a],
            &[DirBudget {
                prefix: "d".into(),
                limit: 5,
            }],
            &mut stats,
        )
        .unwrap();
        // same mtime → archive_name asc → "d/a" first
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].archive_name, "d/a");
        assert_eq!(stats.skipped_dir_budget, 1);
    }

    #[test]
    fn no_budgets_noop() {
        let e = SelectedEntry {
            abs_path: PathBuf::from("/x"),
            archive_name: "x".into(),
            size: 1,
        };
        let mut stats = SelectionStats::default();
        let kept = apply_dir_budgets(vec![e], &[], &mut stats).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(stats.skipped_dir_budget, 0);
    }
}
