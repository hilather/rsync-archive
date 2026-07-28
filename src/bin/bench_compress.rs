//! Fair compression benchmarks: rsync-archive vs native tools.
//!
//! Compares under matched threads / level / non-solid (where applicable).
//!
//! ```text
//! cargo build --release --bin bench_compress --bin rsync-archive
//! ./target/release/bench_compress run --scale small --threads 1,4 --level 1,5
//! ```

use clap::{Parser, Subcommand, ValueEnum};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
#[command(name = "bench_compress", about = "rsync-archive vs native compression benches")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// Generate fixture trees under benchdata/
    Generate {
        #[arg(long, default_value = "small")]
        scale: Scale,
        #[arg(long, default_value = "benchdata")]
        dir: PathBuf,
    },
    /// Run comparisons (generates fixtures if missing)
    Run {
        #[arg(long, default_value = "small")]
        scale: Scale,
        #[arg(long, default_value = "1,4")]
        threads: String,
        #[arg(long, default_value = "1,5")]
        level: String,
        #[arg(long, default_value = "benchdata")]
        dir: PathBuf,
        /// Comma list: lzma2,zstd,lz4,all
        #[arg(long, default_value = "all")]
        methods: String,
        /// Repeat each cell
        #[arg(long, default_value_t = 1)]
        reps: u32,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum Scale {
    /// ~200 files, ~2 MiB — quick
    Tiny,
    /// ~2k files, ~20 MiB
    Small,
    /// ~10k files, ~100 MiB
    Medium,
}

impl Scale {
    fn name(self) -> &'static str {
        match self {
            Scale::Tiny => "tiny",
            Scale::Small => "small",
            Scale::Medium => "medium",
        }
    }

    fn file_count(self) -> usize {
        match self {
            Scale::Tiny => 200,
            Scale::Small => 2_000,
            Scale::Medium => 10_000,
        }
    }

    /// Approximate bytes per file (compressible text-ish pattern).
    fn bytes_per_file(self) -> usize {
        match self {
            Scale::Tiny => 8 * 1024,
            Scale::Small => 10 * 1024,
            Scale::Medium => 10 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Method {
    Lzma2,
    Zstd,
    Lz4,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Method::Lzma2 => "lzma2",
            Method::Zstd => "zstd",
            Method::Lz4 => "lz4",
        }
    }

    fn parse_list(s: &str) -> Vec<Method> {
        let s = s.trim().to_ascii_lowercase();
        if s == "all" {
            return vec![Method::Lzma2, Method::Zstd, Method::Lz4];
        }
        s.split(',')
            .filter_map(|p| match p.trim() {
                "lzma2" | "lzma" => Some(Method::Lzma2),
                "zstd" | "zst" => Some(Method::Zstd),
                "lz4" => Some(Method::Lz4),
                _ => None,
            })
            .collect()
    }
}

/// Map our 0–9 level to zstd CLI level (matches create codec mapping).
fn zstd_cli_level(level: u32) -> i32 {
    match level.min(9) {
        0 | 1 => 1,
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

fn main() {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Generate { scale, dir } => {
            let tree = fixture_tree(&dir, scale);
            generate_fixture(&tree, scale).expect("generate");
            println!("generated {}", tree.display());
        }
        Cmd::Run {
            scale,
            threads,
            level,
            dir,
            methods,
            reps,
        } => {
            run_bench(scale, &threads, &level, &dir, &methods, reps).expect("bench");
        }
    }
}

fn fixture_tree(root: &Path, scale: Scale) -> PathBuf {
    root.join(scale.name()).join("tree")
}

fn generate_fixture(tree: &Path, scale: Scale) -> std::io::Result<()> {
    if tree.join(".bench_ready").exists() {
        return Ok(());
    }
    if tree.exists() {
        fs::remove_dir_all(tree)?;
    }
    fs::create_dir_all(tree)?;
    let n = scale.file_count();
    let bpf = scale.bytes_per_file();
    // Deterministic compressible payload
    let chunk = b"The quick brown fox jumps over the lazy dog. 0123456789\n";
    for i in 0..n {
        let sub = tree.join(format!("d{:03}", i % 50));
        fs::create_dir_all(&sub)?;
        let path = sub.join(format!("f{:05}.txt", i));
        let mut f = File::create(&path)?;
        let mut written = 0usize;
        while written < bpf {
            let take = (bpf - written).min(chunk.len());
            f.write_all(&chunk[..take])?;
            written += take;
        }
    }
    File::create(tree.join(".bench_ready"))?;
    Ok(())
}

fn parse_u32_list(s: &str) -> Vec<u32> {
    s.split(',')
        .filter_map(|p| p.trim().parse().ok())
        .collect()
}

fn which(bin: &str) -> Option<PathBuf> {
    Command::new("which")
        .arg(bin)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
}

fn du_bytes(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map(|m| m.len()).unwrap_or(0);
    }
    let mut total = 0u64;
    if let Ok(rd) = fs::read_dir(path) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                total = total.saturating_add(du_bytes(&p));
            } else if let Ok(m) = p.metadata() {
                total = total.saturating_add(m.len());
            }
        }
    }
    total
}

struct Timed {
    wall: Duration,
    out_bytes: u64,
    ok: bool,
    note: String,
}

fn run_timed(mut cmd: Command) -> Timed {
    let start = Instant::now();
    let out = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    let wall = start.elapsed();
    match out {
        Ok(o) if o.status.success() => Timed {
            wall,
            out_bytes: 0,
            ok: true,
            note: String::new(),
        },
        Ok(o) => Timed {
            wall,
            out_bytes: 0,
            ok: false,
            note: format!(
                "exit {:?}: {}",
                o.status.code(),
                String::from_utf8_lossy(&o.stderr).chars().take(120).collect::<String>()
            ),
        },
        Err(e) => Timed {
            wall,
            out_bytes: 0,
            ok: false,
            note: format!("spawn: {e}"),
        },
    }
}

fn our_bin() -> PathBuf {
    // Prefer sibling release binary next to this bench, else cargo target path.
    let candidates = [
        PathBuf::from("target/release/rsync-archive"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/rsync-archive"),
    ];
    for c in candidates {
        if c.exists() {
            return c;
        }
    }
    PathBuf::from("rsync-archive")
}

fn run_bench(
    scale: Scale,
    threads_s: &str,
    levels_s: &str,
    dir: &Path,
    methods_s: &str,
    reps: u32,
) -> Result<(), Box<dyn std::error::Error>> {
    let tree = fixture_tree(dir, scale);
    generate_fixture(&tree, scale)?;
    let input_bytes = du_bytes(&tree);
    let threads = parse_u32_list(threads_s);
    let levels = parse_u32_list(levels_s);
    let methods = Method::parse_list(methods_s);
    let our = our_bin();
    let sevenz = which("7zz").or_else(|| which("7z"));
    let zstd = which("zstd");
    let lz4 = which("lz4");

    println!("# bench_compress scale={} input_bytes={} ({:.2} MiB)",
        scale.name(),
        input_bytes,
        input_bytes as f64 / (1024.0 * 1024.0)
    );
    println!("# our={}", our.display());
    println!(
        "# native: 7zz={} zstd={} lz4={}",
        sevenz.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "MISSING".into()),
        zstd.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "MISSING".into()),
        lz4.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "MISSING".into()),
    );
    println!();
    println!(
        "{:<8} {:<16} {:>7} {:>10} {:>10} {:>10} {:>8} {:>8}  {}",
        "method", "tool", "threads", "level", "sec", "out_MiB", "ratio", "MiB/s", "notes"
    );

    let out_root = dir.join(scale.name()).join("out");
    fs::create_dir_all(&out_root)?;

    for method in methods {
        for &level in &levels {
            for &t in &threads {
                for rep in 0..reps {
                    // --- our tool ---
                    let our_out = out_root.join(format!(
                        "our-{}-t{}-l{}-r{}.7z",
                        method.as_str(),
                        t,
                        level,
                        rep
                    ));
                    let _ = fs::remove_file(&our_out);
                    let mut cmd = Command::new(&our);
                    cmd.args([
                        "create",
                        "-o",
                        our_out.to_str().unwrap(),
                        "--method",
                        method.as_str(),
                        "--level",
                        &level.to_string(),
                        "--threads",
                        &t.to_string(),
                        "--encode-concurrency",
                        &t.to_string(),
                        "--force",
                    ]);
                    // trailing slash = archive names without tree prefix
                    let src = format!("{}/", tree.display());
                    cmd.arg(&src);
                    let mut timed = run_timed(cmd);
                    if timed.ok {
                        timed.out_bytes = du_bytes(&our_out);
                    }
                    print_row(method.as_str(), "rsync-archive", t, level, input_bytes, &timed);

                    // --- native (non-solid only) ---
                    let native = match method {
                        Method::Lzma2 => native_lzma2(
                            &out_root,
                            &tree,
                            sevenz.as_deref(),
                            t,
                            level,
                            rep,
                        ),
                        Method::Zstd => native_codec_store(
                            CodecKind::Zstd,
                            &out_root,
                            &tree,
                            sevenz.as_deref(),
                            zstd.as_deref(),
                            t,
                            level,
                            rep,
                        ),
                        Method::Lz4 => native_codec_store(
                            CodecKind::Lz4,
                            &out_root,
                            &tree,
                            sevenz.as_deref(),
                            lz4.as_deref(),
                            t,
                            level,
                            rep,
                        ),
                    };
                    print_row(
                        method.as_str(),
                        native.tool,
                        t,
                        level,
                        input_bytes,
                        &native.timed,
                    );
                }
            }
        }
    }

    println!();
    println!("## Fairness notes (non-solid only)");
    println!(
        "- **All baselines are non-solid multi-member archives** (no solid tar streams)."
    );
    println!(
        "- **lzma2**: ours vs `7zz a -m0=LZMA2 -ms=off -mmt=N -mx=L` (same 7z non-solid model)."
    );
    println!(
        "- **zstd/lz4**: stock 7zz cannot do -m0=zstd/lz4. Native proxy = **per-file CLI compress** (parallel up to N workers) then **`7zz a -m0=Copy -ms=off`** store outer. Same idea as our non-solid packs + independent compressed streams."
    );
    println!(
        "- Threads: ours `--threads`/`--encode-concurrency` matched to 7zz `-mmt` (lzma2) or parallel per-file encode workers (zstd/lz4 proxy)."
    );
    println!(
        "- Wall time includes process spawn + full archive write; output size is final artifact bytes."
    );
    Ok(())
}

struct NativeResult {
    tool: &'static str,
    timed: Timed,
}

fn native_lzma2(
    out_root: &Path,
    tree: &Path,
    sevenz: Option<&Path>,
    threads: u32,
    level: u32,
    rep: u32,
) -> NativeResult {
    let Some(sz) = sevenz else {
        return NativeResult {
            tool: "7zz",
            timed: Timed {
                wall: Duration::ZERO,
                out_bytes: 0,
                ok: false,
                note: "7zz/7z not found".into(),
            },
        };
    };
    let out = out_root.join(format!("native-lzma2-t{threads}-l{level}-r{rep}.7z"));
    let _ = fs::remove_file(&out);
    // Non-solid multi-file 7z. Pass absolute paths; include tree contents via `/*`.
    // Using `tree/.` keeps paths under `.` similar to our trailing-slash SRC layout.
    let mut cmd = Command::new(sz);
    cmd.arg("a")
        .arg("-t7z")
        .arg("-m0=LZMA2")
        .arg(format!("-mx={level}"))
        .arg(format!("-mmt={threads}"))
        .arg("-ms=off")
        .arg("-bso0")
        .arg("-bsp0")
        .arg(out.as_os_str())
        .arg(format!("{}/.", tree.display()));
    let mut timed = run_timed(cmd);
    if timed.ok {
        if !out.exists() {
            timed.ok = false;
            timed.note = "7zz reported ok but output missing".into();
        } else {
            timed.out_bytes = du_bytes(&out);
            timed.note = "non-solid 7z -ms=off".into();
        }
    }
    NativeResult {
        tool: "7zz-LZMA2",
        timed,
    }
}

#[derive(Clone, Copy)]
enum CodecKind {
    Zstd,
    Lz4,
}

impl CodecKind {
    fn name(self) -> &'static str {
        match self {
            CodecKind::Zstd => "zstd",
            CodecKind::Lz4 => "lz4",
        }
    }

    fn tool_label(self) -> &'static str {
        match self {
            CodecKind::Zstd => "zstd+7zz-Copy",
            CodecKind::Lz4 => "lz4+7zz-Copy",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            CodecKind::Zstd => "zst",
            CodecKind::Lz4 => "lz4",
        }
    }
}

/// Non-solid native proxy for zstd/lz4:
/// 1) compress each file with CLI (up to `threads` parallel workers)
/// 2) pack compressed blobs into non-solid 7z via `7zz -m0=Copy -ms=off`
///
/// Stock 7zz cannot encode ZSTD/LZ4 methods inside .7z; this is the fair layout peer.
fn native_codec_store(
    codec: CodecKind,
    out_root: &Path,
    tree: &Path,
    sevenz: Option<&Path>,
    codec_bin: Option<&Path>,
    threads: u32,
    level: u32,
    rep: u32,
) -> NativeResult {
    let Some(cbin) = codec_bin else {
        return NativeResult {
            tool: codec.tool_label(),
            timed: Timed {
                wall: Duration::ZERO,
                out_bytes: 0,
                ok: false,
                note: format!("{} CLI not found", codec.name()),
            },
        };
    };
    let Some(sz) = sevenz else {
        return NativeResult {
            tool: codec.tool_label(),
            timed: Timed {
                wall: Duration::ZERO,
                out_bytes: 0,
                ok: false,
                note: "7zz not found (needed for Copy outer)".into(),
            },
        };
    };

    let out = out_root.join(format!(
        "native-{}-t{}-l{}-r{}.7z",
        codec.name(),
        threads,
        level,
        rep
    ));
    let work = out_root.join(format!(
        "native-{}-t{}-l{}-r{}-work",
        codec.name(),
        threads,
        level,
        rep
    ));
    let _ = fs::remove_file(&out);
    let _ = fs::remove_dir_all(&work);

    let start = Instant::now();
    let result = (|| -> Result<Timed, String> {
        fs::create_dir_all(&work).map_err(|e| e.to_string())?;
        let files = collect_regular_files(tree).map_err(|e| e.to_string())?;
        if files.is_empty() {
            return Err("no files in fixture tree".into());
        }

        // Parallel per-file encode with worker cap = threads
        let workers = threads.max(1) as usize;
        let zl = zstd_cli_level(level);
        let lz_level = level.min(9).max(1);

        std::thread::scope(|scope| -> Result<(), String> {
            let mut next = 0usize;
            let mut handles = Vec::new();
            while next < files.len() || !handles.is_empty() {
                while next < files.len() && handles.len() < workers {
                    let src = files[next].clone();
                    let rel = src
                        .strip_prefix(tree)
                        .map_err(|_| "strip_prefix".to_string())?
                        .to_path_buf();
                    // Mirror relative path + codec extension
                    let dest = work.join(format!("{}.{}", rel.display(), codec.ext()));
                    if let Some(parent) = dest.parent() {
                        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    let cbin = cbin.to_path_buf();
                    next += 1;
                    handles.push(scope.spawn(move || {
                        compress_one_file(codec, &cbin, &src, &dest, zl, lz_level)
                    }));
                }
                // Join oldest in-flight job (FIFO) to free a worker slot
                let h = handles.remove(0);
                h.join()
                    .map_err(|_| "worker panicked".to_string())??;
            }
            Ok(())
        })?;

        // Store compressed blobs in non-solid 7z (Copy)
        let st = Command::new(sz)
            .arg("a")
            .arg("-t7z")
            .arg("-m0=Copy")
            .arg("-ms=off")
            .arg("-mmt=1")
            .arg("-bso0")
            .arg("-bsp0")
            .arg(out.as_os_str())
            .arg(format!("{}/.", work.display()))
            .status()
            .map_err(|e| e.to_string())?;
        if !st.success() {
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: format!("7zz Copy exit {:?}", st.code()),
            });
        }
        if !out.exists() {
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: "7zz Copy ok but output missing".into(),
            });
        }
        let _ = fs::remove_dir_all(&work);
        Ok(Timed {
            wall: start.elapsed(),
            out_bytes: du_bytes(&out),
            ok: true,
            note: "per-file CLI + 7zz Copy non-solid".into(),
        })
    })();

    if result.is_err() {
        let _ = fs::remove_dir_all(&work);
    }

    NativeResult {
        tool: codec.tool_label(),
        timed: result.unwrap_or_else(|e| Timed {
            wall: start.elapsed(),
            out_bytes: 0,
            ok: false,
            note: e,
        }),
    }
}

fn collect_regular_files(tree: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            let p = e.path();
            let name = e.file_name();
            if name == ".bench_ready" {
                continue;
            }
            if p.is_dir() {
                walk(&p, out)?;
            } else if p.is_file() {
                out.push(p);
            }
        }
        Ok(())
    }
    walk(tree, &mut out)?;
    out.sort();
    Ok(out)
}

fn compress_one_file(
    codec: CodecKind,
    bin: &Path,
    src: &Path,
    dest: &Path,
    zstd_level: i32,
    lz4_level: u32,
) -> Result<(), String> {
    match codec {
        CodecKind::Zstd => {
            // zstd -T1 -# -f -o dest src  (one file per process; parallelism is across files)
            let st = Command::new(bin)
                .args([
                    "-T1",
                    &format!("-{zstd_level}"),
                    "-f",
                    "-o",
                ])
                .arg(dest)
                .arg(src)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| e.to_string())?;
            if !st.success() {
                return Err(format!("zstd failed on {}", src.display()));
            }
        }
        CodecKind::Lz4 => {
            // lz4 -# -f src dest
            let st = Command::new(bin)
                .arg(format!("-{lz4_level}"))
                .arg("-f")
                .arg(src)
                .arg(dest)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|e| e.to_string())?;
            if !st.success() {
                return Err(format!("lz4 failed on {}", src.display()));
            }
        }
    }
    Ok(())
}

fn print_row(method: &str, tool: &str, threads: u32, level: u32, input: u64, t: &Timed) {
    if !t.ok {
        println!(
            "{:<8} {:<16} {:>7} {:>10} {:>10} {:>10} {:>8} {:>8}  FAIL {}",
            method,
            tool,
            threads,
            level,
            format!("{:.3}", t.wall.as_secs_f64()),
            "-",
            "-",
            "-",
            t.note.replace('\n', " ")
        );
        return;
    }
    let sec = t.wall.as_secs_f64().max(1e-9);
    let out_mib = t.out_bytes as f64 / (1024.0 * 1024.0);
    let in_mib = input as f64 / (1024.0 * 1024.0);
    let ratio = if t.out_bytes > 0 {
        input as f64 / t.out_bytes as f64
    } else {
        0.0
    };
    let mibs = in_mib / sec;
    println!(
        "{:<8} {:<16} {:>7} {:>10} {:>10.3} {:>10.3} {:>8.2} {:>8.1}  {}",
        method,
        tool,
        threads,
        level,
        sec,
        out_mib,
        ratio,
        mibs,
        t.note
    );
}
