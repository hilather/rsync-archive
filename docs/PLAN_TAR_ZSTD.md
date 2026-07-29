# Plan: RA-friendly `tar.zst` create format

**Status:** **Implemented (MVP)** — decisions frozen 2026-07-29; see [`FORMAT_TAR_ZSTD.md`](FORMAT_TAR_ZSTD.md)  
**Date:** 2026-07-29  
**Goal:** Add **`tar.zst`** as an **additional** create format that:

1. Reuses the **same selection + restriction pipeline** as 7z / seekable-zstd.
2. Carries **tar-class metadata** (path, mode, mtime, uid/gid where available).
3. Stays **random-access friendly** under the same constraints we already accept for zstd (member-level extract without full solid scan).

This does **not** replace `--method zstd` (7z packs) or `--format seekable-zstd`.

---

## 1. Problem statement

| Need | Classic `tar \| zstd` | Our 7z `--method zstd` | Our seekable-zstd |
|------|----------------------|------------------------|-------------------|
| Unix meta (mode, owner, mtime) | Strong | Weak (path + mtime only) | Weak (path only) |
| Per-file random access | Poor (solid stream) | Strong (non-solid packs) | Strong (index + seekable frames) |
| Stock `tar`/`zstd -d` interop | Strong | None (7z) | Weak (custom payload) |

We want a format that sits in the **interop + meta** column without giving up **member RA**.

---

## 2. Non-goals (v1)

- Replacing 7z default or removing seekable-zstd.
- Full pax/xattr/ACL parity with GNU tar (can stage later).
- Solid multi-GB single-frame zstd (explicitly out — breaks RA).
- Perfect bit-identical roundtrip with every tar implementation.
- Windows-native create (Linux/Unix meta first).

---

## 3. Recommended on-disk design (v1)

### 3.1 Name and CLI

| Item | Proposal |
|------|----------|
| Format enum | `OutputFormat::TarZstd` |
| Flag | `--format tar-zstd` (aliases: `tar.zst`, `tarzst`) |
| Extension infer | `-o *.tar.zst` **or** `*.tzst` → tar-zstd; **not** `*.tgz` (gzip) or bare `*.zst` (seekable-zstd) |
| Method flag | **Ignored / error if non-default** (same rule as seekable-zstd): compression is Zstd of the tar payload, not 7z method |

```bash
rsync-archive create -o out.tar.zst --format tar-zstd --level 5 \
  --max-total-size 500M --dir-max-files logs/=100 \
  /var/log/
```

### 3.2 Core idea: **valid tar payload + seekable Zstd + RA index**

Two layers, same spirit as current seekable-zstd:

```text
┌──────────────────────────────────────────────────────────────┐
│  Zstd **seekable** frames (zeekstd; independent frames)      │
│  + standard seekable footer                                  │
└──────────────────────────────────────────────────────────────┘
         uncompressed payload =
┌──────────────────────────────────────────────────────────────┐
│  POSIX ustar/pax **tar stream** (concatenated members)       │
│  + optional trailing RA index (skip-friendly)                │
└──────────────────────────────────────────────────────────────┘
```

**Full-archive path (interop):**

1. Seekable-zstd decode entire stream → bytes are a **valid tar**.
2. `tar -tf` / `tar -xf` / `bsdtar` work on the decompressed tar (or on a pipe if tool supports seekable zstd — many do not; document “decompress then tar” as baseline).

**Single-member RA path (our tool):**

1. Read **member index** (path → uncompressed tar offset + size + header size).
2. Use seekable decoder `set_offset` / frame skip to only decode frames covering that range.
3. Parse one ustar/pax header + file data; write out.

### 3.3 Why this preserves “same RA restrictions”

| RA property we care about | How tar-zstd keeps it |
|---------------------------|------------------------|
| No solid cross-file dictionary over whole archive | Seekable **independent frames** (reuse zeekstd policy, e.g. 2 MiB) |
| Name → location without full scan | Trailer **index** (like `RAZSIDX1`, new magic e.g. `RATARIDX1`) |
| Bounded work to extract one file | Decode only frames intersecting `[tar_offset, tar_offset+span)` |
| Selection caps apply the same | Index is built **after** `build_selection` (same entry list) |

**Not claimed:** “GNU tar can random-access extract one file from .tar.zst without our index.” Stock tools stay full-decode (or full decompress + tar scan). **Our** CLI/library keeps RA.

### 3.4 Tar member contents (metadata MVP)

For each `SelectedEntry` emit **ustar** (upgrade to **pax** when path >100 chars or need uid/gid names):

| Field | Source |
|-------|--------|
| `name` / `prefix` | `archive_name` (split per ustar rules) |
| `size` | `entry.size` |
| `mtime` | `entry.mtime_unix` (0 if missing) |
| `mode` | From selection-time mode if we **extend** `SelectedEntry` (new); else default `0644` / `0755` heuristic |
| `uid` / `gid` | From metadata if extended; else 0 |
| `typeflag` | Regular file only (`'0'`) — matches current “regular files only” selection |
| `magic` / `version` | ustar |
| checksum | Standard tar header checksum |

**v1.1 meta extensions (optional PR):** store mode/uid/gid/uname/gname on `SelectedEntry` at walk time (same OPT-03 style: no re-stat on write).

### 3.5 Index format (sketch)

Appended at end of **uncompressed** payload (after tar end-of-archive blocks), or as a final skippable zstd frame. Prefer **uncompressed trailer** after tar EOAs so full decompress still yields tar + ignorable junk *or* put index in a **skippable zstd frame** so pure tar decompress tools that only understand zstd frames may need `zstd -d --long` behavior — **decision spike:**

| Placement | Pros | Cons |
|-----------|------|------|
| **A. After tar EOAs inside payload** | One stream; our reader seeks to `size_decomp - 8` like today | `tar -x` may warn on trailing garbage after two zero blocks (often OK) |
| **B. Skippable zstd frame after seekable body** | Clean tar when decoded with frame-aware tools | Slightly more complex; must document |
| **C. Sidecar `.tar.zst.idx`** | Zero interference with tar | Two files; worse UX |

**Decision (frozen):** **A** — index after tar EOAs inside uncompressed payload (mirror seekable-zstd). Magic `RATARIDX1` and fields:

```text
magic "RATARIDX1"
u32 version = 1
u64 member_count
for each member (selection order):
  u64 name_len
  bytes name
  u64 tar_header_offset   // uncompressed offset of ustar/pax header
  u64 tar_data_offset     // first content byte
  u64 data_len
  u32 mode                // optional, 0 = unknown
  u64 mtime_unix
u64 index_start           // last 8 bytes of uncompressed payload
```

### 3.6 Rejected alternatives (for clarity)

| Idea | Why not for v1 |
|------|----------------|
| Classic solid `tar \| zstd` only | Breaks RA; fails stated requirement |
| One small `.tar.zst` per file in a directory | Not a single archive; not “a tar.zst” |
| Non-solid 7z with tar-inside-each-pack | Double format; no tar interop for the container |
| Replace seekable-zstd payload with tar without index | Loses fast name lookup |

---

## 4. Pipeline integration (no break to restrictions)

```text
build_selection (unchanged)
  filters → min/max size → newer-than → dir-max-size → dir-max-files
  → max-total-size → max-files
        │
        ▼
  entries: Vec<SelectedEntry>   // same list dry-run prints
        │
        ├── format 7z            → existing writer
        ├── format seekable-zstd → existing writer
        └── format tar-zstd      → NEW writer only
```

- Dry-run, collision checks, budgets, compact restriction report: **shared**.
- `--method` with tar-zstd: **error** if not default (document).
- `--threads` / encode pool: v1 can be **sequential tar write into zeekstd encoder** (simpler correctness); parallel “compress frames” later if needed (harder with single tar stream).

---

## 5. Implementation plan (PR slices)

### PR1 — Tar member encode primitives
- `src/archive/tar/` or `src/archive/tar_zstd/ustar.rs`
- Write ustar header + data + padding; pax for long names
- Unit tests: header checksum, roundtrip parse of one member, 511-byte and 512-byte edge sizes

### PR2 — Uncompressed tar stream builder from `SelectedEntry`
- Stream files in selection order into a `Write` (no full buffer of tree)
- Track running offset → build index rows
- Two zero blocks EOA
- Tests with tempfile tree

### PR3 — Seekable zstd wrap + index trailer
- Reuse zeekstd patterns from `seekable_zstd/mod.rs`
- Frame size policy aligned with seekable-zstd (document same 2 MiB default)
- Write `RATARIDX1` + `index_start`
- Library: `write_tar_zstd`, `list_tar_zstd_members`, `extract_tar_zstd_member`

### PR4 — CLI + create pipeline
- `OutputFormat::TarZstd`
- Infer from `-o file.tar.zst`
- Wire `run_create`; reject bad `--method`
- Verify: decompress+list or index parse + sample extract one member

### PR5 — Docs + acceptance
- `docs/FORMAT_TAR_ZSTD.md` (normative layout)
- README format table; SELECTION unchanged for filters
- e2e: create → list → extract sample; compare path + content + mtime
- Optional: pipe decompressed tar to system `tar -t` in CI if `tar` present

### PR6 — Meta completeness
**Status: Done** — `SelectedEntry.{mode,uid,gid}` at walk/files-from; tar.zst + tar.lz4 headers + RATAIDX1 index; pax for oversized uid/gid.  
**uname/gname (done):** `SelectedEntry.{uname,gname}` at walk; ustar + pax when &gt;32 bytes; names in headers only (not index).

---

## 6. Random-access cost model (honest)

| Operation | Cost |
|-----------|------|
| List members | O(index) after seek to trailer — **not** O(archive) |
| Extract one small file | Decode **only frames** covering that tar range (+ small amplification from frame size) |
| Extract whole archive | Full seekable decode → tar extract (same order of work as solid tar.zst decompress) |
| Stock `zstd -d out.tar.zst \| tar -x` | Works if decoder accepts seekable multi-frame (zeekstd/zstd seekable); may need our decompress helper if system zstd is old |

**Frame size tradeoff:** smaller frames → better RA granularity, slightly worse ratio; keep knobs consistent with seekable-zstd.

---

## 7. Compatibility matrix (target)

| Consumer | Support target |
|----------|----------------|
| `rsync-archive` list/extract/verify | Full (index RA) |
| `zstd -d` (seekable-aware) + `tar -x` | Full archive restore |
| Ancient `zstd` without seekable | May fail; document minimum version / ship extract subcommand |
| `7zz` | Not required |
| Browser range GETs | Possible later via seek table (same as seekable-zstd) |

---

## 8. Risks and mitigations

| Risk | Mitigation |
|------|------------|
| Trailing index breaks picky tar | Document; or move index to skippable frame (PR3 spike) |
| Long paths / non-ASCII | pax headers in PR1 |
| Parallel encode vs single tar stream | Sequential v1; don’t block ship on pool |
| Confusion with `--method zstd` | Distinct `--format tar-zstd`; help text + README table |
| Duplicate of seekable-zstd | Different payload (tar vs custom length-prefix); share only zeekstd plumbing |

---

## 9. Success criteria

1. `create --format tar-zstd` produces file extractable as tar after zstd decompress (documented path).
2. Library extract of one member does **not** require decoding the entire archive when member is in early frames (test with large leading payload).
3. Same `build_selection` restrictions change member set identically to 7z dry-run for same flags.
4. Default create remains **7z + lzma2**; no behavior change for existing flags without `--format tar-zstd` / `*.tar.zst`.

---

## 10. Suggested sequencing vs other work

| Priority | Item |
|----------|------|
| Now | This plan review / freeze layout (index placement A vs B) |
| Next | PR1–PR4 implementation |
| Later | PR6 meta fields; optional parallel frame encode; HTTP range demos |

---

## 11. Frozen decisions (2026-07-29)

| # | Decision | Choice |
|---|----------|--------|
| **1** | Index placement | **A — after tar EOA** inside uncompressed payload (`RATARIDX1` + final `index_start` u64), same reader pattern as seekable-zstd |
| **2** | Extension inference | **`*.tar.zst` and `*.tzst`** → tar-zstd; bare `*.zst` stays seekable-zstd; `*.tgz` not used |
| **3** | v1 metadata | **mtime + path (+ size)** from current `SelectedEntry` is enough; mode/uid/gid deferred to optional PR6 |

Implementation can follow **PR1→PR5** without further design churn. PR6 remains optional follow-up.
