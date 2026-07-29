//! CLI smoke tests: help surfaces, usage validation, create dry-run / embed.

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
        .stdout(predicate::str::contains("embed"))
        .stdout(predicate::str::contains("Examples:"))
        .stdout(predicate::str::contains("tar-zstd").or(predicate::str::contains("non-solid")));
}

#[test]
fn short_help_h_shows_guidance() {
    // `-h` is the short surface; must still list formats and key flags.
    bin()
        .args(["create", "-h"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("tar-zstd"))
        .stdout(predicate::str::contains("tar-lz4"))
        .stdout(predicate::str::contains("seekable-zstd"))
        .stdout(predicate::str::contains("--method"))
        .stdout(predicate::str::contains("--dir-max-size"))
        .stdout(predicate::str::contains("recursive"))
        .stdout(predicate::str::contains("Examples:"));
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
        .stdout(predicate::str::contains("--level"))
        .stdout(predicate::str::contains("--threads"))
        .stdout(predicate::str::contains("--encode-size-budget"))
        .stdout(predicate::str::contains("--dir-max-size"))
        .stdout(predicate::str::contains("--dir-max-size-from"))
        .stdout(predicate::str::contains("--dir-max-files"))
        .stdout(predicate::str::contains("--dir-max-files-from"))
        .stdout(predicate::str::contains("--file-size-from"))
        .stdout(predicate::str::contains("--max-total-size"))
        .stdout(predicate::str::contains("--max-files"))
        .stdout(predicate::str::contains("--max-size"))
        .stdout(predicate::str::contains("--min-size"))
        .stdout(predicate::str::contains("--newer-than"))
        .stdout(predicate::str::contains("--method"))
        .stdout(predicate::str::contains("--format"))
        .stdout(predicate::str::contains("seekable-zstd"))
        .stdout(predicate::str::contains("tar-zstd"))
        .stdout(predicate::str::contains("tar-lz4"))
        .stdout(predicate::str::contains("Examples:"))
        .stdout(predicate::str::contains("SELECTION.md").or(predicate::str::contains("rsync-style")));
}

#[test]
fn embed_help_shows_key_flags() {
    bin()
        .args(["embed", "-h"])
        .assert()
        .success()
        .stdout(predicate::str::contains("--dry-run"))
        .stdout(predicate::str::contains("--force"))
        .stdout(predicate::str::contains("--keep-path"))
        .stdout(predicate::str::contains("--require-7z"))
        .stdout(predicate::str::contains("--allow-any"))
        .stdout(predicate::str::contains("Examples:"));
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
fn create_write_success() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"hi create").unwrap();
    let out = dir.path().join("out.7z");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--level",
            "1",
            "--verify",
            f.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(out.exists());
    assert!(out.metadata().unwrap().len() > 32);
}

#[test]
fn create_dry_run_lists_files() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::write(root.join("sub/b.txt"), b"b").unwrap();
    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt"))
        .stdout(predicate::str::contains("sub/b.txt"));
    assert!(!dir.path().join("out.7z").exists());
}

#[test]
fn embed_missing_input_exits_1() {
    bin()
        .args(["embed", "-o", "master.7z", "a.7z"])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("stat").or(predicate::str::contains("a.7z")));
}

#[test]
fn embed_cli_roundtrip() {
    use std::fs;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let a = dir.path().join("one.dat");
    let b = dir.path().join("two.dat");
    fs::write(&a, b"one").unwrap();
    fs::write(&b, b"two").unwrap();
    let out = dir.path().join("master.7z");
    bin()
        .args([
            "embed",
            "-o",
            out.to_str().unwrap(),
            "--allow-any",
            "--verify",
            a.to_str().unwrap(),
            b.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(out.exists());
    assert!(out.metadata().unwrap().len() > 32);
}

#[test]
fn create_validate_unit_rejects_both_modes() {
    use rsync_archive::cli::CreateArgs;
    use std::path::PathBuf;

    let args = CreateArgs {
        output: PathBuf::from("out.7z"),
        format: None,
        dry_run: false,
        force: false,
        exclude: vec![],
        include: vec![],
        exclude_from: None,
        include_from: None,
        files_from: Some(PathBuf::from("list.txt")),
        filter: vec![],
        level: 5,
        method: "lzma2".into(),
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
        sources: vec!["src".into()],
    };
    assert!(args.validate().is_err());
}

#[test]
fn create_validate_unit_accepts_sources_only() {
    use rsync_archive::cli::CreateArgs;
    use std::path::PathBuf;

    let args = CreateArgs {
        output: PathBuf::from("out.7z"),
        format: None,
        dry_run: false,
        force: false,
        exclude: vec![],
        include: vec![],
        exclude_from: None,
        include_from: None,
        files_from: None,
        filter: vec![],
        level: 5,
        method: "lzma2".into(),
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
        sources: vec!["src".into()],
    };
    assert!(args.validate().is_ok());
}
