//! Create pipeline: selection + dry-run (write lands in Stage 6).

use crate::cli::CreateArgs;
use crate::error::{Error, Result};
use crate::select::from_file::{load_exclude_from, load_include_from};
use crate::select::walk::{
    collect_from_files_from, collect_from_sources, SelectedEntry, SelectionStats,
};
use crate::select::{RuleSet, SourceSpec};
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

/// Build the full selection (same path for dry-run and future write).
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
/// Non-dry-run write is Stage 6 (returns not implemented after selection succeeds).
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

    // Selection succeeded (including collision checks). Write is Stage 6.
    if entries.is_empty() {
        return Err(Error::EmptyArchive);
    }

    // Touch force/exists early so users get correct errors before Stage 6 lands.
    if args.output.exists() && !args.force {
        return Err(Error::OutputExists(args.output.clone()));
    }

    let _ = entries;
    Err(Error::NotImplemented(
        "create write (Stage 6 / 6b — dry-run works in Stage 5)",
    ))
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
        // includes pushed before excludes
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
    fn write_path_not_implemented_after_selection() {
        let dir = tempdir().unwrap();
        let f = dir.path().join("a.txt");
        fs::write(&f, b"hi").unwrap();
        let mut args = test_create_args(
            dir.path().join("out.7z"),
            vec![f.to_string_lossy().into()],
        );
        args.dry_run = false;
        let err = run_create(args).unwrap_err();
        assert!(matches!(err, Error::NotImplemented(_)));
    }
}
