//! rsync-archive library: selection, non-solid 7z create, seekable-zstd, and store embed.
//!
//! Stage 2–3: store writer + embed.
//! Stage 4–5: filters + walk + dry-run.
//! Stage 6: non-solid LZMA2 create write.
//! Seekable-zstd: single-stream create with member index.

pub mod archive;
pub mod cli;
pub mod error;
pub mod pipeline;
pub mod select;
pub mod util;

pub use archive::{
    decompress_tar_lz4_payload_to_tar_bytes, decompress_tar_zstd_payload_to_tar_bytes,
    extract_member, extract_member_bytes, extract_tar_lz4_member, extract_tar_lz4_member_bytes,
    extract_tar_zstd_member, extract_tar_zstd_member_bytes, list_members, list_tar_lz4_members,
    list_tar_zstd_members, verify_seekable_zstd, verify_tar_lz4, verify_tar_zstd,
    write_raw_header, write_seekable_zstd, write_start_header, write_tar_lz4, write_tar_zstd,
    CompressMethod, HeaderFile, MemberIndex, MemberIndexEntry, NonsolidLzma2Writer,
    NonsolidStoreWriter, TarMemberIndex, TarMemberIndexEntry, SIG_HEADER_SIZE,
};
pub use error::{Error, Result};
pub use pipeline::{
    cleanup_partial, commit_output, output_exists, partial_path_for, prepare_output, OutputPaths,
};
pub use select::{
    apply_dir_budgets, apply_dir_file_limits, apply_file_size_from, apply_max_files,
    apply_max_total_size, apply_per_file_limits, archive_name_for, collect_dir_file_limits,
    collect_from_files_from, collect_from_sources, load_dir_max_files_from, load_dir_max_size_from,
    load_exclude_from, load_file_size_from, load_filter_from, load_include_from, meta_owner_mode,
    names_for_uid_gid, parse_dir_budgets, parse_dir_file_limits, parse_dir_max_files_arg,
    parse_dir_max_size_arg, parse_dir_restrict_line, parse_duration_secs, parse_file_size_line,
    parse_rule, per_file_limits_from_cli, DirBudget, DirBudgetOutcome, DirFileLimit,
    DirFileLimitOutcome, FileSizeRule, GlobalCountCapOutcome, GlobalSizeCapOutcome, MemberKind,
    PerFileLimits, RestrictionFile, RestrictionReport, Rule, RuleAction, RuleSet, SelectedEntry,
    SelectionStats, SourceKind, SourceSpec, DEFAULT_FILE_MODE,
};

/// Library version (same as package).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
