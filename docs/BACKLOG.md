# Feature backlog

Product requests not yet implemented. Canonical design stages: [`DESIGN.md`](DESIGN.md).  
When a backlog item ships, move it to README status / SELECTION and delete or mark done here.

---

## Selection

### Directory size budgets (newest-first)

**Status:** **Done** — `create --dir-max-size PATH=SIZE` (repeatable)  
**Design ref:** `DESIGN.md` → Stage 9 → “directory size budgets”  
**Impl:** `src/select/dir_budget.rs`; wired in `build_selection`; see [`SELECTION.md`](SELECTION.md).

**Intent:** When a directory has a **size collection limit**, fill the budget with the **most recently modified** files first (newest mtime wins). Files that would exceed the remaining budget are **not** selected and must be **logged** as excluded by the directory limit.

| Requirement | Detail |
|-------------|--------|
| Per-directory budget | Cap total **selected** bytes under a directory (after normal rsync filters) |
| Order | Sort candidates by **mtime descending**, then archive_name ascending |
| Accumulation | Include while `sum + size ≤ limit`; further files under that dir are budget-skips |
| Logging | Compact restriction report (kept+skip path:size lists); counter `skipped_dir_budget` |
| Dry-run | Same selection as write; summary shows budget skips |
| Nesting | Longest matching archive-relative prefix wins |

**Not this feature:** global per-file `--max-size` / `--min-size` (also Stage 9, separate).

### Directory file-count limits (newest-first, recursive)

**Status:** **Done** — `create --dir-max-files PATH=N` + `--dir-max-files-from FILE`  
**IDs:** `SEL-RESTRICT-RSYNC-SCOPE`, `SEL-RESTRICT-INDEPENDENT-LISTS`  
**Impl:** `src/select/dir_budget.rs`; see [`SELECTION.md`](SELECTION.md).

| Requirement | Detail |
|-------------|--------|
| Scope | **Recursive** under `PATH/` (same tree model as `--dir-max-size`) |
| Nesting | Longest matching prefix wins; independent of collection filters |
| Order | Newest mtime first; keep first `N` |
| Logging | Compact restriction report; counter `skipped_dir_file_limit` |

### Log-collection / global restrictions

**Status:** **Done (MVP)** — global + per-file post-filter caps for cautious off-system packs.  
**Canonical notes:** [`SELECTION.md`](SELECTION.md) → “Global / log-collection restrictions”.

| Flag | Behavior |
|------|----------|
| `--max-total-size SIZE` | Global selected-byte cap; newest-mtime-first |
| `--max-files N` | Global count cap; newest-mtime-first |
| `--max-size SIZE` | Skip single file larger than SIZE |
| `--min-size SIZE` | Skip files smaller than SIZE (`0` = off) |
| `--newer-than DURATION` | Only mtime within last DURATION (`7d`/`24h`/`30m`/`90s`) |

Order: filters → per-file size/age → dir budgets → global caps. Compact `RestrictionReport` +
`SelectionStats` counters. Dry-run and write share `build_selection`.

Still open (ideas):

1. **`--older-than` / absolute mtime** — full age window
2. **Head/tail partial file read** — huge `*.log` without full ingest
3. **`--min-free-space`** on output volume before write
4. **`--restriction-strict`** — empty after limits → hard error vs dry-run ok

---

## Performance (create encode path)

**Status:** **Done** (2026-07-28) — OPT-01..14 including optional C backends.

### Implemented

| ID | What shipped |
|----|----------------|
| **OPT-01** | Fixed worker pool (no per-file `spawn`) |
| **OPT-02** | Ordered streaming write as packs complete |
| **OPT-03** | `SelectedEntry.mtime_unix` + size; encode does not re-stat |
| **OPT-04** | CWD resolved once in walk / files-from |
| **OPT-05** | Completion-driven admission (join-any) |
| **OPT-06** | Warn on explicit multi-thread + many tiny files |
| **OPT-07** | LZMA2 `dict_size_for_member` clamp to file size |
| **OPT-08** | Zstd pledged size, frame checksum off, small-file one-shot |
| **OPT-09** | Pack buffer pre-size; `pack_crc` on `CompressedPack` |
| **OPT-10** | Optional **`liblzma`** Cargo feature (raw LZMA2); default pure-Rust |
| **OPT-11** | Zstd `multithread` (`zstdmt`) for large members when concurrency=1 |
| **OPT-12** / **12b** | Optional **`lz4-hc`** feature (liblz4 HC for levels ≥3; 1–2 stay `lz4_flex`) |
| **OPT-13** | `RSYNC_ARCHIVE_TIMINGS=1` → encode phase ms on stderr |
| **OPT-14** | Auto-thread thresholds retuned |

Also: `BufWriter` (1 MiB) on 7z create output; convenience feature **`native-codecs`** = `liblzma` + `lz4-hc`.

### Optional follow-ups (not blocking)

| ID | Notes |
|----|--------|
| **OPT-10c** | Further liblzma match-finder tuning if ratio still lags stock 7zz |
| **OPT-bench** | Publish `native-codecs` vs default benches in `docs/bench/RESULTS.md` |

```bash
cargo build --release --features native-codecs --bin rsync-archive --bin bench_compress
./target/release/bench_compress run --scale tiny --threads 1,4 --level 1,5 --methods all
```

---

## Codecs / formats (random-access friendly)

### Zstd + LZ4 create methods (7z non-solid)

**Status:** **Done** — `create --method zstd|lz4|lzma2`  

File-level random access via independent packs. Uses `zstd` (libzstd) and `lz4_flex`.

### True seekable-Zstd single stream (optional)

**Status:** **Done** (MVP) — `create --format seekable-zstd` / `-o *.zst`  
**IDs:** `CODEC-ZSTD-SEEKABLE-STREAM`  
**Docs:** [`FORMAT_SEEKABLE_ZSTD.md`](FORMAT_SEEKABLE_ZSTD.md)

| Requirement | Detail |
|-------------|--------|
| Format | Zstd **seekable** multi-frame + seek table ([`zeekstd`](https://crates.io/crates/zeekstd)) |
| Payload | Length-prefixed members + trailing `RAZSIDX1` index (name → uncompressed data offset) |
| Use case | Single stream with byte-range reads; list/extract helpers for verify/tests |
| Note | Distinct from per-file Zstd in non-solid 7z (`--method zstd`) |

**Follow-ups (not MVP):** full `extract` subcommand, tar-compatible payload, parallel encode for this format.
---

## Threading (archiveconverter parity)

**Status:** **Implemented** (create encode workers) — keep docs in sync  

Aligned with [archiveconverter](https://github.com/hilather/archiveconverter):

| Flag | Default | Meaning |
|------|---------|---------|
| `--threads` | **auto** (omit) | Explicit worker count; auto: many tiny files → **1**, else `available_parallelism` |
| `--encode-concurrency` | **0** (auto) | Max concurrent file encodes; `0` → use resolved threads |
| `--encode-size-budget` | **`500M`** | Max sum of **uncompressed** sizes in flight; `0` = unlimited |

Auto tiny-file policy matches archiveconverter pack policy (`HIGH_FILE_COUNT=1000`, avg &lt; 64 KiB → 1 thread).

---

## Embed

_(none beyond DESIGN stages)_
