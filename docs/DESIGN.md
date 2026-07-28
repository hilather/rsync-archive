# rsync-archive — Design Document

| Field | Value |
|-------|--------|
| **Title** | rsync-archive: stream-create non-solid 7z with rsync selection + embed |
| **Author** | TBD |
| **Date** | 2026-07-28 |
| **Status** | Revised |
| **Repo** | `/home/mbrewer/projects/rsync-archive` (greenfield; no commits yet) |
| **Companion / prior art** | [hilather/archiveconverter](https://github.com/hilather/archiveconverter) |
| **Language** | Rust 2021 (edition 2021+; target 1.70+) |
| **License** | MIT (proposed) |

---

## Overview

**rsync-archive** is a Rust CLI that builds **non-solid 7z** archives from source trees using **rsync-style path selection**, without staging a full copy of the tree. Sources are **stream-read**; members are **stream-compressed** (LZMA2) with bounded peak RAM and appended to a single non-solid archive. A separate **embed** command wraps multiple finished **regular files intended as `.7z` members** under a master/outer non-solid 7z using **store/Copy** (no recompression), reusing the proven outer **pack-append + header** pattern from archiveconverter.

The product is intentionally narrow for v1:

| Command | Role |
|---------|------|
| **`create`** | Select files → **compress** (LZMA2) → non-solid 7z |
| **`embed`** | Finished `.7z` blobs → **store** (Copy) → outer non-solid 7z |

Solid archives are **out of scope for create**. Embed **does not convert** solid→non-solid; store treats member bytes as opaque (no solid detection required in v1). Selection is a frozen rsync subset; writers are **concrete 7z types** in v1 (trait deferred until a second format).

---

## Background & Motivation

### Current state

- Repo is empty (only `.git`; no `Cargo.toml`, no source).
- Operators today typically: (1) rsync/filter to a temp tree, (2) `7z a -ms=off …`, or (3) hand-roll nested “archive of archives.” Temp trees burn disk and I/O; solid defaults make random access and remount expensive.
- **archiveconverter** solved **non-solid 7z pack-append writers** (placeholder 32-byte signature → append packs → end header → rewrite start signature), custom headers for 7zz / sevenz-rust2 / mounters, and outer **store** append of whole nested `.7z` blobs via Copy. Its LZMA2 writer appends **already-compressed** full packs—it does **not** stream-compress. Porting headers/store is prior art; **streaming LZMA2 encode is new work** for rsync-archive.

### Pain points this project addresses

| Pain | Response |
|------|----------|
| Full tree copy before pack | Stream read + stream compress write; prune excluded dirs early |
| Solid archives by default | Create always non-solid; solid convert stays in archiveconverter |
| Ad-hoc filter scripts | Rsync-style include/exclude/from-file parity in-process |
| Nested “master of 7zs” rebuild | Dedicated `embed` with Copy/store outer |
| Multi-GB single-file RAM blowups | Streaming LZMA2 encode with documented peak RAM |
| Docs drift under agent coding | AGENTS.md + keep-docs-current skill from day one |
| Features/fixes land without tests | AGENTS.md regression policy + keep-tests-current skill (K30) |

### What archiveconverter is *not*

archiveconverter converts **existing** solid nests → non-solid. rsync-archive **creates** archives from the filesystem and **embeds** finished 7zs. Shared value is the **header + pack-append + store outer** stack, not the convert pipeline and **not** streaming compression.

---

## Goals & Non-Goals

### Goals (v1)

1. **`create`**: rsync-style selection over `SRC...` → one non-solid 7z at `-o`.
2. **`embed`**: multiple finished regular files (intended as `.7z` members) → master outer 7z with Copy method; default warn on missing 7z magic; `--require-7z` / `--allow-any` for strict vs wide store.
3. **Streaming / disk-friendly**: no full-tree staging; **streaming LZMA2 encode for large files** (bounded peak RAM); early directory prune on exclude.
4. **Dry-run**: list selected archive paths without writing the archive (same selection path as write).
5. **Non-solid only (create)**: each content file is its own folder/stream.
6. **Concrete 7z writers first**: layout ready for a future `ArchiveWriter` trait; no mandatory trait in v1.
7. **Agent documentation policy**: `AGENTS.md`, `README.md`, full `.grok/skills/keep-docs-current/SKILL.md` as early deliverables; design stays external unless copied in later.
8. **Agent regression test policy**: every behavior-changing commit/PR must include automated tests (K30); full `.grok/skills/keep-tests-current/SKILL.md` in Stage 0.
9. **Testable filter engine**: unit parity against documented rsync rule behavior for the **frozen v1 subset**.
10. **Atomic outputs**: write `OUT.partial` (or same-dir tempfile), rename to `-o` only after successful `finish()`.
11. **Safe overwrite**: error if `-o` exists unless `--force`.

### Non-goals (v1)

- Solid 7z create or convert (including solid detection on embed inputs).
- In-place archive update / append to an existing finished 7z.
- Full rsync protocol (daemon, remote shell, delta transfer, `--link-dest`, etc.).
- `--merge`, dir-merge, per-directory `.rsync-filter`, protect/risk/hide/show.
- Compression methods beyond LZMA2 for create (Copy only for embed).
- Windows-first support (Linux primary; path normalization to `/`).
- Multi-volume 7z, encryption, 7zAES.
- Perfect byte-identical output vs official `7zz a`.
- Nested create-inside-create (use embed after separate creates).
- Empty directory preservation in the archive.
- Parallel encode (Stage 8+); default threads = 1 for v1.
- Mandatory multi-format `ArchiveWriter` trait.

---

## Key Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| K1 | **Two commands: `create` and `embed`** | Different I/O models (compress vs store). Keeps pipelines simple. |
| K2 | **Create always non-solid; embed does not convert solidity** | Random access for create. Embed store is opaque bytes; no solid reject in v1. Solid convert stays in archiveconverter. |
| K3 | **Port header + pack-append + store outer from archiveconverter; streaming LZMA2 encode is new** | Headers/store proven with 7zz / sevenz-rust2 / mounters. Companion does **not** stream-compress—rsync-archive owns streaming encode. |
| K4 | **Concrete SevenZ writers in v1; defer `ArchiveWriter` trait until second format** | Avoid dual trait/`push_packed` churn. Module layout stays ready for a thin trait later. |
| K5 | **Rsync filter subset as first selection dialect (frozen v1 list)** | Users know rsync rules; defer merge/dir-merge/.rsync-filter. |
| K6 | **Stream read + stream LZMA2 compress + append packs (no tree staging)** | Disk-friendly. Peak RAM = O(dict + I/O buffers), not O(file size). Required for multi-GB files before v1 ship. |
| K7 | **Embed uses Copy (method 0x00) for whole member blobs** | No recompress of finished nests; same as archiveconverter outer. |
| K8 | **Docs + AGENTS + full keep-docs-current skill in Stage 0** | Prevent README/CLI drift under agent coding. |
| K8b | **Tests policy + full keep-tests-current skill in Stage 0** | Same strength as docs policy; agents must not ship untested behavior. |
| K9 | **Library + binary crate** | `rsync_archive` lib; bin thin clap front-end. |
| K10 | **Create path layout: rsync-inspired SRC trailing-slash rules** | Explicit table below; only regular files packed. |
| K11 | **Default compression level 5; non-empty files always LZMA2 `0x21`; empty files via FilesInfo empty flags (no pack)** | Align with 7z/archiveconverter empty handling; no Copy for tiny non-empty files in v1. |
| K12 | **Empty archive is an error** (create/embed write) | Matches archiveconverter `finish()`. Dry-run of empty selection exits 0 with message. |
| K13 | **Sequential pack append for create v1** | Correctness first; parallel encode is Stage 8+. |
| K14 | **Deps: clap, sevenz-rust2 (read/test), lzma-rust2 (default), optional liblzma feature, crc32fast, thiserror, tracing, walkdir, tempfile. No rayon until Stage 8.** | Pure-rust default builds; no early MT dependency. |
| K15 | **Overwrite: error if `-o` exists unless `--force`** | Safer for archives; `--force` is a v1 flag. |
| K16 | **Atomic write: `OUT.partial` (same directory) → `finish()` → rename to `-o`** | Crash/Ctrl-C must not leave a fake finished archive at the final path. |
| K17 | **`--files-from` paths relative to CWD** (absolute paths allowed); see **K26** for full mode | One clear base; matches common CLI tools. |
| K18 | **No explicit directory members; empty dirs not preserved** | Paths imply parents; matches archiveconverter HeaderFile model. |
| K19 | **Embed v1: regular files; default warn if missing 7z magic; `--require-7z` hard-errors; `--allow-any` silences magic check for arbitrary store blobs** | Product is “master of 7zs” by default; escape hatch explicit. |
| K20 | **Fail-fast on I/O error reading a selected file** | No silent holes in archives; `--ignore-errors` later if needed. |
| K21 | **Only regular files selected for packing; skip symlinks, fifos, sockets, devices with warn counters** | Predictable create; dry-run lists only packed files (skips reported in summary/verbose). |
| K22 | **Hardlinks: archive as separate full content (no dedup)** | Simple; document. |
| K23 | **Level 0–9 map to encoder presets (lzma-rust2 / liblzma preset N)** | Documented mapping table; method_props = single LZMA2 props byte. |
| K24 | **v1 parallelism default remains 1 when Stage 8 lands until proven** | Avoid surprising RAM/CPU; auto threads later. |
| K25 | **Design doc lives external (`/tmp/...` / process artifact); not required in-repo for Stage 0. Optional later `docs/DESIGN.md`.** | AGENTS says “README is source of truth for behavior; design may be external.” |
| K26 | **`--files-from` mode is exclusive of `SRC...`** | If `--files-from` is set, `SRC...` must be empty (CLI error if both). Files-only lines; `archive_name` from path-as-written rules below. |
| K27 | **Rsync no-`/` patterns match basename** | If pattern (after dir-only trailing-`/` strip) contains no `/`, match against the final path component only—so `*.tmp` matches `dir/a.tmp`. Patterns with `/` match the full relative path. |
| K28 | **Create mtime from source file metadata at open** | Header mtime = `modified()` from non-following metadata; win attrs stay default file attrs in v1 (no full Unix mode port). |
| K29 | **Collision pre-scan before any pack write** | Build full `SelectedEntry` list (or complete name set) and reject duplicate `archive_name` before opening the partial output for pack data. Dry-run uses the same check. |
| K30 | **Every behavior-changing commit must include regression tests in the same change** (same commit preferred; same PR required before merge). No feature or fix lands without automated coverage. *(Added per user requirement for agent test discipline.)* | Agents otherwise ship “manually tested” gaps; filter/writer bugs regress silently. Enforced via AGENTS.md + keep-tests-current skill; CI runs `cargo test` but agents own writing tests. |

---

## Proposed Design

### High-level architecture

```text
┌─────────────────────────────────────────────────────────────────┐
│  CLI (clap)                                                      │
│  rsync-archive create | embed | (future: list)                   │
└───────────────┬─────────────────────────────┬───────────────────┘
                │                             │
                ▼                             ▼
┌───────────────────────────┐   ┌─────────────────────────────────┐
│  select/                  │   │  pipeline/embed.rs              │
│  RuleSet, Matcher, Walk   │   │  name resolve + magic check     │
│  → SelectedEntry stream   │   │  → NonsolidStoreWriter          │
└─────────────┬─────────────┘   └────────────────┬────────────────┘
              │                                  │
              ▼                                  │
┌───────────────────────────┐                    │
│  pipeline/create.rs       │                    │
│  partial path + rename    │                    │
│  stream encode per file   │                    │
└─────────────┬─────────────┘                    │
              ▼                                  │
┌───────────────────────────┐                    │
│  archive/sevenz/          │◄───────────────────┘
│    header.rs              │  write_raw_header / start sig
│    store_writer.rs        │  NonsolidStoreWriter (Copy) — embed
│    lzma2_writer.rs        │  streaming member API — create
│    codec.rs               │  StreamingLzma2Encoder
│  (traits.rs deferred)     │
└───────────────────────────┘
```

### Module layout (proposed)

```text
rsync-archive/
├── AGENTS.md
├── README.md
├── LICENSE
├── Cargo.toml
├── .grok/skills/keep-docs-current/SKILL.md
├── .grok/skills/keep-tests-current/SKILL.md
├── docs/
│   └── SELECTION.md            # filter semantics (from Stage 4)
├── src/
│   ├── main.rs
│   ├── lib.rs
│   ├── cli.rs                  # CreateArgs, EmbedArgs (frozen flags per stage)
│   ├── error.rs
│   ├── select/
│   │   ├── mod.rs
│   │   ├── rules.rs            # parse include/exclude/filter +/-
│   │   ├── matcher.rs          # ordered rule evaluation + prune predicate
│   │   ├── from_file.rs        # include-from, exclude-from, files-from
│   │   ├── walk.rs
│   │   └── pathnorm.rs
│   ├── archive/
│   │   ├── mod.rs
│   │   └── sevenz/
│   │       ├── mod.rs
│   │       ├── header.rs
│   │       ├── lzma2_writer.rs # streaming create writer
│   │       ├── store_writer.rs # embed Copy writer
│   │       └── codec.rs        # StreamingLzma2Encoder + level presets
│   ├── pipeline/
│   │   ├── mod.rs
│   │   ├── create.rs
│   │   ├── embed.rs
│   │   ├── output.rs           # partial path, force check, rename
│   │   └── verify.rs
│   └── util/
│       └── mod.rs
└── tests/
    ├── filter_parity.rs
    ├── e2e_create.rs
    ├── e2e_embed.rs
    ├── streaming_large.rs      # large-file RAM/streaming contract
    └── compare_7zz.rs
```

### Create pipeline (sequence)

```mermaid
sequenceDiagram
    participant CLI
    participant Select
    participant Walk
    participant Create
    participant Enc as StreamingLzma2Encoder
    participant Writer as NonsolidLzma2Writer
    participant FS

    CLI->>Select: build RuleSet from flags/files
    CLI->>Create: run(sources, rules, out, dry_run, force)
    alt dry_run
        Create->>Walk: selected regular files only
        Walk-->>Create: archive names
        Create-->>CLI: print listing + skip summary, exit 0
    else write
        Create->>FS: refuse if out exists and not force
        Create->>Writer: create(out.partial) placeholder 32B sig
        loop each selected regular file
            Create->>Walk: next entry (prune dirs)
            Create->>FS: open O_NOFOLLOW / metadata; must be regular
            Note over Create: Pre-scan complete: full entry list, collisions checked (K29)
            Create->>Writer: start_file(name, mtime)
            loop read chunks (e.g. 256 KiB)
                Create->>Create: content_crc.update(chunk)
                Create->>Enc: feed(chunk) → same LZMA2 stream bytes
                Create->>Writer: write_pack_bytes(compressed)
            end
            Create->>Enc: finish → trailing pack + props
            Create->>Writer: finish_file(unpack_size, content_crc, pack_crc, props)
        end
        Create->>Writer: finish() end header + rewrite sig
        Create->>FS: rename out.partial → out
        Create-->>CLI: ok / optional --verify
    end
```

### Embed pipeline (sequence)

```mermaid
sequenceDiagram
    participant CLI
    participant Embed
    participant Store as NonsolidStoreWriter
    participant FS

    CLI->>Embed: out, inputs, naming, require_7z, allow_any, force
    Embed->>FS: refuse if out exists and not force
    Embed->>Store: create(out.partial)
    loop each input
        Embed->>Embed: resolve member name; collision check
        Embed->>FS: regular file check; read magic
        Embed->>Store: push_path(member_name, path) stream Copy
    end
    Embed->>Store: finish()
    Embed->>FS: rename out.partial → out
```

### Non-solid 7z write model (ported pack-append)

Reuse archiveconverter’s **on-disk** layout (not its whole-buffer compress API):

1. Create **partial** file; write **32-byte zero placeholder** (`SIG_HEADER_SIZE`).
2. For each member, **append pack stream** (streaming LZMA2 output or Copy bytes).
3. Track `HeaderFile { name, pack_size, pack_crc, unpack_size, content_crc, method_id, method_props, empty }`.
4. On `finish()`: `write_raw_header` → append end header → CRC → `write_start_header` → seek 0 rewrite signature.
5. **Rename** partial → final `-o` only after successful `finish()`.

| Kind | method_id | Notes |
|------|-----------|--------|
| Create non-empty | `[0x21]` LZMA2 | `method_props` = 1-byte LZMA2 props; pack CRC of compressed bytes; content CRC of uncompressed stream |
| Create empty | (no pack) | `empty: true`; FilesInfo empty stream/file bits |
| Embed | `[0x00]` Copy | pack_size == unpack_size; pack_crc == content_crc |

### Streaming LZMA2 encode contract (v1 hard requirement)

**v1 create MUST support multi-GB files without loading the whole file into RAM.**

#### API (concrete, not trait)

```rust
/// Streaming LZMA2 encoder for one archive member.
pub struct StreamingLzma2Encoder { /* codec state, level, running pack_crc */ }

impl StreamingLzma2Encoder {
    pub fn new(level: u8) -> Result<Self>; // level 0..=9 → preset

    /// Feed uncompressed bytes. Returns compressed bytes ready to append
    /// to the pack stream (may be empty if codec buffers).
    pub fn feed(&mut self, uncompressed: &[u8]) -> Result<Vec<u8>>; // or write to &mut dyn Write

    /// Finish member: flush codec; return trailing pack bytes + props byte.
    pub fn finish(self) -> Result<Lzma2Finish> {
        // Lzma2Finish { trailing: Vec<u8>, props: u8 }
    }
}

/// Meta recorded when starting a member (create).
pub struct CreateFileMeta {
    pub name: String,
    pub mtime: Option<SystemTime>, // from source metadata at open (K28)
}

/// Non-solid create writer with streaming member API.
impl NonsolidLzma2Writer {
    pub fn create(path: &Path) -> Result<Self>; // writes placeholder sig
    pub fn start_file(&mut self, meta: CreateFileMeta) -> Result<()>;
    /// Append already-compressed pack bytes for current member.
    /// Bytes are fragments of **one** LZMA2 codestream for this member (not separate archives).
    pub fn write_pack_bytes(&mut self, compressed: &[u8]) -> Result<()>;
    pub fn finish_file(
        &mut self,
        unpack_size: u64,
        content_crc: u32,
        pack_crc: u32, // of full pack stream for this member
        props: u8,
    ) -> Result<()>;
    /// Empty file: no pack bytes; records empty HeaderFile (mtime still set if known).
    pub fn push_empty_file(&mut self, meta: CreateFileMeta) -> Result<()>;
    pub fn finish(self) -> Result<()>; // end header + rewrite sig
}
```

Optional internal helper for tests/small fixtures only:

```rust
// NOT used as the sole create path for arbitrary sizes:
fn push_packed_for_tests(name, Lzma2Compressed) { ... }
```

#### One LZMA2 pack stream per member (wire format)

For each non-empty create member, the pack payload is **exactly one** LZMA2 codestream (method `0x21`), which may contain multiple **internal** LZMA2 chunks as the encoder emits them. Readers (7zz / sevenz-rust2) decode that single stream to recover the full file.

| Allowed | Forbidden |
|---------|-----------|
| One streaming encoder instance per member: `feed`/`finish` producing incremental compressed bytes of the **same** codestream | Calling independent whole-buffer `compress(chunk_i)` for each read chunk and **concatenating** those outputs as one pack (N separate LZMA2 blobs) |
| Encoder dictionary/state carried across `feed` calls | Resetting the encoder per chunk while still writing one pack |
| Optional Stage 6 size-gated path: one `compress(entire_file)` → one pack (still one stream) | Holding full multi-GB plaintext **and** claiming “streaming” |

**Stage 6b spike (before or with PR7b):** prove `lzma-rust2` (or feature `liblzma`) can emit incremental LZMA2 chunk bytes for one ongoing stream. If pure-rust cannot, enable/require `liblzma` for Stage 6b or document the binding choice **before** PR7 lands size-gate-only create as the only path.

#### Peak RAM formula (document in README)

```text
peak_ram ≈
    encoder_dict_size(level)   // LZMA2 dictionary for preset (one stream)
  + read_buf                   // fixed, e.g. 256 KiB
  + encode_out_buf             // fixed or small multiple of read_buf
  + header_metadata            // O(num_files) names + HeaderFile rows
  + OS page cache (not process heap)
```

**Not allowed as v1 sole path:** `read_to_end` entire file into `Vec<u8>` then `compress(&[u8])` for unbounded sizes.

#### Interim implementation gate (only if streaming encode lands mid-v1)

If Stage 6 lands a size-gated buffer first:

| File size | Behavior |
|-----------|----------|
| `size == 0` | empty flags |
| `0 < size ≤ STREAM_THRESHOLD` (e.g. 64 MiB) | may buffer for simplicity → **still one** LZMA2 stream per member |
| `size > STREAM_THRESHOLD` | **hard error** with message pointing to streaming Stage 6b |

**v1 ship criterion:** Stage **6b (streaming encode)** is **mandatory before calling the product v1**. Stage 6 without 6b is intermediate only.

#### Content / pack CRC while streaming

- `content_crc = crc32fast` over uncompressed bytes as they are read (running hasher).
- `pack_crc = crc32fast` over all compressed bytes written for that member (running hasher in writer or encoder).
- `unpack_size` = total uncompressed bytes fed.
- `pack_size` = total compressed bytes appended.

#### Create mtime (K28)

- On open (after regular-file / non-follow checks), read `modified()` (or equivalent) from metadata.
- Pass into `CreateFileMeta.mtime`; header writer emits Windows FILETIME from that value (same conversion approach as archiveconverter `filetime_now` but from source time).
- If mtime is unavailable, omit or use a documented fallback (epoch); do **not** silently use “now” for every file (that would make every rebuild look fully touched).
- Mode/owner: v1 keeps default win attributes (`ATTR_FILE`); full Unix mode port is later.

#### Level → preset mapping (v1)

| `--level` | Encoder preset | Notes |
|-----------|----------------|--------|
| 0 | preset 0 | Fastest / smallest dict |
| 1–9 | preset N | Match lzma-rust2 / liblzma preset N as closely as practical |
| default | **5** | |

Exact dict sizes follow the chosen crate’s preset table; document the crate version and props byte source in rustdoc. Tests: compress/decompress roundtrip at levels 0, 1, 5, 9 for a fixed fixture.

### Atomic output & overwrite (`pipeline/output.rs`)

```text
fn prepare_output(out: &Path, force: bool) -> Result<PartialPath>:
  if out.exists() && !force:
    return Err(OutputExists)
  partial = out.with_extension( out.extension + ".partial" )
    // e.g. out.7z → out.7z.partial  (or "{out}.partial")
  if partial.exists(): remove or error (prefer remove if force or stale)
  return PartialPath { partial, final: out }

// on success:
writer.finish()?;
fs::rename(partial, final)?;

// on failure / Ctrl-C:
// leave partial for debugging OR delete; never rename
// never write placeholder-only content at final path
```

**Naming:** prefer `{out}.partial` (e.g. `archive.7z.partial`) in the same directory as `-o` so `rename` is atomic on the same filesystem.

### Directory handling

- **Only regular files** become members.
- Directories are walk nodes only; **empty directories are not preserved** in v1.
- Nested paths like `sub/dir/x.txt` work via the file name string; no explicit dir entries.

---

## API / Interface Changes

Greenfield — all new. **v1 uses concrete types**, not a mandatory trait.

### Create: concrete streaming writer (above)

`pipeline/create.rs` calls `NonsolidLzma2Writer` + `StreamingLzma2Encoder` directly.

### Embed: concrete store writer

```rust
impl NonsolidStoreWriter {
    pub fn create(path: &Path) -> Result<Self>;
    pub fn push_path(&mut self, name: String, src: &Path) -> Result<()>; // 256 KiB read loop
    pub fn push_bytes(&mut self, name: String, data: &[u8]) -> Result<()>;
    pub fn finish(self) -> Result<()>;
}
```

Port from archiveconverter `store_writer.rs` patterns (including streaming `push_path`).

### Future trait (deferred — Stage 9 / second format)

When tar (or another format) is added:

```rust
// Deferred — do not implement in Stage 6
pub trait CompressingArchiveWriter { /* start_file / write / finish_file / finish */ }
pub trait StoredArchiveWriter { /* push_path / finish */ }
```

### Selection API

```rust
pub struct RuleSet { /* ordered rules */ }

pub enum RuleAction { Include, Exclude }

pub struct Rule {
    pub action: RuleAction,
    pub pattern: Pattern, // v1: * ? ** anchored dir-only
}

impl RuleSet {
    pub fn from_cli(...) -> Result<Self>;
    pub fn push_filter_line(&mut self, line: &str) -> Result<()>; // "+ pat" / "- pat"
    pub fn action_for(&self, rel_path: &str, is_dir: bool) -> RuleAction;
    /// True iff directory D should not be descended into.
    pub fn should_prune_dir(&self, dir_rel: &str) -> bool;
}

pub struct SelectedEntry {
    pub abs_path: PathBuf,
    pub archive_name: String, // normalized /
    pub size: u64,            // regular files only
}

// Yields only regular files that are Included; errors on fatal walk I/O.
pub fn walk_selected(
    sources: &[SourceSpec],
    rules: &RuleSet,
) -> impl Iterator<Item = Result<SelectedEntry>>;
```

### CLI sketch (clap) — v1 freeze

```text
rsync-archive create -o OUT.7z [OPTIONS] SRC...
  -n, --dry-run
  --force                      # overwrite -o if exists
  --exclude PATTERN            # repeatable
  --include PATTERN            # repeatable
  --exclude-from FILE
  --include-from FILE
  --files-from FILE            # exclusive of SRC...; paths relative to CWD; absolute OK (see K26)
  --filter RULE                # repeatable; "+ pattern" or "- pattern"
  --level 0-9                  # default 5
  --verify                     # post-write test via sevenz-rust2 (or 7zz if configured)
  -v / -vv

  # NOT v1: --merge, --ffilter, --cvs-exclude, -0/--from0 (may add later)

rsync-archive embed -o MASTER.7z [OPTIONS] FILE...
  -n, --dry-run
  --force
  --prefix PREFIX/             # optional; must be relative, no ".."
  --keep-path                  # use normalized path as member name (overrides default flatten)
  --require-7z                 # hard-error if missing 7z magic
  --allow-any                  # allow non-7z store; skip magic warning
  --verify
  -v / -vv

  # Default naming: flatten to basename (no --flatten flag needed)
```

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0 | Success (including dry-run with zero files, with message) |
| 1 | Operational error (I/O, empty write selection, collision, missing magic with `--require-7z`, output exists without `--force`) |
| 2 | CLI usage error |

No partial-success multi-code scheme in v1 (unlike rsync 23/24).

---

## Data Model Changes

No database. On-disk artifacts:

| Artifact | Format |
|----------|--------|
| Create/embed final output | Single non-solid multi-file 7z at `-o` |
| Partial during write | `{out}.partial` same directory; removed or left on failure |
| Filter list files | Text; `#` comments; blank skip; patterns or `+`/`-` filter lines |

### Embed member naming (complete)

**Algorithm:**

```text
fn member_name(input: &Path, keep_path: bool, prefix: Option<&str>) -> Result<String>:
  base = if keep_path:
           normalize_keep(input)
         else:
           basename_utf8(input)?   // error if ends with / or basename is . or ..

  if base.is_empty() || base == "." || base == "..":
    return Err(InvalidMemberName)

  if let Some(p) = prefix:
    p = normalize_prefix(p)?     // relative; no leading /; no ..; \ → /; ensure trailing / optional → join
    member = join_archive(p, base)
  else:
    member = base

  validate_member(member)?       // non-empty; no NUL; UTF-8; no ".." segments; no leading /
  return member
```

**`normalize_keep(path)`:**

1. Lossy-or-error: path must be valid UTF-8 (v1: **error on non-UTF8**).
2. `\` → `/`; strip leading `/` and drive-style prefixes if any.
3. Collapse `//` → `/`; strip `./` segments.
4. **Reject** if any `..` segment remains.
5. Result non-empty.

**`normalize_prefix`:** same safety rules; reject absolute and `..`; if non-empty and not ending in `/`, append `/` before join (or join with `/` consistently).

**Flag precedence:**

| Flag | Effect |
|------|--------|
| (default) | Flatten to basename |
| `--keep-path` | Use normalized full path (overrides default flatten) |
| `--prefix` | Prepended to whichever base was chosen |
| No `--flatten` flag | Default is flatten; do not ship redundant `--flatten` |

**Collisions:**

- Member names compared as exact UTF-8 strings (case-sensitive).
- Two inputs → same member name ⇒ **error** (including dry-run).
- On case-insensitive filesystems, two different members `A.7z` and `a.7z` are **allowed** in the archive (7z is case-sensitive); extracting both to the same FS may collide—document, do not merge names.

**Order:** CLI argument order = archive member order.

**Magic / type validation:**

| Check | Default | `--require-7z` | `--allow-any` |
|-------|---------|----------------|---------------|
| Must be regular file | error | error | error |
| 7z magic `7z\xBC\xAF\x27\x1C` | **warn**, continue | **error** | skip check |
| Extension `.7z` | not required | not required (content magic only) | n/a |
| Solid detection | not performed | not performed | not performed |

`--require-7z` and `--allow-any` are mutually exclusive (CLI error if both).

### Create archive path layout

| SRC form | Example SRC | File on disk | Archive name |
|----------|-------------|--------------|--------------|
| Directory, no trailing `/` | `photos` | `photos/a.jpg` | `photos/a.jpg` |
| Directory, trailing `/` | `photos/` | `photos/a.jpg` | `a.jpg` |
| Single regular file | `/data/x.bin` | that file | `x.bin` (basename) |
| Single regular file relative | `dir/x.bin` | that file | `x.bin` (basename) |
| Multi SRC | `a/` `b/` | `a/f`, `b/f` | both map to `f` → **collision error** |
| Multi SRC | `a` `b` | `a/f`, `b/f` | `a/f`, `b/f` (OK) |
| Multi SRC files | `p/x` `q/x` | two files | both `x` → **collision error** |

**Rules:**

1. Always `/`-normalized archive names; reject `..` escape outside SRC root when walking.
2. **Only regular files** enter the selection set for packing.
3. **Symlinks:** do not follow; skip with `skipped_symlinks` counter; **not listed** as members in dry-run (verbose/debug may log skips).
4. **Special files** (fifo, socket, device): skip with `skipped_special` counter.
5. **Hardlinks:** each path archived as independent content (no dedup).
6. **Empty directories:** not represented in the archive.
7. **Unreadable / I/O error** on a selected regular file: **fail the job** (exit 1); no skip-by-default.
8. **Open policy:** after walk, reopen with non-following metadata (`symlink_metadata` / `O_NOFOLLOW` where available); if not a regular file at open time, skip with warn (TOCTOU) or fail—**v1: treat as error** to avoid racing symlink plants replacing a file.

### SourceSpec

```rust
pub struct SourceSpec {
    pub path: PathBuf,       // as given
    pub trailing_slash: bool,
    pub kind: SourceKind,    // File | Dir (from metadata at plan time)
}
```

---

## Rsync filter semantics — v1 vs later

### v1 must ship (frozen)

| Feature | v1 |
|---------|----|
| `--exclude=PATTERN` | **Must** |
| `--include=PATTERN` | **Must** |
| `--exclude-from=FILE` | **Must** |
| `--include-from=FILE` | **Must** |
| `--files-from=FILE` | **Must** — exclusive of `SRC...`; paths relative to **CWD**; absolute OK; files only; archive names per K26 |
| `--filter='+ …'` / `- …` | **Must** (basic only) |
| Pattern: `*`, `**`, `?` | **Must** |
| Anchored `/pattern` | **Must** — match from start of relative path under current SRC mapping |
| Directory-only `pattern/` | **Must** — matches directories; see match algorithm |
| First-match-wins | **Must** |
| Default action if no rule matches | **Include** |
| Include-only idiom | User writes `--include='*.c' --exclude='*'` (order matters) |
| `--dry-run` | **Must** |
| Early prune | **Must** — algorithm below |
| Filter/from file size cap | **Must** — max **10 MiB** or **1_000_000 lines** per file; error if exceeded |

### v1 deferred (do not implement)

| Feature | Status |
|---------|--------|
| `--merge` / `--ffilter` | Deferred |
| Per-directory `.rsync-filter` / dir-merge | Deferred |
| protect / risk / hide / show | Deferred |
| `--cvs-exclude` | Deferred |
| `-0` / `--from0` | Deferred (v1.1 candidate) |
| `--max-size` / `--min-size` | Deferred (Stage 9) |
| Regex excludes | Deferred |
| Charset / Windows long paths | Deferred |

### Pattern match algorithm (v1)

Paths for matching are **archive-relative** (after SRC mapping), `/`-separated, with **no** leading `/` on the path under test (e.g. `dir/a.tmp`, not `/dir/a.tmp`).

For rule pattern `pat` (K27):

1. **Dir-only flag:** if `pat` ends with `/`, set `dir_only = true` and strip the trailing `/` for subsequent steps (empty after strip is invalid → parse error).
2. **Anchored flag:** if `pat` starts with `/`, set `anchored = true` and strip the leading `/`.
3. **Match target (rsync core rule):**
   - If the remaining `pat` **contains no `/`** → match against the path’s **basename only** (final component). Example: `*.tmp` matches `dir/a.tmp` because basename `a.tmp` matches.
   - If the remaining `pat` **contains `/`** → match against the **full** relative path. Example: `dir/*` matches `dir/x` but not `dir/sub/x` (`*` does not cross `/`).
4. **Anchored + full-path patterns:** when `anchored` and the pattern has `/` (or is a single segment after strip), the full-path match is constrained to the start of the relative path (rsync anchored). When `anchored` and basename-mode (no `/` in pat), match basename **only if** the path is a single-segment path (i.e. `/foo` matches `foo` but not `bar/foo`).
5. **Dir-only application:**
   - Against a **directory** `D`: pattern matches if the (basename or full-path) rule matches `D`.
   - Against a **file**: a dir-only pattern also matches if any ancestor directory of the file would match as a directory, or if the file path is under a matched directory prefix (e.g. `foo/` excludes `foo/bar`). Exact file `foo` (non-dir) is not excluded by `foo/` alone.
6. **Wildcards:**
   - `*` — one path segment (no `/`)
   - `**` — across segments (including empty)
   - `?` — one character except `/`

**First match wins** among ordered rules; if none match → **Include**.

### Prune algorithm (v1)

When walk reaches directory with relative path `D` (no trailing slash in `D`):

```text
fn should_prune_dir(rules, D) -> bool:
  act = rules.action_for(D, is_dir=true)
  if act == Include:
    return false

  // D is Excluded. Over-approx: do not prune if ANY Include rule
  // (any position in the list) might match D or a path under D.
  for rule in rules where rule.action == Include:
    if include_rule_may_match_under(rule.pattern, D):
      return false
  return true

fn include_rule_may_match_under(pat, D) -> bool:
  // Return true unless we can prove no path "D" or "D/..." can match.
  // Always true (do not prune) when:
  //   - pat (basename-mode) could match some basename under D (e.g. "*.c", "**", "*")
  //   - pat contains "**"
  //   - pat is full-path and has a prefix compatible with D or D + "/"
  // Return false only when clearly impossible, e.g.:
  //   - anchored full-path "/other/..." where first segment != D's first segment
  // Prefer false negatives on prune (walk more) over false positives (skip files).
```

**Invariant:** Prefer **walking too much** over missing Included files.

### `--files-from` mode (K26)

**Mutual exclusion with `SRC...`:**

| Invocation | Result |
|------------|--------|
| `create -o out.7z SRC...` (no `--files-from`) | Normal SRC walk mode |
| `create -o out.7z --files-from list` (no SRC) | Files-from mode |
| `create -o out.7z --files-from list SRC...` | **CLI error** (exit 2): cannot combine |
| `create -o out.7z` (neither) | **CLI error**: need SRC or `--files-from` |

**Line processing:**

1. Read lines; ignore `#` comments and blanks; enforce 10 MiB / 1M line caps.
2. Each non-empty line is a path: relative to **CWD**, or absolute.
3. Resolve to a filesystem path; must exist and be a **regular file** — if directory or missing or special, **error** (clear message). No trailing-slash SRC rules in this mode.
4. Build `SelectedEntry`:
   - `abs_path` = canonicalized or absolute resolved path used for open/read.
   - `archive_name` = **files-from naming rule** (below).
5. Run `RuleSet::action_for(archive_name, is_dir=false)` (and/or path form used consistently—**v1: match filters against `archive_name`**). Exclude drops the entry.
6. Collision: duplicate `archive_name` after naming → **error** (same as multi-SRC).

**`archive_name` from files-from line (v1):**

| Line as written | `archive_name` |
|-----------------|----------------|
| Relative `a/x.txt` | `a/x.txt` after `normalize_keep` (strip `./`, collapse `//`, `\`→`/`, reject `..`) |
| Relative `x.txt` | `x.txt` |
| Absolute `/data/a/x.txt` | **basename only** → `x.txt` (avoid leaking host absolute paths into the archive) |

Examples:

- List `foo/a.txt` and `bar/a.txt` → members `foo/a.txt`, `bar/a.txt` (OK).
- List `/tmp/a.txt` and `/var/a.txt` → both basename `a.txt` → **collision error**.
- List `a.txt` twice → collision error.

Trailing-slash SRC semantics **do not apply** in files-from mode.

### Representative parity cases (Stage 4 matcher)

| # | Rules (ordered) | Path | is_dir | Expect |
|---|-----------------|------|--------|--------|
| 1 | exclude `*.tmp` | `a.tmp` | F | Exclude |
| 2 | exclude `*.tmp` | `a.txt` | F | Include |
| 3 | exclude `*.tmp` | `dir/a.tmp` | F | **Exclude** (basename rule) |
| 4 | exclude `dir/` | `dir` | T | Exclude |
| 5 | exclude `dir/` | `dir/x` | F | Exclude |
| 6 | include `*.c` then exclude `*` | `a.c` | F | Include |
| 7 | exclude `*` then include `*.c` | `a.c` | F | Exclude |
| 8 | exclude `/foo` | `foo` | F | Exclude |
| 9 | include `/foo` | `foo` | F | Include |
| 10 | exclude `/foo` | `bar/foo` | F | Include (no match; default) |
| 11 | exclude `**/*.o` | `x/y.o` | F | Exclude |
| 12 | exclude `?.txt` | `a.txt` | F | Exclude |
| 13 | exclude `?.txt` | `ab.txt` | F | Include |
| 14 | include `sub/**` then exclude `*` | `sub/a` | F | Include |
| 15 | exclude `*` only | `any` | F | Exclude |
| 16 | (no rules) | `any` | F | Include |
| 17 | filter `- *.log` then `+ keep.log` | `keep.log` | F | Exclude (first match) |
| 18 | filter `+ keep.log` then `- *.log` | `keep.log` | F | Include |
| 19 | exclude `foo` (no slash) | `foo` | F | Exclude |
| 20 | exclude `foo` (no slash) | `bar/foo` | F | **Exclude** (basename `foo`) |
| 21 | exclude `bar/foo` | `bar/foo` | F | Exclude |
| 22 | exclude `bar/foo` | `foo` | F | Include (no match) |
| 23 | exclude `dir/*` | `dir/x` | F | Exclude |
| 24 | exclude `dir/*` | `dir/sub/x` | F | Include (`*` one segment) |
| 25 | include-from with `#` comments | (parse) | — | comments ignored |

Walk / files-from / collision cases (Stage 5):

| Case | Expect |
|------|--------|
| exclude `skipme/` with canary `skipme/secret` | canary never opened |
| exclude `skipme/` then include `skipme/keep.txt` (exclude first) | keep not selected; may prune |
| include `skipme/keep.txt` then exclude `skipme/` | keep selected; no prune of `skipme` if include may match under |
| files-from lists `a.txt` + exclude `*.txt` | empty selection (write fails; dry-run ok) |
| files-from `foo/a.txt` + `bar/a.txt` | members `foo/a.txt`, `bar/a.txt` |
| files-from two absolutes same basename | collision error before write |
| multi-SRC `a/` `b/` both have `f` | collision error before write |
| `--files-from` + `SRC` together | CLI error exit 2 |

---

## Alternatives Considered

### A1. Shell out to `rsync --files-from` + `7zz a`

| Pros | Cons |
|------|------|
| Instant filter parity | Temp lists; dual dependency; weak control of non-solid streaming writer |

**Rejected** for core path.

### A2. Stage full filtered tree under `tempfile` then pack

| Pros | Cons |
|------|------|
| Simple | 2× disk; violates streaming goals |

**Rejected**.

### A3. Depend on `archiveconverter` as crate

| Pros | Cons |
|------|------|
| Less port | Not a stable library API; wrong product surface |

**Rejected**; port header/store patterns.

### A4. Use only `sevenz-rust2` high-level compress API

| Pros | Cons |
|------|------|
| Less code | Less control of solid vs stream-append and headers |

**Decision:** sevenz-rust2 for **read/list/test**; **write** via custom non-solid writers.

### A5. Single command with mode flag vs subcommands

**Rejected** in favor of `create` / `embed`.

### A6. In-process filter engine + shell `7zz a -ms=off` (listfile / `-i@`)

| Pros | Cons |
|------|------|
| Official packer | Still process boundary; hard to guarantee streaming/no-temp; library goals fail; non-solid flags vary by 7z build |

**Rejected** for core create; optional compare tests may shell `7zz`.

### A7. Create as uncompressed tar + external xz

| Pros | Cons |
|------|------|
| Simple streaming | Different product (not 7z); embed story breaks |

**Rejected** for v1 (possible later format).

---

## Security & Privacy Considerations

| Threat | Severity | Mitigation |
|--------|----------|------------|
| Path traversal in archive names (`../`) | High | Normalize; reject `..`; validate member names |
| Symlink escape when reading sources | High | Never follow symlinks; `symlink_metadata` / `O_NOFOLLOW` on open; skip non-regular |
| TOCTOU (file replaced by symlink after walk) | High | Re-check type on open; error if not regular |
| Filter/files-from DoS (huge lists) | Medium | Cap 10 MiB / 1M lines per list file |
| Output exists / clobber | Medium | Error unless `--force`; write partial then rename |
| Partial file left world-readable | Low | Same umask as final; document; user responsibility |
| Zip/7z bomb on verify extract | Medium | Verify uses test/list, not full extract of untrusted nests |
| Secrets in trees | Medium | User responsibility; dry-run audits selection |
| Embed of untrusted blobs | Low | Store only; no extract by default |

No credentials or network protocol in v1.

---

## Observability

- **tracing** with `RUST_LOG` / `-v` / `-vv`: error, warn, info, debug, trace.
- Info: start/finish, counts, compression ratio, skip counters.
- Debug: each selected path, prune decisions, rule that matched, magic warnings.
- Trace: codec props, header sizes, chunk feed sizes.
- End summary always printed for create/embed write:

```text
created out.7z: 12480 files, 3.2 GiB → 1.1 GiB (level 5), 412 s (skipped: 3 symlinks, 1 special)
```

### `--verify` definition (v1)

1. Reopen final path with **sevenz-rust2** (preferred) or `7zz t` if configured.
2. Assert archive opens / tests clean.
3. Assert **non-solid** if API available (`is_solid == false`).
4. Assert **file member count** equals number of members written (create: selected files; embed: inputs).
5. Do **not** require full extract of all members; e2e tests may extract samples for byte identity.

### Counters

`selected`, `pruned_dirs`, `skipped_symlinks`, `skipped_special`, `bytes_in`, `bytes_out`, `magic_warnings`.

---

## Rollout Plan

1. **Stage 0–1**: docs + **tests** agent policies + crate skeleton + foundations.
2. **Stage 2–3**: headers + store + embed (CLI freeze for embed).
3. **Stage 4–5**: frozen filter engine + walk + dry-run.
4. **Stage 6**: create writer + pipeline (may size-gate briefly).
5. **Stage 6b**: **streaming LZMA2 encode** — **required for v1**.
6. **Stage 7**: verify, polish, optional 7zz compare, acceptance script.
7. **Stage 8+**: parallel encode, liblzma feature, extended filters, optional trait/tar.

**Cargo features:**

| Feature | Default | Purpose |
|---------|---------|---------|
| (default) | pure `lzma-rust2` | No `lzma-sys` |
| `liblzma` | off | Faster codec via system/lib bindings |

**No rayon** until Stage 8.

**CI (when added):** every PR runs `cargo test` (and `cargo build`). CI **enforces that tests pass**; **agents still own writing** the regression tests required by K30/AGENTS. Missing tests are a process/review failure, not something CI can invent.

**Rollback:** linear history; each PR mergeable independently within deps.

---

## Agent documentation & testing policy design

> **User requirement (2026-07-28):** Agents must always add regression tests for all features and fixes. All commits that change behavior must be covered by tests. Encoded as **K30** and the AGENTS section below.

### Design doc location

- This design may live **outside** the repo (process artifact).  
- **AGENTS.md** must state: behavior source of truth is **README + `src/cli.rs` + tests**; optional `docs/DESIGN.md` only if someone commits it later.  
- Do **not** require committing this `/tmp` design for Stage 0 exit.

### `AGENTS.md` (required content)

1. Project one-liner and non-goals (no solid create; embed does not convert; no full rsync; create compresses / embed stores).
2. **Non-negotiable docs currency** for every user-visible change.
3. **Non-negotiable: tests cover every change** (full section below — same strength as docs policy).
4. Checklist table: CLI flag → README; selection → README + `docs/SELECTION.md` + tests; writer/streaming → README architecture; module map.
5. Project map.
6. Defaults that must stay accurate: non-solid create; level 5; embed flatten default; empty archive error; symlink skip; `--force` overwrite; partial+rename; files-from CWD; streaming create.
7. Build/test commands (`cargo test` mandatory before done).
8. Pointers to **keep-docs-current** and **keep-tests-current** skills.

#### AGENTS.md section: Non-negotiable — tests cover every change

Mandatory policy text for the eventual `AGENTS.md` (agents must treat this as binding):

**Every commit that changes behavior must be covered by automated tests.**

Rules:

1. Every commit that changes **user-visible behavior**, **selection/filter semantics**, **CLI flags/defaults**, **archive write/embed correctness**, **error handling**, or **bug fixes** MUST add or update automated tests in the **same change** (same commit preferred; **same PR required** before merge).
2. **New features** require: unit tests for pure logic **and** at least one integration/e2e test that exercises the CLI or public API path when applicable.
3. **Bug fixes** require a **regression test that fails before the fix and passes after** (red–green). Prefer a minimal fixture that reproduces the bug.
4. **Refactors with no behavior change**: new tests not strictly required if the existing suite still covers the paths; agents must run `cargo test` and must not weaken coverage. If a refactor deletes tests, **replace them** with equivalent coverage.
5. Do **not** claim “tested manually only” for shippable behavior.
6. Do not skip tests with `#[ignore]` for core paths without documenting why **and** a tracked follow-up; **ignored tests do not count** as coverage under this policy.
7. **Test locations:** unit tests co-located (`#[cfg(test)]` in `src/...`) or under module trees; e2e under `tests/`; filter parity under `tests/filter_parity.rs` or module tests as designed.
8. Before marking work done: **`cargo test` green** for the change set.
9. When behavior changes, **docs updates (README / SELECTION / AGENTS defaults) still required** — tests **and** docs together (see docs policy).

**Agent checklist (AGENTS.md must include this table):**

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

### `.grok/skills/keep-docs-current/SKILL.md` (full body required in Stage 0)

Remains **docs-focused**. Stage 0 exit requires a complete skill file including:

- YAML frontmatter (`name`, `description` with triggers).
- When to run.
- Policy table (commit README / SELECTION / AGENTS; do not commit huge fixtures).
- Procedure: diff → reconcile `cli.rs` ↔ README → grep stale flags → streaming/overwrite/filter claims → commit hygiene.
- Done checklist.
- Anti-patterns (solid support claims; whole-file buffer as only path; missing `--force` docs).
- Cross-link: “If code behavior changed, also run **keep-tests-current**.”

### `.grok/skills/keep-tests-current/SKILL.md` (full body required in Stage 0)

**New skill** (sibling to keep-docs-current). AGENTS.md is source of truth; this skill operationalizes K30.

Not outline-only. Stage 0 exit requires a complete skill file including:

- YAML frontmatter, e.g.:
  - `name: keep-tests-current`
  - `description`: triggers on commit, finishing a PR, changing CLI/filters/writers/pipeline/bugs; complements AGENTS.md test policy.
- **When to run:** before every commit/PR when the session touched Rust sources under `src/` or `tests/`, or any behavior-affecting change.
- **Policy summary:** point at AGENTS “tests cover every change”; ignored tests do not count; red–green for bugs.
- **Procedure:**
  1. `git status` / `git diff --stat` — list changed areas (CLI, select, archive, pipeline, errors).
  2. Classify each change against the AGENTS checklist table.
  3. Add or update unit/e2e tests accordingly (same change set).
  4. For bug fixes: write the failing test first when practical; ensure it would fail without the fix.
  5. Run `cargo test` (full suite, or at least packages affected + any new e2e).
  6. Confirm no new `#[ignore]` on core paths without a follow-up note in the PR.
  7. If behavior changed, confirm keep-docs-current also applied.
- **Done checklist:**
  - [ ] Behavior changes have matching tests in this PR
  - [ ] `cargo test` green
  - [ ] No “manual only” claims for shippable paths
  - [ ] Bug fixes have regression coverage
  - [ ] Docs skill run if user-visible
- **Anti-patterns:** landing filter/writer changes without tests; `#[ignore]` to silence failures; deleting tests during refactor without replacement; relying on CI to “catch it later” without writing tests.

### Before-commit agent procedure (both skills)

```text
1. Diff change set
2. keep-tests-current  (if code/behavior)
3. keep-docs-current   (if user-visible)
4. cargo test
5. Commit code + tests + docs together when practical
```

### `README.md` (early + evolving)

- create compresses vs embed stores (repeat explicitly).
- Streaming / peak RAM note.
- Overwrite + partial rename.
- Quick starts, flag tables, selection summary, agent policy link.
- Note that contributors/agents must follow AGENTS test + docs policies.

---

## Implementation Stages

### Stage 0 — Repository bootstrap & agent docs **and tests** policy

**Goal:** Crate + mandatory **docs** and **regression test** contracts with **complete** skill bodies (not outlines).

**Deliverables:**

- `Cargo.toml` (lib + bin), MIT, edition 2021; **no rayon**, default pure-rust (lzma not yet required until Stage 6).
- Stub `main` / `cli` with `create` / `embed` “not implemented”.
- `README.md`, `AGENTS.md` (**docs policy + tests/regression policy / K30**).
- Full `.grok/skills/keep-docs-current/SKILL.md`.
- Full `.grok/skills/keep-tests-current/SKILL.md`.
- `.gitignore`, `LICENSE`.

**Tasks:**

1. Package metadata; lib name `rsync_archive`.
2. Core deps: clap, thiserror, tracing, tracing-subscriber.
3. Write AGENTS.md: design-external note, defaults list, **non-negotiable docs currency**, **non-negotiable tests cover every change** (checklist table), pointers to both skills.
4. Write **full** keep-docs-current SKILL.md (procedure + checklist + anti-patterns + cross-link to tests skill).
5. Write **full** keep-tests-current SKILL.md (procedure + checklist + anti-patterns + `cargo test`).
6. README vision, non-goals, create vs embed, planned flags, link to AGENTS policies.
7. Stub subcommands; ensure `cargo test` runs (empty/passing suite OK).

**Exit criteria:**

- `cargo build` / `cargo test`; `--help` shows create/embed.
- AGENTS.md states **both** docs and regression-test policies clearly (including the change-type → tests table).
- Both skills have full procedure + done checklist (not outline-only).
- README states streaming intent, non-solid create, embed store.

**Docs updates:** README + AGENTS + both skills.

**Regression tests for stage:** suite runnable (`cargo test` green); policy files themselves need no behavior tests.

---

### Stage 1 — Errors, pathnorm, output helpers, tracing

**Goal:** Shared foundations.

**Deliverables:**

- `error.rs` (Io, Cli, Selection, Archive, EmptyArchive, PathTraversal, Collision, OutputExists, InvalidMemberName, FilterFileTooLarge, NotRegularFile, …).
- `pathnorm.rs`, `SourceSpec`, tracing init.
- `pipeline/output.rs` stubs: exists check, partial path naming (unit-tested).

**Tasks:**

1. Error types with stable Display.
2. Path normalization + reject `..` unit tests.
3. Partial path helper unit tests (`out.7z` → `out.7z.partial`).
4. `-v` / `-vv` logging.

**Exit criteria:** unit tests green for pathnorm, SourceSpec, partial naming.

**Docs updates:** AGENTS map; README logging one-liner.

**Regression tests for stage (K30):** pathnorm/`..` reject; SourceSpec trailing slash; partial path naming.

---

### Stage 2 — 7z header + store writer

**Goal:** Valid non-solid **Copy** archives (embed foundation). High-risk compatibility review.

**Deliverables:**

- `header.rs`, `store_writer.rs`
- Roundtrip tests via sevenz-rust2 / optional 7zz

**Tasks:**

1. Port HeaderFile, raw header, start signature, empty bits, names, mtime, attrs.
2. Port store writer create / push_path (256 KiB) / push_bytes / finish.
3. Deps: crc32fast, sevenz-rust2, tempfile.
4. Tests: multi-file, empty file, nested paths, empty finish error (**required**, K30).

**Exit criteria:** list/test/extract OK; non-solid; bytes match; `cargo test` green.

**Docs updates:** README embed foundation; AGENTS codec map.

**Regression tests for stage:** store roundtrip + empty finish error + nested path extract.

**Review note:** treat as high-risk PR (header compatibility).

---

### Stage 3 — Embed pipeline + CLI (embed CLI freeze)

**Goal:** Working `rsync-archive embed` with frozen embed flags.

**Deliverables:**

- `pipeline/embed.rs`, naming algorithm, magic check, partial+rename, `--force`
- e2e `tests/e2e_embed.rs`

**Tasks:**

1. Implement member_name algorithm; collisions; prefix validation.
2. Default magic warn; `--require-7z`; `--allow-any`; mutual exclusion.
3. Stream push_path; finish; rename partial → final.
4. Dry-run: print member names; detect collisions; no file written.
5. `--verify`: count + test + non-solid if available.
6. Refuse overwrite without `--force`.
7. **e2e + unit coverage for all of the above (K30)** — naming edge cases, magic flags, force, dry-run.

**Exit criteria:**

- Embed roundtrip byte-identical.
- Collision / empty input / output-exists errors.
- Dry-run no file; partial not left as final name on failure.
- `cargo test` green including `e2e_embed`.

**Docs updates:** README embed flags (complete freeze for embed).

**Regression tests for stage:** e2e embed roundtrip; collision; `--force`; dry-run; magic warn/require/allow-any.

---

### Stage 4 — Rsync rule engine (filter freeze)

**Goal:** Ordered include/exclude matching; **no merge**.

**Deliverables:**

- `rules.rs`, `matcher.rs`, `from_file.rs`
- `docs/SELECTION.md`
- ≥25 table-driven unit cases + parse tests

**Tasks:**

1. Pattern AST: `*`, `?`, `**`, anchored, dir-only; **basename vs full-path target (K27)**.
2. Parse CLI + filter lines + from files; size/line caps.
3. `action_for` first-match; default Include.
4. `should_prune_dir` over-approx as specified (clean final algorithm only).
5. Expand parity table (≥25 matcher cases including nested basename `*.tmp`, `foo` vs `bar/foo`, `dir/*`).

**Exit criteria:** all table tests pass (incl. basename rule); SELECTION.md matches code; `cargo test` green.

**Docs updates:** `docs/SELECTION.md` + README summary; AGENTS pointer.

**Regression tests for stage:** full filter parity table; from-file parse/size-cap; prune predicate unit cases.

---

### Stage 5 — Walk + create dry-run

**Goal:** Real trees → `SelectedEntry`; dry-run uses **same** path as future write.

**Deliverables:**

- `walk.rs`, `pipeline/create.rs` dry-run, create filter CLI flags

**Tasks:**

1. walkdir + SourceSpec mapping table (SRC mode).
2. Only regular files selected; symlink/special skip counters.
3. Prune with canary test (file under excluded dir must not be opened).
4. **files-from mode (K26):** exclusive of SRC; files only; archive_name rules; filters on archive_name; tests for relative vs absolute naming and collisions.
5. **Collision pre-scan (K29):** collect full `Vec<SelectedEntry>` (or equivalent), detect duplicate `archive_name` **before** any pack write; dry-run uses same builder.
6. Dry-run prints archive names one per line; summary skip counts on stderr or verbose.
7. CLI: error if both `--files-from` and `SRC...`; error if neither.

**Exit criteria:**

- Dry-run matches fixtures (SRC and files-from).
- Canary prune test passes.
- Collision detected without creating final `-o` (and without writing pack data; partial may be absent).
- **Single function** builds selection for dry-run and write (write may still be stub).
- `cargo test` green.

**Docs updates:** README dry-run + files-from examples; selection edge cases.

**Regression tests for stage:** canary prune; files-from naming/collision/SRC mutex; multi-SRC collision; dry-run list fixtures.

---

### Stage 6 — LZMA2 create writer + create pipeline (may size-gate)

**Goal:** End-to-end create for normal trees; wire partial+rename + force; empty files; levels; mtime from source.

**Deliverables:**

- `lzma2_writer.rs`, `codec.rs` (may start with buffered encode ≤ threshold → **one** LZMA2 stream per member)
- create write path; e2e small-tree tests

**Tasks:**

1. Implement writer finish/header path; empty files; **mtime from source metadata (K28)**.
2. Level 0–9 preset mapping; always `0x21` for non-empty.
3. If streaming not ready: **hard-fail** files above threshold (no silent multi-GB buffer).
4. Overwrite / partial / rename; open partial only **after** selection pre-scan succeeds (K29).
5. Fail-fast read errors; open non-follow.
6. e2e small tree extract compare; confirm non-solid; spot-check mtime present if reader exposes it.
7. **No** `tempfile::tempdir` source mirroring in create path.

**Exit criteria:**

- Create works for fixtures under threshold.
- Files over threshold error clearly if 6b not merged yet.
- No source tree mirror under TMPDIR (test: sandbox TMPDIR, assert no unexpected dirs beyond partial).
- Output exists without `--force` → error.
- Collision fails before pack bytes are written.
- `cargo test` green including create e2e.

**Docs updates:** README create; level default; mtime policy; size-gate note if still present.

**Regression tests for stage:** e2e create extract; empty file; force/exists; size-gate error if present; TMPDIR no-mirror.

---

### Stage 6b — Streaming LZMA2 encode (**required for v1**)

**Goal:** Multi-GB files with bounded RAM; remove size-gate hard-fail; **one true LZMA2 codestream per member**.

**Deliverables:**

- `StreamingLzma2Encoder` fully wired into create
- `tests/streaming_large.rs` (large sparse/random file, e.g. ≥256 MiB or env-configurable)
- Peak RAM documented
- Spike note: which crate API provides incremental LZMA2 stream emission

**Tasks:**

1. **Spike:** prove `lzma-rust2` and/or `liblzma` can emit incremental bytes of **one** ongoing LZMA2 codestream (`feed` keeps encoder state). If pure-rust cannot, gate Stage 6b on `liblzma` feature or switch default—document choice before relying on size-gate-only create.
2. Wire incremental feed/finish; **forbid** concatenating independent `compress(chunk)` outputs as one pack.
3. Running content_crc + pack_crc.
4. Integration: create from large file; extract compare; process RSS observation optional in ignored bench.
5. Remove STREAM_THRESHOLD hard-fail (or raise to “unlimited”).
6. README peak RAM formula + “one stream per member” note.

**Exit criteria:**

- File ≫ former threshold succeeds without `read_to_end` of full file (code review + test).
- Roundtrip extract matches (proves single valid LZMA2 stream).
- **v1 cannot ship without this stage.**
- `cargo test` green including streaming large (core streaming path not `#[ignore]`; optional RSS bench may be ignored).

**Docs updates:** README streaming section; remove size-gate warnings.

**Regression tests for stage:** multi-chunk feed roundtrip; large-file create extract; regression if size-gate removed.

---

### Stage 7 — Verify, polish, acceptance script, optional 7zz

**Goal:** Production-ready v1 bar.

**Deliverables:**

- `--verify` for create (and embed already)
- `tests/compare_7zz.rs` optional
- **v1 acceptance script** (shell or rust test) listed below
- README/AGENTS accuracy pass (docs **and** tests policies still accurate)

**Tasks:**

1. Implement verify as specified.
2. Name-set compare vs `7zz a -ms=off` when 7zz present.
3. Error messages include path context.
4. keep-docs-current **and** keep-tests-current pass.
5. Add `scripts/v1_acceptance.sh` or `tests/v1_acceptance.rs`.

**Exit criteria:**

- `cargo test` green.
- Acceptance script passes on clean Linux.
- AGENTS defaults match code; AGENTS still states K30 test policy.
- Streaming stage merged.

**Docs updates:** Full flag tables; version 0.1.0 notes.

**Regression tests for stage:** verify success/fail paths; acceptance script covers create/embed/dry-run/force.

---

### Stage 8 — Performance (optional)

**Goal:** Parallel encode ordered append; optional `liblzma` feature; still default threads=1 until switch documented.

**Exit criteria:** speedup on multi-file workloads; RAM still bounded (cap in-flight packs); existing create e2e still green.

**Docs updates:** `--threads`; features.

**Regression tests for stage (K30):** multi-thread create extract parity vs threads=1; in-flight pack bound (no OOM on fixture); no solid regression.

---

### Stage 9 — Extended selection / second format (optional)

**Goal:** size caps, skip-folder dialect; optional `ArchiveWriter` trait + tar.

**Docs updates:** SELECTION.md v2.

**Regression tests for stage (K30):** new filter dialect table cases; any new format writer roundtrip e2e.

---

## v1 acceptance script (commands + expected outcomes)

```bash
# 0. build
cargo build --release

# 1. embed dry-run / write
./target/release/rsync-archive embed -o /tmp/m.7z --dry-run a.7z b.7z
# expect: planned names; no /tmp/m.7z

./target/release/rsync-archive embed -o /tmp/m.7z a.7z b.7z
# expect: /tmp/m.7z exists; sevenz-rust2 or 7zz t OK

./target/release/rsync-archive embed -o /tmp/m.7z a.7z   # no --force
# expect: exit 1 OutputExists

# 2. create dry-run + filters
./target/release/rsync-archive create -o /tmp/o.7z -n --exclude '*.tmp' tree/
# expect: listing without .tmp; no /tmp/o.7z

./target/release/rsync-archive create -o /tmp/o.7z --exclude '*.tmp' tree/
# expect: archive; verify non-solid; extract sample matches

# 3. streaming smoke (large file)
dd if=/dev/urandom of=tree/large.bin bs=1M count=256
./target/release/rsync-archive create -o /tmp/big.7z tree/large.bin
# expect: success; peak RSS not ≈ 256MiB + huge (manual /test)

# 4. empty selection write fails; dry-run ok
./target/release/rsync-archive create -o /tmp/e.7z --exclude '*' tree/; test $? -eq 1
```

---

## Test Strategy

### Policy (agents — K30)

**Canonical policy lives in `AGENTS.md`** (see Agent documentation & testing policy design). Summary:

- Behavior-changing commits **must** add/update automated tests in the same PR.
- Bug fixes need **red–green** regression tests.
- `#[ignore]` on core paths does not satisfy the policy.
- Before done: `cargo test` green; docs updated when behavior changes.
- Skills: **keep-tests-current** (tests) + **keep-docs-current** (docs).

CI (when present) runs `cargo test` on every PR — it enforces **pass**, not **authorship** of tests.

### Layers

| Layer | What | How |
|-------|------|-----|
| Unit | Pattern parse/match (≥25 cases) | Table-driven |
| Unit | pathnorm, member_name, prefix, partial path | Table-driven |
| Unit | Header encode | Structure + CRC |
| Unit | Store / streaming LZMA2 writers | tempdir + sevenz-rust2 |
| Integration | Prune canary (must not open) | temp tree + poison file |
| Integration | TMPDIR sandbox: no source mirror | assert dir entries |
| e2e | create / embed | extract byte compare |
| Streaming | large file | `streaming_large.rs` (core path not ignored) |
| Optional | 7zz name-set | detect binary; may skip if no 7zz |
| Acceptance | v1 script | Stage 7 |

**Fixtures:** tempfile only for binaries; small filter texts may live in `tests/data/`.

**CI:** `cargo test` on every PR when CI exists; optional job installs `7zz` for extended compare.

---

## Risks & Mitigations

| Risk | Severity | Mitigation |
|------|----------|------------|
| Rsync semantics ≠ OpenRsync | High | Frozen subset; table tests; no full-parity claim |
| Header incompatibility | High | Port known-good headers; 7zz + sevenz-rust2 tests; high-risk PR review |
| Streaming encode hard with crate API | High | Stage 6b spike: one LZMA2 stream API; forbid concat of independent compress(); interim size hard-fail; optional liblzma |
| Prune false positive drops files | High | Conservative over-approx; canary tests |
| Multi-SRC / files-from collisions | Medium | Pre-scan hard error before pack write (K29); dry-run same |
| Agent docs drift | Medium | Stage 0 keep-docs-current + AGENTS |
| Agent ships untested features/fixes | High | **K30** + AGENTS test policy + keep-tests-current; PR review; CI `cargo test` |
| lzma preset mismatch vs 7zz | Medium | Document; test roundtrip not byte-identity to 7zz |
| TOCTOU symlink swap | Medium | O_NOFOLLOW / re-check on open |
| Ctrl-C leaves partial | Low | Document; final path only after rename |

---

## Observability (implementation checklist)

- [ ] tracing spans: `create`, `embed`, `walk`, `compress_file`
- [ ] counters: selected, pruned_dirs, skipped_symlinks, skipped_special, bytes_in, bytes_out, magic_warnings
- [ ] final summary line
- [ ] dry-run and write share one selection builder (Stage 5 exit criterion)
- [ ] verify: open, non-solid, count

---

## Open Questions

Only true unknowns remain:

1. **Shared crate with archiveconverter:** extract `sevenz-nonsolid` later if both projects stay active? Defer until after v1.
2. **When to switch Stage 8 default threads from 1 to auto:** needs benchmarks post-parallel work.
3. **Whether to commit `docs/DESIGN.md`:** optional; not required for v1.
4. **Which LZMA2 streaming backend ships as default after Stage 6b spike** (`lzma-rust2` vs require `liblzma`)—resolved during Stage 6b spike, not a product ambiguity.

*(Overwrite, files-from mode/naming, basename match, mtime, collision pre-scan, embed scope, directory entries, tiny-file method, streaming contract, atomicity, **agent regression test policy (K30)** — promoted to Key Decisions K15–K30.)*

---

## References

- Companion: https://github.com/hilather/archiveconverter  
  - README, AGENTS.md, keep-docs-current skill (docs pattern; rsync-archive adds **keep-tests-current** for K30)  
  - `store_writer.rs`, `writer.rs`, `sevenz_header.rs`  
  - Port: placeholder signature → append packs → end header → rewrite start  
  - Do **not** assume companion provides streaming **compression**
- 7-Zip method IDs: Copy `0x00`, LZMA2 `0x21`
- rsync manpage: INCLUDE/EXCLUDE PATTERN RULES
- sevenz-rust2 (read/list/test)
- This design: `/tmp/grok-1000/grok-design-doc-2301a433.md`

---

## PR Plan

Ordered, independently reviewable PRs. Stages map as noted.

### PR1 — Bootstrap crate + agent docs **and tests** policy
- **Title:** `chore: bootstrap rsync-archive with AGENTS, keep-docs-current, keep-tests-current`
- **Files:** `Cargo.toml`, `src/main.rs`, `src/lib.rs`, `src/error.rs`, `src/cli.rs` (stub), `README.md`, `AGENTS.md` (docs **+** K30 regression policy), `.grok/skills/keep-docs-current/SKILL.md` (**full body**), `.grok/skills/keep-tests-current/SKILL.md` (**full body**), `.gitignore`, `LICENSE`
- **Depends on:** none
- **Description:** Foundation; docs policy; **mandatory regression test policy for agents**; create vs embed in README. **Stage 0**. No rayon. `cargo test` runnable.

### PR2 — Foundations: errors, pathnorm, output helpers, tracing
- **Title:** `feat: error types, path normalization, output partial helpers`
- **Files:** `src/error.rs`, `src/util/*`, `src/pipeline/output.rs`, `src/select/mod.rs` (SourceSpec), unit tests
- **Depends on:** PR1
- **Description:** **Stage 1**. Partial naming + OutputExists helpers tested.

### PR3 — Non-solid 7z headers + store writer (**high-risk review**)
- **Title:** `feat(sevenz): port nonsolid header and Copy store writer`
- **Files:** `src/archive/sevenz/header.rs`, `store_writer.rs`, deps, unit tests
- **Depends on:** PR2
- **Description:** **Stage 2**. Header compatibility is review focus; roundtrip mandatory.

### PR4 — Embed command (embed CLI freeze)
- **Title:** `feat: embed command with naming, magic checks, atomic output`
- **Files:** `src/pipeline/embed.rs`, `cli.rs`, `tests/e2e_embed.rs`, README
- **Depends on:** PR3
- **Description:** **Stage 3**. Freeze embed flags: `--force`, `--keep-path`, `--prefix`, `--require-7z`, `--allow-any`, dry-run, verify. Partial+rename.

### PR5 — Rsync rule engine (filter freeze; parallel track)
- **Title:** `feat(select): frozen v1 rsync include/exclude engine`
- **Files:** `src/select/rules.rs`, `matcher.rs`, `from_file.rs`, `docs/SELECTION.md`, `tests/filter_parity.rs`
- **Depends on:** PR2 (parallel with PR3/PR4)
- **Description:** **Stage 4**. No merge. Basename match when pattern has no `/` (K27). ≥25 parity cases. files-from line parser + size caps (mode wiring in PR6).

### PR6 — Walk + create dry-run
- **Title:** `feat(select): walk, prune, files-from mode, and create --dry-run`
- **Files:** `src/select/walk.rs`, `pipeline/create.rs` (dry-run), create CLI flags, tests
- **Depends on:** PR5
- **Description:** **Stage 5**. Regular files only; canary prune; **K26 files-from exclusive of SRC** with archive_name rules; **K29 collision pre-scan**; shared selection builder.

### PR7 — LZMA2 create writer + create write path
- **Title:** `feat: create non-solid LZMA2 7z with atomic output`
- **Files:** `lzma2_writer.rs`, `codec.rs`, `pipeline/create.rs`, deps (`lzma-rust2`), `tests/e2e_create.rs`, README
- **Depends on:** PR3 + PR6
- **Description:** **Stage 6**. CLI freeze for create flags. Size-gate hard-fail allowed only until PR7b. Source mtime in headers (K28). Pack write only after pre-scan. No whole-tree temp mirror. `--force` / partial+rename.

### PR7b — Streaming LZMA2 encode (**v1 blocker**)
- **Title:** `feat(codec): streaming LZMA2 encode for unbounded file sizes`
- **Files:** `codec.rs`, `lzma2_writer.rs`, `pipeline/create.rs`, `tests/streaming_large.rs`, README peak RAM
- **Depends on:** PR7
- **Description:** **Stage 6b**. One LZMA2 codestream per member via true streaming encoder (not concat of independent compressions). Removes size-gate; running CRCs; multi-GB safe. Spike documents crate choice. **Required before v1 release.**

### PR8 — Verify, polish, acceptance script, optional 7zz
- **Title:** `feat: --verify, v1 acceptance, optional 7zz compare`
- **Files:** `pipeline/verify.rs`, `tests/compare_7zz.rs`, `scripts/v1_acceptance.sh` or tests, README, AGENTS
- **Depends on:** PR4 + PR7b
- **Description:** **Stage 7**. Full v1 hardening; docs pass.

### PR9 — (Optional) Parallel encode + liblzma
- **Title:** `perf: parallel LZMA2 encode with ordered append`
- **Files:** create pipeline, `Cargo.toml` features (`liblzma`), README
- **Depends on:** PR8
- **Description:** **Stage 8**. Default threads remain 1 until explicitly changed in docs.

### PR10 — (Optional) Extended selection / trait+format
- **Title:** `feat(select): size caps and skip-folder dialect`
- **Files:** `select/*`, SELECTION.md, tests; optional `archive/traits.rs`
- **Depends on:** PR6 (ideally PR8)
- **Description:** **Stage 9**.

---

*End of design document (Revised — re-review pass + K30 agent regression test policy).*
