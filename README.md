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
| **`create`** | Rsync-style include/exclude / files-from → **stream-read** sources → **stream-compress** → **non-solid** `.7z` (default), **seekable-zstd**, **tar.zst**, or **tar.lz4** |
| **`embed`** | Take multiple finished files (typically `.7z`) → master/outer **non-solid store** 7z (Copy / method `0x00`), same idea as [archiveconverter](https://github.com/hilather/archiveconverter) outer append |

**Create compresses; embed only stores.** Embed does **not** convert solid→non-solid (use archiveconverter for that).

### Design principles

- **Non-solid only** for create (no solid 7z).
- **Disk-friendly:** no full tree copy to temp; stream read / stream compress / append packs.
- **Rsync-style selection** for targeting, with `--dry-run` for testing filters.
- **Safe output:** error if `-o` exists unless `--force`; write `OUT.partial` then rename (when pipelines land).
- **Formats:** default **7z** non-solid; optional **seekable-zstd**, **tar-zstd**, **tar-lz4** for RA-friendly streams ([`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md), [`docs/FORMAT_TAR_ZSTD.md`](docs/FORMAT_TAR_ZSTD.md), [`docs/FORMAT_TAR_LZ4.md`](docs/FORMAT_TAR_LZ4.md)).

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
# RA-friendly tar payloads:
cargo run -- create -o out.tar.zst --format tar-zstd ./src/
cargo run -- create -o out.tar.lz4 --format tar-lz4 ./src/  # .tar.lz4 / .tlz4

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
rsync-archive create -o o.7z --files-from master.txt --file-size-from sizes.txt --dir-max-size-from dirs.txt
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

**`--format seekable-zstd`:** single Zstd **seekable** stream (zeekstd) with length-prefixed members + trailer index for name → uncompressed offset. Infer from bare `-o *.zst` when `--format` omitted. `--method` is 7z-only. Layout: [`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md).

**`--format tar-zstd`:** valid **ustar/pax tar** payload inside seekable Zstd + `RATAIDX1` member index (path/size/mtime/**mode/uid/gid**; **uname/gname** in tar headers; RA extract). Includes **parent directory members** (`typeflag='5'`, trailing `/`), **symbolic links** (`typeflag='2'`), and **hard links** (`typeflag='1'`, linkname = first archive path for the inode; Unix detection; size 0) from the selection walk. Infer from `-o *.tar.zst` or `*.tzst`. Same selection/restrictions as other formats (7z / seekable-zstd skip symlinks and hard-link members, keeping the first file body). Layout: [`docs/FORMAT_TAR_ZSTD.md`](docs/FORMAT_TAR_ZSTD.md).

**`--format tar-lz4`:** same tar + `RATAIDX1` idea with **independent LZ4 frames** + cleartext `RATLFRM1` frame table (no standard seekable-LZ4); same metadata including uname/gname in headers, parent directory members, **symlinks**, and **hard links**. Infer from `-o *.tar.lz4` or `*.tlz4`. Layout: [`docs/FORMAT_TAR_LZ4.md`](docs/FORMAT_TAR_LZ4.md).

Default create remains **non-solid 7z** with **`--method lzma2`**.

**Trailing `/` on SRC** strips the directory name from archive paths (`photos/` → `a.jpg`; `photos` → `photos/a.jpg`).  
**`--files-from`:** exclusive of `SRC...`; relative lines keep path as member name; absolute lines use basename.  
**`--include-cwd`:** (off by default) also pack files under the process CWD at **archive root** (like a trailing `/` on `.`); skips the `-o` output and its `.partial` temp. May be used alone or with `SRC...` / `--files-from`.  

**Filters:** see [`docs/SELECTION.md`](docs/SELECTION.md) and [`docs/RSYNC_PARITY.md`](docs/RSYNC_PARITY.md). Rule build order: `include-from` → `exclude-from` → **`filter-from`** → `--filter` → `--include` → `--exclude`. Prefer **`--filter-from`** / `--filter` for ordered mixes (`--include`/`--exclude` are batched, not CLI-interleaved).  
**Restriction list files** (only matching paths/prefixes; others ignore that list):  
- **`--file-size-from`:** rsync-like `PATTERN max=SIZE` (no min; first match wins).  
- **`--dir-max-size-from`:** `DIR/ max=SIZE` and optional `files=N`, or legacy `DIR/=SIZE`.  
- **`--dir-max-files-from`:** `PATH=N` or `PATH/ files=N`.  
**`--dir-max-size PATH=SIZE`:** CLI form of dir byte budget (**recursive**; newest mtime first; longest prefix).  
**`--dir-max-files PATH=N`:** CLI form of dir file-count cap (**recursive**).  
**Global caps:** `--max-total-size` / `--max-files` (after dir limits); global `--max-size` / `--min-size` / `--newer-than` apply to all candidates when set.

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
| `--format` / `--output-format` | infer / `7z` | `7z` · `seekable-zstd` (`.zst`) · `tar-zstd` (`.tar.zst` / `.tzst`) · `tar-lz4` (`.tar.lz4` / `.tlz4`) |
| `-n`, `--dry-run` | off | List selection only |
| `--force` | off | Overwrite existing `-o` |
| `--exclude` / `--include` | — | Rsync-style patterns (repeatable; include batch then exclude batch) |
| `--exclude-from` / `--include-from` | — | Pattern files (repeatable) |
| `--filter-from` | — | Ordered `+/-` filter file (repeatable; preferred for full rule lists) |
| `--files-from` | — | Master collect list (exclusive of `SRC...`) |
| `--include-cwd` | off | Pack CWD files at archive root; skip `-o` / `.partial` |
| `--file-size-from` | — | Per-path max size list (`PATTERN max=SIZE`; only matches) |
| `--dir-max-size-from` | — | Dir size/count list (`DIR/ max=SIZE [files=N]`) |
| `--filter` | — | `+ pattern` / `- pattern` (repeatable; CLI order among filters) |
| `--level` | `5` | Level 0–9 (LZMA2 preset / mapped Zstd; LZ4 1–2 fast, ≥3 HC with `--features lz4-hc`) |
| `--method` | `lzma2` | **7z only:** `lzma2` · `zstd` · `lz4` (non-solid per-file packs) |
| `--verify` | off | Post-write: non-solid + member count + sample extract (7z); index/member check (seekable-zstd / tar-zstd / tar-lz4) |
| `--threads` | auto | **7z only:** encode workers (omit = auto: many tiny files → 1, else CPUs) |
| `--encode-concurrency` | `0` | **7z only:** max concurrent encodes (`0` = auto from threads) |
| `--encode-size-budget` | `500M` | **7z only:** max in-flight uncompressed size (`0` = unlimited) |
| `--dir-max-size` | — | Cap selected bytes under archive-relative dir (`PATH=SIZE`, repeatable; listed dirs only) |
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
| RA-friendly tar.zst | **Done** — `--format tar-zstd` / `*.tar.zst` / `*.tzst` ([`docs/FORMAT_TAR_ZSTD.md`](docs/FORMAT_TAR_ZSTD.md)) |
| RA-friendly tar.lz4 | **Done** — `--format tar-lz4` / `*.tar.lz4` / `*.tlz4` ([`docs/FORMAT_TAR_LZ4.md`](docs/FORMAT_TAR_LZ4.md)) |
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
  archive/tar_zstd/       # RA tar.zst (seekable Zstd + RATAIDX1)
  archive/tar_lz4/        # RA tar.lz4 (multi-frame LZ4 + RATLFRM1 + RATAIDX1)
  archive/tar_common.rs   # shared ustar/pax + RATAIDX1 helpers
  select/                 # SourceSpec, pathnorm, rules, matcher, from_file, walk, dir_budget, global_restrict
  pipeline/output.rs      # partial path, --force check, rename helpers
  pipeline/create.rs      # create selection + 7z / seekable-zstd / tar.* write
  pipeline/embed.rs       # embed command (store outer)
  util/                   # tracing init
docs/
  DESIGN.md               # full design
  SELECTION.md            # filter semantics (Stage 4, frozen v1)
  FORMAT_SEEKABLE_ZSTD.md # seekable-zstd on-disk layout
  FORMAT_TAR_ZSTD.md      # tar.zst layout
  FORMAT_TAR_LZ4.md       # tar.lz4 layout
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
- `--include` / `--exclude` / `--filter '+|-'` / `--filter-from` / include-from / exclude-from (repeatable from-files)
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
| **Release** (`.github/workflows/release.yml`) | tag `v*` | Per-distro `.tar.gz` assets; each contains binary **`rsync-archive`** (no OS suffix) |

```bash
# Cut a release (maintainers)
git tag -a v0.5.1 -m "v0.5.1"
git push origin v0.5.1

# Install from a release asset
tar -xzf rsync-archive-rocky8-x86_64.tar.gz
./rsync-archive --version
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
