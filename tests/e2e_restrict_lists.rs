//! End-to-end tests for `--files-from`, `--file-size-from`, `--dir-max-size-from`.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use tempfile::tempdir;

fn bin() -> assert_cmd::Command {
    cargo_bin_cmd!("rsync-archive")
}

#[test]
fn file_size_from_only_caps_matching_paths() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::write(root.join("logs/a.log"), vec![b'x'; 20]).unwrap();
    fs::write(root.join("logs/b.log"), vec![b'y'; 5]).unwrap();
    fs::write(root.join("keep.bin"), vec![b'z'; 100]).unwrap();

    let size_list = dir.path().join("sizes.txt");
    fs::write(&size_list, "*.log max=10\n# keep.bin not listed → no cap\n").unwrap();

    let out = dir.path().join("o.7z");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "-n",
            "--file-size-from",
            size_list.to_str().unwrap(),
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("logs/b.log"))
        .stdout(predicate::str::contains("keep.bin"))
        .stdout(predicate::str::contains("logs/a.log").not())
        .stderr(predicate::str::contains("file-size-from").or(predicate::str::contains("file-size")));
}

#[test]
fn dir_max_size_from_only_listed_prefixes() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();
    // Newest-first: set mtimes so big log is older and gets skipped under budget.
    let big = root.join("logs/big.bin");
    let small = root.join("logs/small.bin");
    let free = root.join("other/free.bin");
    fs::write(&big, vec![0u8; 80]).unwrap();
    fs::write(&small, vec![0u8; 20]).unwrap();
    fs::write(&free, vec![0u8; 200]).unwrap();

    let now = filetime::FileTime::now();
    filetime::set_file_mtime(&small, now).unwrap();
    filetime::set_file_mtime(
        &big,
        filetime::FileTime::from_unix_time(now.unix_seconds() - 100, 0),
    )
    .unwrap();

    let dlist = dir.path().join("dirbudgets.txt");
    fs::write(&dlist, "logs/ max=50\n# other/ not listed\n").unwrap();

    bin()
        .args([
            "create",
            "-o",
            dir.path().join("o.7z").to_str().unwrap(),
            "-n",
            "--dir-max-size-from",
            dlist.to_str().unwrap(),
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("logs/small.bin"))
        .stdout(predicate::str::contains("other/free.bin"))
        .stdout(predicate::str::contains("logs/big.bin").not())
        .stderr(predicate::str::contains("dir-max-size"));
}

#[test]
fn files_from_with_file_size_and_dir_lists() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("t");
    fs::create_dir_all(root.join("logs")).unwrap();
    fs::write(root.join("logs/a.log"), vec![0u8; 8]).unwrap();
    fs::write(root.join("logs/b.log"), vec![0u8; 40]).unwrap();
    fs::write(root.join("x.dat"), b"ok").unwrap();

    let files = dir.path().join("files.txt");
    fs::write(
        &files,
        format!(
            "{}\n{}\n{}\n",
            root.join("logs/a.log").display(),
            root.join("logs/b.log").display(),
            root.join("x.dat").display()
        ),
    )
    .unwrap();

    // files-from absolute → basename only; so names are a.log, b.log, x.dat
    let size_list = dir.path().join("sz.txt");
    fs::write(&size_list, "*.log max=10\n").unwrap();

    bin()
        .args([
            "create",
            "-o",
            dir.path().join("o.7z").to_str().unwrap(),
            "-n",
            "--files-from",
            files.to_str().unwrap(),
            "--file-size-from",
            size_list.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("a.log"))
        .stdout(predicate::str::contains("x.dat"))
        .stdout(predicate::str::contains("b.log").not());
}
