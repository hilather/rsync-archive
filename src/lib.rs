//! rsync-archive library: selection, non-solid 7z create, and store embed.
//!
//! Stage 2 provides non-solid 7z headers and a Copy (store) writer for embed.
//! Create/embed pipelines land in later stages.

pub mod archive;
pub mod cli;
pub mod error;
pub mod pipeline;
pub mod select;
pub mod util;

pub use archive::{
    write_raw_header, write_start_header, HeaderFile, NonsolidStoreWriter, SIG_HEADER_SIZE,
};
pub use error::{Error, Result};
pub use pipeline::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
pub use select::{archive_name_for, SourceKind, SourceSpec};

/// Library version (same as package).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
