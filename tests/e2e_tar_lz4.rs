//! End-to-end tests for `--format tar-lz4` / `*.tar.lz4`.

use assert_cmd::cargo::cargo_bin_cmd;
use predicates::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::tempdir;

fn bin() -> assert_cmd::Command {
    cargo_bin_cmd!("rsync-archive")
}

#[cfg(unix)]
#[test]
fn create_tar_lz4_includes_symlinks() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("target.txt"), b"payload").unwrap();
    std::os::unix::fs::symlink("target.txt", root.join("link.txt")).unwrap();

    let out = dir.path().join("out.tar.lz4");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "tar-lz4",
            "--level",
            "1",
            "--verify",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("verify ok"));

    let index = rsync_archive::list_tar_lz4_members(&out).unwrap();
    assert!(index.get("target.txt").is_some());
    assert!(index.get("link.txt").is_some());
    assert_eq!(index.get("link.txt").unwrap().data_len, 0);
    assert_eq!(
        rsync_archive::extract_tar_lz4_member_bytes(&out, "target.txt").unwrap(),
        b"payload"
    );
}

#[test]
fn create_tar_lz4_roundtrip_list_extract() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("sub")).unwrap();
    fs::write(root.join("a.txt"), b"hello tar lz4").unwrap();
    fs::write(root.join("sub/b.txt"), b"nested").unwrap();
    fs::write(root.join("empty.dat"), b"").unwrap();

    let out = dir.path().join("out.tar.lz4");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "tar-lz4",
            "--level",
            "1",
            "--verify",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("tar-lz4"))
        .stderr(predicate::str::contains("verify ok"));

    assert!(out.exists());

    let index = rsync_archive::list_tar_lz4_members(&out).unwrap();
    // 3 files + "sub/" directory member for nested path
    assert_eq!(index.members.len(), 4);
    assert!(index.get("sub/").is_some());
    assert_eq!(index.get("sub/").unwrap().data_len, 0);
    assert_eq!(
        rsync_archive::extract_tar_lz4_member_bytes(&out, "a.txt").unwrap(),
        b"hello tar lz4"
    );
    assert_eq!(
        rsync_archive::extract_tar_lz4_member_bytes(&out, "sub/b.txt").unwrap(),
        b"nested"
    );
    assert_eq!(
        rsync_archive::extract_tar_lz4_member_bytes(&out, "empty.dat").unwrap(),
        b""
    );
}

#[test]
fn create_tar_lz4_nested_dirs_in_index() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("a/b")).unwrap();
    fs::write(root.join("a/b/c.txt"), b"deep content").unwrap();

    let out = dir.path().join("nested.tar.lz4");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "tar-lz4",
            "--level",
            "1",
            "--verify",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("verify ok"));

    let index = rsync_archive::list_tar_lz4_members(&out).unwrap();
    assert!(index.get("a/").is_some());
    assert!(index.get("a/b/").is_some());
    assert!(index.get("a/b/c.txt").is_some());
    assert_eq!(index.get("a/").unwrap().data_len, 0);
    assert_eq!(index.get("a/b/").unwrap().data_len, 0);
    assert_eq!(
        rsync_archive::extract_tar_lz4_member_bytes(&out, "a/b/c.txt").unwrap(),
        b"deep content"
    );
}

#[test]
fn create_infers_tar_lz4_from_extension() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("t");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("x.txt"), b"x").unwrap();
    let out = dir.path().join("pack.tlz4");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--level",
            "1",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("tar-lz4"));
    assert_eq!(
        rsync_archive::list_tar_lz4_members(&out).unwrap().members.len(),
        1
    );
}

#[test]
fn create_tar_lz4_rejects_method_flag() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("t");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("x.txt"), b"x").unwrap();
    bin()
        .args([
            "create",
            "-o",
            dir.path().join("o.tar.lz4").to_str().unwrap(),
            "--format",
            "tar-lz4",
            "--method",
            "lz4",
            &format!("{}/", root.display()),
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("--method").or(predicate::str::contains("7z")));
}

#[test]
fn create_tar_lz4_preserves_mode_uid_gid() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("t");
    fs::create_dir_all(&root).unwrap();
    let path = root.join("script.sh");
    fs::write(&path, b"#!/bin/sh\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o750);
        fs::set_permissions(&path, perms).unwrap();
        let meta = fs::metadata(&path).unwrap();
        let expect_mode = meta.mode() & 0o7777;
        let expect_uid = meta.uid();
        let expect_gid = meta.gid();

        let out = dir.path().join("meta.tar.lz4");
        bin()
            .args([
                "create",
                "-o",
                out.to_str().unwrap(),
                "--format",
                "tar-lz4",
                "--level",
                "1",
                &format!("{}/", root.display()),
            ])
            .assert()
            .success();

        let index = rsync_archive::list_tar_lz4_members(&out).unwrap();
        let m = index.get("script.sh").unwrap();
        assert_eq!(m.mode, expect_mode);
        assert_eq!(m.uid, expect_uid);
        assert_eq!(m.gid, expect_gid);

        // uname/gname from selection (headers only; not in RATAIDX1).
        let (expect_uname, expect_gname) =
            rsync_archive::names_for_uid_gid(expect_uid, expect_gid);
        let entries = rsync_archive::collect_from_sources(
            &[rsync_archive::SourceSpec::from_user_path(format!("{}/", root.display()).as_str())
                .unwrap()],
            &rsync_archive::RuleSet::new(),
        )
        .unwrap()
        .0;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].uname, expect_uname);
        assert_eq!(entries[0].gname, expect_gname);
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

#[test]
fn create_tar_lz4_dry_run_writes_nothing() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("t");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("x.txt"), b"x").unwrap();
    let out = dir.path().join("out.tar.lz4");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "-n",
            "--format",
            "tar-lz4",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("x.txt"));
    assert!(!out.exists());
}

/// Create interop: decode all LZ4 frames (stop before RATLFRM1) + stock `tar -tf`.
/// Soft-skips when `tar` is not on PATH.
#[test]
fn system_tar_can_list_after_decode_if_tools_present() {
    let tar_ok = Command::new("tar")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !tar_ok {
        eprintln!("skip: system tar not available for create interop probe");
        return;
    }

    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(root.join("nested/deep")).unwrap();
    fs::write(root.join("nested/deep/file.txt"), b"hello interop lz4").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("file.txt", root.join("nested/deep/link.txt")).unwrap();
        fs::write(root.join("nested/deep/hl_a.txt"), b"shared-hl").unwrap();
        fs::hard_link(
            root.join("nested/deep/hl_a.txt"),
            root.join("nested/deep/hl_b.txt"),
        )
        .unwrap();
    }
    #[cfg(not(unix))]
    {
        fs::write(root.join("nested/deep/extra.txt"), b"no-links").unwrap();
    }

    let out = dir.path().join("interop.tar.lz4");
    bin()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--format",
            "tar-lz4",
            "--level",
            "1",
            "--verify",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("verify ok"));

    // Decode every LZ4 frame in order; stop before cleartext RATLFRM1 footer.
    // Stock `lz4 -d` on the whole file is not the interop path (custom multi-frame
    // + footer). Trailing RATAIDX1 after EOA is fine for tar -t.
    let tar_bytes =
        rsync_archive::decompress_tar_lz4_payload_to_tar_bytes(&out).unwrap();
    assert!(tar_bytes.len() >= 1024);
    assert_eq!(&tar_bytes[257..262], b"ustar");

    let plain = dir.path().join("payload.tar");
    fs::write(&plain, &tar_bytes).unwrap();

    let list = Command::new("tar")
        .args(["-tf", plain.to_str().unwrap()])
        .output()
        .expect("run tar -tf");
    assert!(
        list.status.success(),
        "tar -tf failed: {}",
        String::from_utf8_lossy(&list.stderr)
    );
    let listing = String::from_utf8_lossy(&list.stdout);
    let names: Vec<&str> = listing.lines().collect();

    assert!(
        names.iter().any(|n| *n == "nested/" || *n == "./nested/"),
        "missing nested/ in tar -tf: {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| *n == "nested/deep/" || *n == "./nested/deep/"),
        "missing nested/deep/ in tar -tf: {names:?}"
    );
    assert!(
        names
            .iter()
            .any(|n| *n == "nested/deep/file.txt" || *n == "./nested/deep/file.txt"),
        "missing nested/deep/file.txt in tar -tf: {names:?}"
    );
    #[cfg(unix)]
    {
        assert!(
            names
                .iter()
                .any(|n| *n == "nested/deep/link.txt" || *n == "./nested/deep/link.txt"),
            "missing symlink nested/deep/link.txt in tar -tf: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| *n == "nested/deep/hl_a.txt" || *n == "./nested/deep/hl_a.txt"),
            "missing hardlink source in tar -tf: {names:?}"
        );
        assert!(
            names
                .iter()
                .any(|n| *n == "nested/deep/hl_b.txt" || *n == "./nested/deep/hl_b.txt"),
            "missing hardlink member in tar -tf: {names:?}"
        );
    }
}
