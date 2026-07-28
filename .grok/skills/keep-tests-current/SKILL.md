---
name: keep-tests-current
description: >
  Ensure every behavior-changing change in rsync-archive has automated regression
  tests in the same PR. Use when committing, finishing a PR, changing CLI,
  filters, create/embed writers, streaming/codec, error paths, or bug fixes; also
  when the user says "add tests", "regression", "coverage", or runs
  /keep-tests-current. Complements AGENTS.md non-negotiable test policy. Pair
  with keep-docs-current when user-visible.
---

# Keep tests current (K30)

## When to run

Invoke **before every commit** when the session touched:

- Any Rust under `src/` or `tests/`
- CLI, selection, archive, pipeline, errors
- Bug fixes or behavior-affecting refactors

Also run when finishing a PR or when the user asks for regression coverage.

## Policy (summary)

Canonical policy: repo-root **`AGENTS.md`** → **“Non-negotiable: tests cover every change”**.

- Same change / same PR must include tests for behavior diffs.
- Bug fixes: **red–green** regression test.
- `#[ignore]` does **not** count as coverage for core paths.
- CI may run `cargo test`; **agents own writing the tests**.

## Procedure

### 1. Diff and classify

```bash
git status
git diff --stat
git diff
```

Map each change to the AGENTS checklist:

| Change type | Required tests |
|-------------|----------------|
| New CLI flag / default | Parse/unit + create/embed e2e or lib API test using the flag |
| Filter/selection rule or match fix | Table-driven unit case(s); dry-run list assertion if walk-related |
| Create/embed writer fix | Roundtrip list/test + extract sample; non-solid assert if relevant |
| Streaming/codec change | Multi-chunk data; large-file path if size-related |
| Bug fix | Regression that failed before the fix |
| Error-path / exit-code | Assert exit code and/or error variant |
| Docs-only / comment-only | No new tests required |
| Pure refactor | Full suite green; no coverage drop without replacement |

### 2. Add or update tests

- Unit: `#[cfg(test)]` next to logic in `src/`.
- E2e: `tests/*.rs` with `assert_cmd` / tempdirs when CLI or archives are involved.
- For bugs: write the failing test first when practical; confirm it would fail without the fix.

### 3. Run the suite

```bash
cargo test
```

At minimum: packages affected + any new e2e. Prefer full suite before push.

### 4. Guards

- No new `#[ignore]` on core paths without a PR note and follow-up.
- Do not delete tests during refactor without replacement.
- Do not rely on “manual only” for shippable behavior.

### 5. Docs pairing

If user-visible behavior changed, run **keep-docs-current** as well.

## Done criteria

- [ ] Behavior changes have matching tests in this PR
- [ ] `cargo test` green
- [ ] No “manual only” claims for shippable paths
- [ ] Bug fixes have regression coverage
- [ ] Docs skill run if user-visible

## Anti-patterns

- Landing filter/writer changes without tests  
- Using `#[ignore]` to silence failures  
- Deleting tests during refactor without replacement  
- “CI will catch it” without writing a test  
- Skipping red–green for bug fixes  

## Commands (reference)

```bash
cargo test
cargo test --test filter_parity   # when present
cargo test --test e2e_create      # when present
cargo test --test e2e_embed       # when present
```
