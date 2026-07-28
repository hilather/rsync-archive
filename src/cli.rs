//! Clap CLI surface for `rsync-archive`.
//!
//! Flag sets for `create` and `embed` follow `docs/DESIGN.md` (v1 freeze).
//! Pipelines are stubbed until later stages.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// Create non-solid 7z archives with rsync-style selection; embed finished archives.
#[derive(Debug, Parser)]
#[command(
    name = "rsync-archive",
    version,
    about = "Stream-create non-solid 7z with rsync selection; embed finished archives under a master store 7z",
    long_about = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Increase log verbosity (-v = debug, -vv = trace).
    #[arg(short = 'v', long = "verbose", action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a non-solid 7z from filesystem paths using rsync-style filters.
    Create(CreateArgs),
    /// Embed finished archive files under a master non-solid store 7z (Copy method).
    Embed(EmbedArgs),
}

/// Arguments for `rsync-archive create`.
#[derive(Debug, Parser)]
pub struct CreateArgs {
    /// Output archive path (`.7z`).
    #[arg(short = 'o', long = "output", value_name = "OUT")]
    pub output: PathBuf,

    /// List what would be archived without writing.
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// Overwrite `-o` if it already exists.
    #[arg(long = "force")]
    pub force: bool,

    /// Exclude pattern (repeatable; rsync-style).
    #[arg(long = "exclude", value_name = "PATTERN", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Include pattern (repeatable; rsync-style).
    #[arg(long = "include", value_name = "PATTERN", action = clap::ArgAction::Append)]
    pub include: Vec<String>,

    /// Read exclude patterns from file (one per line).
    #[arg(long = "exclude-from", value_name = "FILE")]
    pub exclude_from: Option<PathBuf>,

    /// Read include patterns from file (one per line).
    #[arg(long = "include-from", value_name = "FILE")]
    pub include_from: Option<PathBuf>,

    /// Explicit file list (exclusive of SRC...; paths relative to CWD unless absolute).
    #[arg(long = "files-from", value_name = "FILE")]
    pub files_from: Option<PathBuf>,

    /// Filter rule, e.g. `+ *.rs` or `- *.tmp` (repeatable).
    #[arg(long = "filter", value_name = "RULE", action = clap::ArgAction::Append)]
    pub filter: Vec<String>,

    /// Compression level 0–9 (default 5). Meaning depends on `--method`
    /// (LZMA2 preset / mapped Zstd level; LZ4 ignores fine-grained levels).
    #[arg(long = "level", default_value_t = 5, value_parser = clap::value_parser!(u32).range(0..=9))]
    pub level: u32,

    /// Compression method: `lzma2` (default), `zstd` (fast, strong ratio), or `lz4` (fastest).
    /// All produce **non-solid** per-file packs (file-level random access).
    #[arg(long = "method", default_value = "lzma2")]
    pub method: String,

    /// Encode worker count (archiveconverter-style). Omit for auto:
    /// many tiny files → 1; else available CPU parallelism.
    #[arg(long)]
    pub threads: Option<u32>,

    /// Max concurrent file encodes (`0` = auto from `--threads` / CPUs).
    /// Same idea as archiveconverter `--nested-concurrency`.
    #[arg(long = "encode-concurrency", default_value_t = 0)]
    pub encode_concurrency: usize,

    /// Max total **uncompressed** size of files encoding at once (default `500M`).
    /// `0` = no size cap. Same default as archiveconverter `--nested-size-budget`.
    #[arg(long = "encode-size-budget", default_value = "500M")]
    pub encode_size_budget: String,

    /// Cap total selected bytes under an archive-relative directory (repeatable).
    ///
    /// Format: `PATH=SIZE` (e.g. `logs/=100M`, `cache=50M`). After normal filters,
    /// files under `PATH` are considered newest-mtime-first; further files that
    /// would exceed the budget are skipped (counted as dir-budget skips).
    /// Nested budgets: longest matching prefix wins.
    #[arg(long = "dir-max-size", value_name = "PATH=SIZE", action = clap::ArgAction::Append)]
    pub dir_max_size: Vec<String>,

    /// After write, list/test the archive.
    #[arg(long = "verify")]
    pub verify: bool,

    /// Source paths (dirs and/or files). Required unless `--files-from` is set.
    ///
    /// Stored as strings so a trailing `/` is preserved (rsync-style naming).
    #[arg(value_name = "SRC")]
    pub sources: Vec<String>,
}

/// Arguments for `rsync-archive embed`.
#[derive(Debug, Parser)]
pub struct EmbedArgs {
    /// Output master archive path (`.7z`).
    #[arg(short = 'o', long = "output", value_name = "OUT")]
    pub output: PathBuf,

    /// List members without writing.
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// Overwrite `-o` if it already exists.
    #[arg(long = "force")]
    pub force: bool,

    /// Prefix for all member names (relative path, no `..`).
    #[arg(long = "prefix", value_name = "PREFIX")]
    pub prefix: Option<String>,

    /// Use normalized input path as member name (default is basename flatten).
    #[arg(long = "keep-path")]
    pub keep_path: bool,

    /// Hard-error if an input is missing 7z magic.
    #[arg(long = "require-7z", conflicts_with = "allow_any")]
    pub require_7z: bool,

    /// Allow arbitrary regular files as store members; skip magic warning.
    #[arg(long = "allow-any", conflicts_with = "require_7z")]
    pub allow_any: bool,

    /// After write, list/test the archive.
    #[arg(long = "verify")]
    pub verify: bool,

    /// Input files to embed (typically finished `.7z` archives).
    #[arg(value_name = "FILE", required = true)]
    pub inputs: Vec<PathBuf>,
}

impl CreateArgs {
    /// Validate create mode constraints that clap cannot express alone.
    pub fn validate(&self) -> Result<(), String> {
        let has_files_from = self.files_from.is_some();
        let has_sources = !self.sources.is_empty();
        match (has_files_from, has_sources) {
            (true, true) => Err(
                "cannot combine --files-from with SRC... (use one or the other)".into(),
            ),
            (false, false) => Err("need SRC... or --files-from".into()),
            _ => Ok(()),
        }
    }
}
