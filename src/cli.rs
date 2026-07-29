//! Clap CLI surface for `rsync-archive`.
//!
//! Flag sets for `create` and `embed` follow `docs/DESIGN.md` (v1 freeze).
//! Pipelines are stubbed until later stages.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

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
    /// Create a non-solid 7z (or seekable-zstd stream) from filesystem paths using rsync-style filters.
    Create(CreateArgs),
    /// Embed finished archive files under a master non-solid store 7z (Copy method).
    Embed(EmbedArgs),
}

/// Output container format for `create`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Default)]
pub enum OutputFormat {
    /// Non-solid 7z with per-file packs (default).
    #[default]
    #[value(name = "7z")]
    SevenZ,
    /// Single seekable-zstd stream with member index (byte-range access).
    #[value(name = "seekable-zstd", alias = "zst")]
    SeekableZstd,
    /// Valid tar payload in seekable Zstd + RATARIDX1 member index (RA-friendly).
    #[value(name = "tar-zstd", alias = "tar.zst", alias = "tarzst")]
    TarZstd,
    /// Valid tar payload in multi-frame LZ4 + RATLFRM1 frame table + RATAIDX1 (RA-friendly).
    #[value(name = "tar-lz4", alias = "tar.lz4", alias = "tarlz4")]
    TarLz4,
}

impl OutputFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            OutputFormat::SevenZ => "7z",
            OutputFormat::SeekableZstd => "seekable-zstd",
            OutputFormat::TarZstd => "tar-zstd",
            OutputFormat::TarLz4 => "tar-lz4",
        }
    }
}

/// Arguments for `rsync-archive create`.
#[derive(Debug, Parser)]
pub struct CreateArgs {
    /// Output archive path (`.7z`, `.zst`, `.tar.zst`, `.tzst`, `.tar.lz4`, `.tlz4`).
    #[arg(short = 'o', long = "output", value_name = "OUT")]
    pub output: PathBuf,

    /// Output format: `7z` (default), `seekable-zstd`, `tar-zstd`, or `tar-lz4`.
    /// If omitted, inferred from `-o` extension
    /// (`.tar.zst`/`.tzst` → tar-zstd; `.tar.lz4`/`.tlz4` → tar-lz4;
    /// bare `.zst` → seekable-zstd; else 7z).
    #[arg(
        long = "format",
        visible_alias = "output-format",
        value_name = "FMT",
        value_enum
    )]
    pub format: Option<OutputFormat>,

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

    /// Compression level 0–9 (default 5). Meaning depends on format/`--method`
    /// (LZMA2 preset / mapped Zstd level; LZ4 ignores fine-grained levels).
    #[arg(long = "level", default_value_t = 5, value_parser = clap::value_parser!(u32).range(0..=9))]
    pub level: u32,

    /// Compression method for **7z** format only: `lzma2` (default), `zstd`, or `lz4`.
    /// All produce **non-solid** per-file packs (file-level random access).
    /// Not used with `--format seekable-zstd` / `tar-zstd` / `tar-lz4`
    /// (error if set to a non-default value).
    #[arg(long = "method", default_value = "lzma2")]
    pub method: String,

    /// Encode worker count (archiveconverter-style). Omit for auto:
    /// many tiny files → 1; else available CPU parallelism.
    /// Applies to **7z** create only.
    #[arg(long)]
    pub threads: Option<u32>,

    /// Max concurrent file encodes (`0` = auto from `--threads` / CPUs).
    /// Same idea as archiveconverter `--nested-concurrency`. **7z** only.
    #[arg(long = "encode-concurrency", default_value_t = 0)]
    pub encode_concurrency: usize,

    /// Max total **uncompressed** size of files encoding at once (default `500M`).
    /// `0` = no size cap. Same default as archiveconverter `--nested-size-budget`. **7z** only.
    #[arg(long = "encode-size-budget", default_value = "500M")]
    pub encode_size_budget: String,

    /// Cap total selected bytes under an archive-relative directory (repeatable).
    ///
    /// Format: `PATH=SIZE` (e.g. `logs/=100M`, `cache=50M`). After normal filters,
    /// files under `PATH` are considered newest-mtime-first; further files that
    /// would exceed the budget are skipped (counted as dir-budget skips).
    /// Nested budgets: longest matching prefix wins. Scope is **recursive**.
    #[arg(long = "dir-max-size", value_name = "PATH=SIZE", action = clap::ArgAction::Append)]
    pub dir_max_size: Vec<String>,

    /// Cap number of selected files that are **direct children** of a directory
    /// (repeatable). Nested files under subdirectories are not counted.
    ///
    /// Format: `PATH=N` (e.g. `logs/=10`, `cache=5`). After filters (and size
    /// budgets), direct children of `PATH` are considered newest-mtime-first;
    /// only the `N` newest are kept.
    #[arg(long = "dir-max-files", value_name = "PATH=N", action = clap::ArgAction::Append)]
    pub dir_max_files: Vec<String>,

    /// Read `--dir-max-files` lines from a file (`PATH=N` per line; `#` comments OK).
    #[arg(long = "dir-max-files-from", value_name = "FILE")]
    pub dir_max_files_from: Option<PathBuf>,

    /// Global cap on total selected uncompressed bytes (newest-mtime-first fill).
    ///
    /// Applied after filters, per-file size/age limits, and directory budgets.
    /// Further files that would exceed the budget are skipped (compact report).
    #[arg(long = "max-total-size", value_name = "SIZE")]
    pub max_total_size: Option<String>,

    /// Global max number of selected files (newest-mtime-first).
    #[arg(long = "max-files", value_name = "N")]
    pub max_files: Option<u64>,

    /// Skip any single file larger than SIZE (e.g. `100M`).
    #[arg(long = "max-size", value_name = "SIZE")]
    pub max_size: Option<String>,

    /// Skip files smaller than SIZE (`0` or omit = off).
    #[arg(long = "min-size", value_name = "SIZE")]
    pub min_size: Option<String>,

    /// Only files with mtime within the last DURATION (e.g. `7d`, `24h`, `30m`, `90s`).
    #[arg(long = "newer-than", value_name = "DURATION")]
    pub newer_than: Option<String>,

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
            (true, true) => {
                return Err(
                    "cannot combine --files-from with SRC... (use one or the other)".into(),
                );
            }
            (false, false) => return Err("need SRC... or --files-from".into()),
            _ => {}
        }

        let format = self.resolved_format();
        if matches!(
            format,
            OutputFormat::SeekableZstd | OutputFormat::TarZstd | OutputFormat::TarLz4
        ) {
            // `--method` defaults to lzma2; only reject non-default values so
            // non-7z formats can be used without forcing users to omit --method.
            if self.method != "lzma2" {
                return Err(format!(
                    "--method is for 7z format only (got --method {}); omit --method with --format {}",
                    self.method,
                    format.as_str()
                ));
            }
        }
        Ok(())
    }

    /// Resolve output format: explicit `--format`, else infer from `-o` extension.
    ///
    /// - `.tar.zst` / `.tzst` → tar-zstd
    /// - `.tar.lz4` / `.tlz4` → tar-lz4
    /// - bare `.zst` → seekable-zstd
    /// - otherwise → 7z (including `.7z` and extensionless paths)
    pub fn resolved_format(&self) -> OutputFormat {
        if let Some(f) = self.format {
            return f;
        }
        infer_format_from_path(&self.output)
    }
}

/// Infer create output format from the output path extension.
pub fn infer_format_from_path(path: &Path) -> OutputFormat {
    let s = path.to_string_lossy().to_ascii_lowercase();
    if s.ends_with(".tar.zst") || s.ends_with(".tzst") {
        return OutputFormat::TarZstd;
    }
    if s.ends_with(".tar.lz4") || s.ends_with(".tlz4") {
        return OutputFormat::TarLz4;
    }
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("zst") => OutputFormat::SeekableZstd,
        _ => OutputFormat::SevenZ,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_zst_extension() {
        assert_eq!(
            infer_format_from_path(Path::new("out.zst")),
            OutputFormat::SeekableZstd
        );
        assert_eq!(
            infer_format_from_path(Path::new("out.ZST")),
            OutputFormat::SeekableZstd
        );
        assert_eq!(
            infer_format_from_path(Path::new("out.tar.zst")),
            OutputFormat::TarZstd
        );
        assert_eq!(
            infer_format_from_path(Path::new("out.tzst")),
            OutputFormat::TarZstd
        );
        assert_eq!(
            infer_format_from_path(Path::new("out.tar.lz4")),
            OutputFormat::TarLz4
        );
        assert_eq!(
            infer_format_from_path(Path::new("out.tlz4")),
            OutputFormat::TarLz4
        );
        assert_eq!(
            infer_format_from_path(Path::new("out.7z")),
            OutputFormat::SevenZ
        );
        assert_eq!(
            infer_format_from_path(Path::new("out")),
            OutputFormat::SevenZ
        );
    }

    #[test]
    fn seekable_rejects_non_default_method() {
        let args = CreateArgs {
            output: PathBuf::from("o.zst"),
            format: Some(OutputFormat::SeekableZstd),
            dry_run: true,
            force: false,
            exclude: vec![],
            include: vec![],
            exclude_from: None,
            include_from: None,
            files_from: None,
            filter: vec![],
            level: 5,
            method: "zstd".into(),
            threads: None,
            encode_concurrency: 0,
            encode_size_budget: "500M".into(),
            dir_max_size: vec![],
            dir_max_files: vec![],
            dir_max_files_from: None,
            max_total_size: None,
            max_files: None,
            max_size: None,
            min_size: None,
            newer_than: None,
            verify: false,
            sources: vec![".".into()],
        };
        let err = args.validate().unwrap_err();
        assert!(err.contains("--method"), "{err}");
    }
}
