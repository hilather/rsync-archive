//! RA-friendly tar.zst create format (valid tar + seekable Zstd + RATARIDX1).
//!
//! Layout: [`docs/FORMAT_TAR_ZSTD.md`](../../../docs/FORMAT_TAR_ZSTD.md) /
//! plan: [`docs/PLAN_TAR_ZSTD.md`](../../../docs/PLAN_TAR_ZSTD.md).
//!
//! Uncompressed payload = POSIX ustar/pax tar stream + trailing member index.
//! Wrapped in Zstd **seekable** frames via [`zeekstd`].
//!
//! **Soft-fail create:** skippable open errors skip the member (other members
//! still written). After open, size is taken from post-open metadata (not
//! selection). If the file shrinks or a skippable read error occurs after the
//! tar header is committed, the remainder is **zero-padded** so the archive
//! stays valid; a warn is logged. Growing files are capped at the post-open
//! size. If every selected member is skipped, returns [`Error::EmptyArchive`].

use crate::archive::tar_common::{
    build_tar_headers, dir_meta_for_entry, encode_index, parent_dir_names, parse_index,
    TarMemberIndex, TarMemberIndexEntry, TarMemberMeta,
};
use crate::error::{Error, Result};
use crate::select::SelectedEntry;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::path::Path;
use tracing::debug;
use zeekstd::{Decoder, EncodeOptions, Encoder, FrameSizePolicy};

pub use crate::archive::tar_common::{INDEX_MAGIC, INDEX_VERSION};

/// Default independent-frame size (uncompressed) for seekable encoding.
pub const DEFAULT_FRAME_SIZE: u32 = 2 * 1024 * 1024;

fn zstd_level(level: u32) -> i32 {
    level.min(9) as i32
}

/// Write a seekable tar.zst archive from selected files, symbolic links, and hard links.
///
/// Parent directory prefixes of each member are emitted once as ustar directory
/// members (`typeflag` `'5'`, name ending in `/`) before the member, and included
/// in `RATAIDX1` with `data_len = 0`. Symlinks use typeflag `'2'` and hard links
/// typeflag `'1'`, both with no data body. Empty dirs with no selected members
/// are not added.
///
/// Regular files: open → re-stat for size → write header → stream exactly that
/// many bytes (pad zeros on early EOF / skippable mid-read). Soft-skip only on
/// skippable open failure. See module docs.
///
/// Returns [`CreateWriteStats`]; `members_written == 0` means no content members
/// were archived (caller handles empty / all-vanished; partial is left for cleanup).
pub fn write_tar_zstd(
    path: &Path,
    entries: &[SelectedEntry],
    level: u32,
) -> Result<crate::archive::CreateWriteStats> {
    use crate::archive::CreateWriteStats;

    if entries.is_empty() {
        return Err(Error::EmptyArchive);
    }

    let file = File::create(path).map_err(|e| {
        Error::Archive(format!("create tar.zst {}: {e}", path.display()))
    })?;

    let opts = EncodeOptions::new()
        .compression_level(zstd_level(level))
        .checksum_flag(true)
        .frame_size_policy(FrameSizePolicy::Uncompressed(DEFAULT_FRAME_SIZE));

    let mut encoder = Encoder::with_opts(file, opts).map_err(|e| {
        Error::Compress(format!("tar.zst encoder init: {e}"))
    })?;

    let mut index_entries: Vec<TarMemberIndexEntry> =
        Vec::with_capacity(crate::archive::tar_common::expected_tar_member_count(entries));
    let mut emitted_dirs: HashSet<String> = HashSet::new();
    // File members successfully written (archive names). Hard links whose
    // target was soft-skipped must not be emitted as dangling typeflag '1'.
    let mut written_files: HashSet<String> = HashSet::new();
    let mut pos: u64 = 0;
    let mut stats = CreateWriteStats::default();

    for e in entries {
        let mode = e.mode;
        let mtime = e.mtime_unix.unwrap_or(0);
        let has_body = e.has_data_body();

        // Drop hard-link members whose File target never made it into the archive
        // (e.g. body vanished at open). Prevents dangling typeflag '1' members.
        if e.is_hard_link() {
            let target = e.link_target().unwrap_or("");
            if !written_files.contains(target) {
                crate::util::soft_skip_note(
                    crate::util::SoftKind::Hardlink,
                    &e.archive_name,
                );
                stats.skipped_vanished += 1;
                continue;
            }
        }

        // Open + re-stat before dirs/header so a vanished path skips cleanly.
        // EINTR is retried inside open_file_for_encode (not soft-skipped).
        let (mut body_file, data_len) = if has_body {
            match crate::util::open_file_for_encode(&e.abs_path) {
                Ok(f) => {
                    let actual = f
                        .metadata()
                        .map(|m| m.len())
                        .unwrap_or(e.size);
                    if actual > 0 {
                        (Some(f), actual)
                    } else {
                        (None, 0)
                    }
                }
                Err(Error::Vanished(_)) => {
                    crate::util::soft_skip_note(
                        crate::util::SoftKind::Vanished,
                        &e.archive_name,
                    );
                    stats.skipped_vanished += 1;
                    continue;
                }
                Err(err) => return Err(err),
            }
        } else {
            (None, 0)
        };

        // Emit unique parent directory members (root-first) before the file.
        for dir_name in parent_dir_names(&e.archive_name) {
            if !emitted_dirs.insert(dir_name.clone()) {
                continue;
            }
            let meta = dir_meta_for_entry(e, &dir_name);
            let header_offset = pos;
            let header_bytes = build_tar_headers(&dir_name, &meta)?;
            write_all_encoded(&mut encoder, &header_bytes)?;
            pos = pos
                .checked_add(header_bytes.len() as u64)
                .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;
            // Directory body is empty; data offset is immediately after the header.
            let data_offset = pos;
            index_entries.push(TarMemberIndexEntry {
                name: dir_name.clone(),
                tar_header_offset: header_offset,
                tar_data_offset: data_offset,
                data_len: 0,
                mode: meta.mode,
                mtime_unix: meta.mtime,
                uid: meta.uid,
                gid: meta.gid,
            });
            debug!(
                name = %dir_name,
                header_offset,
                "tar.zst dir member written"
            );
        }

        let header_offset = pos;
        let meta = TarMemberMeta {
            size: data_len,
            mtime,
            mode,
            uid: e.uid,
            gid: e.gid,
            uname: e.uname.clone(),
            gname: e.gname.clone(),
            is_dir: false,
            link_target: e.link_target().map(|s| s.to_string()),
            is_hard_link: e.is_hard_link(),
        };
        let header_bytes = build_tar_headers(&e.archive_name, &meta)?;
        write_all_encoded(&mut encoder, &header_bytes)?;
        pos = pos
            .checked_add(header_bytes.len() as u64)
            .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;

        let data_offset = pos;
        if let Some(ref mut f) = body_file {
            stream_reader_exact_or_pad(&mut encoder, f, data_len, &e.abs_path)?;
        }
        pos = pos
            .checked_add(data_len)
            .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;

        // Pad file data to 512-byte boundary.
        let pad = (512 - (data_len % 512)) % 512;
        if pad > 0 {
            write_all_encoded(&mut encoder, &[0u8; 512][..pad as usize])?;
            pos = pos
                .checked_add(pad)
                .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;
        }

        index_entries.push(TarMemberIndexEntry {
            name: e.archive_name.clone(),
            tar_header_offset: header_offset,
            tar_data_offset: data_offset,
            data_len,
            mode,
            mtime_unix: mtime,
            uid: e.uid,
            gid: e.gid,
        });
        stats.members_written += 1;
        if has_body {
            written_files.insert(e.archive_name.clone());
        }

        debug!(
            name = %e.archive_name,
            header_offset,
            data_offset,
            data_len,
            is_symlink = e.is_symlink(),
            is_hard_link = e.is_hard_link(),
            "tar.zst member written"
        );
    }

    if stats.members_written == 0 {
        // Leave partial for caller cleanup; surface all-vanished via stats.
        drop(encoder);
        return Ok(stats);
    }

    // End-of-archive: two 512-byte zero blocks.
    write_all_encoded(&mut encoder, &[0u8; 1024])?;
    pos = pos
        .checked_add(1024)
        .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;

    // Trailing index + u64 index_start.
    let index_start = pos;
    let index_bytes = encode_index(&index_entries)?;
    write_all_encoded(&mut encoder, &index_bytes)?;
    write_all_encoded(&mut encoder, &index_start.to_le_bytes())?;

    encoder
        .finish()
        .map_err(|e| Error::Compress(format!("tar.zst finish: {e}")))?;
    Ok(stats)
}

fn write_all_encoded<W: Write>(encoder: &mut Encoder<'_, W>, mut data: &[u8]) -> Result<()> {
    while !data.is_empty() {
        let n = encoder
            .compress(data)
            .map_err(|e| Error::Compress(format!("tar.zst compress: {e}")))?;
        if n == 0 {
            return Err(Error::Compress(
                "tar.zst encoder made no progress".into(),
            ));
        }
        data = &data[n..];
    }
    Ok(())
}

/// Stream exactly `declared_len` bytes. Cap at that length (ignore growth).
/// Early EOF or skippable mid-read: pad zeros for the remainder and warn so the
/// tar member size matches the committed header.
fn stream_reader_exact_or_pad<W: Write, R: Read>(
    encoder: &mut Encoder<'_, W>,
    f: &mut R,
    declared_len: u64,
    path: &Path,
) -> Result<()> {
    let mut buf = vec![0u8; 128 * 1024];
    let mut remaining = declared_len;
    while remaining > 0 {
        let chunk = remaining.min(buf.len() as u64) as usize;
        let n = match crate::util::read_retrying(f, &mut buf[..chunk]) {
            Ok(0) => {
                let got = declared_len - remaining;
                crate::util::soft_skip_note_detail(
                    crate::util::SoftKind::Padded,
                    &path.display().to_string(),
                    Some(&format!("eof got={got}/{declared_len}")),
                );
                write_zeros_encoded(encoder, remaining)?;
                return Ok(());
            }
            Ok(n) => n,
            Err(e) if crate::util::is_skippable_fs_io(&e) => {
                crate::util::soft_skip_note_detail(
                    crate::util::SoftKind::Padded,
                    &path.display().to_string(),
                    Some(&format!("io rem={remaining}: {e}")),
                );
                write_zeros_encoded(encoder, remaining)?;
                return Ok(());
            }
            Err(e) => {
                return Err(Error::Archive(format!(
                    "read {} for tar.zst: {e}",
                    path.display()
                )));
            }
        };
        write_all_encoded(encoder, &buf[..n])?;
        remaining -= n as u64;
    }
    Ok(())
}

fn write_zeros_encoded<W: Write>(encoder: &mut Encoder<'_, W>, mut n: u64) -> Result<()> {
    let zero = [0u8; 4096];
    while n > 0 {
        let take = n.min(zero.len() as u64) as usize;
        write_all_encoded(encoder, &zero[..take])?;
        n -= take as u64;
    }
    Ok(())
}

/// List members from a tar.zst archive (index-based; no full decompress).
pub fn list_tar_zstd_members(path: &Path) -> Result<TarMemberIndex> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open tar.zst {}: {e}", path.display()))
    })?;
    list_tar_zstd_from_seekable(&mut file)
}

pub fn list_tar_zstd_from_seekable<S: Read + Seek>(src: S) -> Result<TarMemberIndex> {
    let mut decoder = Decoder::new(src).map_err(|e| {
        Error::Archive(format!("tar.zst decode open: {e}"))
    })?;
    let decomp_size = decoder.seek_table().size_decomp();
    if decomp_size < 8 {
        return Err(Error::Archive(
            "tar.zst: uncompressed payload too small for index footer".into(),
        ));
    }
    decoder
        .set_offset(decomp_size - 8)
        .map_err(|e| Error::Archive(format!("tar.zst seek to index footer: {e}")))?;
    let mut off_buf = [0u8; 8];
    read_exact_decoder(&mut decoder, &mut off_buf)?;
    let index_start = u64::from_le_bytes(off_buf);
    if index_start >= decomp_size - 8 {
        return Err(Error::Archive(format!(
            "tar.zst: invalid index_start {index_start} (decomp_size={decomp_size})"
        )));
    }
    let index_len = (decomp_size - 8)
        .checked_sub(index_start)
        .ok_or_else(|| Error::Archive("tar.zst: index length underflow".into()))?;
    if index_len > 64 * 1024 * 1024 {
        return Err(Error::Archive(format!(
            "tar.zst: index too large ({index_len} bytes)"
        )));
    }
    decoder
        .set_offset(index_start)
        .map_err(|e| Error::Archive(format!("tar.zst seek to index: {e}")))?;
    let mut index_buf = vec![0u8; index_len as usize];
    read_exact_decoder(&mut decoder, &mut index_buf)?;
    parse_index(&index_buf)
}

/// Extract one member by name (seek + decode only needed frames).
pub fn extract_tar_zstd_member(path: &Path, name: &str, out: &mut impl Write) -> Result<u64> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open tar.zst {}: {e}", path.display()))
    })?;
    extract_tar_zstd_from_seekable(&mut file, name, out)
}

pub fn extract_tar_zstd_from_seekable<S: Read + Seek>(
    mut src: S,
    name: &str,
    out: &mut impl Write,
) -> Result<u64> {
    let index = list_tar_zstd_from_seekable(&mut src)?;
    let entry = index
        .get(name)
        .ok_or_else(|| Error::Archive(format!("tar.zst: member not found: {name}")))?;

    src.rewind()
        .map_err(|e| Error::Archive(format!("tar.zst rewind for extract: {e}")))?;

    let mut decoder = Decoder::new(src).map_err(|e| {
        Error::Archive(format!("tar.zst decode open: {e}"))
    })?;
    decoder
        .set_offset(entry.tar_data_offset)
        .map_err(|e| Error::Archive(format!("tar.zst seek to member: {e}")))?;
    let limit = entry
        .tar_data_offset
        .checked_add(entry.data_len)
        .ok_or_else(|| Error::Archive("tar.zst: member range overflow".into()))?;
    decoder
        .set_offset_limit(limit)
        .map_err(|e| Error::Archive(format!("tar.zst set limit: {e}")))?;

    let mut remaining = entry.data_len;
    let mut buf = vec![0u8; 128 * 1024];
    while remaining > 0 {
        let chunk = remaining.min(buf.len() as u64) as usize;
        let n = decoder.decompress(&mut buf[..chunk]).map_err(|e| {
            Error::Archive(format!("tar.zst decompress member: {e}"))
        })?;
        if n == 0 {
            return Err(Error::Archive(format!(
                "tar.zst: unexpected EOF extracting {name} ({remaining} bytes left)"
            )));
        }
        out.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(entry.data_len)
}

pub fn extract_tar_zstd_member_bytes(path: &Path, name: &str) -> Result<Vec<u8>> {
    let index = list_tar_zstd_members(path)?;
    let entry = index
        .get(name)
        .ok_or_else(|| Error::Archive(format!("tar.zst: member not found: {name}")))?;
    if entry.data_len > usize::MAX as u64 {
        return Err(Error::Archive("member too large to buffer".into()));
    }
    let mut out = Vec::with_capacity(entry.data_len as usize);
    extract_tar_zstd_member(path, name, &mut out)?;
    Ok(out)
}

/// Fully decompress the seekable Zstd payload to uncompressed bytes.
///
/// Output is the POSIX tar stream + EOA (two zero blocks) + trailing `RATAIDX1`
/// index + `u64` `index_start`. Stock `tar -t` typically stops at EOA and ignores
/// the trailing index as garbage after the end-of-archive markers.
///
/// Used for create interop / smoke tests (not a product extract CLI).
pub fn decompress_tar_zstd_payload_to_tar_bytes(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|e| {
        Error::Archive(format!("open tar.zst {}: {e}", path.display()))
    })?;
    decompress_tar_zstd_payload_from_seekable(file)
}

/// See [`decompress_tar_zstd_payload_to_tar_bytes`].
pub fn decompress_tar_zstd_payload_from_seekable<S: Read + Seek>(src: S) -> Result<Vec<u8>> {
    let mut decoder = Decoder::new(src).map_err(|e| {
        Error::Archive(format!("tar.zst decode open: {e}"))
    })?;
    let decomp_size = decoder.seek_table().size_decomp();
    if decomp_size > 512 * 1024 * 1024 {
        return Err(Error::Archive(format!(
            "tar.zst: uncompressed payload too large for full buffer ({decomp_size} bytes)"
        )));
    }
    decoder
        .set_offset(0)
        .map_err(|e| Error::Archive(format!("tar.zst seek start: {e}")))?;
    let mut out = vec![0u8; decomp_size as usize];
    if decomp_size > 0 {
        read_exact_decoder(&mut decoder, &mut out)?;
    }
    Ok(out)
}

/// Verify index + extract each member to data_len.
pub fn verify_tar_zstd(path: &Path, expected_count: usize) -> Result<()> {
    let index = list_tar_zstd_members(path)?;
    if index.members.len() != expected_count {
        return Err(Error::Archive(format!(
            "tar.zst verify: expected {expected_count} members, got {}",
            index.members.len()
        )));
    }
    for m in &index.members {
        let mut sink = io::sink();
        let n = extract_tar_zstd_member(path, &m.name, &mut sink)?;
        if n != m.data_len {
            return Err(Error::Archive(format!(
                "tar.zst verify: {} length mismatch: index {} extract {n}",
                m.name, m.data_len
            )));
        }
    }
    Ok(())
}

fn read_exact_decoder<S: zeekstd::Seekable>(
    decoder: &mut Decoder<'_, S>,
    buf: &mut [u8],
) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = decoder.decompress(&mut buf[filled..]).map_err(|e| {
            Error::Archive(format!("tar.zst decompress: {e}"))
        })?;
        if n == 0 {
            return Err(Error::Archive(
                "tar.zst: unexpected EOF while reading".into(),
            ));
        }
        filled += n;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn entry(
        dir: &Path,
        rel: &str,
        data: &[u8],
        mtime: Option<u64>,
        mode: u32,
        uid: u32,
        gid: u32,
    ) -> SelectedEntry {
        let abs = dir.join(rel);
        if let Some(p) = abs.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&abs, data).unwrap();
        SelectedEntry {
            abs_path: abs,
            archive_name: rel.replace('\\', "/"),
            size: data.len() as u64,
            mtime_unix: mtime,
            mode,
            uid,
            gid,
            uname: String::new(),
            gname: String::new(),
            kind: crate::select::MemberKind::File,
        }
    }

    #[test]
    fn ustar_short_path_roundtrip() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let entries = vec![
            entry(&root, "a.txt", b"hello tar", Some(1_700_000_000), 0o640, 1000, 100),
            entry(&root, "sub/b.txt", b"nested", Some(100), 0o644, 0, 0),
            entry(&root, "empty.dat", b"", None, 0o600, 1, 2),
        ];
        let out = dir.path().join("out.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();

        let index = list_tar_zstd_members(&out).unwrap();
        // 3 files + parent dir "sub/" for nested path
        assert_eq!(index.members.len(), 4);
        assert!(index.get("sub/").is_some());
        assert_eq!(index.get("sub/").unwrap().data_len, 0);
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, "a.txt").unwrap(),
            b"hello tar"
        );
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, "sub/b.txt").unwrap(),
            b"nested"
        );
        assert_eq!(extract_tar_zstd_member_bytes(&out, "empty.dat").unwrap(), b"");
        assert_eq!(extract_tar_zstd_member_bytes(&out, "sub/").unwrap(), b"");
        let a = index.get("a.txt").unwrap();
        assert_eq!(a.mtime_unix, 1_700_000_000);
        assert_eq!(a.mode, 0o640);
        assert_eq!(a.uid, 1000);
        assert_eq!(a.gid, 100);
        assert_eq!(index.get("empty.dat").unwrap().mode, 0o600);
        verify_tar_zstd(&out, 4).unwrap();
    }

    #[test]
    fn nested_path_emits_dir_chain() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let entries = vec![entry(
            &root,
            "a/b/c.txt",
            b"deep",
            Some(99),
            0o644,
            0,
            0,
        )];
        let out = dir.path().join("nested.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();
        let index = list_tar_zstd_members(&out).unwrap();
        assert_eq!(index.members.len(), 3); // a/, a/b/, a/b/c.txt
        let names: Vec<_> = index.names().collect();
        assert_eq!(names, vec!["a/", "a/b/", "a/b/c.txt"]);
        assert_eq!(index.get("a/").unwrap().data_len, 0);
        assert_eq!(index.get("a/b/").unwrap().data_len, 0);
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, "a/b/c.txt").unwrap(),
            b"deep"
        );
        // Header typeflag for dir
        let m = index.get("a/").unwrap();
        let file = File::open(&out).unwrap();
        let mut decoder = Decoder::new(file).unwrap();
        decoder.set_offset(m.tar_header_offset).unwrap();
        let mut hdr = [0u8; 512];
        read_exact_decoder(&mut decoder, &mut hdr).unwrap();
        assert_eq!(hdr[156], b'5');
    }

    #[test]
    fn long_path_uses_pax() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let long = format!("dir/{}/file.txt", "x".repeat(120));
        let abs = root.join(&long);
        fs::create_dir_all(abs.parent().unwrap()).unwrap();
        fs::write(&abs, b"longpath").unwrap();
        let entries = vec![SelectedEntry {
            abs_path: abs,
            archive_name: long.clone(),
            size: 8,
            mtime_unix: Some(50),
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: crate::select::MemberKind::File,
        }];
        let out = dir.path().join("long.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, &long).unwrap(),
            b"longpath"
        );
    }

    #[test]
    fn archive_headers_carry_uname_gname() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let abs = root.join("n.txt");
        fs::create_dir_all(&root).unwrap();
        fs::write(&abs, b"n").unwrap();
        let entries = vec![SelectedEntry {
            abs_path: abs,
            archive_name: "n.txt".into(),
            size: 1,
            mtime_unix: Some(1),
            mode: 0o644,
            uid: 1000,
            gid: 100,
            uname: "alice".into(),
            gname: "staff".into(),
            kind: crate::select::MemberKind::File,
        }];
        let out = dir.path().join("names.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();

        let index = list_tar_zstd_members(&out).unwrap();
        let m = index.get("n.txt").unwrap();
        let file = File::open(&out).unwrap();
        let mut decoder = Decoder::new(file).unwrap();
        decoder.set_offset(m.tar_header_offset).unwrap();
        let mut hdr = [0u8; 512];
        read_exact_decoder(&mut decoder, &mut hdr).unwrap();
        let uname_end = hdr[265..297].iter().position(|&b| b == 0).unwrap_or(32);
        let gname_end = hdr[297..329].iter().position(|&b| b == 0).unwrap_or(32);
        assert_eq!(&hdr[265..265 + uname_end], b"alice");
        assert_eq!(&hdr[297..297 + gname_end], b"staff");
    }

    #[test]
    fn symlink_member_typeflag_and_index() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("target.txt"), b"payload").unwrap();
        let link_abs = root.join("link.txt");
        #[cfg(unix)]
        std::os::unix::fs::symlink("target.txt", &link_abs).unwrap();
        #[cfg(not(unix))]
        {
            // Non-Unix: synthesize a symlink SelectedEntry without creating a real link.
            let _ = link_abs;
        }

        let entries = vec![
            entry(&root, "target.txt", b"payload", Some(10), 0o644, 0, 0),
            SelectedEntry {
                abs_path: root.join("link.txt"),
                archive_name: "link.txt".into(),
                size: 0,
                mtime_unix: Some(11),
                mode: 0o777,
                uid: 0,
                gid: 0,
                uname: String::new(),
                gname: String::new(),
                kind: crate::select::MemberKind::Symlink {
                    target: "target.txt".into(),
                },
            },
        ];
        let out = dir.path().join("sym.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();

        let index = list_tar_zstd_members(&out).unwrap();
        assert!(index.get("link.txt").is_some());
        assert_eq!(index.get("link.txt").unwrap().data_len, 0);
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, "target.txt").unwrap(),
            b"payload"
        );
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, "link.txt").unwrap(),
            b""
        );

        let m = index.get("link.txt").unwrap();
        let file = File::open(&out).unwrap();
        let mut decoder = Decoder::new(file).unwrap();
        decoder.set_offset(m.tar_header_offset).unwrap();
        let mut hdr = [0u8; 512];
        read_exact_decoder(&mut decoder, &mut hdr).unwrap();
        assert_eq!(hdr[156], b'2');
        let link_end = hdr[157..257].iter().position(|&b| b == 0).unwrap_or(100);
        assert_eq!(&hdr[157..157 + link_end], b"target.txt");
        verify_tar_zstd(&out, 2).unwrap();
    }

    #[test]
    fn hardlink_member_typeflag_1_and_index() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("a.txt"), b"shared").unwrap();

        let entries = vec![
            entry(&root, "a.txt", b"shared", Some(10), 0o644, 0, 0),
            SelectedEntry {
                abs_path: root.join("b.txt"),
                archive_name: "b.txt".into(),
                size: 0,
                mtime_unix: Some(11),
                mode: 0o644,
                uid: 0,
                gid: 0,
                uname: String::new(),
                gname: String::new(),
                kind: crate::select::MemberKind::HardLink {
                    target: "a.txt".into(),
                },
            },
        ];
        let out = dir.path().join("hl.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();

        let index = list_tar_zstd_members(&out).unwrap();
        assert_eq!(index.get("a.txt").unwrap().data_len, 6);
        assert_eq!(index.get("b.txt").unwrap().data_len, 0);
        assert_eq!(
            extract_tar_zstd_member_bytes(&out, "a.txt").unwrap(),
            b"shared"
        );
        assert_eq!(extract_tar_zstd_member_bytes(&out, "b.txt").unwrap(), b"");

        let m = index.get("b.txt").unwrap();
        let file = File::open(&out).unwrap();
        let mut decoder = Decoder::new(file).unwrap();
        decoder.set_offset(m.tar_header_offset).unwrap();
        let mut hdr = [0u8; 512];
        read_exact_decoder(&mut decoder, &mut hdr).unwrap();
        assert_eq!(hdr[156], b'1');
        let link_end = hdr[157..257].iter().position(|&b| b == 0).unwrap_or(100);
        assert_eq!(&hdr[157..157 + link_end], b"a.txt");
        verify_tar_zstd(&out, 2).unwrap();
    }

    #[test]
    fn full_decompress_payload_contains_tar_and_index() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let entries = vec![entry(&root, "a.txt", b"payload", Some(1), 0o644, 0, 0)];
        let out = dir.path().join("full.tar.zst");
        write_tar_zstd(&out, &entries, 1).unwrap();

        let payload = decompress_tar_zstd_payload_to_tar_bytes(&out).unwrap();
        assert!(payload.len() >= 1024 + 8);
        // ustar magic at offset 257 in first header block
        assert_eq!(&payload[257..262], b"ustar");
        // RATAIDX1 magic appears after EOA
        assert!(payload.windows(8).any(|w| w == INDEX_MAGIC));
        // Last 8 bytes are index_start
        let index_start = u64::from_le_bytes(payload[payload.len() - 8..].try_into().unwrap());
        assert!(index_start < payload.len() as u64 - 8);
        assert_eq!(&payload[index_start as usize..index_start as usize + 8], INDEX_MAGIC);
    }
}
