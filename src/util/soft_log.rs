//! Compact soft-skip / soft-warn logging for create.
//!
//! **Policy (no spam, space-efficient):**
//! - Per-event detail goes to `tracing` at **debug** only.
//! - Stderr gets a **sampled summary** at end of create (or on demand): one line
//!   per skip kind with first few examples and a total count.
//! - Truncation/pad is a soft content warning (same sampling rules).

use std::cell::RefCell;
use std::fmt::Write as _;
use tracing::debug;

/// Max path/name samples kept per soft-skip kind (then "+N more").
pub const SOFT_SKIP_SAMPLE_MAX: usize = 5;

/// Soft event kinds aggregated during create.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SoftKind {
    /// Member path vanished / unreadable at walk, open, or encode.
    Vanished,
    /// Hard-link member dropped because target body was not written.
    Hardlink,
    /// Member short-read / mid-read pad with zeros (tar / seekable).
    Padded,
    /// `--files-from` line soft-skipped (`--files-from-skip-missing`).
    FilesFrom,
    /// Multi-SRC missing root soft-skipped.
    MissingSrc,
    /// Invalid restriction-list line ignored.
    Config,
}

impl SoftKind {
    fn label(self) -> &'static str {
        match self {
            SoftKind::Vanished => "vanished",
            SoftKind::Hardlink => "hardlink",
            SoftKind::Padded => "padded",
            SoftKind::FilesFrom => "files-from",
            SoftKind::MissingSrc => "missing-src",
            SoftKind::Config => "config",
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Bucket {
    count: u64,
    samples: Vec<String>,
}

impl Bucket {
    fn note(&mut self, sample: &str) {
        self.count = self.count.saturating_add(1);
        if self.samples.len() < SOFT_SKIP_SAMPLE_MAX {
            let s = compact_sample(sample);
            if !s.is_empty() {
                self.samples.push(s);
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// One compact line: `skip: vanished 12: a, b, c … (+9)`.
    fn format_line(&self, kind: SoftKind) -> String {
        let mut out = String::with_capacity(96);
        let _ = write!(out, "skip: {} {}", kind.label(), self.count);
        if !self.samples.is_empty() {
            out.push_str(": ");
            out.push_str(&self.samples.join(", "));
            let shown = self.samples.len() as u64;
            if self.count > shown {
                let _ = write!(out, " … (+{})", self.count - shown);
            }
        }
        out
    }
}

/// Prefer archive name / basename; cap length for space.
fn compact_sample(s: &str) -> String {
    const MAX: usize = 64;
    let t = s.trim();
    if t.is_empty() {
        return String::new();
    }
    // Prefer last path component when absolute/long.
    let base = t.rsplit(['/', '\\']).next().unwrap_or(t);
    let use_s = if base.len() >= 8 && base.len() < t.len() {
        base
    } else {
        t
    };
    if use_s.len() <= MAX {
        use_s.to_string()
    } else {
        format!("{}…", &use_s[..MAX.saturating_sub(1)])
    }
}

/// Aggregator for soft skips during one create (thread-local for writer convenience).
#[derive(Debug, Default, Clone)]
pub struct SoftSkipLog {
    vanished: Bucket,
    hardlink: Bucket,
    padded: Bucket,
    files_from: Bucket,
    missing_src: Bucket,
    config: Bucket,
}

impl SoftSkipLog {
    pub fn note(&mut self, kind: SoftKind, sample: &str) {
        match kind {
            SoftKind::Vanished => self.vanished.note(sample),
            SoftKind::Hardlink => self.hardlink.note(sample),
            SoftKind::Padded => self.padded.note(sample),
            SoftKind::FilesFrom => self.files_from.note(sample),
            SoftKind::MissingSrc => self.missing_src.note(sample),
            SoftKind::Config => self.config.note(sample),
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn total(&self) -> u64 {
        self.vanished.count
            + self.hardlink.count
            + self.padded.count
            + self.files_from.count
            + self.missing_src.count
            + self.config.count
    }

    /// Emit compact stderr lines (one per non-empty kind). Returns lines written.
    pub fn eprint_compact(&self) -> usize {
        let mut n = 0;
        for (kind, b) in [
            (SoftKind::Vanished, &self.vanished),
            (SoftKind::Hardlink, &self.hardlink),
            (SoftKind::Padded, &self.padded),
            (SoftKind::FilesFrom, &self.files_from),
            (SoftKind::MissingSrc, &self.missing_src),
            (SoftKind::Config, &self.config),
        ] {
            if !b.is_empty() {
                eprintln!("{}", b.format_line(kind));
                n += 1;
            }
        }
        n
    }

    /// Unit-test helper: format without printing.
    pub fn format_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (kind, b) in [
            (SoftKind::Vanished, &self.vanished),
            (SoftKind::Hardlink, &self.hardlink),
            (SoftKind::Padded, &self.padded),
            (SoftKind::FilesFrom, &self.files_from),
            (SoftKind::MissingSrc, &self.missing_src),
            (SoftKind::Config, &self.config),
        ] {
            if !b.is_empty() {
                out.push(b.format_line(kind));
            }
        }
        out
    }
}

thread_local! {
    static TLS_LOG: RefCell<SoftSkipLog> = RefCell::new(SoftSkipLog::default());
}

/// Record a soft skip: **debug** detail + sample for end-of-create summary.
///
/// `sample` should be a short archive name or path (will be compacted).
pub fn soft_skip_note(kind: SoftKind, sample: &str) {
    soft_skip_note_detail(kind, sample, None);
}

/// Like [`soft_skip_note`] with optional extra detail for debug only.
pub fn soft_skip_note_detail(kind: SoftKind, sample: &str, detail: Option<&str>) {
    match detail {
        Some(d) => debug!(kind = kind.label(), sample, detail = d, "soft-skip"),
        None => debug!(kind = kind.label(), sample, "soft-skip"),
    }
    TLS_LOG.with(|cell| cell.borrow_mut().note(kind, sample));
}

/// Clear TLS aggregator (start of create).
pub fn soft_skip_reset() {
    TLS_LOG.with(|cell| cell.borrow_mut().clear());
}

/// Print compact soft-skip lines to stderr and clear the aggregator.
pub fn soft_skip_flush_stderr() {
    TLS_LOG.with(|cell| {
        let mut log = cell.borrow_mut();
        log.eprint_compact();
        log.clear();
    });
}

/// Snapshot total soft events (for tests).
#[cfg(test)]
pub fn soft_skip_total() -> u64 {
    TLS_LOG.with(|cell| cell.borrow().total())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_and_plus_more() {
        let mut log = SoftSkipLog::default();
        for i in 0..12 {
            log.note(SoftKind::Vanished, &format!("file{i}.log"));
        }
        let lines = log.format_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].starts_with("skip: vanished 12: "));
        assert!(lines[0].contains("file0.log"));
        assert!(lines[0].contains("… (+7)"), "{}", lines[0]);
    }

    #[test]
    fn compact_uses_basename_for_long_paths() {
        let mut log = SoftSkipLog::default();
        log.note(SoftKind::Vanished, "/var/log/app/very/deep/error.log");
        let line = &log.format_lines()[0];
        assert!(line.contains("error.log"), "{line}");
        assert!(!line.contains("/var/log"), "{line}");
    }

    #[test]
    fn empty_log_prints_nothing() {
        let log = SoftSkipLog::default();
        assert!(log.format_lines().is_empty());
        assert_eq!(log.eprint_compact(), 0);
    }

    #[test]
    fn multiple_kinds_separate_lines() {
        let mut log = SoftSkipLog::default();
        log.note(SoftKind::Vanished, "a");
        log.note(SoftKind::Padded, "b");
        log.note(SoftKind::Config, "file-size L3");
        let lines = log.format_lines();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("vanished"));
        assert!(lines[1].contains("padded"));
        assert!(lines[2].contains("config"));
    }

    #[test]
    fn tls_note_and_reset() {
        soft_skip_reset();
        soft_skip_note(SoftKind::FilesFrom, "missing.txt");
        assert_eq!(soft_skip_total(), 1);
        soft_skip_reset();
        assert_eq!(soft_skip_total(), 0);
    }
}
