//! Create pipeline: selection, dry-run, and non-solid 7z or seekable-zstd write.
//!
//! Parallel encode (archiveconverter-style) for **7z** format:
//! - `--threads` omit = auto (many tiny files → 1 worker; else CPU count)
//! - `--encode-concurrency 0` = auto from threads
//! - `--encode-size-budget` default `500M` (in-flight uncompressed bytes)
//!
//! **seekable-zstd** streams members into a zeekstd encoder (see
//! `docs/FORMAT_SEEKABLE_ZSTD.md`).

use crate::archive::sevenz::{
    compress_path_with_size, filetime_from_unix_secs, CompressedPack, CompressMethod,
};
use crate::archive::{write_seekable_zstd, write_tar_lz4, write_tar_zstd, NonsolidLzma2Writer};
use crate::cli::{CreateArgs, OutputFormat};
use crate::error::{Error, Result};
use crate::pipeline::output::{
    cleanup_partial, commit_output, partial_path_for, prepare_output,
};
use crate::select::dir_budget::{
    apply_dir_budgets, apply_dir_file_limits, collect_dir_budgets, collect_dir_file_limits,
    RestrictionReport,
};
use crate::select::from_file::{load_exclude_from, load_include_from};
use crate::select::global_restrict::{
    apply_max_files, apply_max_total_size, apply_per_file_limits, per_file_limits_from_cli,
};
use crate::select::restrict_lists::{apply_file_size_from, load_file_size_from};
use crate::select::walk::{
    collect_from_files_from, collect_from_sources, MemberKind, SelectedEntry, SelectionStats,
};
use crate::select::{RuleSet, SourceSpec};
use crate::util::{
    can_admit, file_stats_from_sizes, parse_byte_size, resolve_encode_concurrency,
    resolve_encode_workers,
};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Instant;
use tracing::{debug, info, warn};

/// Build the ordered filter set from create CLI flags.
///
/// **Rule order (v1):**
/// 1. `--include-from` (file line order)
/// 2. `--exclude-from` (file line order)
/// 3. `--filter` (CLI order)
/// 4. `--include` (CLI order among includes)
/// 5. `--exclude` (CLI order among excludes)
///
/// Clap does not preserve interleaving of different long options. For strict
/// rsync-style interleaving of include/exclude, use repeated `--filter`.
pub fn build_rules(args: &CreateArgs) -> Result<RuleSet> {
    let mut rules = RuleSet::new();
    if let Some(path) = &args.include_from {
        load_include_from(&mut rules, path)?;
    }
    if let Some(path) = &args.exclude_from {
        load_exclude_from(&mut rules, path)?;
    }
    for line in &args.filter {
        rules.push_filter_line(line)?;
    }
    for pat in &args.include {
        rules.push_include(pat)?;
    }
    for pat in &args.exclude {
        rules.push_exclude(pat)?;
    }
    Ok(rules)
}

/// Build the full selection (same path for dry-run and write).
///
/// Order of operations:
/// 1. Master list: rsync filters / walk or `--files-from`, optional `--include-cwd`
/// 2. Global per-file `--max-size` / `--min-size` / `--newer-than`
/// 3. `--file-size-from` (only paths matching a list line; first match wins)
/// 4. `--dir-max-size` / `--dir-max-size-from` then `--dir-max-files` /
///    `--dir-max-files-from` (only listed directory prefixes)
/// 5. Global `--max-total-size` / `--max-files` (newest-first)
///
/// Restriction list files only constrain matching paths/prefixes; other master
/// entries ignore that list.
pub fn build_selection(
    args: &CreateArgs,
) -> Result<(Vec<SelectedEntry>, SelectionStats, RestrictionReport)> {
    let rules = build_rules(args)?;
    let (mut entries, mut stats) = if let Some(list) = &args.files_from {
        collect_from_files_from(list, &rules)?
    } else if !args.sources.is_empty() {
        let mut specs = Vec::with_capacity(args.sources.len());
        for s in &args.sources {
            specs.push(SourceSpec::from_user_path(s)?);
        }
        collect_from_sources(&specs, &rules)?
    } else {
        (Vec::new(), SelectionStats::default())
    };

    if args.include_cwd {
        let (cwd_entries, cwd_stats) = collect_include_cwd(&rules, &args.output)?;
        merge_selection(&mut entries, &mut stats, cwd_entries, cwd_stats)?;
    }

    let mut report = RestrictionReport::default();

    // 2. Global per-file size / age filters.
    let per_file = per_file_limits_from_cli(
        args.max_size.as_deref(),
        args.min_size.as_deref(),
        args.newer_than.as_deref(),
    )?;
    let entries = apply_per_file_limits(entries, &per_file, &mut stats, &mut report)?;

    // 3. Per-path file size list (only matching paths).
    let entries = if let Some(ref path) = args.file_size_from {
        let file_rules = load_file_size_from(path)?;
        apply_file_size_from(entries, &file_rules, &mut stats, &mut report)?
    } else {
        entries
    };

    // 4. Directory budgets and file-count limits (listed prefixes only).
    let mut file_limits =
        collect_dir_file_limits(&args.dir_max_files, args.dir_max_files_from.as_deref())?;
    let budgets = collect_dir_budgets(
        &args.dir_max_size,
        args.dir_max_size_from.as_deref(),
        &mut file_limits,
    )?;
    let entries = apply_dir_budgets(entries, &budgets, &mut stats, &mut report)?;
    let entries = apply_dir_file_limits(entries, &file_limits, &mut stats, &mut report)?;

    // 5. Global caps (newest-mtime-first).
    let entries = if let Some(ref s) = args.max_total_size {
        let limit = parse_byte_size(s)?;
        apply_max_total_size(entries, limit, &mut stats, &mut report)?
    } else {
        entries
    };
    let entries = if let Some(n) = args.max_files {
        apply_max_files(entries, n, &mut stats, &mut report)?
    } else {
        entries
    };

    Ok((entries, stats, report))
}

/// Walk process CWD with trailing-slash semantics (members at archive root).
///
/// Excludes the create `-o` path and its `.partial` sibling so the tool does not
/// archive its own output/temp.
fn collect_include_cwd(
    rules: &RuleSet,
    output: &Path,
) -> Result<(Vec<SelectedEntry>, SelectionStats)> {
    let cwd = std::env::current_dir().map_err(|e| {
        Error::Selection(format!("cwd for --include-cwd: {e}"))
    })?;
    let spec = SourceSpec {
        path: cwd,
        original: "./".into(),
        trailing_slash: true,
        kind: crate::select::SourceKind::Dir,
    };
    let (entries, mut stats) = collect_from_sources(&[spec], rules)?;
    let skip = output_artifact_paths(output);
    let before = entries.len();
    let filtered: Vec<SelectedEntry> = entries
        .into_iter()
        .filter(|e| !is_output_artifact(&e.abs_path, &skip))
        .collect();
    let dropped = before.saturating_sub(filtered.len());
    if dropped > 0 {
        // Counted as excluded rather than special; these are intentional self-skips.
        stats.skipped_excluded = stats.skipped_excluded.saturating_add(dropped as u64);
        debug!(
            dropped,
            output = %output.display(),
            "include-cwd skipped self output/partial"
        );
    }
    stats.selected = filtered.len() as u64;
    Ok((filtered, stats))
}

/// Paths to skip when packing CWD: final `-o` and `{out}.partial` (absolute form).
fn output_artifact_paths(output: &Path) -> (PathBuf, PathBuf) {
    let final_abs = std::path::absolute(output).unwrap_or_else(|_| output.to_path_buf());
    let partial = partial_path_for(output);
    let partial_abs = std::path::absolute(&partial).unwrap_or(partial);
    (final_abs, partial_abs)
}

fn is_output_artifact(abs: &Path, skip: &(PathBuf, PathBuf)) -> bool {
    let abs = std::path::absolute(abs).unwrap_or_else(|_| abs.to_path_buf());
    paths_equal(&abs, &skip.0) || paths_equal(&abs, &skip.1)
}

fn paths_equal(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    // Best-effort when both exist: same file via canonicalize.
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

/// Append `extra` into `base`, merging stats; collision on archive_name errors.
fn merge_selection(
    base: &mut Vec<SelectedEntry>,
    base_stats: &mut SelectionStats,
    extra: Vec<SelectedEntry>,
    extra_stats: SelectionStats,
) -> Result<()> {
    use std::collections::HashSet;
    let mut names: HashSet<String> = base.iter().map(|e| e.archive_name.clone()).collect();
    for e in extra {
        if !names.insert(e.archive_name.clone()) {
            return Err(Error::Collision(e.archive_name));
        }
        base.push(e);
    }
    base_stats.selected = base.len() as u64;
    base_stats.skipped_excluded = base_stats
        .skipped_excluded
        .saturating_add(extra_stats.skipped_excluded);
    base_stats.skipped_symlinks = base_stats
        .skipped_symlinks
        .saturating_add(extra_stats.skipped_symlinks);
    base_stats.skipped_hardlinks = base_stats
        .skipped_hardlinks
        .saturating_add(extra_stats.skipped_hardlinks);
    base_stats.skipped_special = base_stats
        .skipped_special
        .saturating_add(extra_stats.skipped_special);
    Ok(())
}

/// Drop link members for formats that only pack regular files (7z, seekable-zstd).
///
/// Tar formats keep symlinks and hard-link members. Updates
/// `stats.skipped_symlinks` / `stats.skipped_hardlinks` and `stats.selected`.
/// The first regular-file copy of a hard-linked inode is kept as
/// [`MemberKind::File`]; subsequent hard-link members are dropped here.
fn filter_entries_for_format(
    format: OutputFormat,
    entries: Vec<SelectedEntry>,
    stats: &mut SelectionStats,
) -> Vec<SelectedEntry> {
    match format {
        OutputFormat::TarZstd | OutputFormat::TarLz4 => entries,
        OutputFormat::SevenZ | OutputFormat::SeekableZstd => {
            let mut kept = Vec::with_capacity(entries.len());
            for e in entries {
                match e.kind {
                    MemberKind::File => kept.push(e),
                    MemberKind::Symlink { .. } => {
                        stats.skipped_symlinks += 1;
                        debug!(
                            name = %e.archive_name,
                            format = format.as_str(),
                            "skip symlink for non-tar format"
                        );
                    }
                    MemberKind::HardLink { .. } => {
                        stats.skipped_hardlinks += 1;
                        debug!(
                            name = %e.archive_name,
                            format = format.as_str(),
                            "skip hard link for non-tar format"
                        );
                    }
                }
            }
            stats.selected = kept.len() as u64;
            kept
        }
    }
}

/// Run `rsync-archive create`.
///
/// Dry-run lists archive names and exit 0 even when empty.
/// Write builds a non-solid 7z, seekable-zstd, tar.zst, or tar.lz4 stream via partial+rename.
///
/// Symlinks and hard links are selected at walk time; **tar-zstd** / **tar-lz4**
/// archive them as typeflag `'2'` / `'1'` members; **7z** / **seekable-zstd**
/// skip them (counted in `skipped_symlinks` / `skipped_hardlinks`), keeping only
/// the first regular-file body for each hard-linked inode.
pub fn run_create(args: CreateArgs) -> Result<()> {
    let format = args.resolved_format();
    let (entries, mut stats, restrictions) = build_selection(&args)?;
    let entries = filter_entries_for_format(format, entries, &mut stats);

    if args.dry_run {
        for e in &entries {
            println!("{}", e.archive_name);
        }
        restrictions.eprint_compact();
        print_selection_summary(&stats, true);
        info!(
            format = format.as_str(),
            selected = stats.selected,
            skipped_excluded = stats.skipped_excluded,
            skipped_dir_budget = stats.skipped_dir_budget,
            skipped_dir_file_limit = stats.skipped_dir_file_limit,
            skipped_max_size = stats.skipped_max_size,
            skipped_file_size_from = stats.skipped_file_size_from,
            skipped_min_size = stats.skipped_min_size,
            skipped_older_than = stats.skipped_older_than,
            skipped_max_total_size = stats.skipped_max_total_size,
            skipped_max_files = stats.skipped_max_files,
            skipped_symlinks = stats.skipped_symlinks,
            skipped_hardlinks = stats.skipped_hardlinks,
            skipped_special = stats.skipped_special,
            "create dry-run complete"
        );
        return Ok(());
    }

    if entries.is_empty() {
        // Still emit restriction report so operators see why nothing was kept.
        restrictions.eprint_compact();
        print_selection_summary(&stats, false);
        return Err(Error::EmptyArchive);
    }

    match format {
        OutputFormat::SevenZ => run_create_sevenz(&args, &entries, &stats, &restrictions),
        OutputFormat::SeekableZstd => {
            run_create_seekable_zstd(&args, &entries, &stats, &restrictions)
        }
        OutputFormat::TarZstd => run_create_tar_zstd(&args, &entries, &stats, &restrictions),
        OutputFormat::TarLz4 => run_create_tar_lz4(&args, &entries, &stats, &restrictions),
    }
}

fn run_create_sevenz(
    args: &CreateArgs,
    entries: &[SelectedEntry],
    stats: &SelectionStats,
    restrictions: &RestrictionReport,
) -> Result<()> {
    let t0 = Instant::now();
    let method = CompressMethod::parse(&args.method)?;
    let (n, total_bytes) = file_stats_from_sizes(entries.iter().map(|e| e.size));
    let workers = resolve_encode_workers(args.threads, n, total_bytes);
    let concurrency = resolve_encode_concurrency(args.encode_concurrency, workers);
    let size_budget = parse_byte_size(&args.encode_size_budget)?;

    // OPT-06: warn when explicit multi-thread on many-tiny (pool helps but still may not win).
    if args.threads.is_some() && concurrency > 1 && n >= 1000 {
        let avg = if n > 0 { total_bytes / n as u64 } else { 0 };
        if avg < 64 * 1024 {
            warn!(
                concurrency,
                file_count = n,
                avg_bytes = avg,
                "many tiny files: multi-thread may not beat single-thread (worker pool still used)"
            );
        }
    }

    // OPT-11: spare cores for large-member zstd MT when file-level concurrency is 1.
    let zstd_nb_workers = if method == CompressMethod::Zstd && concurrency <= 1 {
        std::thread::available_parallelism()
            .map(|p| p.get() as u32)
            .unwrap_or(1)
            .min(4)
            .max(1)
    } else {
        1
    };

    info!(
        format = "7z",
        method = method.as_str(),
        workers,
        concurrency,
        size_budget,
        file_count = n,
        total_bytes,
        zstd_nb_workers,
        "create encode schedule"
    );

    let paths = prepare_output(&args.output, args.force)?;
    let t_enc = Instant::now();
    match write_create_archive(
        &paths.partial_path,
        entries,
        args.level,
        method,
        concurrency,
        size_budget,
        zstd_nb_workers,
    ) {
        Ok(()) => {
            let encode_ms = t_enc.elapsed().as_millis();
            commit_output(&paths)?;
            restrictions.eprint_compact();
            print_selection_summary(stats, false);
            if timings_enabled() {
                eprintln!(
                    "timings: total={}ms encode={}ms members={} concurrency={}",
                    t0.elapsed().as_millis(),
                    encode_ms,
                    entries.len(),
                    concurrency
                );
            }
            info!(
                path = %paths.final_path.display(),
                count = entries.len(),
                level = args.level,
                method = method.as_str(),
                concurrency,
                "create complete"
            );
            eprintln!(
                "created {} member(s) → {} (format 7z, method {}, concurrency {})",
                entries.len(),
                paths.final_path.display(),
                method.as_str(),
                concurrency
            );
            if args.verify {
                verify_create_archive(&paths.final_path, entries.len())?;
            }
            Ok(())
        }
        Err(e) => {
            cleanup_partial(&paths);
            Err(e)
        }
    }
}

fn timings_enabled() -> bool {
    std::env::var_os("RSYNC_ARCHIVE_TIMINGS").is_some()
}

fn run_create_tar_zstd(
    args: &CreateArgs,
    entries: &[SelectedEntry],
    stats: &SelectionStats,
    restrictions: &RestrictionReport,
) -> Result<()> {
    info!(
        format = "tar-zstd",
        level = args.level,
        file_count = entries.len(),
        "create tar.zst"
    );

    let paths = prepare_output(&args.output, args.force)?;
    match write_tar_zstd(&paths.partial_path, entries, args.level) {
        Ok(()) => {
            commit_output(&paths)?;
            restrictions.eprint_compact();
            print_selection_summary(stats, false);
            let member_count =
                crate::archive::tar_common::expected_tar_member_count(entries);
            info!(
                path = %paths.final_path.display(),
                file_count = entries.len(),
                member_count,
                level = args.level,
                "create tar.zst complete"
            );
            eprintln!(
                "created {member_count} member(s) ({} file(s)) → {} (format tar-zstd, level {})",
                entries.len(),
                paths.final_path.display(),
                args.level
            );
            if args.verify {
                crate::archive::verify_tar_zstd(&paths.final_path, member_count)?;
                eprintln!("verify ok: {member_count} member(s), tar-zstd");
            }
            Ok(())
        }
        Err(e) => {
            cleanup_partial(&paths);
            Err(e)
        }
    }
}

fn run_create_tar_lz4(
    args: &CreateArgs,
    entries: &[SelectedEntry],
    stats: &SelectionStats,
    restrictions: &RestrictionReport,
) -> Result<()> {
    info!(
        format = "tar-lz4",
        level = args.level,
        file_count = entries.len(),
        "create tar.lz4"
    );

    let paths = prepare_output(&args.output, args.force)?;
    match write_tar_lz4(&paths.partial_path, entries, args.level) {
        Ok(()) => {
            commit_output(&paths)?;
            restrictions.eprint_compact();
            print_selection_summary(stats, false);
            let member_count =
                crate::archive::tar_common::expected_tar_member_count(entries);
            info!(
                path = %paths.final_path.display(),
                file_count = entries.len(),
                member_count,
                level = args.level,
                "create tar.lz4 complete"
            );
            eprintln!(
                "created {member_count} member(s) ({} file(s)) → {} (format tar-lz4, level {})",
                entries.len(),
                paths.final_path.display(),
                args.level
            );
            if args.verify {
                crate::archive::verify_tar_lz4(&paths.final_path, member_count)?;
                eprintln!("verify ok: {member_count} member(s), tar-lz4");
            }
            Ok(())
        }
        Err(e) => {
            cleanup_partial(&paths);
            Err(e)
        }
    }
}

fn run_create_seekable_zstd(
    args: &CreateArgs,
    entries: &[SelectedEntry],
    stats: &SelectionStats,
    restrictions: &RestrictionReport,
) -> Result<()> {
    info!(
        format = "seekable-zstd",
        level = args.level,
        file_count = entries.len(),
        "create seekable-zstd"
    );

    let paths = prepare_output(&args.output, args.force)?;
    match write_seekable_zstd(&paths.partial_path, entries, args.level) {
        Ok(()) => {
            commit_output(&paths)?;
            restrictions.eprint_compact();
            print_selection_summary(stats, false);
            info!(
                path = %paths.final_path.display(),
                count = entries.len(),
                level = args.level,
                "create seekable-zstd complete"
            );
            eprintln!(
                "created {} member(s) → {} (format seekable-zstd, level {})",
                entries.len(),
                paths.final_path.display(),
                args.level
            );
            if args.verify {
                crate::archive::verify_seekable_zstd(&paths.final_path, entries.len())?;
                eprintln!(
                    "verify ok: {} member(s), seekable-zstd",
                    entries.len()
                );
            }
            Ok(())
        }
        Err(e) => {
            cleanup_partial(&paths);
            Err(e)
        }
    }
}

/// One file fully compressed (or empty), ready for ordered pack append.
struct PreparedMember {
    name: String,
    mtime: Option<u64>,
    empty: bool,
    compressed: Option<CompressedPack>,
}

fn write_create_archive(
    partial: &Path,
    entries: &[SelectedEntry],
    level: u32,
    method: CompressMethod,
    concurrency: usize,
    size_budget: u64,
    zstd_nb_workers: u32,
) -> Result<()> {
    if concurrency <= 1 {
        let mut w = NonsolidLzma2Writer::create_with_method_workers(
            partial,
            level,
            method,
            zstd_nb_workers,
        )?;
        for e in entries {
            w.push_entry(e)?;
        }
        return w.finish();
    }

    // OPT-01/02/05: fixed worker pool + completion-driven ordered streaming write.
    write_create_parallel(partial, entries, level, method, concurrency, size_budget)
}

fn encode_one(
    entry: &SelectedEntry,
    level: u32,
    method: CompressMethod,
) -> Result<PreparedMember> {
    // OPT-03: use selection-time size/mtime; no re-stat.
    let mtime = entry.mtime_unix.map(filetime_from_unix_secs);

    if entry.size == 0 {
        return Ok(PreparedMember {
            name: entry.archive_name.clone(),
            mtime,
            empty: true,
            compressed: None,
        });
    }

    // File-level workers: no extra zstd nbWorkers (avoid oversubscription).
    let compressed =
        compress_path_with_size(&entry.abs_path, method, level, Some(entry.size), 0)?;
    Ok(PreparedMember {
        name: entry.archive_name.clone(),
        mtime,
        empty: false,
        compressed: Some(compressed),
    })
}

struct Job {
    idx: usize,
    size: u64,
    entry: SelectedEntry,
}

struct JobQueue {
    q: Mutex<VecDeque<Option<Job>>>,
    cvar: Condvar,
}

/// Persistent worker pool: encode with size-budget admission; write packs in
/// selection order as soon as each next index is ready (OPT-01/02/05).
fn write_create_parallel(
    partial: &Path,
    entries: &[SelectedEntry],
    level: u32,
    method: CompressMethod,
    max_workers: usize,
    size_budget: u64,
) -> Result<()> {
    let n = entries.len();
    if n == 0 {
        return Err(Error::EmptyArchive);
    }

    let mut writer = NonsolidLzma2Writer::create_with_method(partial, level, method)?;
    let queue = Arc::new(JobQueue {
        q: Mutex::new(VecDeque::new()),
        cvar: Condvar::new(),
    });
    let (res_tx, res_rx) = mpsc::channel::<(usize, u64, Result<PreparedMember>)>();
    let panic_flag = Arc::new(AtomicBool::new(false));

    thread::scope(|scope| -> Result<()> {
        for _ in 0..max_workers {
            let queue = Arc::clone(&queue);
            let res_tx = res_tx.clone();
            let panic_flag = Arc::clone(&panic_flag);
            scope.spawn(move || {
                loop {
                    let job = {
                        let mut g = queue.q.lock().unwrap();
                        loop {
                            if let Some(item) = g.pop_front() {
                                break item;
                            }
                            g = queue.cvar.wait(g).unwrap();
                        }
                    };
                    let Some(job) = job else {
                        break; // shutdown
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        encode_one(&job.entry, level, method)
                    }));
                    let result = match result {
                        Ok(r) => r,
                        Err(_) => {
                            panic_flag.store(true, Ordering::SeqCst);
                            Err(Error::Message("encode worker panicked".into()))
                        }
                    };
                    let _ = res_tx.send((job.idx, job.size, result));
                }
            });
        }
        drop(res_tx); // workers hold the only remaining senders

        let outcome = (|| -> Result<()> {
            let mut next_admit = 0usize;
            let mut next_write = 0usize;
            let mut running_sum = 0u64;
            let mut running_count = 0usize;
            let mut pending: BTreeMap<usize, PreparedMember> = BTreeMap::new();

            while next_write < n {
                if panic_flag.load(Ordering::SeqCst) {
                    return Err(Error::Message("encode worker panicked".into()));
                }

                // Admit jobs under concurrency + size budget.
                while next_admit < n
                    && can_admit(
                        running_sum,
                        running_count,
                        entries[next_admit].size,
                        size_budget,
                        max_workers,
                    )
                {
                    let size = entries[next_admit].size;
                    let job = Job {
                        idx: next_admit,
                        size,
                        entry: entries[next_admit].clone(),
                    };
                    {
                        let mut g = queue.q.lock().unwrap();
                        g.push_back(Some(job));
                    }
                    queue.cvar.notify_one();
                    running_sum = running_sum.saturating_add(size);
                    running_count += 1;
                    next_admit += 1;
                }

                if running_count == 0 {
                    if next_admit < n {
                        return Err(Error::Message(format!(
                            "encode scheduler stuck at index {next_admit} (workers={max_workers} budget={size_budget})"
                        )));
                    }
                    break;
                }

                // OPT-05: wait for *any* completion (not FIFO join).
                let (idx, size, result) = res_rx.recv().map_err(|_| {
                    Error::Message("encode workers exited unexpectedly".into())
                })?;
                running_sum = running_sum.saturating_sub(size);
                running_count = running_count.saturating_sub(1);

                let prepared = result?;
                pending.insert(idx, prepared);

                // OPT-02: stream write in order as soon as next index is ready.
                while let Some(p) = pending.remove(&next_write) {
                    if p.empty {
                        writer.push_packed_with_mtime(
                            p.name,
                            CompressedPack {
                                data: vec![],
                                method_id: vec![0x00],
                                method_props: vec![],
                                crc32: 0,
                                uncompressed_size: 0,
                                pack_crc: 0,
                            },
                            p.mtime,
                        )?;
                    } else if let Some(c) = p.compressed {
                        writer.push_packed_with_mtime(p.name, c, p.mtime)?;
                    }
                    next_write += 1;
                }
            }
            Ok(())
        })();

        shutdown_workers(&queue, max_workers);
        drop(res_rx);
        outcome?;
        writer.finish()
    })
}

fn shutdown_workers(queue: &JobQueue, n: usize) {
    {
        let mut g = queue.q.lock().unwrap();
        for _ in 0..n {
            g.push_back(None);
        }
    }
    queue.cvar.notify_all();
}

/// Max member size for optional verify sample extract (keeps verify cheap).
const VERIFY_SPOT_MAX_BYTES: u64 = 16 * 1024 * 1024;

fn verify_create_archive(path: &Path, expected_files: usize) -> Result<()> {
    use sevenz_rust2::ArchiveReader;

    let mut reader = ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|e| {
        Error::Archive(format!("verify open {}: {e}", path.display()))
    })?;

    // Collect checks under an immutable borrow, then drop before sample extract.
    let (n, spot): (usize, Option<(String, u64)>) = {
        let archive = reader.archive();
        if archive.is_solid {
            return Err(Error::Archive(format!(
                "verify: archive is solid (expected non-solid): {}",
                path.display()
            )));
        }
        let n = archive
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .count();
        if n != expected_files {
            return Err(Error::Archive(format!(
                "verify member count: expected {expected_files}, got {n}"
            )));
        }
        // Optional content spot-check: first suitable member (validates CRC on extract).
        let spot = archive
            .files
            .iter()
            .filter(|e| !e.is_directory() && e.size() > 0 && e.size() <= VERIFY_SPOT_MAX_BYTES)
            .map(|e| (e.name().to_string(), e.size()))
            .next();
        (n, spot)
    };

    let mut sample_note = "";
    if let Some((name, expected_size)) = spot {
        let data = reader.read_file(&name).map_err(|e| {
            Error::Archive(format!(
                "verify sample extract {name} in {}: {e}",
                path.display()
            ))
        })?;
        if data.len() as u64 != expected_size {
            return Err(Error::Archive(format!(
                "verify sample size: {name}: expected {expected_size}, got {}",
                data.len()
            )));
        }
        sample_note = " + sample extract";
    }

    eprintln!("verify ok: {n} file member(s), non-solid{sample_note}");
    Ok(())
}

fn print_selection_summary(stats: &SelectionStats, dry_run: bool) {
    let mode = if dry_run { "dry-run" } else { "create" };
    eprintln!(
        "{mode}: {} selected, {} excluded, {} dir-budget skipped, {} dir-file-limit skipped, {} max-size skipped, {} file-size-from skipped, {} min-size skipped, {} older-than skipped, {} max-total-size skipped, {} max-files skipped, {} symlinks skipped, {} hardlinks skipped, {} special skipped",
        stats.selected,
        stats.skipped_excluded,
        stats.skipped_dir_budget,
        stats.skipped_dir_file_limit,
        stats.skipped_max_size,
        stats.skipped_file_size_from,
        stats.skipped_min_size,
        stats.skipped_older_than,
        stats.skipped_max_total_size,
        stats.skipped_max_files,
        stats.skipped_symlinks,
        stats.skipped_hardlinks,
        stats.skipped_special
    );
}

/// Helper for tests: parse sources preserving trailing slash via strings.
pub fn sources_from_strings(paths: &[impl AsRef<str>]) -> Result<Vec<SourceSpec>> {
    paths
        .iter()
        .map(|p| SourceSpec::from_user_path(p.as_ref()))
        .collect()
}

/// Build a minimal CreateArgs for library tests.
#[cfg(test)]
pub(crate) fn test_create_args(
    output: std::path::PathBuf,
    sources: Vec<String>,
) -> CreateArgs {
    CreateArgs {
        output,
        format: None,
        dry_run: true,
        force: false,
        exclude: vec![],
        include: vec![],
        exclude_from: None,
        include_from: None,
        files_from: None,
        include_cwd: false,
        filter: vec![],
        level: 5,
        method: "lzma2".into(),
        threads: Some(1),
        encode_concurrency: 1,
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
        sources,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sevenz_rust2::{ArchiveReader, Password};
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn dry_run_lists_and_excludes() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        fs::create_dir_all(root.join("cache")).unwrap();
        fs::write(root.join("a.rs"), b"fn main(){}").unwrap();
        fs::write(root.join("a.tmp"), b"tmp").unwrap();
        fs::write(root.join("cache/x"), b"c").unwrap();

        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![format!("{}/", root.display())],
        );
        args.exclude.push("*.tmp".into());
        args.exclude.push("cache/".into());
        args.dry_run = true;

        let (entries, stats, _) = build_selection(&args).unwrap();
        let names: Vec<_> = entries.iter().map(|e| e.archive_name.as_str()).collect();
        assert_eq!(names, vec!["a.rs"]);
        assert!(stats.skipped_excluded >= 1);
    }

    #[test]
    fn rule_order_include_then_exclude() {
        let mut args = test_create_args(std::path::PathBuf::from("o.7z"), vec![]);
        args.include.push("*.c".into());
        args.exclude.push("*".into());
        let rules = build_rules(&args).unwrap();
        assert_eq!(
            rules.action_for("a.c", false),
            crate::select::RuleAction::Include
        );
        assert_eq!(
            rules.action_for("a.o", false),
            crate::select::RuleAction::Exclude
        );
    }

    #[test]
    fn empty_dry_run_ok() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("empty");
        fs::create_dir_all(&root).unwrap();
        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![root.to_string_lossy().into()],
        );
        args.dry_run = true;
        args.exclude.push("*".into());
        run_create(args).unwrap();
    }

    #[test]
    fn create_write_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello create").unwrap();
        fs::write(root.join("sub/b.txt"), b"nested create").unwrap();
        fs::write(root.join("empty.dat"), b"").unwrap();

        let out = dir.path().join("out.7z");
        let mut args = test_create_args(out.clone(), vec![format!("{}/", root.display())]);
        args.dry_run = false;
        args.level = 1;
        args.verify = true;
        args.threads = Some(2);
        args.encode_concurrency = 2;
        run_create(args).unwrap();

        assert!(out.exists());
        assert!(!dir.path().join("out.7z.partial").exists());

        let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        assert!(!reader.archive().is_solid);
        assert_eq!(reader.read_file("a.txt").unwrap(), b"hello create");
        assert_eq!(reader.read_file("sub/b.txt").unwrap(), b"nested create");
        assert_eq!(reader.read_file("empty.dat").unwrap(), b"");
    }

    #[test]
    fn create_refuses_overwrite_without_force() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("a.txt");
        fs::write(&f, b"x").unwrap();
        let out = dir.path().join("out.7z");
        fs::write(&out, b"old").unwrap();
        let mut args = test_create_args(out, vec![f.to_string_lossy().into()]);
        args.dry_run = false;
        let err = run_create(args).unwrap_err();
        assert!(matches!(err, Error::OutputExists(_)));
    }

    #[test]
    fn create_zstd_and_lz4_roundtrip() {
        for method in ["zstd", "lz4"] {
            let dir = tempdir().unwrap();
            let f = dir.path().join("a.txt");
            fs::write(&f, format!("payload-{method}")).unwrap();
            let out = dir.path().join("out.7z");
            let mut args = test_create_args(out.clone(), vec![f.to_string_lossy().into()]);
            args.dry_run = false;
            args.method = method.into();
            args.level = 3;
            args.verify = true;
            run_create(args).unwrap();
            let mut reader =
                ArchiveReader::open(&out, Password::empty()).expect("open");
            assert!(!reader.archive().is_solid);
            assert_eq!(
                reader.read_file("a.txt").unwrap(),
                format!("payload-{method}").as_bytes()
            );
        }
    }

    #[test]
    fn create_writes_only_partial_then_final_in_output_dir() {
        // Ensure we do not stage a full source tree beside the output.
        let dir = tempdir().unwrap();
        let src = dir.path().join("src");
        fs::create_dir_all(&src).unwrap();
        fs::write(src.join("f.txt"), b"data").unwrap();
        let out = dir.path().join("out.7z");
        let mut args = test_create_args(out.clone(), vec![format!("{}/", src.display())]);
        args.dry_run = false;
        args.level = 1;
        run_create(args).unwrap();

        assert!(out.exists());
        assert!(!dir.path().join("out.7z.partial").exists());
        // Source still intact; no "mirror" copy of the tree for archiving.
        assert!(src.join("f.txt").exists());
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        // Only src/ and out.7z expected (no staged copy tree).
        assert!(entries.iter().any(|n| n == "out.7z"));
        assert!(entries.iter().any(|n| n == "src"));
        assert_eq!(entries.len(), 2, "unexpected siblings: {entries:?}");
    }

    fn set_mtime(path: &std::path::Path, secs: i64) {
        let ft = filetime::FileTime::from_unix_time(secs, 0);
        filetime::set_file_mtime(path, ft).unwrap();
    }

    #[test]
    fn dir_budget_newest_first_dry_run_and_write_match() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("logs")).unwrap();
        fs::create_dir_all(root.join("other")).unwrap();
        let old = root.join("logs/old.bin");
        let mid = root.join("logs/mid.bin");
        let new = root.join("logs/new.bin");
        let keep = root.join("other/keep.txt");
        fs::write(&old, vec![0u8; 10]).unwrap();
        fs::write(&mid, vec![0u8; 20]).unwrap();
        fs::write(&new, vec![0u8; 30]).unwrap();
        fs::write(&keep, b"ok").unwrap();
        set_mtime(&old, 100);
        set_mtime(&mid, 200);
        set_mtime(&new, 300);
        set_mtime(&keep, 50);

        let src = format!("{}/", root.display());
        let mut args = test_create_args(dir.path().join("out.7z"), vec![src.clone()]);
        args.dir_max_size = vec!["logs/=35".into()];
        args.dry_run = true;

        let (dry_entries, stats, _) = build_selection(&args).unwrap();
        let dry_names: std::collections::HashSet<_> = dry_entries
            .iter()
            .map(|e| e.archive_name.as_str())
            .collect();
        assert_eq!(
            dry_names,
            ["logs/new.bin", "other/keep.txt"].into_iter().collect()
        );
        assert_eq!(stats.skipped_dir_budget, 2);

        // Write path must use the same selection.
        let out = dir.path().join("budget.7z");
        let mut write_args = test_create_args(out.clone(), vec![src]);
        write_args.dir_max_size = vec!["logs/=35".into()];
        write_args.dry_run = false;
        write_args.level = 1;
        write_args.verify = true;
        write_args.threads = Some(1);
        run_create(write_args).unwrap();

        let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        assert!(!reader.archive().is_solid);
        let members: Vec<String> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(members.len(), 2);
        assert!(members.iter().any(|n| n == "logs/new.bin"));
        assert!(members.iter().any(|n| n == "other/keep.txt"));
        assert!(!members.iter().any(|n| n.contains("old.bin") || n.contains("mid.bin")));
        assert_eq!(reader.read_file("logs/new.bin").unwrap().len(), 30);
    }

    #[test]
    fn dir_budget_nested_longest_prefix() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("logs/nested")).unwrap();
        let a = root.join("logs/a.bin");
        let b = root.join("logs/nested/b.bin");
        let c = root.join("logs/nested/c.bin");
        fs::write(&a, vec![0u8; 10]).unwrap();
        fs::write(&b, vec![0u8; 10]).unwrap();
        fs::write(&c, vec![0u8; 10]).unwrap();
        set_mtime(&a, 100);
        set_mtime(&b, 300);
        set_mtime(&c, 200);

        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![format!("{}/", root.display())],
        );
        args.dir_max_size = vec!["logs/=1000".into(), "logs/nested/=15".into()];
        let (entries, stats, _) = build_selection(&args).unwrap();
        let names: std::collections::HashSet<_> =
            entries.iter().map(|e| e.archive_name.as_str()).collect();
        assert_eq!(
            names,
            ["logs/a.bin", "logs/nested/b.bin"].into_iter().collect()
        );
        assert_eq!(stats.skipped_dir_budget, 1);
    }

    #[test]
    fn dir_file_limit_recursive_newest_first() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("logs/nested")).unwrap();
        let old = root.join("logs/old.bin");
        let mid = root.join("logs/mid.bin");
        let new = root.join("logs/new.bin");
        let deep = root.join("logs/nested/deep.bin");
        let keep = root.join("root.txt");
        fs::write(&old, b"old").unwrap();
        fs::write(&mid, b"mid").unwrap();
        fs::write(&new, b"new").unwrap();
        fs::write(&deep, b"deep").unwrap();
        fs::write(&keep, b"keep").unwrap();
        set_mtime(&old, 100);
        set_mtime(&mid, 200);
        set_mtime(&new, 300);
        set_mtime(&deep, 50);
        set_mtime(&keep, 1);

        let src = format!("{}/", root.display());
        let mut args = test_create_args(dir.path().join("out.7z"), vec![src.clone()]);
        args.dir_max_files = vec!["logs/=2".into()];
        let (entries, stats, _) = build_selection(&args).unwrap();
        let names: std::collections::HashSet<_> =
            entries.iter().map(|e| e.archive_name.as_str()).collect();
        // recursive: newest 2 under logs/** = new+mid; deep+old skipped
        assert_eq!(
            names,
            ["logs/mid.bin", "logs/new.bin", "root.txt"]
                .into_iter()
                .collect()
        );
        assert_eq!(stats.skipped_dir_file_limit, 2);
        assert!(!names.contains("logs/old.bin"));
        assert!(!names.contains("logs/nested/deep.bin"));

        // Write path same selection.
        let out = dir.path().join("files.7z");
        let mut write_args = test_create_args(out.clone(), vec![src]);
        write_args.dir_max_files = vec!["logs/=2".into()];
        write_args.dry_run = false;
        write_args.level = 1;
        write_args.verify = true;
        write_args.threads = Some(1);
        run_create(write_args).unwrap();

        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        let members: std::collections::HashSet<String> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(members.len(), 3);
        assert!(members.contains("logs/new.bin"));
        assert!(members.contains("logs/mid.bin"));
        assert!(!members.iter().any(|n| n.contains("old.bin") || n.contains("deep.bin")));
    }

    #[test]
    fn dir_file_limit_from_file() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("cache")).unwrap();
        let a = root.join("cache/a");
        let b = root.join("cache/b");
        let c = root.join("cache/c");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();
        set_mtime(&a, 100);
        set_mtime(&b, 200);
        set_mtime(&c, 300);

        let list = dir.path().join("limits.txt");
        fs::write(&list, "# comment\ncache/=1\n").unwrap();

        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![format!("{}/", root.display())],
        );
        args.dir_max_files_from = Some(list);
        let (entries, stats, _) = build_selection(&args).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_name, "cache/c");
        assert_eq!(stats.skipped_dir_file_limit, 2);
    }

    #[test]
    fn create_seekable_zstd_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("a.txt"), b"hello zst").unwrap();
        fs::write(root.join("sub/b.txt"), b"nested zst").unwrap();
        fs::write(root.join("empty.dat"), b"").unwrap();

        let out = dir.path().join("out.zst");
        let mut args = test_create_args(out.clone(), vec![format!("{}/", root.display())]);
        args.dry_run = false;
        args.level = 1;
        args.verify = true;
        // format inferred from .zst
        run_create(args).unwrap();

        assert!(out.exists());
        assert!(!dir.path().join("out.zst.partial").exists());

        let index = crate::archive::list_members(&out).unwrap();
        let mut names: Vec<_> = index.names().map(|s| s.to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "empty.dat", "sub/b.txt"]);
        assert_eq!(
            crate::archive::extract_member_bytes(&out, "a.txt").unwrap(),
            b"hello zst"
        );
        assert_eq!(
            crate::archive::extract_member_bytes(&out, "sub/b.txt").unwrap(),
            b"nested zst"
        );
        assert_eq!(
            crate::archive::extract_member_bytes(&out, "empty.dat").unwrap(),
            b""
        );
    }

    #[test]
    fn create_seekable_zstd_dry_run_no_file() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"x").unwrap();
        let out = dir.path().join("out.zst");
        let mut args = test_create_args(out.clone(), vec![format!("{}/", root.display())]);
        args.dry_run = true;
        args.format = Some(OutputFormat::SeekableZstd);
        run_create(args).unwrap();
        assert!(!out.exists());
    }

    #[test]
    fn create_default_still_sevenz_lzma2() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("a.txt");
        fs::write(&f, b"default-7z").unwrap();
        let out = dir.path().join("out.7z");
        let mut args = test_create_args(out.clone(), vec![f.to_string_lossy().into()]);
        args.dry_run = false;
        args.level = 1;
        // no --format; default 7z
        run_create(args).unwrap();
        let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        assert!(!reader.archive().is_solid);
        assert_eq!(reader.read_file("a.txt").unwrap(), b"default-7z");
    }

    #[test]
    fn global_max_total_size_newest_first_dry_run_and_write() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        let old = root.join("old.bin");
        let mid = root.join("mid.bin");
        let new = root.join("new.bin");
        fs::write(&old, vec![0u8; 10]).unwrap();
        fs::write(&mid, vec![0u8; 20]).unwrap();
        fs::write(&new, vec![0u8; 30]).unwrap();
        set_mtime(&old, 100);
        set_mtime(&mid, 200);
        set_mtime(&new, 300);

        let src = format!("{}/", root.display());
        let mut args = test_create_args(dir.path().join("out.7z"), vec![src.clone()]);
        args.max_total_size = Some("35".into());
        args.dry_run = true;
        let (entries, stats, report) = build_selection(&args).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_name, "new.bin");
        assert_eq!(stats.skipped_max_total_size, 2);
        assert!(report.format_compact().contains("max-total-size="));

        let out = dir.path().join("cap.7z");
        let mut write_args = test_create_args(out.clone(), vec![src]);
        write_args.max_total_size = Some("35".into());
        write_args.dry_run = false;
        write_args.level = 1;
        write_args.verify = true;
        write_args.threads = Some(1);
        run_create(write_args).unwrap();
        let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        let members: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(members, vec!["new.bin".to_string()]);
        assert_eq!(reader.read_file("new.bin").unwrap().len(), 30);
    }

    #[test]
    fn global_max_files_and_per_file_max_size() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        let a = root.join("a.bin");
        let b = root.join("b.bin");
        let c = root.join("c.bin");
        let huge = root.join("huge.bin");
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        fs::write(&c, b"c").unwrap();
        fs::write(&huge, vec![0u8; 100]).unwrap();
        set_mtime(&a, 100);
        set_mtime(&b, 200);
        set_mtime(&c, 300);
        set_mtime(&huge, 400);

        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![format!("{}/", root.display())],
        );
        args.max_size = Some("50".into());
        args.max_files = Some(2);
        let (entries, stats, report) = build_selection(&args).unwrap();
        // huge skipped by max-size; then max-files keeps newest 2 of a,b,c → b,c
        let names: std::collections::HashSet<_> =
            entries.iter().map(|e| e.archive_name.as_str()).collect();
        assert_eq!(names, ["b.bin", "c.bin"].into_iter().collect());
        assert_eq!(stats.skipped_max_size, 1);
        assert_eq!(stats.skipped_max_files, 1);
        let text = report.format_compact();
        assert!(text.contains("max-size:"), "{text}");
        assert!(text.contains("max-files="), "{text}");
    }

    #[test]
    fn min_size_and_newer_than() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("tree");
        fs::create_dir_all(&root).unwrap();
        let tiny = root.join("tiny.bin");
        let old = root.join("old.bin");
        let ok = root.join("ok.bin");
        fs::write(&tiny, b"x").unwrap();
        fs::write(&old, vec![0u8; 20]).unwrap();
        fs::write(&ok, vec![0u8; 20]).unwrap();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        set_mtime(&tiny, now - 10);
        set_mtime(&old, now - 10_000);
        set_mtime(&ok, now - 10);

        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![format!("{}/", root.display())],
        );
        args.min_size = Some("10".into());
        args.newer_than = Some("100s".into());
        let (entries, stats, _) = build_selection(&args).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].archive_name, "ok.bin");
        assert_eq!(stats.skipped_min_size, 1);
        assert_eq!(stats.skipped_older_than, 1);
    }
}
