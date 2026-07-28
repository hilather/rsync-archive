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

**Suggested implementation order:** after Stage 6 create write (so archives actually contain the budgeted set).

**Acceptance ideas:**

1. Dir with files 10/20/30 MiB, budget 35 MiB, newest = 30 then 20 then 10 → select 30 only (or 30+… depending mtimes); log the rest.  
2. Dry-run listing matches write membership for budgeted dirs.  
3. Verbose/stderr shows budget exclusions distinctly from `--exclude` patterns.

---

## Create / archive

_(none beyond DESIGN stages 6–8)_

---

## Embed

_(none beyond DESIGN stages)_
