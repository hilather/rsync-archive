//! Clap CLI surface for `rsync-archive`.
//!
//! **Help text here is user-facing.** Keep `///` docs, `about`, and `after_help`
//! accurate whenever flags or formats change (see `AGENTS.md` CLI policy).
//! Short help (`-h`) shows only the first line of multi-line docs.

use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

/// Create non-solid 7z archives with rsync-style selection; embed finished archives.
#[derive(Debug, Parser)]
#[command(
    name = "rsync-archive",
    version,
    about = "Stream-create archives with rsync-style selection (default: non-solid 7z); embed finished files under a store 7z",
    long_about = "rsync-archive creates archives from the filesystem using rsync-style \
include/exclude filters and optional size/count limits.\n\n\
Default create format is non-solid 7z (per-file packs, random-access friendly). \
Also: seekable-zstd, tar-zstd, tar-lz4.\n\n\
embed wraps finished regular files (typically .7z) under a master store/Copy 7z \
without recompression.",
    after_help = "Examples:\n  \
rsync-archive create -o out.7z --level 5 ./data/\n  \
rsync-archive create -o pack.tar.zst --format tar-zstd ./logs/\n  \
rsync-archive create -o stream.zst --format seekable-zstd ./src/\n  \
rsync-archive embed -o master.7z nest1.7z nest2.7z\n\n\
Use `create -h` / `embed -h` for flags; `create --help` for full long descriptions."
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
    /// Create an archive (7z / seekable-zstd / tar-zstd / tar-lz4) with rsync-style selection.
    Create(CreateArgs),
    /// Embed finished files under a master non-solid store 7z (Copy method, no recompress).
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
#[command(
    about = "Create an archive with rsync-style selection (default format: non-solid 7z)",
    long_about = "Walk SRC paths (or --files-from), apply rsync-style filters and optional \
size/count limits, then stream-create an archive.\n\n\
Formats: 7z (default, --method lzma2|zstd|lz4), seekable-zstd, tar-zstd, tar-lz4. \
Infer format from -o when --format is omitted (.tar.zst/.tzst, .tar.lz4/.tlz4, .zst, else 7z).\n\n\
Tar formats include regular files, derived directory members, symlinks, and Unix hard links. \
7z/seekable-zstd keep regular files only (symlinks/hard-link members skipped).",
    after_help = "Examples:\n  \
rsync-archive create -o out.7z --method zstd --level 5 ./data/\n  \
rsync-archive create -o logs.tar.zst --max-total-size 100M --dir-max-files logs/=50 /var/log/\n  \
rsync-archive create -o o.7z --files-from master.txt --file-size-from sizes.txt --dir-max-size-from dirs.txt\n  \
rsync-archive create -o pack.7z --include-cwd -n   # CWD files at archive root; skip pack.7z\n  \
rsync-archive create -o pack.tlz4 -n ./tree/   # dry-run; .tlz4 → tar-lz4\n  \
rsync-archive create -o o.7z --exclude '*.tmp' --newer-than 7d ./src/\n  \
rsync-archive create -n -o o.7z --filter-from rules.txt ./src/   # ordered +/- file\n\n\
Filters: use --filter / --filter-from for ordered mixes; --include then --exclude are batched \
(not CLI-interleaved). See docs/SELECTION.md and docs/RSYNC_PARITY.md."
)]
pub struct CreateArgs {
    /// Output path (`.7z`, `.zst`, `.tar.zst`/`.tzst`, `.tar.lz4`/`.tlz4`).
    #[arg(short = 'o', long = "output", value_name = "OUT")]
    pub output: PathBuf,

    /// Format: `7z` (default), `seekable-zstd`, `tar-zstd`, `tar-lz4` (or infer from `-o`).
    ///
    /// Inference: `.tar.zst`/`.tzst` → tar-zstd; `.tar.lz4`/`.tlz4` → tar-lz4;
    /// bare `.zst` → seekable-zstd; otherwise 7z.
    #[arg(
        long = "format",
        visible_alias = "output-format",
        value_name = "FMT",
        value_enum
    )]
    pub format: Option<OutputFormat>,

    /// Dry-run: print selected archive paths; do not write `-o`.
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// Overwrite `-o` if it already exists (otherwise error).
    #[arg(long = "force")]
    pub force: bool,

    /// Exclude pattern (rsync-style; repeatable; basename match if no `/`).
    ///
    /// All `--include` patterns are applied as a batch **before** all `--exclude`
    /// patterns (clap does not interleave heterogeneous flags). Prefer `--filter` or
    /// `--filter-from` when rule order must mix include and exclude.
    #[arg(long = "exclude", value_name = "PATTERN", action = clap::ArgAction::Append)]
    pub exclude: Vec<String>,

    /// Include pattern (rsync-style; repeatable).
    ///
    /// Batched before all `--exclude` (not interleaved with them). Prefer `--filter`
    /// / `--filter-from` for rsync-style ordered mixes (e.g. exclude-all then include).
    #[arg(long = "include", value_name = "PATTERN", action = clap::ArgAction::Append)]
    pub include: Vec<String>,

    /// Exclude patterns from file (one per line; `#` comments; repeatable).
    ///
    /// Bare lines default to exclude; `+`/`-`/`include`/`exclude` prefixes override.
    /// Multiple files load in CLI order. Rule build order: include-from → exclude-from
    /// → filter-from → filter → include → exclude.
    #[arg(long = "exclude-from", value_name = "FILE", action = clap::ArgAction::Append)]
    pub exclude_from: Vec<PathBuf>,

    /// Include patterns from file (one per line; `#` comments; repeatable).
    ///
    /// Bare lines default to include; `+`/`-`/`include`/`exclude` prefixes override.
    /// Multiple files load in CLI order (all include-from before all exclude-from).
    #[arg(long = "include-from", value_name = "FILE", action = clap::ArgAction::Append)]
    pub include_from: Vec<PathBuf>,

    /// Explicit path list (exclusive of SRC...; relative to CWD unless absolute).
    #[arg(long = "files-from", value_name = "FILE")]
    pub files_from: Option<PathBuf>,

    /// Also pack all files under the process CWD at archive root (trailing-`/` style).
    ///
    /// Off by default. Skips the `-o` output file and its `.partial` temp. Combines
    /// with SRC... or `--files-from`, or may be used alone.
    /// **Ignores rsync include/exclude filters** (those apply only to SRC/`--files-from`);
    /// otherwise a trailing `- *` in a filter file would drop every CWD root file.
    #[arg(long = "include-cwd", default_value_t = false)]
    pub include_cwd: bool,

    /// Ordered filter rules from file (`+`/`-`/`include`/`exclude` lines; repeatable).
    ///
    /// Best rsync-like path for a full ordered rule list (analogue of merge-file without
    /// dir-merge). Multiple files append in CLI order. Loaded after include-from/exclude-from
    /// and before CLI `--filter`.
    #[arg(long = "filter-from", value_name = "FILE", action = clap::ArgAction::Append)]
    pub filter_from: Vec<PathBuf>,

    /// Filter rule such as `+ *.rs` or `- *.tmp` (repeatable; CLI order preserved).
    ///
    /// Prefer this or `--filter-from` when include/exclude must interleave; `--include`
    /// and `--exclude` are batched separately.
    #[arg(long = "filter", value_name = "RULE", action = clap::ArgAction::Append)]
    pub filter: Vec<String>,

    /// Compression level 0–9 (default 5; LZMA2/Zstd mapping; LZ4 mostly ignores).
    #[arg(long = "level", default_value_t = 5, value_parser = clap::value_parser!(u32).range(0..=9))]
    pub level: u32,

    /// 7z-only method: `lzma2` (default), `zstd`, or `lz4` (non-solid packs; error on tar formats).
    #[arg(long = "method", default_value = "lzma2")]
    pub method: String,

    /// 7z encode workers (omit = auto: many tiny files → 1, else CPU count).
    #[arg(long)]
    pub threads: Option<u32>,

    /// 7z max concurrent file encodes (`0` = auto from `--threads` / CPUs).
    #[arg(long = "encode-concurrency", default_value_t = 0)]
    pub encode_concurrency: usize,

    /// 7z max in-flight uncompressed encode size (default `500M`; `0` = unlimited).
    #[arg(long = "encode-size-budget", default_value = "500M")]
    pub encode_size_budget: String,

    /// Cap selected bytes under dir PATH (recursive; `PATH=SIZE`; newest-mtime first).
    ///
    /// Example: `logs/=100M`. Nested budgets: longest matching prefix wins.
    /// Applied after filters and per-file size/age limits. Only listed dirs are capped.
    #[arg(long = "dir-max-size", value_name = "PATH=SIZE", action = clap::ArgAction::Append)]
    pub dir_max_size: Vec<String>,

    /// Dir size/count list file (rsync-like: `logs/ max=500M`, `logs/ files=100`, or `logs/=100M`).
    ///
    /// Only prefixes in the file are restricted; other master-list paths ignore this list.
    /// `files=N` lines merge with `--dir-max-files` / `--dir-max-files-from`.
    #[arg(long = "dir-max-size-from", value_name = "FILE")]
    pub dir_max_size_from: Option<PathBuf>,

    /// Cap selected file count under dir PATH (recursive; `PATH=N`; newest-mtime first).
    ///
    /// Example: `logs/=10`. Nested limits: longest matching prefix wins.
    /// Applied after filters, per-file limits, and size budgets.
    #[arg(long = "dir-max-files", value_name = "PATH=N", action = clap::ArgAction::Append)]
    pub dir_max_files: Vec<String>,

    /// Dir file-count list (`PATH=N` or `PATH/ files=N`; `#` comments OK).
    #[arg(long = "dir-max-files-from", value_name = "FILE")]
    pub dir_max_files_from: Option<PathBuf>,

    /// Per-path max-size list (rsync-like: `**/*.log max=100M`). Only matching paths capped.
    ///
    /// First matching line wins. Paths not matched by any line ignore this list.
    #[arg(long = "file-size-from", value_name = "FILE")]
    pub file_size_from: Option<PathBuf>,

    /// Global cap on selected uncompressed bytes (newest-mtime first; after dir limits).
    #[arg(long = "max-total-size", value_name = "SIZE")]
    pub max_total_size: Option<String>,

    /// Global max selected file count (newest-mtime first).
    #[arg(long = "max-files", value_name = "N")]
    pub max_files: Option<u64>,

    /// Skip any single file larger than SIZE (global; e.g. `100M`). Prefer `--file-size-from` for per-path.
    #[arg(long = "max-size", value_name = "SIZE")]
    pub max_size: Option<String>,

    /// Skip files smaller than SIZE (`0` or omit = off). Global only.
    #[arg(long = "min-size", value_name = "SIZE")]
    pub min_size: Option<String>,

    /// Keep only files with mtime within DURATION (e.g. `7d`, `24h`, `30m`, `90s`).
    #[arg(long = "newer-than", value_name = "DURATION")]
    pub newer_than: Option<String>,

    /// After write, verify archive (member count / sample extract by format).
    #[arg(long = "verify")]
    pub verify: bool,

    /// Source paths (dirs/files). Required unless `--files-from` or `--include-cwd`.
    /// Trailing `/` strips dir name.
    #[arg(value_name = "SRC")]
    pub sources: Vec<String>,
}

/// Arguments for `rsync-archive embed`.
#[derive(Debug, Parser)]
#[command(
    about = "Embed finished files under a master non-solid store 7z (Copy, no recompress)",
    long_about = "Wrap finished regular files (typically non-solid .7z nested archives) under \
a master 7z using the store/Copy method (no recompression).\n\n\
Default member names are basenames; use --keep-path and optional --prefix for tree names.",
    after_help = "Examples:\n  \
rsync-archive embed -o master.7z nest1.7z nest2.7z\n  \
rsync-archive embed -o master.7z --keep-path --prefix packs/ ./build/a.7z\n  \
rsync-archive embed -o master.7z --require-7z --verify a.7z b.7z"
)]
pub struct EmbedArgs {
    /// Output master archive path (`.7z`).
    #[arg(short = 'o', long = "output", value_name = "OUT")]
    pub output: PathBuf,

    /// Dry-run: list planned member names; do not write `-o`.
    #[arg(short = 'n', long = "dry-run")]
    pub dry_run: bool,

    /// Overwrite `-o` if it already exists (otherwise error).
    #[arg(long = "force")]
    pub force: bool,

    /// Prefix all member names (relative path; no `..`).
    #[arg(long = "prefix", value_name = "PREFIX")]
    pub prefix: Option<String>,

    /// Keep normalized input path as member name (default: basename only).
    #[arg(long = "keep-path")]
    pub keep_path: bool,

    /// Error if an input is missing 7z magic (conflicts with `--allow-any`).
    #[arg(long = "require-7z", conflicts_with = "allow_any")]
    pub require_7z: bool,

    /// Allow any regular file as a store member; skip non-7z magic warning.
    #[arg(long = "allow-any", conflicts_with = "require_7z")]
    pub allow_any: bool,

    /// After write, verify master archive (list/test members).
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
            (false, false) if !self.include_cwd => {
                return Err("need SRC..., --files-from, or --include-cwd".into());
            }
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
            exclude_from: vec![],
            include_from: vec![],
            files_from: None,
            include_cwd: false,
            filter_from: vec![],
            filter: vec![],
            level: 5,
            method: "zstd".into(),
            threads: None,
            encode_concurrency: 0,
            encode_size_budget: "500M".into(),
            dir_max_size: vec![],
            dir_max_size_from: None,
            dir_max_files: vec![],
            dir_max_files_from: None,
            file_size_from: None,
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
