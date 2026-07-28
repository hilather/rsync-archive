//! Stage 0 CLI smoke tests: help surfaces and usage validation.

use assert_cmd::Command;
use predicates::prelude::*;

fn bin() -> Command {
    Command::cargo_bin("rsync-archive").expect("binary")
}

#[test]
fn help_lists_create_and_embed() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("create"))
        .stdout(predicate::str::contains("embed"));
}

#[test]
fn create_help_shows_key_flags() {
    bin()
        .args(["create", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--force"))
        .stdout(predicate::str::contains("--exclude"))
        .stdout(predicate::str::contains("--files-from"))
        .stdout(predicate::str::contains("--level"));
}

#[test]
fn embed_help_shows_key_flags() {
    bin()
        .args(["embed", "--help"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--force"))
        .stdout(predicate::str::contains("--keep-path"))
        .stdout(predicate::str::contains("--require-7z"))
        .stdout(predicate::str::contains("--allow-any"));
}

#[test]
fn create_without_src_or_files_from_is_usage_error() {
    bin()
        .args(["create", "-o", "out.7z"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("SRC").or(predicate::str::contains("files-from")));
}

#[test]
fn create_files_from_with_src_is_usage_error() {
    bin()
        .args(["create", "-o", "out.7z", "--files-from", "list.txt", "srcdir"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("files-from"));
}

#[test]
fn create_with_src_is_not_implemented_exit_1() {
    bin()
        .args(["create", "-o", "out.7z", "."])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not implemented"));
}

#[test]
fn embed_is_not_implemented_exit_1() {
    bin()
        .args(["embed", "-o", "master.7z", "a.7z"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("not implemented"));
}

#[test]
fn create_validate_unit_rejects_both_modes() {
    use rsync_archive::cli::CreateArgs;
    use std::path::PathBuf;

    let args = CreateArgs {
        output: PathBuf::from("out.7z"),
        dry_run: false,
        force: false,
        exclude: vec![],
        include: vec![],
        exclude_from: None,
        include_from: None,
        files_from: Some(PathBuf::from("list.txt")),
        filter: vec![],
        level: 5,
        verify: false,
        sources: vec![PathBuf::from("src")],
    };
    assert!(args.validate().is_err());
}

#[test]
fn create_validate_unit_accepts_sources_only() {
    use rsync_archive::cli::CreateArgs;
    use std::path::PathBuf;

    let args = CreateArgs {
        output: PathBuf::from("out.7z"),
        dry_run: false,
        force: false,
        exclude: vec![],
        include: vec![],
        exclude_from: None,
        include_from: None,
        files_from: None,
        filter: vec![],
        level: 5,
        verify: false,
        sources: vec![PathBuf::from("src")],
    };
    assert!(args.validate().is_ok());
}
