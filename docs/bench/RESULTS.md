# Compression bench results (non-solid only)

Host: Linux, **12** cores. Date: **2026-07-28**.  
Harness: `bench_compress` (release). **No solid tar streams.**

## Fairness

| Method | Ours | Native (non-solid) |
|--------|------|---------------------|
| lzma2 | non-solid 7z + LZMA2 | **7zz** non-solid LZMA2 (`-ms=off`) |
| zstd | non-solid 7z + Zstd packs | **zstd CLI per file** (≤N workers) + **7zz Copy** non-solid |
| lz4 | non-solid 7z + LZ4 packs | **lz4 CLI per file** (≤N workers) + **7zz Copy** non-solid |

## Scale: tiny (~200 files, ~1.7 MiB)

| method | tool | t | level | sec | out_MiB | ratio | MiB/s |
|--------|------|--:|------:|----:|--------:|------:|------:|
| lzma2 | rsync-archive | 1 | 1 | 0.176 | 0.048 | 34.4 | 9.4 |
| lzma2 | **7zz-LZMA2** | 1 | 1 | **0.055** | **0.037** | **44.6** | 29.9 |
| lzma2 | **rsync-archive** | 4 | 1 | **0.098** | 0.048 | 34.4 | 17.0 |
| lzma2 | 7zz-LZMA2 | 4 | 1 | 0.195 | 0.037 | 44.6 | 8.5 |
| lzma2 | rsync-archive | 1 | 5 | 0.531 | 0.048 | 34.4 | 3.1 |
| lzma2 | **7zz-LZMA2** | 1 | 5 | **0.107** | **0.037** | **44.6** | 15.5 |
| lzma2 | rsync-archive | 4 | 5 | 0.433 | 0.048 | 34.4 | 3.8 |
| lzma2 | **7zz-LZMA2** | 4 | 5 | **0.281** | **0.037** | **44.6** | 5.9 |
| zstd | **rsync-archive** | 1 | 1 | **0.042** | 0.044 | 37.6 | **39** |
| zstd | zstd+7zz-Copy | 1 | 1 | 0.486 | **0.032** | **51.2** | 3.4 |
| zstd | **rsync-archive** | 4 | 1 | **0.026** | 0.044 | 37.6 | **63** |
| zstd | zstd+7zz-Copy | 4 | 1 | 0.180 | **0.032** | **51.3** | 9.2 |
| zstd | **rsync-archive** | 1 | 5 | **0.050** | 0.044 | 37.5 | **33** |
| zstd | zstd+7zz-Copy | 1 | 5 | 0.402 | **0.032** | **51.1** | 4.1 |
| lz4 | **rsync-archive** | 1 | 1 | **0.038** | 0.052 | 31.9 | **44** |
| lz4 | lz4+7zz-Copy | 1 | 1 | 0.680 | **0.040** | **41.1** | 2.4 |
| lz4 | **rsync-archive** | 4 | 1 | **0.075** | 0.052 | 31.9 | 22 |
| lz4 | lz4+7zz-Copy | 4 | 1 | 0.257 | **0.040** | **41.2** | 6.5 |

## Scale: small (~2k files, ~20 MiB)

| method | tool | t | level | sec | out_MiB | ratio | MiB/s |
|--------|------|--:|------:|----:|--------:|------:|------:|
| lzma2 | rsync-archive | 1 | 1 | 1.01 | 0.321 | 60.9 | 19 |
| lzma2 | **7zz-LZMA2** | 1 | 1 | **0.66** | **0.209** | **93.5** | 30 |
| lzma2 | rsync-archive | 4 | 1 | 0.99 | 0.321 | 60.9 | 20 |
| lzma2 | **7zz-LZMA2** | 4 | 1 | **0.73** | **0.209** | **93.5** | 27 |
| lzma2 | rsync-archive | 1 | 5 | 3.42 | 0.321 | 60.9 | 5.7 |
| lzma2 | **7zz-LZMA2** | 1 | 5 | **1.28** | **0.209** | **93.5** | 15 |
| lzma2 | rsync-archive | 4 | 5 | 3.55 | 0.321 | 60.9 | 5.5 |
| lzma2 | **7zz-LZMA2** | 4 | 5 | **2.40** | **0.209** | **93.5** | 8.1 |
| zstd | **rsync-archive** | 1 | 1 | **0.07** | 0.273 | 71.5 | **270** |
| zstd | zstd+7zz-Copy | 1 | 1 | 7.73 | **0.154** | **127** | 2.5 |
| zstd | **rsync-archive** | 4 | 1 | **0.49** | 0.273 | 71.5 | 40 |
| zstd | zstd+7zz-Copy | 4 | 1 | 2.59 | **0.154** | **127** | 7.6 |
| zstd | **rsync-archive** | 1 | 5 | **0.46** | 0.273 | 71.5 | **43** |
| zstd | zstd+7zz-Copy | 1 | 5 | 5.61 | **0.154** | **127** | 3.5 |
| lz4 | **rsync-archive** | 1 | 1 | **0.11** | 0.357 | 54.7 | **180** |
| lz4 | lz4+7zz-Copy | 1 | 1 | 4.15 | **0.240** | **81** | 4.7 |
| lz4 | **rsync-archive** | 4 | 1 | **0.69** | 0.357 | 54.7 | 28 |
| lz4 | lz4+7zz-Copy | 4 | 1 | 2.04 | **0.240** | **81** | 9.6 |

## Takeaways (non-solid peers)

1. **lzma2 vs 7zz (true peer):** 7zz still **smaller** (~1.3–1.5×) and usually **faster**, especially at level 5. Our multi-worker encode sometimes wins wall time at low level / tiny sets.
2. **zstd/lz4 vs CLI+Copy proxy:** We are **much faster** (in-process encode vs thousands of process spawns + second 7zz pass). Native proxy still **smaller** (libzstd/lz4 CLI + lean store outer).
3. Process-spawn proxy is a **harsh** but fair **layout** peer; installing **7-Zip-zstd** would enable a true `-m0=zstd/-m0=lz4 -ms=off` peer later.
4. Within our tool, **`--method zstd`** remains the throughput pick; **lzma2** the size pick.

## Reproduce

```bash
cargo build --release --bin rsync-archive --bin bench_compress
export PATH="$HOME/.local/bin:$PATH"
./target/release/bench_compress run --scale small --threads 1,4 --level 1,5 --methods all
```
