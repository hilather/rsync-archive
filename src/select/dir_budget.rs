//! Directory size budgets and per-directory file-count limits (newest-first).
//!
//! After normal include/exclude selection:
//!
//! - **Size budgets** (`--dir-max-size`): recursive under `PATH/`; keep while
//!   `running_sum + size <= limit`. Nested budgets: longest matching prefix wins.
//! - **File-count limits** (`--dir-max-files`): **immediate children only** of
//!   `PATH` (one path segment under the prefix; nested files are not counted).
//!   Keep the `N` newest by mtime; further direct children are file-limit skips.
//!
//! Both sort by mtime descending, then `archive_name` ascending.

use super::pathnorm::normalize_archive_str;
use super::walk::{SelectedEntry, SelectionStats};
use crate::error::{Error, Result};
use crate::util::parse_byte_size;
use std::fmt::Write as _;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Max archive names listed per kept/skip line (rest summarized as `+N more`).
const RESTRICTION_LIST_CAP: usize = 24;

/// One file kept or skipped under a directory restriction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestrictionFile {
    pub archive_name: String,
    pub size: u64,
}

/// Outcome of one `--dir-max-size` group (only emitted when the budget matched candidates).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirBudgetOutcome {
    pub prefix: String,
    pub limit: u64,
    pub kept: Vec<RestrictionFile>,
    pub skipped: Vec<RestrictionFile>,
}

/// Outcome of one `--dir-max-files` group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirFileLimitOutcome {
    pub prefix: String,
    pub max_count: u64,
    pub kept: Vec<RestrictionFile>,
    pub skipped: Vec<RestrictionFile>,
}

/// Outcome of global `--max-total-size` (only when the limit is set and candidates exist).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalSizeCapOutcome {
    pub limit: u64,
    pub kept: Vec<RestrictionFile>,
    pub skipped: Vec<RestrictionFile>,
}

/// Outcome of global `--max-files`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalCountCapOutcome {
    pub max_count: u64,
    pub kept: Vec<RestrictionFile>,
    pub skipped: Vec<RestrictionFile>,
}

/// Compact report of post-filter restrictions (dir + global).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RestrictionReport {
    pub size_budgets: Vec<DirBudgetOutcome>,
    pub file_limits: Vec<DirFileLimitOutcome>,
    /// Files skipped by `--max-size` (per-file).
    pub skipped_max_size: Vec<RestrictionFile>,
    /// Files skipped by `--min-size` (per-file).
    pub skipped_min_size: Vec<RestrictionFile>,
    /// Files skipped by `--newer-than` (too old).
    pub skipped_older_than: Vec<RestrictionFile>,
    /// Global `--max-total-size` outcome (if applied).
    pub max_total_size: Option<GlobalSizeCapOutcome>,
    /// Global `--max-files` outcome (if applied).
    pub max_files: Option<GlobalCountCapOutcome>,
}

impl RestrictionReport {
    pub fn is_empty(&self) -> bool {
        self.size_budgets.is_empty()
            && self.file_limits.is_empty()
            && self.skipped_max_size.is_empty()
            && self.skipped_min_size.is_empty()
            && self.skipped_older_than.is_empty()
            && self.max_total_size.is_none()
            && self.max_files.is_none()
    }

    /// Space-efficient multi-line text for stderr (empty string if nothing applied).
    pub fn format_compact(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        // Per-file filters first (order of application).
        if !self.skipped_max_size.is_empty() {
            let skip_b: u64 = self.skipped_max_size.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "max-size: skip {} ({})",
                self.skipped_max_size.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  skip", &self.skipped_max_size);
        }
        if !self.skipped_min_size.is_empty() {
            let skip_b: u64 = self.skipped_min_size.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "min-size: skip {} ({})",
                self.skipped_min_size.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  skip", &self.skipped_min_size);
        }
        if !self.skipped_older_than.is_empty() {
            let skip_b: u64 = self.skipped_older_than.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "newer-than: skip {} ({})",
                self.skipped_older_than.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  skip", &self.skipped_older_than);
        }
        for o in &self.size_budgets {
            let kept_b: u64 = o.kept.iter().map(|f| f.size).sum();
            let skip_b: u64 = o.skipped.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "dir-max-size {}/={}: kept {} ({}) skip {} ({})",
                o.prefix,
                format_bytes_short(o.limit),
                o.kept.len(),
                format_bytes_short(kept_b),
                o.skipped.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  kept", &o.kept);
            append_name_list(&mut out, "  skip", &o.skipped);
        }
        for o in &self.file_limits {
            let kept_b: u64 = o.kept.iter().map(|f| f.size).sum();
            let skip_b: u64 = o.skipped.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "dir-max-files {}/={}: kept {} ({}) skip {} ({})",
                o.prefix,
                o.max_count,
                o.kept.len(),
                format_bytes_short(kept_b),
                o.skipped.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  kept", &o.kept);
            append_name_list(&mut out, "  skip", &o.skipped);
        }
        if let Some(o) = &self.max_total_size {
            let kept_b: u64 = o.kept.iter().map(|f| f.size).sum();
            let skip_b: u64 = o.skipped.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "max-total-size={}: kept {} ({}) skip {} ({})",
                format_bytes_short(o.limit),
                o.kept.len(),
                format_bytes_short(kept_b),
                o.skipped.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  kept", &o.kept);
            append_name_list(&mut out, "  skip", &o.skipped);
        }
        if let Some(o) = &self.max_files {
            let kept_b: u64 = o.kept.iter().map(|f| f.size).sum();
            let skip_b: u64 = o.skipped.iter().map(|f| f.size).sum();
            let _ = writeln!(
                out,
                "max-files={}: kept {} ({}) skip {} ({})",
                o.max_count,
                o.kept.len(),
                format_bytes_short(kept_b),
                o.skipped.len(),
                format_bytes_short(skip_b),
            );
            append_name_list(&mut out, "  kept", &o.kept);
            append_name_list(&mut out, "  skip", &o.skipped);
        }
        out
    }

    /// Print compact restriction report to stderr (no-op if empty).
    pub fn eprint_compact(&self) {
        let s = self.format_compact();
        if !s.is_empty() {
            eprint!("{s}");
        }
    }
}

fn format_bytes_short(n: u64) -> String {
    const K: u64 = 1024;
    const M: u64 = K * 1024;
    const G: u64 = M * 1024;
    if n >= G {
        format!("{:.1}G", n as f64 / G as f64)
    } else if n >= M {
        format!("{:.1}M", n as f64 / M as f64)
    } else if n >= K {
        format!("{:.1}K", n as f64 / K as f64)
    } else {
        format!("{n}B")
    }
}

/// `  kept: a:1B b:2B … +3` — path:size tokens, capped.
fn append_name_list(out: &mut String, label: &str, files: &[RestrictionFile]) {
    if files.is_empty() {
        return;
    }
    let _ = write!(out, "{label}:");
    let show = files.len().min(RESTRICTION_LIST_CAP);
    for f in files.iter().take(show) {
        let _ = write!(
            out,
            " {}:{}",
            f.archive_name,
            format_bytes_short(f.size)
        );
    }
    if files.len() > show {
        let _ = write!(out, " +{}", files.len() - show);
    }
    let _ = writeln!(out);
}

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
            "invalid directory PATH: empty after stripping trailing slash".into(),
        ));
    }
    normalize_archive_str(trimmed)
}

/// One `--dir-max-files PATH=N` limit (immediate children only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirFileLimit {
    /// Archive-relative directory prefix (no trailing `/`).
    pub prefix: String,
    /// Max number of **direct child** files kept under this directory.
    pub max_count: u64,
}

/// Parse a single `--dir-max-files PATH=N` argument.
///
/// `PATH` is an archive-relative directory prefix (trailing `/` optional).
/// `N` is a non-negative integer (file count).
pub fn parse_dir_max_files_arg(s: &str) -> Result<DirFileLimit> {
    let s = s.trim();
    let (path, count) = s.split_once('=').ok_or_else(|| {
        Error::Message(format!(
            "invalid --dir-max-files '{s}': expected PATH=N (e.g. logs/=10)"
        ))
    })?;
    let path = path.trim();
    let count = count.trim();
    if path.is_empty() {
        return Err(Error::Message(
            "invalid --dir-max-files: empty PATH before '='".into(),
        ));
    }
    if count.is_empty() {
        return Err(Error::Message(format!(
            "invalid --dir-max-files '{s}': empty N after '='"
        )));
    }
    let prefix = normalize_budget_prefix(path)?;
    let max_count: u64 = count.parse().map_err(|_| {
        Error::Message(format!(
            "invalid --dir-max-files '{s}': N must be a non-negative integer"
        ))
    })?;
    Ok(DirFileLimit { prefix, max_count })
}

/// Parse many CLI args into file limits; duplicate prefixes error.
pub fn parse_dir_file_limits(args: &[String]) -> Result<Vec<DirFileLimit>> {
    let mut out = Vec::with_capacity(args.len());
    for a in args {
        let lim = parse_dir_max_files_arg(a)?;
        if out.iter().any(|x: &DirFileLimit| x.prefix == lim.prefix) {
            return Err(Error::Message(format!(
                "duplicate --dir-max-files for directory '{}'",
                lim.prefix
            )));
        }
        out.push(lim);
    }
    Ok(out)
}

/// Load `--dir-max-files-from FILE`: one `PATH=N` per non-comment line.
///
/// Blank lines and `#` comments are skipped. Duplicate prefixes (within the
/// file or against `existing`) error.
pub fn load_dir_max_files_from(
    path: &Path,
    existing: &mut Vec<DirFileLimit>,
) -> Result<()> {
    use super::from_file::read_capped_lines;
    let lines = read_capped_lines(path)?;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let lim = parse_dir_max_files_arg(trimmed).map_err(|e| {
            Error::Selection(format!("{}:{}: {e}", path.display(), idx + 1))
        })?;
        if existing.iter().any(|x| x.prefix == lim.prefix) {
            return Err(Error::Message(format!(
                "{}:{}: duplicate --dir-max-files for directory '{}'",
                path.display(),
                idx + 1,
                lim.prefix
            )));
        }
        existing.push(lim);
    }
    Ok(())
}

/// Merge CLI `--dir-max-files` args with optional `--dir-max-files-from`.
pub fn collect_dir_file_limits(
    cli_args: &[String],
    from_file: Option<&Path>,
) -> Result<Vec<DirFileLimit>> {
    let mut limits = parse_dir_file_limits(cli_args)?;
    if let Some(path) = from_file {
        load_dir_max_files_from(path, &mut limits)?;
    }
    Ok(limits)
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

/// Whether `archive_name` is a **direct child** of directory prefix `prefix`.
///
/// True only for one path segment under `prefix` (e.g. `logs/a.txt` under `logs`).
/// Nested paths (`logs/nested/b`) and the prefix itself are not direct children.
pub fn is_direct_child(archive_name: &str, prefix: &str) -> bool {
    if !is_under_budget_dir(archive_name, prefix) {
        return false;
    }
    let rest = &archive_name[prefix.len() + 1..];
    !rest.is_empty() && !rest.contains('/')
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
///
/// Outcomes (kept + skipped under each budget) are appended to `report` for a
/// single compact stderr dump — not one log line per file.
pub fn apply_dir_budgets(
    entries: Vec<SelectedEntry>,
    budgets: &[DirBudget],
    stats: &mut SelectionStats,
    report: &mut RestrictionReport,
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
        let mut outcome = DirBudgetOutcome {
            prefix: budget.prefix.clone(),
            limit: budget.limit,
            kept: Vec::new(),
            skipped: Vec::new(),
        };
        for &(_mtime, _name, i) in &ranked {
            let e = &entries[i];
            let next = sum.saturating_add(e.size);
            if next <= budget.limit {
                keep[i] = true;
                sum = next;
                outcome.kept.push(RestrictionFile {
                    archive_name: e.archive_name.clone(),
                    size: e.size,
                });
            } else {
                stats.skipped_dir_budget += 1;
                outcome.skipped.push(RestrictionFile {
                    archive_name: e.archive_name.clone(),
                    size: e.size,
                });
            }
        }
        report.size_budgets.push(outcome);
    }

    let out: Vec<SelectedEntry> = entries
        .into_iter()
        .enumerate()
        .filter_map(|(i, e)| if keep[i] { Some(e) } else { None })
        .collect();
    stats.selected = out.len() as u64;
    Ok(out)
}

/// Apply per-directory file-count limits (**recursive** under PATH/, rsync-style tree).
///
/// Scope matches size budgets: any selected file under `prefix/` counts.
/// Nested limits: **longest matching prefix** wins (independent of collection filters).
/// Newest mtime first; keep at most `max_count` files per group.
///
/// Outcomes are appended to `report` for a compact stderr dump.
pub fn apply_dir_file_limits(
    entries: Vec<SelectedEntry>,
    limits: &[DirFileLimit],
    stats: &mut SelectionStats,
    report: &mut RestrictionReport,
) -> Result<Vec<SelectedEntry>> {
    if limits.is_empty() || entries.is_empty() {
        stats.selected = entries.len() as u64;
        return Ok(entries);
    }

    // Recursive under prefix; longest matching restriction prefix wins when nested.
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); limits.len()];
    let mut unlimited: Vec<usize> = Vec::new();
    for (i, e) in entries.iter().enumerate() {
        let mut best: Option<(usize, usize)> = None; // (index, prefix_len)
        for (g, lim) in limits.iter().enumerate() {
            if is_under_budget_dir(&e.archive_name, &lim.prefix) {
                let len = lim.prefix.len();
                if best.map(|(_, l)| len > l).unwrap_or(true) {
                    best = Some((g, len));
                }
            }
        }
        match best {
            Some((g, _)) => groups[g].push(i),
            None => unlimited.push(i),
        }
    }

    let mut keep = vec![false; entries.len()];
    for i in unlimited {
        keep[i] = true;
    }

    for (g, indices) in groups.iter().enumerate() {
        if indices.is_empty() {
            continue;
        }
        let limit = &limits[g];
        let mut ranked: Vec<(u64, &str, usize)> = indices
            .iter()
            .map(|&i| {
                let e = &entries[i];
                let mtime = file_mtime_secs(&e.abs_path);
                (mtime, e.archive_name.as_str(), i)
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.cmp(b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        let mut kept_count = 0u64;
        let mut outcome = DirFileLimitOutcome {
            prefix: limit.prefix.clone(),
            max_count: limit.max_count,
            kept: Vec::new(),
            skipped: Vec::new(),
        };
        for &(_mtime, _name, i) in &ranked {
            let e = &entries[i];
            if kept_count < limit.max_count {
                keep[i] = true;
                kept_count += 1;
                outcome.kept.push(RestrictionFile {
                    archive_name: e.archive_name.clone(),
                    size: e.size,
                });
            } else {
                stats.skipped_dir_file_limit += 1;
                outcome.skipped.push(RestrictionFile {
                    archive_name: e.archive_name.clone(),
                    size: e.size,
                });
            }
        }
        report.file_limits.push(outcome);
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
            mtime_unix: None,
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
        let mut report = RestrictionReport::default();
        let kept = apply_dir_budgets(entries, &budgets, &mut stats, &mut report).unwrap();
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
        let mut report = RestrictionReport::default();
        let kept = apply_dir_budgets(
            vec![e10, e20, e30],
            &[DirBudget {
                prefix: "logs".into(),
                limit: 35,
            }],
            &mut stats,
            &mut report,
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
        let mut report = RestrictionReport::default();
        let kept = apply_dir_budgets(vec![a, b, c], &budgets, &mut stats, &mut report).unwrap();
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
        let mut report = RestrictionReport::default();
        let kept = apply_dir_budgets(
            vec![b, a],
            &[DirBudget {
                prefix: "d".into(),
                limit: 5,
            }],
            &mut stats,
            &mut report,
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
            mtime_unix: None,
        };
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_dir_budgets(vec![e], &[], &mut stats, &mut report).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(stats.skipped_dir_budget, 0);
    }

    #[test]
    fn parse_path_eq_count() {
        let lim = parse_dir_max_files_arg("logs/=10").unwrap();
        assert_eq!(lim.prefix, "logs");
        assert_eq!(lim.max_count, 10);

        let lim = parse_dir_max_files_arg("cache=0").unwrap();
        assert_eq!(lim.prefix, "cache");
        assert_eq!(lim.max_count, 0);
    }

    #[test]
    fn parse_max_files_rejects_bad() {
        assert!(parse_dir_max_files_arg("noscale").is_err());
        assert!(parse_dir_max_files_arg("=10").is_err());
        assert!(parse_dir_max_files_arg("logs/=").is_err());
        assert!(parse_dir_max_files_arg("logs/=-1").is_err());
        assert!(parse_dir_max_files_arg("logs/=1.5").is_err());
        assert!(parse_dir_max_files_arg("../x=1").is_err());
    }

    #[test]
    fn direct_child_match() {
        assert!(is_direct_child("logs/a.txt", "logs"));
        assert!(is_direct_child("logs/old.bin", "logs"));
        assert!(!is_direct_child("logs/nested/x", "logs"));
        assert!(!is_direct_child("logs", "logs"));
        assert!(!is_direct_child("logs2/a", "logs"));
        assert!(!is_direct_child("other/a", "logs"));
        assert!(is_direct_child("logs/nested/b", "logs/nested"));
    }

    #[test]
    fn file_limit_newest_first_recursive() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let old = entry(root, "logs/old.bin", b"old");
        let mid = entry(root, "logs/mid.bin", b"mid");
        let new = entry(root, "logs/new.bin", b"new");
        // Nested files count against logs/=2 (rsync-style tree scope)
        let nested = entry(root, "logs/nested/deep.bin", b"deep");
        let other = entry(root, "keep/me.txt", b"ok");

        set_mtime(&old.abs_path, 100);
        set_mtime(&mid.abs_path, 200);
        set_mtime(&new.abs_path, 300);
        set_mtime(&nested.abs_path, 50);

        let entries = vec![old, mid, new, nested, other];
        let limits = vec![DirFileLimit {
            prefix: "logs".into(),
            max_count: 2,
        }];
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_dir_file_limits(entries, &limits, &mut stats, &mut report).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // newest 2 under logs/**: new, mid; old+nested skipped; other unlimited
        assert_eq!(names, vec!["logs/mid.bin", "logs/new.bin", "keep/me.txt"]);
        assert_eq!(stats.skipped_dir_file_limit, 2);
        assert_eq!(stats.selected, 3);
    }

    #[test]
    fn file_limit_zero_skips_all_recursive() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let a = entry(root, "d/a", b"a");
        let nested = entry(root, "d/sub/b", b"b");
        set_mtime(&a.abs_path, 100);
        set_mtime(&nested.abs_path, 200);
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_dir_file_limits(
            vec![a, nested],
            &[DirFileLimit {
                prefix: "d".into(),
                max_count: 0,
            }],
            &mut stats,
            &mut report,
        )
        .unwrap();
        assert!(kept.is_empty());
        assert_eq!(stats.skipped_dir_file_limit, 2);
    }

    #[test]
    fn file_limit_name_tiebreak() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let b = entry(root, "d/b", b"b");
        let a = entry(root, "d/a", b"a");
        set_mtime(&a.abs_path, 100);
        set_mtime(&b.abs_path, 100);
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_dir_file_limits(
            vec![b, a],
            &[DirFileLimit {
                prefix: "d".into(),
                max_count: 1,
            }],
            &mut stats,
            &mut report,
        )
        .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].archive_name, "d/a");
        assert_eq!(stats.skipped_dir_file_limit, 1);
    }

    #[test]
    fn file_limit_nested_dirs_independent() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let outer = entry(root, "logs/a.bin", b"a");
        let inner_old = entry(root, "logs/nested/old.bin", b"o");
        let inner_new = entry(root, "logs/nested/new.bin", b"n");
        set_mtime(&outer.abs_path, 100);
        set_mtime(&inner_old.abs_path, 100);
        set_mtime(&inner_new.abs_path, 300);

        let limits = vec![
            DirFileLimit {
                prefix: "logs".into(),
                max_count: 1,
            },
            DirFileLimit {
                prefix: "logs/nested".into(),
                max_count: 1,
            },
        ];
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept =
            apply_dir_file_limits(vec![outer, inner_old, inner_new], &limits, &mut stats, &mut report).unwrap();
        let names: Vec<_> = kept.iter().map(|e| e.archive_name.as_str()).collect();
        // outer: a kept (only direct under logs); nested: newest only
        assert_eq!(names, vec!["logs/a.bin", "logs/nested/new.bin"]);
        assert_eq!(stats.skipped_dir_file_limit, 1);
    }

    #[test]
    fn no_file_limits_noop() {
        let e = SelectedEntry {
            abs_path: PathBuf::from("/x"),
            archive_name: "x".into(),
            size: 1,
            mtime_unix: None,
        };
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let kept = apply_dir_file_limits(vec![e], &[], &mut stats, &mut report).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(stats.skipped_dir_file_limit, 0);
    }

    #[test]
    fn compact_report_lists_kept_and_skip() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        let old = entry(root, "logs/old.bin", &vec![0u8; 20]);
        let new = entry(root, "logs/new.bin", &vec![0u8; 20]);
        set_mtime(&old.abs_path, 100);
        set_mtime(&new.abs_path, 300);
        let mut stats = SelectionStats::default();
        let mut report = RestrictionReport::default();
        let _ = apply_dir_budgets(
            vec![old, new],
            &[DirBudget {
                prefix: "logs".into(),
                limit: 25,
            }],
            &mut stats,
            &mut report,
        )
        .unwrap();
        let text = report.format_compact();
        assert!(text.contains("dir-max-size logs/=25B"), "{text}");
        assert!(text.contains("kept:"), "{text}");
        assert!(text.contains("skip:"), "{text}");
        assert!(text.contains("logs/new.bin"), "{text}");
        assert!(text.contains("logs/old.bin"), "{text}");
        // One summary line + kept + skip; not per-field spam
        assert!(text.lines().count() <= 4, "{text}");
    }
}
