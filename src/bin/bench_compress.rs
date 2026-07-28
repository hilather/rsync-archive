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
        "{:<8} {:<6} {:>7} {:>10} {:>10} {:>10} {:>8} {:>8}  {}",
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

                    // --- native ---
                    let native = match method {
                        Method::Lzma2 => native_lzma2(
                            &out_root,
                            &tree,
                            sevenz.as_deref(),
                            t,
                            level,
                            rep,
                        ),
                        Method::Zstd => native_zstd(
                            &out_root,
                            &tree,
                            zstd.as_deref(),
                            t,
                            level,
                            rep,
                        ),
                        Method::Lz4 => native_lz4(
                            &out_root,
                            &tree,
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
    println!("## Fairness notes");
    println!(
        "- **lzma2**: both produce **non-solid 7z** (ours vs `7zz a -m0=LZMA2 -ms=off -mmt=N -mx=L`)."
    );
    println!(
        "- **zstd**: ours = non-solid 7z+Zstd packs; native = `tar | zstd -T N` → **solid stream** (different random-access model). Same mapped zstd level."
    );
    println!(
        "- **lz4**: ours = non-solid 7z+LZ4 frames; native = `tar | lz4 -#` stream (if multi-thread N/A for classic lz4 CLI)."
    );
    println!(
        "- Threads: ours `--threads`/`--encode-concurrency` matched to native `-mmt` / `zstd -T` where applicable."
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

fn native_zstd(
    out_root: &Path,
    tree: &Path,
    zstd: Option<&Path>,
    threads: u32,
    level: u32,
    rep: u32,
) -> NativeResult {
    let Some(z) = zstd else {
        return NativeResult {
            tool: "zstd",
            timed: Timed {
                wall: Duration::ZERO,
                out_bytes: 0,
                ok: false,
                note: "zstd not found".into(),
            },
        };
    };
    let out = out_root.join(format!("native-zstd-t{threads}-l{level}-r{rep}.tar.zst"));
    let _ = fs::remove_file(&out);
    let zl = zstd_cli_level(level);
    // tar -C tree -cf - . | zstd -T threads -level -o out
    let start = Instant::now();
    let result = (|| -> Result<Timed, String> {
        let mut tar = Command::new("tar")
            .args(["-C", tree.to_str().ok_or("tree path")?, "-cf", "-", "."])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("tar spawn: {e}"))?;
        let tar_out = tar.stdout.take().ok_or_else(|| "no tar stdout".to_string())?;
        let zstd_out = Command::new(z)
            .arg(format!("-T{threads}"))
            .arg(format!("-{zl}"))
            .arg("-f") // overwrite
            .arg("-o")
            .arg(&out)
            .stdin(Stdio::from(tar_out))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("zstd: {e}"))?;
        let tar_status = tar.wait().map_err(|e| e.to_string())?;
        if !tar_status.success() {
            let err = tar.stderr.take(); // already waited
            let _ = err;
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: format!("tar exit {:?}", tar_status.code()),
            });
        }
        if !zstd_out.status.success() {
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: format!(
                    "zstd exit {:?}: {}",
                    zstd_out.status.code(),
                    String::from_utf8_lossy(&zstd_out.stderr)
                        .chars()
                        .take(80)
                        .collect::<String>()
                ),
            });
        }
        if !out.exists() {
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: "zstd ok but output missing".into(),
            });
        }
        Ok(Timed {
            wall: start.elapsed(),
            out_bytes: du_bytes(&out),
            ok: true,
            note: "tar|zstd solid stream".into(),
        })
    })();
    NativeResult {
        tool: "tar|zstd",
        timed: result.unwrap_or_else(|e| Timed {
            wall: start.elapsed(),
            out_bytes: 0,
            ok: false,
            note: e,
        }),
    }
}

fn native_lz4(
    out_root: &Path,
    tree: &Path,
    lz4: Option<&Path>,
    threads: u32,
    level: u32,
    rep: u32,
) -> NativeResult {
    let Some(l) = lz4 else {
        return NativeResult {
            tool: "lz4",
            timed: Timed {
                wall: Duration::ZERO,
                out_bytes: 0,
                ok: false,
                note: "lz4 not found".into(),
            },
        };
    };
    let out = out_root.join(format!("native-lz4-t{threads}-l{level}-r{rep}.tar.lz4"));
    let _ = fs::remove_file(&out);
    // Classic lz4 CLI is single-threaded for stream; note thread param unused.
    let lz_level = level.min(9).max(1);
    let start = Instant::now();
    let result = (|| -> Result<Timed, String> {
        let mut tar = Command::new("tar")
            .args(["-C", tree.to_str().ok_or("tree path")?, "-cf", "-", "."])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("tar spawn: {e}"))?;
        let tar_out = tar.stdout.take().ok_or_else(|| "no tar stdout".to_string())?;
        // lz4 -# -f - out.lz4  (stdin → file)
        let lz_out = Command::new(l)
            .arg(format!("-{lz_level}"))
            .arg("-f")
            .arg("-")
            .arg(&out)
            .stdin(Stdio::from(tar_out))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("lz4: {e}"))?;
        let tar_st = tar.wait().map_err(|e| e.to_string())?;
        if !tar_st.success() {
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: format!("tar exit {:?}", tar_st.code()),
            });
        }
        if !lz_out.status.success() {
            return Ok(Timed {
                wall: start.elapsed(),
                out_bytes: 0,
                ok: false,
                note: format!("lz4 exit {:?}", lz_out.status.code()),
            });
        }
        let mut note = "tar|lz4 solid stream".to_string();
        if threads > 1 {
            note.push_str("; lz4 CLI typically ST (threads N/A)");
        }
        Ok(Timed {
            wall: start.elapsed(),
            out_bytes: du_bytes(&out),
            ok: true,
            note,
        })
    })();
    NativeResult {
        tool: "tar|lz4",
        timed: result.unwrap_or_else(|e| Timed {
            wall: start.elapsed(),
            out_bytes: 0,
            ok: false,
            note: e,
        }),
    }
}

fn print_row(method: &str, tool: &str, threads: u32, level: u32, input: u64, t: &Timed) {
    if !t.ok {
        println!(
            "{:<8} {:<12} {:>7} {:>10} {:>10} {:>10} {:>8} {:>8}  FAIL {}",
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
        "{:<8} {:<12} {:>7} {:>10} {:>10.3} {:>10.3} {:>8.2} {:>8.1}  {}",
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
