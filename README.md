# rsync-archive

**Stream-create non-solid 7z** archives from the filesystem using **rsync-style path selection**, and **embed** finished archives under a master store 7z (Copy method, no recompress).

[![CI](https://github.com/hilather/rsync-archive/actions/workflows/ci.yml/badge.svg)](https://github.com/hilather/rsync-archive/actions/workflows/ci.yml)
[![Release](https://github.com/hilather/rsync-archive/actions/workflows/release.yml/badge.svg)](https://github.com/hilather/rsync-archive/actions/workflows/release.yml)

| | |
|--|--|
| **Status** | Stages 2–6 done; **`create` and `embed` work** (non-solid 7z) |
| **Selection** | [`docs/SELECTION.md`](docs/SELECTION.md) (rsync include/exclude v1) |
| **License** | MIT |
| **Design** | [`docs/DESIGN.md`](docs/DESIGN.md) |
| **Agent policy** | [`AGENTS.md`](AGENTS.md) (docs **and** tests required on every change) |

---

## What it does

| Command | Role |
|---------|------|
| **`create`** | Rsync-style include/exclude / files-from → **stream-read** sources → **stream-compress** → **non-solid** `.7z` (default) or **seekable-zstd** `.zst` |
| **`embed`** | Take multiple finished files (typically `.7z`) → master/outer **non-solid store** 7z (Copy / method `0x00`), same idea as [archiveconverter](https://github.com/hilather/archiveconverter) outer append |

**Create compresses; embed only stores.** Embed does **not** convert solid→non-solid (use archiveconverter for that).

### Design principles

- **Non-solid only** for create (no solid 7z).
- **Disk-friendly:** no full tree copy to temp; stream read / stream compress / append packs.
- **Rsync-style selection** for targeting, with `--dry-run` for testing filters.
- **Safe output:** error if `-o` exists unless `--force`; write `OUT.partial` then rename (when pipelines land).
- **Formats:** default **7z** non-solid; optional **seekable-zstd** for byte-range streams ([`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md)).

---

## Requirements

- **Rust** 1.70+ (edition 2021)
- Optional later: `7zz` for compare tests

## Build

```bash
cargo build --release
# binary: target/release/rsync-archive
```

## Quick start (current)

```bash
# Inspect CLI (create / embed flags are registered; bodies not implemented yet)
cargo run -- --help
cargo run -- create --help
cargo run -- embed --help

# create non-solid LZMA2 7z (stream compress):
cargo run -- create -o out.7z --exclude '*.tmp' --level 5 --verify ./src/
cargo run -- create -o out.7z -n --exclude '*.tmp' ./src/   # dry-run
cargo run -- create -o out.7z --files-from list.txt

# create seekable-zstd stream (byte-range access; infer from .zst or --format):
cargo run -- create -o out.zst --format seekable-zstd --level 5 ./src/
cargo run -- create -o pack.zst --level 3 ./data/           # .zst → seekable-zstd

# embed finished files under a master store 7z:
cargo run -- embed -o master.7z --allow-any a.bin b.bin
cargo run -- embed -o master.7z --force --verify nest1.7z nest2.7z
```

### `create` (Stages 5–6)

```bash
rsync-archive create -o game.7z \
  --exclude '*.tmp' \
  --exclude-from excludes.txt \
  --filter '- cache/**' \
  --level 5 --verify \
  /data/game/

rsync-archive create -o out.7z --files-from list.txt --dry-run
rsync-archive create -o out.7z --force --level 1 /data/tree
rsync-archive create -o o.7z --dir-max-size logs/=100M --dir-max-size cache=50M tree/
rsync-archive create -o o.7z --dir-max-files logs/=10 --dir-max-files-from limits.txt tree/
rsync-archive create -o logs.7z --max-total-size 500M --max-files 1000 --max-size 50M --newer-than 7d /var/log/
```

**Write model (default `--format 7z`):** non-solid 7z, **per-file packs** (file-level random access):

| `--method` | Codec | Notes |
|------------|--------|--------|
| `lzma2` (default) | LZMA2 `0x21` | Best ratio, slower |
| `zstd` | Zstd `04 F7 11 01` | Best speed×ratio (`zstd`/libzstd); independent frames per member |
| `lz4` | LZ4 `04 F7 11 04` | Fastest |

Empty files use empty flags; mtime from source; `OUT.partial` → rename. Parallel encode: fixed worker pool + **500M** in-flight size budget.  
Optional denser codecs: `cargo build --features native-codecs` (`liblzma` LZMA2 + `lz4-hc` for lz4 levels ≥3).

**`--format seekable-zstd`:** single Zstd **seekable** stream (zeekstd) with length-prefixed members + trailer index for name → uncompressed offset. Infer from `-o *.zst` when `--format` omitted. `--method` is 7z-only (error if non-default with seekable-zstd). Layout: [`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md).

Default create remains **non-solid 7z** with **`--method lzma2`**.

**Trailing `/` on SRC** strips the directory name from archive paths (`photos/` → `a.jpg`; `photos` → `photos/a.jpg`).  
**`--files-from`:** exclusive of `SRC...`; relative lines keep path as member name; absolute lines use basename.  
**Filters:** see [`docs/SELECTION.md`](docs/SELECTION.md). Rule build order: include-from → exclude-from → `--filter` → `--include` → `--exclude` (use `--filter` for strict interleaving).  
**`--dir-max-size PATH=SIZE`:** after filters (and per-file size/age), cap total selected bytes under an archive-relative directory (**recursive**; newest mtime first; nested → longest prefix).  
**`--dir-max-files PATH=N`** / **`--dir-max-files-from`:** cap file count under a directory tree (**recursive**; newest mtime first; longest prefix wins).  
**Global / log-collection caps** (after dir limits): `--max-total-size SIZE`, `--max-files N` (newest-first); per-file `--max-size` / `--min-size` / `--newer-than DURATION` (`7d`, `24h`, …) apply before dir budgets.

### `embed` (Stage 3 — implemented)

```bash
rsync-archive embed -o master.7z nest1.7z nest2.7z
rsync-archive embed -o master.7z --keep-path --prefix packs/ ./build/a.7z
rsync-archive embed -o master.7z --require-7z --verify a.7z b.7z
rsync-archive embed -o master.7z --allow-any --dry-run blob.bin
```

Default naming flattens to **basename**. Missing 7z magic **warns** (stderr log) unless `--require-7z` (error) or `--allow-any` (silent). Write uses `OUT.partial` then rename; refuse overwrite without `--force`.

---

## CLI overview (flags registered)

### Global

| Flag | Meaning |
|------|---------|
| `-v` / `-vv` | Debug / trace logging on stderr (`info` → `debug` → `trace`; `RUST_LOG` overrides) |

### `create`

| Flag | Default | Meaning |
|------|---------|---------|
| `-o`, `--output` | required | Output path (`.7z` or `.zst`) |
| `--format` / `--output-format` | infer / `7z` | `7z` · `seekable-zstd` (`.zst` extension → seekable-zstd) |
| `-n`, `--dry-run` | off | List selection only |
| `--force` | off | Overwrite existing `-o` |
| `--exclude` / `--include` | — | Rsync-style patterns (repeatable) |
| `--exclude-from` / `--include-from` | — | Pattern files |
| `--files-from` | — | Explicit file list (exclusive of `SRC...`) |
| `--filter` | — | `+ pattern` / `- pattern` (repeatable) |
| `--level` | `5` | Level 0–9 (LZMA2 preset / mapped Zstd; LZ4 1–2 fast, ≥3 HC with `--features lz4-hc`) |
| `--method` | `lzma2` | **7z only:** `lzma2` · `zstd` · `lz4` (non-solid per-file packs) |
| `--verify` | off | Post-write: non-solid + member count + sample extract (7z); index check (seekable-zstd) |
| `--threads` | auto | **7z only:** encode workers (omit = auto: many tiny files → 1, else CPUs) |
| `--encode-concurrency` | `0` | **7z only:** max concurrent encodes (`0` = auto from threads) |
| `--encode-size-budget` | `500M` | **7z only:** max in-flight uncompressed size (`0` = unlimited) |
| `--dir-max-size` | — | Cap selected bytes under archive-relative dir (`PATH=SIZE`, repeatable; recursive; newest-first) |
| `--dir-max-files` | — | Cap file count under dir tree (`PATH=N`, recursive; newest-first; longest prefix) |
| `--dir-max-files-from` | — | File of `PATH=N` lines (same as `--dir-max-files`) |
| `--max-total-size` | — | Global selected-byte cap (newest-mtime-first fill) |
| `--max-files` | — | Global max selected file count (newest-mtime-first) |
| `--max-size` | — | Skip any single file larger than SIZE |
| `--min-size` | — | Skip files smaller than SIZE (`0` = off) |
| `--newer-than` | — | Only files with mtime within last DURATION (`7d`, `24h`, `30m`, `90s`) |
| `SRC...` | — | Sources (required unless `--files-from`) |

### `embed`

| Flag | Default | Meaning |
|------|---------|---------|
| `-o`, `--output` | required | Master `.7z` |
| `-n`, `--dry-run` | off | List members only |
| `--force` | off | Overwrite existing `-o` |
| `--prefix` | — | Prefix for member names |
| `--keep-path` | off | Keep path as name (default: basename) |
| `--require-7z` | off | Fail if missing 7z magic |
| `--allow-any` | off | Allow non-7z store blobs |
| `--verify` | off | Post-write test |
| `FILE...` | required | Inputs to embed |

**Exit codes:** `0` success · `1` operational error · `2` usage error

---

## Implementation status

See [`docs/DESIGN.md`](docs/DESIGN.md) for stages and PR plan.

| Stage | Status |
|-------|--------|
| 0 Bootstrap + agent docs/tests policy | **Done** |
| 1 Foundations (errors, pathnorm, output helpers) | **Done** |
| 2 7z header + store writer | **Done** (library: `NonsolidStoreWriter` Copy / method `0x00`; embed foundation) |
| 3 Embed pipeline | **Done** — `rsync-archive embed` (store/Copy, atomic partial, dry-run, verify) |
| 4 Rsync filter engine | **Done** — see [`docs/SELECTION.md`](docs/SELECTION.md) |
| 5 Walk + create dry-run | **Done** — `create -n` / selection / files-from / prune |
| 6 Create LZMA2 write | **Done** — non-solid LZMA2 create + verify |
| 6b Streaming large-file e2e | **Done** — `tests/e2e_large_file.rs` (64 MiB stream roundtrip + soft RSS) |
| 7 Verify + acceptance | **Done** — non-solid + member count + sample extract; `scripts/v1_acceptance.sh` |
| 8 Parallel encode (AC-style threads) | **Done** — worker pool + ordered streaming write; `--threads` / `--encode-concurrency` / `--encode-size-budget 500M` |
| Codec: Zstd + LZ4 in 7z | **Done** — `--method zstd|lz4` (file-level RA); optional `--features native-codecs` |
| True seekable-zstd stream (zeekstd) | **Done** — `--format seekable-zstd` / `-o *.zst` ([`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md)) |
| 9 Directory size budgets (newest-first) | **Done** — `--dir-max-size PATH=SIZE` (recursive; longest prefix) |
| Directory file-count limits (newest-first) | **Done** — `--dir-max-files PATH=N` / from-file (**recursive** tree scope) |
| Global log-collection limits | **Done** — `--max-total-size` / `--max-files` / `--max-size` / `--min-size` / `--newer-than` |
| Encode performance (OPT-01..14) | **Done** — pool, streaming write, dict clamp, zstd pledges; optional liblzma/lz4-hc |
---

## Project layout

```text
src/
  main.rs, cli.rs, lib.rs, error.rs
  archive/sevenz/         # non-solid header + store + create writers
  archive/seekable_zstd/  # seekable-zstd create + list/extract helpers
  select/                 # SourceSpec, pathnorm, rules, matcher, from_file, walk, dir_budget, global_restrict
  pipeline/output.rs      # partial path, --force check, rename helpers
  pipeline/create.rs      # create selection + 7z / seekable-zstd write
  pipeline/embed.rs       # embed command (store outer)
  util/                   # tracing init
docs/
  DESIGN.md               # full design
  SELECTION.md            # filter semantics (Stage 4, frozen v1)
  FORMAT_SEEKABLE_ZSTD.md # seekable-zstd on-disk layout
AGENTS.md                 # mandatory agent policy (docs + tests)
.grok/skills/
  keep-docs-current/
  keep-tests-current/
tests/                    # cli_smoke, e2e_create, e2e_create_dry_run, e2e_large_file,
                          # e2e_seekable_zstd, e2e_embed, filter_parity
```

### Selection (Stage 4+)

Rsync-style include/exclude + walk + post-filter restrictions:

- Ordered first-match-wins; default **Include** if no rule matches
- `--include` / `--exclude` / `--filter '+|-'` / include-from / exclude-from
- Basename match when pattern has no `/` (`*.tmp` matches `dir/a.tmp`) — K27
- Anchored `/pat`, dir-only `pat/`, wildcards `*` `**` `?`
- Dir prune during walk; filter files capped at **10 MiB** / **1M lines**
- Dir + global size/count/age limits: see [`docs/SELECTION.md`](docs/SELECTION.md)

Details and parity table: [`docs/SELECTION.md`](docs/SELECTION.md).

---

## Benchmarks

Fair comparisons vs native tools (matched threads/levels where possible):

```bash
cargo build --release --bin rsync-archive --bin bench_compress
./target/release/bench_compress run --scale small --threads 1,4 --level 1,5 --methods all
```

Docs: [`docs/BENCH.md`](docs/BENCH.md) · published numbers: [`docs/bench/RESULTS.md`](docs/bench/RESULTS.md)

---

## CI and releases

| Workflow | When | What |
|----------|------|------|
| **CI** (`.github/workflows/ci.yml`) | push/PR to `main` | **Build** on Ubuntu **22.04** / **24.04** and Rocky Linux **8** / **9** / **10**; full `cargo test` on Ubuntu 24.04 |
| **Release** (`.github/workflows/release.yml`) | tag `v*` | Release binaries per distro + GitHub Release assets |

```bash
# Cut a release (maintainers)
git tag -a v0.1.0 -m "v0.1.0"
git push origin v0.1.0
```

Rocky builds run in official `rockylinux/rockylinux` containers. Ubuntu uses GitHub-hosted runners.

---

## Contributing / agents

**Every behavior-changing change must update docs and include regression tests** in the same PR. See:

- [`AGENTS.md`](AGENTS.md)
- `.grok/skills/keep-docs-current/SKILL.md`
- `.grok/skills/keep-tests-current/SKILL.md`

```bash
cargo test
cargo run -- --help
```

---

## License

MIT
