//! Seekable Zstd create format (distinct from 7z `--method zstd`).
//!
//! On-disk layout is documented in `docs/FORMAT_SEEKABLE_ZSTD.md`.
//!
//! Uncompressed payload is a length-prefixed member stream with a trailing
//! binary index; the whole payload is wrapped in the Zstd **seekable** format
//! via [`zeekstd`] (independent frames + seek table).

use crate::error::{Error, Result};
use crate::select::SelectedEntry;
use std::fs::File;
use std::io::{self, Read, Seek, Write};
use std::path::Path;
use tracing::debug;
use zeekstd::{Decoder, EncodeOptions, Encoder, FrameSizePolicy};

/// Magic prefix of the uncompressed member index block.
pub const INDEX_MAGIC: &[u8; 8] = b"RAZSIDX1";

/// Index format version.
pub const INDEX_VERSION: u32 = 1;

/// Default independent-frame size (uncompressed bytes) for seekable encoding.
pub const DEFAULT_FRAME_SIZE: u32 = 2 * 1024 * 1024; // 2 MiB

/// One member recorded in the trailer index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberIndexEntry {
    /// Archive-relative path (`/`-separated).
    pub name: String,
    /// Uncompressed offset of the first **data** byte (after name header).
    pub data_offset: u64,
    /// Uncompressed data length in bytes.
    pub data_len: u64,
}

/// Full member index parsed from a seekable-zstd archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemberIndex {
    pub version: u32,
    pub members: Vec<MemberIndexEntry>,
}

impl MemberIndex {
    /// Look up a member by exact archive name.
    pub fn get(&self, name: &str) -> Option<&MemberIndexEntry> {
        self.members.iter().find(|m| m.name == name)
    }

    /// Member names in archive order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.members.iter().map(|m| m.name.as_str())
    }
}

/// Map CLI level 0–9 to a zstd compression level (i32).
fn zstd_level(level: u32) -> i32 {
    // Match common CLI mapping: 0 ≈ store-ish / fastest, 9 strong.
    level.min(9) as i32
}

/// Write a seekable-zstd archive to `path` from selected regular files.
///
/// Streaming: each file is read in chunks into the encoder; peak RAM is
/// O(frame size + I/O buffers), not O(total archive size).
pub fn write_seekable_zstd(
    path: &Path,
    entries: &[SelectedEntry],
    level: u32,
) -> Result<()> {
    if entries.is_empty() {
        return Err(Error::EmptyArchive);
    }

    let file = File::create(path).map_err(|e| {
        Error::Archive(format!("create seekable-zstd {}: {e}", path.display()))
    })?;

    let opts = EncodeOptions::new()
        .compression_level(zstd_level(level))
        .checksum_flag(true)
        .frame_size_policy(FrameSizePolicy::Uncompressed(DEFAULT_FRAME_SIZE));

    let mut encoder = Encoder::with_opts(file, opts).map_err(|e| {
        Error::Compress(format!("seekable-zstd encoder init: {e}"))
    })?;

    let mut index_entries: Vec<MemberIndexEntry> = Vec::with_capacity(entries.len());
    let mut uncompressed_pos: u64 = 0;

    for e in entries {
        let name_bytes = e.archive_name.as_bytes();
        let name_len = name_bytes.len() as u64;
        let data_len = e.size;

        // Open first so vanished files skip the whole member.
        let mut body = if data_len > 0 {
            match File::open(&e.abs_path) {
                Ok(f) => Some(f),
                Err(err) if crate::util::is_skippable_fs_io(&err) => {
                    tracing::warn!(
                        path = %e.abs_path.display(),
                        name = %e.archive_name,
                        error = %err,
                        "skip vanished or inaccessible file in seekable-zstd"
                    );
                    continue;
                }
                Err(err) => {
                    return Err(Error::Archive(format!(
                        "open {} for seekable-zstd: {err}",
                        e.abs_path.display()
                    )));
                }
            }
        } else {
            None
        };

        // Header: name_len | name | data_len
        write_all_encoded(&mut encoder, &name_len.to_le_bytes())?;
        write_all_encoded(&mut encoder, name_bytes)?;
        write_all_encoded(&mut encoder, &data_len.to_le_bytes())?;

        let data_offset = uncompressed_pos
            .checked_add(8 + name_len + 8)
            .ok_or_else(|| Error::Archive("uncompressed offset overflow".into()))?;

        let written = if let Some(ref mut f) = body {
            stream_reader_into_encoder(&mut encoder, f, data_len, &e.abs_path)?
        } else {
            0
        };
        if written != data_len {
            return Err(Error::Archive(format!(
                "size changed while archiving {}: expected {data_len}, wrote {written}",
                e.abs_path.display()
            )));
        }

        index_entries.push(MemberIndexEntry {
            name: e.archive_name.clone(),
            data_offset,
            data_len,
        });

        uncompressed_pos = data_offset
            .checked_add(data_len)
            .ok_or_else(|| Error::Archive("uncompressed offset overflow".into()))?;

        debug!(
            name = %e.archive_name,
            data_len,
            data_offset,
            "seekable-zstd member written"
        );
    }

    // Trailing index + u64 index_start (points at INDEX_MAGIC).
    let index_start = uncompressed_pos;
    let index_bytes = encode_index(&index_entries)?;
    write_all_encoded(&mut encoder, &index_bytes)?;
    write_all_encoded(&mut encoder, &index_start.to_le_bytes())?;

    encoder.finish().map_err(|e| {
        Error::Compress(format!("seekable-zstd finish: {e}"))
    })?;

    Ok(())
}

fn encode_index(members: &[MemberIndexEntry]) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    buf.extend_from_slice(INDEX_MAGIC);
    buf.extend_from_slice(&INDEX_VERSION.to_le_bytes());
    buf.extend_from_slice(&(members.len() as u64).to_le_bytes());
    for m in members {
        let name_bytes = m.name.as_bytes();
        buf.extend_from_slice(&(name_bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(name_bytes);
        buf.extend_from_slice(&m.data_offset.to_le_bytes());
        buf.extend_from_slice(&m.data_len.to_le_bytes());
    }
    Ok(buf)
}

fn write_all_encoded<W: Write>(encoder: &mut Encoder<'_, W>, mut data: &[u8]) -> Result<()> {
    while !data.is_empty() {
        let n = encoder
            .compress(data)
            .map_err(|e| Error::Compress(format!("seekable-zstd compress: {e}")))?;
        if n == 0 {
            return Err(Error::Compress(
                "seekable-zstd encoder made no progress".into(),
            ));
        }
        data = &data[n..];
    }
    Ok(())
}

fn stream_file_into_encoder<W: Write>(
    encoder: &mut Encoder<'_, W>,
    path: &Path,
    expected_len: u64,
) -> Result<u64> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if crate::util::is_skippable_fs_io(&e) => {
            return Err(Error::Vanished(path.to_path_buf()));
        }
        Err(e) => {
            return Err(Error::Archive(format!(
                "open {} for seekable-zstd: {e}",
                path.display()
            )));
        }
    };
    stream_reader_into_encoder(encoder, &mut f, expected_len, path)
}

fn stream_reader_into_encoder<W: Write, R: Read>(
    encoder: &mut Encoder<'_, W>,
    f: &mut R,
    expected_len: u64,
    path: &Path,
) -> Result<u64> {
    let mut buf = vec![0u8; 128 * 1024];
    let mut total = 0u64;
    loop {
        let n = f.read(&mut buf).map_err(|e| {
            if crate::util::is_skippable_fs_io(&e) {
                Error::Vanished(path.to_path_buf())
            } else {
                Error::Archive(format!("read {} for seekable-zstd: {e}", path.display()))
            }
        })?;
        if n == 0 {
            break;
        }
        write_all_encoded(encoder, &buf[..n])?;
        total = total.saturating_add(n as u64);
        if total > expected_len {
            return Err(Error::Archive(format!(
                "file grew while archiving {}: expected at most {expected_len}",
                path.display()
            )));
        }
    }
    Ok(total)
}

/// Open a seekable-zstd archive and parse the member index (seek-based; no full decompress).
pub fn list_members(path: &Path) -> Result<MemberIndex> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open seekable-zstd {}: {e}", path.display()))
    })?;
    list_members_from_seekable(&mut file)
}

/// List members from any Read+Seek seekable-zstd source.
pub fn list_members_from_seekable<S: Read + Seek>(src: S) -> Result<MemberIndex> {
    let mut decoder = Decoder::new(src).map_err(|e| {
        Error::Archive(format!("seekable-zstd decode open: {e}"))
    })?;
    let decomp_size = decoder.seek_table().size_decomp();
    if decomp_size < 8 {
        return Err(Error::Archive(
            "seekable-zstd: uncompressed payload too small for index footer".into(),
        ));
    }

    // Last 8 bytes: index_start offset.
    decoder
        .set_offset(decomp_size - 8)
        .map_err(|e| Error::Archive(format!("seekable-zstd seek to index footer: {e}")))?;
    let mut off_buf = [0u8; 8];
    read_exact_decoder(&mut decoder, &mut off_buf)?;
    let index_start = u64::from_le_bytes(off_buf);
    if index_start >= decomp_size - 8 {
        return Err(Error::Archive(format!(
            "seekable-zstd: invalid index_start {index_start} (decomp_size={decomp_size})"
        )));
    }

    let index_len = (decomp_size - 8)
        .checked_sub(index_start)
        .ok_or_else(|| Error::Archive("seekable-zstd: index length underflow".into()))?;
    if index_len > 64 * 1024 * 1024 {
        return Err(Error::Archive(format!(
            "seekable-zstd: index too large ({index_len} bytes)"
        )));
    }

    decoder
        .set_offset(index_start)
        .map_err(|e| Error::Archive(format!("seekable-zstd seek to index: {e}")))?;
    let mut index_buf = vec![0u8; index_len as usize];
    read_exact_decoder(&mut decoder, &mut index_buf)?;
    parse_index(&index_buf)
}

/// Extract one member by name into `out` (seek + decode only needed frames).
pub fn extract_member(path: &Path, name: &str, out: &mut impl Write) -> Result<u64> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open seekable-zstd {}: {e}", path.display()))
    })?;
    extract_member_from_seekable(&mut file, name, out)
}

/// Extract one member from a seekable source.
pub fn extract_member_from_seekable<S: Read + Seek>(
    src: S,
    name: &str,
    out: &mut impl Write,
) -> Result<u64> {
    // Need two passes on same source: list then extract. Re-open via rewind.
    // DecodeOptions takes ownership of src; we list first with a separate open
    // for File paths — for generic S we require Seek::rewind.
    let mut src = src;
    let index = {
        let index = list_members_from_seekable(&mut src)?;
        index
    };
    let entry = index.get(name).ok_or_else(|| {
        Error::Archive(format!("seekable-zstd: member not found: {name}"))
    })?;

    src.rewind().map_err(|e| {
        Error::Archive(format!("seekable-zstd rewind for extract: {e}"))
    })?;

    let mut decoder = Decoder::new(src).map_err(|e| {
        Error::Archive(format!("seekable-zstd decode open: {e}"))
    })?;
    decoder
        .set_offset(entry.data_offset)
        .map_err(|e| Error::Archive(format!("seekable-zstd seek to member: {e}")))?;
    let limit = entry
        .data_offset
        .checked_add(entry.data_len)
        .ok_or_else(|| Error::Archive("seekable-zstd: member range overflow".into()))?;
    decoder
        .set_offset_limit(limit)
        .map_err(|e| Error::Archive(format!("seekable-zstd set limit: {e}")))?;

    let mut remaining = entry.data_len;
    let mut buf = vec![0u8; 128 * 1024];
    while remaining > 0 {
        let chunk = remaining.min(buf.len() as u64) as usize;
        let n = decoder.decompress(&mut buf[..chunk]).map_err(|e| {
            Error::Archive(format!("seekable-zstd decompress member: {e}"))
        })?;
        if n == 0 {
            return Err(Error::Archive(format!(
                "seekable-zstd: unexpected EOF extracting {name} ({remaining} bytes left)"
            )));
        }
        out.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    Ok(entry.data_len)
}

/// Extract one member fully into a `Vec<u8>`.
pub fn extract_member_bytes(path: &Path, name: &str) -> Result<Vec<u8>> {
    let index = list_members(path)?;
    let entry = index.get(name).ok_or_else(|| {
        Error::Archive(format!("seekable-zstd: member not found: {name}"))
    })?;
    if entry.data_len > usize::MAX as u64 {
        return Err(Error::Archive("member too large to buffer".into()));
    }
    let mut out = Vec::with_capacity(entry.data_len as usize);
    extract_member(path, name, &mut out)?;
    Ok(out)
}

/// Verify archive: index parses and each member extracts to `data_len` bytes
/// (content check optional via `expected` names/lens).
pub fn verify_archive(path: &Path, expected_count: usize) -> Result<()> {
    let index = list_members(path)?;
    if index.members.len() != expected_count {
        return Err(Error::Archive(format!(
            "seekable-zstd verify: expected {expected_count} members, got {}",
            index.members.len()
        )));
    }
    for m in &index.members {
        let mut sink = io::sink();
        let n = extract_member(path, &m.name, &mut sink)?;
        if n != m.data_len {
            return Err(Error::Archive(format!(
                "seekable-zstd verify: {} length mismatch: index {} extract {n}",
                m.name, m.data_len
            )));
        }
    }
    Ok(())
}

fn parse_index(buf: &[u8]) -> Result<MemberIndex> {
    if buf.len() < 8 + 4 + 8 {
        return Err(Error::Archive(
            "seekable-zstd: index truncated (header)".into(),
        ));
    }
    if &buf[0..8] != INDEX_MAGIC {
        return Err(Error::Archive(format!(
            "seekable-zstd: bad index magic (expected RAZSIDX1)"
        )));
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != INDEX_VERSION {
        return Err(Error::Archive(format!(
            "seekable-zstd: unsupported index version {version}"
        )));
    }
    let count = u64::from_le_bytes(buf[12..20].try_into().unwrap());
    let mut pos = 20usize;
    let mut members = Vec::with_capacity(count as usize);
    for i in 0..count {
        if pos + 8 > buf.len() {
            return Err(Error::Archive(format!(
                "seekable-zstd: index truncated at member {i} name_len"
            )));
        }
        let name_len = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap()) as usize;
        pos += 8;
        if pos + name_len + 16 > buf.len() {
            return Err(Error::Archive(format!(
                "seekable-zstd: index truncated at member {i} body"
            )));
        }
        let name = std::str::from_utf8(&buf[pos..pos + name_len])
            .map_err(|e| Error::Archive(format!("seekable-zstd: invalid UTF-8 name: {e}")))?
            .to_string();
        pos += name_len;
        let data_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let data_len = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        members.push(MemberIndexEntry {
            name,
            data_offset,
            data_len,
        });
    }
    if pos != buf.len() {
        return Err(Error::Archive(format!(
            "seekable-zstd: index has {} trailing bytes",
            buf.len() - pos
        )));
    }
    Ok(MemberIndex {
        version,
        members,
    })
}

fn read_exact_decoder<S: zeekstd::Seekable>(
    decoder: &mut Decoder<'_, S>,
    buf: &mut [u8],
) -> Result<()> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = decoder.decompress(&mut buf[filled..]).map_err(|e| {
            Error::Archive(format!("seekable-zstd decompress: {e}"))
        })?;
        if n == 0 {
            return Err(Error::Archive(
                "seekable-zstd: unexpected EOF while reading".into(),
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

    fn entry(dir: &Path, rel: &str, data: &[u8]) -> SelectedEntry {
        let abs = dir.join(rel);
        if let Some(p) = abs.parent() {
            fs::create_dir_all(p).unwrap();
        }
        fs::write(&abs, data).unwrap();
        SelectedEntry {
            abs_path: abs,
            archive_name: rel.replace('\\', "/"),
            size: data.len() as u64,
            mtime_unix: None,
            mode: 0o644,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: crate::select::MemberKind::File,
        }
    }

    #[test]
    fn roundtrip_multi_member() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let entries = vec![
            entry(&root, "a.txt", b"hello seekable"),
            entry(&root, "sub/b.txt", b"nested data"),
            entry(&root, "empty.dat", b""),
        ];
        let out = dir.path().join("out.zst");
        write_seekable_zstd(&out, &entries, 1).unwrap();
        assert!(out.exists());

        let index = list_members(&out).unwrap();
        assert_eq!(index.members.len(), 3);
        let mut names: Vec<_> = index.names().map(|s| s.to_string()).collect();
        names.sort();
        assert_eq!(names, vec!["a.txt", "empty.dat", "sub/b.txt"]);

        assert_eq!(
            extract_member_bytes(&out, "a.txt").unwrap(),
            b"hello seekable"
        );
        assert_eq!(
            extract_member_bytes(&out, "sub/b.txt").unwrap(),
            b"nested data"
        );
        assert_eq!(extract_member_bytes(&out, "empty.dat").unwrap(), b"");
        verify_archive(&out, 3).unwrap();
    }

    #[test]
    fn missing_member_errors() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let entries = vec![entry(&root, "only.txt", b"x")];
        let out = dir.path().join("out.zst");
        write_seekable_zstd(&out, &entries, 1).unwrap();
        let err = extract_member_bytes(&out, "nope").unwrap_err();
        assert!(matches!(err, Error::Archive(_)));
    }
}
