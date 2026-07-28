//! Create dry-run and selection e2e tests (Stage 5).

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn bin() -> Command {
    Command::cargo_bin("rsync-archive").expect("binary")
}

#[test]
fn dry_run_prunes_excluded_dir() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("skipme")).unwrap();
    fs::write(root.join("keep.txt"), b"k").unwrap();
    fs::write(root.join("skipme/secret"), b"secret").unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--exclude",
            "skipme/",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("keep.txt"))
        .stdout(predicate::str::contains("secret").not());
}

#[test]
fn dry_run_basename_exclude_tmp() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("sub/a.tmp"), b"t").unwrap();
    fs::write(root.join("sub/a.txt"), b"x").unwrap();
    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--exclude",
            "*.tmp",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("sub/a.txt"))
        .stdout(predicate::str::contains("a.tmp").not());
}

#[test]
fn multi_src_collision_errors() {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::create_dir_all(&a).unwrap();
    fs::create_dir_all(&b).unwrap();
    fs::write(a.join("f"), b"1").unwrap();
    fs::write(b.join("f"), b"2").unwrap();
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            &format!("{}/", a.display()),
            &format!("{}/", b.display()),
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("duplicate").or(predicate::str::contains("f")));
}

#[test]
fn files_from_relative_names() {
    let dir = tempdir().unwrap();
    fs::create_dir_all(dir.path().join("foo")).unwrap();
    fs::write(dir.path().join("foo/a.txt"), b"a").unwrap();
    fs::write(dir.path().join("b.txt"), b"b").unwrap();
    let list = dir.path().join("list.txt");
    fs::write(&list, "foo/a.txt\nb.txt\n").unwrap();

    bin()
        .current_dir(dir.path())
        .args([
            "create",
            "-o",
            "out.7z",
            "-n",
            "--files-from",
            "list.txt",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("foo/a.txt"))
        .stdout(predicate::str::contains("b.txt"));
}

#[test]
fn files_from_absolute_basename_collision() {
    let dir = tempdir().unwrap();
    let x = dir.path().join("x");
    let y = dir.path().join("y");
    fs::create_dir_all(&x).unwrap();
    fs::create_dir_all(&y).unwrap();
    fs::write(x.join("a.txt"), b"1").unwrap();
    fs::write(y.join("a.txt"), b"2").unwrap();
    let list = dir.path().join("list.txt");
    fs::write(
        &list,
        format!("{}\n{}\n", x.join("a.txt").display(), y.join("a.txt").display()),
    )
    .unwrap();

    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--files-from",
            list.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .code(1)
        .stderr(predicate::str::contains("duplicate").or(predicate::str::contains("a.txt")));
}

#[test]
fn dry_run_does_not_write_output() {
    let dir = tempdir().unwrap();
    let f = dir.path().join("only.txt");
    fs::write(&f, b"x").unwrap();
    let out = dir.path().join("out.7z");
    bin()
        .args(["create", "-o", out.to_str().unwrap(), "-n", f.to_str().unwrap()])
        .assert()
        .success()
        .stdout(predicate::str::contains("only.txt"));
    assert!(!out.exists());
    assert!(!dir.path().join("out.7z.partial").exists());
}
