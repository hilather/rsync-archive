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
    // -----------------------------------------------------------------
    // Dir-only INCLUDE: matches directory only (not descendants)
    // -----------------------------------------------------------------
    Case {
        name: "36 + /data/ - * → data (dir) INCLUDE",
        rules: &[Spec::In("/data/"), Spec::Ex("*")],
        path: "data",
        is_dir: true,
        expect: RuleAction::Include,
    },
    Case {
        name: "37 + /data/ - * → data/x file EXCLUDE",
        rules: &[Spec::In("/data/"), Spec::Ex("*")],
        path: "data/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "38 + /data/ - * → data/es/x EXCLUDE",
        rules: &[Spec::In("/data/"), Spec::Ex("*")],
        path: "data/es/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "39 + /data/ - * → data/es (dir) EXCLUDE",
        rules: &[Spec::In("/data/"), Spec::Ex("*")],
        path: "data/es",
        is_dir: true,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "40 + /data/ - * → other EXCLUDE",
        rules: &[Spec::In("/data/"), Spec::Ex("*")],
        path: "other",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // -----------------------------------------------------------------
    // + /data/ + /data/** - *: whole tree under data
    // -----------------------------------------------------------------
    Case {
        name: "41 + /data/ + /data/** - * → data (dir) INCLUDE",
        rules: &[Spec::In("/data/"), Spec::In("/data/**"), Spec::Ex("*")],
        path: "data",
        is_dir: true,
        expect: RuleAction::Include,
    },
    Case {
        name: "42 + /data/ + /data/** - * → data/x INCLUDE",
        rules: &[Spec::In("/data/"), Spec::In("/data/**"), Spec::Ex("*")],
        path: "data/x",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "43 + /data/ + /data/** - * → data/es/x INCLUDE",
        rules: &[Spec::In("/data/"), Spec::In("/data/**"), Spec::Ex("*")],
        path: "data/es/x",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "44 + /data/ + /data/** - * → data/es (dir) INCLUDE",
        rules: &[Spec::In("/data/"), Spec::In("/data/**"), Spec::Ex("*")],
        path: "data/es",
        is_dir: true,
        expect: RuleAction::Include,
    },
    Case {
        name: "45 + /data/ + /data/** - * → outside EXCLUDE",
        rules: &[Spec::In("/data/"), Spec::In("/data/**"), Spec::Ex("*")],
        path: "other/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "46 + /data/** alone - * → data itself via **",
        rules: &[Spec::In("/data/**"), Spec::Ex("*")],
        path: "data",
        is_dir: true,
        expect: RuleAction::Include,
    },
    // -----------------------------------------------------------------
    // - /data/elasticsearch/** + /data/** - * first-match order
    // -----------------------------------------------------------------
    Case {
        name: "47 exclude es/** first → data/es/x EXCLUDE",
        rules: &[
            Spec::Ex("/data/elasticsearch/**"),
            Spec::In("/data/**"),
            Spec::Ex("*"),
        ],
        path: "data/elasticsearch/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "48 exclude es/** first → data/other INCLUDE",
        rules: &[
            Spec::Ex("/data/elasticsearch/**"),
            Spec::In("/data/**"),
            Spec::Ex("*"),
        ],
        path: "data/other",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "49 exclude es/** first → data/elasticsearch (dir) EXCLUDE",
        rules: &[
            Spec::Ex("/data/elasticsearch/**"),
            Spec::In("/data/**"),
            Spec::Ex("*"),
        ],
        path: "data/elasticsearch",
        is_dir: true,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "50 exclude es/** first → outside EXCLUDE",
        rules: &[
            Spec::Ex("/data/elasticsearch/**"),
            Spec::In("/data/**"),
            Spec::Ex("*"),
        ],
        path: "var/log",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "51 include data/** first then exclude es/** → es still INCLUDE (first wins)",
        rules: &[
            Spec::In("/data/**"),
            Spec::Ex("/data/elasticsearch/**"),
            Spec::Ex("*"),
        ],
        path: "data/elasticsearch/x",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "52 filter-form - /data/es/** + /data/** - * → es EXCLUDE",
        rules: &[
            Spec::Filt("- /data/elasticsearch/**"),
            Spec::Filt("+ /data/**"),
            Spec::Filt("- *"),
        ],
        path: "data/elasticsearch/indices/0",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "53 filter-form - /data/es/** + /data/** - * → data/ok INCLUDE",
        rules: &[
            Spec::Filt("- /data/elasticsearch/**"),
            Spec::Filt("+ /data/**"),
            Spec::Filt("- *"),
        ],
        path: "data/ok",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // -----------------------------------------------------------------
    // basename *.tmp, path dir/*.tmp, **/*.o (extra breadth)
    // -----------------------------------------------------------------
    Case {
        name: "54 include *.tmp - * → dir/a.tmp INCLUDE (basename)",
        rules: &[Spec::In("*.tmp"), Spec::Ex("*")],
        path: "dir/a.tmp",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "55 exclude dir/*.tmp → dir/a.tmp EXCLUDE",
        rules: &[Spec::Ex("dir/*.tmp")],
        path: "dir/a.tmp",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "56 exclude dir/*.tmp → sub/dir/a.tmp EXCLUDE (end-anchor)",
        rules: &[Spec::Ex("dir/*.tmp")],
        path: "sub/dir/a.tmp",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "57 exclude dir/*.tmp → dir/sub/a.tmp no (* one segment)",
        rules: &[Spec::Ex("dir/*.tmp")],
        path: "dir/sub/a.tmp",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "58 exclude **/*.o → a/b/c.o",
        rules: &[Spec::Ex("**/*.o")],
        path: "a/b/c.o",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "59 exclude **/*.o → a/b/c.c no match",
        rules: &[Spec::Ex("**/*.o")],
        path: "a/b/c.c",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "60 include **/*.o - * → deep.o INCLUDE",
        rules: &[Spec::In("**/*.o"), Spec::Ex("*")],
        path: "x/y/z.o",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // -----------------------------------------------------------------
    // anchored /foo vs bar/foo
    // -----------------------------------------------------------------
    Case {
        name: "61 exclude /foo → foo EXCLUDE",
        rules: &[Spec::Ex("/foo")],
        path: "foo",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "62 exclude /foo → bar/foo INCLUDE (anchored)",
        rules: &[Spec::Ex("/foo")],
        path: "bar/foo",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "63 exclude /foo/bar → foo/bar EXCLUDE",
        rules: &[Spec::Ex("/foo/bar")],
        path: "foo/bar",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "64 exclude /foo/bar → x/foo/bar INCLUDE",
        rules: &[Spec::Ex("/foo/bar")],
        path: "x/foo/bar",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "65 include /foo - * → foo INCLUDE bar/foo EXCLUDE path",
        rules: &[Spec::In("/foo"), Spec::Ex("*")],
        path: "bar/foo",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "66 include /foo - * → foo INCLUDE",
        rules: &[Spec::In("/foo"), Spec::Ex("*")],
        path: "foo",
        is_dir: false,
        expect: RuleAction::Include,
    },
    // -----------------------------------------------------------------
    // Filter line forms: +pat, - pat, include, exclude
    // -----------------------------------------------------------------
    Case {
        name: "67 +pat no space then - * → a.c INCLUDE",
        rules: &[Spec::Filt("+*.c"), Spec::Filt("-*")],
        path: "a.c",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "68 +pat no space then - * → a.o EXCLUDE",
        rules: &[Spec::Filt("+*.c"), Spec::Filt("-*")],
        path: "a.o",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "69 - pat with space → a.tmp EXCLUDE",
        rules: &[Spec::Filt("- *.tmp")],
        path: "dir/a.tmp",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "70 include keyword then exclude * → x.c INCLUDE",
        rules: &[Spec::Filt("include *.c"), Spec::Filt("exclude *")],
        path: "x.c",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "71 INCLUDE keyword case-insensitive → x.c",
        rules: &[Spec::Filt("INCLUDE *.c"), Spec::Filt("EXCLUDE *")],
        path: "x.c",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "72 exclude keyword → x.o EXCLUDE",
        rules: &[Spec::Filt("include *.c"), Spec::Filt("exclude *")],
        path: "x.o",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // -----------------------------------------------------------------
    // Extra dir-only EXCLUDE / nested under data/
    // -----------------------------------------------------------------
    Case {
        name: "73 unanchored data/ exclude → nested/data/x EXCLUDE (basename dir)",
        rules: &[Spec::Ex("data/")],
        path: "nested/data/x",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "74 anchored /data/ exclude → nested/data/x no match",
        rules: &[Spec::Ex("/data/")],
        path: "nested/data/x",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "75 + data/ - * → data dir INCLUDE file EXCLUDE",
        rules: &[Spec::In("data/"), Spec::Ex("*")],
        path: "data/file",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    // -----------------------------------------------------------------
    // P1: unanchored multi-segment end-anchor; bare ** full-path
    // -----------------------------------------------------------------
    Case {
        name: "76 exclude foo/bar → a/foo/bar EXCLUDE (end-anchor)",
        rules: &[Spec::Ex("foo/bar")],
        path: "a/foo/bar",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "77 exclude foo/bar → foo/bar/x no match",
        rules: &[Spec::Ex("foo/bar")],
        path: "foo/bar/x",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "78 exclude /foo/bar → a/foo/bar INCLUDE (anchored)",
        rules: &[Spec::Ex("/foo/bar")],
        path: "a/foo/bar",
        is_dir: false,
        expect: RuleAction::Include,
    },
    Case {
        name: "79 exclude ** → any deep path EXCLUDE",
        rules: &[Spec::Ex("**")],
        path: "a/b/c",
        is_dir: false,
        expect: RuleAction::Exclude,
    },
    Case {
        name: "80 include sub/** - * → other/sub/a INCLUDE (end-anchor)",
        rules: &[Spec::In("sub/**"), Spec::Ex("*")],
        path: "other/sub/a",
        is_dir: false,
        expect: RuleAction::Include,
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

#[test]
fn prune_skipme_with_and_without_includes_under() {
    // skipme/ alone → prune
    let only_ex = build(&[Spec::Ex("skipme/")]);
    assert!(only_ex.should_prune_dir("skipme"));
    assert!(!only_ex.should_prune_dir("keep"));

    // include under skipme → do not prune
    let with_in = build(&[Spec::In("skipme/keep.txt"), Spec::Ex("skipme/")]);
    assert!(!with_in.should_prune_dir("skipme"));

    // Unanchored other/keep.txt can end-match skipme/other/keep.txt → no prune.
    let other_in = build(&[Spec::In("other/keep.txt"), Spec::Ex("skipme/")]);
    assert!(!other_in.should_prune_dir("skipme"));

    // Anchored /other/keep.txt cannot match under skipme → prune.
    let anchored_other = build(&[Spec::In("/other/keep.txt"), Spec::Ex("skipme/")]);
    assert!(anchored_other.should_prune_dir("skipme"));
}

#[test]
fn prune_data_dir_only_include_star_exclude() {
    // + /data/ - * : data itself is Include → no prune; nested dir is Exclude and
    // no include can match under it → prune nested.
    let rs = build(&[Spec::In("/data/"), Spec::Ex("*")]);
    assert!(!rs.should_prune_dir("data"));
    assert!(rs.should_prune_dir("data/elasticsearch"));
    assert!(rs.should_prune_dir("other"));
}

#[test]
fn prune_data_tree_include_no_prune_under_data() {
    let rs = build(&[Spec::In("/data/"), Spec::In("/data/**"), Spec::Ex("*")]);
    assert!(!rs.should_prune_dir("data"));
    assert!(!rs.should_prune_dir("data/elasticsearch"));
    // Include pattern contains `**` → conservative may_match_under is true for any
    // excluded dir (prefer over-walk). Do not assert prune on unrelated dirs.
    assert!(!rs.should_prune_dir("other"));
}

#[test]
fn prune_es_excluded_but_data_tree_included() {
    // - /data/elasticsearch/** + /data/** - *
    // elasticsearch is excluded as a path; include `/data/**` has ** → never prune
    // (conservative over-approx).
    let rs = build(&[
        Spec::Ex("/data/elasticsearch/**"),
        Spec::In("/data/**"),
        Spec::Ex("*"),
    ]);
    assert!(!rs.should_prune_dir("data"));
    assert!(!rs.should_prune_dir("data/elasticsearch"));
    assert!(!rs.should_prune_dir("var"));
}

#[test]
fn prune_without_double_star_can_prune_unrelated() {
    // Anchored full-path include without **: prove disjoint dirs are safe to prune.
    // (Unanchored keep/file.txt could end-match under any dir → never prune.)
    let rs = build(&[Spec::In("/keep/file.txt"), Spec::Ex("*")]);
    assert!(!rs.should_prune_dir("keep"));
    assert!(rs.should_prune_dir("other"));
    assert!(rs.should_prune_dir("skipme"));
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
fn filter_from_preserves_line_order() {
    // Classic exclude-first would drop *.c if order flipped; file order is authoritative.
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "- *").unwrap();
    writeln!(f, "+ *.c").unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_filter_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.rules()[0].action, RuleAction::Exclude);
    assert_eq!(rs.rules()[0].pattern, "*");
    assert_eq!(rs.rules()[1].action, RuleAction::Include);
    assert_eq!(rs.rules()[1].pattern, "*.c");
    assert_eq!(rs.action_for("a.c", false), RuleAction::Exclude);
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

#[test]
fn filter_line_forms_plus_no_space_and_long() {
    let mut rs = RuleSet::new();
    rs.push_filter_line("+*.tmp").unwrap();
    rs.push_filter_line("- *.log").unwrap();
    rs.push_filter_line("include keep.dat").unwrap();
    rs.push_filter_line("exclude *").unwrap();
    assert_eq!(rs.action_for("a.tmp", false), RuleAction::Include);
    assert_eq!(rs.action_for("a.log", false), RuleAction::Exclude);
    assert_eq!(rs.action_for("keep.dat", false), RuleAction::Include);
    assert_eq!(rs.action_for("other", false), RuleAction::Exclude);
    // comments / blank are no-ops
    rs.push_filter_line("# comment").unwrap();
    rs.push_filter_line("").unwrap();
    assert_eq!(rs.len(), 4);
}

#[test]
fn from_file_mixed_plus_minus_and_comments() {
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "# header comment").unwrap();
    writeln!(f).unwrap();
    writeln!(f, "+ /data/").unwrap();
    writeln!(f, "+ /data/**").unwrap();
    writeln!(f, "- /data/elasticsearch/**").unwrap();
    writeln!(f, "# trailing").unwrap();
    writeln!(f, "- *").unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_filter_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.len(), 4);
    assert_eq!(rs.action_for("data", true), RuleAction::Include);
    assert_eq!(rs.action_for("data/ok", false), RuleAction::Include);
    // - /data/elasticsearch/** comes after + /data/** → first match is include
    assert_eq!(
        rs.action_for("data/elasticsearch/x", false),
        RuleAction::Include
    );
    assert_eq!(rs.action_for("outside", false), RuleAction::Exclude);
}

#[test]
fn from_file_exclude_from_mixed_with_include_override() {
    // First-match: exception includes must appear *before* broader excludes.
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "# mixed +/- and long form; bare → exclude default").unwrap();
    writeln!(f, "+ important.tmp").unwrap();
    writeln!(f, "include keep.me").unwrap();
    writeln!(f, "*.tmp").unwrap();
    writeln!(f, "- secret.log").unwrap();
    writeln!(f).unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_exclude_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.len(), 4);
    assert_eq!(rs.rules()[0].action, RuleAction::Include);
    assert_eq!(rs.rules()[0].pattern, "important.tmp");
    assert_eq!(rs.rules()[1].action, RuleAction::Include);
    assert_eq!(rs.rules()[1].pattern, "keep.me");
    assert_eq!(rs.rules()[2].action, RuleAction::Exclude);
    assert_eq!(rs.rules()[2].pattern, "*.tmp");
    assert_eq!(rs.rules()[3].action, RuleAction::Exclude);
    assert_eq!(rs.rules()[3].pattern, "secret.log");

    assert_eq!(rs.action_for("important.tmp", false), RuleAction::Include);
    assert_eq!(rs.action_for("keep.me", false), RuleAction::Include);
    assert_eq!(rs.action_for("a.tmp", false), RuleAction::Exclude);
    assert_eq!(rs.action_for("secret.log", false), RuleAction::Exclude);
    // bare *.tmp after +important still excludes other tmp via first-match
    assert_eq!(rs.action_for("other.txt", false), RuleAction::Include);
}

#[test]
fn from_file_include_from_with_exclude_prefix() {
    // Exception exclude before broader include so first-match can hit it.
    let mut f = NamedTempFile::new().unwrap();
    writeln!(f, "- bad.c").unwrap();
    writeln!(f, "*.c").unwrap();
    writeln!(f, "# ignore").unwrap();
    f.flush().unwrap();

    let mut rs = RuleSet::new();
    load_include_from(&mut rs, f.path()).unwrap();
    assert_eq!(rs.len(), 2);
    assert_eq!(rs.rules()[0].action, RuleAction::Exclude);
    assert_eq!(rs.rules()[1].action, RuleAction::Include);
    assert_eq!(rs.action_for("bad.c", false), RuleAction::Exclude);
    assert_eq!(rs.action_for("good.c", false), RuleAction::Include);
    // default for unmatched is still Include (no trailing - *)
    assert_eq!(rs.action_for("other.o", false), RuleAction::Include);
}
