# Tar.lz4 create format (RA-friendly)

**Status:** Implemented (MVP)  
**CLI:** `rsync-archive create -o OUT.tar.lz4 --format tar-lz4 …`  
**Aliases:** `--format tar.lz4` / `tarlz4`; infer from `-o *.tar.lz4` or `*.tlz4`  
**Library:** `write_tar_lz4`, `list_tar_lz4_members`, `extract_tar_lz4_member` / `_bytes`, `verify_tar_lz4`, `decompress_tar_lz4_payload_to_tar_bytes`

Distinct from:

- `create --method lz4` → **non-solid 7z** with per-file LZ4 packs  
- `create --format tar-zstd` → same tar + RATAIDX1 idea, but seekable Zstd (zeekstd)

There is no standard “seekable LZ4” peer to zeekstd; this format uses **independent
LZ4 frames** plus a cleartext **frame table** so we can map uncompressed offsets
to compressed byte ranges.

---

## On-disk layout

```text
┌─────────────────────────────────────────────────────────────┐
│  Independent LZ4 frames (lz4_flex frame format)             │
│  each ≤ 2 MiB uncompressed of tar payload (default)         │
└─────────────────────────────────────────────────────────────┘
│  Cleartext RATLFRM1 frame table + footer                    │
└─────────────────────────────────────────────────────────────┘

uncompressed concatenation of frames =
┌─────────────────────────────────────────────────────────────┐
│  POSIX ustar/pax tar stream                                 │
│    regular files + hard links ('1') + symlinks ('2')        │
│    + parent dirs (typeflag '5')                             │
│  + two 512-byte zero blocks (EOA)                           │
│  + RATAIDX1 member index                                    │
│  + u64 LE index_start (last 8 uncompressed bytes)           │
└─────────────────────────────────────────────────────────────┘
```

### Member index (`RATAIDX1`, version 1)

Same layout as [`FORMAT_TAR_ZSTD.md`](FORMAT_TAR_ZSTD.md):

```text
magic        "RATAIDX1"
u32 LE       version = 1
u64 LE       member_count
for each member:
  u64 LE     name_len
  bytes      name (UTF-8 archive path)
  u64 LE     tar_header_offset
  u64 LE     tar_data_offset
  u64 LE     data_len
  u32 LE     mode
  u64 LE     mtime_unix
  u32 LE     uid
  u32 LE     gid
u64 LE       index_start   // final 8 bytes of uncompressed payload
```

Same member metadata as [`FORMAT_TAR_ZSTD.md`](FORMAT_TAR_ZSTD.md) (mode/uid/gid from selection; uname/gname in tar headers only; **directory members** for parent prefixes; **symlinks** as `typeflag='2'` and **hard links** as `typeflag='1'`, both with no data body, `data_len=0` in index). Regular-file **size** is post-open re-stat (soft-fail / pad policy same as tar.zst).

### Frame table (`RATLFRM1`, version 1)

Cleartext trailer after all LZ4 frames. Last 8 file bytes are `footer_offset`
(pointing at the start of this table).

```text
magic                "RATLFRM1"
u32 LE               version = 1
u64 LE               frame_count
for each frame:
  u64 LE             compressed_offset
  u64 LE             compressed_size
  u64 LE             uncompressed_offset
  u64 LE             uncompressed_size
u64 LE               total_uncompressed
u64 LE               footer_offset   // also last 8 bytes of file
```

Readers: read last 8 bytes → seek to frame table → parse frames →
decompress only frames covering `[index_start, total)` for list, or
`[tar_data_offset, tar_data_offset+data_len)` for extract.

### Metadata (v1 complete)

Same as tar.zst: path, size, mtime, mode, uid, gid (from selection); uname/gname resolved at walk into ustar (pax if &gt;32 bytes); regular files, **hard links** (`typeflag='1'`, linkname = first archive path), **symbolic links** (`typeflag='2'`, linkname / pax `linkpath`, size 0), **and** parent directory members (`typeflag='5'`, trailing `/`, size 0). Names and link targets are **not** in `RATAIDX1` (headers only).

**Hard-link soft-fail (same as tar.zst):** `write_tar_lz4` emits a hard-link member only if the target File body was written this run; if the body was soft-skipped at open, dependent HardLink members are soft-skipped (`skipped_vanished`) — no dangling typeflag `'1'`.

---

## CLI

```bash
rsync-archive create -o out.tar.lz4 --format tar-lz4 --level 1 ./data/
rsync-archive create -o pack.tlz4 ./data/    # .tlz4 → tar-lz4
```

`--method` is **7z-only** (error if non-default with tar-lz4).  
`--level` is accepted for CLI parity; the default pure-Rust LZ4 frame encoder
does not use fine-grained levels (same as 7z `--method lz4` without `lz4-hc`).

---

## Random access

- **Our tool:** member list/extract via RATAIDX1 + frame table (decode only needed frames).  
- **Stock tools:** concatenate/decompress all LZ4 frames, then `tar -x`. Trailing
  cleartext frame table is after compressed frames (not part of the tar stream);
  a naive `lz4 -d wholefile` may not apply — use our list/extract or strip the
  footer first. Full interop path: decode each frame in order until footer magic.

---

## Interop (create → stock tools)

Multi-frame LZ4 + cleartext `RATLFRM1` is **custom** (no standard seekable-LZ4 peer).
Stock `lz4 -d` on the whole file may fail or leave the footer as garbage.

**Supported path for listing/extract with GNU/bsdtar:**

1. Decode every independent LZ4 frame **in order**, stop **before** the cleartext
   `RATLFRM1` frame table (library: `decompress_tar_lz4_payload_to_tar_bytes`).
2. Result is tar + EOA + trailing `RATAIDX1` (same as tar.zst uncompressed payload).
3. Write to a temp `.tar` and run `tar -tf` / `tar -xf`. Tar typically ignores
   trailing index after EOA.

Covered by e2e smoke: `tests/e2e_tar_lz4.rs` → `system_tar_can_list_after_decode_if_tools_present` (soft-skips if `tar` missing).
