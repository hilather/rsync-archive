//! Non-solid 7z create writer: one compressed pack stream per file (any method).

use super::codec::{
    compress_reader_append_pack_sized, CompressedPack,
};
use super::header::{write_raw_header, write_start_header, HeaderFile, SIG_HEADER_SIZE};
use super::method::CompressMethod;
use crate::error::{Error, Result};
use crate::select::SelectedEntry;
use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

/// Non-solid multi-file 7z writer (LZMA2 / Zstd / LZ4 per member).
pub struct NonsolidLzma2Writer {
    file: BufWriter<File>,
    files: Vec<HeaderFile>,
    level: u32,
    method: CompressMethod,
    /// Optional zstd intra-frame workers for large members (0/1 = off).
    zstd_nb_workers: u32,
}

impl NonsolidLzma2Writer {
    /// Create with LZMA2 (legacy helper).
    pub fn create(path: &Path, level: u32) -> Result<Self> {
        Self::create_with_method(path, level, CompressMethod::Lzma2)
    }

    /// Create output path with chosen compression method.
    pub fn create_with_method(path: &Path, level: u32, method: CompressMethod) -> Result<Self> {
        Self::create_with_method_workers(path, level, method, 0)
    }

    /// Create with optional zstd multi-worker for large members.
    pub fn create_with_method_workers(
        path: &Path,
        level: u32,
        method: CompressMethod,
        zstd_nb_workers: u32,
    ) -> Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        let file = File::create(path)?;
        let mut file = BufWriter::with_capacity(1024 * 1024, file);
        file.write_all(&[0u8; SIG_HEADER_SIZE as usize])?;
        Ok(Self {
            file,
            files: Vec::new(),
            level: level.min(9),
            method,
            zstd_nb_workers,
        })
    }

    /// Append using selection metadata (no re-stat). Streams compress → pack.
    ///
    /// Returns `Ok(true)` if a member was written, `Ok(false)` if the source
    /// vanished/became inaccessible and was soft-skipped (no orphan pack bytes).
    ///
    /// Zero-byte selected files still open the path so a vanish race soft-skips
    /// (no phantom empty member).
    pub fn push_entry(&mut self, entry: &SelectedEntry) -> Result<bool> {
        let mtime = entry
            .mtime_unix
            .map(super::header::filetime_from_unix_secs);

        // EINTR is retried inside open_file_for_encode (not soft-skipped).
        let mut input = match crate::util::open_file_for_encode(&entry.abs_path) {
            Ok(f) => f,
            Err(Error::Vanished(_)) => {
                crate::util::soft_skip_note(
                    crate::util::SoftKind::Vanished,
                    &entry.archive_name,
                );
                return Ok(false);
            }
            Err(e) => return Err(e),
        };

        if entry.size == 0 {
            let mut hf = HeaderFile::empty_file(entry.archive_name.clone());
            hf.mtime = mtime;
            self.files.push(hf);
            return Ok(true);
        }

        let zstd_w = if self.method == CompressMethod::Zstd {
            self.zstd_nb_workers
        } else {
            0
        };

        self.push_opened_reader(
            &entry.archive_name,
            entry.size,
            &mut input,
            &entry.abs_path,
            mtime,
            zstd_w,
        )
    }

    /// Compress from an already-opened reader (pack path). Soft-skips on
    /// [`Error::Vanished`] with pack rollback so neighbors stay consistent.
    ///
    /// Used by [`push_entry`] and by unit tests injecting mid-read failures.
    pub(crate) fn push_opened_reader(
        &mut self,
        archive_name: &str,
        size: u64,
        input: &mut dyn std::io::Read,
        src_path: &Path,
        mtime: Option<u64>,
        zstd_nb_workers: u32,
    ) -> Result<bool> {
        // Snapshot pack offset so a mid-compress failure can roll back orphan bytes.
        self.file.flush().map_err(Error::Io)?;
        let pack_start = self.file.get_mut().stream_position().map_err(Error::Io)?;

        let compress_result = compress_reader_append_pack_sized(
            input,
            self.method,
            self.level,
            Some(size),
            zstd_nb_workers,
            &mut self.file,
            Some(src_path),
        );

        match compress_result {
            Ok((props, content_crc, unpack_size, pack_crc, pack_size)) => {
                if unpack_size == 0 && pack_size == 0 {
                    let mut hf = HeaderFile::empty_file(archive_name.to_string());
                    hf.mtime = mtime;
                    self.files.push(hf);
                    return Ok(true);
                }

                self.files.push(HeaderFile {
                    name: archive_name.to_string(),
                    pack_size,
                    pack_crc,
                    unpack_size,
                    content_crc,
                    method_id: self.method.method_id().to_vec(),
                    method_props: props,
                    empty: false,
                    mtime,
                });
                Ok(true)
            }
            Err(Error::Vanished(_)) => {
                self.rollback_pack(pack_start)?;
                crate::util::soft_skip_note(crate::util::SoftKind::Vanished, archive_name);
                Ok(false)
            }
            Err(e) => {
                // Any compress failure may have written partial pack data — roll back
                // before re-raising so the next member (or finish) stays consistent.
                self.rollback_pack(pack_start)?;
                Err(e)
            }
        }
    }

    /// Truncate the pack stream back to `pack_start` (after a failed/soft-skipped member).
    fn rollback_pack(&mut self, pack_start: u64) -> Result<()> {
        // Flush so BufWriter buffer is empty; then seek+truncate the underlying file.
        self.file.flush().map_err(Error::Io)?;
        let f = self.file.get_mut();
        f.seek(SeekFrom::Start(pack_start)).map_err(Error::Io)?;
        f.set_len(pack_start).map_err(Error::Io)?;
        Ok(())
    }

    /// Append a source file by path (re-stats; prefer [`push_entry`]).
    ///
    /// Returns `Ok(true)` if written, `Ok(false)` if soft-skipped (vanished).
    pub fn push_path(&mut self, name: String, src: &Path) -> Result<bool> {
        let meta = std::fs::symlink_metadata(src).map_err(|e| {
            Error::Archive(format!("stat {} for create: {e}", src.display()))
        })?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(Error::NotRegularFile(src.to_path_buf()));
        }
        let mtime_unix = meta.modified().ok().and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        });
        let (mode, uid, gid) = crate::select::meta_owner_mode(&meta);
        let (uname, gname) = crate::select::names_for_uid_gid(uid, gid);
        let entry = SelectedEntry {
            abs_path: src.to_path_buf(),
            archive_name: name,
            size: meta.len(),
            mtime_unix,
            mode,
            uid,
            gid,
            uname,
            gname,
            kind: crate::select::MemberKind::File,
        };
        self.push_entry(&entry)
    }

    /// Append precompressed pack (tests / parallel encode path).
    pub fn push_packed(&mut self, name: String, compressed: CompressedPack) -> Result<()> {
        self.push_packed_with_mtime(name, compressed, None)
    }

    /// Append precompressed pack with optional mtime (FILETIME).
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
        let pack_crc = if compressed.pack_crc != 0 {
            compressed.pack_crc
        } else {
            crc32fast::hash(&compressed.data)
        };
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

        self.file.flush()?;
        let file = self.file.get_mut();
        let header_pos = file.stream_position()?;
        file.write_all(&header)?;
        let header_crc = crc32fast::hash(&header);

        let next_header_offset = header_pos - SIG_HEADER_SIZE;
        let next_header_size = header.len() as u64;
        let sig = write_start_header(next_header_offset, next_header_size, header_crc);

        file.seek(SeekFrom::Start(0))?;
        file.write_all(&sig)?;
        file.flush()?;
        Ok(())
    }
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

    fn entry_for(path: &Path, name: &str, size: u64) -> SelectedEntry {
        SelectedEntry {
            abs_path: path.to_path_buf(),
            archive_name: name.into(),
            size,
            mtime_unix: None,
            mode: crate::select::DEFAULT_FILE_MODE,
            uid: 0,
            gid: 0,
            uname: String::new(),
            gname: String::new(),
            kind: crate::select::MemberKind::File,
        }
    }

    #[test]
    fn open_time_vanish_soft_skips_keeps_others() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let c = dir.path().join("c.txt");
        fs::write(&a, b"alpha").unwrap();
        fs::write(&b, b"bravo-will-vanish").unwrap();
        fs::write(&c, b"charlie").unwrap();

        let out = dir.path().join("out.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        assert!(w.push_entry(&entry_for(&a, "a.txt", 5)).unwrap());
        // Delete after selection-style entry is built; open soft-skips.
        fs::remove_file(&b).unwrap();
        assert!(!w
            .push_entry(&entry_for(&b, "b.txt", 17))
            .unwrap());
        assert!(w.push_entry(&entry_for(&c, "c.txt", 7)).unwrap());
        w.finish().unwrap();

        assert_eq!(extract(&out, "a.txt"), b"alpha");
        assert_eq!(extract(&out, "c.txt"), b"charlie");
        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        let names: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(names, vec!["a.txt".to_string(), "c.txt".to_string()]);
    }

    #[test]
    fn soft_skip_methods_zstd_lz4() {
        for method in [CompressMethod::Zstd, CompressMethod::Lz4] {
            let dir = tempfile::tempdir().unwrap();
            let good = dir.path().join("good.txt");
            let gone = dir.path().join("gone.txt");
            fs::write(&good, b"keep-me").unwrap();
            fs::write(&gone, b"delete-me").unwrap();
            let out = dir.path().join("m.7z");
            let mut w =
                NonsolidLzma2Writer::create_with_method(&out, 1, method).unwrap();
            assert!(w.push_entry(&entry_for(&good, "good.txt", 7)).unwrap());
            fs::remove_file(&gone).unwrap();
            assert!(!w
                .push_entry(&entry_for(&gone, "gone.txt", 9))
                .unwrap());
            w.finish().unwrap();
            assert_eq!(extract(&out, "good.txt"), b"keep-me");
        }
    }

    #[test]
    fn all_vanished_yields_empty_archive_error() {
        let dir = tempfile::tempdir().unwrap();
        let gone = dir.path().join("gone.txt");
        fs::write(&gone, b"x").unwrap();
        let out = dir.path().join("empty.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        fs::remove_file(&gone).unwrap();
        assert!(!w.push_entry(&entry_for(&gone, "gone.txt", 1)).unwrap());
        assert!(matches!(w.finish().unwrap_err(), Error::EmptyArchive));
    }

    /// T1: zero-byte file deleted before encode must soft-skip (no phantom empty).
    #[test]
    fn zero_byte_vanish_soft_skips_no_phantom_empty() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.dat");
        let keep = dir.path().join("keep.txt");
        fs::write(&empty, b"").unwrap();
        fs::write(&keep, b"neighbor").unwrap();

        let out = dir.path().join("out.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        fs::remove_file(&empty).unwrap();
        assert!(
            !w.push_entry(&entry_for(&empty, "empty.dat", 0)).unwrap(),
            "vanished zero-byte must soft-skip, not write phantom empty"
        );
        assert!(w.push_entry(&entry_for(&keep, "keep.txt", 8)).unwrap());
        w.finish().unwrap();

        assert_eq!(extract(&out, "keep.txt"), b"neighbor");
        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        let names: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(names, vec!["keep.txt".to_string()]);
        assert!(!names.iter().any(|n| n.contains("empty")));
    }

    /// Zero-byte file that still exists is archived as empty.
    #[test]
    fn zero_byte_present_writes_empty_member() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.dat");
        fs::write(&empty, b"").unwrap();
        let out = dir.path().join("out.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        assert!(w.push_entry(&entry_for(&empty, "empty.dat", 0)).unwrap());
        w.finish().unwrap();
        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        let names: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(names, vec!["empty.dat".to_string()]);
        assert_eq!(extract(&out, "empty.dat"), b"");
    }

    /// T3: mid-read skippable I/O → Vanished soft-skip + pack rollback; neighbor ok.
    #[test]
    fn mid_read_soft_skip_rolls_back_keeps_neighbors() {
        use std::io::Read;

        struct FailAfter {
            data: Vec<u8>,
            pos: usize,
            fail_at: usize,
            kind: std::io::ErrorKind,
        }
        impl Read for FailAfter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.pos >= self.fail_at {
                    return Err(std::io::Error::new(self.kind, "injected mid-read"));
                }
                let remain = self
                    .fail_at
                    .saturating_sub(self.pos)
                    .min(self.data.len().saturating_sub(self.pos));
                let n = remain.min(buf.len());
                if n == 0 {
                    return Ok(0);
                }
                buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let c = dir.path().join("c.txt");
        fs::write(&a, b"alpha").unwrap();
        fs::write(&c, b"charlie").unwrap();
        let out = dir.path().join("mid.7z");
        let mut w = NonsolidLzma2Writer::create(&out, 1).unwrap();
        assert!(w.push_entry(&entry_for(&a, "a.txt", 5)).unwrap());

        let payload = b"partial payload that is long enough to stream".repeat(200);
        let mut bad = FailAfter {
            data: payload.clone(),
            pos: 0,
            fail_at: 64,
            kind: std::io::ErrorKind::NotFound,
        };
        let fake = dir.path().join("vanished-mid.bin");
        assert!(
            !w.push_opened_reader(
                "vanished-mid.bin",
                payload.len() as u64,
                &mut bad,
                &fake,
                None,
                0,
            )
            .unwrap(),
            "mid-read NotFound must soft-skip"
        );

        assert!(w.push_entry(&entry_for(&c, "c.txt", 7)).unwrap());
        w.finish().unwrap();

        assert_eq!(extract(&out, "a.txt"), b"alpha");
        assert_eq!(extract(&out, "c.txt"), b"charlie");
        let reader = ArchiveReader::open(&out, Password::empty()).unwrap();
        let names: Vec<_> = reader
            .archive()
            .files
            .iter()
            .filter(|e| !e.is_directory())
            .map(|e| e.name().to_string())
            .collect();
        assert_eq!(names, vec!["a.txt".to_string(), "c.txt".to_string()]);
    }

    /// T3: mid-read PermissionDenied also soft-skips with rollback.
    #[test]
    fn mid_read_permission_denied_soft_skips() {
        use std::io::Read;

        struct FailAfter {
            data: Vec<u8>,
            pos: usize,
            fail_at: usize,
        }
        impl Read for FailAfter {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                if self.pos >= self.fail_at {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::PermissionDenied,
                        "eacces",
                    ));
                }
                let remain = self
                    .fail_at
                    .saturating_sub(self.pos)
                    .min(self.data.len().saturating_sub(self.pos));
                let n = remain.min(buf.len());
                if n == 0 {
                    return Ok(0);
                }
                buf[..n].copy_from_slice(&self.data[self.pos..self.pos + n]);
                self.pos += n;
                Ok(n)
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let keep = dir.path().join("keep.txt");
        fs::write(&keep, b"ok").unwrap();
        let out = dir.path().join("eacces.7z");
        let mut w = NonsolidLzma2Writer::create_with_method(
            &out,
            1,
            CompressMethod::Zstd,
        )
        .unwrap();
        let data = b"z".repeat(4096);
        let mut bad = FailAfter {
            data: data.clone(),
            pos: 0,
            fail_at: 100,
        };
        assert!(!w
            .push_opened_reader(
                "denied.bin",
                data.len() as u64,
                &mut bad,
                Path::new("/unreadable"),
                None,
                0,
            )
            .unwrap());
        assert!(w.push_entry(&entry_for(&keep, "keep.txt", 2)).unwrap());
        w.finish().unwrap();
        assert_eq!(extract(&out, "keep.txt"), b"ok");
    }
}
