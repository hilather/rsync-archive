//! Stage 4 rsync filter parity table and parse/size-cap tests.
//!
//! Semantics: docs/SELECTION.md + docs/DESIGN.md (K27, prune algorithm).

use rsync_archive::select::from_file::{
    load_exclude_from, load_filter_from, load_include_from, read_capped_lines_from_reader,
    MAX_FILTER_FILE_BYTES, MAX_FILTER_FILE_LINES,
};
use rsync_archive::{Error, RuleAction, RuleSet};
use std::fs::File;
use std::io::Write;
use tempfile::NamedTempFile;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Spec {
    Ex(&'static str),
    In(&'static str),
    Filt(&'static str),
}

fn build(specs: &[Spec]) -> RuleSet {
    let mut rs = RuleSet::new();
    for s in specs {
        match *s {
            Spec::Ex(p) => rs.push_exclude(p).unwrap(),
            Spec::In(p) => rs.push_include(p).unwrap(),
            Spec::Filt(l) => rs.push_filter_line(l).unwrap(),
        }
    }
    rs
}

fn act(rs: &RuleSet, path: &str, is_dir: bool) -> RuleAction {
    rs.action_for(path, is_dir)
}

// ---------------------------------------------------------------------------
// Table-driven parity (≥25 cases from DESIGN.md + extras)
// ---------------------------------------------------------------------------

struct Case {
    name: &'static str,
    rules: &'static [Spec],
    path: &'static str,
    is_dir: bool,
    expect: RuleAction,
}

const PARITY: &[Case] = &[
    // 1–3 basename *.tmp
    Case {
        name: "1 exclude *.tmp → a.tmp",
        rules: &[Spec::Ex("*.tmp")],
        path: "a.tmp",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "2 exclude *.tmp → a.txt",
        rules: &[Spec::Ex("*.tmp")],
        path: "a.txt",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "3 exclude *.tmp → dir/a.tmp (basename)",
        rules: &[Spec::Ex("*.tmp")],
        path: "dir/a.tmp",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // 4–5 dir-only
    Case {
        name: "4 exclude dir/ → dir (dir)",
        rules: &[Spec::Ex("dir/")],
        path: "dir",
        is_dir: true,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "5 exclude dir/ → dir/x (file under)",
        rules: &[Spec::Ex("dir/")],
        path: "dir/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // 6–7 include-only idiom / order
    Case {
        name: "6 include *.c then exclude * → a.c",
        rules: &[Spec::In("*.c"), Spec::Ex("*")],
        path: "a.c",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "7 exclude * then include *.c → a.c (first wins)",
        rules: &[Spec::Ex("*"), Spec::In("*.c")],
        path: "a.c",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // 8–10 anchored
    Case {
        name: "8 exclude /foo → foo",
        rules: &[Spec::Ex("/foo")],
        path: "foo",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "9 include /foo → foo",
        rules: &[Spec::In("/foo")],
        path: "foo",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "10 exclude /foo → bar/foo (no match)",
        rules: &[Spec::Ex("/foo")],
        path: "bar/foo",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // 11 **
    Case {
        name: "11 exclude **/*.o → x/y.o",
        rules: &[Spec::Ex("**/*.o")],
        path: "x/y.o",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // 12–13 ?
    Case {
        name: "12 exclude ?.txt → a.txt",
        rules: &[Spec::Ex("?.txt")],
        path: "a.txt",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "13 exclude ?.txt → ab.txt",
        rules: &[Spec::Ex("?.txt")],
        path: "ab.txt",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // 14 sub/**
    Case {
        name: "14 include sub/** then exclude * → sub/a",
        rules: &[Spec::In("sub/**"), Spec::Ex("*")],
        path: "sub/a",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // 15–16 star / default
    Case {
        name: "15 exclude * only → any",
        rules: &[Spec::Ex("*")],
        path: "any",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "16 no rules → any",
        rules: &[],
        path: "any",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // 17–18 filter order
    Case {
        name: "17 filter - *.log then + keep.log → keep.log",
        rules: &[Spec::Filt("- *.log"), Spec::Filt("+ keep.log")],
        path: "keep.log",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "18 filter + keep.log then - *.log → keep.log",
        rules: &[Spec::Filt("+ keep.log"), Spec::Filt("- *.log")],
        path: "keep.log",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // 19–20 basename foo
    Case {
        name: "19 exclude foo → foo",
        rules: &[Spec::Ex("foo")],
        path: "foo",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "20 exclude foo → bar/foo (basename)",
        rules: &[Spec::Ex("foo")],
        path: "bar/foo",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // 21–22 full path bar/foo
    Case {
        name: "21 exclude bar/foo → bar/foo",
        rules: &[Spec::Ex("bar/foo")],
        path: "bar/foo",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "22 exclude bar/foo → foo",
        rules: &[Spec::Ex("bar/foo")],
        path: "foo",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // 23–24 dir/*
    Case {
        name: "23 exclude dir/* → dir/x",
        rules: &[Spec::Ex("dir/*")],
        path: "dir/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "24 exclude dir/* → dir/sub/x (* one segment)",
        rules: &[Spec::Ex("dir/*")],
        path: "dir/sub/x",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // Extra cases for breadth
    Case {
        name: "25 dir-only does not exclude file named dir",
        rules: &[Spec::Ex("dir/")],
        path: "dir",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "26 exclude **/*.o → y.o at root",
        rules: &[Spec::Ex("**/*.o")],
        path: "y.o",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "27 include sub/** matches sub itself as path",
        rules: &[Spec::In("sub/**"), Spec::Ex("*")],
        path: "sub",
        is_dir: true,
        expect: RuleAction::Include,
    },
    Case {
        name: "28 nested dir-only cache/ → cache/a/b",
        rules: &[Spec::Ex("cache/")],
        path: "cache/a/b",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "29 exclude *.tmp does not hit a.tmp.bak",
        rules: &[Spec::Ex("*.tmp")],
        path: "dir/a.tmp.bak",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "30 full path a/b/* mid",
        rules: &[Spec::Ex("a/b/*")],
        path: "a/b/c",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "31 full path a/b/* not deeper",
        rules: &[Spec::Ex("a/b/*")],
        path: "a/b/c/d",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "32 anchored /dir/x → dir/x",
        rules: &[Spec::Ex("/dir/x")],
        path: "dir/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "33 multi exclude first match",
        rules: &[Spec::Ex("*.o"), Spec::Ex("*.a")],
        path: "lib.a",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "34 include deep then star",
        rules: &[Spec::In("src/**/*.rs"), Spec::Ex("*")],
        path: "src/main.rs",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "35 include deep not match other",
        rules: &[Spec::In("src/**/*.rs"), Spec::Ex("*")],
        path: "lib/main.rs",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
];

#[test]
fn parity_table() {
    let mut failed = Vec::new();
    for c in PARITY {
        let rs = build(c.rules);
        let got = act(&rs, c.path, c.is_dir);
        if got != c.expect {
            failed.push(format!(
                "{}: path={} is_dir={} got={:?} expect={:?}",
                c.name, c.path, c.is_dir, got, c.expect
            ));
        }
    }
    assert!(
        failed.is_empty(),
        "{} parity failures:\n{}",
        failed.len(),
        failed.join("\n")
    );
    assert!(
        PARITY.len() >= 25,
        "need ≥25 parity cases, have {}",
        PARITY.len()
    );
}

// ---------------------------------------------------------------------------
// Prune
// ---------------------------------------------------------------------------

#[test]
fn prune_excluded_dir_with_no_include() {
    let rs = build(&[Spec::Ex("skipme/")]);
    assert!(rs.should_prune_dir("skipme"));
    // unrelated dirs: action is Include (default) → no prune
    assert!(!rs.should_prune_dir("other"));
}

#[test]
fn prune_not_when_basename_include_may_match() {
    // include-only idiom: dirs match `*` → Exclude, but `*.c` may match under
    let rs = build(&[Spec::In("*.c"), Spec::Ex("*")]);
    assert_eq!(rs.action_for("src", true), RuleAction::Exclude);
    assert!(!rs.should_prune_dir("src"));
}

#[test]
fn prune_not_when_explicit_include_under() {
    let rs = build(&[Spec::In("skipme/keep.txt"), Spec::Ex("skipme/")]);
    // skipme is excluded as dir by second rule? First match for skipme as dir:
    // include `skipme/keep.txt` does not match dir `skipme`; exclude `skipme/` does.
    assert_eq!(rs.action_for("skipme", true), RuleAction::Exclude);
    // But include may match under → do not prune
    assert!(!rs.should_prune_dir("skipme"));
    // keep is included
    assert_eq!(
        rs.action_for("skipme/keep.txt", false),
        RuleAction::Include
    );
}

#[test]
fn prune_when_include_cannot_match_under() {
    // Anchored include of something else cannot match under `skipme`
    let rs = build(&[Spec::In("/other"), Spec::Ex("skipme/")]);
    assert!(rs.should_prune_dir("skipme"));
}

#[test]
fn prune_exclude_first_then_include_under_same_dir() {
    // exclude first: keep not selected; may prune if no later include can help
    // wait: include is after exclude but may_match looks at ANY include rule
    let rs = build(&[Spec::Ex("skipme/"), Spec::In("skipme/keep.txt")]);
    assert_eq!(
        rs.action_for("skipme/keep.txt", false),
        RuleAction::Exclude
    ); // first match is dir-only exclude on ancestors
    // include still may match under → conservative no prune
    assert!(!rs.should_prune_dir("skipme"));
}

// ---------------------------------------------------------------------------
// Parse / from-file / size caps
// ---------------------------------------------------------------------------

#[test]
fn include_from_comments_case_25() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "# this is a comment").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "*.c").unwrap();
    writeln!(f, "# another").unwrap();
    writeln!(f, "*.h").unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_include_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.len(), 2);
    assert_eq!(rs.rules()[0].pattern, "*.c");
    assert_eq!(rs.rules()[1].pattern, "*.h");
    assert!(rs.rules().iter().all(|r| r.action == RuleAction::Include));
}

#[test]
fn exclude_from_bare_defaults_to_exclude() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "*.tmp").unwrap();
    writeln!(f, "+ important.tmp").unwrap();
    writeln!(f, "- extra.log").unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_exclude_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.len(), 3);
    assert_eq!(rs.rules()[0].action, RuleAction::Exclude);
    assert_eq!(rs.rules()[1].action, RuleAction::Include);
    assert_eq!(rs.rules()[2].action, RuleAction::Exclude);
}

#[test]
fn filter_from_file() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "+ *.c").unwrap();
    writeln!(f, "- *").unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_filter_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.action_for("a.c", false), RuleAction::Include);
    assert_eq!(rs.action_for("a.o", false), RuleAction::Exclude);
}

#[test]
fn size_cap_bytes() {
    let mut f = NamedTempFile::new().unwrap();
    write!(f, "hello world").unwrap();
    f.flush().unwrap();
    let file = File::open(f.path()).unwrap();
    let err = read_capped_lines_from_reader(f.path(), file, 4, MAX_FILTER_FILE_LINES).unwrap_err();
    match err {
        Error::FilterFileTooLarge { .. } => {}
        other => panic!("expected FilterFileTooLarge, got {other:?}"),
    }
}

#[test]
fn size_cap_lines() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "a").unwrap();
    writeln!(f, "b").unwrap();
    writeln!(f, "c").unwrap();
    f.flush().unwrap();
    let file = File::open(f.path()).unwrap();
    let err = read_capped_lines_from_reader(f.path(), file, MAX_FILTER_FILE_BYTES, 2).unwrap_err();
    match err {
        Error::FilterFileTooLarge { .. } => {}
        other => panic!("expected FilterFileTooLarge, got {other:?}"),
    }
}

#[test]
fn empty_pattern_rejected() {
    let mut rs = RuleSet::new();
    assert!(rs.push_exclude("").is_err());
    assert!(rs.push_exclude("/").is_err());
    assert!(rs.push_filter_line("+").is_err());
}

#[test]
fn filter_long_form() {
    let mut rs = RuleSet::new();
    rs.push_filter_line("include *.c").unwrap();
    rs.push_filter_line("exclude *").unwrap();
    assert_eq!(rs.action_for("x.c", false), RuleAction::Include);
    assert_eq!(rs.action_for("x.o", false), RuleAction::Exclude);
}
