---
name: keep-docs-current
description: >
  Keep rsync-archive README, AGENTS.md, and docs/SELECTION.md synchronized with
  code on every commit. Use when committing, finishing a PR, changing CLI flags,
  create/embed behavior, selection filters, streaming claims, or defaults; also
  when the user says "update docs", "sync docs", "docs drift", or runs
  /keep-docs-current. Complements AGENTS.md mandatory docs policy. If code
  behavior changed, also run keep-tests-current.
---

# Keep docs current

## When to run

Invoke **before every commit** (and before push) if the session touched:

- CLI (`src/cli.rs`, create/embed flags or defaults) — **including help text**
- Selection, pipeline, archive writers, streaming/codec
- Anything user-visible that README or AGENTS still describes differently

Also run when the user asks to document, release, or “make sure README is up to date.”

## Policy (summary)

Canonical agent policy: repo-root **`AGENTS.md`**. This skill operationalizes the **docs** half
and the **CLI help** half (clap `///` strings are user docs).

| Commit to git | Do not commit |
|---------------|---------------|
| `src/cli.rs` (help/`about`/`after_help`) | `target/**` |
| `README.md` | Multi-MB archives / fixtures |
| `AGENTS.md` (if defaults/map/CLI policy changed) | `*.partial` leftovers |
| `docs/SELECTION.md` (when present) | Huge `benchdata/**` |
| `tests/cli_smoke.rs` when flags change | |
| This skill if policy changes | |

## Procedure

### 1. Diff the change set

```bash
git status
git diff --stat
```

List which of: **CLI**, **create/embed**, **selection**, **streaming**, **defaults** actually changed.

### 2. Reconcile CLI help (mandatory if flags changed)

1. Edit **`src/cli.rs`**: every flag’s first `///` line must be usable under **`create -h`** (clap short help = first line only).
2. Keep `about` / `long_about` / `after_help` honest (formats, examples).
3. **Run and read** (do not skip):
   ```bash
   cargo run --bin rsync-archive -- create -h
   cargo run --bin rsync-archive -- create --help
   cargo run --bin rsync-archive -- embed -h
   ```
4. Extend **`tests/cli_smoke.rs`** for new flag names / key phrases in help.

### 3. Reconcile prose with code

1. Read `src/cli.rs` (and `create -h` / `create --help` / `embed -h`) as flag source of truth.
2. Update **`README.md`**: feature table, flags, status table, quick start, architecture notes.
3. Update **`docs/SELECTION.md`** when filter semantics change (after Stage 4).
4. Update **`AGENTS.md`** defaults table if defaults moved.
5. Grep for stale claims:

```bash
grep -RInE 'solid|whole.file|temp tree|overwrite|--force|files-from|stream' README.md AGENTS.md docs/ || true
```

Fix anything that no longer matches code.

### 4. Defaults that must match code

Verify docs still say (once implemented; until then, say “planned” accurately):

- Create default: **non-solid 7z**; also seekable-zstd / tar-zstd / tar-lz4; level default **5**
- Embed: **store/Copy**; default **basename** naming
- Overwrite: error unless **`--force`**
- Atomic: **`.partial` then rename**
- Streaming create; no solid create

### 5. Commit hygiene

Prefer **one commit** that includes code + help + README updates. If docs/help were forgotten, add a follow-up *before push*:

`docs: sync README and CLI help with <feature>`

### 6. Done criteria

- [ ] **`create -h` / `embed -h`** reflect new flags and correct semantics
- [ ] README reflects current flags and implementation status
- [ ] AGENTS defaults match code (or explicitly “planned”)
- [ ] No solid-create claims; create vs embed roles correct
- [ ] `cli_smoke` help tests updated if flags changed
- [ ] If behavior changed, **keep-tests-current** also run
- [ ] Intended docs staged with the change

## Cross-link

If code behavior changed, also run **`.grok/skills/keep-tests-current/SKILL.md`**.

## Anti-patterns

- Shipping new flags without **clap help** and README tables  
- Vague first-line help that only makes sense with `--help`  
- Claiming streaming create while only whole-file buffers exist (call out size-gate honestly)  
- Documenting solid 7z support  
- Leaving “not implemented” status table stale after a stage lands  
