//! Shared utilities.

pub mod auto_threads;
pub mod size_parse;

pub use auto_threads::{
    file_stats_from_sizes, resolve_encode_concurrency, resolve_encode_workers, HIGH_FILE_COUNT,
    SMALL_AVG_SIZE,
};
pub use size_parse::{can_admit, parse_byte_size, DEFAULT_ENCODE_SIZE_BUDGET};

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
