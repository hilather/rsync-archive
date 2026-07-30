//! Per-file compress helpers for non-solid 7z create (LZMA2 / Zstd / LZ4).
//!
//! Each member is **one independent pack** (file-level random access). Zstd uses
//! standard frames via libzstd (`zstd` crate); non-solid multi-file layout provides
//! seek-to-member. True single-stream seekable-zstd (zeekstd) remains optional later.
//!
//! Performance notes (OPT-07/08/09/10/11/12/12b):
//! - LZMA2 dict is clamped to member size (avoids 4 MiB setup on 8 KiB files).
//! - Optional **`liblzma`** feature: C-backed raw LZMA2 (same 7z dict prop byte).
//! - Zstd: no redundant frame checksum (7z content CRC already stored); pledged size
//!   when known; bulk path for small files; optional intra-frame MT for large members.
//! - LZ4: levels 1–2 always `lz4_flex` (fast). With **`lz4-hc`**, levels ≥3 use
//!   liblz4 HC frames (still streaming; decodeable by sevenz-rust2 / 7zz-zstd).

use super::method::CompressMethod;
use crate::error::{Error, Result};
use std::io::{Read, Write};
use std::path::Path;

/// Files at or below this size use one-shot / bulk compress paths.
const SMALL_FILE_ONESHOT: u64 = 1024 * 1024; // 1 MiB

/// Members at or above this size may use Zstd multi-worker (when cores free).
const ZSTD_MT_MIN_SIZE: u64 = 8 * 1024 * 1024; // 8 MiB

/// Create levels at or above this use LZ4HC when the `lz4-hc` feature is enabled.
#[cfg(feature = "lz4-hc")]
const LZ4_HC_MIN_LEVEL: u32 = 3;
/// Compressed pack payload ready for a non-solid 7z member.
#[derive(Debug, Clone)]
pub struct CompressedPack {
    pub data: Vec<u8>,
    pub method_id: Vec<u8>,
    pub method_props: Vec<u8>,
    pub crc32: u32,
    pub uncompressed_size: u64,
    /// CRC of `data` (pack stream); set so writers need not re-hash.
    pub pack_crc: u32,
}

/// Backward-compatible alias used by older call sites / tests.
pub type Lzma2Compressed = CompressedPack;

/// Dict size for LZMA2 levels 0–9 (upper bound; see [`dict_size_for_member`]).
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

/// OPT-07: clamp dict to member size (power-of-two, min 4 KiB, max level dict).
pub fn dict_size_for_member(level: u32, uncompressed_size: u64) -> u32 {
    let full = dict_size_for_level(level);
    if uncompressed_size == 0 {
        return 4096;
    }
    let need = uncompressed_size.saturating_add(1).next_power_of_two();
    let clamped = need.clamp(4096, u64::from(full));
    clamped as u32
}

/// Encode dict_size as the single LZMA2 property byte used by 7-Zip.
pub fn lzma2_dict_prop(dict_size: u32) -> u8 {
    let dict_size = dict_size.clamp(4096, 0xFFFF_FFF0);
    let lead = dict_size.leading_zeros();
    let second_bit = (dict_size >> (30u32.wrapping_sub(lead))).wrapping_sub(2);
    (19u32.wrapping_sub(lead) * 2 + second_bit) as u8
}

/// Active LZMA2 encode backend name (`"liblzma"` or `"lzma-rust2"`).
pub fn lzma2_backend_name() -> &'static str {
    if cfg!(feature = "liblzma") {
        "liblzma"
    } else {
        "lzma-rust2"
    }
}

/// Whether LZ4HC (liblz4) is available for higher create levels.
pub fn lz4_hc_available() -> bool {
    cfg!(feature = "lz4-hc")
}

/// Dict size used for a member at `level` (clamped when size known).
fn lzma2_dict_for(level: u32, uncompressed_size: Option<u64>) -> u32 {
    let level = level.min(9);
    match uncompressed_size {
        Some(sz) => dict_size_for_member(level, sz),
        None => dict_size_for_level(level),
    }
}

/// Map create `--level` 0–9 to Zstd levels (approx 1–19).
pub fn zstd_level(level: u32) -> i32 {
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

/// Map create `--level` 0–9 to LZ4 / LZ4HC level byte stored in 7z method props
/// and passed to liblz4 when `lz4-hc` is enabled (HC effective roughly ≥3).
pub fn lz4_level_byte(level: u32) -> u8 {
    level.min(12).max(1) as u8
}

/// liblz4 compression_level for HC path (3–12).
#[cfg(feature = "lz4-hc")]
fn lz4_hc_compression_level(level: u32) -> u32 {
    // liblz4: 0 = fast default; ≥3 uses HC. Cap at 12 (CLEVEL_MAX).
    let n = level.min(9).max(LZ4_HC_MIN_LEVEL);
    match n {
        3 => 3,
        4 => 5,
        5 => 7,
        6 => 9,
        7 => 10,
        8 => 11,
        _ => 12, // 9+
    }
}
/// Compress bytes with the selected method.
pub fn compress_bytes(input: &[u8], method: CompressMethod, level: u32) -> Result<CompressedPack> {
    compress_reader_sized(&mut &input[..], method, level, Some(input.len() as u64), 0)
}

/// Stream-read and compress a path (size known when metadata available).
pub fn compress_path(path: &Path, method: CompressMethod, level: u32) -> Result<CompressedPack> {
    compress_path_with_size(path, method, level, None, 0)
}

/// Compress path with optional known size and optional zstd intra-frame workers.
pub fn compress_path_with_size(
    path: &Path,
    method: CompressMethod,
    level: u32,
    known_size: Option<u64>,
    zstd_nb_workers: u32,
) -> Result<CompressedPack> {
    let size = known_size.or_else(|| std::fs::metadata(path).ok().map(|m| m.len()));
    // Small-file fast path: one read + bulk compress.
    if let Some(sz) = size {
        if sz > 0 && sz <= SMALL_FILE_ONESHOT {
            let data = match std::fs::read(path) {
                Ok(d) => d,
                Err(e) if crate::util::is_skippable_fs_io(&e) => {
                    return Err(Error::Vanished(path.to_path_buf()));
                }
                Err(e) => {
                    return Err(Error::Archive(format!(
                        "read {} for compress: {e}",
                        path.display()
                    )));
                }
            };
            return compress_bytes_inner(&data, method, level, zstd_nb_workers);
        }
    }
    let mut f = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if crate::util::is_skippable_fs_io(&e) => {
            return Err(Error::Vanished(path.to_path_buf()));
        }
        Err(e) => {
            return Err(Error::Archive(format!(
                "open {} for compress: {e}",
                path.display()
            )));
        }
    };
    compress_reader_sized(&mut f, method, level, size, zstd_nb_workers)
}

/// Stream-read `reader` into an in-memory compressed pack.
pub fn compress_reader(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
) -> Result<CompressedPack> {
    compress_reader_sized(reader, method, level, None, 0)
}

fn compress_bytes_inner(
    input: &[u8],
    method: CompressMethod,
    level: u32,
    zstd_nb_workers: u32,
) -> Result<CompressedPack> {
    let mut out = Vec::with_capacity(input.len() / 2 + 64);
    let (props, content_crc, unpack_size) = match method {
        CompressMethod::Lzma2 => {
            compress_lzma2(&mut &input[..], level, Some(input.len() as u64), &mut out)?
        }
        CompressMethod::Zstd => compress_zstd(
            &mut &input[..],
            level,
            Some(input.len() as u64),
            zstd_nb_workers,
            &mut out,
        )?,
        CompressMethod::Lz4 => compress_lz4(&mut &input[..], level, &mut out)?,
    };
    let pack_crc = crc32fast::hash(&out);
    Ok(CompressedPack {
        data: out,
        method_id: method.method_id().to_vec(),
        method_props: props,
        crc32: content_crc,
        uncompressed_size: unpack_size,
        pack_crc,
    })
}

fn compress_reader_sized(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    known_size: Option<u64>,
    zstd_nb_workers: u32,
) -> Result<CompressedPack> {
    let cap = known_size
        .map(|s| (s / 2).saturating_add(64) as usize)
        .unwrap_or(64 * 1024);
    let mut out = Vec::with_capacity(cap);
    let (props, content_crc, unpack_size) =
        compress_reader_to_writer_sized(reader, method, level, known_size, zstd_nb_workers, &mut out)?;
    let pack_crc = crc32fast::hash(&out);
    Ok(CompressedPack {
        data: out,
        method_id: method.method_id().to_vec(),
        method_props: props,
        crc32: content_crc,
        uncompressed_size: unpack_size,
        pack_crc,
    })
}

/// Stream compress into `pack_out`. Returns `(method_props, content_crc, unpack_size)`.
pub fn compress_reader_to_writer(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    compress_reader_to_writer_sized(reader, method, level, None, 0, pack_out)
}

fn compress_reader_to_writer_sized(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    known_size: Option<u64>,
    zstd_nb_workers: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    match method {
        CompressMethod::Lzma2 => compress_lzma2(reader, level, known_size, pack_out),
        CompressMethod::Zstd => {
            compress_zstd(reader, level, known_size, zstd_nb_workers, pack_out)
        }
        CompressMethod::Lz4 => compress_lz4(reader, level, pack_out),
    }
}

/// Stream compress into a pack sink with pack CRC.
///
/// Returns `(props, content_crc, unpack_size, pack_crc, pack_size)`.
pub fn compress_reader_append_pack(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64, u32, u64)> {
    compress_reader_append_pack_sized(reader, method, level, None, 0, pack_out)
}

/// Like [`compress_reader_append_pack`] with known size and zstd workers.
pub fn compress_reader_append_pack_sized(
    reader: &mut dyn Read,
    method: CompressMethod,
    level: u32,
    known_size: Option<u64>,
    zstd_nb_workers: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64, u32, u64)> {
    let mut pack = PackCrcWriter::new(pack_out);
    let (props, content_crc, unpack_size) = compress_reader_to_writer_sized(
        reader,
        method,
        level,
        known_size,
        zstd_nb_workers,
        &mut pack,
    )?;
    let (pack_crc, pack_size) = pack.into_crc_and_size();
    Ok((props, content_crc, unpack_size, pack_crc, pack_size))
}

fn compress_lzma2(
    reader: &mut dyn Read,
    level: u32,
    known_size: Option<u64>,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    let level = level.min(9);
    let dict = lzma2_dict_for(level, known_size);
    let props = vec![lzma2_dict_prop(dict)];
    let mut content_hasher = crc32fast::Hasher::new();
    let mut unpack_size = 0u64;

    #[cfg(feature = "liblzma")]
    {
        compress_lzma2_liblzma(
            reader,
            level,
            dict,
            pack_out,
            &mut content_hasher,
            &mut unpack_size,
        )?;
        return Ok((props, content_hasher.finalize(), unpack_size));
    }

    #[cfg(not(feature = "liblzma"))]
    {
        compress_lzma2_rust(
            reader,
            level,
            dict,
            pack_out,
            &mut content_hasher,
            &mut unpack_size,
        )?;
        Ok((props, content_hasher.finalize(), unpack_size))
    }
}

#[cfg(not(feature = "liblzma"))]
fn compress_lzma2_rust(
    reader: &mut dyn Read,
    level: u32,
    dict: u32,
    pack_out: &mut dyn Write,
    content_hasher: &mut crc32fast::Hasher,
    unpack_size: &mut u64,
) -> Result<()> {
    use lzma_rust2::{Lzma2Options, Lzma2Writer};
    let mut opt = Lzma2Options::with_preset(level);
    opt.chunk_size = None;
    opt.lzma_options.dict_size = dict;
    let mut enc = Lzma2Writer::new(CountingWrite::new(pack_out), opt);
    copy_hashed(reader, &mut enc, content_hasher, unpack_size)?;
    enc.finish()
        .map_err(|e| Error::Compress(format!("LZMA2 finish: {e}")))?;
    Ok(())
}

/// Raw LZMA2 via liblzma (`lzma_raw_encoder` + LZMA2 filter) — same codestream
/// family as 7-Zip method `0x21`. Streaming; does not buffer whole multi-GB inputs.
#[cfg(feature = "liblzma")]
fn compress_lzma2_liblzma(
    reader: &mut dyn Read,
    level: u32,
    dict: u32,
    pack_out: &mut dyn Write,
    content_hasher: &mut crc32fast::Hasher,
    unpack_size: &mut u64,
) -> Result<()> {
    use liblzma::stream::{Filters, LzmaOptions, Stream};
    use liblzma::write::XzEncoder;

    let mut opts = LzmaOptions::new_preset(level)
        .map_err(|e| Error::Compress(format!("liblzma preset: {e:?}")))?;
    opts.dict_size(dict);
    // Filters must outlive the Stream (options pointer stored in filter chain).
    let mut filters = Filters::new();
    filters.lzma2(&opts);
    let stream = Stream::new_raw_encoder(&filters)
        .map_err(|e| Error::Compress(format!("liblzma raw LZMA2 encoder: {e:?}")))?;
    let mut enc = XzEncoder::new_stream(CountingWrite::new(pack_out), stream);
    copy_hashed(reader, &mut enc, content_hasher, unpack_size)?;
    enc.finish()
        .map_err(|e| Error::Compress(format!("liblzma LZMA2 finish: {e}")))?;
    Ok(())
}
fn compress_zstd(
    reader: &mut dyn Read,
    level: u32,
    known_size: Option<u64>,
    zstd_nb_workers: u32,
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
        // 7z already stores content CRC — skip redundant frame XXH64.
        enc.include_checksum(false)
            .map_err(|e| Error::Compress(format!("zstd checksum: {e}")))?;
        if let Some(sz) = known_size {
            enc.set_pledged_src_size(Some(sz))
                .map_err(|e| Error::Compress(format!("zstd pledged size: {e}")))?;
        }
        // OPT-11: intra-frame MT only for large members when requested (`zstdmt` feature).
        if zstd_nb_workers > 1 {
            if let Some(sz) = known_size {
                if sz >= ZSTD_MT_MIN_SIZE {
                    enc.multithread(zstd_nb_workers)
                        .map_err(|e| Error::Compress(format!("zstd multithread: {e}")))?;
                }
            }
        }
        copy_hashed(reader, &mut enc, &mut content_hasher, &mut unpack_size)?;
        enc.finish()
            .map_err(|e| Error::Compress(format!("zstd finish: {e}")))?;
    }
    Ok((props, content_hasher.finalize(), unpack_size))
}

fn compress_lz4(
    reader: &mut dyn Read,
    level: u32,
    pack_out: &mut dyn Write,
) -> Result<(Vec<u8>, u32, u64)> {
    // Props: major=1, minor=0, level byte (1–12 for peer parity with 7zz-zstd).
    let props = vec![1u8, 0u8, lz4_level_byte(level)];
    let mut content_hasher = crc32fast::Hasher::new();
    let mut unpack_size = 0u64;

    #[cfg(feature = "lz4-hc")]
    if level >= LZ4_HC_MIN_LEVEL {
        compress_lz4_hc(
            reader,
            level,
            pack_out,
            &mut content_hasher,
            &mut unpack_size,
        )?;
        return Ok((props, content_hasher.finalize(), unpack_size));
    }

    // Fast path: pure-Rust lz4_flex frames (levels 1–2, or all levels without lz4-hc).
    let mut enc = lz4_flex::frame::FrameEncoder::new(CountingWrite::new(pack_out));
    copy_hashed(reader, &mut enc, &mut content_hasher, &mut unpack_size)?;
    enc.finish()
        .map_err(|e| Error::Compress(format!("lz4 finish: {e}")))?;
    Ok((props, content_hasher.finalize(), unpack_size))
}

/// Stream LZ4 **frame** encode via liblz4 (HC when level maps ≥3).
#[cfg(feature = "lz4-hc")]
fn compress_lz4_hc(
    reader: &mut dyn Read,
    level: u32,
    pack_out: &mut dyn Write,
    content_hasher: &mut crc32fast::Hasher,
    unpack_size: &mut u64,
) -> Result<()> {
    let hc = lz4_hc_compression_level(level);
    let mut enc = lz4::EncoderBuilder::new()
        .level(hc)
        .block_size(lz4::BlockSize::Max4MB)
        .block_mode(lz4::BlockMode::Linked)
        // 7z already stores content CRC — skip redundant frame checksum.
        .checksum(lz4::ContentChecksum::NoChecksum)
        .build(CountingWrite::new(pack_out))
        .map_err(|e| Error::Compress(format!("lz4-hc encoder: {e}")))?;
    copy_hashed(reader, &mut enc, content_hasher, unpack_size)?;
    let (_w, res) = enc.finish();
    res.map_err(|e| Error::Compress(format!("lz4-hc finish: {e}")))?;
    Ok(())
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
pub struct PackCrcWriter<'a, W: Write + ?Sized> {
    inner: &'a mut W,
    hasher: crc32fast::Hasher,
    size: u64,
}

impl<'a, W: Write + ?Sized> PackCrcWriter<'a, W> {
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

impl<W: Write + ?Sized> Write for PackCrcWriter<'_, W> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use lzma_rust2::Lzma2Reader;
    use std::io::Cursor;

    fn decode_lzma2(pack: &[u8], dict: u32) -> Vec<u8> {
        let mut decoded = Vec::new();
        Lzma2Reader::new(Cursor::new(pack), dict, None)
            .read_to_end(&mut decoded)
            .unwrap();
        decoded
    }

    fn decode_lz4_frame(pack: &[u8]) -> Vec<u8> {
        let mut decoded = Vec::new();
        lz4_flex::frame::FrameDecoder::new(pack)
            .read_to_end(&mut decoded)
            .unwrap();
        decoded
    }

    #[test]
    fn lzma2_roundtrip() {
        let msg = b"hello multi codec ".repeat(100);
        let out = compress_bytes(&msg, CompressMethod::Lzma2, 1).unwrap();
        assert_eq!(out.method_id, vec![0x21]);
        assert_eq!(out.pack_crc, crc32fast::hash(&out.data));
        let dict = dict_size_for_member(1, msg.len() as u64);
        assert_eq!(out.method_props, vec![lzma2_dict_prop(dict)]);
        assert_eq!(decode_lzma2(&out.data, dict), msg);
    }

    #[test]
    fn lzma2_roundtrip_multiple_levels() {
        let msg = b"lzma2 level sweep payload #".repeat(400);
        for level in [0u32, 1, 3, 5, 7, 9] {
            let out = compress_bytes(&msg, CompressMethod::Lzma2, level).unwrap();
            assert_eq!(out.method_id, vec![0x21], "level {level}");
            assert_eq!(out.crc32, crc32fast::hash(&msg), "level {level}");
            assert_eq!(out.uncompressed_size, msg.len() as u64);
            let dict = dict_size_for_member(level, msg.len() as u64);
            assert_eq!(
                out.method_props,
                vec![lzma2_dict_prop(dict)],
                "level {level} dict prop"
            );
            assert_eq!(decode_lzma2(&out.data, dict), msg, "level {level}");
        }
    }

    #[test]
    fn lzma2_stream_reader_path() {
        let msg = b"streamed lzma2 member data!!".repeat(300);
        let mut cursor = Cursor::new(&msg[..]);
        let out = compress_reader_sized(
            &mut cursor,
            CompressMethod::Lzma2,
            5,
            Some(msg.len() as u64),
            0,
        )
        .unwrap();
        let dict = dict_size_for_member(5, msg.len() as u64);
        assert_eq!(decode_lzma2(&out.data, dict), msg);
    }

    #[test]
    fn dict_clamp_tiny_member() {
        let d = dict_size_for_member(5, 8000);
        assert!(d <= 16 * 1024, "dict={d}");
        assert!(d >= 4096);
        assert!(d < dict_size_for_level(5));
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
        assert_eq!(out.method_props, vec![1, 0, 1]);
        assert_eq!(decode_lz4_frame(&out.data), msg);
    }

    #[test]
    fn lz4_roundtrip_multiple_levels() {
        let msg = b"lz4 level sweep data @@".repeat(600);
        for level in [1u32, 2, 3, 5, 7, 9] {
            let out = compress_bytes(&msg, CompressMethod::Lz4, level).unwrap();
            assert_eq!(out.method_id, CompressMethod::Lz4.method_id(), "level {level}");
            assert_eq!(
                out.method_props,
                vec![1, 0, lz4_level_byte(level)],
                "level {level} props"
            );
            assert_eq!(out.crc32, crc32fast::hash(&msg), "level {level}");
            assert_eq!(decode_lz4_frame(&out.data), msg, "level {level}");
        }
    }

    #[test]
    fn lz4_stream_reader_high_level() {
        let msg = b"streamed lz4 high level payload!".repeat(400);
        let mut cursor = Cursor::new(&msg[..]);
        let out =
            compress_reader_sized(&mut cursor, CompressMethod::Lz4, 5, Some(msg.len() as u64), 0)
                .unwrap();
        assert_eq!(decode_lz4_frame(&out.data), msg);
    }

    #[test]
    fn dict_prop_stable() {
        assert_eq!(lzma2_dict_prop(1 << 22), lzma2_dict_prop(1 << 22));
    }

    #[test]
    fn backend_names_are_stable() {
        let name = lzma2_backend_name();
        assert!(name == "liblzma" || name == "lzma-rust2");
        // Availability matches feature flags (compile-time).
        assert_eq!(lz4_hc_available(), cfg!(feature = "lz4-hc"));
        assert_eq!(name == "liblzma", cfg!(feature = "liblzma"));
    }

    #[cfg(feature = "lz4-hc")]
    #[test]
    fn lz4_hc_path_roundtrips_and_uses_level_props() {
        // Feature path: levels ≥3 must encode via liblz4 HC frames and still
        // decode with lz4_flex FrameDecoder (and sevenz-rust2).
        let msg = b"ABCD".repeat(50_000);
        let out = compress_bytes(&msg, CompressMethod::Lz4, 5).unwrap();
        assert_eq!(out.method_props, vec![1, 0, 5]);
        assert_eq!(decode_lz4_frame(&out.data), msg);
        // High-level HC should produce a valid non-empty pack smaller than raw.
        assert!(out.data.len() < msg.len() / 10, "pack={}", out.data.len());
        assert!(lz4_hc_available());
    }

    #[cfg(feature = "liblzma")]
    #[test]
    fn liblzma_backend_is_active() {
        assert_eq!(lzma2_backend_name(), "liblzma");
        let msg = b"liblzma active path ".repeat(200);
        let out = compress_bytes(&msg, CompressMethod::Lzma2, 5).unwrap();
        let dict = dict_size_for_member(5, msg.len() as u64);
        assert_eq!(decode_lzma2(&out.data, dict), msg);
    }
}
