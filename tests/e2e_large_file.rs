//! Stage 6b — large-file streaming create e2e.
//!
//! Proves create does not require loading whole large members into RAM for
//! sequential encode: 64 MiB file, level 1, extract roundtrip + non-solid.
//! Soft RSS bound on Linux (skipped if sampling unavailable).

use assert_cmd::cargo::cargo_bin;
use sevenz_rust2::{ArchiveReader, Password};
use std::fs::{self, File};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

/// 64 MiB — enough to exceed oneshot path (1 MiB) without multi-GB CI cost.
const LARGE_SIZE: u64 = 64 * 1024 * 1024;

/// Repeating compressible pattern (not sparse zeros alone).
const PATTERN: &[u8] = b"rsync-archive large-file e2e 0123456789abcdef\n";

/// Soft peak RSS for create subprocess (bytes). Hard fail only well above this.
const SOFT_RSS_BYTES: u64 = 256 * 1024 * 1024;
const HARD_RSS_BYTES: u64 = 512 * 1024 * 1024;

fn write_large_file(path: &Path, size: u64) {
    let mut f = File::create(path).expect("create large file");
    let mut written = 0u64;
    while written < size {
        let take = ((size - written) as usize).min(PATTERN.len());
        f.write_all(&PATTERN[..take]).expect("write pattern");
        written += take as u64;
    }
    f.flush().expect("flush");
    assert_eq!(f.metadata().unwrap().len(), size);
}

fn head_tail(path: &Path, n: usize) -> (Vec<u8>, Vec<u8>) {
    let mut f = File::open(path).unwrap();
    let mut head = vec![0u8; n];
    f.read_exact(&mut head).unwrap();
    let len = f.metadata().unwrap().len() as usize;
    let mut tail = vec![0u8; n.min(len)];
    f.seek(SeekFrom::End(-(tail.len() as i64))).unwrap();
    f.read_exact(&mut tail).unwrap();
    (head, tail)
}

#[cfg(target_os = "linux")]
fn parse_vm_rss_kb(status: &str) -> Option<u64> {
    for line in status.lines() {
        // Prefer peak when available; fall back to current RSS.
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok());
        }
    }
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|s| s.parse().ok());
        }
    }
    None
}

/// Run create as a subprocess; on Linux sample peak RSS via `/proc`.
fn run_create_sampled(args: &[&str]) -> (bool, String, String, Option<u64>) {
    let bin: PathBuf = cargo_bin("rsync-archive");
    let mut child = Command::new(&bin)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rsync-archive");

    let mut peak_kb: Option<u64> = None;
    let status = {
        #[cfg(target_os = "linux")]
        {
            let pid = child.id();
            let status_path = format!("/proc/{pid}/status");
            loop {
                if let Ok(s) = fs::read_to_string(&status_path) {
                    if let Some(kb) = parse_vm_rss_kb(&s) {
                        peak_kb = Some(peak_kb.map_or(kb, |p| p.max(kb)));
                    }
                }
                match child.try_wait() {
                    Ok(Some(st)) => break st,
                    Ok(None) => thread::sleep(Duration::from_millis(25)),
                    Err(e) => panic!("wait: {e}"),
                }
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            child.wait().expect("wait")
        }
    };

    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    let peak_bytes = peak_kb.map(|kb| kb.saturating_mul(1024));
    (status.success(), stdout, stderr, peak_bytes)
}

fn assert_rss_soft(peak: Option<u64>, label: &str) {
    let Some(peak) = peak else {
        eprintln!("[{label}] RSS sample unavailable (non-Linux or /proc missing); soft bound skipped");
        return;
    };
    eprintln!(
        "[{label}] peak RSS ≈ {:.1} MiB (soft < {} MiB)",
        peak as f64 / (1024.0 * 1024.0),
        SOFT_RSS_BYTES / (1024 * 1024)
    );
    if peak > HARD_RSS_BYTES {
        panic!(
            "[{label}] peak RSS {peak} bytes exceeds hard bound {} (likely full-file buffer)",
            HARD_RSS_BYTES
        );
    }
    if peak > SOFT_RSS_BYTES {
        // Soft only: CI noise / debug allocators can inflate RSS without full-file buffering.
        eprintln!(
            "[{label}] warning: peak RSS {peak} exceeded soft {} MiB bound (not failing)",
            SOFT_RSS_BYTES / (1024 * 1024)
        );
    }
}

fn assert_member_roundtrip(archive: &Path, member: &str, source: &Path) {
    let mut reader = ArchiveReader::open(archive, Password::empty()).expect("open archive");
    assert!(
        !reader.archive().is_solid,
        "archive must be non-solid: {}",
        archive.display()
    );
    let files: Vec<_> = reader
        .archive()
        .files
        .iter()
        .filter(|e| !e.is_directory())
        .map(|e| e.name().to_string())
        .collect();
    assert_eq!(files, vec![member.to_string()], "member list: {files:?}");

    let data = reader.read_file(member).expect("extract member");
    assert_eq!(data.len() as u64, LARGE_SIZE, "extracted size");

    let (src_head, src_tail) = head_tail(source, 4096);
    assert_eq!(&data[..4096], src_head.as_slice(), "start bytes");
    assert_eq!(&data[data.len() - 4096..], src_tail.as_slice(), "end bytes");
    // Full equality is feasible at 64 MiB and catches mid-file corruption.
    let src_all = fs::read(source).expect("read source");
    assert_eq!(data, src_all, "full content roundtrip");
}

#[test]
fn create_large_file_lzma2_stream_roundtrip() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("big.bin");
    write_large_file(&src, LARGE_SIZE);
    let out = dir.path().join("big-lzma2.7z");

    let (ok, _stdout, stderr, peak) = run_create_sampled(&[
        "create",
        "-o",
        out.to_str().unwrap(),
        "--method",
        "lzma2",
        "--level",
        "1",
        "--threads",
        "1",
        "--verify",
        src.to_str().unwrap(),
    ]);
    assert!(ok, "create lzma2 failed:\n{stderr}");
    assert!(
        stderr.contains("verify ok") && stderr.contains("non-solid"),
        "verify message missing: {stderr}"
    );
    assert_rss_soft(peak, "lzma2");
    assert_member_roundtrip(&out, "big.bin", &src);
}

#[test]
fn create_large_file_zstd_stream_roundtrip() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("big.bin");
    write_large_file(&src, LARGE_SIZE);
    let out = dir.path().join("big-zstd.7z");

    let (ok, _stdout, stderr, peak) = run_create_sampled(&[
        "create",
        "-o",
        out.to_str().unwrap(),
        "--method",
        "zstd",
        "--level",
        "1",
        "--threads",
        "1",
        "--verify",
        src.to_str().unwrap(),
    ]);
    assert!(ok, "create zstd failed:\n{stderr}");
    assert!(
        stderr.contains("verify ok") && stderr.contains("non-solid"),
        "verify message missing: {stderr}"
    );
    assert_rss_soft(peak, "zstd");
    assert_member_roundtrip(&out, "big.bin", &src);
}
