//! FLUX-003 — the shared `~/.flux/<flattened-cwd>/` read model and the `/flux/status` panel,
//! pinned against the upstream Python they port.
//!
//! `crates/cyrup-flux/src/state.rs` is a function-for-function port of
//! `code_puppy_core_plugins/flux_bootstrap/bundled/scripts/flux_status.py` @v0.0.40 (`flatten_cwd`
//! `:84-86`, `derive_base` `:89-93`, `parse_frontmatter` `:96-112`, `collect_todos` `:129-137`,
//! `collect_done` `:140-155`, `format_timestamp` `:158-163`, `collect_reviews` `:166-178`), and
//! `render_status.rs` of its `render()` (`:181-270`) plus `main()`'s section validation and
//! empty-state line (`:289-341`). The panel is the cross-harness contract `lib.rs` names as the
//! crate's purpose: one project's task tree must read the same in both harnesses, so every
//! expectation here was produced by RUNNING that Python script (or importing it) — not by reading
//! the Rust. Every table value in this file is the upstream's output.
//!
//! How the goldens were made (re-run this if the fixture spec below or the upstream tag changes):
//! extract the script with `git -C tmp/code_puppy_core_plugins show
//! v0.0.40:code_puppy_core_plugins/flux_bootstrap/bundled/scripts/flux_status.py > flux_status.py`,
//! write the `SMALL` / `WIDE` trees below to disk byte-for-byte (plus the empty `review/high/`
//! directory `build` creates), then `python3 flux_status.py --no-color --base <tree> [sections…]`
//! and strip `print`'s single trailing newline. The pure-function tables were produced by importing
//! the module and calling `flatten_cwd` / `format_timestamp` / `parse_frontmatter` directly.
//!
//! Red before / green after: the pins of behaviour that was already faithful are green on both
//! sides (they are the row's missing evidence, not a regression). Two tests are RED against the
//! pre-change `state.rs` and name the defects they expose:
//! `parse_frontmatter_decodes_invalid_utf8_lossily_like_errors_replace` (FLUX-006 —
//! `read_to_string` emptied the whole map on one bad byte where the Python's `errors="replace"`
//! keeps parsing) and `parse_frontmatter_splits_lines_where_python_splitlines_does` (`str::lines`
//! breaks only on `\n`, `str.splitlines()` also on `\r`, `\x0b`, `\x0c`, `\x1c`-`\x1e`, U+0085,
//! U+2028 and U+2029 — a lone-`\r` file parsed to an empty map here and to a full one upstream).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use cyrup_flux::render_status::{parse_sections, render};
use cyrup_flux::state::{
    SEVERITIES, collect_done, collect_reviews, collect_todos, derive_base_from, flatten_cwd,
    format_timestamp, parse_frontmatter,
};

// ---------------------------------------------------------------------------------------------
// Fixture trees — `(relative path, content)`, written byte-for-byte. The Python run that produced
// the goldens wrote exactly these files (its generator held the same list).
// ---------------------------------------------------------------------------------------------

/// Every collector edge in one tree: an unknown status text, a frontmatter-less todo, a non-`.md`
/// sibling, three `done/` runs (one with a non-timestamp name, one with no `.md` rows, one row
/// without `status`), a stray file directly under `done/`, an unknown severity directory and a
/// non-`.md` file inside a severity directory.
const SMALL: &[(&str, &str)] = &[
    (
        "todo/01-alpha.md",
        "---\nstage: exec\nstatus: in-progress\n---\n# alpha\n",
    ),
    (
        "todo/02-bravo.md",
        "---\nstage: qa\nstatus: needs-rework\n---\n",
    ),
    ("todo/03-charlie.md", "---\nstage: aug\n---\n"),
    (
        "todo/04-delta.md",
        "---\nstage: exec\nstatus: blocked\n---\n",
    ),
    ("todo/05-echo.md", "no frontmatter here\n"),
    (
        "todo/06-foxtrot.md",
        "---\nstage: done\nstatus: done\n---\n",
    ),
    (
        "todo/notes.txt",
        "---\nstage: exec\nstatus: in-progress\n---\n",
    ),
    (
        "done/2026-04-29-16-57/z-last.md",
        "---\nstage: commit\nstatus: completed\n---\n",
    ),
    (
        "done/2026-04-29-16-57/a-first.md",
        "---\nstage: tests\n---\n",
    ),
    (
        "done/2026-05-01-09-00/only.md",
        "---\nstage: exec\nstatus: done\n---\n",
    ),
    ("done/misc-run/x.md", "---\nstage: exec\n---\n"),
    ("done/empty-run/.keep", ""),
    ("done/stray.md", "---\nstage: exec\nstatus: done\n---\n"),
    ("review/critical/c1.md", "# c1\n"),
    ("review/medium/m2.md", "# m2\n"),
    ("review/medium/m1.md", "# m1\n"),
    ("review/low/l1.md", "# l1\n"),
    ("review/bogus/b1.md", "# b1\n"),
    ("review/medium/readme.txt", ""),
];

/// A 57-character task name — past the `min(name_w, 50)` cap (`flux_status.py:191`) — plus a
/// `status:` key with an EMPTY value in a done row (present-but-blank is not the same as absent).
const LONG_NAME: &str = "a-very-long-task-name-that-exceeds-the-fifty-char-cap-xyz";

fn wide_spec() -> Vec<(String, String)> {
    vec![
        (
            format!("todo/{LONG_NAME}.md"),
            "---\nstage: exec\nstatus: in-progress\n---\n".into(),
        ),
        (
            "todo/short.md".into(),
            "---\nstage: qa\nstatus: done\n---\n".into(),
        ),
        (
            "done/2026-06-01-12-30/blank-status.md".into(),
            "---\nstage: exec\nstatus:\n---\n".into(),
        ),
        ("review/high/h1.md".into(), String::new()),
    ]
}

fn write_tree(root: &Path, spec: &[(String, String)]) {
    for (rel, content) in spec {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content.as_bytes()).unwrap();
    }
}

fn small_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let spec: Vec<(String, String)> = SMALL
        .iter()
        .map(|(r, c)| ((*r).to_string(), (*c).to_string()))
        .collect();
    write_tree(dir.path(), &spec);
    // A severity directory that exists but holds nothing — a column with no rows.
    fs::create_dir_all(dir.path().join("review").join("high")).unwrap();
    dir
}

fn wide_tree() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    write_tree(dir.path(), &wide_spec());
    dir
}

fn assert_panel(actual: &str, expected: &str, what: &str) {
    if actual != expected {
        let a: Vec<&str> = actual.lines().collect();
        let e: Vec<&str> = expected.lines().collect();
        for (i, (l, r)) in a.iter().zip(e.iter()).enumerate() {
            assert_eq!(l, r, "{what}: first divergence at line {i}");
        }
        panic!(
            "{what}: rendered {} lines, the Python golden has {}",
            a.len(),
            e.len()
        );
    }
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect()
}

/// One `parse_frontmatter` case: `(name, file bytes, expected key/value pairs)`.
type FrontmatterCase = (
    &'static str,
    &'static [u8],
    &'static [(&'static str, &'static str)],
);

fn frontmatter_of(dir: &Path, name: &str, bytes: &[u8]) -> BTreeMap<String, String> {
    let path = dir.join(format!("{name}.md"));
    fs::write(&path, bytes).unwrap();
    parse_frontmatter(&path)
}

// ---------------------------------------------------------------------------------------------
// state.rs — pure functions
// ---------------------------------------------------------------------------------------------

/// `re.sub(r"[^a-zA-Z0-9]+", "-", cwd)` (`flux_status.py:84-86`): a leading run, a trailing run
/// and a run of mixed separators each collapse to ONE hyphen; non-ASCII letters are separators
/// too (the class is ASCII-only); case is kept. This decides the DIRECTORY every task file lands
/// in, so an off-by-one here would split a project's state into two trees.
#[test]
fn flatten_cwd_matches_the_python_regex() {
    let table = [
        ("/home/user/cyrup", "-home-user-cyrup"),
        ("/Users/dm/My Project (v2)/", "-Users-dm-My-Project-v2-"),
        ("", ""),
        ("///", "-"),
        ("abc", "abc"),
        ("a__b--c", "a-b-c"),
        ("-a-", "-a-"),
        ("/tmp/x.y/z_w", "-tmp-x-y-z-w"),
        ("übung/ß/naïve", "-bung-na-ve"),
        ("C:\\Users\\dm\\proj", "C-Users-dm-proj"),
        ("a/b/c/", "a-b-c-"),
        ("trailing-", "trailing-"),
        (".hidden", "-hidden"),
    ];
    for (input, want) in table {
        assert_eq!(flatten_cwd(input), want, "flatten_cwd({input:?})");
    }
}

/// `flux_status.py:158-163`: exactly five `-`-separated parts become `YYYY-MM-DD HH:MM`; four,
/// six, a trailing separator (six parts, last empty) or five empties all pass through verbatim.
#[test]
fn format_timestamp_rewrites_five_part_names_and_passes_everything_else_through() {
    let table = [
        ("2026-04-29-16-57", "2026-04-29 16:57"),
        ("2026-04-29", "2026-04-29"),
        ("misc-run", "misc-run"),
        ("", ""),
        ("a-b-c-d-e-f", "a-b-c-d-e-f"),
        ("-----", "-----"),
        ("2026-04-29-16-57-", "2026-04-29-16-57-"),
    ];
    for (input, want) in table {
        assert_eq!(format_timestamp(input), want, "format_timestamp({input:?})");
    }
}

/// `flux_status.py:96-112` on well-formed UTF-8: a missing file, a file without the opening
/// `---`, a leading space before it and a BOM before it are all the empty map; an unterminated
/// block parses to the end; `partition(":")` splits on the FIRST colon (so `a: b: c` keeps its
/// inner colons and `:leading` yields the empty key); keys and values are `strip()`ped; a
/// terminator is `strip() == "---"` (NBSP-padded included); the last duplicate key wins; CRLF
/// is fine; a file that is only `---` is empty.
#[test]
fn parse_frontmatter_tolerates_missing_no_frontmatter_unterminated_and_colons() {
    let dir = tempfile::tempdir().unwrap();
    assert!(parse_frontmatter(&dir.path().join("nope.md")).is_empty());
    let table: &[FrontmatterCase] = &[
        ("no_frontmatter", b"stage: exec\nstatus: done\n", &[]),
        (
            "unterminated",
            b"---\nstage: exec\nstatus: in-progress\n# body\nno colon line\n",
            &[("stage", "exec"), ("status", "in-progress")],
        ),
        (
            "colons",
            b"---\ntitle: a: b: c\nurl: https://example.com/x\n  padded :  v  \nnocolon\n:leading\nempty:\n---\nafter: no\n",
            &[
                ("", "leading"),
                ("empty", ""),
                ("padded", "v"),
                ("title", "a: b: c"),
                ("url", "https://example.com/x"),
            ],
        ),
        (
            "crlf",
            b"---\r\nstage: exec\r\nstatus: done\r\n---\r\n",
            &[("stage", "exec"), ("status", "done")],
        ),
        ("leading_space", b" ---\nstage: exec\n---\n", &[]),
        ("dup_keys", b"---\nstage: a\nstage: b\n---\n", &[("stage", "b")]),
        ("only_open", b"---", &[]),
        ("empty", b"", &[]),
        ("dashes_then_text", b"----\nstage: exec\n---\n", &[("stage", "exec")]),
        (
            "terminator_with_spaces",
            b"---\nstage: exec\n  ---  \nstatus: done\n---\n",
            &[("stage", "exec")],
        ),
        (
            "nbsp_terminator",
            "---\nstage: exec\n\u{a0}---\u{a0}\nstatus: done\n---\n".as_bytes(),
            &[("stage", "exec")],
        ),
        ("tab_only_line", b"---\n\t\nstage: exec\n---\n", &[("stage", "exec")]),
        ("colon_in_key_ws", b"---\n stage : exec \n---\n", &[("stage", "exec")]),
        ("bom", "\u{feff}---\nstage: exec\n---\n".as_bytes(), &[]),
    ];
    for (name, bytes, want) in table {
        assert_eq!(
            frontmatter_of(dir.path(), name, bytes),
            map(want),
            "parse_frontmatter case {name}"
        );
    }
}

/// FLUX-006. `flux_status.py:100` reads with `errors="replace"`: an undecodable byte becomes
/// U+FFFD and parsing CONTINUES — inside the block (`caf\xe9` -> `caf\u{fffd}`), after it
/// (`\xff\xfe` in the body must not touch the two keys above it), and a truncated multi-byte
/// sequence is one U+FFFD. The pre-change `read_to_string` turned every one of these into an
/// empty map, which `render_status` shows as a blank STAGE and `(unknown)`.
#[test]
fn parse_frontmatter_decodes_invalid_utf8_lossily_like_errors_replace() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        frontmatter_of(
            dir.path(),
            "non_utf8",
            b"---\nstage: exec\nstatus: in-progress\nnote: caf\xe9\n---\n"
        ),
        map(&[
            ("note", "caf\u{fffd}"),
            ("stage", "exec"),
            ("status", "in-progress")
        ])
    );
    assert_eq!(
        frontmatter_of(
            dir.path(),
            "bad_byte_after_block",
            b"---\nstage: exec\nstatus: in-progress\n---\n\xff\xfe body\n"
        ),
        map(&[("stage", "exec"), ("status", "in-progress")])
    );
    assert_eq!(
        frontmatter_of(
            dir.path(),
            "truncated_multibyte",
            b"---\nstage: ex\xe2\x82\nstatus: done\n---\n"
        ),
        map(&[("stage", "ex\u{fffd}"), ("status", "done")])
    );
}

/// `flux_status.py:105` is `text.splitlines()`, whose boundaries are `\n`, `\r`, `\r\n`, `\x0b`,
/// `\x0c`, `\x1c`, `\x1d`, `\x1e`, U+0085, U+2028 and U+2029 (Python `str.splitlines` docs).
/// `str::lines` breaks on `\n` only, so a lone-`\r` file (a classic-Mac write, or a `\r`
/// terminator after a value) parsed differently in the two harnesses before this pin.
#[test]
fn parse_frontmatter_splits_lines_where_python_splitlines_does() {
    let dir = tempfile::tempdir().unwrap();
    let both = &[("stage", "exec"), ("status", "done")];
    let table: &[FrontmatterCase] = &[
        (
            "lone_cr",
            b"---\rstage: exec\rstatus: done\r---\rbody: no\r",
            both,
        ),
        (
            "crlf_mixed",
            b"---\r\nstage: exec\nstatus: done\r\n---\n",
            both,
        ),
        (
            "form_feed",
            b"---\nstage: exec\x0cstatus: done\n---\n",
            both,
        ),
        (
            "vertical_tab",
            b"---\nstage: exec\x0bstatus: done\n---\n",
            both,
        ),
        (
            "fs_gs_rs",
            b"---\nstage: exec\x1cstatus: done\x1dnote: x\x1eextra: y\n---\n",
            &[
                ("extra", "y"),
                ("note", "x"),
                ("stage", "exec"),
                ("status", "done"),
            ],
        ),
        (
            "nel",
            "---\nstage: exec\u{85}status: done\n---\n".as_bytes(),
            both,
        ),
        (
            "ls_ps",
            "---\nstage: exec\u{2028}status: done\u{2029}note: x\n---\n".as_bytes(),
            &[("note", "x"), ("stage", "exec"), ("status", "done")],
        ),
        (
            "cr_terminator",
            b"---\nstage: exec\r---\nstatus: done\n---\n",
            &[("stage", "exec")],
        ),
    ];
    for (name, bytes, want) in table {
        assert_eq!(
            frontmatter_of(dir.path(), name, bytes),
            map(want),
            "parse_frontmatter splitlines case {name}"
        );
    }
}

/// `derive_base` decomposed over its inputs: a non-empty `FLUX_ROOT` wins as-is; a set-but-empty
/// one is unset; otherwise `$HOME/.flux`; with neither, the relative `.flux` (cyrup's arm — the
/// Python has only `Path.home()`, `flux_status.py:92-93`). The leaf is always `flatten_cwd` of the
/// working directory's display form, never a raw path segment.
#[test]
fn derive_base_from_applies_flux_root_then_home_then_relative() {
    let cwd = Path::new("/home/user/My Project (v2)");
    let with = |root: Option<&str>, home: Option<&str>| {
        let root = root.map(OsString::from);
        let home = home.map(OsString::from);
        derive_base_from(
            &|key| match key {
                "FLUX_ROOT" => root.clone(),
                "HOME" => home.clone(),
                _ => None,
            },
            cwd,
        )
    };
    assert_eq!(
        with(Some("/srv/flux"), Some("/home/user")),
        PathBuf::from("/srv/flux/-home-user-My-Project-v2-")
    );
    assert_eq!(
        with(Some(""), Some("/home/user")),
        PathBuf::from("/home/user/.flux/-home-user-My-Project-v2-")
    );
    assert_eq!(
        with(None, Some("/home/user")),
        PathBuf::from("/home/user/.flux/-home-user-My-Project-v2-")
    );
    assert_eq!(
        with(None, None),
        PathBuf::from(".flux/-home-user-My-Project-v2-")
    );
}

// ---------------------------------------------------------------------------------------------
// state.rs — collectors over the fixture tree
// ---------------------------------------------------------------------------------------------

/// `collect_todos` (`flux_status.py:129-137`): `sorted(todo.glob("*.md"))`, so filename order,
/// `.md` only; `stage`/`status` default to `""` (`fm.get(..., "")`), including for a file with no
/// frontmatter at all.
#[test]
fn collect_todos_sorts_by_filename_ignores_non_md_and_defaults_to_empty() {
    let dir = small_tree();
    let got = collect_todos(dir.path());
    let want: Vec<(String, String, String)> = [
        ("01-alpha", "exec", "in-progress"),
        ("02-bravo", "qa", "needs-rework"),
        ("03-charlie", "aug", ""),
        ("04-delta", "exec", "blocked"),
        ("05-echo", "", ""),
        ("06-foxtrot", "done", "done"),
    ]
    .iter()
    .map(|(a, b, c)| ((*a).to_string(), (*b).to_string(), (*c).to_string()))
    .collect();
    assert_eq!(got, want);
    assert!(collect_todos(&dir.path().join("absent")).is_empty());
}

/// `collect_done` (`flux_status.py:140-155`): `sorted(done.iterdir(), reverse=True)` — so
/// `misc-run` sorts BEFORE the two timestamps (`m` > `2`), and the newer run before the older;
/// a plain file under `done/` is skipped; a run directory with no `.md` rows is omitted; rows are
/// filename-sorted within a run; `status` defaults to `"completed"` (`fm.get("status",
/// "completed")`), NOT `""` — the line that makes a code-puppy-written done file render.
#[test]
fn collect_done_reverse_sorts_runs_defaults_status_to_completed_and_omits_empty_runs() {
    let dir = small_tree();
    let got = collect_done(dir.path());
    let row = |a: &str, b: &str, c: &str| (a.to_string(), b.to_string(), c.to_string());
    assert_eq!(
        got,
        vec![
            ("misc-run".to_string(), vec![row("x", "exec", "completed")]),
            (
                "2026-05-01 09:00".to_string(),
                vec![row("only", "exec", "done")]
            ),
            (
                "2026-04-29 16:57".to_string(),
                vec![
                    row("a-first", "tests", "completed"),
                    row("z-last", "commit", "completed")
                ]
            ),
        ]
    );
    // Present-but-blank `status:` is NOT absent: the default does not apply (`dict.get`).
    let wide = wide_tree();
    assert_eq!(
        collect_done(wide.path()),
        vec![(
            "2026-06-01 12:30".to_string(),
            vec![row("blank-status", "exec", "")]
        )]
    );
}

/// `collect_reviews` (`flux_status.py:166-178`): the FIXED order critical -> high -> medium ->
/// low regardless of directory listing order, filename-sorted within a severity, a missing or
/// empty severity directory contributes nothing, and a directory that is not one of the four
/// (`bogus/`) is never scanned.
#[test]
fn collect_reviews_scans_severities_in_fixed_order_and_skips_unknown_dirs() {
    assert_eq!(SEVERITIES, ["critical", "high", "medium", "low"]);
    let dir = small_tree();
    let got = collect_reviews(dir.path());
    let want: Vec<(String, String)> = [
        ("c1", "critical"),
        ("m1", "medium"),
        ("m2", "medium"),
        ("l1", "low"),
    ]
    .iter()
    .map(|(a, b)| ((*a).to_string(), (*b).to_string()))
    .collect();
    assert_eq!(got, want);
}

// ---------------------------------------------------------------------------------------------
// render_status.rs
// ---------------------------------------------------------------------------------------------

/// `main()`'s validation (`flux_status.py:309-326`): no tokens -> every section; only the three
/// names are valid; any bad token fails the WHOLE call with the bad names sorted and deduped
/// (`sorted(sections - valid_sections)`), never a partial panel. Whitespace-only args count as
/// "no args" here — the shell would never deliver an empty positional to the Python, so the
/// case has no upstream line (labelled inference).
#[test]
fn parse_sections_accepts_the_three_names_and_rejects_the_rest_sorted_and_deduped() {
    assert_eq!(parse_sections(""), Ok((true, true, true)));
    assert_eq!(parse_sections("   "), Ok((true, true, true)));
    assert_eq!(parse_sections("todo"), Ok((true, false, false)));
    assert_eq!(parse_sections("done"), Ok((false, true, false)));
    assert_eq!(parse_sections("review"), Ok((false, false, true)));
    assert_eq!(parse_sections(" review  todo "), Ok((true, false, true)));
    assert_eq!(parse_sections("todo done review"), Ok((true, true, true)));
    assert_eq!(
        parse_sections("todo bogus done zzz bogus"),
        Err(vec!["bogus".to_string(), "zzz".to_string()])
    );
    assert_eq!(parse_sections("TODO"), Err(vec!["TODO".to_string()]));
    assert_eq!(
        parse_sections("todo,done"),
        Err(vec!["todo,done".to_string()])
    );
}

/// The golden the row said to write first: the whole panel, byte-for-byte against
/// `flux_status.py --no-color` on the same tree — column arithmetic (`name_w` = longest name + 2,
/// `total_w` = max(name_w + 8 + 18, 48)), every status glyph, the `(unknown)` cell for an empty
/// status and the bare text for an unknown one, the reverse-sorted run groups with their
/// `── label ──` rules, the review grid's fixed column widths and its `rstrip()`ped rows, and
/// the section separators that appear only after a previous section rendered.
#[test]
fn render_status_matches_flux_status_py_no_color_on_the_small_tree() {
    let dir = small_tree();
    let b = dir.path();
    assert_panel(
        &render(b, true, true, true),
        STATUS_SMALL_ALL,
        "all sections",
    );
    assert_panel(&render(b, true, false, false), STATUS_SMALL_TODO, "todo");
    assert_panel(&render(b, false, true, false), STATUS_SMALL_DONE, "done");
    assert_panel(
        &render(b, false, false, true),
        STATUS_SMALL_REVIEW,
        "review",
    );
    assert_panel(
        &render(b, true, false, true),
        STATUS_SMALL_TODO_REVIEW,
        "todo review",
    );
    assert_panel(
        &render(b, false, true, true),
        STATUS_SMALL_DONE_REVIEW,
        "done review",
    );
    // `--sections ""` upstream: no body at all, just the title between its two rules.
    assert_panel(
        &render(b, false, false, false),
        STATUS_SMALL_NONE,
        "no sections",
    );
}

/// `name_w` is capped at 50 (`flux_status.py:191`): a 57-char name overflows its column with no
/// padding (`str.ljust` never truncates), the rules still use the capped width (50 + 8 + 18 =
/// 76), the review underline is `name_w + 29 = 79` (`:257`, NOT `total_w`), and a done row whose
/// `status:` is present but blank renders `(unknown)`.
#[test]
fn render_status_caps_the_name_column_at_fifty_like_the_python() {
    let dir = wide_tree();
    assert_panel(
        &render(dir.path(), true, true, true),
        STATUS_WIDE_ALL,
        "wide tree",
    );
}

/// An existing base with nothing in it: the TODO section always renders (with `(no todos)`),
/// COMPLETED and REVIEW are skipped when empty (`:221`, `:243`), and the width floor of 48
/// applies (`_MIN_PANEL_W`, `:69`).
#[test]
fn render_status_on_an_empty_base_shows_no_todos_and_the_width_floor() {
    let dir = tempfile::tempdir().unwrap();
    assert_panel(
        &render(dir.path(), true, true, true),
        STATUS_EMPTY_ALL,
        "empty base",
    );
}

/// `main()`'s pre-render check (`flux_status.py:336-338`): a base that does not exist is a single
/// `(no flux state at <base>)` line, never a panel.
#[test]
fn render_status_names_the_missing_base() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nope");
    assert_eq!(
        render(&missing, true, true, true),
        format!("(no flux state at {})", missing.display())
    );
}

// ---------------------------------------------------------------------------------------------
// Goldens — verbatim `flux_status.py --no-color` output @v0.0.40 (trailing newline stripped).
// ---------------------------------------------------------------------------------------------

const STATUS_SMALL_ALL: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

TODO-FILE   STAGE   STATUS
────────────────────────────────────────────────
01-alpha    exec    🔄  in-progress
02-bravo    qa      🔁  needs-rework
03-charlie  aug     (unknown)
04-delta    exec    blocked
05-echo             (unknown)
06-foxtrot  done    ✅  done

════════════════════════════════════════════════
COMPLETED TASKS

TASK-FILE   STAGE   STATUS
── misc-run ──
x           exec    ✅  completed
── 2026-05-01 09:00 ──
only        exec    ✅  done
── 2026-04-29 16:57 ──
a-first     tests   ✅  completed
z-last      commit  ✅  completed

════════════════════════════════════════════════
REVIEW TASKS

REVIEW-FILE CRITICAL  HIGH  MEDIUM  LOW  
────────────────────────────────────────────────
c1          ●
m1                          ●
m2                          ●
l1                                  ●
════════════════════════════════════════════════"##;

const STATUS_SMALL_TODO: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

TODO-FILE   STAGE   STATUS
────────────────────────────────────────────────
01-alpha    exec    🔄  in-progress
02-bravo    qa      🔁  needs-rework
03-charlie  aug     (unknown)
04-delta    exec    blocked
05-echo             (unknown)
06-foxtrot  done    ✅  done
════════════════════════════════════════════════"##;

const STATUS_SMALL_DONE: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

COMPLETED TASKS

TASK-FILE  STAGE   STATUS
── misc-run ──
x          exec    ✅  completed
── 2026-05-01 09:00 ──
only       exec    ✅  done
── 2026-04-29 16:57 ──
a-first    tests   ✅  completed
z-last     commit  ✅  completed
════════════════════════════════════════════════"##;

const STATUS_SMALL_REVIEW: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

REVIEW TASKS

REVIEW-FILECRITICAL  HIGH  MEDIUM  LOW  
────────────────────────────────────────────────
c1         ●
m1                         ●
m2                         ●
l1                                 ●
════════════════════════════════════════════════"##;

const STATUS_SMALL_TODO_REVIEW: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

TODO-FILE   STAGE   STATUS
────────────────────────────────────────────────
01-alpha    exec    🔄  in-progress
02-bravo    qa      🔁  needs-rework
03-charlie  aug     (unknown)
04-delta    exec    blocked
05-echo             (unknown)
06-foxtrot  done    ✅  done

════════════════════════════════════════════════
REVIEW TASKS

REVIEW-FILE CRITICAL  HIGH  MEDIUM  LOW  
────────────────────────────────────────────────
c1          ●
m1                          ●
m2                          ●
l1                                  ●
════════════════════════════════════════════════"##;

const STATUS_SMALL_DONE_REVIEW: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

COMPLETED TASKS

TASK-FILE  STAGE   STATUS
── misc-run ──
x          exec    ✅  completed
── 2026-05-01 09:00 ──
only       exec    ✅  done
── 2026-04-29 16:57 ──
a-first    tests   ✅  completed
z-last     commit  ✅  completed

════════════════════════════════════════════════
REVIEW TASKS

REVIEW-FILECRITICAL  HIGH  MEDIUM  LOW  
────────────────────────────────────────────────
c1         ●
m1                         ●
m2                         ●
l1                                 ●
════════════════════════════════════════════════"##;

const STATUS_SMALL_NONE: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════
════════════════════════════════════════════════"##;

const STATUS_WIDE_ALL: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════════════════════════════════

TODO-FILE                                         STAGE   STATUS
────────────────────────────────────────────────────────────────────────────
a-very-long-task-name-that-exceeds-the-fifty-char-cap-xyzexec    🔄  in-progress
short                                             qa      ✅  done

════════════════════════════════════════════════════════════════════════════
COMPLETED TASKS

TASK-FILE                                         STAGE   STATUS
── 2026-06-01 12:30 ──
blank-status                                      exec    (unknown)

════════════════════════════════════════════════════════════════════════════
REVIEW TASKS

REVIEW-FILE                                       CRITICAL  HIGH  MEDIUM  LOW  
───────────────────────────────────────────────────────────────────────────────
h1                                                          ●
════════════════════════════════════════════════════════════════════════════"##;

const STATUS_EMPTY_ALL: &str = r##"𝕱 FLUX STATUS
════════════════════════════════════════════════

TODO-FILE  STAGE   STATUS
────────────────────────────────────────────────
(no todos)
════════════════════════════════════════════════"##;
