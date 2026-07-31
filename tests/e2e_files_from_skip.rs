//! E2E: `--files-from-skip-missing` and multi-SRC missing-root soft-skip.

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn bin() -> Command {
    Command::cargo_bin("rsync-archive").expect("binary")
}

#[test]
fn files_from_missing_hard_fails_without_flag() {
    let dir = tempdir().unwrap();
    let list = dir.path().join("list.txt");
    fs::write(
        &list,
        format!("{}\n", dir.path().join("nope.txt").display()),
    )
    .unwrap();
    bin()
        .current_dir(dir.path())
        .args([
            "create",
            "-n",
            "-o",
            "out.7z",
            "--files-from",
            list.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1);
}

#[test]
fn files_from_skip_missing_keeps_present_lines() {
    let dir = tempdir().unwrap();
    let keep = dir.path().join("keep.txt");
    fs::write(&keep, b"hello").unwrap();
    let list = dir.path().join("list.txt");
    fs::write(
        &list,
        format!(
            "{}\n{}\n",
            dir.path().join("gone.txt").display(),
            keep.display()
        ),
    )
    .unwrap();
    bin()
        .current_dir(dir.path())
        .args([
            "create",
            "-n",
            "-o",
            "out.7z",
            "--files-from",
            list.to_str().unwrap(),
            "--files-from-skip-missing",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("keep.txt"))
        .stderr(
            predicate::str::contains("files-from-miss")
                .or(predicate::str::contains("skip: files-from")),
        );
}

#[test]
fn multi_src_one_missing_soft_skips_and_packs_other() {
    let dir = tempdir().unwrap();
    let keep = dir.path().join("keep");
    fs::create_dir_all(&keep).unwrap();
    fs::write(keep.join("a.txt"), b"a").unwrap();
    let missing = dir.path().join("missing-root");
    bin()
        .current_dir(dir.path())
        .args([
            "create",
            "-n",
            "-o",
            "out.7z",
            &format!("{}/", missing.display()),
            &format!("{}/", keep.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt"))
        .stderr(
            predicate::str::contains("missing-src")
                .or(predicate::str::contains("skip: missing-src")),
        );
}

#[test]
fn single_missing_src_fails() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("nope");
    bin()
        .current_dir(dir.path())
        .args([
            "create",
            "-n",
            "-o",
            "out.7z",
            missing.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1);
}
