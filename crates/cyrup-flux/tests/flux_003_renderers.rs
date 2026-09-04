//! FLUX-003 — `/flux/cheatsheet`, `/flux/about` and the bundled reference docs, pinned against
//! the upstream Python they port.
//!
//! `render_cheatsheet.rs` reimplements `flux_cheatsheet.py` @v0.0.40
//! (`code_puppy_core_plugins/flux_bootstrap/bundled/scripts/`) — its two regexes
//! (`SLASH_CMD`, `PIPELINE_HEADING`, `:64-65`) as character predicates, `parse_pipelines`
//! (`:85-131`), `render` (`:144-164`) and `main()`'s filter validation and empty-state lines
//! (`:201-241`). `render_about.rs` reimplements `flux_about.py`'s `SLASH_CMD_RE`
//! (`(?<![:\w/])//(?=\w)`, `:53`) as a hand-rolled lookbehind. Every expectation here is the
//! Python's own output: the tables were produced by importing the scripts and calling
//! `strip_slashes`, `PIPELINE_HEADING.match` and `SLASH_CMD_RE.sub` directly; the panels by
//! `python3 flux_cheatsheet.py --no-color --docs <dir> [pipeline]` on (a) a synthetic
//! `pipeline.md` written to exercise every branch of the parser (`SYNTHETIC_PIPELINE_MD` below)
//! and (b) the vendored `resources/prompts/flux/_docs/`, with `print`'s trailing newline
//! stripped. Re-run those commands from the extracted script (`git -C tmp/code_puppy_core_plugins
//! show v0.0.40:code_puppy_core_plugins/flux_bootstrap/bundled/scripts/flux_cheatsheet.py`)
//! whenever the vendored doc or the synthetic one changes.
//!
//! Red before / green after: the parser and layout pins are green on both sides (the row's
//! missing evidence). One test is RED against the pre-change `render_about.rs`:
//! `normalize_slash_cmd_matches_the_python_lookbehind` — its `is_word` was ASCII-only where
//! Python's `\w` is Unicode, so `//é` was left alone and `é//x` was rewritten, both the
//! opposite of upstream.
//!
//! The last test is FLUX-005's item (8): the four `_docs`/`skills/flux/reference` pairs are
//! shipped twice with no sync mechanism; byte-equality here turns a silent divergence into a red
//! build.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_flux::bundle::{bundled_file, bundled_files};
use cyrup_flux::render_about;
use cyrup_flux::render_cheatsheet::{
    match_pipeline_heading, parse_arg, render, render_doc, strip_slashes,
};

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

// ---------------------------------------------------------------------------------------------
// render_cheatsheet.rs
// ---------------------------------------------------------------------------------------------

/// `PIPELINE_HEADING = re.compile(r"^##\s+PIPELINE\s+([A-Za-z0-9]+)\s*:")`
/// (`flux_cheatsheet.py:65`), `.match` anchored at the start, group upper-cased (`:97`):
/// exactly two hashes, at least one whitespace after each keyword (tab and NBSP count), an
/// alphanumeric run of any length, optional whitespace, then the colon; anything else — a third
/// hash, a leading space, lowercase `pipeline`, a hyphen inside the letter, a missing colon —
/// is not a heading.
#[test]
fn match_pipeline_heading_matches_the_python_regex() {
    let table: &[(&str, Option<&str>)] = &[
        ("## PIPELINE A:", Some("A")),
        ("##  PIPELINE b :", Some("B")),
        ("##PIPELINE C:", None),
        ("# PIPELINE D:", None),
        ("## PIPELINE E", None),
        ("## PIPELINE 2: numeric", Some("2")),
        ("## PIPELINE A: desc", Some("A")),
        ("##\tPIPELINE\tA\t:", Some("A")),
        ("## PIPELINE :", None),
        ("## PIPELINE Ab1:", Some("AB1")),
        ("## PIPELINE A-1:", None),
        ("## PIPELINE A: x", Some("A")),
        ("## PIPELINE\u{a0}A:", Some("A")),
        ("### PIPELINE A:", None),
        (" ## PIPELINE A:", None),
        ("## PIPELINEA:", None),
        ("## pipeline A:", None),
        ("", None),
    ];
    for (line, want) in table {
        assert_eq!(
            match_pipeline_heading(line).as_deref(),
            *want,
            "PIPELINE_HEADING.match({line:?})"
        );
    }
}

/// `SLASH_CMD = re.compile(r"/+flux/")` / `.sub("/flux/", line)` (`:64`, `:82`): any run of
/// slashes directly before `flux/` collapses to one, everywhere in the line — including after
/// `http:` — and nothing else is touched (`//flux` without the trailing slash, `//fluxx`).
#[test]
fn strip_slashes_matches_the_python_regex() {
    let table = [
        ("//flux/new", "/flux/new"),
        ("///flux/x", "/flux/x"),
        ("/flux/new", "/flux/new"),
        ("a//fluxx", "a//fluxx"),
        ("//flux", "//flux"),
        ("x//flux/y ///flux/z", "x/flux/y /flux/z"),
        ("  //flux/exec 3  # comment", "  /flux/exec 3  # comment"),
        ("http://flux/", "http:/flux/"),
        ("flux/", "flux/"),
        ("", ""),
    ];
    for (line, want) in table {
        assert_eq!(strip_slashes(line), want, "strip_slashes({line:?})");
    }
}

/// `main()` (`flux_cheatsheet.py:203-210`): the positional filter is `strip().upper()`ed and
/// must be one of `A`-`D`; empty means no filter; anything else is an error carrying the RAW
/// argument (`{selected_pipeline!r}`), so the caller can quote it as the Python does.
#[test]
fn parse_arg_normalises_a_to_d_and_rejects_everything_else_with_the_raw_text() {
    assert_eq!(parse_arg(""), Ok(None));
    assert_eq!(parse_arg("   "), Ok(None));
    assert_eq!(parse_arg("a"), Ok(Some("A".into())));
    assert_eq!(parse_arg(" B "), Ok(Some("B".into())));
    assert_eq!(parse_arg("c"), Ok(Some("C".into())));
    assert_eq!(parse_arg("D"), Ok(Some("D".into())));
    assert_eq!(parse_arg("e"), Err("e".into()));
    assert_eq!(parse_arg(" e "), Err(" e ".into()));
    assert_eq!(parse_arg("AB"), Err("AB".into()));
    assert_eq!(parse_arg("a b"), Err("a b".into()));
}

/// A `pipeline.md` written to hit every branch of `parse_pipelines` (`flux_cheatsheet.py:85-131`):
/// prose and a non-pipeline `##` heading before the first pipeline; a description search that
/// skips blank lines, `---` and `#`-headings and takes the first other line (leaving `//flux/`
/// in the description UNtouched — only flow lines are `strip_slashes`d, `:157` vs `:160`); a
/// fence with leading/trailing blank lines trimmed and an inner blank line kept; a second fence
/// ignored; a lowercase heading with a space before the colon; a fence-first pipeline (empty
/// description, prose after the fence ignored); a description-only pipeline; a numeric
/// "letter"; and a `##PIPELINE` line that is NOT a heading and so is swallowed by the previous
/// pipeline's tail.
const SYNTHETIC_PIPELINE_MD: &str = r##"# Flux Pipelines

Intro text that is not a pipeline.

## Available Commands

- `//flux/new` — start

## PIPELINE A:

---

# Not the description (heading)

First pipeline description with //flux/new inside.

Second paragraph is ignored.

```
//flux/new "idea"
  # comment with ///flux/split

//flux/exec 3
```

```
second fence is ignored
```

## PIPELINE b :
```

  //flux/ask

```
Description after the fence never counts.

## PIPELINE C:
Only a description, no fence.

## PIPELINE 2: numeric letter
```
/flux/status
```

##PIPELINE D:
not a heading, so this belongs to PIPELINE 2's tail.
"##;

/// `render` (`:144-164`) over the synthetic doc, byte-for-byte against the Python: the 60-wide
/// rules, the `─` separator + blank line only BETWEEN pipelines, the description line only when
/// non-empty, the blank line after it even when the flow is empty, and the closing blank + rule.
#[test]
fn render_doc_matches_flux_cheatsheet_py_no_color_on_the_synthetic_doc() {
    assert_panel(
        &render_doc(SYNTHETIC_PIPELINE_MD, None),
        CHEAT_SYN_ALL,
        "synthetic, all pipelines",
    );
    assert_panel(
        &render_doc(SYNTHETIC_PIPELINE_MD, Some("A")),
        CHEAT_SYN_A,
        "synthetic, A",
    );
    assert_panel(
        &render_doc(SYNTHETIC_PIPELINE_MD, Some("B")),
        CHEAT_SYN_B,
        "synthetic, B",
    );
    assert_panel(
        &render_doc(SYNTHETIC_PIPELINE_MD, Some("C")),
        CHEAT_SYN_C,
        "synthetic, C",
    );
}

/// `main()`'s two empty-state lines (`flux_cheatsheet.py:236`, `:240`): a valid filter that
/// matches no pipeline, and a document with no pipeline headings at all.
#[test]
fn render_doc_reports_a_missing_pipeline_and_a_pipeline_less_doc_in_the_python_wording() {
    assert_eq!(
        render_doc(SYNTHETIC_PIPELINE_MD, Some("D")),
        "(no PIPELINE D found)"
    );
    assert_eq!(
        render_doc("# nothing\n\nprose only\n", None),
        "(no pipelines found in pipeline.md)"
    );
    assert_eq!(render_doc("", None), "(no pipelines found in pipeline.md)");
    assert_eq!(render_doc("", Some("A")), "(no PIPELINE A found)");
}

/// The shipped panel: `render(None)` over the compiled-in `_docs/pipeline.md` is what the
/// Python prints for the same file with `--docs` pointed at the vendored directory, and it IS the
/// bundle's copy of that file (the `include_str!` and the embedded bundle cannot drift apart).
#[test]
fn render_matches_flux_cheatsheet_py_no_color_on_the_vendored_doc() {
    assert_panel(&render(None), CHEAT_REAL_ALL, "vendored, all pipelines");
    assert_panel(&render(Some("A")), CHEAT_REAL_A, "vendored, A");
    let bundled =
        std::str::from_utf8(bundled_file("prompts/flux/_docs/pipeline.md").unwrap()).unwrap();
    assert_eq!(render(None), render_doc(bundled, None));
    for letter in ["A", "B", "C", "D"] {
        assert!(
            render(Some(letter)).contains(&format!("PIPELINE {letter}")),
            "the vendored doc must define PIPELINE {letter}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// render_about.rs
// ---------------------------------------------------------------------------------------------

/// `SLASH_CMD_RE = re.compile(r"(?<![:\w/])//(?=\w)")` / `.sub("/", body)` (`flux_about.py:53`,
/// `:61`): `//` becomes `/` only when followed by a word character and NOT preceded by `:`, `/`
/// or a word character — so URLs (`https://…//path`), `foo//bar`, `///x` and a trailing `//`
/// are untouched, while a start-of-string, whitespace, bracket or quote before it rewrites.
/// `\w` is Unicode in a Python `str` pattern: `//é` rewrites and `é//x` does not.
#[test]
fn normalize_slash_cmd_matches_the_python_lookbehind() {
    let table = [
        ("//flux/about", "/flux/about"),
        ("run //flux/new now", "run /flux/new now"),
        ("https://example.com//path", "https://example.com//path"),
        ("foo//bar", "foo//bar"),
        (" //x", " /x"),
        ("///x", "///x"),
        ("(//x)", "(/x)"),
        ("end//", "end//"),
        ("//_x", "/_x"),
        ("//é", "/é"),
        ("é//x", "é//x"),
        ("a://b", "a://b"),
        ("//9", "/9"),
        ("//-x", "//-x"),
        ("\"//flux/status\"", "\"/flux/status\""),
        ("x /flux/y", "x /flux/y"),
        ("//a//b", "/a//b"),
        ("«//flux»", "«/flux»"),
        ("", ""),
    ];
    for (input, want) in table {
        assert_eq!(
            render_about::normalize_slash_cmd(input),
            want,
            "SLASH_CMD_RE.sub({input:?})"
        );
    }
}

/// `/flux/about` renders the bundled `_docs/about.md` (vendored already frontmatter- and
/// preamble-stripped), trimmed, through the slash normalisation — and the one Wibey-era `//flux`
/// the vendored body still carries comes out as `/flux`.
#[test]
fn render_about_is_the_bundled_body_normalised() {
    let bundled =
        std::str::from_utf8(bundled_file("prompts/flux/_docs/about.md").unwrap()).unwrap();
    let out = render_about::render();
    assert_eq!(out, render_about::normalize_slash_cmd(bundled.trim()));
    assert!(out.starts_with("# /flux/about"), "{out:.40}");
    assert!(bundled.contains("Show the //flux pipeline cheatsheet"));
    assert!(out.contains("Show the /flux pipeline cheatsheet"));
    assert!(!out.contains("//flux"), "a `//flux` survived normalisation");
    assert!(
        !out.starts_with("---"),
        "frontmatter must be stripped at vendor time"
    );
    assert!(
        !out.contains("Output the following overview exactly"),
        "the AI-only preamble must be stripped at vendor time"
    );
}

// ---------------------------------------------------------------------------------------------
// FLUX-005 — the duplicated reference docs
// ---------------------------------------------------------------------------------------------

/// `_docs/{README,cheatsheet,pipeline,synopsis}.md` (compiled into the renderers and installed
/// under `prompts/`) and `skills/flux/reference/{…}.md` (what `/skill:flux` reads) are two
/// hand-maintained copies. Byte-equality is the sync mechanism until one copy goes. The census
/// is pinned too: `_docs` is five files (`about.md` has no `reference/` twin), `reference` four.
#[test]
fn the_four_docs_reference_pairs_are_byte_identical() {
    for name in ["README.md", "cheatsheet.md", "pipeline.md", "synopsis.md"] {
        let docs = bundled_file(&format!("prompts/flux/_docs/{name}"))
            .unwrap_or_else(|| panic!("_docs/{name} missing from the bundle"));
        let reference = bundled_file(&format!("skills/flux/reference/{name}"))
            .unwrap_or_else(|| panic!("skills/flux/reference/{name} missing from the bundle"));
        assert!(
            docs == reference,
            "_docs/{name} and skills/flux/reference/{name} have diverged — edit both or delete one"
        );
    }
    let docs: Vec<&str> = bundled_files()
        .iter()
        .map(|f| f.rel)
        .filter(|r| r.starts_with("prompts/flux/_docs/"))
        .collect();
    let reference: Vec<&str> = bundled_files()
        .iter()
        .map(|f| f.rel)
        .filter(|r| r.starts_with("skills/flux/reference/"))
        .collect();
    assert_eq!(
        docs,
        [
            "prompts/flux/_docs/README.md",
            "prompts/flux/_docs/about.md",
            "prompts/flux/_docs/cheatsheet.md",
            "prompts/flux/_docs/pipeline.md",
            "prompts/flux/_docs/synopsis.md",
        ]
    );
    assert_eq!(
        reference,
        [
            "skills/flux/reference/README.md",
            "skills/flux/reference/cheatsheet.md",
            "skills/flux/reference/pipeline.md",
            "skills/flux/reference/synopsis.md",
        ]
    );
}

// ---------------------------------------------------------------------------------------------
// Goldens — verbatim `flux_cheatsheet.py --no-color` output @v0.0.40 (trailing newline stripped).
// ---------------------------------------------------------------------------------------------

const CHEAT_SYN_ALL: &str = r##"𝕱 FLUX CHEATSHEET
════════════════════════════════════════════════════════════

PIPELINE A
First pipeline description with //flux/new inside.

/flux/new "idea"
  # comment with /flux/split

/flux/exec 3

────────────────────────────────────────────────────────────

PIPELINE B

  /flux/ask

────────────────────────────────────────────────────────────

PIPELINE C
Only a description, no fence.


────────────────────────────────────────────────────────────

PIPELINE 2

/flux/status

════════════════════════════════════════════════════════════"##;

const CHEAT_SYN_A: &str = r##"𝕱 FLUX CHEATSHEET
════════════════════════════════════════════════════════════

PIPELINE A
First pipeline description with //flux/new inside.

/flux/new "idea"
  # comment with /flux/split

/flux/exec 3

════════════════════════════════════════════════════════════"##;

const CHEAT_SYN_B: &str = r##"𝕱 FLUX CHEATSHEET
════════════════════════════════════════════════════════════

PIPELINE B

  /flux/ask

════════════════════════════════════════════════════════════"##;

const CHEAT_SYN_C: &str = r##"𝕱 FLUX CHEATSHEET
════════════════════════════════════════════════════════════

PIPELINE C
Only a description, no fence.


════════════════════════════════════════════════════════════"##;

const CHEAT_REAL_ALL: &str = r##"𝕱 FLUX CHEATSHEET
════════════════════════════════════════════════════════════

PIPELINE A
New ticket, feature, or bug fix

/flux/new
 -> /flux/ask
  -> /flux/split
   -> /flux/aug
    -> /flux/exec
     -> /flux/qa
      -> /flux/tests
       -> /flux/commit
        -> run the app, test the changes
         -> /flux/create-pr

────────────────────────────────────────────────────────────

PIPELINE B
Review my own changes on the current branch

/flux/review
 -> /flux/address-feedback
  -> /flux/ask
   -> /flux/exec
    -> /flux/qa
     -> /flux/tests
      -> /flux/commit

────────────────────────────────────────────────────────────

PIPELINE C
Address review feedback

/flux/address-feedback
 -> /flux/ask
  -> /flux/exec
   -> /flux/qa
    -> /flux/tests
     -> /flux/commit

────────────────────────────────────────────────────────────

PIPELINE D
Review someone else's PR

# If the review is done from the PR branch itself (recommended)

/flux/review

# If the review is done from another branch, you need to provide PR number

/flux/review <PR#>

# For both cases, after the review has completed:
# -> post a comment on the PR, "Code changes suggested", and attach the zip file created
# (e.g. ~/.flux/<flattened-dir>/review.zip)

════════════════════════════════════════════════════════════"##;

const CHEAT_REAL_A: &str = r##"𝕱 FLUX CHEATSHEET
════════════════════════════════════════════════════════════

PIPELINE A
New ticket, feature, or bug fix

/flux/new
 -> /flux/ask
  -> /flux/split
   -> /flux/aug
    -> /flux/exec
     -> /flux/qa
      -> /flux/tests
       -> /flux/commit
        -> run the app, test the changes
         -> /flux/create-pr

════════════════════════════════════════════════════════════"##;
