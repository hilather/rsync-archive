//! End-to-end embed CLI tests (Stage 3).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn bin() -> Command {
    Command::cargo_bin("rsync-archive").expect("binary")
}

#[test]
fn embed_dry_run_lists_basenames() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("sub").join("pack.7z");
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::write(&a, b"payload").unwrap();
    bin()
        .current_dir(dir.path())
        .args([
            "embed",
            "-o",
            "master.7z",
            "-n",
            "--allow-any",
            "sub/pack.7z",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("pack.7z"));
    assert!(!dir.path().join("master.7z").exists());
}

#[test]
fn embed_keep_path_and_prefix() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("build").join("a.bin");
    fs::create_dir_all(a.parent().unwrap()).unwrap();
    fs::write(&a, b"data").unwrap();
    let out = dir.path().join("master.7z");
    bin()
        .args([
            "embed",
            "-o",
            out.to_str().unwrap(),
            "--allow-any",
            "--keep-path",
            "--prefix",
            "packs",
            a.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(out.exists());
}

#[test]
fn embed_require_7z_rejects_non_magic() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("x.bin");
    fs::write(&a, b"not-sevenz").unwrap();
    bin()
        .args([
            "embed",
            "-o",
            dir.path().join("m.7z").to_str().unwrap(),
            "--require-7z",
            a.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("magic"));
}
