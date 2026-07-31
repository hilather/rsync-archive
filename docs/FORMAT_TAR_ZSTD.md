# Tar.zst create format (RA-friendly)

**Status:** Implemented (MVP)  
**CLI:** `rsync-archive create -o OUT.tar.zst --format tar-zstd …`  
**Aliases:** `--format tar.zst` / `tarzst`; infer from `-o *.tar.zst` or `*.tzst`  
**Library:** `write_tar_zstd`, `list_tar_zstd_members`, `extract_tar_zstd_member` / `_bytes`, `verify_tar_zstd`, `decompress_tar_zstd_payload_to_tar_bytes`

Distinct from:

- `create --method zstd` → **non-solid 7z** with per-file Zstd packs  
- `create --format seekable-zstd` / bare `*.zst` → custom length-prefixed members  

Plan / decisions: [`PLAN_TAR_ZSTD.md`](PLAN_TAR_ZSTD.md).

---

## On-disk layout

```text
┌─────────────────────────────────────────────────────────────┐
│  Zstd seekable frames (zeekstd; default 2 MiB uncompressed) │
│  + seek table footer                                        │
└─────────────────────────────────────────────────────────────┘
         uncompressed payload =
┌─────────────────────────────────────────────────────────────┐
│  POSIX ustar/pax tar stream                                 │
│    regular files + symlinks (typeflag '2') + parent dirs    │
│    (typeflag '5')                                           │
│  + two 512-byte zero blocks (EOA)                           │
│  + RATARIDX1 member index                                   │
│  + u64 LE index_start (last 8 uncompressed bytes)           │
└─────────────────────────────────────────────────────────────┘
```

### Index (`RATAIDX1`, version 1)

(8-byte magic on disk; plan text “RATARIDX1” was 9 chars.)

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

Readers: seek to `size_decomp - 8`, read `index_start`, parse index, then
`set_offset(tar_data_offset)` / `set_offset_limit` for single-member extract.

### Metadata (v1 complete)

| Field | Source |
|-------|--------|
| path | `SelectedEntry.archive_name` (ustar name/prefix or pax `path=`) |
| size | **post-open re-stat** for regular files (not selection size); directories, symlinks, and hard links: always 0. Soft-fail: skippable open skips member; short read after header is zero-padded |
| mtime | `mtime_unix` or 0 (directories: statted from real dir when available; links: `lstat`) |
| mode | `st_mode & 0o7777` at selection (default `0644` if unknown; dirs default `0755`) |
| uid / gid | at selection (0 if unknown / non-Unix); pax if &gt; 7-digit octal |
| uname / gname | resolved at selection via `getpwuid_r` / `getgrgid_r` (empty if unknown / non-Unix); ustar fields (32 bytes); pax `uname=` / `gname=` when longer |
| type | regular file (`'0'`), **hard link** (`'1'`, linkname = first archive path), **symbolic link** (`'2'`, linkname / pax `linkpath`), **and** parent directory members (`'5'`, name ends with `/`) |

### Directory members

Derived from selected **file, symlink, and hard-link** archive paths (not a separate selection walk):

- Before each member, unique parent prefixes are emitted once as directory headers (`typeflag='5'`, name with trailing `/`, size 0).
- Example: selected `a/b/c.txt` → members `a/`, `a/b/`, `a/b/c.txt`.
- Empty directories with no selected members under them are **not** archived.
- Directory meta (mode/uid/gid/uname/gname/mtime) is taken from the real filesystem directory when it can be resolved from `SelectedEntry.abs_path`; otherwise mode `0755` and zero/empty ownership.
- **7z** and **seekable-zstd** create ignore this (they never inject dir members).

### Symbolic links

- Walk / `--files-from` include symlinks that pass filters (`MemberKind::Symlink`, size 0).
- Tar create emits `typeflag='2'`; target from `read_link` (as stored) in ustar `linkname` (100 bytes) or pax `linkpath=` when longer.
- No file data body; `RATAIDX1` has `data_len=0` (target is only in the tar header).
- **7z** and **seekable-zstd** skip symlink entries at encode time (counted as `skipped_symlinks`).

### Hard links

- On **Unix**, walk / `--files-from` detect hard links via `(st_dev, st_ino)`: first regular-file path is `MemberKind::File` with content size; later paths are `MemberKind::HardLink { target }` where `target` is the first path’s `archive_name` (size 0 for restriction accounting).
- Tar create emits `typeflag='1'`; linkname / pax `linkpath` is that first archive path (not a filesystem path rewrite).
- No file data body; `RATAIDX1` has `data_len=0`.
- **Encode soft-fail:** `write_tar_zstd` only emits a hard-link member if the target File body was actually written this run. If the body was soft-skipped at open (vanished / EACCES / ESTALE), later HardLink members pointing at that name are soft-skipped too (`skipped_vanished`) so the archive never contains a dangling typeflag `'1'`. Pre-encode `filter_hardlinks_without_targets` cannot catch this race.
- **Non-Unix:** no hard-link detection (each path is a full `File` member).
- **7z** and **seekable-zstd** skip hard-link entries at encode time (`skipped_hardlinks`), keeping only the first file body for the inode.

**Index note:** `RATAIDX1` stores path/size/mtime/mode/uid/gid for **files, directories, symlinks, and hard links** (`data_len=0` for dirs and links). Owner **names** and link **targets** are in tar headers (and pax when needed), not in the trailer index.

---

## CLI

```bash
rsync-archive create -o out.tar.zst --format tar-zstd --level 5 ./data/
rsync-archive create -o pack.tzst --level 3 ./data/    # .tzst → tar-zstd
# Same restrictions as other formats:
rsync-archive create -o logs.tar.zst --max-total-size 100M --dir-max-files var/log/=50 /var/log/
```

`--method` is **7z-only** (error if non-default with tar-zstd).

---

## Random access

- **Our tool:** member list/extract via index (no full solid scan).  
- **Stock tar:** decompress full stream (seekable multi-frame may need a seekable-aware zstd), then `tar -x`. Trailing index after EOA is typically ignored by tar as trailing garbage after two zero blocks.

---

## Interop (create → stock tools)

**Supported path for listing/extract with GNU/bsdtar** (no product extract CLI required):

1. Fully decompress the seekable Zstd payload to uncompressed bytes with a seekable-aware decoder  
   (library: `decompress_tar_zstd_payload_to_tar_bytes`; uses `zeekstd`).  
   Stock `zstd -d` on the whole file is **not** guaranteed for multi-frame seekable streams.
2. Write those bytes to a temp `.tar` (payload = tar + EOA + trailing `RATAIDX1`).
3. Run `tar -tf payload.tar` / `tar -xf payload.tar`. Tar stops at the two zero EOA blocks and typically ignores the trailing index.

Covered by e2e smoke: `tests/e2e_tar_zstd.rs` → `system_tar_can_list_after_decode_if_tools_present` (soft-skips if `tar` missing).
