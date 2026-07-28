//! rsync-archive library: selection, non-solid 7z create, and store embed.
//!
//! Stage 1 provides path normalization, source specs, and atomic output helpers.
//! Create/embed pipelines land in later stages.

pub mod cli;
pub mod error;
pub mod pipeline;
pub mod select;
pub mod util;

pub use error::{Error, Result};
pub use pipeline::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
pub use select::{archive_name_for, SourceKind, SourceSpec};

/// Library version (same as package).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
