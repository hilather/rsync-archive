//! Per-file compress helpers for non-solid 7z create (LZMA2 / Zstd / LZ4).
//!
//! Each member is **one independent pack** (file-level random access). Zstd uses
//! standard frames via libzstd (`zstd` crate); non-solid multi-file layout provides
//! seek-to-member. True single-stream seekable-zstd (zeekstd) remains optional later.

use super::method::CompressMethod;
use crate::error::{Error, Result};
use lzma_rust2::{Lzma2Options, Lzma2Writer};
use std::io::{Read, Write};
use std::path::Path;

/// Compressed pack payload ready for a non-solid 7z member.
#[derive(Debug, Clone)]
pub struct CompressedPack {
    pub data: Vec<u8>,
    pub method_id: Vec<u8>,
    pub method_props: Vec<u8>,
    pub crc32: u32,
    pub uncompressed_size: u64,
}

/// Backward-compatible alias used by older call sites / tests.
pub type Lzma2Compressed = CompressedPack;

/// Dict size for LZMA2 levels 0–9.
pub fn dict_size_for_level(level: u32) -> u32 {
    let level = level.min(9);
    match level {
        0 => 64 * 1024,
        1 => 1 << 16,
        2 => 1 << 18,
        3 => 1 << 20,
        4 => 1 << 20,
        5 => 1 << 22,
        6 => 1 << 23,
        7 => 1 << 24,
        8 => 1 << 25,
        _ => 1 << 26,
    }
}

/// Encode dict_size as the single LZMA2 property byte used by 7-Zip.
pub fn lzma2_dict_prop(dict_size: u32) -> u8 {
    let dict_size = dict_size.clamp(4096, 0xFFFF_FFF0);
    let lead = dict_size.leading_zeros();
    let second_bit = (dict_size >> (30u32.wrapping_sub(lead))).wrapping_sub(2);
    (19u32.wrapping_sub(lead) * 2 + second_bit) as u8
}

fn lzma2_options(level: u32) -> Lzma2Options {
    let level = level.min(9);
    let mut opt = Lzma2Options::with_preset(level);
    opt.chunk_size = None;
    opt.lzma_options.dict_size = dict_size_for_level(level);
    opt
}

/// Map create `--level` 0–9 to Zstd levels (approx 1–19).
fn zstd_level(level: u32) -> i32 {
    // 0→1 … 9→19-ish (zstd max useful ~22)
    let level = level.min(9);
    match level {
        0 => 1,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 5,
        5 => 7,
        6 => 9,
        7 => 12,
        8 => 15,
        _ => 19,
    }
}

/// Compress bytes with the selected method.
pub fn compress_bytes(input: &[u8], method: CompressMethod, level: u32) -> Result<CompressedPack> {
    compress_reader(&mut &input[..], method, level)
}

/// Stream-read and compress a path.
pub fn compress_path(path: &Path, method: CompressMethod, level: u32) -> Result<CompressedPack> {
    let mut f = std::fs::File::open(path).map_err(|e| {
        Error::Archive(format!("open {} for compress: {e}", path.display()))
    })?;
    compress_reader(&mut f, method, level)
}

/// Stream-read `reader` into an in-memory compressed pack.
pub fn compress_reader(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
) -> Result<CompressedPack> {
    let mut out = Vec::new();
    let (props, content_crc, unpack_size) =
        compress_reader_to_writer(reader, method, level, &mut out)?;
    Ok(CompressedPack {
        data: out,
        method_id: method.method_id().to_vec(),
        method_props: props,
        crc32: content_crc,
        uncompressed_size: unpack_size,
    })
}

/// Stream compress into `pack_out`. Returns `(method_props, content_crc, unpack_size)`.
pub fn compress_reader_to_writer(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    match method {
        CompressMethod::Lzma2 => compress_lzma2(reader, level, pack_out),
        CompressMethod::Zstd => compress_zstd(reader, level, pack_out),
        CompressMethod::Lz4 => compress_lz4(reader, pack_out),
    }
}

/// Stream compress into file with pack CRC.
///
/// Returns `(props, content_crc, unpack_size, pack_crc, pack_size)`.
pub fn compress_reader_append_pack(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    file: &mut std::fs::File,
) -> Result<(Vec<u8>, u32, u64, u32, u64)> {
    let mut pack = PackCrcWriter::new(file);
    let (props, content_crc, unpack_size) =
        compress_reader_to_writer(reader, method, level, &mut pack)?;
    let (pack_crc, pack_size) = pack.into_crc_and_size();
    Ok((props, content_crc, unpack_size, pack_crc, pack_size))
}

fn compress_lzma2(
    reader: &mut dyn Read,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    let options = lzma2_options(level);
    let props = vec![lzma2_dict_prop(options.lzma_options.dict_size)];
    let mut content_hasher = crc32fast::Hasher::new();
    let mut unpack_size = 0u64;
    {
        let mut enc = Lzma2Writer::new(CountingWrite::new(pack_out), options);
        copy_hashed(reader, &mut enc, &mut content_hasher, &mut unpack_size)?;
        enc.finish()
            .map_err(|e| Error::Compress(format!("LZMA2 finish: {e}")))?;
    }
    Ok((props, content_hasher.finalize(), unpack_size))
}

fn compress_zstd(
    reader: &mut dyn Read,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    let zlevel = zstd_level(level);
    // 7z ZSTD props: major, minor, level (matches sevenz-rust2)
    let props = vec![
        zstd::zstd_safe::VERSION_MAJOR as u8,
        zstd::zstd_safe::VERSION_MINOR as u8,
        zlevel.clamp(1, 22) as u8,
    ];
    let mut content_hasher = crc32fast::Hasher::new();
    let mut unpack_size = 0u64;
    {
        let mut enc = zstd::stream::write::Encoder::new(CountingWrite::new(pack_out), zlevel)
            .map_err(|e| Error::Compress(format!("zstd encoder: {e}")))?;
        // Independent frame per member (file-level seek in non-solid 7z).
        enc.include_checksum(true)
            .map_err(|e| Error::Compress(format!("zstd checksum: {e}")))?;
        copy_hashed(reader, &mut enc, &mut content_hasher, &mut unpack_size)?;
        enc.finish()
            .map_err(|e| Error::Compress(format!("zstd finish: {e}")))?;
    }
    Ok((props, content_hasher.finalize(), unpack_size))
}

fn compress_lz4(
    reader: &mut dyn Read,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    // sevenz-rust2 / 7-Zip-zstd props: major=1, minor=0, level=3 (fast)
    let props = vec![1u8, 0u8, 3u8];
    let mut content_hasher = crc32fast::Hasher::new();
    let mut unpack_size = 0u64;
    // Standard LZ4 frame (lz4_flex FrameEncoder) — decoded by sevenz-rust2.
    {
        let mut enc = lz4_flex::frame::FrameEncoder::new(CountingWrite::new(pack_out));
        copy_hashed(reader, &mut enc, &mut content_hasher, &mut unpack_size)?;
        enc.finish()
            .map_err(|e| Error::Compress(format!("lz4 finish: {e}")))?;
    }
    Ok((props, content_hasher.finalize(), unpack_size))
}

fn copy_hashed(
    reader: &mut dyn Read,
    writer: &mut dyn Write,
    content_hasher: &mut crc32fast::Hasher,
    unpack_size: &mut u64,
) -> Result<()> {
    let mut buf = [0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut buf).map_err(Error::Io)?;
        if n == 0 {
            break;
        }
        content_hasher.update(&buf[..n]);
        *unpack_size += n as u64;
        writer
            .write_all(&buf[..n])
            .map_err(|e| Error::Compress(format!("encode write: {e}")))?;
    }
    Ok(())
}

/// Writer that forwards to an inner sink and tracks pack CRC + size.
pub struct PackCrcWriter<'a, W: Write> {
    inner: &'a mut W,
    hasher: crc32fast::Hasher,
    size: u64,
}

impl<'a, W: Write> PackCrcWriter<'a, W> {
    pub fn new(inner: &'a mut W) -> Self {
        Self {
            inner,
            hasher: crc32fast::Hasher::new(),
            size: 0,
        }
    }

    pub fn into_crc_and_size(self) -> (u32, u64) {
        (self.hasher.finalize(), self.size)
    }
}

impl<W: Write> Write for PackCrcWriter<'_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write_all(buf)?;
        self.hasher.update(buf);
        self.size += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

struct CountingWrite<'a> {
    inner: &'a mut dyn Write,
}

impl<'a> CountingWrite<'a> {
    fn new(inner: &'a mut dyn Write) -> Self {
        Self { inner }
    }
}

impl Write for CountingWrite<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.inner.write_all(buf)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

// --- legacy helpers used by sequential path ---

#[cfg(test)]
mod tests {
    use super::*;
    use lzma_rust2::Lzma2Reader;
    use std::io::Cursor;

    #[test]
    fn lzma2_roundtrip() {
        let msg = b"hello multi codec ".repeat(100);
        let out = compress_bytes(&msg, CompressMethod::Lzma2, 1).unwrap();
        assert_eq!(out.method_id, vec![0x21]);
        let mut decoded = Vec::new();
        Lzma2Reader::new(Cursor::new(&out.data), dict_size_for_level(1), None)
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn zstd_roundtrip() {
        let msg = b"zstd member payload!!".repeat(80);
        let out = compress_bytes(&msg, CompressMethod::Zstd, 3).unwrap();
        assert_eq!(out.method_id, CompressMethod::Zstd.method_id());
        assert_eq!(out.method_props.len(), 3);
        let decoded = zstd::stream::decode_all(out.data.as_slice()).unwrap();
        assert_eq!(decoded, msg);
        assert_eq!(out.crc32, crc32fast::hash(&msg));
    }

    #[test]
    fn lz4_roundtrip() {
        let msg = b"lz4 fast payload".repeat(50);
        let out = compress_bytes(&msg, CompressMethod::Lz4, 1).unwrap();
        assert_eq!(out.method_id, CompressMethod::Lz4.method_id());
        let mut decoded = Vec::new();
        lz4_flex::frame::FrameDecoder::new(out.data.as_slice())
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn dict_prop_stable() {
        assert_eq!(lzma2_dict_prop(1 << 22), lzma2_dict_prop(1 << 22));
    }
}
