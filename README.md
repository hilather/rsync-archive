<div align="center">

# rsync-archive

### Stream-create random-access archives with rsync-grade path selection

**Select** like rsync · **Compress** like 7-Zip · **Access** like a seekable stream

[![CI](https://github.com/hilather/rsync-archive/actions/workflows/ci.yml/badge.svg)](https://github.com/hilather/rsync-archive/actions/workflows/ci.yml)
[![Release](https://github.com/hilather/rsync-archive/actions/workflows/release.yml/badge.svg)](https://github.com/hilather/rsync-archive/actions/workflows/release.yml)
[![Version](https://img.shields.io/github/v/release/hilather/rsync-archive?label=version)](https://github.com/hilather/rsync-archive/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.70%2B-orange.svg)](https://www.rust-lang.org/)

```bash
rsync-archive create -o game.7z --method zstd --exclude '*.tmp' --level 5 /data/game/
rsync-archive create -o logs.tar.zst --max-total-size 500M --newer-than 7d /var/log/
rsync-archive embed  -o master.7z nest1.7z nest2.7z
```

[Install](#installation) · [Quick start](#quick-start) · [Features](#features) · [CLI reference](#cli-reference) · [Formats](#output-formats) · [Selection](#selection--filters) · [Docs](#documentation)

</div>

---

## Why rsync-archive?

Packing large trees for backup, distribution, or incident response usually forces a trade-off:

| Traditional approach | Pain |
|----------------------|------|
| Solid 7z / tar\|zstd | Great ratio, **no per-file random access** |
| Full 7-Zip GUI / `7zz a` | Strong archives, **weak rsync-style selection** |
| `rsync` + archive later | Great filters, **two tools, two passes, temp disk** |
| Solid streams on live trees | Mid-run vanish → **whole job aborts** |

**rsync-archive** unifies that workflow:

1. **Rsync-style include/exclude** (and budgets) decide *what* goes in  
2. **Streaming compress** writes *without* copying the tree to temp  
3. **Non-solid / seekable** layouts keep **file-level random access**  
4. **Embed** nests finished archives into a master store 7z (**Copy**, no recompress)

Built in **Rust**, tested across Ubuntu and Rocky Linux, released as distro-matched x86_64 binaries.

---

## At a glance

| | |
|--|--|
| **Version** | [v0.5.3](https://github.com/hilather/rsync-archive/releases/tag/v0.5.3) |
| **Commands** | `create` · `embed` |
| **Default format** | Non-solid **7z** (per-file packs) |
| **Also** | Seekable-zstd · tar.zst · tar.lz4 |
| **Codecs (7z)** | LZMA2 · Zstd · LZ4 |
| **Selection** | rsync filters + dir/global size & count budgets |
| **Safety** | Atomic `OUT.partial` → rename · soft-skip live-tree races |
| **License** | MIT |

---

## Features

### Create — select, stream, archive

| Capability | Details |
|------------|---------|
| **Rsync-grade filters** | `--include` / `--exclude` / `--filter` / `--filter-from` / `*-from` files; first-match-wins; basename rules (`*.tmp`); `*` / `**` / `?`; anchored `/pat`; dir-only `pat/` |
| **Master lists** | `SRC...` walks, `--files-from` explicit lists, optional `--include-cwd` |
| **Directory budgets** | `--dir-max-size PATH=SIZE`, `--dir-max-files PATH=N` — recursive, newest-mtime-first, longest prefix wins |
| **Global caps** | `--max-total-size`, `--max-files`, `--max-size`, `--min-size`, `--newer-than` |
| **Restriction lists** | `--file-size-from`, `--dir-max-size-from`, `--dir-max-files-from` (rsync-like line syntax) |
| **Dry-run** | `-n` lists selected archive paths without writing |
| **Parallel encode (7z)** | Worker pool, ordered write, `--threads`, `--encode-concurrency`, `--encode-size-budget` (default **500M**) |
| **Live-tree resilience** | Soft-skip vanished/unreadable members; 7z pack rollback; tar/seekable zero-pad + warn |
| **Verify** | `--verify` post-write integrity (format-specific) |
| **Safe output** | Refuse overwrite without `--force`; write via `.partial` then rename |

### Embed — master store without recompression

| Capability | Details |
|------------|---------|
| **Store / Copy 7z** | Method `0x00` — wrap finished files as members |
| **Naming** | Basename by default; `--keep-path` + optional `--prefix` |
| **Magic policy** | Warn on non-7z · `--require-7z` hard-fail · `--allow-any` silent |
| **Dry-run / verify / force** | Same safety model as create |

### Formats — choose access pattern

| Format | Random access | Metadata | Symlinks / hard links | Infer from `-o` |
|--------|---------------|----------|------------------------|-----------------|
| **7z** (default) | Per-file packs | mtime, empty flags | File bodies only | `.7z` / default |
| **seekable-zstd** | Byte-range + name index | Member names | File bodies only | `.zst` |
| **tar-zstd** | Seekable Zstd + `RATAIDX1` | mode/uid/gid, uname/gname, dirs | Full | `.tar.zst` / `.tzst` |
| **tar-lz4** | Multi-frame LZ4 + `RATLFRM1` + `RATAIDX1` | same as tar-zstd | Full | `.tar.lz4` / `.tlz4` |

> **Create compresses. Embed only stores.**  
> Embed does **not** convert solid → non-solid (use [archiveconverter](https://github.com/hilather/archiveconverter) for that).

### Design principles

- **Non-solid only** for create — every member independently accessible  
- **Disk-friendly** — stream read / stream compress / append packs; no full-tree temp copy  
- **Selection first** — `--dry-run` before you spend CPU on compression  
- **Fail soft on live trees** — one rotated log should not kill a 100k-file pack  
- **Docs + tests required** on every behavior change ([`AGENTS.md`](AGENTS.md))

---

## Installation

### Prebuilt binaries (recommended)

Release assets for **Ubuntu 22.04 / 24.04** and **Rocky Linux 8 / 9 / 10** (x86_64, glibc-linked):

```bash
# Example: Ubuntu 24.04
curl -fsSL -o rsync-archive.tar.gz \
  https://github.com/hilather/rsync-archive/releases/download/v0.5.3/rsync-archive-ubuntu24.04-x86_64.tar.gz
tar -xzf rsync-archive.tar.gz
./rsync-archive --version
# optional: install system-wide
sudo install -m755 rsync-archive /usr/local/bin/
```

Each tarball contains a binary named **`rsync-archive`** (no OS suffix inside the archive).  
SHA-256 sidecars are published next to every asset.

### Build from source

**Requirements:** Rust **1.70+** (edition 2021)

```bash
git clone https://github.com/hilather/rsync-archive.git
cd rsync-archive
cargo build --release
# binary: target/release/rsync-archive
```

#### Optional native codecs

```bash
# Denser LZMA2 (liblzma) + LZ4HC for --method lz4 at levels ≥3
cargo build --release --features native-codecs
```

| Feature | Effect |
|---------|--------|
| `liblzma` | C-backed LZMA2 via system or bundled xz |
| `liblzma-static` | Force static/bundled liblzma |
| `lz4-hc` | True LZ4HC for levels ≥3 (`lz4` crate / vendored sources) |
| `native-codecs` | Convenience: `liblzma` + `lz4-hc` |

Default build uses pure-Rust **lzma-rust2** + **lz4_flex** + libzstd bindings — no system liblzma required.

---

## Quick start

```bash
# Help
rsync-archive --help
rsync-archive create --help
rsync-archive embed --help

# Non-solid 7z (LZMA2 default)
rsync-archive create -o out.7z --exclude '*.tmp' --level 5 --verify ./src/

# Fast path — Zstd packs inside 7z
rsync-archive create -o out.7z --method zstd --level 5 ./data/

# Dry-run selection
rsync-archive create -o out.7z -n --exclude '*.tmp' ./src/

# Explicit file list
rsync-archive create -o out.7z --files-from list.txt

# Seekable Zstd stream (byte-range access)
rsync-archive create -o out.zst --format seekable-zstd --level 5 ./src/
rsync-archive create -o pack.zst --level 3 ./data/          # .zst → seekable-zstd

# RA-friendly tar containers
rsync-archive create -o out.tar.zst --format tar-zstd ./src/
rsync-archive create -o out.tar.lz4 --format tar-lz4 ./src/

# Log collection with budgets
rsync-archive create -o logs.7z \
  --max-total-size 500M \
  --max-files 1000 \
  --max-size 50M \
  --newer-than 7d \
  /var/log/

# Embed finished archives under a master store 7z
rsync-archive embed -o master.7z nest1.7z nest2.7z
rsync-archive embed -o master.7z --force --verify nest1.7z nest2.7z
rsync-archive embed -o master.7z --allow-any a.bin b.bin
```

### Realistic create example

```bash
rsync-archive create -o game.7z \
  --exclude '*.tmp' \
  --exclude-from excludes.txt \
  --filter '- cache/**' \
  --method zstd --level 5 --verify \
  /data/game/
```

### Directory & list restrictions

```bash
rsync-archive create -o o.7z --dir-max-size logs/=100M --dir-max-size cache=50M tree/
rsync-archive create -o o.7z --dir-max-files logs/=10 --dir-max-files-from limits.txt tree/
rsync-archive create -o o.7z \
  --files-from master.txt \
  --file-size-from sizes.txt \
  --dir-max-size-from dirs.txt
```

---

## Output formats

### Non-solid 7z (default)

Per-file compressed packs → **file-level random access**.

| `--method` | Codec ID | Notes |
|------------|----------|--------|
| `lzma2` (default) | LZMA2 `0x21` | Best ratio among built-in options; slower |
| `zstd` | Zstd `04 F7 11 01` | Best **speed × ratio** for many small files |
| `lz4` | LZ4 `04 F7 11 04` | Fastest; levels 1–2 flex, ≥3 HC with `lz4-hc` |

- Empty files use empty flags; mtimes from source  
- Parallel encode: fixed worker pool + **500M** in-flight uncompressed budget  
- Symlinks / hard-link *members* are skipped (first regular-file body for a hard-linked inode is kept)

### Seekable-zstd

Single **seekable** Zstd stream ([zeekstd](https://crates.io/crates/zeekstd)) with length-prefixed members and a trailer index (`name → uncompressed offset`).

- Infer from bare `-o *.zst` when `--format` omitted  
- `--method` is **7z-only** (omit for this format)  
- Layout: [`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md)

### tar-zstd

Valid **ustar/pax tar** inside seekable Zstd + **`RATAIDX1`** member index (path, size, mtime, mode, uid, gid; uname/gname in tar headers).

Includes:
- Parent directory members (`typeflag='5'`, trailing `/`)
- Symbolic links (`typeflag='2'`)
- Hard links (`typeflag='1'`, linkname = first archive path for the inode)

Infer from `-o *.tar.zst` or `*.tzst`.  
Layout: [`docs/FORMAT_TAR_ZSTD.md`](docs/FORMAT_TAR_ZSTD.md)

### tar-lz4

Same tar + index idea with **independent LZ4 frames** + cleartext **`RATLFRM1`** frame table (not a standard “seekable LZ4” format).

Infer from `-o *.tar.lz4` or `*.tlz4`.  
Layout: [`docs/FORMAT_TAR_LZ4.md`](docs/FORMAT_TAR_LZ4.md)

### Format inference

| Output path ends with | Resolved format |
|-----------------------|-----------------|
| `.tar.zst` / `.tzst` | tar-zstd |
| `.tar.lz4` / `.tlz4` | tar-lz4 |
| `.zst` | seekable-zstd |
| `.7z` / other | 7z |

---

## Selection & filters

Full semantics: [`docs/SELECTION.md`](docs/SELECTION.md) · rsync parity: [`docs/RSYNC_PARITY.md`](docs/RSYNC_PARITY.md)

### Core rules

| Rule | Behavior |
|------|----------|
| First match wins | Ordered rule list |
| No match | **Include** (default) |
| No `/` and no `**` in pattern | **Basename** match (`*.tmp` matches `dir/a.tmp`) |
| Leading `/` | Anchored from archive root |
| Trailing `/` | Directory-only (exclude prunes the tree) |
| Unanchored multi-segment | End-anchored suffix (`foo/bar` matches `a/foo/bar`) |
| `*` / `?` | One path segment |
| `**` | Across segments; forces full-path mode |
| Filter file caps | **10 MiB** or **1,000,000** lines per file |

### Rule build order (`create`)

Clap stores each flag type in its own list — heterogeneous flags are **not** interleaved by CLI position:

```text
1. --include-from FILE…
2. --exclude-from FILE…
3. --filter-from FILE…     ← preferred for full ordered lists
4. --filter RULE…
5. --include PATTERN…      (all includes as a batch)
6. --exclude PATTERN…      (all excludes as a batch)
```

Prefer **`--filter-from`** or repeated **`--filter`** for rsync-like ordered mixes.

```bash
# Include-only idiom (batch order works)
rsync-archive create -o o.7z --include '*.c' --exclude '*' ./src/

# True ordered mix
rsync-archive create -o o.7z --filter '- *' --filter '+ *.c' ./src/
rsync-archive create -n -o o.7z --filter-from rules.txt ./src/
```

Filter line syntax:

```text
+ pattern
- pattern
include pattern
exclude pattern
# comments and blank lines ignored
```

### Source mapping

| Source | Behavior |
|--------|----------|
| `SRC` without trailing `/` | Archive paths include the directory name (`photos` → `photos/a.jpg`) |
| `SRC/` with trailing `/` | Strips the directory name (`photos/` → `a.jpg`) |
| `--files-from` | Exclusive of `SRC...`; relative lines keep path; absolute lines use basename |
| `--files-from-skip-missing` | Soft-skip missing list lines (default: hard-fail) |
| `--include-cwd` | Pack CWD at archive root; **ignores** rsync filters; skips `-o` / `.partial` |
| Multi-SRC missing roots | ≥2 roots (or SRC + `--include-cwd`): soft-skip missing root; single missing SRC hard-fails |

### Selection pipeline

```text
1. Master collect   SRC… / --files-from / --include-cwd
2. Rsync filters    include · exclude · filter (-from)
3. Per-file global  --max-size · --min-size · --newer-than
4. --file-size-from pattern max=SIZE (first match wins)
5. Dir budgets      --dir-max-size[-from] then --dir-max-files[-from]
6. Global fill      --max-total-size then --max-files  (newest mtime first)
```

Restriction lists only affect **matching** paths; unlisted paths ignore that list.

### Restriction list formats

**`--file-size-from`**

```text
**/*.log          max=100M
var/log/app.log   max=10M
```

**`--dir-max-size-from`**

```text
logs/             max=500M
cache/            max=1G files=50
logs/=100M        # legacy PATH=SIZE
```

**`--dir-max-files-from`**

```text
logs/=10
cache/ files=50
```

### Members & live trees

| Kind | tar-zstd / tar-lz4 | 7z / seekable-zstd |
|------|--------------------|--------------------|
| Regular file | Full body | Full body |
| Symlink | Stored as link | Skipped |
| Hard link (Unix) | Stored as hard link (size 0) | First body only |
| Special (fifo/device/…) | Skipped | Skipped |

**Encode soft-skip:** members that vanish between selection and open are omitted (counted as vanished). 7z rolls back partial packs; tar/seekable re-stat after open and zero-pad short mid-reads with a warning. If **all** members soft-skip, create errors unless **`--allow-empty`**.

---

## CLI reference

### Global

| Flag | Meaning |
|------|---------|
| `-v` / `-vv` | Debug / trace logging on stderr (`info` → `debug` → `trace`) |
| `RUST_LOG` | Overrides verbosity filter |

**Exit codes:** `0` success · `1` operational error · `2` usage error

---

### `create`

```text
rsync-archive create -o OUT [OPTIONS] [SRC...]
```

#### Output & control

| Flag | Default | Meaning |
|------|---------|---------|
| `-o`, `--output` | *required* | Output path |
| `--format` / `--output-format` | infer / `7z` | `7z` · `seekable-zstd` · `tar-zstd` · `tar-lz4` |
| `-n`, `--dry-run` | off | List selection only; no write |
| `--force` | off | Overwrite existing `-o` |
| `--verify` | off | Post-write integrity check |
| `--allow-empty` | off | Empty / all-vanished → exit 0, no `-o` |
| `--level` | `5` | Compression level **0–9** |
| `--method` | `lzma2` | **7z only:** `lzma2` · `zstd` · `lz4` |
| `--threads` | auto | **7z only:** encode workers (many tiny files → 1, else CPUs) |
| `--encode-concurrency` | `0` | **7z only:** max concurrent encodes (`0` = auto) |
| `--encode-size-budget` | `500M` | **7z only:** max in-flight uncompressed size (`0` = unlimited) |

#### Selection inputs

| Flag | Default | Meaning |
|------|---------|---------|
| `SRC...` | — | Source paths (required unless `--files-from` or `--include-cwd`) |
| `--files-from` | — | Master collect list (exclusive of `SRC...`) |
| `--files-from-skip-missing` | off | Soft-skip missing/unreadable list lines |
| `--include-cwd` | off | Pack CWD at archive root; skip `-o` / `.partial` |

#### Filters

| Flag | Meaning |
|------|---------|
| `--exclude` / `--include` | Rsync-style patterns (repeatable; batched) |
| `--exclude-from` / `--include-from` | Pattern files (repeatable) |
| `--filter-from` | Ordered `+/-` filter file (repeatable; preferred) |
| `--filter` | `+ pattern` / `- pattern` (repeatable; CLI order among filters) |

#### Size, count & age limits

| Flag | Meaning |
|------|---------|
| `--dir-max-size PATH=SIZE` | Cap selected bytes under dir tree (repeatable) |
| `--dir-max-size-from FILE` | Dir size/count list file |
| `--dir-max-files PATH=N` | Cap file count under dir tree (repeatable) |
| `--dir-max-files-from FILE` | Dir file-count list file |
| `--file-size-from FILE` | Per-path max size (`PATTERN max=SIZE`) |
| `--max-total-size SIZE` | Global selected-byte cap (newest-first) |
| `--max-files N` | Global max selected file count (newest-first) |
| `--max-size SIZE` | Skip any single file larger than SIZE |
| `--min-size SIZE` | Skip files smaller than SIZE (`0` = off) |
| `--newer-than DURATION` | Keep mtime within window (`7d`, `24h`, `30m`, `90s`) |

**SIZE syntax:** raw bytes or `K` / `M` / `G` suffixes (e.g. `100M`, `1G`).  
**DURATION syntax:** integer + optional `d` / `h` / `m` / `s`.

---

### `embed`

```text
rsync-archive embed -o OUT [OPTIONS] FILE...
```

| Flag | Default | Meaning |
|------|---------|---------|
| `-o`, `--output` | *required* | Master `.7z` path |
| `FILE...` | *required* | Inputs to embed (typically finished `.7z`) |
| `-n`, `--dry-run` | off | List planned member names only |
| `--force` | off | Overwrite existing `-o` |
| `--prefix PREFIX` | — | Prefix all member names |
| `--keep-path` | off | Keep path as name (default: basename) |
| `--require-7z` | off | Fail if missing 7z magic |
| `--allow-any` | off | Allow non-7z store blobs (silent) |
| `--verify` | off | Post-write test of master archive |

```bash
rsync-archive embed -o master.7z nest1.7z nest2.7z
rsync-archive embed -o master.7z --keep-path --prefix packs/ ./build/a.7z
rsync-archive embed -o master.7z --require-7z --verify a.7z b.7z
rsync-archive embed -o master.7z --allow-any --dry-run blob.bin
```

---

## Performance notes

Fair non-solid benchmarks vs stock `7zz` / [7-Zip-zstd](https://github.com/mcmilk/7-Zip-zstd) (host: 12-core, 2026-07-28). Full tables: [`docs/bench/RESULTS.md`](docs/bench/RESULTS.md).

| Goal | Winner |
|------|--------|
| **Wall time** (zstd / lz4, many small files) | **rsync-archive** — often **2–10×** faster than 7zz-zstd |
| **Wall time** (lzma2 single-thread) | stock **7zz** still ~1.2–1.4× ahead |
| **Wall time** (lzma2 multi-thread, small L5) | **rsync-archive** can beat 7zz |
| **Archive size / ratio** | Native tools denser (~1.3–1.9× smaller) |

**Product picks:**

| Priority | Recommendation |
|----------|----------------|
| Speed | `--method zstd` |
| Best ratio we offer | `--method lzma2` (+ optional `--features native-codecs`) |
| Max throughput | `--method lz4` |
| Metadata + links | `--format tar-zstd` or `tar-lz4` |
| Simple byte-range stream | `--format seekable-zstd` |

```bash
cargo build --release --bin rsync-archive --bin bench_compress
./target/release/bench_compress run --scale small --threads 1,4 --level 1,5 --methods all
```

---

## Architecture

```text
src/
  main.rs, cli.rs, lib.rs, error.rs
  archive/
    sevenz/            # non-solid header, store writer, LZMA2/Zstd/LZ4 packs
    seekable_zstd/     # seekable-zstd create + list/extract helpers
    tar_zstd/          # RA tar.zst (seekable Zstd + RATAIDX1)
    tar_lz4/           # RA tar.lz4 (multi-frame LZ4 + RATLFRM1 + RATAIDX1)
    tar_common.rs      # shared ustar/pax + index helpers
  select/              # pathnorm, rules, matcher, walk, dir budgets, global restrict
  pipeline/
    create.rs          # selection + multi-format write
    embed.rs           # store outer embed
    output.rs          # partial path, --force, atomic rename
  util/                # size parse, soft log, auto threads
docs/                  # design, selection, format layouts, benches
tests/                 # CLI smoke, e2e create/embed/formats, filter parity, soft-fail
```

### Implementation status

| Stage | Status |
|-------|--------|
| Bootstrap, foundations, store writer | **Done** |
| Embed pipeline | **Done** |
| Rsync filter engine + walk + dry-run | **Done** |
| Non-solid LZMA2 create + verify | **Done** |
| Large-file streaming e2e | **Done** |
| Parallel encode (threads / concurrency / budget) | **Done** |
| Zstd + LZ4 in 7z | **Done** |
| Seekable-zstd / tar-zstd / tar-lz4 | **Done** |
| Dir size & file-count budgets | **Done** |
| Global log-collection limits | **Done** |
| Encode performance (OPT-01..14) | **Done** |

See [`docs/DESIGN.md`](docs/DESIGN.md) and [`docs/BACKLOG.md`](docs/BACKLOG.md) for roadmap items (e.g. `--older-than`, head/tail sampling, free-disk guards).

---

## CI & releases

| Workflow | Trigger | What |
|----------|---------|------|
| **CI** | push / PR to `main` | Build on Ubuntu **22.04 / 24.04** and Rocky **8 / 9 / 10**; full `cargo test` on Ubuntu 24.04 |
| **Release** | tag `v*` | Per-distro `.tar.gz` + `.sha256` assets |

```bash
# Maintainers: cut a release
git tag -a v0.5.3 -m "v0.5.3"
git push origin v0.5.3
```

---

## Documentation

| Document | Contents |
|----------|----------|
| [`docs/DESIGN.md`](docs/DESIGN.md) | Full design, stages, decisions |
| [`docs/SELECTION.md`](docs/SELECTION.md) | Filter & restriction **source of truth** (v1) |
| [`docs/RSYNC_PARITY.md`](docs/RSYNC_PARITY.md) | Parity matrix vs rsync |
| [`docs/FORMAT_SEEKABLE_ZSTD.md`](docs/FORMAT_SEEKABLE_ZSTD.md) | Seekable-zstd on-disk layout |
| [`docs/FORMAT_TAR_ZSTD.md`](docs/FORMAT_TAR_ZSTD.md) | tar.zst layout |
| [`docs/FORMAT_TAR_LZ4.md`](docs/FORMAT_TAR_LZ4.md) | tar.lz4 layout |
| [`docs/BENCH.md`](docs/BENCH.md) | Benchmark harness & fairness rules |
| [`docs/bench/RESULTS.md`](docs/bench/RESULTS.md) | Published numbers |
| [`docs/BACKLOG.md`](docs/BACKLOG.md) | Future work |
| [`AGENTS.md`](AGENTS.md) | Contributor / agent policy |

---

## Contributing

**Every behavior-changing change must update docs and include regression tests** in the same PR.

- [`AGENTS.md`](AGENTS.md)
- [`.grok/skills/keep-docs-current/`](.grok/skills/keep-docs-current/)
- [`.grok/skills/keep-tests-current/`](.grok/skills/keep-tests-current/)

```bash
cargo test
cargo run -- --help
./scripts/v1_acceptance.sh   # when present / for release checks
```

Live rsync parity tests soft-skip if `rsync` is not installed (CI stays green).

---

## Related projects

| Project | Relationship |
|---------|--------------|
| [archiveconverter](https://github.com/hilather/archiveconverter) | Solid → non-solid conversion; outer store append sibling to **embed** |
| [rsync](https://rsync.samba.org/) | Selection semantics inspiration |
| [7-Zip / 7-Zip-zstd](https://github.com/mcmilk/7-Zip-zstd) | Native non-solid peers for benchmarks |

---

## License

[MIT](LICENSE) © hilather contributors

---

<div align="center">

**Select with confidence. Archive without thrashing the disk. Extract one file without unpacking the world.**

[↑ Back to top](#rsync-archive)

</div>
