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
| Logging | Log each budget-excluded file (path, size, mtime, limit); counter `skipped_dir_budget` |
| Dry-run | Same selection as write; summary shows budget skips |
| Nesting | Longest matching archive-relative prefix wins |

**Not this feature:** global per-file `--max-size` / `--min-size` (also Stage 9, separate).

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
