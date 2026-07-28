# rsync-archive

**Stream-create non-solid 7z** archives from the filesystem using **rsync-style path selection**, and **embed** finished archives under a master store 7z (Copy method, no recompress).

| | |
|--|--|
| **Status** | Stage 2 complete (header + Copy store writer); create/embed pipelines not implemented yet |
| **License** | MIT |
| **Design** | [`docs/DESIGN.md`](docs/DESIGN.md) |
| **Agent policy** | [`AGENTS.md`](AGENTS.md) (docs **and** tests required on every change) |

---

## What it does

| Command | Role |
|---------|------|
| **`create`** | Rsync-style include/exclude / files-from → **stream-read** sources → **stream-compress** (LZMA2) → single **non-solid** `.7z` |
| **`embed`** | Take multiple finished files (typically `.7z`) → master/outer **non-solid store** 7z (Copy / method `0x00`), same idea as [archiveconverter](https://github.com/hilather/archiveconverter) outer append |

**Create compresses; embed only stores.** Embed does **not** convert solid→non-solid (use archiveconverter for that).

### Design principles

- **Non-solid only** for create (no solid 7z).
- **Disk-friendly:** no full tree copy to temp; stream read / stream compress / append packs.
- **Rsync-style selection** for targeting, with `--dry-run` for testing filters.
- **Safe output:** error if `-o` exists unless `--force`; write `OUT.partial` then rename (when pipelines land).
- **Future formats** may share module layout; v1 is 7z only.

---

## Requirements

- **Rust** 1.70+ (edition 2021)
- Optional later: `7zz` for compare tests

## Build

```bash
cargo build --release
# binary: target/release/rsync-archive
```

## Quick start (current)

```bash
# Inspect CLI (create / embed flags are registered; bodies not implemented yet)
cargo run -- --help
cargo run -- create --help
cargo run -- embed --help

# These currently exit with "not implemented" after flag validation:
# cargo run -- create -o out.7z ./src
# cargo run -- embed -o master.7z a.7z b.7z
```

### Planned `create` (from design)

```bash
rsync-archive create -o game.7z \
  --exclude '*.tmp' \
  --exclude-from excludes.txt \
  --filter '- cache/**' \
  /data/game/

rsync-archive create -o out.7z --files-from list.txt --dry-run
rsync-archive create -o out.7z --force --level 5 --verify /data/tree
```

### Planned `embed` (from design)

```bash
rsync-archive embed -o master.7z nest1.7z nest2.7z
rsync-archive embed -o master.7z --keep-path --prefix packs/ ./build/*.7z
rsync-archive embed -o master.7z --require-7z --verify a.7z b.7z
```

---

## CLI overview (flags registered)

### Global

| Flag | Meaning |
|------|---------|
| `-v` / `-vv` | Debug / trace logging on stderr (`info` → `debug` → `trace`; `RUST_LOG` overrides) |

### `create`

| Flag | Default | Meaning |
|------|---------|---------|
| `-o`, `--output` | required | Output `.7z` |
| `-n`, `--dry-run` | off | List selection only |
| `--force` | off | Overwrite existing `-o` |
| `--exclude` / `--include` | — | Rsync-style patterns (repeatable) |
| `--exclude-from` / `--include-from` | — | Pattern files |
| `--files-from` | — | Explicit file list (exclusive of `SRC...`) |
| `--filter` | — | `+ pattern` / `- pattern` (repeatable) |
| `--level` | `5` | LZMA2 level 0–9 |
| `--verify` | off | Post-write test |
| `SRC...` | — | Sources (required unless `--files-from`) |

### `embed`

| Flag | Default | Meaning |
|------|---------|---------|
| `-o`, `--output` | required | Master `.7z` |
| `-n`, `--dry-run` | off | List members only |
| `--force` | off | Overwrite existing `-o` |
| `--prefix` | — | Prefix for member names |
| `--keep-path` | off | Keep path as name (default: basename) |
| `--require-7z` | off | Fail if missing 7z magic |
| `--allow-any` | off | Allow non-7z store blobs |
| `--verify` | off | Post-write test |
| `FILE...` | required | Inputs to embed |

**Exit codes:** `0` success · `1` operational error · `2` usage error

---

## Implementation status

See [`docs/DESIGN.md`](docs/DESIGN.md) for stages and PR plan.

| Stage | Status |
|-------|--------|
| 0 Bootstrap + agent docs/tests policy | **Done** |
| 1 Foundations (errors, pathnorm, output helpers) | **Done** |
| 2 7z header + store writer | **Done** (library: `NonsolidStoreWriter` Copy / method `0x00`; embed foundation) |
| 3 Embed pipeline | Planned |
| 4 Rsync filter engine | Planned |
| 5 Walk + create dry-run | Planned |
| 6 Create LZMA2 write | Planned |
| 6b Streaming LZMA2 (v1 blocker) | Planned |
| 7 Verify + acceptance | Planned |

---

## Project layout

```text
src/
  main.rs, cli.rs, lib.rs, error.rs
  archive/sevenz/    # non-solid header + NonsolidStoreWriter (Copy) for embed
  select/            # SourceSpec, pathnorm (filters/walk later)
  pipeline/output.rs # partial path, --force check, rename helpers
  util/              # tracing init
  # later: archive/sevenz/lzma2_writer, pipeline/{create,embed}
docs/
  DESIGN.md          # full design
  SELECTION.md       # filter semantics (Stage 4)
AGENTS.md            # mandatory agent policy (docs + tests)
.grok/skills/
  keep-docs-current/
  keep-tests-current/
tests/               # cli_smoke + later e2e/parity
```

---

## Contributing / agents

**Every behavior-changing change must update docs and include regression tests** in the same PR. See:

- [`AGENTS.md`](AGENTS.md)
- `.grok/skills/keep-docs-current/SKILL.md`
- `.grok/skills/keep-tests-current/SKILL.md`

```bash
cargo test
cargo run -- --help
```

---

## License

MIT
