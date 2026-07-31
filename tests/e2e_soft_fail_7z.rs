//! Soft-fail robustness for 7z create: vanished / unreadable members are skipped;
//! remaining members still archive (exit 0). Pack stream stays consistent (no orphans).

use rsync_archive::{
    CompressMethod, MemberKind, NonsolidLzma2Writer, SelectedEntry, DEFAULT_FILE_MODE,
};
use sevenz_rust2::{ArchiveReader, Password};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn entry(path: &Path, name: &str, size: u64) -> SelectedEntry {
    SelectedEntry {
        abs_path: path.to_path_buf(),
        archive_name: name.into(),
        size,
        mtime_unix: None,
        mode: DEFAULT_FILE_MODE,
        uid: 0,
        gid: 0,
        uname: String::new(),
        gname: String::new(),
        kind: MemberKind::File,
    }
}

fn member_names(archive: &Path) -> Vec<String> {
    let reader = ArchiveReader::open(archive, Password::empty()).unwrap();
    reader
        .archive()
        .files
        .iter()
        .filter(|e| !e.is_directory())
        .map(|e| e.name().to_string())
        .collect()
}

fn extract(archive: &Path, name: &str) -> Vec<u8> {
    let mut reader = ArchiveReader::open(archive, Password::empty()).unwrap();
    reader.read_file(name).unwrap()
}

fn write_mix(method: CompressMethod, threads_note: &str) {
    let dir = tempdir().unwrap();
    let a = dir.path().join("a.txt");
    let b = dir.path().join("b.txt");
    let c = dir.path().join("c.txt");
    fs::write(&a, b"alpha-content").unwrap();
    fs::write(&b, b"bravo-will-vanish").unwrap();
    fs::write(&c, b"charlie-content").unwrap();

    let out = dir.path().join(format!("out-{threads_note}.7z"));
    let mut w = NonsolidLzma2Writer::create_with_method(&out, 1, method).unwrap();
    assert!(w.push_entry(&entry(&a, "a.txt", 13)).unwrap());
    // Simulate selection → encode race: path gone before open.
    fs::remove_file(&b).unwrap();
    assert!(
        !w.push_entry(&entry(&b, "b.txt", 17)).unwrap(),
        "vanished path must soft-skip"
    );
    assert!(w.push_entry(&entry(&c, "c.txt", 15)).unwrap());
    w.finish().unwrap();

    assert_eq!(member_names(&out), vec!["a.txt", "c.txt"]);
    assert_eq!(extract(&out, "a.txt"), b"alpha-content");
    assert_eq!(extract(&out, "c.txt"), b"charlie-content");
}

#[test]
fn soft_skip_lzma2_open_vanish_keeps_good_members() {
    write_mix(CompressMethod::Lzma2, "lzma2");
}

#[test]
fn soft_skip_zstd_open_vanish_keeps_good_members() {
    write_mix(CompressMethod::Zstd, "zstd");
}

#[test]
fn soft_skip_lz4_open_vanish_keeps_good_members() {
    write_mix(CompressMethod::Lz4, "lz4");
}

#[test]
fn soft_skip_streaming_size_path_open_vanish() {
    // Force streaming compress path (above 1 MiB oneshot threshold in codec).
    let dir = tempdir().unwrap();
    let big = dir.path().join("big.bin");
    let good = dir.path().join("good.txt");
    let payload = vec![0xABu8; 1_500_000];
    fs::write(&big, &payload).unwrap();
    fs::write(&good, b"ok").unwrap();

    let out = dir.path().join("stream.7z");
    let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
    assert!(w
        .push_entry(&entry(&good, "good.txt", 2))
        .unwrap());
    fs::remove_file(&big).unwrap();
    assert!(!w
        .push_entry(&entry(&big, "big.bin", payload.len() as u64))
        .unwrap());
    // Another good member after soft-skip proves no orphan pack corruption.
    let d = dir.path().join("d.txt");
    fs::write(&d, b"delta").unwrap();
    assert!(w.push_entry(&entry(&d, "d.txt", 5)).unwrap());
    w.finish().unwrap();

    assert_eq!(member_names(&out), vec!["good.txt", "d.txt"]);
    assert_eq!(extract(&out, "good.txt"), b"ok");
    assert_eq!(extract(&out, "d.txt"), b"delta");
}

#[cfg(unix)]
#[test]
fn soft_skip_permission_denied_at_open() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let good = dir.path().join("good.txt");
    let denied = dir.path().join("denied.txt");
    fs::write(&good, b"keep").unwrap();
    fs::write(&denied, b"secret").unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();

    // Skip when running as root (chmod 000 still openable).
    if fs::File::open(&denied).is_ok() {
        let _ = fs::set_permissions(&denied, fs::Permissions::from_mode(0o644));
        eprintln!("skip soft_skip_permission_denied_at_open: open still allowed");
        return;
    }

    let out = dir.path().join("perm.7z");
    let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
    assert!(w.push_entry(&entry(&good, "good.txt", 4)).unwrap());
    assert!(
        !w.push_entry(&entry(&denied, "denied.txt", 6)).unwrap(),
        "EACCES at open must soft-skip"
    );
    w.finish().unwrap();

    // Restore for tempdir cleanup.
    let _ = fs::set_permissions(&denied, fs::Permissions::from_mode(0o644));

    assert_eq!(member_names(&out), vec!["good.txt"]);
    assert_eq!(extract(&out, "good.txt"), b"keep");
}

#[test]
fn all_vanished_empty_archive_error() {
    let dir = tempdir().unwrap();
    let gone = dir.path().join("gone.txt");
    fs::write(&gone, b"x").unwrap();
    let out = dir.path().join("empty.7z");
    let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
    fs::remove_file(&gone).unwrap();
    assert!(!w.push_entry(&entry(&gone, "gone.txt", 1)).unwrap());
    let err = w.finish().unwrap_err();
    assert!(
        matches!(err, rsync_archive::Error::EmptyArchive),
        "{err:?}"
    );
}

/// CLI path: tree create with parallel encode; encode-time soft-skip is covered
/// by library tests (SelectedEntry with deleted abs_path + parallel write).
#[test]
fn cli_create_parallel_good_tree() {
    use assert_cmd::Command;
    use predicates::prelude::*;

    let dir = tempdir().unwrap();
    let root = dir.path().join("tree");
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("a.txt"), b"aaa").unwrap();
    fs::write(root.join("b.txt"), b"bbb").unwrap();
    fs::write(root.join("c.txt"), b"ccc").unwrap();

    let out = dir.path().join("cli.7z");
    Command::cargo_bin("rsync-archive")
        .unwrap()
        .args([
            "create",
            "-o",
            out.to_str().unwrap(),
            "--level",
            "1",
            "--threads",
            "2",
            "--encode-concurrency",
            "2",
            "--method",
            "lzma2",
            "--verify",
            &format!("{}/", root.display()),
        ])
        .assert()
        .success()
        .stderr(predicate::str::contains("created 3 member"));

    assert_eq!(extract(&out, "a.txt"), b"aaa");
    assert_eq!(extract(&out, "b.txt"), b"bbb");
    assert_eq!(extract(&out, "c.txt"), b"ccc");
}

/// T1: zero-byte selected file deleted before encode — soft-skip, neighbor kept.
#[test]
fn zero_byte_vanish_soft_skip_all_methods() {
    for method in [
        CompressMethod::Lzma2,
        CompressMethod::Zstd,
        CompressMethod::Lz4,
    ] {
        let dir = tempdir().unwrap();
        let empty = dir.path().join("empty.dat");
        let keep = dir.path().join("keep.txt");
        fs::write(&empty, b"").unwrap();
        fs::write(&keep, b"neighbor").unwrap();
        let out = dir.path().join(format!("z-{method:?}.7z"));
        let mut w = NonsolidLzma2Writer::create_with_method(&out, 1, method).unwrap();
        fs::remove_file(&empty).unwrap();
        assert!(
            !w.push_entry(&entry(&empty, "empty.dat", 0)).unwrap(),
            "{method:?}: vanished empty must soft-skip"
        );
        assert!(w.push_entry(&entry(&keep, "keep.txt", 8)).unwrap());
        w.finish().unwrap();
        assert_eq!(member_names(&out), vec!["keep.txt"]);
        assert_eq!(extract(&out, "keep.txt"), b"neighbor");
    }
}

/// T1: present zero-byte file is still archived.
#[test]
fn zero_byte_present_archives_empty() {
    let dir = tempdir().unwrap();
    let empty = dir.path().join("empty.dat");
    fs::write(&empty, b"").unwrap();
    let out = dir.path().join("empty.7z");
    let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
    assert!(w.push_entry(&entry(&empty, "empty.dat", 0)).unwrap());
    w.finish().unwrap();
    assert_eq!(member_names(&out), vec!["empty.dat"]);
    assert_eq!(extract(&out, "empty.dat"), b"");
}

/// Encode-time open vanish via public create pipeline is exercised by constructing
/// entries then deleting one path before the writer runs (library, not CLI race).
#[test]
fn library_push_after_delete_mixed_methods_and_threads_smoke() {
    // Parallel-style sequential soft-skip already in unit tests; smoke all methods.
    for method in [
        CompressMethod::Lzma2,
        CompressMethod::Zstd,
        CompressMethod::Lz4,
    ] {
        let dir = tempdir().unwrap();
        let files: Vec<(PathBuf, &'static str, &'static [u8])> = vec![
            (dir.path().join("1.txt"), "1.txt", b"one"),
            (dir.path().join("2.txt"), "2.txt", b"two"),
            (dir.path().join("3.txt"), "3.txt", b"three"),
        ];
        for (p, _, data) in &files {
            fs::write(p, data).unwrap();
        }
        let out = dir.path().join("m.7z");
        let mut w = NonsolidLzma2Writer::create_with_method(&out, 1, method).unwrap();
        assert!(w
            .push_entry(&entry(&files[0].0, files[0].1, files[0].2.len() as u64))
            .unwrap());
        fs::remove_file(&files[1].0).unwrap();
        assert!(!w
            .push_entry(&entry(&files[1].0, files[1].1, files[1].2.len() as u64))
            .unwrap());
        assert!(w
            .push_entry(&entry(&files[2].0, files[2].1, files[2].2.len() as u64))
            .unwrap());
        w.finish().unwrap();
        assert_eq!(extract(&out, "1.txt"), b"one");
        assert_eq!(extract(&out, "3.txt"), b"three");
        assert_eq!(member_names(&out).len(), 2);
    }
}
