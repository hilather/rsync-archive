# Rsync filter parity audit (v1)

Audit of **rsync-archive** selection filters against **rsync 3.x** man-page
[FILTER RULES](https://download.samba.org/pub/rsync/rsync.1#FILTER_RULES)
/ pattern-matching rules.

| | |
|--|--|
| **Sources of truth (us)** | `docs/SELECTION.md`, `src/select/{rules,matcher,from_file,walk}.rs`, `src/pipeline/create.rs` (`build_rules`), `tests/filter_parity.rs` |
| **Sources of truth (rsync)** | rsync 3.x man page: first-match, default include, basename vs full path, `/` anchor, trailing `/`, `**`, prune-on-exclude |
| **Scope** | Selection only (not transfer/delete, xattrs, daemon modules) |
| **Date** | 2026-07-30 |

Related: [`SELECTION.md`](SELECTION.md) (frozen v1 behavior), [`DESIGN.md`](DESIGN.md) (K27).

---

## Intentional v1 non-goals

These are **out of scope** for the frozen filter dialect (not bugs unless docs claim otherwise):

| Feature | Notes |
|---------|--------|
| `--merge` / `.` merge-file rules | Single global rule list only |
| `--ffilter` / dir-merge `:` / per-dir `.rsync-filter` / `-F` | No per-directory rule inheritance |
| protect / risk / hide / show (`P`/`R`/`H`/`S`) | Sender/receiver dual-sided filters N/A (we only select) |
| Filter modifiers (`s`, `r`, `p`, `!`, `/` absolute, `C`, `x`) | No comma-modifier syntax after `+`/`-` |
| List-clearing `!` rule | Not parsed |
| `--cvs-exclude` / `-C` | Not implemented |
| `-0` / `--from0` | Line-oriented UTF-8 only |
| Character classes `[a-z]` / `[[:alpha:]]` | Only `*`, `?`, `**` |
| Trailing `***` shorthand | rsync: `dir/***`` ≡ `dir/` + `dir/**`; use two rules or `dir/**` + dir include |
| Absolute-path filter match (`-/ /etc/passwd`) | Patterns are archive-relative only |
| Delete-side filter semantics | No receiver file list |
| Regex excludes / charset / iconv name rules | N/A |
| Whole-tree temp rsync then pack | In-process walk + filters |

---

## Known matches (aligned with rsync 3.x)

| Behavior | rsync | rsync-archive v1 | Evidence |
|----------|-------|------------------|----------|
| **First-match-wins** | First matching rule decides | Same in `RuleSet::action_for` | `matcher.rs`; parity cases 6–7, 17–18 |
| **Default include** | Unmatched → include in file list | Unmatched → `RuleAction::Include` | SELECTION.md; case 16 |
| **Basename patterns** | No non-trailing `/` and no `**` → match final component | No `/` and no `**` in stored pattern → basename (K27); bare `**` is full-path | K27; cases 1–3, 19–20, 79 |
| **Anchored `/pat`** | Leading `/` anchors to transfer root | Leading `/` → `anchored`; basename form only matches single-segment paths | Cases 8–10, 32 |
| **Dir-only `pat/`** | Trailing `/` matches directories only | `dir_only`; file named exactly `pat` (not a dir) does not match | Cases 4–5, 25 |
| **Exclude dir short-circuits tree** | Excluded dir → contents not scanned | `should_prune_dir` + walk `filter_entry`; also dir-only exclude matches descendants via ancestors | `walk.rs`, cases 4–5, 28 |
| **Include dir ≠ include contents** | `+ /data/` + `- *` does **not** include files under `data/` | Dir-only **include** matches directory only (not descendants); need `+ /data/**` (or similar) | Recent fix; `matcher.rs` tests; classic idiom documented in SELECTION.md |
| **`*` / `?`** | Non-slash / one non-slash char | Same within a segment | Cases 12–13, 23–24 |
| **`**` as path segments** | Cross-slash wild | Tokenized as full segments (`a/**/b`, `**/*.o`, `sub/**`) | Cases 11, 14, 26–27, 34–35 |
| **Include-only idiom** | `+ *.c` then `- *` | Same when rule order is correct | Case 6 |
| **From-file comments / blanks** | `#` full-line comments; blank skip | Same | `from_file.rs`; case 25 in filter_parity |
| **`+/-` / include/exclude keywords** | Filter lines | `push_filter_line` | rules.rs |
| **include-from / exclude-from bare lines** | Default action per file type; `+/-` overrides | `push_from_file_line` | from_file tests |
| **Conservative prune** | Must not miss includes under excluded dirs when parent dirs still walkable | Prefer over-walk: if any Include may match under D, do not prune | `should_prune_dir`; prune tests |

### Include dir vs include files under dir (detail)

Rsync and we both need **explicit** rules for contents:

```text
+ /data/          # allow walking into data/
+ /data/**        # include files and dirs under data/
- *               # exclude everything else
```

With only `+ /data/` and `- *`, a file `data/elasticsearch/x` does **not** match the dir-only include; it matches `- *` → **exclude**. Directory `data` itself is include so walk may enter; subdirs without a matching include may be pruned when excluded by `*`.

---

## Gaps / bugs (prioritized)

### P0 — wrong results for common CLI usage

| ID | Gap | Detail | Suggested fix (for fix agent) |
|----|-----|--------|-------------------------------|
| **P0-1** | **CLI flag interleaving not preserved** | `build_rules` still uses fixed buckets (see below). Clap does not interleave heterogeneous flags. | **Impact:** `--exclude='*' --include='*.c'` is rewritten as include-then-exclude → **includes `*.c`**, whereas rsync excludes everything (first match). | **Mitigated:** prefer `--filter-from` / repeated `--filter` (documented in CLI help, SELECTION, README). True argv interleave remains open if product needs it. |
| **P0-2** | **Documented order ≠ rsync multi-option order** | Mixed `--include`/`--exclude` cannot interleave. | **Mitigated:** warning in `create -h` / README / SELECTION; `--filter-from` is the supported ordered path. |

### P1 — pattern semantics (status)

| ID | Gap | Detail | Status |
|----|-----|--------|--------|
| **P1-1** | **Unanchored path patterns end-anchored** | rsync: without leading `/`, multi-segment pattern matches path **suffix** (`foo/bar` → `a/foo/bar`). | **Fixed** — `glob_match_path_end_anchored` in `matcher.rs`; parity cases 76–78, 80 |
| **P1-2** | **`**` forces full-path match** | rsync: presence of `**` ⇒ full pathname match (bare `**` not basename). | **Fixed** — `Rule::basename_mode` requires no `/` and no `**`; case 79 |
| **P1-3** | **`include-from` / `exclude-from` multiplicity** | rsync: repeatable; each file at CLI position. | **Partial** — both are `Vec` (repeatable) and load in CLI order **within** their bucket; still not interleaved with other flag types (P0-1). |
| **P1-4** | **`--filter-from` for ordered rules** | rsync merge-file style one-file ordered list. | **Fixed** — create CLI `--filter-from FILE` (repeatable) → `load_filter_from`; slot 3 in `build_rules` |

### P2 — missing wildcards / polish / over-walk

| ID | Gap | Detail | Suggested fix |
|----|-----|--------|---------------|
| **P2-1** | Character classes `[…]` | Not implemented | Optional later |
| **P2-2** | Trailing `***` | Not implemented; users must split rules | Optional sugar: expand `pat/***` → dir-only include/exclude semantics + `pat/**` |
| **P2-3** | Backslash escaping | rsync: `\` escapes wildcards when any wildcard present; we normalize `\` → `/` in patterns | Document; only matter for Windows-ish patterns |
| **P2-4** | `**` mid-token (`a**b`) | rsync can span slashes inside the pattern string; we only special-case `**` as a whole segment | Rare; document |
| **P2-5** | Prune vs rsync exclude | rsync never descends excluded dirs. We may still walk an excluded dir if a later/earlier Include *could* match under it — even when first-match would still exclude those paths (e.g. exclude `skipme/` before include `skipme/keep.txt`). Correct selection, extra I/O. | Optional: prune when no Include that both may_match_under **and** can win first-match (harder analysis) |
| **P2-6** | Filter file caps | We enforce 10 MiB / 1M lines; rsync does not | Intentional safety; keep |
| **P2-7** | Underscore as space in filter rules | rsync allows `-_*.o` instead of `- *.o` | Optional |
| **P2-8** | Transfer-root vs multi-SRC prefixes | rsync anchoring is transfer-root dependent (trailing slash on SRC, `--relative`). We match **archive_name** (after SRC trailing-slash mapping). Document that multi-SRC without trailing slash uses basename prefixes in archive paths — filters must use those names. | Docs + examples only |

### Recently fixed (not gaps)

| Item | Status |
|------|--------|
| Dir-only **include** matching all descendants (false include under `+ /data/` + `- *`) | **Fixed** — include dir-only is directory-only; exclude dir-only still covers descendants via ancestors |

---

## Feature checklist (claimed v1 vs rsync)

| Topic | rsync 3.x | Our v1 | Verdict |
|-------|-----------|--------|---------|
| First-match-wins | Yes | Yes | **Match** |
| Default include | Yes | Yes | **Match** |
| `*` basename vs path with `/` | Basename if no `/` and no `**` | Basename if no `/` and no `**` | **Match** |
| Unanchored multi-segment path | End-anchored (suffix) | End-anchored (suffix) | **Match** |
| Anchored `/pat` | Start of transfer | Start of archive path | **Match** (path model differs; see P2-8) |
| `**` across segments | Yes | Yes (segment tokens) | **Match** for common forms |
| Dir-only `pat/` | Dir only; exclude implies tree skip | Dir only; exclude + prune + ancestor match | **Match** |
| Include dir vs files under | Dir alone does not include files | Same after fix | **Match** |
| Prune / not descending | Exclude dir → no scan | `should_prune_dir` (conservative) | **Match** on correctness; over-walk P2-5 |
| include-from / exclude-from line order | File order at CLI insertion point | In-file order; multi-file CLI order within bucket | **Partial** (P0-1) |
| `--filter` / `--filter-from` line order | CLI / file order | Order among those flags; filter-from before filter | **Match** within those sources |
| build_rules vs rsync CLI order | Full interleave | Bucketed (see below) | **Gap P0-1** (mitigated by filter-from) |

### `build_rules` order (current)

```text
1. --include-from FILE…  (each file CLI order; bare → Include)
2. --exclude-from FILE…  (each file CLI order; bare → Exclude)
3. --filter-from FILE…   (each file CLI order; +/- required)
4. --filter RULE…        (CLI order among filters)
5. --include PATTERN…    (CLI order among includes)
6. --exclude PATTERN…    (CLI order among excludes)
```

Implementation: `src/pipeline/create.rs` → `build_rules`.

**Gap vs rsync:** rsync appends each `--include` / `--exclude` / `--filter` / `*-from` **when it appears on the command line**. Clap does not interleave heterogeneous flags. **Recommended** rsync-compatible workflow:

```bash
# One ordered file (preferred)
rsync-archive create -n SRC/ -o out.7z --filter-from rules.txt

# Or repeated --filter
rsync-archive create -n SRC/ -o out.7z \
  --filter='+ *.c' --filter='- *'

# Bucket order only when it matches intent (all includes before all excludes):
rsync-archive create -n SRC/ -o out.7z \
  --include='*.c' --exclude='*'
```

---

## Recommended regression cases

Unit / `filter_parity` style (path, is_dir, expected action) and walk dry-run where noted.

| Name | Rules (ordered) | Tree / subject | Expected relative paths (or action) |
|------|-----------------|----------------|-------------------------------------|
| `first_match_exclude_star` | `- *`, `+ *.c` | file `a.c` | **Exclude** (first match) |
| `first_match_include_c` | `+ *.c`, `- *` | `a.c`, `a.o` | Include `a.c` only |
| `basename_tmp` | `- *.tmp` | `dir/a.tmp`, `a.txt` | Exclude only `dir/a.tmp` |
| `anchored_foo` | `- /foo` | `foo`, `bar/foo` | Exclude `foo` only |
| `dir_only_exclude` | `- cache/` | `cache/x`, `cache/a/b`, file `cache` | Exclude under `cache/`; file `cache` included |
| `dir_only_include_no_descendants` | `+ /data/`, `- *` | `data/file.txt`, `data/sub/x` | **None** of the files (only dir for walk) |
| `dir_include_with_starstar` | `+ /data/`, `+ /data/**`, `- *` | under `data/…`, `other/x` | All under `data/`; not `other/x` |
| `double_star_o` | `- **/*.o` | `x/y.o`, `y.o`, `x/y.c` | Exclude `.o` paths |
| `one_segment_star` | `- dir/*` | `dir/x`, `dir/sub/x` | Exclude `dir/x` only |
| `sub_double_star` | `+ sub/**`, `- *` | `sub`, `sub/a`, `other` | Include `sub` tree only |
| `unanchored_suffix_path` **(P1-1 fixed)** | `- foo/bar` | `foo/bar`, `a/foo/bar` | Both exclude |
| `starstar_forces_path` **(P1-2 fixed)** | pattern `**` | deep paths | Exclude all |
| `cli_exclude_then_include` **(P0-1 open)** | CLI: `--exclude='*' --include='*.c'` | `a.c` | rsync: exclude; **us: include** (use filter-from) |
| `cli_include_then_exclude` | CLI: `--include='*.c' --exclude='*'` | `a.c` | Include (both) |
| `filter_interleave` | `--filter='- *' --filter='+ *.c'` | `a.c` | Exclude (filter order OK) |
| `filter_from_tree` | `--filter-from` with `+ /data/` `+ /data/**` `- *` | under `data/`, `other/` | Only under `data/` |
| `include_from_before_exclude_from` | from-files with competing patterns | — | Document bucket order |
| `prune_skipme` | `- skipme/` | `keep.txt`, `skipme/secret` | Dry-run: only `keep.txt` |
| `prune_include_under` | `+ skipme/keep.txt`, `- skipme/` | `skipme/keep.txt`, `skipme/other` | Keep only `keep.txt`; no prune of `skipme` |
| `deep_rs` | `+ src/**/*.rs`, `- *` | `src/main.rs`, `lib/main.rs` | Only `src/main.rs` |
| `question_mark` | `- ?.txt` | `a.txt`, `ab.txt` | Exclude `a.txt` only |
| `nested_dir_only` | `- cache/` | `cache/a/b` | Exclude |
| `manpage_deep_file` | `+ x/`, `+ x/y/`, `+ x/y/file.txt`, `- *` | tree from rsync man example | Only `x/y/file.txt` (and dirs for walk) |

Add P0-1 and P1-1 as **red** tests before code changes.

---

## Live rsync comparison methodology

Use a local dry-run against a fixture tree; compare path sets to `rsync-archive create -n`.

### Fixture

```bash
FIX=$(mktemp -d)
mkdir -p "$FIX/src/data/elasticsearch" "$FIX/src/other" "$FIX/src/cache/x" "$FIX/src/sub/a"
echo c > "$FIX/src/a.c"
echo o > "$FIX/src/a.o"
echo t > "$FIX/src/dir/a.tmp" 2>/dev/null || mkdir -p "$FIX/src/dir" && echo t > "$FIX/src/dir/a.tmp"
echo d > "$FIX/src/data/file.txt"
echo e > "$FIX/src/data/elasticsearch/x"
echo k > "$FIX/src/other/x"
echo s > "$FIX/src/cache/x/secret"
echo z > "$FIX/src/sub/a/z"
DEST=$(mktemp -d)
```

### List paths with rsync (no copy)

Prefer **trailing slash on SRC** so transfer-root matches “contents at archive root” (same as our trailing-slash SRC → archive names without top dir):

```bash
# Generic: filter rules via repeated -f, names only
rsync -a -n --out-format='%n' \
  -f '+ /data/' -f '+ /data/**' -f '- *' \
  "$FIX/src/" "$DEST/" \
  | grep -v '/$' \
  | sort
```

Notes:

- `%n` is the filename; directories often appear with trailing `/` — drop them if comparing file-only selection (we select files/symlinks/hardlinks, not empty dir members).
- Use `-a` (implies `-r`) so recursion matches walk.
- For basename excludes: `-f '- *.tmp'`.
- For include-only: `-f '+ *.c' -f '- *'`.
- **CLI order test:** compare  
  `rsync … --exclude='*' --include='*.c'`  
  vs  
  `rsync-archive create -n … --exclude='*' --include='*.c'`.

### List paths with rsync-archive

```bash
cargo run --quiet -- create -n "$FIX/src/" -o /tmp/unused.7z \
  --filter='+ /data/' --filter='+ /data/**' --filter='- *' \
  | sort
```

Dry-run prints selected **archive member** paths (one per line). Sort both lists and `diff -u`.

### One-liner compare helper sketch

```bash
cmp_lists() {
  local name="$1"; shift
  # remaining args: shared filter flags for both tools — keep identical order
  rsync -a -n --out-format='%n' "$@" "$FIX/src/" "$DEST/" \
    | grep -v '/$' | grep -v '^\.$' | sort -u > /tmp/rsync.list
  # map "$@" into rsync-archive --filter / --include / --exclude carefully
  cargo run --quiet -- create -n "$FIX/src/" -o /tmp/unused.7z "$@" \
    | sort -u > /tmp/ra.list
  if ! diff -u /tmp/rsync.list /tmp/ra.list; then
    echo "FAIL: $name"
  else
    echo "OK: $name"
  fi
}
```

Prefer expressing rules as **`--filter=` only** on both sides so order is unambiguous.

### Cases to run live

1. No filters (all regular files under `src/`).
2. `- *.tmp` basename.
3. `+ *.c` / `- *` include-only.
4. `- *` / `+ *.c` first-match (P0-1 for our CLI `--exclude`/`--include`).
5. `- cache/` prune.
6. `+ /data/` + `- *` (no files under data).
7. `+ /data/` + `+ /data/**` + `- *`.
8. `- foo/bar` with path `a/foo/bar` (P1-1).
9. `- **/*.o`.
10. Man-page deep include: `+ x/` `+ x/y/` `+ x/y/file.txt` `- *` on a matching tree.

Record rsync version (`rsync --version`) with any saved diffs.

---

## Fixes status (summary)

Do **not** expand scope into merge/dir-merge/modifiers unless product asks.

| Item | Status |
|------|--------|
| **P1-4** `--filter-from` | **Done** — create CLI, multi-file Append, build_rules slot 3 |
| **P1-3** multi include-from/exclude-from | **Done** within buckets (`Vec` Append) |
| **P1-1** end-anchor unanchored multi-segment | **Done** |
| **P1-2** bare `**` full-path | **Done** |
| **P0-1** true CLI interleave of include/exclude/filter | **Open** — mitigated by filter-from / filter docs |
| **P0-2** document non-interleave | **Done** — SELECTION, README, create help |
| Live rsync suite | `tests/rsync_live_parity.rs` (soft-skip if no rsync) |

### Remaining optional work

1. True argv-order interleave for filter flags (P0-1), or hard error/warn when both `--include` and `--exclude` appear in an order that clap rewrites.
2. P2 items (character classes, `***`, prune over-walk, …) — unchanged.

---

## References

- rsync man page: [FILTER RULES](https://download.samba.org/pub/rsync/rsync.1#FILTER_RULES), [PATTERN MATCHING RULES](https://download.samba.org/pub/rsync/rsync.1#PATTERN_MATCHING_RULES), [ANCHORING](https://download.samba.org/pub/rsync/rsync.1#ANCHORING_INCLUDE_EXCLUDE_PATTERNS)
- Key rsync quotes used in this audit:
  - “The first rule that matches is the one that takes effect.”
  - “The default for any unmatched file/dir is for it to be included.”
  - “When a directory is excluded, all its contents … are also excluded. The sender doesn't scan through any of it.”
  - “If a pattern contains a `/` (not counting a trailing slash) or a `**` … matched against the full pathname… If the pattern doesn't contain a (non-trailing) `/` or a `**`, then it is matched only against the final component…”
  - “A pattern that starts with a `/` is anchored to the start of the transfer path **instead of the end**.”
  - Trailing `***` ≡ directory + all contents in one rule.
