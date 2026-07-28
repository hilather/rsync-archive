# Compression benchmarks

Compare **rsync-archive** to native tools under matched threads and levels.

## Fairness matrix

| Method | Our tool | Native baseline | Matched knobs | Caveat |
|--------|----------|-----------------|---------------|--------|
| **lzma2** | `create --method lzma2` → non-solid `.7z` | `7zz a -t7z -m0=LZMA2 -ms=off -mmt=N -mx=L` | threads, level, non-solid | Closest apples-to-apples |
| **zstd** | `create --method zstd` → non-solid `.7z` | `tar \| zstd -T N -L` → `.tar.zst` | threads, mapped zstd level | Native is **solid stream** (no per-file random access) |
| **lz4** | `create --method lz4` → non-solid `.7z` | `tar \| lz4 -L` → `.tar.lz4` | level; threads often N/A for lz4 CLI | Native solid stream; classic `lz4` is typically single-thread |

Default create method remains **`lzma2`**.

### Level mapping (zstd)

Our `--level 0..9` → CLI zstd levels: 1,1,2,3,5,7,9,12,15,19 (same as `codec::zstd_level`).

## Requirements

```bash
cargo build --release --bin rsync-archive --bin bench_compress
# PATH: 7zz (or 7z), zstd, lz4, tar
```

## Run

```bash
# Quick (~seconds–minutes)
./target/release/bench_compress run --scale tiny --threads 1,4 --level 1 --methods all

# Standard
./target/release/bench_compress run --scale small --threads 1,4,8 --level 1,5 --methods all

# Heavier
./target/release/bench_compress run --scale medium --threads 1,4,12 --level 1,5 --methods lzma2,zstd
```

Fixtures and outputs land in `benchdata/` (**gitignored**). Publish tables under `docs/bench/` when sharing results.

## Scales

| Scale | Files | ~bytes/file | ~total |
|-------|------:|------------:|-------:|
| tiny | 200 | 8 KiB | ~1.6 MiB |
| small | 2 000 | 10 KiB | ~20 MiB |
| medium | 10 000 | 10 KiB | ~100 MiB |

Content is highly compressible repeated text (stresses entropy coding, not incompressible noise).
