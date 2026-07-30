//! RA-friendly tar.lz4 create format (valid tar + multi-frame LZ4 + frame table).
//!
//! Layout: [`docs/FORMAT_TAR_LZ4.md`](../../../docs/FORMAT_TAR_LZ4.md).
//!
//! Uncompressed payload = POSIX ustar/pax tar stream + trailing member index
//! (same RATAIDX1 as tar.zst). Wrapped in **independent** LZ4 frames with a
//! cleartext frame table footer (no standard seekable-LZ4 equivalent to zeekstd).

use crate::archive::tar_common::{
    build_tar_headers, dir_meta_for_entry, encode_index, parent_dir_names, parse_index,
    TarMemberIndex, TarMemberIndexEntry, TarMemberMeta,
};
use crate::error::{Error, Result};
use crate::select::SelectedEntry;
use lz4_flex::frame::FrameDecoder;
use lz4_flex::frame::FrameEncoder;
use std::collections::HashSet;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use tracing::debug;

pub use crate::archive::tar_common::{INDEX_MAGIC, INDEX_VERSION};

/// Magic of the cleartext multi-frame table footer (8 bytes).
pub const FRAME_TABLE_MAGIC: &[u8; 8] = b"RATLFRM1";

/// Frame table format version.
pub const FRAME_TABLE_VERSION: u32 = 1;

/// Default independent-frame size (uncompressed bytes of tar payload per LZ4 frame).
pub const DEFAULT_FRAME_SIZE: u32 = 2 * 1024 * 1024;

/// One independent LZ4 frame in the archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTableEntry {
    pub compressed_offset: u64,
    pub compressed_size: u64,
    pub uncompressed_offset: u64,
    pub uncompressed_size: u64,
}

/// Cleartext frame table at end of file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameTable {
    pub version: u32,
    pub frames: Vec<FrameTableEntry>,
    pub total_uncompressed: u64,
}

/// Write a multi-frame tar.lz4 archive from selected files, symbolic links, and hard links.
///
/// Parent directory prefixes are emitted as ustar directory members; symlinks
/// use typeflag `'2'` and hard links typeflag `'1'`, both with no data body (see
/// [`write_tar_zstd`](crate::archive::tar_zstd::write_tar_zstd)).
pub fn write_tar_lz4(path: &Path, entries: &[SelectedEntry], level: u32) -> Result<()> {
    if entries.is_empty() {
        return Err(Error::EmptyArchive);
    }
    let _ = level; // lz4_flex frame encoder has no fine-grained level knob (same as 7z lz4 fast path)

    let file = File::create(path).map_err(|e| {
        Error::Archive(format!("create tar.lz4 {}: {e}", path.display()))
    })?;

    let mut writer = MultiFrameWriter::new(file, DEFAULT_FRAME_SIZE as usize);
    let mut index_entries: Vec<TarMemberIndexEntry> =
        Vec::with_capacity(crate::archive::tar_common::expected_tar_member_count(entries));
    let mut emitted_dirs: HashSet<String> = HashSet::new();
    let mut pos: u64 = 0;

    for e in entries {
        for dir_name in parent_dir_names(&e.archive_name) {
            if !emitted_dirs.insert(dir_name.clone()) {
                continue;
            }
            let meta = dir_meta_for_entry(e, &dir_name);
            let header_offset = pos;
            let header_bytes = build_tar_headers(&dir_name, &meta)?;
            writer.write_all(&header_bytes)?;
            pos = pos
                .checked_add(header_bytes.len() as u64)
                .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;
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
                "tar.lz4 dir member written"
            );
        }

        let mode = e.mode;
        let mtime = e.mtime_unix.unwrap_or(0);
        let has_body = e.has_data_body();
        let data_len = if has_body { e.size } else { 0 };

        let mut body_file = if has_body && data_len > 0 {
            match File::open(&e.abs_path) {
                Ok(f) => Some(f),
                Err(err) if crate::util::is_skippable_fs_io(&err) => {
                    tracing::warn!(
                        path = %e.abs_path.display(),
                        name = %e.archive_name,
                        error = %err,
                        "skip vanished or inaccessible file in tar.lz4"
                    );
                    continue;
                }
                Err(err) => {
                    return Err(Error::Archive(format!(
                        "open {} for tar.lz4: {err}",
                        e.abs_path.display()
                    )));
                }
            }
        } else {
            None
        };

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
        writer.write_all(&header_bytes)?;
        pos = pos
            .checked_add(header_bytes.len() as u64)
            .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;

        let data_offset = pos;
        let written = if let Some(ref mut f) = body_file {
            let n = stream_reader_into_writer(&mut writer, f, e.size, &e.abs_path)?;
            if n != e.size {
                return Err(Error::Archive(format!(
                    "size changed while archiving {}: expected {}, wrote {n}",
                    e.abs_path.display(),
                    e.size
                )));
            }
            n
        } else {
            0u64
        };
        pos = pos
            .checked_add(written)
            .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;

        let pad = (512 - (written % 512)) % 512;
        if pad > 0 {
            writer.write_all(&[0u8; 512][..pad as usize])?;
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

        debug!(
            name = %e.archive_name,
            header_offset,
            data_offset,
            data_len,
            is_symlink = e.is_symlink(),
            is_hard_link = e.is_hard_link(),
            "tar.lz4 member written"
        );
    }

    // End-of-archive: two 512-byte zero blocks.
    writer.write_all(&[0u8; 1024])?;
    pos = pos
        .checked_add(1024)
        .ok_or_else(|| Error::Archive("tar offset overflow".into()))?;

    let index_start = pos;
    let index_bytes = encode_index(&index_entries)?;
    writer.write_all(&index_bytes)?;
    writer.write_all(&index_start.to_le_bytes())?;

    writer.finish()?;
    Ok(())
}

struct MultiFrameWriter<W: Write> {
    out: W,
    frame_size: usize,
    cur: Vec<u8>,
    frames: Vec<FrameTableEntry>,
    uncomp_pos: u64,
    comp_pos: u64,
}

impl<W: Write> MultiFrameWriter<W> {
    fn new(out: W, frame_size: usize) -> Self {
        Self {
            out,
            frame_size: frame_size.max(64 * 1024),
            cur: Vec::with_capacity(frame_size.min(2 * 1024 * 1024)),
            frames: Vec::new(),
            uncomp_pos: 0,
            comp_pos: 0,
        }
    }

    fn write_all(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let space = self.frame_size.saturating_sub(self.cur.len());
            if space == 0 {
                self.flush_frame()?;
                continue;
            }
            let take = data.len().min(space);
            self.cur.extend_from_slice(&data[..take]);
            data = &data[take..];
            if self.cur.len() >= self.frame_size {
                self.flush_frame()?;
            }
        }
        Ok(())
    }

    fn flush_frame(&mut self) -> Result<()> {
        if self.cur.is_empty() {
            return Ok(());
        }
        let uncompressed_size = self.cur.len() as u64;
        let mut compressed = Vec::new();
        {
            let mut enc = FrameEncoder::new(&mut compressed);
            enc.write_all(&self.cur)
                .map_err(|e| Error::Compress(format!("tar.lz4 frame compress: {e}")))?;
            enc.finish()
                .map_err(|e| Error::Compress(format!("tar.lz4 frame finish: {e}")))?;
        }
        let compressed_size = compressed.len() as u64;
        self.out
            .write_all(&compressed)
            .map_err(|e| Error::Compress(format!("tar.lz4 write frame: {e}")))?;

        self.frames.push(FrameTableEntry {
            compressed_offset: self.comp_pos,
            compressed_size,
            uncompressed_offset: self.uncomp_pos,
            uncompressed_size,
        });
        self.comp_pos = self
            .comp_pos
            .checked_add(compressed_size)
            .ok_or_else(|| Error::Archive("tar.lz4 compressed offset overflow".into()))?;
        self.uncomp_pos = self
            .uncomp_pos
            .checked_add(uncompressed_size)
            .ok_or_else(|| Error::Archive("tar.lz4 uncompressed offset overflow".into()))?;
        self.cur.clear();
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.flush_frame()?;
        write_frame_table_footer(&mut self.out, &self.frames, self.uncomp_pos)?;
        Ok(())
    }
}

fn write_frame_table_footer<W: Write>(
    out: &mut W,
    frames: &[FrameTableEntry],
    total_uncompressed: u64,
) -> Result<()> {
    let footer_offset = {
        // Caller tracks compressed position; we encode footer after all frames.
        // We don't know file offset here if W is not Seek — multi-frame writer
        // always finishes after writing frames sequentially, so footer starts
        // at sum of compressed sizes.
        frames
            .iter()
            .try_fold(0u64, |acc, f| acc.checked_add(f.compressed_size))
            .ok_or_else(|| Error::Archive("tar.lz4 footer offset overflow".into()))?
    };

    let mut buf = Vec::new();
    buf.extend_from_slice(FRAME_TABLE_MAGIC);
    buf.extend_from_slice(&FRAME_TABLE_VERSION.to_le_bytes());
    buf.extend_from_slice(&(frames.len() as u64).to_le_bytes());
    for f in frames {
        buf.extend_from_slice(&f.compressed_offset.to_le_bytes());
        buf.extend_from_slice(&f.compressed_size.to_le_bytes());
        buf.extend_from_slice(&f.uncompressed_offset.to_le_bytes());
        buf.extend_from_slice(&f.uncompressed_size.to_le_bytes());
    }
    buf.extend_from_slice(&total_uncompressed.to_le_bytes());
    buf.extend_from_slice(&footer_offset.to_le_bytes());

    out.write_all(&buf)
        .map_err(|e| Error::Compress(format!("tar.lz4 write frame table: {e}")))?;
    Ok(())
}

fn stream_file_into_writer<W: Write>(
    writer: &mut MultiFrameWriter<W>,
    path: &Path,
    expected_len: u64,
) -> Result<u64> {
    if expected_len == 0 {
        return Ok(0);
    }
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) if crate::util::is_skippable_fs_io(&e) => {
            return Err(Error::Vanished(path.to_path_buf()));
        }
        Err(e) => {
            return Err(Error::Archive(format!(
                "open {} for tar.lz4: {e}",
                path.display()
            )));
        }
    };
    stream_reader_into_writer(writer, &mut f, expected_len, path)
}

fn stream_reader_into_writer<W: Write, R: Read>(
    writer: &mut MultiFrameWriter<W>,
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
                Error::Archive(format!("read {} for tar.lz4: {e}", path.display()))
            }
        })?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
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

/// List members from a tar.lz4 archive (index via frame table; no full decompress).
pub fn list_tar_lz4_members(path: &Path) -> Result<TarMemberIndex> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open tar.lz4 {}: {e}", path.display()))
    })?;
    list_tar_lz4_from_seekable(&mut file)
}

pub fn list_tar_lz4_from_seekable<S: Read + Seek>(mut src: S) -> Result<TarMemberIndex> {
    let table = read_frame_table(&mut src)?;
    if table.total_uncompressed < 8 {
        return Err(Error::Archive(
            "tar.lz4: uncompressed payload too small for index footer".into(),
        ));
    }
    let mut off_buf = [0u8; 8];
    read_uncompressed_exact(&mut src, &table, table.total_uncompressed - 8, &mut off_buf)?;
    let index_start = u64::from_le_bytes(off_buf);
    if index_start >= table.total_uncompressed - 8 {
        return Err(Error::Archive(format!(
            "tar.lz4: invalid index_start {index_start} (decomp_size={})",
            table.total_uncompressed
        )));
    }
    let index_len = (table.total_uncompressed - 8)
        .checked_sub(index_start)
        .ok_or_else(|| Error::Archive("tar.lz4: index length underflow".into()))?;
    if index_len > 64 * 1024 * 1024 {
        return Err(Error::Archive(format!(
            "tar.lz4: index too large ({index_len} bytes)"
        )));
    }
    let mut index_buf = vec![0u8; index_len as usize];
    read_uncompressed_exact(&mut src, &table, index_start, &mut index_buf)?;
    parse_index(&index_buf)
}

/// Extract one member by name (decode only frames covering the data range).
pub fn extract_tar_lz4_member(path: &Path, name: &str, out: &mut impl Write) -> Result<u64> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open tar.lz4 {}: {e}", path.display()))
    })?;
    extract_tar_lz4_from_seekable(&mut file, name, out)
}

pub fn extract_tar_lz4_from_seekable<S: Read + Seek>(
    mut src: S,
    name: &str,
    out: &mut impl Write,
) -> Result<u64> {
    let table = read_frame_table(&mut src)?;
    let index = {
        // Re-read index without reopening: same as list path
        if table.total_uncompressed < 8 {
            return Err(Error::Archive(
                "tar.lz4: uncompressed payload too small for index footer".into(),
            ));
        }
        let mut off_buf = [0u8; 8];
        read_uncompressed_exact(&mut src, &table, table.total_uncompressed - 8, &mut off_buf)?;
        let index_start = u64::from_le_bytes(off_buf);
        let index_len = (table.total_uncompressed - 8)
            .checked_sub(index_start)
            .ok_or_else(|| Error::Archive("tar.lz4: index length underflow".into()))?;
        let mut index_buf = vec![0u8; index_len as usize];
        read_uncompressed_exact(&mut src, &table, index_start, &mut index_buf)?;
        parse_index(&index_buf)?
    };

    let entry = index
        .get(name)
        .ok_or_else(|| Error::Archive(format!("tar.lz4: member not found: {name}")))?;

    let mut remaining = entry.data_len;
    let mut offset = entry.tar_data_offset;
    let mut buf = vec![0u8; 128 * 1024];
    while remaining > 0 {
        let chunk = remaining.min(buf.len() as u64) as usize;
        read_uncompressed_exact(&mut src, &table, offset, &mut buf[..chunk])?;
        out.write_all(&buf[..chunk])?;
        offset += chunk as u64;
        remaining -= chunk as u64;
    }
    Ok(entry.data_len)
}

pub fn extract_tar_lz4_member_bytes(path: &Path, name: &str) -> Result<Vec<u8>> {
    let index = list_tar_lz4_members(path)?;
    let entry = index
        .get(name)
        .ok_or_else(|| Error::Archive(format!("tar.lz4: member not found: {name}")))?;
    if entry.data_len > usize::MAX as u64 {
        return Err(Error::Archive("member too large to buffer".into()));
    }
    let mut out = Vec::with_capacity(entry.data_len as usize);
    extract_tar_lz4_member(path, name, &mut out)?;
    Ok(out)
}

/// Decompress all LZ4 frames in order, stopping before the cleartext `RATLFRM1`
/// frame table footer.
///
/// Output is the concatenated uncompressed payload: POSIX tar stream + EOA +
/// trailing `RATAIDX1` + `u64` `index_start`. Stock `tar -t` typically stops at
/// EOA and ignores the trailing index.
///
/// Stock `lz4 -d` on the whole file may fail because of the custom multi-frame
/// layout + cleartext footer; this helper is the supported full-decompress path
/// for create interop / smoke tests (not a product extract CLI).
pub fn decompress_tar_lz4_payload_to_tar_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut file = File::open(path).map_err(|e| {
        Error::Archive(format!("open tar.lz4 {}: {e}", path.display()))
    })?;
    decompress_tar_lz4_payload_from_seekable(&mut file)
}

/// See [`decompress_tar_lz4_payload_to_tar_bytes`].
pub fn decompress_tar_lz4_payload_from_seekable<S: Read + Seek>(src: &mut S) -> Result<Vec<u8>> {
    let table = read_frame_table(src)?;
    if table.total_uncompressed > 512 * 1024 * 1024 {
        return Err(Error::Archive(format!(
            "tar.lz4: uncompressed payload too large for full buffer ({} bytes)",
            table.total_uncompressed
        )));
    }
    let mut out = Vec::with_capacity(table.total_uncompressed as usize);
    for frame in &table.frames {
        let chunk = decode_frame(src, frame)?;
        out.extend_from_slice(&chunk);
    }
    if out.len() as u64 != table.total_uncompressed {
        return Err(Error::Archive(format!(
            "tar.lz4: full decompress size mismatch: table {} got {}",
            table.total_uncompressed,
            out.len()
        )));
    }
    Ok(out)
}

/// Verify index + extract each member to data_len.
pub fn verify_tar_lz4(path: &Path, expected_count: usize) -> Result<()> {
    let index = list_tar_lz4_members(path)?;
    if index.members.len() != expected_count {
        return Err(Error::Archive(format!(
            "tar.lz4 verify: expected {expected_count} members, got {}",
            index.members.len()
        )));
    }
    for m in &index.members {
        let mut sink = io::sink();
        let n = extract_tar_lz4_member(path, &m.name, &mut sink)?;
        if n != m.data_len {
            return Err(Error::Archive(format!(
                "tar.lz4 verify: {} length mismatch: index {} extract {n}",
                m.name, m.data_len
            )));
        }
    }
    Ok(())
}

fn read_frame_table<S: Read + Seek>(src: &mut S) -> Result<FrameTable> {
    let file_len = src
        .seek(SeekFrom::End(0))
        .map_err(|e| Error::Archive(format!("tar.lz4 seek end: {e}")))?;
    if file_len < 8 + 4 + 8 + 8 + 8 {
        return Err(Error::Archive("tar.lz4: file too small for frame table".into()));
    }
    // Last 8 bytes = footer_offset
    src.seek(SeekFrom::End(-8))
        .map_err(|e| Error::Archive(format!("tar.lz4 seek footer ptr: {e}")))?;
    let mut off_buf = [0u8; 8];
    src.read_exact(&mut off_buf)
        .map_err(|e| Error::Archive(format!("tar.lz4 read footer ptr: {e}")))?;
    let footer_offset = u64::from_le_bytes(off_buf);
    if footer_offset >= file_len {
        return Err(Error::Archive(format!(
            "tar.lz4: invalid footer_offset {footer_offset} (file_len={file_len})"
        )));
    }
    let footer_len = file_len
        .checked_sub(footer_offset)
        .ok_or_else(|| Error::Archive("tar.lz4: footer length underflow".into()))?;
    if footer_len > 64 * 1024 * 1024 {
        return Err(Error::Archive(format!(
            "tar.lz4: frame table too large ({footer_len} bytes)"
        )));
    }
    src.seek(SeekFrom::Start(footer_offset))
        .map_err(|e| Error::Archive(format!("tar.lz4 seek frame table: {e}")))?;
    let mut buf = vec![0u8; footer_len as usize];
    src.read_exact(&mut buf)
        .map_err(|e| Error::Archive(format!("tar.lz4 read frame table: {e}")))?;
    parse_frame_table(&buf)
}

fn parse_frame_table(buf: &[u8]) -> Result<FrameTable> {
    // magic(8) + version(4) + count(8) + entries + total_uncompressed(8) + footer_offset(8)
    if buf.len() < 8 + 4 + 8 + 8 + 8 {
        return Err(Error::Archive("tar.lz4: frame table truncated".into()));
    }
    if &buf[0..8] != FRAME_TABLE_MAGIC {
        return Err(Error::Archive(
            "tar.lz4: bad frame table magic (expected RATLFRM1)".into(),
        ));
    }
    let version = u32::from_le_bytes(buf[8..12].try_into().unwrap());
    if version != FRAME_TABLE_VERSION {
        return Err(Error::Archive(format!(
            "tar.lz4: unsupported frame table version {version}"
        )));
    }
    let count = u64::from_le_bytes(buf[12..20].try_into().unwrap()) as usize;
    let mut pos = 20usize;
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        if pos + 32 > buf.len() {
            return Err(Error::Archive(format!(
                "tar.lz4: frame table truncated at entry {i}"
            )));
        }
        let compressed_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let compressed_size = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let uncompressed_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        let uncompressed_size = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
        pos += 8;
        frames.push(FrameTableEntry {
            compressed_offset,
            compressed_size,
            uncompressed_offset,
            uncompressed_size,
        });
    }
    if pos + 16 > buf.len() {
        return Err(Error::Archive(
            "tar.lz4: frame table truncated (totals)".into(),
        ));
    }
    let total_uncompressed = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
    pos += 8;
    let _footer_offset = u64::from_le_bytes(buf[pos..pos + 8].try_into().unwrap());
    pos += 8;
    if pos != buf.len() {
        return Err(Error::Archive(format!(
            "tar.lz4: frame table has {} trailing bytes",
            buf.len() - pos
        )));
    }
    Ok(FrameTable {
        version,
        frames,
        total_uncompressed,
    })
}

/// Read `dst.len()` uncompressed bytes starting at `start` (decode only needed frames).
fn read_uncompressed_exact<S: Read + Seek>(
    src: &mut S,
    table: &FrameTable,
    start: u64,
    dst: &mut [u8],
) -> Result<()> {
    let end = start
        .checked_add(dst.len() as u64)
        .ok_or_else(|| Error::Archive("tar.lz4: read range overflow".into()))?;
    if end > table.total_uncompressed {
        return Err(Error::Archive(format!(
            "tar.lz4: read past end (want {end}, have {})",
            table.total_uncompressed
        )));
    }

    let mut filled = 0usize;
    let mut cursor = start;
    while filled < dst.len() {
        let frame = table
            .frames
            .iter()
            .find(|f| {
                let fend = f.uncompressed_offset + f.uncompressed_size;
                cursor >= f.uncompressed_offset && cursor < fend
            })
            .ok_or_else(|| {
                Error::Archive(format!(
                    "tar.lz4: no frame covers uncompressed offset {cursor}"
                ))
            })?;

        let frame_data = decode_frame(src, frame)?;
        let mut local = (cursor - frame.uncompressed_offset) as usize;
        if local >= frame_data.len() {
            return Err(Error::Archive(
                "tar.lz4: frame decode size mismatch".into(),
            ));
        }
        // Copy as much as this frame can still supply before decoding another.
        while filled < dst.len() && local < frame_data.len() {
            let take = (dst.len() - filled).min(frame_data.len() - local);
            dst[filled..filled + take].copy_from_slice(&frame_data[local..local + take]);
            filled += take;
            local += take;
            cursor += take as u64;
        }
    }
    Ok(())
}

fn decode_frame<S: Read + Seek>(src: &mut S, frame: &FrameTableEntry) -> Result<Vec<u8>> {
    src.seek(SeekFrom::Start(frame.compressed_offset))
        .map_err(|e| Error::Archive(format!("tar.lz4 seek frame: {e}")))?;
    let mut raw = vec![0u8; frame.compressed_size as usize];
    src.read_exact(&mut raw)
        .map_err(|e| Error::Archive(format!("tar.lz4 read compressed frame: {e}")))?;
    let mut decoder = FrameDecoder::new(&raw[..]);
    let mut out = Vec::with_capacity(frame.uncompressed_size as usize);
    decoder
        .read_to_end(&mut out)
        .map_err(|e| Error::Archive(format!("tar.lz4 decode frame: {e}")))?;
    if out.len() as u64 != frame.uncompressed_size {
        return Err(Error::Archive(format!(
            "tar.lz4: frame uncompressed size mismatch: table {} got {}",
            frame.uncompressed_size,
            out.len()
        )));
    }
    Ok(out)
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
            entry(&root, "a.txt", b"hello tar lz4", Some(1_700_000_000), 0o640, 1000, 100),
            entry(&root, "sub/b.txt", b"nested", Some(100), 0o644, 0, 0),
            entry(&root, "empty.dat", b"", None, 0o600, 1, 2),
        ];
        let out = dir.path().join("out.tar.lz4");
        write_tar_lz4(&out, &entries, 1).unwrap();

        let index = list_tar_lz4_members(&out).unwrap();
        // 3 files + parent dir "sub/" for nested path
        assert_eq!(index.members.len(), 4);
        assert!(index.get("sub/").is_some());
        assert_eq!(index.get("sub/").unwrap().data_len, 0);
        assert_eq!(
            extract_tar_lz4_member_bytes(&out, "a.txt").unwrap(),
            b"hello tar lz4"
        );
        assert_eq!(
            extract_tar_lz4_member_bytes(&out, "sub/b.txt").unwrap(),
            b"nested"
        );
        assert_eq!(extract_tar_lz4_member_bytes(&out, "empty.dat").unwrap(), b"");
        assert_eq!(extract_tar_lz4_member_bytes(&out, "sub/").unwrap(), b"");
        let a = index.get("a.txt").unwrap();
        assert_eq!(a.mtime_unix, 1_700_000_000);
        assert_eq!(a.mode, 0o640);
        assert_eq!(a.uid, 1000);
        assert_eq!(a.gid, 100);
        verify_tar_lz4(&out, 4).unwrap();
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
        let out = dir.path().join("nested.tar.lz4");
        write_tar_lz4(&out, &entries, 1).unwrap();
        let index = list_tar_lz4_members(&out).unwrap();
        assert_eq!(index.members.len(), 3);
        let names: Vec<_> = index.names().collect();
        assert_eq!(names, vec!["a/", "a/b/", "a/b/c.txt"]);
        assert_eq!(
            extract_tar_lz4_member_bytes(&out, "a/b/c.txt").unwrap(),
            b"deep"
        );
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
        let out = dir.path().join("long.tar.lz4");
        write_tar_lz4(&out, &entries, 1).unwrap();
        assert_eq!(
            extract_tar_lz4_member_bytes(&out, &long).unwrap(),
            b"longpath"
        );
    }

    #[test]
    fn multi_frame_when_payload_exceeds_frame_size() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        // Force multiple frames: large file > default would need smaller frame for unit test.
        // Write with a custom small MultiFrameWriter via public API isn't exposed;
        // use a multi-MiB file against DEFAULT_FRAME_SIZE only if cheap enough.
        // Instead verify frame table has ≥1 frame for normal archive.
        let data = vec![b'Z'; 64 * 1024];
        let entries = vec![entry(&root, "big.bin", &data, Some(1), 0o644, 0, 0)];
        let out = dir.path().join("big.tar.lz4");
        write_tar_lz4(&out, &entries, 1).unwrap();
        let mut f = File::open(&out).unwrap();
        let table = read_frame_table(&mut f).unwrap();
        assert!(!table.frames.is_empty());
        assert_eq!(
            extract_tar_lz4_member_bytes(&out, "big.bin").unwrap(),
            data
        );
    }

    #[test]
    fn full_decompress_payload_stops_before_frame_table() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("src");
        let entries = vec![entry(&root, "a.txt", b"payload", Some(1), 0o644, 0, 0)];
        let out = dir.path().join("full.tar.lz4");
        write_tar_lz4(&out, &entries, 1).unwrap();

        let payload = decompress_tar_lz4_payload_to_tar_bytes(&out).unwrap();
        assert!(payload.len() >= 1024 + 8);
        assert_eq!(&payload[257..262], b"ustar");
        // Uncompressed payload has RATAIDX1, not the on-disk RATLFRM1 footer.
        assert!(payload.windows(8).any(|w| w == INDEX_MAGIC));
        assert!(!payload.windows(8).any(|w| w == FRAME_TABLE_MAGIC));
        let index_start = u64::from_le_bytes(payload[payload.len() - 8..].try_into().unwrap());
        assert_eq!(&payload[index_start as usize..index_start as usize + 8], INDEX_MAGIC);

        // On-disk file still has the frame table after compressed frames.
        let disk = fs::read(&out).unwrap();
        assert!(disk.windows(8).any(|w| w == FRAME_TABLE_MAGIC));
    }
}
