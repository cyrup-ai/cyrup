//! TUI-020's permanent regression cover: the OSC-8 hyperlink contract.
//!
//! The escapes exist ONLY in a rendered `Buffer` — `crate::osc::inject`
//! (`osc.rs:132-169`) runs after the widget, at `cache.rs:215` for the live
//! viewport and `app/draw.rs:191` for the scrollback flush. `App::scrollback_text`
//! is filled BEFORE that (`app/draw.rs:174`) and can never observe them, so every
//! test here paints through `Component::render` into a `TestBackend` and reads
//! `cell.symbol()` back.
//!
//! MIRROR: clause 8's `!bel.contains("8;;")` assertion lives in
//! `crate::tests::tool_result_sanitize::osc_sequences_do_not_survive_as_literal_text`
//! (`src/tests/tool_result_sanitize.rs:62-73`) and is NOT duplicated here; this
//! module covers the interaction that file cannot see — a linked HEADER above a
//! result BODY whose own OSC-8 payload was stripped.
//!
//! KNOWN LIMITATION, deliberately untested (see "Out of scope" in the brief):
//! `inject` stamps `CellDiffOption::ForcedWidth(1)` (`osc.rs:68-71`) on the head
//! and tail cells of every run. When that cell holds a WIDE grapheme — a CJK path
//! component at a wrap boundary — the forced width understates the true column
//! count by one, so `Buffer::diff_iter` (ratatui-core-0.1.2
//! `buffer/diff.rs:133-140`) does not skip the grapheme's trailing continuation
//! cell and that row's diff accounting is off by one column. The fix is to capture
//! `cell.symbol().cell_width()` (trait `ratatui::buffer::CellWidth`,
//! `buffer/cell_width.rs:19-24`) BEFORE prepending the escape — `Cell::cell_width`
//! returns the forced value once it is set (`buffer/cell.rs:309-317`) — and that
//! is a change to `osc.rs`, which this task may not make.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::ansi::strip_ansi;
use crate::transcript::*;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use serde_json::json;

/// A deterministic session cwd. Absolute and outside `$HOME`, so `shorten_path`
/// (`tool_result.rs:261-269`) is the identity and no assertion depends on the
/// developer's environment.
const CWD: &str = "/tmp/aug-osc";

/// `\x1b]8;;<url>\x07` — `osc::open`, restated here because it is private.
fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}
/// `\x1b]8;;\x07` — `osc::CLOSE`.
const CLOSE: &str = "\u{1b}]8;;\u{7}";

/// One live tool run in a view whose gate and cwd are pinned.
fn view(tool: &str, args: serde_json::Value, hyperlinks: bool) -> TranscriptView {
    let mut v = TranscriptView::new();
    v.set_cwd(Some(std::path::PathBuf::from(CWD)));
    v.set_hyperlinks(hyperlinks);
    v.push_tool_start(tool, args);
    v
}

/// Paint the active region and concatenate every cell symbol, row by row — the
/// injected escapes ride in `cell.symbol()`, so this string is the real byte
/// stream the backend would print. Height must exceed the block's logical line
/// count or `render` tail-anchors and scrolls the head off (`cache.rs:200-206`).
fn paint(v: &mut TranscriptView, theme: &UiTheme, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|frame| {
        let area = frame.area();
        v.render(frame, area, theme);
    })
    .unwrap();
    let buf = term.backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// Clause 1: the escape reaches the `Buffer` at all, in BEL-terminated OSC-8 form,
/// with the href built from the resolved absolute path.
#[test]
fn a_read_header_carries_the_open_and_close_escapes() {
    let theme = UiTheme::dark();
    let mut v = view("read", json!({ "file_path": "/tmp/aug-osc/main.rs" }), true);
    let text = paint(&mut v, &theme, 60, 12);
    // One contiguous run at width 60 (content 58, header 25 cols): `inject` prepends
    // `open` to the head cell and appends CLOSE to the tail cell, so the row reads
    // back as one uninterrupted sequence.
    let href = "file:///tmp/aug-osc/main.rs";
    assert!(
        text.contains(&format!("{}{}{CLOSE}", open(href), "/tmp/aug-osc/main.rs")),
        "no OSC-8-wrapped path in:\n{text:?}"
    );
}

/// Clause 2: `path_to_file_url` encoding, and the `link_style` split where the href
/// takes the **raw** path (`tool_args.rs:70-84`) and the span takes the
/// `shorten_path`-shortened one.
#[test]
fn the_href_is_the_raw_path_percent_encoded_and_the_text_is_shortened() {
    use cyrup_tools::path::path_to_file_url;
    use std::path::Path;

    // `path.rs:276-299` — SAFE excludes space, `#`, `%` and every non-ASCII byte.
    assert_eq!(path_to_file_url(Path::new("/tmp/café")), "file:///tmp/caf%C3%A9");
    assert_eq!(path_to_file_url(Path::new("/tmp/a b")), "file:///tmp/a%20b");
    assert_eq!(path_to_file_url(Path::new("/tmp/a#b")), "file:///tmp/a%23b");
    assert_eq!(path_to_file_url(Path::new("/tmp/a%b")), "file:///tmp/a%25b");

    // And through the render: `shorten_path` shortens the SPAN, never the href.
    // Read `$HOME` rather than writing it — six sibling test binaries share this
    // process's environment.
    let Ok(home) = std::env::var("HOME") else { return };
    if home.is_empty() || !home.starts_with('/') {
        return;
    }
    let raw = format!("{home}/aug osc/café.rs");
    let href = path_to_file_url(Path::new(&raw));
    assert!(href.contains("/aug%20osc/caf%C3%A9.rs"), "href not encoded: {href}");

    let theme = UiTheme::dark();
    let mut v = view("read", json!({ "file_path": raw.clone() }), true);
    let text = paint(&mut v, &theme, 70, 12);
    assert!(
        text.contains(&format!("{}~/aug osc/café.rs{CLOSE}", open(&href))),
        "the href must be raw+encoded and the text `~`-shortened:\n{text:?}"
    );
    let plain = strip_ansi(&text);
    assert!(plain.contains("~/aug osc/café.rs"), "visible text lost:\n{plain}");
    assert!(!plain.contains("%20"), "the encoded form must never be visible:\n{plain}");
}

/// Clause 3: all three arms of `tool_path_span` (`tool_args.rs:47-55`) — the
/// `Some(".")` empty fallback only `ls` passes, `[invalid arg]`, and `...`. The
/// last two also exercise `inject`'s `is_empty()` early-out (`osc.rs:133-135`).
#[test]
fn ls_links_the_session_cwd_and_the_two_unlinked_arms_emit_no_escape() {
    let theme = UiTheme::dark();

    // `ls` with no `path` → `empty_fallback = Some(".")` → shorten_path(".") == "."
    // linked to `resolve_to_cwd(".", cwd)` == the session cwd.
    let mut ls = view("ls", json!({}), true);
    let text = paint(&mut ls, &theme, 60, 12);
    assert!(
        text.contains(&format!("{}.{CLOSE}", open("file:///tmp/aug-osc"))),
        "`ls` must link the session cwd:\n{text:?}"
    );

    // A non-string path → `StrArg::Invalid` → `[invalid arg]`, error_style, no link.
    let mut invalid = view("read", json!({ "file_path": 42 }), true);
    let text = paint(&mut invalid, &theme, 60, 12);
    assert!(strip_ansi(&text).contains("[invalid arg]"), "arm lost:\n{text:?}");
    assert!(!text.contains('\u{1b}'), "`[invalid arg]` must stay inert:\n{text:?}");

    // No path at all and no fallback → the `...` placeholder, tool_output_style, no link.
    let mut missing = view("read", json!({}), true);
    let text = paint(&mut missing, &theme, 60, 12);
    assert!(strip_ansi(&text).contains("read ..."), "arm lost:\n{text:?}");
    assert!(!text.contains('\u{1b}'), "the `...` placeholder must stay inert:\n{text:?}");
}

/// Clause 4: the deliberate non-parity — only four call sites link, and
/// `push_search_path` (`tool_args.rs:90-96`) / `compact_read_call`
/// (`tool_args.rs:351`) are not among them. Each half asserts the text is still
/// present, so the test cannot pass by the tail simply disappearing.
#[test]
fn grep_find_tails_and_the_compact_read_header_stay_unlinked() {
    let theme = UiTheme::dark();

    for tool in ["grep", "find"] {
        let mut v = view(tool, json!({ "pattern": "x", "path": "/tmp/aug-osc" }), true);
        let text = paint(&mut v, &theme, 60, 12);
        assert!(
            strip_ansi(&text).contains("/tmp/aug-osc"),
            "`{tool}` lost its ` in <path>` tail:\n{text:?}"
        );
        assert!(
            !text.contains('\u{1b}'),
            "`push_search_path` is deliberately unlinked (`tool_args.rs:90-96`):\n{text:?}"
        );
    }

    // The collapsed compact `read` header is `compact_read_call` (`tool_args.rs:351`),
    // which never reaches `tool_path_span`.
    let mut v = view("read", json!({ "file_path": "/tmp/aug-osc/CLAUDE.md" }), true);
    let text = paint(&mut v, &theme, 60, 12);
    assert!(
        strip_ansi(&text).contains("read resource CLAUDE.md"),
        "compact header lost:\n{text:?}"
    );
    assert!(!text.contains('\u{1b}'), "the compact header is unlinked:\n{text:?}");
}

/// Clause 5: pi's `if (!getCapabilities().hyperlinks) return styledText` early
/// return (`tool_args.rs:71-73`) — the branch every pre-existing cyrup-tui test
/// runs, since `ImageOpts::default` seeds `hyperlinks: false` (`tool_render.rs:145`).
#[test]
fn the_gate_off_buffer_is_byte_identical_to_today() {
    let theme = UiTheme::dark();
    let args = json!({ "file_path": "/tmp/aug-osc/main.rs" });
    let mut off = view("read", args.clone(), false);
    let text = paint(&mut off, &theme, 60, 12);
    assert!(text.contains("/tmp/aug-osc/main.rs"), "path lost:\n{text:?}");
    assert!(!text.contains('\u{1b}'), "no ESC with the gate off:\n{text:?}");
    assert!(!text.contains("]8;;"), "no OSC-8 payload with the gate off:\n{text:?}");

    // And the gate-on render is the same STRING once the escapes are stripped —
    // the same buffer, plus escapes, never plus columns.
    let mut on = view("read", args, true);
    assert_eq!(strip_ansi(&paint(&mut on, &theme, 60, 12)), text);
}

/// Clause 6 — the clause the `osc.rs:35-49` design note exists for. At width 40 the
/// content width is 38; the path below is 64 columns, so `wrap_line`'s long-word
/// arm (`layout.rs:72-86`) hard-breaks it into a 38-column piece and a 26-column
/// piece on two rows, each left-padded by `box_lines` and right-padded by
/// `apply_bg`. That is two marked runs of ONE id, and both must resolve to the
/// same href.
#[test]
fn a_wrapped_path_emits_one_pair_per_row_with_the_same_href() {
    let theme = UiTheme::dark();
    let path = "/tmp/aug-osc/a-really-long-directory-name/and-another-one/file.rs";
    let href = format!("file://{path}");

    let mut on = view("read", json!({ "file_path": path }), true);
    let linked = paint(&mut on, &theme, 40, 12);
    let mut off = view("read", json!({ "file_path": path }), false);
    let plain = paint(&mut off, &theme, 40, 12);

    // Columns do not move: the escapes are the ONLY difference between the buffers.
    assert_eq!(strip_ansi(&linked), plain, "the wrap moved a column");

    // One open/close pair PER ROW — `box_lines` padding breaks the run in two, and
    // `inject` resolves both to the SAME href because the id names the link outright.
    assert_eq!(
        linked.matches(&open(&href)).count(),
        2,
        "expected one `open` per wrapped row:\n{linked:?}"
    );
    assert_eq!(linked.matches(CLOSE).count(), 2, "unbalanced close:\n{linked:?}");
    // No OTHER href was emitted — a cyclic id scheme would have produced one.
    assert_eq!(linked.matches("\u{1b}]8;;file://").count(), 2, "stray href:\n{linked:?}");
}

/// Clause 7 — the most important test in the module. This is the regression the
/// original design's global `seen` counter would have shipped, and the reason
/// `mark` (`osc.rs:88-97`) hands out pass-unique one-based ids. Both runs are live
/// in the same `active_tools` list, so both are marked against the one `LinkSink`
/// `cached_render` built at `cache.rs:35`.
#[test]
fn two_links_in_one_pass_resolve_to_distinct_hrefs() {
    let theme = UiTheme::dark();
    let mut on = TranscriptView::new();
    on.set_cwd(Some(std::path::PathBuf::from(CWD)));
    on.set_hyperlinks(true);
    on.push_tool_start("read", json!({ "file_path": "/tmp/aug-osc/first.rs" }));
    on.push_tool_start("write", json!({ "file_path": "/tmp/aug-osc/second.rs" }));
    let linked = paint(&mut on, &theme, 60, 16);

    // Each visible path is wrapped by ITS OWN href. A cyclic counter links the
    // second header to the first file — this pair of asserts is what catches it.
    for name in ["first.rs", "second.rs"] {
        let p = format!("/tmp/aug-osc/{name}");
        assert!(
            linked.contains(&format!("{}{p}{CLOSE}", open(&format!("file://{p}")))),
            "`{name}` is not wrapped by its own href:\n{linked:?}"
        );
    }
    assert_eq!(linked.matches("\u{1b}]8;;file://").count(), 2, "id reuse:\n{linked:?}");

    // Columns do not move, and the content height is gate-independent.
    let mut off = TranscriptView::new();
    off.set_cwd(Some(std::path::PathBuf::from(CWD)));
    off.set_hyperlinks(false);
    off.push_tool_start("read", json!({ "file_path": "/tmp/aug-osc/first.rs" }));
    off.push_tool_start("write", json!({ "file_path": "/tmp/aug-osc/second.rs" }));
    assert_eq!(strip_ansi(&linked), paint(&mut off, &theme, 60, 16));
    assert_eq!(on.content_height(60, &theme), off.content_height(60, &theme));
}

/// Clause 8, redirected: the `!bel.contains("8;;")` assertion is already covered at
/// `src/tests/tool_result_sanitize.rs:62-73`. What neither file sees is the
/// INTERACTION — `result_text` (`tool_result.rs:59-76`) strips the BODY's escapes
/// at materialisation time while `inject` adds the HEADER's after the widget has
/// run. Opposite directions, one buffer.
#[test]
fn a_linked_header_sits_above_a_result_body_whose_own_osc_8_was_stripped() {
    let theme = UiTheme::dark();
    let mut v = view("ls", json!({}), true);
    v.push_tool_end(
        "ls",
        false,
        Some(json!({ "content": [{ "type": "text",
            "text": "\u{1b}]8;;file:///tmp/evil\u{7}linked\u{1b}]8;;\u{7}\nplain.txt" }] })),
    );
    let text = paint(&mut v, &theme, 60, 16);

    // The header's escape survives — it was added AFTER the widget wrote the cells.
    assert!(
        text.contains(&format!("{}.{CLOSE}", open("file:///tmp/aug-osc"))),
        "header link lost:\n{text:?}"
    );
    // The body's did not — `result_text` → `ansi::sanitize_display_text` removed it.
    assert!(!text.contains("file:///tmp/evil"), "body OSC-8 survived:\n{text:?}");
    let plain = strip_ansi(&text);
    assert!(plain.contains("linked"), "body content lost:\n{plain}");
    assert!(plain.contains("plain.txt"), "body content lost:\n{plain}");
}

/// Clause 9's executable half: a linked header never grows a visible ` (url)`
/// suffix — pi's `linkPath` (`render-utils.ts:19-23`) returns the styled text
/// wrapped, never annotated. The loop also covers the fourth linking call site,
/// `edit` (`tool_builtin.rs:175`), which no other test here reaches. (Clause 9's
/// second half — "`image.rs` and `ansi.rs` are untouched" — is a statement about a
/// diff, not a runtime property, and is not testable.)
#[test]
fn there_is_no_visible_url_fallback() {
    let theme = UiTheme::dark();
    for (tool, args) in [
        ("read", json!({ "file_path": "/tmp/aug-osc/main.rs" })),
        ("write", json!({ "file_path": "/tmp/aug-osc/main.rs" })),
        ("edit", json!({ "file_path": "/tmp/aug-osc/main.rs" })),
        ("ls", json!({ "path": "/tmp/aug-osc" })),
    ] {
        let mut on = view(tool, args.clone(), true);
        let linked = paint(&mut on, &theme, 60, 16);
        let mut off = view(tool, args, false);
        let plain = paint(&mut off, &theme, 60, 16);

        assert!(linked.contains("\u{1b}]8;;file://"), "`{tool}` must link:\n{linked:?}");
        let visible = strip_ansi(&linked);
        assert!(!visible.contains("(file://"), "`{tool}` grew a url suffix:\n{visible}");
        assert!(!visible.contains(" (url"), "`{tool}` grew a url suffix:\n{visible}");
        // The strongest form of the same claim: the gate adds bytes, never columns.
        assert_eq!(visible, plain, "`{tool}` moved a column when linked");
    }
}
