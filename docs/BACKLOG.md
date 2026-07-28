# Feature backlog

Product requests not yet implemented. Canonical design stages: [`DESIGN.md`](DESIGN.md).  
When a backlog item ships, move it to README status / SELECTION and delete or mark done here.

---

## Selection

### Directory size budgets (newest-first)

**Status:** Planned (Stage 9+)  
**Design ref:** `DESIGN.md` → Stage 9 → “directory size budgets”

**Intent:** When a directory has a **size collection limit**, fill the budget with the **most recently modified** files first (newest mtime wins). Files that would exceed the remaining budget are **not** selected and must be **logged** as excluded by the directory limit.

| Requirement | Detail |
|-------------|--------|
| Per-directory budget | Cap total **selected** bytes under a directory (after normal rsync filters) |
| Order | Sort candidates by **mtime descending**, then stable path |
| Accumulation | Include while `sum + size ≤ limit`; further files under that dir are budget-skips |
| Logging | Log each budget-excluded file (path, size, mtime, limit); counter `skipped_dir_budget` |
| Dry-run | Same selection as write; summary shows budget skips |

**Not this feature:** global per-file `--max-size` / `--min-size` (also Stage 9, separate).

**Suggested implementation order:** after create write (done) + method plugins.

**Acceptance ideas:**

1. Dir with files 10/20/30 MiB, budget 35 MiB, newest = 30 then 20 then 10 → select 30 only (or 30+… depending mtimes); log the rest.  
2. Dry-run listing matches write membership for budgeted dirs.  
3. Verbose/stderr shows budget exclusions distinctly from `--exclude` patterns.

---

## Codecs / formats (random-access friendly)

### Seekable Zstd create method

**Status:** Planned  
**IDs:** `CODEC-ZSTD-SEEKABLE`

| Requirement | Detail |
|-------------|--------|
| Method | Per-member **Zstd** packs in non-solid 7z **or** seekable-zstd outer for single-blob / tar-like use |
| Seekable | Prefer **Zstd seekable format** (independent frames + seek table) for byte-range access |
| Rust crates | Primary: [`zstd`](https://crates.io/crates/zstd) (libzstd); seekable: [`zeekstd`](https://crates.io/crates/zeekstd) / [`zstd-framed`](https://crates.io/crates/zstd-framed) |
| CLI sketch | `--method zstd` (default later?) · level map · keep non-solid multi-file semantics |
| Why | Best speed × ratio for remount / partial read vs LZMA2 |

### LZ4 create method

**Status:** Planned  
**IDs:** `CODEC-LZ4`

| Requirement | Detail |
|-------------|--------|
| Method | Per-member **LZ4** packs in non-solid 7z (file-level random access) |
| Rust crates | [`lz4_flex`](https://crates.io/crates/lz4_flex) (pure Rust) or `lz4` bindings |
| CLI sketch | `--method lz4` |
| Why | Maximum encode/decode speed; weaker ratio than Zstd |

### Shared method plumbing

**Status:** Planned with codecs  

- `create --method lzma2|zstd|lz4` (lzma2 = current default until Zstd proven)  
- Same streaming / ordered-append / thread budget pipeline  
- Embed remains **Copy/store** only (no recompress)

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
