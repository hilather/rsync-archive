//! Human-friendly byte size parsing (`500M`, `1G`, raw integers).
//!
//! Ported to match archiveconverter defaults and CLI shape.

use crate::error::{Error, Result};

/// Default encode/nested-style size budget: 500 MiB (archiveconverter default).
pub const DEFAULT_ENCODE_SIZE_BUDGET: u64 = 500 * 1024 * 1024;

/// Parse a size string into bytes.
///
/// Accepts plain integers (bytes) or a number with optional suffix:
/// `K`/`KB`/`KiB`, `M`/`MB`/`MiB`, `G`/`GB`/`GiB` (binary, 1024-based).
pub fn parse_byte_size(s: &str) -> Result<u64> {
    let s = s.trim();
    if s.is_empty() {
        return Err(Error::Message("empty size string".into()));
    }
    let lower = s.to_ascii_lowercase();
    let (num_str, mult) = if let Some(n) = lower.strip_suffix("kib") {
        (n, 1024u64)
    } else if let Some(n) = lower.strip_suffix("mib") {
        (n, 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("gib") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("kb") {
        (n, 1024)
    } else if let Some(n) = lower.strip_suffix("mb") {
        (n, 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix("gb") {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix('k') {
        (n, 1024)
    } else if let Some(n) = lower.strip_suffix('m') {
        (n, 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix('g') {
        (n, 1024 * 1024 * 1024)
    } else if let Some(n) = lower.strip_suffix('b') {
        (n, 1)
    } else {
        (lower.as_str(), 1)
    };
    let num_str = num_str.trim();
    if num_str.is_empty() {
        return Err(Error::Message(format!("missing number in size '{s}'")));
    }
    let n: f64 = num_str
        .parse()
        .map_err(|_| Error::Message(format!("invalid size number in '{s}'")))?;
    if n < 0.0 || !n.is_finite() {
        return Err(Error::Message(format!("invalid size '{s}'")));
    }
    let bytes = n * mult as f64;
    if bytes > u64::MAX as f64 {
        return Err(Error::Message(format!("size too large: '{s}'")));
    }
    Ok(bytes.round() as u64)
}

/// Whether a job of `job_size` may start given current in-flight state.
///
/// Matches archiveconverter nested admission:
/// - Never exceeds `max_workers`.
/// - If nothing is running, always admit (single oversized job is allowed).
/// - If `budget == 0`, size is unlimited (workers only).
/// - Else require `running_sum + job_size <= budget`.
pub fn can_admit(
    running_sum: u64,
    running_count: usize,
    job_size: u64,
    budget: u64,
    max_workers: usize,
) -> bool {
    if max_workers == 0 || running_count >= max_workers {
        return false;
    }
    if running_count == 0 {
        return true;
    }
    if budget == 0 {
        return true;
    }
    running_sum.saturating_add(job_size) <= budget
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_suffixes() {
        assert_eq!(parse_byte_size("500M").unwrap(), 500 * 1024 * 1024);
        assert_eq!(parse_byte_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert_eq!(parse_byte_size("1024").unwrap(), 1024);
        assert_eq!(parse_byte_size("0").unwrap(), 0);
    }

    #[test]
    fn admit_budget() {
        let budget = 1000;
        assert!(can_admit(0, 0, 2000, budget, 4)); // first job always
        assert!(can_admit(500, 1, 400, budget, 4));
        assert!(!can_admit(500, 1, 600, budget, 4));
        assert!(!can_admit(0, 4, 1, budget, 4)); // workers full
        assert!(can_admit(9999, 1, 1, 0, 4)); // budget 0 = unlimited size
    }
}
