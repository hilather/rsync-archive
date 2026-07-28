# Selection & rsync filter semantics (v1)

This document is the **behavior source of truth** for Stage 4+ path selection
filters in rsync-archive. Implementation: `src/select/{rules,matcher,from_file}.rs`.
Walk / create dry-run wiring is Stage 5.

Related: [`DESIGN.md`](DESIGN.md) (K26, K27, prune algorithm), [`README.md`](../README.md).

---

## Scope (v1 frozen)

| Feature | v1 |
|---------|----|
| `--exclude=PATTERN` | Yes |
| `--include=PATTERN` | Yes |
| `--exclude-from=FILE` | Yes |
| `--include-from=FILE` | Yes |
| `--filter='+ …'` / `- …` | Yes (basic) |
| Patterns `*`, `**`, `?` | Yes |
| Anchored `/pattern` | Yes |
| Directory-only `pattern/` | Yes |
| First-match-wins | Yes |
| Default if no rule matches | **Include** |
| Include-only idiom | `--include='*.c' --exclude='*'` (order matters) |
| Filter/from file size cap | **10 MiB** or **1_000_000 lines** per file |
| `--files-from` list reading | Line/caps helper present; full mode Stage 5 |

### Not in v1

- `--merge` / `--ffilter` / dir-merge / per-directory `.rsync-filter`
- protect / risk / hide / show
- `--cvs-exclude`, `-0` / `--from0`
- Regex excludes, charset rules

---

## Paths under test

Matching uses **archive-relative** paths:

- `/`-separated
- **No** leading `/` (e.g. `dir/a.tmp`, not `/dir/a.tmp`)
- No `..` segments (rejected at normalize time for archive names)
- Directory paths for prune / dir rules have **no** trailing `/` (e.g. `cache`, not `cache/`)

When walk lands (Stage 5), filters are applied to the planned **archive member name**
(after SRC trailing-slash mapping). `--files-from` matches against its
`archive_name` rules (Stage 5).

---

## Rule list

Rules are **ordered**. Evaluation:

1. For each rule in order, test whether the pattern matches the path (and `is_dir`).
2. **First match wins** → that rule’s action (`Include` or `Exclude`).
3. If **no** rule matches → **Include**.

CLI / API build order is append order:

| Source | Action for bare pattern |
|--------|-------------------------|
| `--exclude PAT` / `push_exclude` | Exclude |
| `--include PAT` / `push_include` | Include |
| `--filter '+ PAT'` | Include |
| `--filter '- PAT'` | Exclude |
| `--filter 'include PAT'` / `'exclude PAT'` | Include / Exclude (long form) |
| `--exclude-from` bare line | Exclude |
| `--include-from` bare line | Include |
| from-file line with `+`/`-`/`include`/`exclude` prefix | Explicit action (overrides file default) |

### Filter line syntax

```text
+ pattern
- pattern
+pattern          # space optional after +/-
include pattern   # case-insensitive keyword, then whitespace
exclude pattern
```

Empty lines and lines whose first non-whitespace character is `#` are skipped.

### From-file format

- One pattern (or filter line) per line
- UTF-8 text
- `#` starts a full-line comment (after trim)
- Blank lines skipped
- Size/line caps enforced before parse (see below)

### Size / line caps

Per filter or list file:

| Limit | Value |
|-------|------:|
| Max bytes | **10 MiB** (`10 * 1024 * 1024`) |
| Max lines | **1_000_000** |

Exceeding either → `Error::FilterFileTooLarge`.

---

## Pattern parse

For a raw pattern string `pat`:

1. **Dir-only:** if `pat` ends with `/`, set `dir_only = true` and strip trailing `/`
   characters. Empty after strip → parse error.
2. **Anchored:** if remaining starts with `/`, set `anchored = true` and strip leading `/`.
   Empty after strip → parse error.
3. Reject empty path segments (`a//b`).
4. `\` in patterns is normalized to `/`.

Stored fields: `action`, `pattern` (stripped), `anchored`, `dir_only`.

**Basename mode (K27):** if the stored `pattern` contains **no** `/`, matching uses
the path’s **basename** only; otherwise the **full** relative path.

Examples:

| Raw pattern | anchored | dir_only | basename mode | stored pattern |
|-------------|----------|----------|---------------|----------------|
| `*.tmp` | no | no | yes | `*.tmp` |
| `/foo` | yes | no | yes | `foo` |
| `cache/` | no | yes | yes | `cache` |
| `/cache/` | yes | yes | yes | `cache` |
| `dir/*` | no | no | no | `dir/*` |
| `**/*.o` | no | no | no | `**/*.o` |

---

## Pattern match algorithm

### Wildcards

| Token | Meaning |
|-------|---------|
| `*` | Within one path segment: any characters except `/` |
| `?` | One character except `/` |
| `**` | Zero or more **full path segments** (only as a `/`-separated segment, e.g. `a/**/b`) |

Full-path patterns are split on `/` into tokens (`**` vs ordinary segment globs).
A path matches only if the whole path matches (not a partial suffix match).

### Target selection

1. If **basename mode** (no `/` in stored pattern):
   - Match `pattern` against the path’s final component only.
   - Example: `*.tmp` matches `dir/a.tmp` because basename `a.tmp` matches.
   - If **anchored**: match only when the path is a **single segment**
     (`/foo` matches `foo`, not `bar/foo`).
2. If **full-path mode** (pattern contains `/`):
   - Match `pattern` against the entire relative path with segment-aware globs.
   - Example: `dir/*` matches `dir/x` but not `dir/sub/x` (`*` does not cross `/`).
   - Example: `**/*.o` matches `x/y.o` and `y.o`.

### Dir-only application

When `dir_only` is true:

| Subject | Behavior |
|---------|----------|
| **Directory** `D` | Pattern matches if the basename/full-path rule matches `D` |
| **File** | Matches if **any ancestor directory** of the file would match as a directory (prefix). Example: `foo/` excludes `foo/bar` and `foo/a/b`. Exact file path `foo` (non-dir) is **not** excluded by `foo/` alone |

### First match + default

After the first matching rule’s action is taken, remaining rules are ignored.
No match → **Include**.

### Include-only idiom

```bash
--include '*.c' --exclude '*'
```

- `a.c` matches include first → Include  
- `a.o` fails include, matches exclude `*` → Exclude  
- Order matters: `--exclude '*' --include '*.c'` excludes everything first.

---

## Directory prune

When a walk reaches directory relative path `D` (no trailing slash):

```text
should_prune_dir(D):
  if action_for(D, is_dir=true) == Include:
    return false   # do not prune

  # D is Excluded. Over-approx: do not prune if ANY Include rule
  # (any position) might match D or a path under D.
  for each Include rule:
    if include_rule_may_match_under(rule, D):
      return false
  return true
```

### `include_rule_may_match_under`

Return **true** unless it is clear that neither `D` nor any `D/...` can match:

| Situation | Result |
|-----------|--------|
| Rule already matches `D` as a directory | true (do not prune) |
| Basename-mode, unanchored (e.g. `*.c`, `foo`, `*`) | true (some basename under D may match) |
| Basename-mode, **anchored** | false if D is non-empty (anchored basename only matches single-segment root paths; D itself already tested) |
| Full-path pattern contains `**` | true (conservative) |
| Full-path without `**` | true if pattern segments are compatible with prefix `D` (pattern can match `D` or longer paths under `D`); false if clearly disjoint (e.g. anchored/full `other/...` vs dir `skipme`) |

**Invariant:** Prefer **walking too much** (false negative on prune) over **missing Included files** (false positive prune).

Examples:

| Rules | Dir | Prune? |
|-------|-----|--------|
| exclude `skipme/` | `skipme` | **yes** (no include can match under) |
| include `*.c`, exclude `*` | `src` | **no** (`*.c` may match under) |
| include `skipme/keep.txt`, exclude `skipme/` | `skipme` | **no** (include may match under) |
| include `/other`, exclude `skipme/` | `skipme` | **yes** |

---

## API sketch

```rust
pub enum RuleAction { Include, Exclude }

pub struct Rule {
    pub action: RuleAction,
    pub pattern: String,  // stripped
    pub anchored: bool,
    pub dir_only: bool,
}

pub struct RuleSet { /* ordered rules */ }

impl RuleSet {
    pub fn new() -> Self;
    pub fn push_exclude(&mut self, pat: &str) -> Result<()>;
    pub fn push_include(&mut self, pat: &str) -> Result<()>;
    pub fn push_filter_line(&mut self, line: &str) -> Result<()>;
    pub fn action_for(&self, rel_path: &str, is_dir: bool) -> RuleAction;
    pub fn should_prune_dir(&self, dir_rel: &str) -> bool;
}

// from_file
load_exclude_from(&mut rules, path)?;
load_include_from(&mut rules, path)?;
load_filter_from(&mut rules, path)?;
read_capped_lines(path)?;  // shared with future files-from
```

---

## Representative parity cases

| # | Rules (ordered) | Path | is_dir | Expect |
|---|-----------------|------|--------|--------|
| 1 | exclude `*.tmp` | `a.tmp` | F | Exclude |
| 2 | exclude `*.tmp` | `a.txt` | F | Include |
| 3 | exclude `*.tmp` | `dir/a.tmp` | F | **Exclude** (basename) |
| 4 | exclude `dir/` | `dir` | T | Exclude |
| 5 | exclude `dir/` | `dir/x` | F | Exclude |
| 6 | include `*.c` then exclude `*` | `a.c` | F | Include |
| 7 | exclude `*` then include `*.c` | `a.c` | F | Exclude |
| 8 | exclude `/foo` | `foo` | F | Exclude |
| 9 | include `/foo` | `foo` | F | Include |
| 10 | exclude `/foo` | `bar/foo` | F | Include (no match) |
| 11 | exclude `**/*.o` | `x/y.o` | F | Exclude |
| 12 | exclude `?.txt` | `a.txt` | F | Exclude |
| 13 | exclude `?.txt` | `ab.txt` | F | Include |
| 14 | include `sub/**` then exclude `*` | `sub/a` | F | Include |
| 15 | exclude `*` only | `any` | F | Exclude |
| 16 | (no rules) | `any` | F | Include |
| 17 | filter `- *.log` then `+ keep.log` | `keep.log` | F | Exclude (first match) |
| 18 | filter `+ keep.log` then `- *.log` | `keep.log` | F | Include |
| 19 | exclude `foo` | `foo` | F | Exclude |
| 20 | exclude `foo` | `bar/foo` | F | **Exclude** (basename) |
| 21 | exclude `bar/foo` | `bar/foo` | F | Exclude |
| 22 | exclude `bar/foo` | `foo` | F | Include |
| 23 | exclude `dir/*` | `dir/x` | F | Exclude |
| 24 | exclude `dir/*` | `dir/sub/x` | F | Include (`*` one segment) |
| 25 | include-from with `#` comments | (parse) | — | comments ignored |

Automated: `tests/filter_parity.rs` (extends this table).

---

## Stage boundary

| Done in Stage 4 | Stage 5+ |
|-----------------|----------|
| Rule parse / match / prune predicate | `walkdir` + SRC mapping |
| from-file load + caps | Apply rules during walk; canary prune test |
| Unit + parity tests | `--files-from` exclusive mode + archive_name |
| This document | Dry-run listing selected archive paths |

---

## Directory size budgets (`--dir-max-size`)

After normal include/exclude selection (and collision checks), optional per-directory
**byte budgets** cap how much is selected under a given archive-relative directory.

| Flag | Format | Example |
|------|--------|---------|
| `--dir-max-size` (repeatable) | `PATH=SIZE` | `--dir-max-size logs/=100M` |

- **PATH** — archive-relative directory prefix (trailing `/` optional; normalized like member paths; no `..`).
- **SIZE** — same syntax as encode budgets (`100M`, `1G`, `500K`, raw bytes).
- **Order of operations:** rsync filters first → budget post-process on the candidate list.
- **Scope:** recursive regular files whose `archive_name` is under `PATH/` (not a file named exactly `PATH`).
- **Ordering:** under each budget, sort by **mtime descending**, then `archive_name` ascending.
- **Accumulation:** include while `running_sum + size ≤ limit`; further files are **budget-skips** (not rsync excludes).
- **Nesting:** if multiple budgets match a file, the **longest matching prefix** wins; budgets apply independently per group.
- **Logging:** each budget-skip is logged at warn with path, size, mtime, budget dir/limit, running sum.
- **Counters:** `SelectionStats.skipped_dir_budget`.
- **Dry-run:** same `build_selection` path as write.

Implementation: `src/select/dir_budget.rs`.

---

## Defaults checklist (must stay accurate)

| Topic | Rule |
|-------|------|
| No match | **Include** |
| First match | wins |
| No `/` in pattern | **basename** match (K27) |
| Trailing `/` | directory-only |
| Leading `/` | anchored |
| `*` | one segment |
| `**` | across segments |
| Filter file caps | 10 MiB / 1M lines |
| Merge / dir-merge | **not** implemented |
| Dir size budgets | newest-mtime-first; longest prefix; post-filter |
