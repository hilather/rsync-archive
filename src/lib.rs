//! rsync-archive library: selection, non-solid 7z create, and store embed.
//!
//! Stage 2–3: store writer + embed.
//! Stage 4–5: filters + walk + dry-run.
//! Stage 6: non-solid LZMA2 create write.

pub mod archive;
pub mod cli;
pub mod error;
pub mod pipeline;
pub mod select;
pub mod util;

pub use archive::{
    write_raw_header, write_start_header, CompressMethod, HeaderFile, NonsolidLzma2Writer,
    NonsolidStoreWriter, SIG_HEADER_SIZE,
};
pub use error::{Error, Result};
pub use pipeline::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
pub use select::{
    archive_name_for, collect_from_files_from, collect_from_sources, load_exclude_from,
    load_filter_from, load_include_from, parse_rule, Rule, RuleAction, RuleSet, SelectedEntry,
    SelectionStats, SourceKind, SourceSpec,
};

/// Library version (same as package).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
