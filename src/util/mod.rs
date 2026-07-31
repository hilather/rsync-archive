//! Shared utilities.

pub mod auto_threads;
pub mod size_parse;
pub mod soft_log;

pub use auto_threads::{
    file_stats_from_sizes, resolve_encode_concurrency, resolve_encode_workers, HIGH_FILE_COUNT,
    SMALL_AVG_SIZE,
};
pub use size_parse::{can_admit, parse_byte_size, DEFAULT_ENCODE_SIZE_BUDGET};
pub use soft_log::{
    soft_skip_flush_stderr, soft_skip_note, soft_skip_note_detail, soft_skip_reset, SoftKind,
    SoftSkipLog, SOFT_SKIP_SAMPLE_MAX,
};

/// How many consecutive `EINTR`/`Interrupted` results to tolerate before failing.
///
/// Covers a burst of signal interruptions without turning a single retryable
/// error into a soft-skip or hard job failure.
pub const INTERRUPTED_RETRY_LIMIT: u32 = 16;

/// Transient filesystem failures that should not abort create (soft-skip the member).
///
/// Used at walk, open, and mid-stream read. Covers:
/// - `ENOENT` races (vanished temp/log files)
/// - `EACCES` unreadable files under broad roots
/// - NFS `ESTALE` (`StaleNetworkFileHandle` when the OS maps it)
///
/// Does **not** treat `Interrupted` (`EINTR`) as skippable — that is retryable
/// via [`retry_interrupted`] / [`open_file_for_encode`]. Does **not** treat
/// generic `InvalidInput` as skippable (that often signals real
/// programmer/protocol errors). Network timeouts remain hard failures.
pub fn is_skippable_fs_io(e: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    match e.kind() {
        ErrorKind::NotFound
        | ErrorKind::PermissionDenied
        | ErrorKind::StaleNetworkFileHandle => true,
        // Older Rust / some platforms map ESTALE to Other with raw os error.
        ErrorKind::Other => {
            #[cfg(unix)]
            {
                e.raw_os_error() == Some(libc::ESTALE)
            }
            #[cfg(not(unix))]
            {
                false
            }
        }
        _ => false,
    }
}

/// `EINTR` / [`std::io::ErrorKind::Interrupted`] — retry the syscall; do not soft-skip.
///
/// Unlike [`is_skippable_fs_io`], this is a transient condition that should be
/// retried a few times and only then surfaced as a hard error.
pub fn is_retryable_fs_io(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Interrupted
}

/// Retry `f` while it returns [`std::io::ErrorKind::Interrupted`].
///
/// After [`INTERRUPTED_RETRY_LIMIT`] consecutive interruptions, the last error
/// is returned as-is (caller decides hard-fail vs other mapping). Non-interrupt
/// results (success or other errors) are returned immediately.
pub fn retry_interrupted<T>(mut f: impl FnMut() -> std::io::Result<T>) -> std::io::Result<T> {
    let mut attempts = 0u32;
    loop {
        match f() {
            Err(e) if is_retryable_fs_io(&e) => {
                attempts += 1;
                if attempts >= INTERRUPTED_RETRY_LIMIT {
                    return Err(e);
                }
            }
            other => return other,
        }
    }
}

/// Read into `buf`, retrying `EINTR`/`Interrupted` a few times.
pub fn read_retrying(
    reader: &mut dyn std::io::Read,
    buf: &mut [u8],
) -> std::io::Result<usize> {
    retry_interrupted(|| reader.read(buf))
}

/// Open `path` for encode: retry `EINTR`, then map skippable I/O → [`crate::error::Error::Vanished`].
///
/// Hard (non-skippable, non-exhausted-retry) failures become [`crate::error::Error::Archive`].
pub fn open_file_for_encode(path: &std::path::Path) -> crate::error::Result<std::fs::File> {
    match retry_interrupted(|| std::fs::File::open(path)) {
        Ok(f) => Ok(f),
        Err(e) if is_skippable_fs_io(&e) => Err(crate::error::Error::Vanished(path.to_path_buf())),
        Err(e) => Err(crate::error::Error::Archive(format!(
            "open {}: {e}",
            path.display()
        ))),
    }
}

/// Map an I/O error from reading/opening `path` into soft-skip or hard failure.
///
/// Skippable → [`crate::error::Error::Vanished`]; otherwise `Archive`/`Io` message.
/// Callers should [`retry_interrupted`] first when the operation is retryable.
pub fn fs_io_to_error(path: &std::path::Path, e: std::io::Error, context: &str) -> crate::error::Error {
    if is_skippable_fs_io(&e) {
        crate::error::Error::Vanished(path.to_path_buf())
    } else {
        crate::error::Error::Archive(format!("{context} {}: {e}", path.display()))
    }
}

#[cfg(test)]
mod skippable_io_tests {
    use super::*;
    use std::io::{Error, ErrorKind};
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn skippable_kinds() {
        assert!(is_skippable_fs_io(&Error::new(ErrorKind::NotFound, "x")));
        assert!(is_skippable_fs_io(&Error::new(ErrorKind::PermissionDenied, "x")));
        assert!(is_skippable_fs_io(&Error::new(
            ErrorKind::StaleNetworkFileHandle,
            "x"
        )));
        // EINTR is retryable, not a soft-skip.
        assert!(!is_skippable_fs_io(&Error::new(ErrorKind::Interrupted, "x")));
        assert!(is_retryable_fs_io(&Error::new(ErrorKind::Interrupted, "x")));
        assert!(!is_retryable_fs_io(&Error::new(ErrorKind::NotFound, "x")));
        assert!(!is_skippable_fs_io(&Error::new(ErrorKind::InvalidInput, "x")));
        assert!(!is_skippable_fs_io(&Error::new(ErrorKind::TimedOut, "x")));
        assert!(!is_skippable_fs_io(&Error::new(ErrorKind::OutOfMemory, "x")));
    }

    #[cfg(unix)]
    #[test]
    fn estale_raw_os_error_skippable() {
        let e = Error::from_raw_os_error(libc::ESTALE);
        assert!(is_skippable_fs_io(&e));
        assert!(!is_retryable_fs_io(&e));
    }

    #[test]
    fn retry_interrupted_succeeds_after_eintr() {
        let n = AtomicU32::new(0);
        let out = retry_interrupted(|| {
            let i = n.fetch_add(1, Ordering::SeqCst);
            if i < 3 {
                Err(Error::new(ErrorKind::Interrupted, "eintr"))
            } else {
                Ok(42u32)
            }
        })
        .unwrap();
        assert_eq!(out, 42);
        assert_eq!(n.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn retry_interrupted_exhausts_and_fails() {
        let n = AtomicU32::new(0);
        let err = retry_interrupted(|| {
            n.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(Error::new(ErrorKind::Interrupted, "still eintr"))
        })
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Interrupted);
        assert_eq!(n.load(Ordering::SeqCst), INTERRUPTED_RETRY_LIMIT);
    }

    #[test]
    fn retry_interrupted_passes_through_non_retryable() {
        let n = AtomicU32::new(0);
        let err = retry_interrupted(|| {
            n.fetch_add(1, Ordering::SeqCst);
            Err::<(), _>(Error::new(ErrorKind::NotFound, "gone"))
        })
        .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert_eq!(n.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn open_file_for_encode_vanished_on_missing() {
        let path = std::path::Path::new("/no/such/rsync-archive-eintr-test-path");
        let err = open_file_for_encode(path).unwrap_err();
        match err {
            crate::error::Error::Vanished(p) => assert_eq!(p, path),
            other => panic!("expected Vanished, got {other:?}"),
        }
    }
}

use tracing_subscriber::EnvFilter;

/// Initialize stderr tracing from `-v` count and optional `RUST_LOG`.
///
/// | verbose | default filter |
/// |---------|----------------|
/// | 0       | info           |
/// | 1 (`-v`)| debug          |
/// | 2+      | trace          |
///
/// `RUST_LOG` / `EnvFilter` from the environment overrides the default when set.
pub fn init_tracing(verbose: u8) {
    let default = match verbose {
        0 => "info",
        1 => "debug",
        _ => "trace",
    };
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .try_init();
}
