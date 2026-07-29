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

## Member kinds (files, symlinks, hard links)

Walk and `--files-from` select:

| Kind | How | `SelectedEntry` | Size |
|------|-----|-----------------|------|
| Regular file | `symlink_metadata` is file; first path for its inode | `MemberKind::File` | content length |
| Hard link | Unix: same `(st_dev, st_ino)` as an earlier regular file | `MemberKind::HardLink { target }` (`target` = first path’s `archive_name`) | **0** |
| Symbolic link | not followed; `read_link` target | `MemberKind::Symlink { target }` | **0** |
| Special (fifo/socket/device) | skipped | — | — (counter `skipped_special`) |

- **Hard-link detection (Unix only):** while selecting regular files, keep a map
  `(dev, ino) → first archive_name`. The first occurrence is a full file body;
  later paths for that inode become hard-link members (size 0 so dir budgets /
  `--max-total-size` do not double-count content). Non-Unix builds treat every
  regular file as `File` (no hard-link detection). Cross-device hard links do not
  exist on Unix and need no special case.
- Symlinks are **not** hard links: only `meta.is_file()` paths participate in the
  inode map; symlink inodes never become `HardLink`.
- Filters (`action_for`) apply to the link’s **archive path** (not the target).
- Metadata for links uses **lstat** (`symlink_metadata`): mode/uid/gid/mtime.
- Symlinks and hard links participate in collision checks, dir budgets (size 0),
  and global `--max-files`.
- **Format encoding:**
  - **tar-zstd / tar-lz4:** emit symlink members (ustar typeflag `'2'`) and hard-link
    members (typeflag `'1'`, linkname = first archive path / pax `linkpath` when long).
  - **7z / seekable-zstd:** drop symlink and hard-link entries at create time
    (`skipped_symlinks` / `skipped_hardlinks`); the first regular-file body for each
    hard-linked inode remains.
- Dry-run lists only members that the resolved **output format** will archive (after that filter).

---

## List files overview (master vs restriction)

| Role | Flag(s) | Applied to |
|------|---------|------------|
| **Master collect** | `SRC...` and/or `--files-from` and/or **`--include-cwd`** | What enters the candidate set |
| **Rsync include/exclude** | `--include`/`--exclude`/`--filter` + `*-from` | Filter master set |
| **Per-path file size** | **`--file-size-from`** | Only paths matching a line; others **ignore** this list |
| **Dir byte/count** | `--dir-max-size`, **`--dir-max-size-from`**, `--dir-max-files`, `--dir-max-files-from` | Only listed directory prefixes; others **ignore** |
| **Global** | `--max-size`, `--min-size`, `--newer-than`, `--max-total-size`, `--max-files` | **All** remaining candidates when set |

**Ignore rule:** a restriction list never drops a master-list entry unless that entry
matches a rule/prefix in **that** list. Unlisted paths skip that list entirely.

Implementation: `src/select/restrict_lists.rs`, `dir_budget.rs`, `global_restrict.rs`.

### Restriction list line format (rsync-like + fields)

Blank lines and `#` comments skipped. Same **10 MiB / 1_000_000 line** caps as filter files.

**`--file-size-from`** (max only; no `min=`):

```text
# PATTERN  max=SIZE
**/*.log          max=100M
var/log/app.log   max=10M
max=50M           core
```

- Pattern uses the same glob rules as rsync filters (`*`, `**`, `?`, basename if no `/`).
- Exactly one `max=SIZE` per line. **First matching line wins.**
- Paths not matched by any line are not size-capped by this file.

**`--dir-max-size-from`**:

```text
# DIR/  max=SIZE  [files=N]
logs/             max=500M
logs/             files=100
cache/            max=1G files=50
logs/=100M        # legacy PATH=SIZE (size only)
```

- Prefix normalized like archive dirs (trailing `/` optional).
- `files=N` merges into the same file-count limit set as `--dir-max-files`.
- Only files under listed prefixes are budgeted/counted.

**`--dir-max-files-from`**: legacy `PATH=N` **or** `PATH/ files=N` (files only; use
`--dir-max-size-from` for `max=`).

---

## Directory size budgets (`--dir-max-size`)

After normal include/exclude selection (and collision checks), optional per-directory
**byte budgets** cap how much is selected under a given archive-relative directory.

| Flag | Format | Example |
|------|--------|---------|
| `--dir-max-size` (repeatable) | `PATH=SIZE` | `--dir-max-size logs/=100M` |
| `--dir-max-size-from` | rsync-like list (above) | `--dir-max-size-from dirs.txt` |

- **PATH** — archive-relative directory prefix (trailing `/` optional; normalized like member paths; no `..`).
- **SIZE** — same syntax as encode budgets (`100M`, `1G`, `500K`, raw bytes).
- **Order of operations:** see pipeline below.
- **Scope:** recursive regular files whose `archive_name` is under `PATH/` (not a file named exactly `PATH`).
- **Ordering:** under each budget, sort by **mtime descending**, then `archive_name` ascending.
- **Accumulation:** include while `running_sum + size ≤ limit`; further files are **budget-skips** (not rsync excludes).
- **Nesting:** if multiple budgets match a file, the **longest matching prefix** wins; budgets apply independently per group.
- **Logging (compact):** one stderr block per budgeted dir after selection — summary line with kept/skip counts and byte totals, then `kept:` / `skip:` path:size lists (capped; `+N` for overflow). No per-file warn spam.
- **Counters:** `SelectionStats.skipped_dir_budget`.
- **Dry-run:** same `build_selection` path as write; restriction report on stderr; selected names on stdout.

Implementation: `src/select/dir_budget.rs` (`RestrictionReport`).

---

## Directory file-count limits (`--dir-max-files`)

After filters and optional size budgets, optional per-directory **file-count**
limits cap how many selected files are kept under a directory **tree**.

| Flag | Format | Example |
|------|--------|---------|
| `--dir-max-files` (repeatable) | `PATH=N` | `--dir-max-files logs/=10` |
| `--dir-max-files-from` | `PATH=N` or `PATH/ files=N` | `--dir-max-files-from limits.txt` |

- **PATH** — archive-relative directory prefix (trailing `/` optional; same normalization as size budgets; no `..`).
- **N** — non-negative integer (max files kept under the tree).
- **Scope:** **recursive** under `PATH/` (same as size budgets). Nested limits: **longest matching prefix** wins. Collection filters are an independent rule set.
- **Ordering:** under each limit, sort by **mtime descending**, then `archive_name` ascending; keep the first `N`.
- **List file:** blank lines and `#` comments skipped; same size/line caps as other from-files; duplicate prefixes (CLI ↔ file) error.
- **Logging (compact):** same style as size budgets — `dir-max-files PATH/=N: kept … skip …` plus path:size lists.
- **Counters:** `SelectionStats.skipped_dir_file_limit`.
- **Dry-run:** same `build_selection` path as write.

Implementation: `src/select/dir_budget.rs` (`DirFileLimit`, `apply_dir_file_limits`, `RestrictionReport`).

---

## Global / log-collection restrictions

Post-filter caps for size-cautious packs (e.g. off-host logs). Same path for dry-run and write.
Soft-fail: excess files are **skipped** with a compact stderr report (not hard error unless the
final selection is empty on write).

### Order of operations (`build_selection`)

1. Master list: rsync filters / walk (`--include` / `--exclude` / `--files-from` / optional **`--include-cwd`**, …)

### `--include-cwd` (optional, default off)

When set, walk the **process current working directory** with trailing-slash semantics:
member names are archive-**root** relative (`./a.txt` → `a.txt`, not `cwdname/a.txt`).

- Combines with `SRC...` or `--files-from` (merge; `archive_name` collisions error).
- May be used **alone** (no `SRC` / `--files-from`).
- Always skips the create **`-o`** path and its **`{out}.partial`** sibling so the tool
  does not archive its own output or in-progress temp.
- Same include/exclude / restriction pipeline as other selected files.
2. **Global per-file:** `--max-size`, `--min-size`, `--newer-than` (all candidates)
3. **`--file-size-from`:** only matching patterns (first match wins)
4. **Directory:** `--dir-max-size` / `--dir-max-size-from` then `--dir-max-files` / `--dir-max-files-from`
5. **Global:** `--max-total-size` then `--max-files`

| Flag | Format | Behavior |
|------|--------|----------|
| `--max-size SIZE` | same as encode budgets (`100M`, …) | Skip any single file with `size > SIZE` |
| `--min-size SIZE` | same | Skip files with `size < SIZE`; **`0` or omit = off** |
| `--newer-than DURATION` | `7d` / `24h` / `30m` / `90s` / bare seconds | Keep only files with mtime ≥ now − DURATION |
| `--max-total-size SIZE` | same size syntax | Global selected-byte cap; **newest mtime first**, then `archive_name` asc; keep while `sum + size ≤ limit` |
| `--max-files N` | non-negative integer | Global max member count; **newest mtime first** |

- **Logging (compact):** stderr blocks such as `max-size: skip N (bytes)`, `min-size: …`,
  `newer-than: …`, `max-total-size=…: kept … skip …`, `max-files=N: kept … skip …` with
  capped path:size lists (`+N` overflow). Shared with dir restriction report.
- **Counters:** `SelectionStats.skipped_max_size`, `skipped_min_size`, `skipped_older_than`,
  `skipped_max_total_size`, `skipped_max_files`.
- **Duration parse:** integer + optional suffix `d`/`h`/`m`/`s` only (no floats).

Implementation: `src/select/global_restrict.rs`; report fields on `RestrictionReport`.

---

## Future restriction ideas (log collection / size-cautious)

Brainstorm for off-host log/forensic packs (not all implemented). Prefer options that fail soft with a compact report.

| Idea | Flag sketch | Why useful |
|------|-------------|------------|
| ~~Global archive size budget~~ | **`--max-total-size`** | **Done** |
| ~~Global file count~~ | **`--max-files`** | **Done** |
| ~~Per-file max/min size~~ | **`--max-size` / `--min-size`** | **Done** |
| ~~Age window (recent)~~ | **`--newer-than`** | **Done** (`--older-than` still open) |
| Head/tail of large files | `--file-head-bytes` / `--file-tail-bytes` | Sample huge logs without full copy |
| Free-disk guard | `--min-free-space SIZE` | Refuse write if destination volume too full |
| Output path quota | `--output-max-size` | Stop encoding when archive would exceed |
| Rate / inode cap | `--max-inodes` / walk depth | Huge trees under `/var` |
| Dedup by content hash | optional later | Same rotated log hardlinks |
| Priority paths | `--prefer PATH` newest-first global | Incident dirs first under global budget |
| Sample rate | `--sample 1/N` | Statistical collect when full pack impossible |
| Deny patterns always | already: `--exclude` | Secrets, keys, `/proc` |
| Soft vs hard fail | `--restriction-strict` | Empty after limits → error vs empty dry-run ok |
| Older-than / absolute mtime | `--older-than` / `--mtime-after` | Full age window |

See also Stage 9 in `docs/DESIGN.md`.

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
| Dir size budgets | newest-mtime-first; recursive under PATH/; longest prefix; post-filter |
| Dir file-count limits | newest-mtime-first; **recursive** under PATH/; longest prefix; post-filter |
| Per-file max/min size | `--max-size` / `--min-size` (`0`=off); after filters, before dir budgets |
| Newer-than | `--newer-than` duration (`7d`/`24h`/`30m`/`s`); mtime window |
| Global max-total-size / max-files | newest-mtime-first; after dir limits |
