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
    compress_path, filetime_from_unix_secs, CompressedPack, CompressMethod,
};
use crate::archive::{write_seekable_zstd, NonsolidLzma2Writer};
use crate::cli::{CreateArgs, OutputFormat};
use crate::error::{Error, Result};
use crate::pipeline::output::{cleanup_partial, commit_output, prepare_output};
use crate::select::dir_budget::{apply_dir_budgets, parse_dir_budgets};
use crate::select::from_file::{load_exclude_from, load_include_from};
use crate::select::walk::{
    collect_from_files_from, collect_from_sources, SelectedEntry, SelectionStats,
};
use crate::select::{RuleSet, SourceSpec};
use crate::util::{
    can_admit, file_stats_from_sizes, parse_byte_size, resolve_encode_concurrency,
    resolve_encode_workers,
};
use std::path::Path;
use std::thread;
use tracing::info;

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
/// Performs collision pre-scan inside collectors (K29), then applies
/// `--dir-max-size` budgets (newest-first) when configured.
pub fn build_selection(args: &CreateArgs) -> Result<(Vec<SelectedEntry>, SelectionStats)> {
    let rules = build_rules(args)?;
    let (entries, mut stats) = if let Some(list) = &args.files_from {
        collect_from_files_from(list, &rules)?
    } else {
        let mut specs = Vec::with_capacity(args.sources.len());
        for s in &args.sources {
            specs.push(SourceSpec::from_user_path(s)?);
        }
        collect_from_sources(&specs, &rules)?
    };
    let budgets = parse_dir_budgets(&args.dir_max_size)?;
    let entries = apply_dir_budgets(entries, &budgets, &mut stats)?;
    Ok((entries, stats))
}

/// Run `rsync-archive create`.
///
/// Dry-run lists archive names and exit 0 even when empty.
/// Write builds a non-solid 7z or seekable-zstd stream via partial+rename.
pub fn run_create(args: CreateArgs) -> Result<()> {
    let format = args.resolved_format();
    let (entries, stats) = build_selection(&args)?;

    if args.dry_run {
        for e in &entries {
            println!("{}", e.archive_name);
        }
        print_selection_summary(&stats, true);
        info!(
            format = format.as_str(),
            selected = stats.selected,
            skipped_excluded = stats.skipped_excluded,
            skipped_dir_budget = stats.skipped_dir_budget,
            skipped_symlinks = stats.skipped_symlinks,
            skipped_special = stats.skipped_special,
            "create dry-run complete"
        );
        return Ok(());
    }

    if entries.is_empty() {
        return Err(Error::EmptyArchive);
    }

    match format {
        OutputFormat::SevenZ => run_create_sevenz(&args, &entries, &stats),
        OutputFormat::SeekableZstd => run_create_seekable_zstd(&args, &entries, &stats),
    }
}

fn run_create_sevenz(
    args: &CreateArgs,
    entries: &[SelectedEntry],
    stats: &SelectionStats,
) -> Result<()> {
    let method = CompressMethod::parse(&args.method)?;
    let (n, total_bytes) = file_stats_from_sizes(entries.iter().map(|e| e.size));
    let workers = resolve_encode_workers(args.threads, n, total_bytes);
    let concurrency = resolve_encode_concurrency(args.encode_concurrency, workers);
    let size_budget = parse_byte_size(&args.encode_size_budget)?;

    info!(
        format = "7z",
        method = method.as_str(),
        workers,
        concurrency,
        size_budget,
        file_count = n,
        total_bytes,
        "create encode schedule"
    );

    let paths = prepare_output(&args.output, args.force)?;
    match write_create_archive(
        &paths.partial_path,
        entries,
        args.level,
        method,
        concurrency,
        size_budget,
    ) {
        Ok(()) => {
            commit_output(&paths)?;
            print_selection_summary(stats, false);
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

fn run_create_seekable_zstd(
    args: &CreateArgs,
    entries: &[SelectedEntry],
    stats: &SelectionStats,
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
) -> Result<()> {
    if concurrency <= 1 {
        let mut w = NonsolidLzma2Writer::create_with_method(partial, level, method)?;
        for e in entries {
            w.push_path(e.archive_name.clone(), &e.abs_path)?;
        }
        return w.finish();
    }

    let prepared = encode_parallel(entries, level, method, concurrency, size_budget)?;
    let mut w = NonsolidLzma2Writer::create_with_method(partial, level, method)?;
    for p in prepared {
        if p.empty {
            w.push_packed_with_mtime(
                p.name,
                CompressedPack {
                    data: vec![],
                    method_id: vec![0x00],
                    method_props: vec![],
                    crc32: 0,
                    uncompressed_size: 0,
                },
                p.mtime,
            )?;
        } else if let Some(c) = p.compressed {
            w.push_packed_with_mtime(p.name, c, p.mtime)?;
        }
    }
    w.finish()
}

fn encode_one(
    entry: &SelectedEntry,
    level: u32,
    method: CompressMethod,
) -> Result<PreparedMember> {
    let meta = std::fs::symlink_metadata(&entry.abs_path).map_err(|e| {
        Error::Archive(format!("stat {} for create: {e}", entry.abs_path.display()))
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(Error::NotRegularFile(entry.abs_path.clone()));
    }
    let mtime = meta.modified().ok().and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| filetime_from_unix_secs(d.as_secs()))
    });

    if meta.len() == 0 {
        return Ok(PreparedMember {
            name: entry.archive_name.clone(),
            mtime,
            empty: true,
            compressed: None,
        });
    }

    let compressed = compress_path(&entry.abs_path, method, level)?;
    Ok(PreparedMember {
        name: entry.archive_name.clone(),
        mtime,
        empty: false,
        compressed: Some(compressed),
    })
}

/// Size-aware concurrent encode; results in **selection order**.
fn encode_parallel(
    entries: &[SelectedEntry],
    level: u32,
    method: CompressMethod,
    max_workers: usize,
    size_budget: u64,
) -> Result<Vec<PreparedMember>> {
    let n = entries.len();
    if n == 0 {
        return Ok(vec![]);
    }

    // Slot results; filled out-of-order by workers.
    let mut slots: Vec<Option<Result<PreparedMember>>> = (0..n).map(|_| None).collect();

    thread::scope(|scope| {
        let mut i = 0usize;
        let mut running_sum = 0u64;
        // (job_size, index, JoinHandle)
        let mut handles: Vec<(u64, usize, thread::ScopedJoinHandle<'_, Result<PreparedMember>>)> =
            Vec::new();

        while i < n || !handles.is_empty() {
            // Admit new jobs.
            while i < n
                && can_admit(
                    running_sum,
                    handles.len(),
                    entries[i].size,
                    size_budget,
                    max_workers,
                )
            {
                let idx = i;
                let size = entries[i].size;
                let entry = entries[i].clone();
                running_sum = running_sum.saturating_add(size);
                i += 1;
                let handle = scope.spawn(move || encode_one(&entry, level, method));
                handles.push((size, idx, handle));
            }

            if handles.is_empty() {
                // Cannot admit next job (shouldn't happen: first job always admits).
                if i < n {
                    return Err(Error::Message(format!(
                        "encode scheduler stuck at index {i} (workers={max_workers} budget={size_budget})"
                    )));
                }
                break;
            }

            // Join the oldest in-flight job (FIFO) to free budget.
            let (size, idx, handle) = handles.remove(0);
            let result = handle.join().map_err(|_| {
                Error::Message("encode worker panicked".into())
            })?;
            running_sum = running_sum.saturating_sub(size);
            slots[idx] = Some(result);
        }

        Ok(())
    })?;

    let mut out = Vec::with_capacity(n);
    for (i, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(Ok(p)) => out.push(p),
            Some(Err(e)) => return Err(e),
            None => {
                return Err(Error::Message(format!(
                    "internal: missing encode result for index {i}"
                )));
            }
        }
    }
    Ok(out)
}

fn verify_create_archive(path: &Path, expected_files: usize) -> Result<()> {
    use sevenz_rust2::ArchiveReader;

    let reader = ArchiveReader::open(path, sevenz_rust2::Password::empty()).map_err(|e| {
        Error::Archive(format!("verify open {}: {e}", path.display()))
    })?;
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
    eprintln!("verify ok: {n} file member(s), non-solid");
    Ok(())
}

fn print_selection_summary(stats: &SelectionStats, dry_run: bool) {
    let mode = if dry_run { "dry-run" } else { "create" };
    eprintln!(
        "{mode}: {} selected, {} excluded, {} dir-budget skipped, {} symlinks skipped, {} special skipped",
        stats.selected,
        stats.skipped_excluded,
        stats.skipped_dir_budget,
        stats.skipped_symlinks,
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
        filter: vec![],
        level: 5,
        method: "lzma2".into(),
        threads: Some(1),
        encode_concurrency: 1,
        encode_size_budget: "500M".into(),
        dir_max_size: vec![],
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

        let (entries, stats) = build_selection(&args).unwrap();
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

        let (dry_entries, stats) = build_selection(&args).unwrap();
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
        let (entries, stats) = build_selection(&args).unwrap();
        let names: std::collections::HashSet<_> =
            entries.iter().map(|e| e.archive_name.as_str()).collect();
        assert_eq!(
            names,
            ["logs/a.bin", "logs/nested/b.bin"].into_iter().collect()
        );
        assert_eq!(stats.skipped_dir_budget, 1);
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
}
