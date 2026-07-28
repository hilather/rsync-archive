//! Automatic thread / worker policy (aligned with archiveconverter).
//!
//! archiveconverter lessons:
//! - Many tiny files → single-thread pack is often faster (`HIGH_FILE_COUNT` / small avg).
//! - Explicit `--threads` always wins.
//! - Nested (here: concurrent file encodes) workers auto from threads/CPUs when `0`.

/// Defaults tuned like archiveconverter: many tiny files → prefer 1 pack worker.
pub const HIGH_FILE_COUNT: usize = 1_000;
pub const SMALL_AVG_SIZE: u64 = 64 * 1024; // 64 KiB

/// Resolve pack / encode worker count for create.
///
/// - If `explicit` is set, always honor it (at least 1).
/// - Else if selection looks like "many tiny files", return `1`.
/// - Else return available parallelism (at least 1).
pub fn resolve_encode_workers(
    explicit: Option<u32>,
    file_count: usize,
    total_bytes: u64,
) -> usize {
    if let Some(t) = explicit {
        return (t.max(1)) as usize;
    }
    if file_count == 0 {
        return 1;
    }
    let avg = total_bytes / file_count as u64;
    if file_count >= HIGH_FILE_COUNT || avg < SMALL_AVG_SIZE {
        tracing::debug!(
            file_count,
            avg_bytes = avg,
            "auto encode workers → 1 (many/tiny files; archiveconverter policy)"
        );
        1
    } else {
        let n = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        tracing::debug!(
            file_count,
            avg_bytes = avg,
            workers = n,
            "auto encode workers → available_parallelism"
        );
        n
    }
}

/// Resolve concurrent encode slots from `--encode-concurrency` (0 = auto).
///
/// Matches archiveconverter `--nested-concurrency` default `0` → auto from threads.
pub fn resolve_encode_concurrency(explicit: usize, threads_or_auto: usize) -> usize {
    if explicit == 0 {
        threads_or_auto.max(1)
    } else {
        explicit.max(1)
    }
}

/// Stats helper: file count and total bytes from sizes.
pub fn file_stats_from_sizes(sizes: impl Iterator<Item = u64>) -> (usize, u64) {
    let mut n = 0usize;
    let mut bytes = 0u64;
    for s in sizes {
        n += 1;
        bytes = bytes.saturating_add(s);
    }
    (n, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn honors_explicit() {
        assert_eq!(resolve_encode_workers(Some(4), 1_000_000, 100), 4);
        assert_eq!(resolve_encode_workers(Some(0), 10, 10), 1); // clamp to 1
    }

    #[test]
    fn many_tiny_forces_one() {
        assert_eq!(resolve_encode_workers(None, 50_000, 50_000 * 300), 1);
    }

    #[test]
    fn few_large_uses_parallelism() {
        let w = resolve_encode_workers(None, 10, 10 * 10 * 1024 * 1024);
        assert!(w >= 1);
    }

    #[test]
    fn concurrency_auto() {
        assert_eq!(resolve_encode_concurrency(0, 8), 8);
        assert_eq!(resolve_encode_concurrency(2, 8), 2);
    }
}
