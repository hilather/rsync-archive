//! Tests for `--include-cwd` (pack process CWD at archive root; skip self output).

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn bin() -> assert_cmd::Command {
    cargo_bin_cmd!("rsync-archive")
}

#[test]
fn include_cwd_packs_root_members_and_skips_output() {
    let dir = tempdir().unwrap();
    let root = dir.path();
    fs::write(root.join("a.txt"), b"alpha").unwrap();
    fs::write(root.join("b.bin"), b"beta").unwrap();
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/nested.txt"), b"nest").unwrap();

    let out = root.join("pack.7z");
    // Stale partial should also be ignored if present.
    fs::write(root.join("pack.7z.partial"), b"partial-junk").unwrap();

    bin()
        .current_dir(root)
        .args([
            "create",
            "-o",
            "pack.7z",
            "--include-cwd",
            "-n",
            "--level",
            "1",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.txt"))
        .stdout(predicate::str::contains("b.bin"))
        .stdout(predicate::str::contains("sub/nested.txt"))
        .stdout(predicate::str::contains("pack.7z").not())
        .stdout(predicate::str::contains("pack.7z.partial").not());

    // Real write: output exists and does not embed itself as a member name.
    bin()
        .current_dir(root)
        .args([
            "create",
            "-o",
            "pack.7z",
            "--include-cwd",
            "--force",
            "--level",
            "1",
            "--verify",
        ])
        .assert()
        .success();
    assert!(out.exists());
}

#[test]
fn include_cwd_off_by_default_requires_src() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("x.txt"), b"x").unwrap();
    bin()
        .current_dir(dir.path())
        .args(["create", "-o", "o.7z", "-n"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn include_cwd_merges_with_src() {
    let dir = tempdir().unwrap();
    let cwd = dir.path();
    fs::write(cwd.join("root.txt"), b"root").unwrap();
    let extra = cwd.join("extra");
    fs::create_dir_all(&extra).unwrap();
    fs::write(extra.join("e.txt"), b"e").unwrap();

    bin()
        .current_dir(cwd)
        .args([
            "create",
            "-o",
            "o.7z",
            "--include-cwd",
            "-n",
            &format!("{}/", extra.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("root.txt"))
        .stdout(predicate::str::contains("e.txt"));
}

/// Regression: filter files ending in `- *` must not wipe `--include-cwd` members.
#[test]
fn include_cwd_ignores_rsync_star_exclude() {
    let dir = tempdir().unwrap();
    let cwd = dir.path();
    fs::write(cwd.join("readme.md"), b"meta").unwrap();
    fs::write(cwd.join("notes.txt"), b"notes").unwrap();
    fs::create_dir_all(cwd.join("data/keep")).unwrap();
    fs::write(cwd.join("data/keep/a.log"), b"log").unwrap();

    let filter = cwd.join("rules.txt");
    // Would exclude everything if applied to CWD pack.
    fs::write(&filter, "+ /data/\n+ /data/**\n- *\n").unwrap();

    // CWD-only: still get root files + data tree (no filters on include-cwd).
    bin()
        .current_dir(cwd)
        .args([
            "create",
            "-o",
            "pack.7z",
            "--include-cwd",
            "--filter-from",
            filter.to_str().unwrap(),
            "-n",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("readme.md"))
        .stdout(predicate::str::contains("notes.txt"))
        .stdout(predicate::str::contains("data/keep/a.log"));
}
