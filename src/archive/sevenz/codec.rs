//! LZMA2 encode helpers for non-solid 7z create.
//!
//! Uses `lzma-rust2::Lzma2Writer` so each member is **one** LZMA2 codestream
//! (internal chunks OK). Input is stream-read; compressed bytes are stream-written
//! to the pack sink. Peak RAM is dominated by the encoder dictionary for the
//! chosen level (not by full file size).

use crate::error::{Error, Result};
use lzma_rust2::{Lzma2Options, Lzma2Writer};
use std::io::{Read, Write};
use std::path::Path;

/// Result of compressing one member's content to raw LZMA2.
#[derive(Debug, Clone)]
pub struct Lzma2Compressed {
    /// Raw LZMA2 payload (method id `0x21`).
    pub data: Vec<u8>,
    /// 7z LZMA2 properties byte (dict size encoding).
    pub props: u8,
    /// CRC32 of **uncompressed** data.
    pub crc32: u32,
    /// Uncompressed size.
    pub uncompressed_size: u64,
}

/// Dict size for levels 0–9 (powers of two; aligned with common 7z presets).
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

/// Build LZMA2 options for a 7z level 0–9.
pub fn options_for_level(level: u32) -> Lzma2Options {
    let level = level.min(9);
    let mut opt = Lzma2Options::with_preset(level);
    // Keep a single logical stream; Lzma2Writer still emits internal LZMA2 chunks.
    opt.chunk_size = None;
    opt.lzma_options.dict_size = dict_size_for_level(level);
    opt
}

/// Compress an in-memory buffer (tests / small fixtures).
pub fn compress_bytes(input: &[u8], level: u32) -> Result<Lzma2Compressed> {
    compress_reader(&mut &input[..], level)
}

/// Stream-read `reader` and produce one LZMA2 pack buffer.
pub fn compress_reader(reader: &mut dyn Read, level: u32) -> Result<Lzma2Compressed> {
    let mut out = Vec::new();
    let (props, content_crc, unpack_size) = compress_reader_to_writer(reader, level, &mut out)?;
    Ok(Lzma2Compressed {
        data: out,
        props,
        crc32: content_crc,
        uncompressed_size: unpack_size,
    })
}

/// Stream-read `reader`, LZMA2-encode, write pack bytes to `pack_out`.
///
/// Returns `(props_byte, content_crc, uncompressed_size)`.
pub fn compress_reader_to_writer(
    reader: &mut dyn Read,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(u8, u32, u64)> {
    let options = options_for_level(level);
    let dict_size = options.lzma_options.dict_size;
    let props = lzma2_dict_prop(dict_size);

    let mut content_hasher = crc32fast::Hasher::new();
    let mut unpack_size = 0u64;

    {
        let mut enc = Lzma2Writer::new(CountingWrite::new(pack_out), options);
        let mut buf = [0u8; 256 * 1024];
        loop {
            let n = reader.read(&mut buf).map_err(Error::Io)?;
            if n == 0 {
                break;
            }
            content_hasher.update(&buf[..n]);
            unpack_size += n as u64;
            enc.write_all(&buf[..n])
                .map_err(|e| Error::Compress(format!("LZMA2 encode write: {e}")))?;
        }
        enc.finish()
            .map_err(|e| Error::Compress(format!("LZMA2 encode finish: {e}")))?;
    }

    Ok((props, content_hasher.finalize(), unpack_size))
}

/// Compress a regular file at `path` (stream read) into an in-memory pack.
pub fn compress_path(path: &Path, level: u32) -> Result<Lzma2Compressed> {
    let mut f = std::fs::File::open(path).map_err(|e| {
        Error::Archive(format!("open {} for compress: {e}", path.display()))
    })?;
    compress_reader(&mut f, level)
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

    pub fn size(&self) -> u64 {
        self.size
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

/// Stream compress `reader` into `file`, computing pack CRC.
///
/// Returns `(props, content_crc, unpack_size, pack_crc, pack_size)`.
pub fn compress_reader_append_pack(
    reader: &mut dyn Read,
    level: u32,
    file: &mut std::fs::File,
) -> Result<(u8, u32, u64, u32, u64)> {
    let mut pack = PackCrcWriter::new(file);
    let (props, content_crc, unpack_size) =
        compress_reader_to_writer(reader, level, &mut pack)?;
    let (pack_crc, pack_size) = pack.into_crc_and_size();
    Ok((props, content_crc, unpack_size, pack_crc, pack_size))
}

/// Thin Write adapter used to satisfy Lzma2Writer ownership while writing
/// into a borrowed pack sink.
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

#[cfg(test)]
mod tests {
    use super::*;
    use lzma_rust2::Lzma2Reader;
    use std::io::Cursor;

    #[test]
    fn compress_roundtrip_small() {
        let msg = b"hello phase6 codec ".repeat(200);
        let out = compress_bytes(&msg, 1).unwrap();
        assert!(!out.data.is_empty());
        assert_eq!(out.uncompressed_size, msg.len() as u64);
        assert_eq!(out.crc32, crc32fast::hash(&msg));

        let mut decoded = Vec::new();
        let mut r = Lzma2Reader::new(Cursor::new(&out.data), dict_size_for_level(1), None);
        r.read_to_end(&mut decoded).unwrap();
        assert_eq!(decoded, msg);
    }

    #[test]
    fn dict_prop_stable() {
        let p = lzma2_dict_prop(1 << 22);
        assert_ne!(p, 0);
        assert_eq!(lzma2_dict_prop(1 << 22), p);
    }

    #[test]
    fn stream_compress_decodes() {
        let msg = b"streamed content data!!".repeat(50);
        let mut cursor = Cursor::new(msg.as_slice());
        let b = compress_reader(&mut cursor, 3).unwrap();
        let mut db = Vec::new();
        Lzma2Reader::new(Cursor::new(&b.data), dict_size_for_level(3), None)
            .read_to_end(&mut db)
            .unwrap();
        assert_eq!(db, msg.as_slice());
    }
}
