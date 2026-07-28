//! Non-solid 7z writer that LZMA2-compresses each file as its own pack stream.

use super::codec::{compress_reader_append_pack, Lzma2Compressed};
use super::header::{write_raw_header, write_start_header, HeaderFile, SIG_HEADER_SIZE};
use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::time::SystemTime;

/// Streaming non-solid 7z writer: one LZMA2 pack stream per non-empty file.
pub struct NonsolidLzma2Writer {
    file: File,
    files: Vec<HeaderFile>,
    level: u32,
}

impl NonsolidLzma2Writer {
    /// Create output path and write a placeholder start header (32 zero bytes).
    pub fn create(path: &Path, level: u32) -> Result<Self> {
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
        })
    }

    /// Append a source file: stream-read, LZMA2-encode, stream-write pack.
    ///
    /// Empty files become empty-flag members (no pack). mtime from source metadata.
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
            compress_reader_append_pack(&mut input, self.level, &mut self.file)?;

        if unpack_size == 0 {
            // Race: became empty; no pack was written if encoder wrote nothing —
            // but pack may have end markers. Prefer error if pack_size > 0 with 0 unpack.
            if pack_size == 0 {
                let mut hf = HeaderFile::empty_file(name);
                hf.mtime = mtime;
                self.files.push(hf);
                return Ok(());
            }
        }

        let mut hf = HeaderFile {
            name,
            pack_size,
            pack_crc,
            unpack_size,
            content_crc,
            method_id: vec![0x21],
            method_props: vec![props],
            empty: false,
            mtime,
        };
        // Ensure empty flag consistency
        if unpack_size == 0 && pack_size == 0 {
            hf.empty = true;
            hf.method_id = vec![0x00];
            hf.method_props.clear();
        }
        self.files.push(hf);
        Ok(())
    }

    /// Append precompressed pack (tests / batch).
    pub fn push_packed(&mut self, name: String, compressed: Lzma2Compressed) -> Result<()> {
        if compressed.uncompressed_size == 0 && compressed.data.is_empty() {
            self.files.push(HeaderFile::empty_file(name));
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
            method_id: vec![0x21],
            method_props: vec![compressed.props],
            empty: false,
            mtime: None,
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
    fn lzma2_writer_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"alpha data content for create").unwrap();
        fs::write(&b, b"beta data content for create!!").unwrap();

        let out = dir.path().join("out.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        w.push_path("a.txt".into(), &a).unwrap();
        w.push_path("nested/b.txt".into(), &b).unwrap();
        w.finish().unwrap();

        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        assert!(!reader.archive().is_solid);
        drop(reader);

        assert_eq!(extract(&out, "a.txt"), b"alpha data content for create");
        assert_eq!(
            extract(&out, "nested/b.txt"),
            b"beta data content for create!!"
        );
    }

    #[test]
    fn empty_file_member() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("e.dat");
        fs::write(&empty, b"").unwrap();
        let data = dir.path().join("d.txt");
        fs::write(&data, b"x").unwrap();
        let out = dir.path().join("e.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        w.push_path("e.dat".into(), &empty).unwrap();
        w.push_path("d.txt".into(), &data).unwrap();
        w.finish().unwrap();
        assert_eq!(extract(&out, "e.dat"), b"");
        assert_eq!(extract(&out, "d.txt"), b"x");
    }

    #[test]
    fn empty_finish_errors() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("empty.7z");
        let w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        assert!(matches!(w.finish().unwrap_err(), Error::EmptyArchive));
    }
}
