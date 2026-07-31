//! Soft-fail create for tar-zstd, tar-lz4, and seekable-zstd.
//!
//! - Vanished path at open: skip member, other members remain.
//! - Size race: post-open re-stat (not selection size) drives header/index.
//! - All members skipped → `CreateWriteStats { members_written: 0, … }`.

use rsync_archive::{
    extract_member_bytes, extract_tar_lz4_member_bytes, extract_tar_zstd_member_bytes,
    list_members, list_tar_lz4_members, list_tar_zstd_members, write_seekable_zstd, write_tar_lz4,
    write_tar_zstd, MemberKind, SelectedEntry,
};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::tempdir;

fn file_entry(abs: PathBuf, archive_name: &str, size: u64) -> SelectedEntry {
    SelectedEntry {
        abs_path: abs,
        archive_name: archive_name.replace('\\', "/"),
        size,
        mtime_unix: Some(1_700_000_000),
        mode: 0o644,
        uid: 0,
        gid: 0,
        uname: String::new(),
        gname: String::new(),
        kind: MemberKind::File,
    }
}

fn write_src(root: &Path, rel: &str, data: &[u8]) -> PathBuf {
    let abs = root.join(rel);
    if let Some(p) = abs.parent() {
        fs::create_dir_all(p).unwrap();
    }
    fs::write(&abs, data).unwrap();
    abs
}

// ── vanish at open ──────────────────────────────────────────────────────────

#[test]
fn tar_zstd_skip_vanished_keeps_other_members() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let keep = write_src(&root, "keep.txt", b"alive");
    let gone = write_src(&root, "gone.txt", b"bye");
    let entries = vec![
        file_entry(keep, "keep.txt", 5),
        file_entry(gone.clone(), "gone.txt", 3),
    ];
    fs::remove_file(&gone).unwrap();

    let out = dir.path().join("out.tar.zst");
    write_tar_zstd(&out, &entries, 1).unwrap();

    let index = list_tar_zstd_members(&out).unwrap();
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("gone.txt").is_none());
    assert_eq!(
        extract_tar_zstd_member_bytes(&out, "keep.txt").unwrap(),
        b"alive"
    );
}

#[test]
fn tar_lz4_skip_vanished_keeps_other_members() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let keep = write_src(&root, "keep.txt", b"alive");
    let gone = write_src(&root, "gone.txt", b"bye");
    let entries = vec![
        file_entry(keep, "keep.txt", 5),
        file_entry(gone.clone(), "gone.txt", 3),
    ];
    fs::remove_file(&gone).unwrap();

    let out = dir.path().join("out.tar.lz4");
    write_tar_lz4(&out, &entries, 1).unwrap();

    let index = list_tar_lz4_members(&out).unwrap();
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("gone.txt").is_none());
    assert_eq!(
        extract_tar_lz4_member_bytes(&out, "keep.txt").unwrap(),
        b"alive"
    );
}

#[test]
fn seekable_zstd_skip_vanished_keeps_other_members() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let keep = write_src(&root, "keep.txt", b"alive");
    let gone = write_src(&root, "gone.txt", b"bye");
    let entries = vec![
        file_entry(keep, "keep.txt", 5),
        file_entry(gone.clone(), "gone.txt", 3),
    ];
    fs::remove_file(&gone).unwrap();

    let out = dir.path().join("out.zst");
    write_seekable_zstd(&out, &entries, 1).unwrap();

    let index = list_members(&out).unwrap();
    assert_eq!(index.members.len(), 1);
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("gone.txt").is_none());
    assert_eq!(extract_member_bytes(&out, "keep.txt").unwrap(), b"alive");
}

#[test]
fn all_vanished_returns_zero_members_stats() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let a = write_src(&root, "a.txt", b"a");
    let b = write_src(&root, "b.txt", b"b");
    let entries = vec![
        file_entry(a.clone(), "a.txt", 1),
        file_entry(b.clone(), "b.txt", 1),
    ];
    fs::remove_file(&a).unwrap();
    fs::remove_file(&b).unwrap();

    let out_tz = dir.path().join("empty.tar.zst");
    let s = write_tar_zstd(&out_tz, &entries, 1).unwrap();
    assert_eq!(s.members_written, 0);
    assert_eq!(s.skipped_vanished, 2);

    let out_tl = dir.path().join("empty.tar.lz4");
    let s = write_tar_lz4(&out_tl, &entries, 1).unwrap();
    assert_eq!(s.members_written, 0);
    assert_eq!(s.skipped_vanished, 2);

    let out_sz = dir.path().join("empty.zst");
    let s = write_seekable_zstd(&out_sz, &entries, 1).unwrap();
    assert_eq!(s.members_written, 0);
    assert_eq!(s.skipped_vanished, 2);
}

// ── T1: zero-byte vanish (open soft-skip; no phantom empty) ─────────────────

#[test]
fn tar_zstd_zero_byte_vanish_soft_skips() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let empty = write_src(&root, "empty.dat", b"");
    let keep = write_src(&root, "keep.txt", b"alive");
    let entries = vec![
        file_entry(empty.clone(), "empty.dat", 0),
        file_entry(keep, "keep.txt", 5),
    ];
    fs::remove_file(&empty).unwrap();
    let out = dir.path().join("z.tar.zst");
    let s = write_tar_zstd(&out, &entries, 1).unwrap();
    assert_eq!(s.members_written, 1);
    assert_eq!(s.skipped_vanished, 1);
    let index = list_tar_zstd_members(&out).unwrap();
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("empty.dat").is_none());
}

#[test]
fn tar_lz4_zero_byte_vanish_soft_skips() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let empty = write_src(&root, "empty.dat", b"");
    let keep = write_src(&root, "keep.txt", b"alive");
    let entries = vec![
        file_entry(empty.clone(), "empty.dat", 0),
        file_entry(keep, "keep.txt", 5),
    ];
    fs::remove_file(&empty).unwrap();
    let out = dir.path().join("z.tar.lz4");
    let s = write_tar_lz4(&out, &entries, 1).unwrap();
    assert_eq!(s.members_written, 1);
    assert_eq!(s.skipped_vanished, 1);
    let index = list_tar_lz4_members(&out).unwrap();
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("empty.dat").is_none());
}

#[test]
fn seekable_zstd_zero_byte_vanish_soft_skips() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let empty = write_src(&root, "empty.dat", b"");
    let keep = write_src(&root, "keep.txt", b"alive");
    let entries = vec![
        file_entry(empty.clone(), "empty.dat", 0),
        file_entry(keep, "keep.txt", 5),
    ];
    fs::remove_file(&empty).unwrap();
    let out = dir.path().join("z.zst");
    let s = write_seekable_zstd(&out, &entries, 1).unwrap();
    assert_eq!(s.members_written, 1);
    assert_eq!(s.skipped_vanished, 1);
    let index = list_members(&out).unwrap();
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("empty.dat").is_none());
}

// ── T2: hardlink dangling prevention (body vanished before encode) ──────────

#[cfg(unix)]
fn hardlink_entries(root: &Path) -> (Vec<SelectedEntry>, PathBuf) {
    use rsync_archive::MemberKind;
    let body = write_src(root, "body.txt", b"shared-payload");
    let hl = root.join("link.txt");
    fs::hard_link(&body, &hl).unwrap();
    let keep = write_src(root, "keep.txt", b"unrelated");
    let entries = vec![
        SelectedEntry {
            abs_path: body.clone(),
            archive_name: "body.txt".into(),
            size: 14,
            mtime_unix: Some(1_700_000_000),
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: MemberKind::File,
        },
        SelectedEntry {
            abs_path: hl,
            archive_name: "link.txt".into(),
            size: 0,
            mtime_unix: Some(1_700_000_000),
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: MemberKind::HardLink {
                target: "body.txt".into(),
            },
        },
        file_entry(keep, "keep.txt", 9),
    ];
    (entries, body)
}

#[cfg(unix)]
#[test]
fn tar_zstd_hardlink_body_vanish_no_dangling_link() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    fs::create_dir_all(&root).unwrap();
    let (entries, body) = hardlink_entries(&root);
    fs::remove_file(&body).unwrap();

    let out = dir.path().join("hl.tar.zst");
    let s = write_tar_zstd(&out, &entries, 1).unwrap();
    assert!(s.members_written >= 1, "keep must survive");
    assert!(s.skipped_vanished >= 1);

    let index = list_tar_zstd_members(&out).unwrap();
    assert!(
        index.get("keep.txt").is_some(),
        "keep missing: {:?}",
        index.names().collect::<Vec<_>>()
    );
    assert!(
        index.get("body.txt").is_none(),
        "body must soft-skip: {:?}",
        index.names().collect::<Vec<_>>()
    );
    assert!(
        index.get("link.txt").is_none(),
        "dangling hardlink must not be archived: {:?}",
        index.names().collect::<Vec<_>>()
    );
    assert_eq!(
        extract_tar_zstd_member_bytes(&out, "keep.txt").unwrap(),
        b"unrelated"
    );
}

#[cfg(unix)]
#[test]
fn tar_lz4_hardlink_body_vanish_no_dangling_link() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    fs::create_dir_all(&root).unwrap();
    let (entries, body) = hardlink_entries(&root);
    fs::remove_file(&body).unwrap();

    let out = dir.path().join("hl.tar.lz4");
    let s = write_tar_lz4(&out, &entries, 1).unwrap();
    assert!(s.members_written >= 1);
    assert!(s.skipped_vanished >= 1);

    let index = list_tar_lz4_members(&out).unwrap();
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("body.txt").is_none());
    assert!(
        index.get("link.txt").is_none(),
        "dangling hardlink must not be archived"
    );
    assert_eq!(
        extract_tar_lz4_member_bytes(&out, "keep.txt").unwrap(),
        b"unrelated"
    );
}

// ── size shrink (selection size > on-disk) ──────────────────────────────────
// Policy: re-stat after open → header/index use actual size (not selection).

#[test]
fn tar_zstd_size_shrink_uses_actual() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let abs = write_src(&root, "small.txt", b"0123456789"); // 10 bytes
    // Selection claimed 1000 bytes.
    let entries = vec![file_entry(abs, "small.txt", 1000)];
    let out = dir.path().join("shrink.tar.zst");
    write_tar_zstd(&out, &entries, 1).unwrap();

    let index = list_tar_zstd_members(&out).unwrap();
    let m = index.get("small.txt").unwrap();
    assert_eq!(m.data_len, 10);
    assert_eq!(
        extract_tar_zstd_member_bytes(&out, "small.txt").unwrap(),
        b"0123456789"
    );
}

#[test]
fn tar_lz4_size_shrink_uses_actual() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let abs = write_src(&root, "small.txt", b"0123456789");
    let entries = vec![file_entry(abs, "small.txt", 1000)];
    let out = dir.path().join("shrink.tar.lz4");
    write_tar_lz4(&out, &entries, 1).unwrap();

    let index = list_tar_lz4_members(&out).unwrap();
    assert_eq!(index.get("small.txt").unwrap().data_len, 10);
    assert_eq!(
        extract_tar_lz4_member_bytes(&out, "small.txt").unwrap(),
        b"0123456789"
    );
}

#[test]
fn seekable_zstd_size_shrink_uses_actual() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let abs = write_src(&root, "small.txt", b"0123456789");
    let entries = vec![file_entry(abs, "small.txt", 1000)];
    let out = dir.path().join("shrink.zst");
    write_seekable_zstd(&out, &entries, 1).unwrap();

    let index = list_members(&out).unwrap();
    assert_eq!(index.get("small.txt").unwrap().data_len, 10);
    assert_eq!(
        extract_member_bytes(&out, "small.txt").unwrap(),
        b"0123456789"
    );
}

// ── size grow (selection size < on-disk) ────────────────────────────────────
// Policy: re-stat after open → archive full current size (not selection cap).

#[test]
fn tar_zstd_size_grow_uses_actual() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let data = b"hello-world-full";
    let abs = write_src(&root, "big.txt", data);
    let entries = vec![file_entry(abs, "big.txt", 5)]; // selection only saw 5
    let out = dir.path().join("grow.tar.zst");
    write_tar_zstd(&out, &entries, 1).unwrap();

    let index = list_tar_zstd_members(&out).unwrap();
    assert_eq!(index.get("big.txt").unwrap().data_len, data.len() as u64);
    assert_eq!(
        extract_tar_zstd_member_bytes(&out, "big.txt").unwrap(),
        data
    );
}

#[test]
fn tar_lz4_size_grow_uses_actual() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let data = b"hello-world-full";
    let abs = write_src(&root, "big.txt", data);
    let entries = vec![file_entry(abs, "big.txt", 5)];
    let out = dir.path().join("grow.tar.lz4");
    write_tar_lz4(&out, &entries, 1).unwrap();

    let index = list_tar_lz4_members(&out).unwrap();
    assert_eq!(index.get("big.txt").unwrap().data_len, data.len() as u64);
    assert_eq!(
        extract_tar_lz4_member_bytes(&out, "big.txt").unwrap(),
        data
    );
}

#[test]
fn seekable_zstd_size_grow_uses_actual() {
    let dir = tempdir().unwrap();
    let root = dir.path().join("src");
    let data = b"hello-world-full";
    let abs = write_src(&root, "big.txt", data);
    let entries = vec![file_entry(abs, "big.txt", 5)];
    let out = dir.path().join("grow.zst");
    write_seekable_zstd(&out, &entries, 1).unwrap();

    let index = list_members(&out).unwrap();
    assert_eq!(index.get("big.txt").unwrap().data_len, data.len() as u64);
    assert_eq!(extract_member_bytes(&out, "big.txt").unwrap(), data);
}
