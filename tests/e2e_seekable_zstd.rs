//! End-to-end seekable-zstd create tests.

use assert_cmd::Command;
use predicates::prelude::*;
use rsync_archive::{extract_member_bytes, list_members};
use sevenz_rust2::{ArchiveReader, Password};
use std::fs;
use tempfile::tempdir;

fn bin() -> Command {
    Command::cargo_bin("rsync-archive").expect("binary")
}

#[test]
fn create_seekable_zstd_list_extract_matches() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("a.txt"), b"aaa-zst").unwrap();
    fs::write(root.join("d/b.txt"), b"bbb-zst").unwrap();
    fs::write(root.join("skip.tmp"), b"nope").unwrap();

    let out = dir.path().join("out.zst");
    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "seekable-zstd",
            "--level",
            "1",
            "--exclude",
            "*.tmp",
            "--verify",
            &src,
        ])
        .assert()
        .success()
        .stderr(
            predicate::str::contains("verify ok")
                .and(predicate::str::contains("seekable-zstd")),
        );

    assert!(out.exists());
    assert!(!dir.path().join("out.zst.partial").exists());

    let index = list_members(&out).unwrap();
    let mut names: Vec<_> = index.names().map(|s| s.to_string()).collect();
    names.sort();
    assert_eq!(names, vec!["a.txt", "d/b.txt"]);
    assert_eq!(extract_member_bytes(&out, "a.txt").unwrap(), b"aaa-zst");
    assert_eq!(extract_member_bytes(&out, "d/b.txt").unwrap(), b"bbb-zst");
}

/// Stage 7: create `--verify` on seekable-zstd (index + per-member length).
#[test]
fn create_seekable_zstd_verify_flag() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"verify-zst").unwrap();
    let out = dir.path().join("v.zst");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "seekable-zstd",
            "--level",
            "1",
            "--verify",
            f.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("verify ok: 1 member(s), seekable-zstd"));
    assert_eq!(
        extract_member_bytes(&out, "a.txt").unwrap(),
        b"verify-zst"
    );
}

#[test]
fn create_infers_seekable_zstd_from_zst_extension() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"infer-ext").unwrap();
    let out = dir.path().join("pack.zst");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--level",
            "1",
            f.to_str().unwrap(),
        ])
        .assert()
        .success();

    let index = list_members(&out).unwrap();
    assert_eq!(index.names().collect::<Vec<_>>(), vec!["a.txt"]);
    assert_eq!(
        extract_member_bytes(&out, "a.txt").unwrap(),
        b"infer-ext"
    );
}

#[test]
fn create_default_output_is_still_sevenz_lzma2() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"still-7z").unwrap();
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

    let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
    assert!(!reader.archive().is_solid);
    assert_eq!(reader.read_file("a.txt").unwrap(), b"still-7z");
}

#[test]
fn create_seekable_zstd_dry_run_writes_nothing() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"x").unwrap();
    let out = dir.path().join("dry.zst");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "seekable-zstd",
            "-n",
            f.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt"));
    assert!(!out.exists());
}

#[test]
fn create_seekable_zstd_rejects_method_flag() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"x").unwrap();
    let out = dir.path().join("bad.zst");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "seekable-zstd",
            "--method",
            "zstd",
            f.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--method"));
}
