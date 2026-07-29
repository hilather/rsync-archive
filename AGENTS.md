# Agent instructions — rsync-archive

You are working in **rsync-archive**, a Rust tool that:

1. **`create`** — builds **non-solid** 7z archives from the filesystem with **rsync-style selection**, streaming read/compress/write (no full tree staging).
2. **`embed`** — wraps finished regular files (typically `.7z`) under a **master store/Copy** 7z without recompression.

This file is **mandatory policy** for every coding agent session in this repo (Grok, Claude, Codex, Cursor, etc.).

**Behavior source of truth:** `README.md` + `src/cli.rs` + tests.  
**Design background:** `docs/DESIGN.md` (optional deep reference; do not let it drift ahead of implemented behavior in README).

---

## Non-goals (do not implement unless design is updated)

- Solid 7z create or solid detection/convert on embed (use archiveconverter for solid→non-solid).
- Full rsync protocol, daemon, or network sync.
- Nested create-inside-create (create once, then **embed**).
- Whole-tree temp copy as the main create path.

---

## Non-negotiable: docs stay current

**Every commit that changes user-visible behavior, CLI, defaults, or architecture must update documentation in the same change** (or the same PR before merge).

| Change type | Update these |
|-------------|----------------|
| New/changed CLI flag or default | `README.md` flag tables + quick start; `src/cli.rs` help text |
| Selection / filter semantics | `README.md` + `docs/SELECTION.md` (when present) + tests |
| Create/embed/streaming behavior | `README.md` architecture / status table |
| Module layout change | `README.md` project layout; this file’s project map |
| Docs-only / comment-only | No extra churn required; fix anything you know is wrong |

When removing or renaming a flag, grep README + AGENTS + skills and fix all hits.

**Skill:** `.grok/skills/keep-docs-current/SKILL.md`  
Run before commit when user-visible behavior or docs might be stale.

---

## Non-negotiable: tests cover every change

**Every commit that changes behavior must be covered by automated tests.**

Rules:

1. Every commit that changes **user-visible behavior**, **selection/filter semantics**, **CLI flags/defaults**, **archive write/embed correctness**, **error handling**, or **bug fixes** MUST add or update automated tests in the **same change** (same commit preferred; **same PR required** before merge).
2. **New features** require: unit tests for pure logic **and** at least one integration/e2e test that exercises the CLI or public API path when applicable.
3. **Bug fixes** require a **regression test that fails before the fix and passes after** (red–green). Prefer a minimal fixture that reproduces the bug.
4. **Refactors with no behavior change:** new tests not strictly required if the existing suite still covers the paths; agents must run `cargo test` and must not weaken coverage. If a refactor deletes tests, **replace them**.
5. Do **not** claim “tested manually only” for shippable behavior.
6. Do not skip tests with `#[ignore]` for core paths without documenting why **and** a tracked follow-up; **ignored tests do not count** as coverage under this policy.
7. **Test locations:** unit tests co-located (`#[cfg(test)]` in `src/...`); e2e under `tests/`; filter parity under `tests/filter_parity.rs` when Stage 4 lands.
8. Before marking work done: **`cargo test` green** for the change set.
9. When behavior changes, **docs updates still required** — tests **and** docs together.

### Agent checklist — required tests by change type

| Change type | Required tests |
|-------------|----------------|
| New CLI flag / default | Parse/unit + create/embed e2e or lib API test using the flag |
| Filter/selection rule or match fix | Table-driven unit case(s); dry-run list assertion if walk-related |
| Create/embed writer fix | Roundtrip list/test + extract sample; non-solid assert if relevant |
| Streaming/codec change | Unit or integration with multi-chunk data; large-file path if size-related |
| Bug fix | Regression test that reproduced the bug (red then green) |
| Error-path / exit-code change | Assert exit code and/or error variant |
| Docs-only / comment-only | No new tests required |
| Pure refactor | Run full suite; no coverage drop without replacement |

**Skill:** `.grok/skills/keep-tests-current/SKILL.md`  
Run before every commit when `src/` or `tests/` (or behavior) changed.

---

## Before-commit procedure

```text
1. Diff change set
2. keep-tests-current  (if code/behavior)
3. keep-docs-current   (if user-visible)
4. cargo test
5. Commit code + tests + docs together when practical
```

---

## Defaults that must stay accurate in docs

| Topic | Default / rule |
|-------|----------------|
| Create format | **Non-solid** 7z only |
| Create compression | LZMA2 `0x21` for non-empty; empty via empty flags |
| Compression level | **5** |
| Create method | **lzma2** default; also `zstd`, `lz4` |
| Encode threads | **auto** (omit `--threads`; many tiny files → 1) |
| Encode concurrency | **0** → auto from threads |
| Encode size budget | **500M** in-flight uncompressed (like archiveconverter nested budget) |
| Dir size budgets | `--dir-max-size PATH=SIZE` (optional); recursive; newest-mtime-first; longest prefix wins |
| Dir file-count limits | `--dir-max-files PATH=N` / `--dir-max-files-from` (optional); **recursive** under PATH/; longest prefix; newest-mtime-first |
| Global size/count caps | `--max-total-size` / `--max-files` (optional); newest-mtime-first; after dir limits |
| Per-file size/age | `--max-size` / `--min-size` (`0`=off) / `--newer-than` (e.g. `7d`); before dir budgets |
| Embed method | **Copy** `0x00` (store), no recompress |
| Embed naming | **Basename flatten** unless `--keep-path` |
| Overwrite | **Error** if `-o` exists unless `--force` |
| Atomic write | `OUT.partial` → rename after successful finish |
| Empty write | **Error** (dry-run of empty selection may exit 0 with message) |
| Symlinks | **tar-zstd / tar-lz4:** archived as typeflag `'2'` (linkname; size 0). **7z / seekable-zstd:** skipped (counted). Special files always skipped. |
| Hard links | **Unix:** first `(dev,ino)` path is full file; later → `HardLink` (size 0). **tar-zstd / tar-lz4:** typeflag `'1'` (linkname = first archive path). **7z / seekable-zstd:** skip hard-link members (keep first file). Non-Unix: no detection. |
| `--files-from` | Exclusive of `SRC...`; paths relative to **CWD** unless absolute |
| Patterns without `/` | Match **basename** (e.g. `*.tmp` matches `dir/a.tmp`) |
| Streaming | Peak RAM O(dict + I/O buffers), not O(file size) for create |

---

## Project map (short)

| Path | Role |
|------|------|
| `src/cli.rs` | Clap surface — source of truth for flags |
| `src/error.rs` | Error types |
| `src/main.rs` | Binary entry, exit codes |
| `src/select/pathnorm.rs` | Archive path normalization; reject `..` |
| `src/select/mod.rs` | `SourceSpec`, archive name mapping; re-exports filter API |
| `src/select/rules.rs` | `Rule`, `RuleAction`, `RuleSet`, pattern parse |
| `src/select/matcher.rs` | Path match, `action_for`, `should_prune_dir` |
| `src/select/from_file.rs` | include-from / exclude-from / filter files; size/line caps |
| `src/select/walk.rs` | SRC walk + `--files-from` → `SelectedEntry`; prune; collisions |
| `src/select/dir_budget.rs` | `--dir-max-size` + `--dir-max-files` (both recursive); `RestrictionReport` |
| `src/select/global_restrict.rs` | `--max-size`/`--min-size`/`--newer-than` + global `--max-total-size`/`--max-files` |
| `src/pipeline/output.rs` | `*.partial` naming, `--force` check, rename commit |
| `src/pipeline/create.rs` | **`create` selection + multi-method 7z / seekable-zstd** (partial+rename, verify) |
| `src/archive/sevenz/lzma2_writer.rs` | `NonsolidLzma2Writer` — non-solid create packs (all methods) |
| `src/archive/sevenz/codec.rs` | LZMA2 / Zstd / LZ4 encode; optional `liblzma` + `lz4-hc` features |
| `src/archive/seekable_zstd/` | Seekable-zstd create + member index list/extract |
| `src/archive/tar_common.rs` | Shared ustar/pax headers + RATAIDX1 index encode/parse |
| `src/archive/tar_zstd/` | RA-friendly tar.zst (ustar/pax + seekable Zstd + RATAIDX1) |
| `src/archive/tar_lz4/` | RA-friendly tar.lz4 (ustar/pax + multi-frame LZ4 + RATLFRM1 + RATAIDX1) |
| `src/util/` | Tracing init (`-v` / `-vv`) |
| `src/archive/mod.rs` | Archive module root; re-exports store + seekable-zstd API |
| `src/archive/sevenz/header.rs` | `HeaderFile`, `write_raw_header`, `write_start_header`, empty bits, names, mtime, attrs |
| `src/archive/sevenz/store_writer.rs` | `NonsolidStoreWriter` (Copy `0x00`) — embed foundation |
| `src/archive/sevenz/` | Headers + store writer; LZMA2 create writer (later) |
| `src/pipeline/embed.rs` | **`embed` command** — naming, magic, store write, verify |
| `docs/DESIGN.md` | Full design (stages, decisions) |
| `docs/SELECTION.md` | **Filter semantics source of truth** (Stage 4 frozen v1; keep in sync with `src/select/`) |
| `docs/BACKLOG.md` | Product feature backlog (seekable-zstd, …) |
| `src/util/auto_threads.rs` | AC-style auto workers (tiny-file → 1) |
| `src/util/size_parse.rs` | `500M` budget parse + admit helper |
| `tests/` | e2e and parity (`cli_smoke`, `filter_parity`) |
| `.grok/skills/keep-docs-current/` | Docs sync skill |
| `.grok/skills/keep-tests-current/` | Regression test skill |

---

## Build / test expectations

```bash
cargo test
cargo build --release --bins
cargo run -- --help
cargo run -- create --help
cargo run -- embed --help
```

Do not commit multi-hundred-MB fixtures or `target/`. Prefer small fixtures under `tests/` or tempdirs.

---

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Success (including dry-run with zero files, with message) |
| 1 | Operational error |
| 2 | CLI usage error |

---

## Companion

Prior art for non-solid 7z headers and store outer append:

- [hilather/archiveconverter](https://github.com/hilather/archiveconverter)

Do **not** assume archiveconverter provides streaming **compression** for create; rsync-archive owns that path.
