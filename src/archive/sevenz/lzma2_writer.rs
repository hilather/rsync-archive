//! Non-solid 7z create writer: one compressed pack stream per file (any method).

use super::codec::{compress_reader_append_pack, CompressedPack};
use super::header::{write_raw_header, write_start_header, HeaderFile, SIG_HEADER_SIZE};
use super::method::CompressMethod;
use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::SystemTime;

/// Non-solid multi-file 7z writer (LZMA2 / Zstd / LZ4 per member).
pub struct NonsolidLzma2Writer {
    file: File,
    files: Vec<HeaderFile>,
    level: u32,
    method: CompressMethod,
}

impl NonsolidLzma2Writer {
    /// Create with LZMA2 (legacy helper).
    pub fn create(path: &Path, level: u32) -> Result<Self> {
        Self::create_with_method(path, level, CompressMethod::Lzma2)
    }

    /// Create output path with chosen compression method.
    pub fn create_with_method(path: &Path, level: u32, method: CompressMethod) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let mut file = File::create(path)?;
        file.write_all(&[0u8; SIG_HEADER_SIZE as usize])?;
        Ok(Self {
            file,
            files: Vec::new(),
            level: level.min(9),
            method,
        })
    }

    /// Append a source file: stream-read, compress, stream-write pack.
    pub fn push_path(&mut self, name: String, src: &Path) -> Result<()> {
        let meta = std::fs::symlink_metadata(src).map_err(|e| {
            Error::Archive(format!("stat {} for create: {e}", src.display()))
        })?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(Error::NotRegularFile(src.to_path_buf()));
        }
        let mtime = meta.modified().ok().and_then(system_time_to_filetime);

        if meta.len() == 0 {
            let mut hf = HeaderFile::empty_file(name);
            hf.mtime = mtime;
            self.files.push(hf);
            return Ok(());
        }

        let mut input = open_nofollow_read(src)?;
        let (props, content_crc, unpack_size, pack_crc, pack_size) =
            compress_reader_append_pack(&mut input, self.method, self.level, &mut self.file)?;

        if unpack_size == 0 && pack_size == 0 {
            let mut hf = HeaderFile::empty_file(name);
            hf.mtime = mtime;
            self.files.push(hf);
            return Ok(());
        }

        self.files.push(HeaderFile {
            name,
            pack_size,
            pack_crc,
            unpack_size,
            content_crc,
            method_id: self.method.method_id().to_vec(),
            method_props: props,
            empty: false,
            mtime,
        });
        Ok(())
    }

    /// Append precompressed pack (tests / parallel encode path).
    pub fn push_packed(&mut self, name: String, compressed: CompressedPack) -> Result<()> {
        self.push_packed_with_mtime(name, compressed, None)
    }

    /// Append precompressed pack with optional mtime.
    pub fn push_packed_with_mtime(
        &mut self,
        name: String,
        compressed: CompressedPack,
        mtime: Option<u64>,
    ) -> Result<()> {
        if compressed.uncompressed_size == 0 && compressed.data.is_empty() {
            let mut hf = HeaderFile::empty_file(name);
            hf.mtime = mtime;
            self.files.push(hf);
            return Ok(());
        }
        let pack_crc = crc32fast::hash(&compressed.data);
        self.file.write_all(&compressed.data)?;
        self.files.push(HeaderFile {
            name,
            pack_size: compressed.data.len() as u64,
            pack_crc,
            unpack_size: compressed.uncompressed_size,
            content_crc: compressed.crc32,
            method_id: compressed.method_id,
            method_props: compressed.method_props,
            empty: false,
            mtime,
        });
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Write end header and fix start signature.
    pub fn finish(mut self) -> Result<()> {
        if self.files.is_empty() {
            return Err(Error::EmptyArchive);
        }

        let mut header = Vec::with_capacity(64 * 1024 + self.files.len() * 64);
        write_raw_header(&mut header, &self.files)?;

        let header_pos = self.file.stream_position()?;
        self.file.write_all(&header)?;
        let header_crc = crc32fast::hash(&header);

        let next_header_offset = header_pos - SIG_HEADER_SIZE;
        let next_header_size = header.len() as u64;
        let sig = write_start_header(next_header_offset, next_header_size, header_crc);

        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&sig)?;
        self.file.flush()?;
        Ok(())
    }
}

fn system_time_to_filetime(t: SystemTime) -> Option<u64> {
    let secs = t.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
    Some(super::header::filetime_from_unix_secs(secs))
}

fn open_nofollow_read(path: &Path) -> Result<File> {
    let meta = std::fs::symlink_metadata(path).map_err(|e| {
        Error::Archive(format!("stat {}: {e}", path.display()))
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(Error::NotRegularFile(path.to_path_buf()));
    }
    File::open(path).map_err(|e| Error::Archive(format!("open {}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sevenz_rust2::{ArchiveReader, Password};
    use std::fs;

    fn extract(archive: &Path, name: &str) -> Vec<u8> {
        let mut reader = ArchiveReader::open(archive, Password::empty()).unwrap();
        reader.read_file(name).unwrap()
    }

    #[test]
    fn lzma2_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        fs::write(&a, b"alpha lzma2").unwrap();
        let out = dir.path().join("out.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        w.push_path("a.txt".into(), &a).unwrap();
        w.finish().unwrap();
        assert_eq!(extract(&out, "a.txt"), b"alpha lzma2");
    }

    #[test]
    fn zstd_roundtrip_via_writer() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        fs::write(&a, b"alpha zstd data content").unwrap();
        let out = dir.path().join("z.7z");
        let mut w =
            NonsolidLzma2Writer::create_with_method(&out, 3, CompressMethod::Zstd).unwrap();
        w.push_path("a.txt".into(), &a).unwrap();
        w.finish().unwrap();
        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        assert!(!reader.archive().is_solid);
        drop(reader);
        assert_eq!(extract(&out, "a.txt"), b"alpha zstd data content");
    }

    #[test]
    fn lz4_roundtrip_via_writer() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        fs::write(&a, b"alpha lz4 data").unwrap();
        let out = dir.path().join("l.7z");
        let mut w = NonsolidLzma2Writer::create_with_method(&out, 1, CompressMethod::Lz4).unwrap();
        w.push_path("a.txt".into(), &a).unwrap();
        w.finish().unwrap();
        assert_eq!(extract(&out, "a.txt"), b"alpha lz4 data");
    }

    #[test]
    fn empty_finish_errors() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("empty.7z");
        let w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        assert!(matches!(w.finish().unwrap_err(), Error::EmptyArchive));
    }
}
