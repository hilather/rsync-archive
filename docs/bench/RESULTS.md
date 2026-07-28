# Compression bench results (non-solid only)

Host: Linux, **12** cores (Intel i7-8750H). Date: **2026-07-28**.  
Harness: `bench_compress` (release, default features). **No solid tar streams.**  
Peers: stock `7zz` (LZMA2), `7zz-zstd` (ZSTD/LZ4 methods).  

Post performance work: worker pool, ordered streaming write, LZMA2 dict clamp, zstd pledges.

## Fairness

| Method | Ours | Native (non-solid) |
|--------|------|---------------------|
| lzma2 | non-solid 7z + LZMA2 | **7zz** `-m0=LZMA2 -ms=off` |
| zstd | non-solid 7z + Zstd packs | **7zz-zstd** `-m0=zstd -ms=off` |
| lz4 | non-solid 7z + LZ4 packs | **7zz-zstd** `-m0=lz4 -ms=off` |

**Bold** = better wall time or smaller size in that row pair.

## Scale: tiny (~200 files, ~1.7 MiB)

| method | tool | t | level | sec | out_MiB | ratio | MiB/s |
|--------|------|--:|------:|----:|--------:|------:|------:|
| lzma2 | rsync-archive | 1 | 1 | 0.179 | 0.048 | 34.4 | 9.3 |
| lzma2 | **7zz-LZMA2** | 1 | 1 | **0.141** | **0.037** | **44.6** | 11.7 |
| lzma2 | **rsync-archive** | 4 | 1 | **0.132** | 0.048 | 34.4 | **12.6** |
| lzma2 | 7zz-LZMA2 | 4 | 1 | 0.135 | **0.037** | **44.6** | 12.3 |
| lzma2 | rsync-archive | 1 | 5 | 0.250 | 0.048 | 34.4 | 6.6 |
| lzma2 | **7zz-LZMA2** | 1 | 5 | **0.168** | **0.037** | **44.6** | 9.9 |
| lzma2 | **rsync-archive** | 4 | 5 | **0.089** | 0.048 | 34.4 | **18.7** |
| lzma2 | 7zz-LZMA2 | 4 | 5 | 0.188 | **0.037** | **44.6** | 8.8 |
| zstd | **rsync-archive** | 1 | 1 | **0.019** | 0.044 | 38.1 | **86** |
| zstd | 7zz-zstd | 1 | 1 | 0.067 | **0.031** | **53.0** | 25 |
| zstd | **rsync-archive** | 4 | 1 | **0.025** | 0.044 | 38.1 | **67** |
| zstd | 7zz-zstd | 4 | 1 | 0.074 | **0.031** | **53.0** | 22 |
| zstd | **rsync-archive** | 1 | 5 | **0.033** | 0.044 | 38.0 | **51** |
| zstd | 7zz-zstd | 1 | 5 | 0.067 | **0.031** | **52.8** | 25 |
| zstd | **rsync-archive** | 4 | 5 | **0.018** | 0.044 | 38.0 | **92** |
| zstd | 7zz-zstd | 4 | 5 | 0.074 | **0.031** | **52.8** | 22 |
| lz4 | **rsync-archive** | 1 | 1 | **0.025** | 0.052 | 31.9 | **67** |
| lz4 | 7zz-lz4 | 1 | 1 | 0.093 | **0.042** | **39.5** | 18 |
| lz4 | **rsync-archive** | 4 | 1 | **0.018** | 0.052 | 31.9 | **93** |
| lz4 | 7zz-lz4 | 4 | 1 | 0.096 | **0.044** | **37.4** | 17 |
| lz4 | **rsync-archive** | 1 | 5 | **0.024** | 0.052 | 31.9 | **68** |
| lz4 | 7zz-lz4 | 1 | 5 | 0.109 | **0.041** | **40.1** | 15 |
| lz4 | **rsync-archive** | 4 | 5 | **0.018** | 0.052 | 31.9 | **93** |
| lz4 | 7zz-lz4 | 4 | 5 | 0.118 | **0.044** | **37.9** | 14 |

## Scale: small (~2k files, ~20 MiB)

| method | tool | t | level | sec | out_MiB | ratio | MiB/s |
|--------|------|--:|------:|----:|--------:|------:|------:|
| lzma2 | rsync-archive | 1 | 1 | 0.695 | 0.321 | 60.9 | 28 |
| lzma2 | **7zz-LZMA2** | 1 | 1 | **0.551** | **0.209** | **93.5** | 36 |
| lzma2 | rsync-archive | 4 | 1 | 0.600 | 0.321 | 60.9 | 33 |
| lzma2 | **7zz-LZMA2** | 4 | 1 | **0.553** | **0.209** | **93.5** | 35 |
| lzma2 | rsync-archive | 1 | 5 | 1.742 | 0.321 | 60.9 | 11 |
| lzma2 | **7zz-LZMA2** | 1 | 5 | **1.259** | **0.209** | **93.5** | 16 |
| lzma2 | **rsync-archive** | 4 | 5 | **1.081** | 0.321 | 60.9 | **18** |
| lzma2 | 7zz-LZMA2 | 4 | 5 | 2.319 | **0.209** | **93.5** | 8.4 |
| zstd | **rsync-archive** | 1 | 1 | **0.054** | 0.267 | 73.1 | **359** |
| zstd | 7zz-zstd | 1 | 1 | 0.142 | **0.144** | **136** | 138 |
| zstd | **rsync-archive** | 4 | 1 | **0.025** | 0.267 | 73.1 | **797** |
| zstd | 7zz-zstd | 4 | 1 | 0.127 | **0.144** | **136** | 154 |
| zstd | **rsync-archive** | 1 | 5 | **0.113** | 0.267 | 73.1 | **174** |
| zstd | 7zz-zstd | 1 | 5 | 0.436 | **0.144** | **136** | 45 |
| zstd | **rsync-archive** | 4 | 5 | **0.052** | 0.267 | 73.1 | **374** |
| zstd | 7zz-zstd | 4 | 5 | 0.522 | **0.144** | **136** | 37 |
| lz4 | **rsync-archive** | 1 | 1 | **0.055** | 0.357 | 54.7 | **354** |
| lz4 | 7zz-lz4 | 1 | 1 | 0.682 | **0.257** | **76** | 29 |
| lz4 | **rsync-archive** | 4 | 1 | **0.031** | 0.357 | 54.7 | **623** |
| lz4 | 7zz-lz4 | 4 | 1 | 0.546 | **0.280** | **70** | 36 |
| lz4 | **rsync-archive** | 1 | 5 | **0.056** | 0.357 | 54.7 | **348** |
| lz4 | 7zz-lz4 | 1 | 5 | 0.846 | **0.257** | **76** | 23 |
| lz4 | **rsync-archive** | 4 | 5 | **0.105** | 0.357 | 54.7 | **187** |
| lz4 | 7zz-lz4 | 4 | 5 | 0.845 | **0.280** | **70** | 23 |

## Takeaways

| Goal | Winner |
|------|--------|
| **Wall time** (zstd / lz4, many small files) | **rsync-archive** — often 2–10× faster than 7zz-zstd |
| **Wall time** (lzma2 single-thread) | **7zz** still ahead (~1.2–1.4×) |
| **Wall time** (lzma2 multi-thread, small L5) | **rsync-archive** can beat 7zz on this host |
| **Archive size / ratio** | **Native** always denser (~1.3–1.9× smaller output) |

Default product pick for speed: **`--method zstd`**. Best ratio we offer: **`--method lzma2`** (still behind stock 7zz). Optional denser codecs: `cargo build --features native-codecs`.

Re-run:

```bash
cargo build --release --bin rsync-archive --bin bench_compress
./target/release/bench_compress run --scale tiny --threads 1,4 --level 1,5 --methods all
./target/release/bench_compress run --scale small --threads 1,4 --level 1,5 --methods all
```

See [`docs/BENCH.md`](../BENCH.md).
