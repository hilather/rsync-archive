# Compression benchmarks (non-solid only)

Compare **rsync-archive** to native tools using **only non-solid multi-member** archives.

Solid `tar|zstd` / `tar|lz4` streams are **not** used — they are a different product (no per-file random access).

## Fairness matrix

| Method | Our tool | Native baseline | Matched knobs |
|--------|----------|-----------------|---------------|
| **lzma2** | `create --method lzma2` non-solid `.7z` | `7zz a -m0=LZMA2 -ms=off -mmt=N -mx=L` | threads, level, non-solid 7z |
| **zstd** | `create --method zstd` non-solid `.7z` | Per-file `zstd -T1 -#` (≤N workers) + `7zz a -m0=Copy -ms=off` | workers, mapped zstd level, non-solid multi-member |
| **lz4** | `create --method lz4` non-solid `.7z` | Per-file `lz4 -#` (≤N workers) + `7zz a -m0=Copy -ms=off` | workers, level, non-solid multi-member |

Stock **7zz does not encode ZSTD/LZ4 methods** inside `.7z`. The zstd/lz4 native path is therefore a **layout-equivalent proxy** (independent compressed streams + non-solid store outer), not bit-identical 7z method IDs.

Default create method remains **`lzma2`**.

### Level mapping (zstd)

Our `--level 0..9` → CLI zstd: 1,1,2,3,5,7,9,12,15,19.

## Requirements

```bash
cargo build --release --bin rsync-archive --bin bench_compress
# PATH: 7zz (or 7z), zstd, lz4
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
