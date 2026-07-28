//! Non-solid 7z writer that **stores** (Copy method) whole files as pack streams.
//!
//! Used for the outer/embed archive: finished member blobs are appended without
//! recompression. Headers match sevenz-rust2 layout for 7zz / sevenz-rust2 /
//! mounter compatibility.

use super::header::{write_raw_header, write_start_header, HeaderFile, SIG_HEADER_SIZE};
use crate::error::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::SystemTime;

/// Streaming non-solid 7z writer using the **Copy** (store) method per file.
pub struct NonsolidStoreWriter {
    file: File,
    files: Vec<HeaderFile>,
}

impl NonsolidStoreWriter {
    /// Create output path and write a placeholder start header (32 zero bytes).
    ///
    /// Parent directories are created if needed. An existing file at `path` is
    /// replaced. Callers that want atomic rename semantics should pass a
    /// `*.partial` path and rename after [`finish`](Self::finish).
    pub fn create(path: &Path) -> Result<Self> {
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
        })
    }

    /// Append raw file bytes as a stored pack stream (no recompression).
    ///
    /// Reads in 256 KiB chunks, streaming CRC and pack write so peak RAM stays
    /// O(buffer), not O(file size). Empty sources become empty-flag members
    /// (no pack stream).
    pub fn push_path(&mut self, name: String, src: &Path) -> Result<()> {
        let meta = std::fs::metadata(src).map_err(|e| {
            Error::Archive(format!("stat {} for store append: {e}", src.display()))
        })?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(system_time_to_filetime);

        if meta.len() == 0 {
            // Empty file: no pack stream; marked empty for FilesInfo.
            let mut hf = HeaderFile::empty_file(name);
            hf.mtime = mtime;
            self.files.push(hf);
            return Ok(());
        }

        let mut input = File::open(src).map_err(|e| {
            Error::Archive(format!("open {} for store append: {e}", src.display()))
        })?;
        let mut hasher = crc32fast::Hasher::new();
        let mut buf = [0u8; 256 * 1024];
        let mut size = 0u64;
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            self.file.write_all(&buf[..n])?;
            size += n as u64;
        }
        let crc = hasher.finalize();
        let mut hf = HeaderFile::stored(name, size, crc);
        hf.mtime = mtime;
        self.files.push(hf);
        Ok(())
    }

    /// Append an in-memory buffer as a stored member.
    pub fn push_bytes(&mut self, name: String, data: &[u8]) -> Result<()> {
        if data.is_empty() {
            self.files.push(HeaderFile::empty_file(name));
            return Ok(());
        }
        let crc = crc32fast::hash(data);
        self.file.write_all(data)?;
        self.files.push(HeaderFile::stored(name, data.len() as u64, crc));
        Ok(())
    }

    /// Number of members queued (including empty).
    pub fn len(&self) -> usize {
        self.files.len()
    }

    /// Whether no members have been pushed yet.
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Write end header and fix start signature. Consumes the writer.
    ///
    /// Returns [`Error::EmptyArchive`] if no members were pushed.
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

#[cfg(test)]
mod tests {
    use super::*;
    use sevenz_rust2::{ArchiveReader, Password};
    use std::fs;

    fn list_file_names(archive: &Path) -> Vec<String> {
        let reader = ArchiveReader::open(archive, Password::empty()).expect("open archive");
        reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect()
    }

    fn test_archive(archive: &Path) {
        let mut reader = ArchiveReader::open(archive, Password::empty()).expect("open");
        reader
            .for_each_entries(|_e, r| {
                let mut sink = std::io::sink();
                std::io::copy(r, &mut sink)?;
                Ok(true)
            })
            .expect("test/decode all entries");
    }

    fn extract_member(archive: &Path, name: &str) -> Vec<u8> {
        let mut reader = ArchiveReader::open(archive, Password::empty()).expect("open");
        reader.read_file(name).expect("read member")
    }

    fn is_solid(archive: &Path) -> bool {
        let reader = ArchiveReader::open(archive, Password::empty()).expect("open");
        reader.archive().is_solid
    }

    #[test]
    fn store_writer_roundtrip_list_extract() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.bin");
        let b = dir.path().join("b.txt");
        fs::write(&a, b"nested-like-payload-aaaa").unwrap();
        fs::write(&b, b"passthrough hello").unwrap();

        let out = dir.path().join("outer.7z");
        let mut w = NonsolidStoreWriter::create(&out).unwrap();
        w.push_path("nested/a.7z".into(), &a).unwrap();
        w.push_path("readme.txt".into(), &b).unwrap();
        w.finish().unwrap();

        test_archive(&out);
        assert!(!is_solid(&out), "store writer must produce non-solid archive");

        let names = list_file_names(&out);
        assert!(
            names.iter().any(|n| n == "nested/a.7z" || n.ends_with("a.7z")),
            "{names:?}"
        );
        assert!(
            names.iter().any(|n| n == "readme.txt"),
            "{names:?}"
        );

        let got_a = extract_member(&out, "nested/a.7z");
        assert_eq!(got_a, b"nested-like-payload-aaaa");
        let got_b = extract_member(&out, "readme.txt");
        assert_eq!(got_b, b"passthrough hello");
    }

    #[test]
    fn store_writer_empty_file_and_nested_paths() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("e.7z");
        let mut w = NonsolidStoreWriter::create(&out).unwrap();
        w.push_bytes("empty.dat".into(), b"").unwrap();
        w.push_bytes("sub/x.txt".into(), b"hi").unwrap();
        w.push_bytes("nested/deep/y.bin".into(), b"deep-bytes").unwrap();
        w.finish().unwrap();

        test_archive(&out);
        assert!(!is_solid(&out));

        let names = list_file_names(&out);
        assert!(
            names.iter().any(|n| n.ends_with("empty.dat")),
            "{names:?}"
        );
        assert!(names.iter().any(|n| n.contains("x.txt")), "{names:?}");
        assert!(
            names.iter().any(|n| n == "nested/deep/y.bin" || n.ends_with("y.bin")),
            "{names:?}"
        );

        // Empty file extracts to empty (or is listed with size 0).
        let empty = extract_member(&out, "empty.dat");
        assert!(empty.is_empty(), "empty member should extract empty");
        assert_eq!(extract_member(&out, "sub/x.txt"), b"hi");
        assert_eq!(extract_member(&out, "nested/deep/y.bin"), b"deep-bytes");
    }

    #[test]
    fn empty_finish_errors() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("empty.7z");
        let w = NonsolidStoreWriter::create(&out).unwrap();
        let err = w.finish().unwrap_err();
        assert!(
            matches!(err, Error::EmptyArchive),
            "expected EmptyArchive, got {err:?}"
        );
    }

    #[test]
    fn push_path_empty_source_is_empty_member() {
        let dir = tempfile::tempdir().unwrap();
        let empty_src = dir.path().join("zero");
        fs::write(&empty_src, b"").unwrap();
        let out = dir.path().join("z.7z");
        let mut w = NonsolidStoreWriter::create(&out).unwrap();
        w.push_path("zero.bin".into(), &empty_src).unwrap();
        w.push_bytes("one.txt".into(), b"x").unwrap();
        w.finish().unwrap();
        test_archive(&out);
        assert_eq!(extract_member(&out, "zero.bin"), b"");
        assert_eq!(extract_member(&out, "one.txt"), b"x");
    }

    #[test]
    fn multi_file_bytes_roundtrip_crc_match() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("multi.7z");
        let payloads: &[(&str, &[u8])] = &[
            ("a.txt", b"alpha"),
            ("b/c.dat", b"beta-gamma-delta"),
            ("d.bin", &[0u8, 1, 2, 255, 128]),
        ];
        let mut w = NonsolidStoreWriter::create(&out).unwrap();
        for (name, data) in payloads {
            w.push_bytes((*name).into(), data).unwrap();
        }
        assert_eq!(w.len(), 3);
        assert!(!w.is_empty());
        w.finish().unwrap();

        test_archive(&out);
        assert!(!is_solid(&out));
        for (name, data) in payloads {
            assert_eq!(&extract_member(&out, name)[..], *data);
        }
    }
}
