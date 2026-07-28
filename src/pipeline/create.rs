//! Create pipeline: selection, dry-run, and LZMA2 non-solid write.

use crate::archive::NonsolidLzma2Writer;
use crate::cli::CreateArgs;
use crate::error::{Error, Result};
use crate::pipeline::output::{cleanup_partial, commit_output, prepare_output};
use crate::select::from_file::{load_exclude_from, load_include_from};
use crate::select::walk::{
    collect_from_files_from, collect_from_sources, SelectedEntry, SelectionStats,
};
use crate::select::{RuleSet, SourceSpec};
use std::path::Path;
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
/// Performs collision pre-scan inside collectors (K29).
pub fn build_selection(args: &CreateArgs) -> Result<(Vec<SelectedEntry>, SelectionStats)> {
    let rules = build_rules(args)?;
    if let Some(list) = &args.files_from {
        collect_from_files_from(list, &rules)
    } else {
        let mut specs = Vec::with_capacity(args.sources.len());
        for s in &args.sources {
            specs.push(SourceSpec::from_user_path(s)?);
        }
        collect_from_sources(&specs, &rules)
    }
}

/// Run `rsync-archive create`.
///
/// Dry-run lists archive names and exit 0 even when empty.
/// Write builds a non-solid LZMA2 7z via partial+rename.
pub fn run_create(args: CreateArgs) -> Result<()> {
    let (entries, stats) = build_selection(&args)?;

    if args.dry_run {
        for e in &entries {
            println!("{}", e.archive_name);
        }
        print_selection_summary(&stats, true);
        info!(
            selected = stats.selected,
            skipped_excluded = stats.skipped_excluded,
            skipped_symlinks = stats.skipped_symlinks,
            skipped_special = stats.skipped_special,
            "create dry-run complete"
        );
        return Ok(());
    }

    if entries.is_empty() {
        return Err(Error::EmptyArchive);
    }

    let paths = prepare_output(&args.output, args.force)?;
    match write_create_archive(&paths.partial_path, &entries, args.level) {
        Ok(()) => {
            commit_output(&paths)?;
            print_selection_summary(&stats, false);
            info!(
                path = %paths.final_path.display(),
                count = entries.len(),
                level = args.level,
                "create complete"
            );
            eprintln!(
                "created {} member(s) → {}",
                entries.len(),
                paths.final_path.display()
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

fn write_create_archive(
    partial: &Path,
    entries: &[SelectedEntry],
    level: u32,
) -> Result<()> {
    let mut w = NonsolidLzma2Writer::create(partial, level)?;
    for e in entries {
        w.push_path(e.archive_name.clone(), &e.abs_path)?;
    }
    w.finish()
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
        "{mode}: {} selected, {} excluded, {} symlinks skipped, {} special skipped",
        stats.selected,
        stats.skipped_excluded,
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
        dry_run: true,
        force: false,
        exclude: vec![],
        include: vec![],
        exclude_from: None,
        include_from: None,
        files_from: None,
        filter: vec![],
        level: 5,
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
}
