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

#[test]
fn dry_run_dir_budget_excludes_older_files() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("logs")).unwrap();
    let old = root.join("logs/old.bin");
    let new = root.join("logs/new.bin");
    let keep = root.join("root.txt");
    fs::write(&old, vec![0u8; 20]).unwrap();
    fs::write(&new, vec![0u8; 20]).unwrap();
    fs::write(&keep, b"keep").unwrap();
    filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(100, 0)).unwrap();
    filetime::set_file_mtime(&new, filetime::FileTime::from_unix_time(300, 0)).unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--dir-max-size",
            "logs/=25",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("logs/new.bin"))
        .stdout(predicate::str::contains("root.txt"))
        .stdout(predicate::str::contains("old.bin").not())
        .stderr(predicate::str::contains("dir-max-size"))
        .stderr(predicate::str::contains("skip:"))
        .stderr(predicate::str::contains("logs/old.bin"))
        .stderr(predicate::str::contains("dir-budget skipped"));
    assert!(!dir.path().join("out.7z").exists());
}

#[test]
fn dry_run_dir_file_limit_recursive() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("logs/nested")).unwrap();
    let old = root.join("logs/old.bin");
    let new = root.join("logs/new.bin");
    let deep = root.join("logs/nested/deep.bin");
    let keep = root.join("root.txt");
    fs::write(&old, b"old").unwrap();
    fs::write(&new, b"new").unwrap();
    fs::write(&deep, b"deep").unwrap();
    fs::write(&keep, b"keep").unwrap();
    filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(100, 0)).unwrap();
    filetime::set_file_mtime(&new, filetime::FileTime::from_unix_time(300, 0)).unwrap();
    filetime::set_file_mtime(&deep, filetime::FileTime::from_unix_time(50, 0)).unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--dir-max-files",
            "logs/=1",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("logs/new.bin"))
        .stdout(predicate::str::contains("root.txt"))
        // recursive: nested files count against the limit too
        .stdout(predicate::str::contains("logs/nested/deep.bin").not())
        .stdout(predicate::str::contains("old.bin").not())
        .stderr(predicate::str::contains("dir-max-files"))
        .stderr(predicate::str::contains("skip:"))
        .stderr(predicate::str::contains("dir-file-limit skipped"));
    assert!(!dir.path().join("out.7z").exists());
}

#[test]
fn dry_run_dir_file_limit_from_file() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("logs")).unwrap();
    let old = root.join("logs/old.bin");
    let new = root.join("logs/new.bin");
    fs::write(&old, b"old").unwrap();
    fs::write(&new, b"new").unwrap();
    filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(100, 0)).unwrap();
    filetime::set_file_mtime(&new, filetime::FileTime::from_unix_time(300, 0)).unwrap();

    let list = dir.path().join("limits.txt");
    fs::write(&list, "logs/=1\n").unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--dir-max-files-from",
            list.to_str().unwrap(),
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("logs/new.bin"))
        .stdout(predicate::str::contains("old.bin").not());
}

#[test]
fn dry_run_max_total_size_newest_first() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    let old = root.join("old.bin");
    let new = root.join("new.bin");
    fs::write(&old, vec![0u8; 20]).unwrap();
    fs::write(&new, vec![0u8; 20]).unwrap();
    filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(100, 0)).unwrap();
    filetime::set_file_mtime(&new, filetime::FileTime::from_unix_time(300, 0)).unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--max-total-size",
            "25",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("new.bin"))
        .stdout(predicate::str::contains("old.bin").not())
        .stderr(predicate::str::contains("max-total-size="))
        .stderr(predicate::str::contains("max-total-size skipped"));
    assert!(!dir.path().join("out.7z").exists());
}

#[test]
fn dry_run_max_files_and_max_size() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    let a = root.join("a.bin");
    let b = root.join("b.bin");
    let huge = root.join("huge.bin");
    fs::write(&a, b"a").unwrap();
    fs::write(&b, b"b").unwrap();
    fs::write(&huge, vec![0u8; 100]).unwrap();
    filetime::set_file_mtime(&a, filetime::FileTime::from_unix_time(100, 0)).unwrap();
    filetime::set_file_mtime(&b, filetime::FileTime::from_unix_time(200, 0)).unwrap();
    filetime::set_file_mtime(&huge, filetime::FileTime::from_unix_time(300, 0)).unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--max-size",
            "50",
            "--max-files",
            "1",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("b.bin"))
        .stdout(predicate::str::contains("a.bin").not())
        .stdout(predicate::str::contains("huge.bin").not())
        .stderr(predicate::str::contains("max-size:"))
        .stderr(predicate::str::contains("max-files="))
        .stderr(predicate::str::contains("max-size skipped"))
        .stderr(predicate::str::contains("max-files skipped"));
}

#[test]
fn dry_run_min_size_and_newer_than() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    let tiny = root.join("tiny.bin");
    let old = root.join("old.bin");
    let ok = root.join("ok.bin");
    fs::write(&tiny, b"x").unwrap();
    fs::write(&old, vec![0u8; 20]).unwrap();
    fs::write(&ok, vec![0u8; 20]).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    filetime::set_file_mtime(&tiny, filetime::FileTime::from_unix_time(now - 10, 0)).unwrap();
    filetime::set_file_mtime(&old, filetime::FileTime::from_unix_time(now - 10_000, 0)).unwrap();
    filetime::set_file_mtime(&ok, filetime::FileTime::from_unix_time(now - 10, 0)).unwrap();

    let src = format!("{}/", root.display());
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("out.7z").to_str().unwrap(),
            "-n",
            "--min-size",
            "10",
            "--newer-than",
            "100s",
            &src,
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("ok.bin"))
        .stdout(predicate::str::contains("tiny.bin").not())
        .stdout(predicate::str::contains("old.bin").not())
        .stderr(predicate::str::contains("min-size:"))
        .stderr(predicate::str::contains("newer-than:"))
        .stderr(predicate::str::contains("min-size skipped"))
        .stderr(predicate::str::contains("older-than skipped"));
}
