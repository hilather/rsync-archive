# Compression bench results

Host: Linux laptop, **12** threads available. Date: **2026-07-28**.  
Harness: `bench_compress` · binaries release-built · fixtures under `benchdata/` (gitignored).

## Fairness

| Method | Ours | Native | Same |
|--------|------|--------|------|
| lzma2 | non-solid `.7z` | `7zz` non-solid LZMA2 `-ms=off -mmt=N -mx=L` | container + codec + threads + level |
| zstd | non-solid `.7z`+Zstd packs | `tar \| zstd -T N` solid `.tar.zst` | threads + mapped level only |
| lz4 | non-solid `.7z`+LZ4 | `tar \| lz4` solid stream | level; lz4 CLI usually single-thread |

**Important:** zstd/lz4 native baselines are **solid streams** (better ratio, no per-file random access). Ours always pays for **non-solid multi-member** layout (headers + independent packs).

## Scale: tiny (~200 files, ~1.7 MiB compressible text)

| method | tool | threads | level | sec | out_MiB | ratio | MiB/s |
|--------|------|--------:|------:|----:|--------:|------:|------:|
| lzma2 | **rsync-archive** | 1 | 1 | 0.131 | 0.048 | 34.4 | 12.7 |
| lzma2 | 7zz-LZMA2 | 1 | 1 | 0.081 | 0.037 | 44.6 | 20.4 |
| lzma2 | **rsync-archive** | 4 | 1 | **0.044** | 0.048 | 34.4 | **37.8** |
| lzma2 | 7zz-LZMA2 | 4 | 1 | 0.077 | 0.037 | 44.6 | 21.6 |
| lzma2 | rsync-archive | 1 | 5 | 0.384 | 0.048 | 34.4 | 4.3 |
| lzma2 | **7zz-LZMA2** | 1 | 5 | **0.136** | **0.037** | **44.6** | 12.2 |
| lzma2 | rsync-archive | 4 | 5 | 0.213 | 0.048 | 34.4 | 7.8 |
| lzma2 | **7zz-LZMA2** | 4 | 5 | **0.111** | **0.037** | **44.6** | 15.0 |
| zstd | rsync-archive | 1 | 1 | 0.013 | 0.044 | 37.6 | 132 |
| zstd | tar\|zstd (solid) | 1 | 1 | 0.011 | 0.015 | 109 | 152 |
| zstd | rsync-archive | 4 | 1 | 0.031 | 0.044 | 37.6 | 54 |
| zstd | tar\|zstd (solid) | 4 | 1 | 0.009 | 0.015 | 109 | 175 |
| zstd | rsync-archive | 1 | 5 | 0.077 | 0.044 | 37.5 | 21 |
| zstd | tar\|zstd (solid) | 1 | 5 | 0.017 | 0.015 | 112 | 95 |
| lz4 | rsync-archive | 1 | 1 | 0.015 | 0.052 | 31.9 | 110 |
| lz4 | tar\|lz4 (solid) | 1 | 1 | 0.010 | 0.024 | 69.6 | 162 |
| lz4 | rsync-archive | 1 | 5 | 0.016 | 0.052 | 31.9 | 106 |
| lz4 | tar\|lz4 (solid) | 1 | 5 | 0.016 | 0.022 | 74.5 | 103 |

## Scale: small (~2k files, ~20 MiB)

| method | tool | threads | level | sec | out_MiB | ratio | MiB/s |
|--------|------|--------:|------:|----:|--------:|------:|------:|
| lzma2 | rsync-archive | 1 | 1 | 0.715 | 0.321 | 60.9 | 27.3 |
| lzma2 | **7zz-LZMA2** | 1 | 1 | **0.517** | **0.209** | **93.5** | 37.7 |
| lzma2 | **rsync-archive** | 4 | 1 | **0.287** | 0.321 | 60.9 | **68.0** |
| lzma2 | 7zz-LZMA2 | 4 | 1 | 0.573 | 0.209 | 93.5 | 34.1 |
| lzma2 | rsync-archive | 1 | 5 | 2.739 | 0.321 | 60.9 | 7.1 |
| lzma2 | **7zz-LZMA2** | 1 | 5 | **1.123** | **0.209** | **93.5** | 17.4 |
| lzma2 | rsync-archive | 4 | 5 | 1.600 | 0.321 | 60.9 | 12.2 |
| lzma2 | **7zz-LZMA2** | 4 | 5 | **0.653** | **0.209** | **93.5** | 29.9 |
| zstd | rsync-archive | 1 | 1 | 0.070 | 0.273 | 71.5 | 277 |
| zstd | tar\|zstd (solid) | 1 | 1 | 0.037 | 0.023 | 859 | 523 |
| zstd | rsync-archive | 4 | 1 | 0.193 | 0.273 | 71.5 | 102 |
| zstd | tar\|zstd (solid) | 4 | 1 | 0.034 | 0.023 | 859 | 573 |
| zstd | rsync-archive | 1 | 5 | 0.387 | 0.273 | 71.5 | 50 |
| zstd | tar\|zstd (solid) | 1 | 5 | 0.051 | 0.018 | 1091 | 383 |
| lz4 | rsync-archive | 1 | 1 | 0.100 | 0.357 | 54.7 | 195 |
| lz4 | tar\|lz4 (solid) | 1 | 1 | 0.045 | 0.112 | 174 | 430 |
| lz4 | rsync-archive | 1 | 5 | 0.099 | 0.357 | 54.7 | 197 |
| lz4 | tar\|lz4 (solid) | 1 | 5 | 0.094 | 0.100 | 195 | 207 |

## Takeaways

1. **lzma2 vs 7zz (fair non-solid 7z):** Official **7zz is smaller and often faster at level 5**; at **level 1 with 4 threads**, **rsync-archive can win on wall time** (parallel file encode). Ratio gap ~1.3–1.5× in 7zz’s favor (better LZMA2 + packing).
2. **Parallelism helps us on multi-file trees** more than 7zz at low level (our encode workers vs 7zz `-mmt` on small files).
3. **zstd/lz4 solid native streams crush size** (one solid stream of repeated text). Our larger outputs buy **per-file random access** in 7z.
4. **Within our tool**, prefer **`--method zstd`** for speed; **lzma2** for size; **lz4** when encode latency matters most.

## Reproduce

```bash
cargo build --release --bin rsync-archive --bin bench_compress
export PATH="$HOME/.local/bin:$PATH"   # 7zz
./target/release/bench_compress run --scale small --threads 1,4 --level 1,5 --methods all
```

See also [`docs/BENCH.md`](../BENCH.md).
