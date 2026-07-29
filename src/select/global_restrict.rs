//! Global / log-collection post-filter restrictions.
//!
//! Applied after rsync selection (and coordinated with dir budgets):
//!
//! 1. Per-file: `--max-size`, `--min-size`, `--newer-than`
//! 2. Dir budgets / file limits (elsewhere)
//! 3. Global: `--max-total-size`, `--max-files` (newest-mtime-first)

use super::dir_budget::{
    GlobalCountCapOutcome, GlobalSizeCapOutcome, RestrictionFile, RestrictionReport,
};
use super::walk::{SelectedEntry, SelectionStats};
#[cfg(test)]
use super::walk::MemberKind;
use crate::error::{Error, Result};
use crate::util::parse_byte_size;
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-file size and age filters (applied before dir / global caps).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PerFileLimits {
    /// Skip file if `size > max_size`. `None` = off.
    pub max_size: Option<u64>,
    /// Skip file if `size < min_size`. `None` or `Some(0)` = off.
    pub min_size: Option<u64>,
    /// Keep only files with mtime ≥ `now - newer_than_secs`. `None` = off.
    pub newer_than_secs: Option<u64>,
}

impl PerFileLimits {
    pub fn is_active(&self) -> bool {
        self.max_size.is_some()
            || self.min_size.filter(|&n| n > 0).is_some()
            || self.newer_than_secs.is_some()
    }
}

/// Parse simple duration strings into seconds.
///
/// Accepts optional suffix: `d` (days), `h` (hours), `m` (minutes), `s` (seconds).
/// Bare integers are seconds. Examples: `7d`, `24h`, `30m`, `90s`, `3600`.
pub fn parse_duration_secs(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(Error::Message("empty duration string".into()));
    }
    let lower = s.to_ascii_lowercase();
    let (num_str, mult) = if let Some(n) = lower.strip_suffix('d') {
        (n, 86_400u64)
    } else if let Some(n) = lower.strip_suffix('h') {
        (n, 3_600)
    } else if let Some(n) = lower.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = lower.strip_suffix('s') {
        (n, 1)
    } else {
        (lower.as_str(), 1)
    };
    let num_str = num_str.trim();
    if num_str.is_empty() {
        return Err(Error::Message(format!(
            "missing number in duration '{s}' (e.g. 7d, 24h, 30m)"
        )));
    }
    // Integer only (simple parse).
    let n: u64 = num_str.parse().map_err(|_| {
        Error::Message(format!(
            "invalid duration '{s}': expected non-negative integer with optional d/h/m/s suffix"
        ))
    })?;
    n.checked_mul(mult).ok_or_else(|| {
        Error::Message(format!("duration too large: '{s}'"))
    })
}

/// Build [`PerFileLimits`] from optional CLI size/duration strings.
pub fn per_file_limits_from_cli(
    max_size: Option<&str>,
    min_size: Option<&str>,
    newer_than: Option<&str>,
) -> Result<PerFileLimits> {
    let max_size = match max_size {
        Some(s) => Some(parse_byte_size(s)?),
        None => None,
    };
    let min_size = match min_size {
        Some(s) => {
            let n = parse_byte_size(s)?;
            if n == 0 {
                None
            } else {
                Some(n)
            }
        }
        None => None,
    };
    let newer_than_secs = match newer_than {
        Some(s) => Some(parse_duration_secs(s)?),
        None => None,
    };
    Ok(PerFileLimits {
        max_size,
        min_size,
        newer_than_secs,
    })
}

fn entry_mtime_secs(e: &SelectedEntry) -> u64 {
    if let Some(t) = e.mtime_unix {
        return t;
    }
    std::fs::symlink_metadata(&e.abs_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t: SystemTime| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn now_unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn as_restriction(e: &SelectedEntry) -> RestrictionFile {
    RestrictionFile {
        archive_name: e.archive_name.clone(),
        size: e.size,
    }
}

/// Apply per-file max/min size and newer-than filters.
///
/// Preserves relative order of kept entries. Skips are recorded on `stats` and
/// compact lists on `report`.
pub fn apply_per_file_limits(
    entries: Vec<SelectedEntry>,
    limits: &PerFileLimits,
    stats: &mut SelectionStats,
    report: &mut RestrictionReport,
) -> Result<Vec<SelectedEntry>> {
    if !limits.is_active() || entries.is_empty() {
        stats.selected = entries.len() as u64;
        return Ok(entries);
    }

    let min_active = limits.min_size.filter(|&n| n > 0);
    let cutoff = limits.newer_than_secs.map(|d| now_unix_secs().saturating_sub(d));

    let mut kept = Vec::with_capacity(entries.len());
    for e in entries {
        if let Some(max) = limits.max_size {
            if e.size > max {
                stats.skipped_max_size += 1;
                report.skipped_max_size.push(as_restriction(&e));
                continue;
            }
        }
        if let Some(min) = min_active {
            if e.size < min {
                stats.skipped_min_size += 1;
                report.skipped_min_size.push(as_restriction(&e));
                continue;
            }
        }
        if let Some(cut) = cutoff {
            let mtime = entry_mtime_secs(&e);
            if mtime < cut {
                stats.skipped_older_than += 1;
                report.skipped_older_than.push(as_restriction(&e));
                continue;
            }
        }
        kept.push(e);
    }
    stats.selected = kept.len() as u64;
    Ok(kept)
}

/// Rank indices newest-mtime-first, then archive_name ascending.
fn rank_newest_first(entries: &[SelectedEntry]) -> Vec<usize> {
    let mut ranked: Vec<(u64, &str, usize)> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (entry_mtime_secs(e), e.archive_name.as_str(), i))
        .collect();
    ranked.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.cmp(b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    ranked.into_iter().map(|(_, _, i)| i).collect()
}

/// Global `--max-total-size`: keep newest-first while sum + size ≤ limit.
///
/// Preserves relative order of kept entries from the input list. Emits a report
/// block only when at least one file is considered (limit always set by caller).
pub fn apply_max_total_size(
    entries: Vec<SelectedEntry>,
    limit: u64,
    stats: &mut SelectionStats,
    report: &mut RestrictionReport,
) -> Result<Vec<SelectedEntry>> {
    if entries.is_empty() {
        stats.selected = 0;
        return Ok(entries);
    }

    let order = rank_newest_first(&entries);
    let mut keep = vec![false; entries.len()];
    let mut sum = 0u64;
    let mut outcome = GlobalSizeCapOutcome {
        limit,
        kept: Vec::new(),
        skipped: Vec::new(),
    };

    for i in order {
        let e = &entries[i];
        let next = sum.saturating_add(e.size);
        if next <= limit {
            keep[i] = true;
            sum = next;
            outcome.kept.push(as_restriction(e));
        } else {
            stats.skipped_max_total_size += 1;
            outcome.skipped.push(as_restriction(e));
        }
    }
    report.max_total_size = Some(outcome);

    let out: Vec<SelectedEntry> = entries
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| if keep[i] { Some(e) } else { None })
        .collect();
    stats.selected = out.len() as u64;
    Ok(out)
}

/// Global `--max-files`: keep at most `max_count` newest files.
pub fn apply_max_files(
    entries: Vec<SelectedEntry>,
    max_count: u64,
    stats: &mut SelectionStats,
    report: &mut RestrictionReport,
) -> Result<Vec<SelectedEntry>> {
    if entries.is_empty() {
        stats.selected = 0;
        return Ok(entries);
    }

    let order = rank_newest_first(&entries);
    let mut keep = vec![false; entries.len()];
    let mut kept_count = 0u64;
    let mut outcome = GlobalCountCapOutcome {
        max_count,
        kept: Vec::new(),
        skipped: Vec::new(),
    };

    for i in order {
        let e = &entries[i];
        if kept_count < max_count {
            keep[i] = true;
            kept_count += 1;
            outcome.kept.push(as_restriction(e));
        } else {
            stats.skipped_max_files += 1;
            outcome.skipped.push(as_restriction(e));
        }
    }
    report.max_files = Some(outcome);

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
        let mtime_unix = abs
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs());
        SelectedEntry {
            abs_path: abs,
            archive_name: rel.replace('\\', "/"),
            size: data.len() as u64,
            mtime_unix,
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: MemberKind::File,
        }
    }

    fn set_mtime(path: &std::path::Path, secs: i64) {
        let ft = filetime::FileTime::from_unix_time(secs, 0);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    #[test]
    fn parse_duration_suffixes() {
        assert_eq!(parse_duration_secs("7d").unwrap(), 7 * 86_400);
        assert_eq!(parse_duration_secs("24h").unwrap(), 24 * 3_600);
        assert_eq!(parse_duration_secs("30m").unwrap(), 30 * 60);
        assert_eq!(parse_duration_secs("90s").unwrap(), 90);
        assert_eq!(parse_duration_secs("3600").unwrap(), 3600);
        assert_eq!(parse_duration_secs("0").unwrap(), 0);
        assert!(parse_duration_secs("").is_err());
        assert!(parse_duration_secs("d").is_err());
        assert!(parse_duration_secs("-1h").is_err());
        assert!(parse_duration_secs("1.5h").is_err());
    }

    #[test]
    fn max_size_skips_large() {
        let dir = tempdir().unwrap();
        let small = entry(dir.path(), "small.bin", &vec![0u8; 10]);
        let big = entry(dir.path(), "big.bin", &vec![0u8; 100]);
        let limits = PerFileLimits {
            max_size: Some(50),
            ..Default::default()
        };
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_per_file_limits(vec![small, big], &limits, &mut stats, &mut report)
            .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].archive_name, "small.bin");
        assert_eq!(stats.skipped_max_size, 1);
        assert_eq!(report.skipped_max_size.len(), 1);
    }

    #[test]
    fn min_size_skips_tiny_zero_off() {
        let dir = tempdir().unwrap();
        let tiny = entry(dir.path(), "tiny.bin", b"x");
        let ok = entry(dir.path(), "ok.bin", &vec![0u8; 20]);
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let limits = PerFileLimits {
            min_size: Some(10),
            ..Default::default()
        };
        let kept = apply_per_file_limits(vec![tiny, ok], &limits, &mut stats, &mut report).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].archive_name, "ok.bin");
        assert_eq!(stats.skipped_min_size, 1);

        // 0 = off
        let mut stats2 = SelectionStats::default();
        let mut report2 = RestrictionReport::default();
        let limits0 = PerFileLimits {
            min_size: Some(0),
            ..Default::default()
        };
        let e = entry(dir.path(), "z.bin", b"z");
        let kept0 = apply_per_file_limits(vec![e], &limits0, &mut stats2, &mut report2).unwrap();
        assert_eq!(kept0.len(), 1);
        assert_eq!(stats2.skipped_min_size, 0);
    }

    #[test]
    fn newer_than_skips_old() {
        let dir = tempdir().unwrap();
        let old = entry(dir.path(), "old.bin", b"old");
        let new = entry(dir.path(), "new.bin", b"new");
        let now = now_unix_secs() as i64;
        set_mtime(&old.abs_path, now - 10_000);
        set_mtime(&new.abs_path, now - 10);
        // refresh mtime_unix after set
        let mut old = old;
        let mut new = new;
        old.mtime_unix = Some((now - 10_000) as u64);
        new.mtime_unix = Some((now - 10) as u64);

        let limits = PerFileLimits {
            newer_than_secs: Some(100), // last 100s
            ..Default::default()
        };
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_per_file_limits(vec![old, new], &limits, &mut stats, &mut report).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].archive_name, "new.bin");
        assert_eq!(stats.skipped_older_than, 1);
    }

    #[test]
    fn max_total_size_newest_first() {
        let dir = tempdir().unwrap();
        let e10 = entry(dir.path(), "old.bin", &vec![0u8; 10]);
        let e20 = entry(dir.path(), "mid.bin", &vec![0u8; 20]);
        let e30 = entry(dir.path(), "new.bin", &vec![0u8; 30]);
        set_mtime(&e10.abs_path, 100);
        set_mtime(&e20.abs_path, 200);
        set_mtime(&e30.abs_path, 300);
        let mut e10 = e10;
        let mut e20 = e20;
        let mut e30 = e30;
        e10.mtime_unix = Some(100);
        e20.mtime_unix = Some(200);
        e30.mtime_unix = Some(300);

        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept =
            apply_max_total_size(vec![e10, e20, e30], 35, &mut stats, &mut report).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // newest 30 fits; mid 20 → 50 > 35 skip; old 10 → 40 > 35 skip
        assert_eq!(names, vec!["new.bin"]);
        assert_eq!(stats.skipped_max_total_size, 2);
        let text = report.format_compact();
        assert!(text.contains("max-total-size="), "{text}");
        assert!(text.contains("skip:"), "{text}");
    }

    #[test]
    fn max_files_newest_first() {
        let dir = tempdir().unwrap();
        let a = entry(dir.path(), "a.bin", b"a");
        let b = entry(dir.path(), "b.bin", b"b");
        let c = entry(dir.path(), "c.bin", b"c");
        let mut a = a;
        let mut b = b;
        let mut c = c;
        a.mtime_unix = Some(100);
        b.mtime_unix = Some(200);
        c.mtime_unix = Some(300);

        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_max_files(vec![a, b, c], 2, &mut stats, &mut report).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // preserve input order of kept: b, c
        assert_eq!(names, vec!["b.bin", "c.bin"]);
        assert_eq!(stats.skipped_max_files, 1);
        assert_eq!(report.max_files.as_ref().unwrap().skipped[0].archive_name, "a.bin");
    }

    #[test]
    fn max_files_zero_skips_all() {
        let e = SelectedEntry {
            abs_path: PathBuf::from("/x"),
            archive_name: "x".into(),
            size: 1,
            mtime_unix: Some(1),
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: MemberKind::File,
        };
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_max_files(vec![e], 0, &mut stats, &mut report).unwrap();
        assert!(kept.is_empty());
        assert_eq!(stats.skipped_max_files, 1);
    }

    #[test]
    fn per_file_from_cli_parses() {
        let p = per_file_limits_from_cli(Some("1M"), Some("0"), Some("7d")).unwrap();
        assert_eq!(p.max_size, Some(1024 * 1024));
        assert_eq!(p.min_size, None); // 0 off
        assert_eq!(p.newer_than_secs, Some(7 * 86_400));
    }
}
