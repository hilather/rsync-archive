//! rsync-archive library: selection, non-solid 7z create, and store embed.
//!
//! Stage 2: non-solid 7z headers + Copy store writer (embed foundation).
//! Stage 4: frozen v1 rsync include/exclude engine (walk lands in Stage 5).
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
pub use select::{
    archive_name_for, load_exclude_from, load_filter_from, load_include_from, parse_rule, Rule,
    RuleAction, RuleSet, SourceKind, SourceSpec,
};

/// Library version (same as package).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
