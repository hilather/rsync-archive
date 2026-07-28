//! End-to-end create write tests (Stage 6).

use assert_cmd::Command;
use predicates::prelude::*;
use sevenz_rust2::{ArchiveReader, Password};
use std::fs;
use tempfile::tempdir;

fn bin() -> Command {
    Command::cargo_bin("rsync-archive").expect("binary")
}

#[test]
fn create_tree_extract_matches() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("d")).unwrap();
    fs::write(root.join("a.txt"), b"aaa").unwrap();
    fs::write(root.join("d/b.txt"), b"bbb").unwrap();
    fs::write(root.join("skip.tmp"), b"nope").unwrap();

    let out = dir.path().join("out.7z");
    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--level",
            "1",
            "--exclude",
            "*.tmp",
            "--verify",
            &src,
        ])
        .assert()
        .success();

    let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
    assert!(!reader.archive().is_solid);
    assert_eq!(reader.read_file("a.txt").unwrap(), b"aaa");
    assert_eq!(reader.read_file("d/b.txt").unwrap(), b"bbb");
    let names: Vec<_> = reader
        .archive()
        .files
        .iter()
        .filter(|e| !e.is_directory())
        .map(|e| e.name().to_string())
        .collect();
    assert!(!names.iter().any(|n| n.contains("skip.tmp")), "{names:?}");
}

#[test]
fn create_force_overwrite() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"v1").unwrap();
    let out = dir.path().join("out.7z");
    bin()
        .args(["create", "-o", out.to_str().unwrap(), "--level", "1", f.to_str().unwrap()])
        .assert()
        .success();
    fs::write(&f, b"v2-longer").unwrap();
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--level",
            "1",
            "--force",
            f.to_str().unwrap(),
        ])
        .assert()
        .success();
    let mut reader = ArchiveReader::open(&out, Password::empty()).unwrap();
    assert_eq!(reader.read_file("a.txt").unwrap(), b"v2-longer");
}

#[test]
fn create_exists_without_force_fails() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("a.txt");
    fs::write(&f, b"x").unwrap();
    let out = dir.path().join("out.7z");
    fs::write(&out, b"old").unwrap();
    bin()
        .args(["create", "-o", out.to_str().unwrap(), f.to_str().unwrap()])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("exists").or(predicate::str::contains("force")));
}
