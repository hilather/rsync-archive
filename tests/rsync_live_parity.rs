//! Live dry-run parity: rsync vs `rsync-archive create -n`.
//!
//! Soft-skips when `rsync` is not installed (eprintln + return; not `#[ignore]`).
//! Compares selected **file** path sets (order-independent) for trailing-slash SRC
//! mapping (archive root = contents of SRC, not the SRC basename).
//!
//! See `docs/SELECTION.md` § Live rsync parity.

use assert_cmd::cargo::cargo_bin;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

fn rsync_available() -> bool {
    Command::new("rsync")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Fixture tree used by all scenarios:
/// ```text
/// root/
///   a.txt
///   a.c
///   b.tmp
///   data/keep.txt
///   data/nested/x.log
///   data/elasticsearch/indices/shard.dat
///   other/y.txt
///   src/a.c
/// ```
fn build_fixture() -> (TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().join("root");
    fs::create_dir_all(root.join("data/nested")).unwrap();
    fs::create_dir_all(root.join("data/elasticsearch/indices")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("a.txt"), b"a").unwrap();
    fs::write(root.join("a.c"), b"root-c").unwrap();
    fs::write(root.join("b.tmp"), b"tmp").unwrap();
    fs::write(root.join("data/keep.txt"), b"keep").unwrap();
    fs::write(root.join("data/nested/x.log"), b"log").unwrap();
    fs::write(root.join("data/elasticsearch/indices/shard.dat"), b"shard").unwrap();
    fs::write(root.join("other/y.txt"), b"y").unwrap();
    fs::write(root.join("src/a.c"), b"src-c").unwrap();
    (tmp, root)
}

fn normalize_rel(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // rsync may emit directory members with trailing `/` — drop them for file-set parity.
    if s.ends_with('/') {
        return None;
    }
    let s = s.strip_prefix("./").unwrap_or(s);
    let s = s.trim_start_matches('/');
    if s.is_empty() {
        return None;
    }
    Some(s.replace('\\', "/"))
}

/// List relative file paths rsync would transfer (dry-run), trailing-slash SRC.
fn rsync_file_set(src: &Path, filter_args: &[String]) -> BTreeSet<String> {
    let dest = src
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("rsync-dest");
    fs::create_dir_all(&dest).unwrap();

    let src_slash = format!("{}/", src.display());
    let dest_slash = format!("{}/", dest.display());

    let mut cmd = Command::new("rsync");
    cmd.arg("-a")
        .arg("-n")
        .arg("--out-format=%n");
    for f in filter_args {
        cmd.arg(f);
    }
    cmd.arg(&src_slash).arg(&dest_slash);

    let out = cmd.output().expect("spawn rsync");
    assert!(
        out.status.success(),
        "rsync failed (status {:?}):\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(normalize_rel)
        .collect()
}

/// Our create dry-run archive names (one per stdout line).
fn our_file_set(src: &Path, cli_filter_args: &[String]) -> BTreeSet<String> {
    let out_path = src
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("out.7z");
    let bin = cargo_bin!("rsync-archive");
    let src_slash = format!("{}/", src.display());

    let mut cmd = Command::new(&bin);
    cmd.arg("create")
        .arg("-o")
        .arg(&out_path)
        .arg("-n");
    for a in cli_filter_args {
        cmd.arg(a);
    }
    cmd.arg(&src_slash);

    let out = cmd.output().expect("spawn rsync-archive");
    assert!(
        out.status.success(),
        "rsync-archive failed (status {:?}):\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!out_path.exists(), "dry-run must not create -o");

    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(normalize_rel)
        .collect()
}

fn assert_sets_eq(scenario: &str, rsync: &BTreeSet<String>, ours: &BTreeSet<String>) {
    if rsync == ours {
        return;
    }
    let only_rsync: BTreeSet<_> = rsync.difference(ours).cloned().collect();
    let only_ours: BTreeSet<_> = ours.difference(rsync).cloned().collect();
    panic!(
        "scenario `{scenario}` file-set mismatch\n\
         only in rsync ({n_r}): {only_rsync:?}\n\
         only in rsync-archive ({n_o}): {only_ours:?}\n\
         rsync full: {rsync:?}\n\
         ours full:  {ours:?}",
        n_r = only_rsync.len(),
        n_o = only_ours.len(),
    );
}

/// Write a merge-filter file and return rsync `--filter=. PATH` plus our `--filter=LINE` args.
fn filter_file_args(dir: &Path, name: &str, body: &str) -> (Vec<String>, Vec<String>) {
    let path = dir.join(name);
    fs::write(&path, body).unwrap();
    let rsync = vec![format!("--filter=. {}", path.display())];
    let ours: Vec<String> = body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        // clap treats a following arg that starts with `-` as a flag; use `--filter=…`.
        .map(|l| format!("--filter={l}"))
        .collect();
    (rsync, ours)
}

#[test]
fn live_rsync_exclude_tmp() {
    if !rsync_available() {
        eprintln!("skipping live rsync parity (exclude *.tmp): rsync not found");
        return;
    }
    let (_tmp, root) = build_fixture();
    let rsync_args = vec!["--exclude=*.tmp".to_string()];
    let our_args = vec!["--exclude=*.tmp".to_string()];
    let r = rsync_file_set(&root, &rsync_args);
    let o = our_file_set(&root, &our_args);
    assert_sets_eq("exclude *.tmp", &r, &o);
    assert!(!r.contains("b.tmp"), "tmp must be excluded");
    assert!(r.contains("a.txt") && r.contains("src/a.c"));
}

#[test]
fn live_rsync_include_only_c() {
    if !rsync_available() {
        eprintln!("skipping live rsync parity (include-only *.c): rsync not found");
        return;
    }
    let (tmp, root) = build_fixture();
    // Classic rsync include-only: allow directory descent, keep `*.c`, drop the rest.
    // Plain `+ *.c` / `- *` alone does not enter dirs in rsync; `+ */` is required to
    // pack nested `src/a.c`. Both tools then list the same .c files.
    let body = "\
+ */
+ *.c
- *
";
    let (rsync_args, our_args) = filter_file_args(tmp.path(), "include-c.filter", body);
    let r = rsync_file_set(&root, &rsync_args);
    let o = our_file_set(&root, &our_args);
    assert_sets_eq("include-only *.c", &r, &o);
    assert_eq!(
        r,
        BTreeSet::from([
            "a.c".to_string(),
            "src/a.c".to_string(),
        ])
    );
}

#[test]
fn live_rsync_tree_include_with_starstar() {
    if !rsync_available() {
        eprintln!("skipping live rsync parity (tree include + /**): rsync not found");
        return;
    }
    let (tmp, root) = build_fixture();
    let body = "\
+ /data/
+ /data/**
- *
";
    let (rsync_args, our_args) = filter_file_args(tmp.path(), "tree-starstar.filter", body);
    let r = rsync_file_set(&root, &rsync_args);
    let o = our_file_set(&root, &our_args);
    assert_sets_eq("tree include + /data/**", &r, &o);
    assert!(r.contains("data/keep.txt"));
    assert!(r.contains("data/nested/x.log"));
    assert!(r.contains("data/elasticsearch/indices/shard.dat"));
    assert!(!r.contains("a.txt"));
    assert!(!r.contains("other/y.txt"));
    assert!(!r.iter().any(|p| p.starts_with("other/")));
    assert!(!r.iter().any(|p| p.starts_with("src/")));
}

#[test]
fn live_rsync_tree_include_without_starstar() {
    if !rsync_available() {
        eprintln!("skipping live rsync parity (tree include without /**): rsync not found");
        return;
    }
    let (tmp, root) = build_fixture();
    // Directory include alone does not include files under the dir (rsync-aligned).
    let body = "\
+ /data/
- *
";
    let (rsync_args, our_args) = filter_file_args(tmp.path(), "tree-dir-only.filter", body);
    let r = rsync_file_set(&root, &rsync_args);
    let o = our_file_set(&root, &our_args);
    assert_sets_eq("tree include without /**", &r, &o);
    // Expect no files under data/ (rsync lists only the `data/` directory, which we drop).
    assert!(
        r.is_empty(),
        "expected no files when only + /data/ then - *; got {r:?}"
    );
}
