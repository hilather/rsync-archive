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

- CLI (`src/cli.rs`, create/embed flags or defaults)
- Selection, pipeline, archive writers, streaming/codec
- Anything user-visible that README or AGENTS still describes differently

Also run when the user asks to document, release, or “make sure README is up to date.”

## Policy (summary)

Canonical agent policy: repo-root **`AGENTS.md`**. This skill operationalizes the **docs** half.

| Commit to git | Do not commit |
|---------------|---------------|
| `README.md` | `target/**` |
| `AGENTS.md` (if defaults/map changed) | Multi-MB archives / fixtures |
| `docs/SELECTION.md` (when present) | `*.partial` leftovers |
| This skill if policy changes | Huge `benchdata/**` |

## Procedure

### 1. Diff the change set

```bash
git status
git diff --stat
```

List which of: **CLI**, **create/embed**, **selection**, **streaming**, **defaults** actually changed.

### 2. Reconcile prose with code

1. Read `src/cli.rs` (and `create --help` / `embed --help`) as flag source of truth.
2. Update **`README.md`**: feature table, flags, status table, quick start, architecture notes.
3. Update **`docs/SELECTION.md`** when filter semantics change (after Stage 4).
4. Update **`AGENTS.md`** defaults table if defaults moved.
5. Grep for stale claims:

```bash
grep -RInE 'solid|whole.file|temp tree|overwrite|--force|files-from|stream' README.md AGENTS.md docs/ || true
```

Fix anything that no longer matches code.

### 3. Defaults that must match code

Verify docs still say (once implemented; until then, say “planned” accurately):

- Create: **non-solid** only; level default **5**
- Embed: **store/Copy**; default **basename** naming
- Overwrite: error unless **`--force`**
- Atomic: **`.partial` then rename**
- Regular files only; streaming create; no solid create

### 4. Commit hygiene

Prefer **one commit** that includes code + doc updates. If docs were forgotten, add a follow-up *before push*:

`docs: sync README with <feature>`

### 5. Done criteria

- [ ] README reflects current flags and implementation status
- [ ] AGENTS defaults match code (or explicitly “planned”)
- [ ] No solid-create claims; create vs embed roles correct
- [ ] If behavior changed, **keep-tests-current** also run
- [ ] Intended docs staged with the change

## Cross-link

If code behavior changed, also run **`.grok/skills/keep-tests-current/SKILL.md`**.

## Anti-patterns

- Shipping new flags without README tables  
- Claiming streaming create while only whole-file buffers exist (call out size-gate honestly)  
- Documenting solid 7z support  
- Leaving “not implemented” status table stale after a stage lands  
