//! Ordered include/exclude rules (rsync v1 subset, no merge).
//!
//! See [`docs/SELECTION.md`](../../../docs/SELECTION.md) and design K27.

use crate::error::{Error, Result};

/// Include or exclude a matched path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Include,
    Exclude,
}

/// One ordered filter rule after pattern parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub action: RuleAction,
    /// Pattern with leading `/` (anchor) and trailing `/` (dir-only) stripped.
    pub pattern: String,
    /// True if the original pattern started with `/` (anchored to path root).
    pub anchored: bool,
    /// True if the original pattern ended with `/` (directory-only).
    pub dir_only: bool,
}

impl Rule {
    /// Patterns with no `/` and no `**` match the basename only (K27 / rsync).
    ///
    /// Presence of `**` forces full-path matching even without `/` (e.g. bare `**`).
    pub fn basename_mode(&self) -> bool {
        !self.pattern.contains('/') && !self.pattern.contains("**")
    }
}

/// Ordered rule list; first match wins; default action is Include.
#[derive(Debug, Clone, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Number of rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Borrow ordered rules.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Append an exclude pattern (as given on CLI / file).
    pub fn push_exclude(&mut self, pat: &str) -> Result<()> {
        let rule = parse_rule(RuleAction::Exclude, pat)?;
        self.rules.push(rule);
        Ok(())
    }

    /// Append an include pattern.
    pub fn push_include(&mut self, pat: &str) -> Result<()> {
        let rule = parse_rule(RuleAction::Include, pat)?;
        self.rules.push(rule);
        Ok(())
    }

    /// Parse a filter line: `+ pattern`, `- pattern`, or long `include` / `exclude`.
    ///
    /// Leading/trailing whitespace is trimmed. Empty lines and `#` comments are
    /// skipped (no-op success).
    pub fn push_filter_line(&mut self, line: &str) -> Result<()> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(());
        }
        let (action, rest) = parse_filter_prefix(line)?;
        let rule = parse_rule(action, rest)?;
        self.rules.push(rule);
        Ok(())
    }

    /// Push a bare pattern with a default action (for include-from / exclude-from).
    ///
    /// If the line already has a `+`/`-`/`include`/`exclude` prefix, that wins;
    /// otherwise `default` is used.
    pub fn push_from_file_line(&mut self, line: &str, default: RuleAction) -> Result<()> {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return Ok(());
        }
        if let Ok((action, rest)) = parse_filter_prefix(line) {
            let rule = parse_rule(action, rest)?;
            self.rules.push(rule);
            return Ok(());
        }
        // Bare pattern
        let rule = parse_rule(default, line)?;
        self.rules.push(rule);
        Ok(())
    }
}

/// Parse `+ …` / `- …` / `include …` / `exclude …` prefix from a non-empty line.
fn parse_filter_prefix(line: &str) -> Result<(RuleAction, &str)> {
    let line = line.trim();
    if let Some(rest) = line.strip_prefix('+') {
        let rest = rest.trim_start();
        if rest.is_empty() {
            return Err(Error::Selection(
                "filter line '+' requires a pattern".into(),
            ));
        }
        return Ok((RuleAction::Include, rest));
    }
    if let Some(rest) = line.strip_prefix('-') {
        let rest = rest.trim_start();
        if rest.is_empty() {
            return Err(Error::Selection(
                "filter line '-' requires a pattern".into(),
            ));
        }
        return Ok((RuleAction::Exclude, rest));
    }
    // Long form: "include PAT" / "exclude PAT" (word + whitespace)
    if let Some(rest) = strip_keyword(line, "include") {
        return Ok((RuleAction::Include, rest));
    }
    if let Some(rest) = strip_keyword(line, "exclude") {
        return Ok((RuleAction::Exclude, rest));
    }
    Err(Error::Selection(format!(
        "invalid filter line (expected +/− or include/exclude): {line}"
    )))
}

fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    if line.len() < kw.len() {
        return None;
    }
    if !line[..kw.len()].eq_ignore_ascii_case(kw) {
        return None;
    }
    let rest = &line[kw.len()..];
    if rest.is_empty() {
        return None;
    }
    // Require whitespace after keyword so "included" is not matched.
    let mut chars = rest.chars();
    let first = chars.next()?;
    if !first.is_whitespace() {
        return None;
    }
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    Some(rest)
}

/// Parse a raw pattern string into a [`Rule`] with the given action.
pub fn parse_rule(action: RuleAction, pat: &str) -> Result<Rule> {
    let pat = pat.trim();
    if pat.is_empty() {
        return Err(Error::Selection("empty filter pattern".into()));
    }

    let mut s = pat.to_string();

    // 1. Dir-only: trailing `/`
    let dir_only = s.ends_with('/');
    if dir_only {
        s.pop();
        // Collapse any extra trailing slashes
        while s.ends_with('/') {
            s.pop();
        }
    }

    // 2. Anchored: leading `/`
    let anchored = s.starts_with('/');
    if anchored {
        s = s.trim_start_matches('/').to_string();
    }

    if s.is_empty() {
        return Err(Error::Selection(format!(
            "invalid filter pattern (empty after strip): {pat:?}"
        )));
    }

    // Normalize `\` → `/` in pattern (path-style patterns from Windows notes)
    let s = s.replace('\\', "/");
    // Reject empty path segments like "a//b"
    for part in s.split('/') {
        if part.is_empty() {
            return Err(Error::Selection(format!(
                "invalid filter pattern (empty segment): {pat:?}"
            )));
        }
    }

    Ok(Rule {
        action,
        pattern: s,
        anchored,
        dir_only,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_simple_exclude() {
        let r = parse_rule(RuleAction::Exclude, "*.tmp").unwrap();
        assert_eq!(r.pattern, "*.tmp");
        assert!(!r.anchored);
        assert!(!r.dir_only);
        assert!(r.basename_mode());
    }

    #[test]
    fn parse_anchored_and_dir_only() {
        let r = parse_rule(RuleAction::Exclude, "/cache/").unwrap();
        assert_eq!(r.pattern, "cache");
        assert!(r.anchored);
        assert!(r.dir_only);
        assert!(r.basename_mode());
    }

    #[test]
    fn parse_full_path_pattern() {
        let r = parse_rule(RuleAction::Exclude, "dir/*").unwrap();
        assert_eq!(r.pattern, "dir/*");
        assert!(!r.basename_mode());
        assert!(!r.anchored);
    }

    #[test]
    fn bare_double_star_not_basename_mode() {
        let r = parse_rule(RuleAction::Exclude, "**").unwrap();
        assert_eq!(r.pattern, "**");
        assert!(!r.basename_mode());
        assert!(!r.anchored);
    }

    #[test]
    fn parse_empty_errors() {
        assert!(parse_rule(RuleAction::Include, "").is_err());
        assert!(parse_rule(RuleAction::Include, "/").is_err());
        assert!(parse_rule(RuleAction::Include, "///").is_err());
    }

    #[test]
    fn filter_line_plus_minus() {
        let mut rs = RuleSet::new();
        rs.push_filter_line("+ *.c").unwrap();
        rs.push_filter_line("- *").unwrap();
        assert_eq!(rs.len(), 2);
        assert_eq!(rs.rules()[0].action, RuleAction::Include);
        assert_eq!(rs.rules()[1].action, RuleAction::Exclude);
    }

    #[test]
    fn filter_line_long_form() {
        let mut rs = RuleSet::new();
        rs.push_filter_line("include foo").unwrap();
        rs.push_filter_line("exclude bar").unwrap();
        assert_eq!(rs.rules()[0].action, RuleAction::Include);
        assert_eq!(rs.rules()[0].pattern, "foo");
        assert_eq!(rs.rules()[1].action, RuleAction::Exclude);
    }

    #[test]
    fn filter_line_comment_and_blank() {
        let mut rs = RuleSet::new();
        rs.push_filter_line("").unwrap();
        rs.push_filter_line("  # comment").unwrap();
        rs.push_filter_line("# full").unwrap();
        assert!(rs.is_empty());
    }

    #[test]
    fn filter_line_invalid() {
        let mut rs = RuleSet::new();
        assert!(rs.push_filter_line("nope").is_err());
        assert!(rs.push_filter_line("+").is_err());
        assert!(rs.push_filter_line("-   ").is_err());
    }

    #[test]
    fn from_file_line_default_and_prefix() {
        let mut rs = RuleSet::new();
        rs.push_from_file_line("*.tmp", RuleAction::Exclude)
            .unwrap();
        rs.push_from_file_line("+ keep.tmp", RuleAction::Exclude)
            .unwrap();
        assert_eq!(rs.rules()[0].action, RuleAction::Exclude);
        assert_eq!(rs.rules()[1].action, RuleAction::Include);
    }

    #[test]
    fn plus_without_space() {
        let mut rs = RuleSet::new();
        rs.push_filter_line("+foo").unwrap();
        assert_eq!(rs.rules()[0].pattern, "foo");
        assert_eq!(rs.rules()[0].action, RuleAction::Include);
    }
}
