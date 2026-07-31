# Seekable-zstd create format

**Status:** Implemented (MVP)  
**CLI:** `rsync-archive create -o OUT.zst --format seekable-zstd …`  
**Library:** `write_seekable_zstd`, `list_members`, `extract_member` / `extract_member_bytes`

This format is **distinct** from `create --method zstd` (which writes **non-solid 7z** with per-file Zstd packs). Seekable-zstd produces a **single** Zstd seekable stream suitable for byte-range random access over a concatenated member payload.

---

## File layout (on disk)

```text
┌─────────────────────────────────────────────────────────────┐
│  Zstd seekable frames (zeekstd / Zstd Seekable Format)      │
│  Independent frames (default 2 MiB uncompressed each)       │
│  + seek table footer (standard seekable skippable frame)    │
└─────────────────────────────────────────────────────────────┘
```

The **uncompressed** payload inside those frames is:

```text
┌── for each member (selection order) ────────────────────────┐
│  u64 LE  name_len                                           │
│  bytes   name (UTF-8, `/`-separated archive path)           │
│  u64 LE  data_len                                           │
│  bytes   raw file contents (data_len bytes)                 │
└─────────────────────────────────────────────────────────────┘
┌── trailer index ────────────────────────────────────────────┐
│  magic        "RAZSIDX1"  (8 bytes)                         │
│  u32 LE       version = 1                                   │
│  u64 LE       member_count                                  │
│  for each member:                                           │
│    u64 LE     name_len                                      │
│    bytes      name (UTF-8)                                  │
│    u64 LE     data_offset   // uncompressed offset of data  │
│    u64 LE     data_len                                      │
│  u64 LE       index_start   // offset of "RAZSIDX1" magic   │
└─────────────────────────────────────────────────────────────┘
```

`data_offset` points at the first **file content** byte (after that member’s `name_len|name|data_len` header), measured from the start of the **uncompressed** payload.

**Soft-fail create:** `data_len` is post-open re-stat (not selection size). Skippable open skips the member; short read after the header is zero-padded. All members skipped → empty archive error.

The final `index_start` is always the last 8 uncompressed bytes, so readers can:

1. Open the file with a seekable-zstd decoder (`zeekstd::Decoder`).
2. Read `size_decomp` from the seek table.
3. Seek to `size_decomp - 8`, read `index_start`.
4. Seek to `index_start`, parse the index (`RAZSIDX1` …).
5. For extract: `set_offset(data_offset)` / `set_offset_limit(data_offset + data_len)` and decompress.

---

## Compression parameters

| Parameter | Default | Notes |
|-----------|---------|--------|
| Codec | Zstd seekable (`zeekstd`) | Independent frames + seek table |
| Frame size | **2 MiB** uncompressed | `FrameSizePolicy::Uncompressed` |
| Level | CLI `--level` 0–9 | Mapped directly to zstd level |
| Checksums | on | Frame checksums enabled |
| Atomic write | `OUT.partial` → rename | Same as 7z create |

`--method` applies only to **7z** create. Combining a non-default `--method` with seekable-zstd is a **usage error**.

---

## CLI

```bash
# Explicit format
rsync-archive create -o out.zst --format seekable-zstd --level 5 ./src/

# Infer from .zst extension
rsync-archive create -o pack.zst --level 3 ./data/

# Dry-run (no file written)
rsync-archive create -o out.zst --format seekable-zstd -n ./src/

# Alias
rsync-archive create -o out.zst --output-format seekable-zstd ./src/
```

Default create format remains **`7z`** (non-solid, method `lzma2`) when `-o` is not `.zst` and `--format` is omitted.

---

## Library helpers (tests / tooling)

| API | Role |
|-----|------|
| `write_seekable_zstd(path, entries, level)` | Stream create |
| `list_members(path) → MemberIndex` | Parse trailer index via seeks |
| `extract_member(path, name, &mut Write)` | Seek + decode one member |
| `extract_member_bytes(path, name)` | Convenience buffer |
| `verify_seekable_zstd(path, expected_count)` | Index + length check |

There is no full `rsync-archive extract` subcommand yet; helpers are for verify and tests.

---

## Non-goals (MVP)

- Tar / pax compatibility of the inner payload
- Solid 7z or 7z-with-zstd mixed containers
- Directory entries, ownership (regular files only; same as 7z create)
- Symlinks and hard-link members are selected at walk but **skipped** for this format (`skipped_symlinks` / `skipped_hardlinks`); the first regular-file body for a hard-linked inode is kept. Use **tar-zstd** / **tar-lz4** to archive link members
- Parallel multi-file encode workers (single streaming encoder)

---

## Compatibility notes

- Any **seekable-format-aware** Zstd decoder can decompress the whole payload linearly; the trailing index is just more uncompressed bytes at the end of the logical stream.
- A **non-seekable** zstd decoder that ignores the seek-table skippable frame can still decode the concatenated frames as a normal multi-frame stream (implementation-dependent).
- The member index is **rsync-archive-specific**; third-party tools will see raw length-prefixed records + index unless they implement this layout.
