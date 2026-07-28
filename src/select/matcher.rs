//! Pattern matching, first-match `action_for`, and directory prune.
//!
//! Paths under test are archive-relative, `/`-separated, **no** leading `/`.

use super::rules::{Rule, RuleAction, RuleSet};

impl RuleSet {
    /// First matching rule wins; if none match → [`RuleAction::Include`].
    pub fn action_for(&self, rel_path: &str, is_dir: bool) -> RuleAction {
        let path = normalize_match_path(rel_path);
        for rule in self.rules() {
            if path_matches_rule(rule, path, is_dir) {
                return rule.action;
            }
        }
        RuleAction::Include
    }

    /// Whether walk may skip descending into directory `dir_rel`.
    ///
    /// Prefer walking too much over missing includes (conservative over-approx).
    pub fn should_prune_dir(&self, dir_rel: &str) -> bool {
        let d = normalize_match_path(dir_rel);
        if self.action_for(d, true) == RuleAction::Include {
            return false;
        }
        // D is excluded. Do not prune if any Include rule might match D or under D.
        for rule in self.rules() {
            if rule.action == RuleAction::Include && include_rule_may_match_under(rule, d) {
                return false;
            }
        }
        true
    }
}

/// Strip accidental leading `/` and trailing `/` for match paths.
fn normalize_match_path(p: &str) -> &str {
    let p = p.trim_matches('/');
    p
}

fn basename(path: &str) -> &str {
    match path.rfind('/') {
        Some(i) => &path[i + 1..],
        None => path,
    }
}

/// Whether `rule` matches archive-relative `path` (`is_dir` for dir-only rules).
pub fn path_matches_rule(rule: &Rule, path: &str, is_dir: bool) -> bool {
    let path = normalize_match_path(path);
    if path.is_empty() {
        return false;
    }

    if rule.dir_only {
        if is_dir {
            return match_target(rule, path);
        }
        // File: match if any ancestor directory matches as a directory.
        // e.g. `foo/` excludes `foo/bar` and `foo/a/b`.
        for ancestor in path_ancestors(path) {
            if match_target(rule, ancestor) {
                return true;
            }
        }
        return false;
    }

    match_target(rule, path)
}

/// Match against basename or full path per K27 / SELECTION.md (no dir-only handling).
fn match_target(rule: &Rule, path: &str) -> bool {
    if rule.basename_mode() {
        if rule.anchored {
            // Anchored basename: only single-segment paths (`/foo` matches `foo`, not `bar/foo`).
            if path.contains('/') {
                return false;
            }
        }
        return glob_match_segment(&rule.pattern, basename(path));
    }

    // Full relative path (with `/` in pattern).
    glob_match_path(&rule.pattern, path)
}

/// Ancestors of a path, outermost first, excluding the path itself.
///
/// `a/b/c` → `a`, `a/b`
fn path_ancestors(path: &str) -> impl Iterator<Item = &str> {
    let bytes = path.as_bytes();
    let mut positions = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'/' && i > 0 {
            positions.push(i);
        }
    }
    positions.into_iter().map(move |i| &path[..i])
}

// ---------------------------------------------------------------------------
// Glob: `*` one segment, `**` across segments, `?` one non-slash char
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    /// One path segment pattern (may contain `*` and `?`, not `/`).
    Seg(String),
    /// `**` — zero or more full segments.
    AnyDirs,
}

fn tokenize_pattern(pat: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for part in pat.split('/') {
        if part == "**" {
            // Collapse consecutive **
            if !matches!(out.last(), Some(Tok::AnyDirs)) {
                out.push(Tok::AnyDirs);
            }
        } else {
            out.push(Tok::Seg(part.to_string()));
        }
    }
    out
}

/// Full-path glob match (pattern may contain `/`, `*`, `**`, `?`).
pub fn glob_match_path(pat: &str, path: &str) -> bool {
    let toks = tokenize_pattern(pat);
    let segs: Vec<&str> = if path.is_empty() {
        Vec::new()
    } else {
        path.split('/').collect()
    };
    match_tokens(&toks, 0, &segs, 0)
}

fn match_tokens(toks: &[Tok], ti: usize, segs: &[&str], si: usize) -> bool {
    if ti >= toks.len() {
        return si >= segs.len();
    }
    match &toks[ti] {
        Tok::AnyDirs => {
            // Match zero or more segments.
            for k in si..=segs.len() {
                if match_tokens(toks, ti + 1, segs, k) {
                    return true;
                }
            }
            false
        }
        Tok::Seg(p) => {
            if si >= segs.len() {
                return false;
            }
            if glob_match_segment(p, segs[si]) {
                match_tokens(toks, ti + 1, segs, si + 1)
            } else {
                false
            }
        }
    }
}

/// Match a single path segment against a segment pattern (`*`, `?`, literals).
///
/// `*` does not cross `/` (segment has none). `**` inside a segment is treated
/// as two `*` (unusual); prefer `**` as its own path segment in patterns.
pub fn glob_match_segment(pat: &str, seg: &str) -> bool {
    glob_match_segment_bytes(pat.as_bytes(), seg.as_bytes())
}

fn glob_match_segment_bytes(pat: &[u8], seg: &[u8]) -> bool {
    let mut pi = 0;
    let mut si = 0;
    let mut star_pi: Option<usize> = None;
    let mut star_si = 0;

    while si < seg.len() {
        if pi < pat.len() {
            match pat[pi] {
                b'?' => {
                    pi += 1;
                    si += 1;
                    continue;
                }
                b'*' => {
                    // Collapse multiple *
                    while pi < pat.len() && pat[pi] == b'*' {
                        pi += 1;
                    }
                    star_pi = Some(pi);
                    star_si = si;
                    continue;
                }
                c if c == seg[si] => {
                    pi += 1;
                    si += 1;
                    continue;
                }
                _ => {}
            }
        }
        // Backtrack to last *
        if let Some(sp) = star_pi {
            star_si += 1;
            si = star_si;
            pi = sp;
            continue;
        }
        return false;
    }

    // Consume trailing * in pattern
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }
    pi == pat.len()
}

// ---------------------------------------------------------------------------
// Prune helper: may an Include rule match D or something under D?
// ---------------------------------------------------------------------------

/// Conservative: return true unless we can prove no path `D` or `D/...` can match.
pub fn include_rule_may_match_under(rule: &Rule, dir: &str) -> bool {
    let dir = normalize_match_path(dir);
    if dir.is_empty() {
        return true;
    }

    // Could the include match D itself?
    if path_matches_rule(rule, dir, true) {
        return true;
    }

    // Basename-mode: any basename under D might match (e.g. `*.c`, `foo`, `*`).
    // Anchored basename can only match single-segment paths, so cannot match under D
    // (paths under D have at least one `/`). Still could match... no, under D means
    // `D/child` which is multi-segment. Anchored basename never matches multi-segment.
    // But wait: could it match a path equal to something? Under D are multi-segment
    // relative to archive root. Anchored `/foo` never matches `D/foo` if D is non-empty.
    if rule.basename_mode() {
        if rule.anchored {
            // Only single-segment paths match; nothing under a non-empty D is single-segment
            // at archive root. D itself already checked. So: no.
            return false;
        }
        // Unanchored basename can match any final component under D.
        return true;
    }

    // Full-path patterns: check segment compatibility with prefix `dir`.
    if rule.pattern.contains("**") {
        // Hard to prove negative; walk more.
        return true;
    }

    pattern_may_match_under_dir(&rule.pattern, dir, rule.anchored, rule.dir_only)
}

/// Whether a full-path pattern (no `**`) might match `dir` or a path under `dir`.
fn pattern_may_match_under_dir(pat: &str, dir: &str, _anchored: bool, dir_only: bool) -> bool {
    let toks = tokenize_pattern(pat);
    let dir_segs: Vec<&str> = dir.split('/').collect();

    // Can pattern match some path that has `dir` as a prefix (or equals dir)?
    // Try: match dir_segs as a prefix of a successful match, with optional extra segs after.
    if match_tokens_prefix_under(&toks, 0, &dir_segs, 0, dir_only) {
        return true;
    }
    false
}

/// Like match_tokens but after consuming all dir segments, remaining pattern may
/// match additional path segments under the directory.
fn match_tokens_prefix_under(
    toks: &[Tok],
    ti: usize,
    dir_segs: &[&str],
    si: usize,
    dir_only: bool,
) -> bool {
    // Consumed entire directory prefix: remaining pattern can match under it.
    if si >= dir_segs.len() {
        // Remaining tokens: if empty, pattern matched exactly dir (already handled
        // by path_matches_rule usually). Still: pattern might need more segments
        // (e.g. pat `dir/x` under `dir`) → those would be under dir → true if
        // remaining tokens can match some non-empty or empty continuation.
        return remaining_can_match_under(toks, ti, dir_only);
    }

    if ti >= toks.len() {
        // Pattern exhausted but dir still has segments → pattern matches a proper
        // prefix of dir, not dir or under dir as full match... actually matching
        // requires full path match, so a shorter pattern won't match longer paths.
        return false;
    }

    match &toks[ti] {
        Tok::AnyDirs => {
            for k in si..=dir_segs.len() {
                if match_tokens_prefix_under(toks, ti + 1, dir_segs, k, dir_only) {
                    return true;
                }
            }
            false
        }
        Tok::Seg(p) => {
            if glob_match_segment(p, dir_segs[si]) {
                match_tokens_prefix_under(toks, ti + 1, dir_segs, si + 1, dir_only)
            } else {
                false
            }
        }
    }
}

fn remaining_can_match_under(toks: &[Tok], ti: usize, _dir_only: bool) -> bool {
    if ti >= toks.len() {
        // Exact match of dir only — caller already checked path_matches for dir;
        // "under" needs something longer. Exact-only is not "under", but may_match_under
        // includes D itself, already checked. Return false for pure-under.
        // However: dir_only patterns match children via ancestor logic; for include
        // of a dir_only pattern that matched exactly... already handled.
        // For non-dir-only full path that matched exactly dir: path under wouldn't
        // match unless pattern ends with * that can eat more — but tokens empty.
        return false;
    }
    // Any remaining tokens imply there exist longer paths that might match
    // (e.g. Seg("x"), or *). Conservative: true.
    // Exception: if we can prove remaining can't match any string? Unlikely for
    // valid patterns. Always true.
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::select::rules::RuleSet;

    #[test]
    fn glob_segment_basics() {
        assert!(glob_match_segment("*.tmp", "a.tmp"));
        assert!(!glob_match_segment("*.tmp", "a.txt"));
        assert!(glob_match_segment("?.txt", "a.txt"));
        assert!(!glob_match_segment("?.txt", "ab.txt"));
        assert!(glob_match_segment("*", "anything"));
        assert!(glob_match_segment("foo", "foo"));
        assert!(!glob_match_segment("foo", "bar"));
    }

    #[test]
    fn glob_path_double_star() {
        assert!(glob_match_path("**/*.o", "x/y.o"));
        assert!(glob_match_path("**/*.o", "y.o"));
        assert!(glob_match_path("**/*.o", "a/b/c.o"));
        assert!(!glob_match_path("**/*.o", "x/y.c"));
        assert!(glob_match_path("sub/**", "sub"));
        assert!(glob_match_path("sub/**", "sub/a"));
        assert!(glob_match_path("sub/**", "sub/a/b"));
        assert!(!glob_match_path("sub/**", "other/a"));
    }

    #[test]
    fn glob_path_one_star_segment() {
        assert!(glob_match_path("dir/*", "dir/x"));
        assert!(!glob_match_path("dir/*", "dir/sub/x"));
        assert!(!glob_match_path("dir/*", "other/x"));
    }

    #[test]
    fn action_default_include() {
        let rs = RuleSet::new();
        assert_eq!(rs.action_for("any", false), RuleAction::Include);
    }

    #[test]
    fn dir_only_excludes_children() {
        let mut rs = RuleSet::new();
        rs.push_exclude("cache/").unwrap();
        assert_eq!(rs.action_for("cache", true), RuleAction::Exclude);
        assert_eq!(rs.action_for("cache/x", false), RuleAction::Exclude);
        assert_eq!(rs.action_for("cache/a/b", false), RuleAction::Exclude);
        // Exact file named cache (not a dir) is NOT excluded by cache/
        assert_eq!(rs.action_for("cache", false), RuleAction::Include);
        assert_eq!(rs.action_for("other", false), RuleAction::Include);
    }

    #[test]
    fn prune_excluded_no_include() {
        let mut rs = RuleSet::new();
        rs.push_exclude("skipme/").unwrap();
        assert!(rs.should_prune_dir("skipme"));
    }

    #[test]
    fn prune_not_when_include_may_match() {
        let mut rs = RuleSet::new();
        rs.push_include("*.c").unwrap();
        rs.push_exclude("*").unwrap();
        // Directory is excluded by `*`, but `*.c` may match under it.
        assert!(!rs.should_prune_dir("src"));
    }
}
