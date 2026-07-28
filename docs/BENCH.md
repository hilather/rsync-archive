# Compression benchmarks (non-solid only)

Compare **rsync-archive** to native tools using **only non-solid multi-member** archives.

Solid `tar|zstd` / `tar|lz4` streams are **not** used — they are a different product (no per-file random access).

## Fairness matrix

| Method | Our tool | Native baseline | Matched knobs |
|--------|----------|-----------------|---------------|
| **lzma2** | `create --method lzma2` non-solid `.7z` | Stock `7zz a -m0=LZMA2 -ms=off -mmt=N -mx=L` | threads, level, non-solid 7z |
| **zstd** | `create --method zstd` non-solid `.7z` | **Preferred:** [7-Zip-zstd](https://github.com/mcmilk/7-Zip-zstd) `7zz-zstd a -m0=zstd -ms=off -mmt=N -mx=L` | threads, mapped zstd level, non-solid 7z + ZSTD method |
| **lz4** | `create --method lz4` non-solid `.7z` | **Preferred:** `7zz-zstd a -m0=lz4 -ms=off -mmt=N -mx=L` | threads, level, non-solid 7z + LZ4 method |

Stock mainline **7zz does not encode** ZSTD/LZ4 methods inside `.7z` (it can only handle bare `.zst` files). Fair zstd/lz4 peers require the **7-Zip-zstd** fork (method IDs `4F71101` / `4F71104`).

**Fallback** (no `7zz-zstd`): per-file `zstd`/`lz4` CLI (≤N workers) + `7zz a -m0=Copy -ms=off`. Layout-ish only — slower due to process spawn; not preferred for published numbers.

Default create method remains **`lzma2`**.

### Level mapping (zstd)

Our `--level 0..9` → zstd / 7zz-zstd `-mx`: 1,1,2,3,5,7,9,12,15,19.

## Requirements

```bash
cargo build --release --bin rsync-archive --bin bench_compress
# PATH:
#   7zz (or 7z)              — LZMA2 baseline
#   7zz-zstd                 — preferred zstd/lz4 baseline (7-Zip-zstd)
#   zstd, lz4                — only if 7zz-zstd missing (proxy fallback)
```

### Install 7-Zip-zstd (once)

```bash
# Linux x86_64 example (installs as 7zz-zstd, keeps stock 7zz for LZMA2)
curl -fsSL -o /tmp/7zs.zip \
  https://github.com/mcmilk/7-Zip-zstd/releases/download/v26.02-v1.5.7-R2/linux-gcc-x64.zip
unzip -d /tmp/7zs /tmp/7zs.zip
install -m755 /tmp/7zs/7zz ~/.local/bin/7zz-zstd
7zz-zstd i | grep -E '4F71101|4F71104'   # expect ZSTD + LZ4 encode codecs
```

## Run

```bash
./target/release/bench_compress run --scale tiny --threads 1,4 --level 1,5 --methods all
./target/release/bench_compress run --scale small --threads 1,4 --level 1,5 --methods all
```

Fixtures/outputs: `benchdata/` (gitignored). Published tables: [`docs/bench/RESULTS.md`](bench/RESULTS.md).

## Scales

| Scale | Files | ~bytes/file | ~total |
|-------|------:|------------:|-------:|
| tiny | 200 | 8 KiB | ~1.6 MiB |
| small | 2 000 | 10 KiB | ~20 MiB |
| medium | 10 000 | 10 KiB | ~100 MiB |
