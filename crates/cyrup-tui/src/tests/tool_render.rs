//! Per-tool rich tool-execution rendering (`tool-execution.ts` dispatch to each built-in's
//! `renderCall`/`renderResult`: read/write/edit/bash/grep/find/ls) + `Ctrl+O` expand + the edit
//! self-diff. Pi has no gear/check glyph — execution state is the block background tint, which the
//! `TestBackend` symbol grid does not carry, so these assert on the rendered text.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::{Component, TranscriptView, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use serde_json::json;

fn render(view: &mut TranscriptView, w: u16, h: u16) -> String {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let theme = UiTheme::dark();
    term.draw(|f| view.render(f, Rect::new(0, 0, w, h), &theme)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..h {
        for x in 0..w {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// A `{content:[{type:text,text}], details}` tool result value.
fn text_result(text: &str, details: serde_json::Value) -> serde_json::Value {
    json!({ "content": [{ "type": "text", "text": text }], "details": details })
}

#[test]
fn read_call_shows_path_and_line_range() {
    let mut view = TranscriptView::new();
    view.push_tool_start("read", json!({ "path": "src/main.rs", "offset": 10, "limit": 5 }));
    assert!(view.has_active(), "a live tool keeps the viewport active");
    let text = render(&mut view, 70, 8);
    // `read <path>:<start>-<end>` (read.ts:74-77): no gear glyph, no `read(...)` parens.
    assert!(text.contains("read src/main.rs:10-14"), "read header + range: {text:?}");
    assert!(!text.contains("read("), "no generic paren-wrapped marker: {text:?}");
    assert!(!text.contains('⚙'), "Pi has no running glyph: {text:?}");
}

#[test]
fn read_result_hidden_until_expanded() {
    let mut view = TranscriptView::new();
    view.push_tool_start("read", json!({ "path": "a.rs" }));
    view.push_tool_end("read", false, Some(text_result("line one\nline two\nline three", json!(null))));
    // Collapsed: `formatReadResult` returns "" for a non-error read (read.ts:173-175) — header only.
    let collapsed = render(&mut view, 70, 10);
    assert!(collapsed.contains("read a.rs"), "header present: {collapsed:?}");
    assert!(!collapsed.contains("line one"), "body hidden when collapsed: {collapsed:?}");
    // Ctrl+O expands → the file body appears.
    assert!(view.toggle_tool_expanded());
    let expanded = render(&mut view, 70, 10);
    assert!(expanded.contains("line one"), "body shown when expanded: {expanded:?}");
    assert!(expanded.contains("line three"), "full body when expanded: {expanded:?}");
}

#[test]
fn bash_call_and_output_tail() {
    let mut view = TranscriptView::new();
    view.push_tool_start("bash", json!({ "command": "ls" }));
    // 25 lines so the collapsed tail-5 (BASH_PREVIEW_LINES, bash.ts:174) hides the first 20.
    let body: String = (1..=25).map(|i| format!("ln{i:02}")).collect::<Vec<_>>().join("\n");
    view.push_tool_end("bash", false, Some(text_result(&body, json!(null))));
    let collapsed = render(&mut view, 70, 20);
    assert!(collapsed.contains("$ ls"), "bash `$ command` header: {collapsed:?}");
    assert!(collapsed.contains("ln25"), "tail line previewed: {collapsed:?}");
    assert!(collapsed.contains("ln21"), "5-line tail starts at ln21: {collapsed:?}");
    assert!(!collapsed.contains("ln20"), "earlier lines hidden: {collapsed:?}");
    assert!(collapsed.contains("20 earlier lines"), "hidden-head hint: {collapsed:?}");
    assert!(collapsed.contains("Took"), "duration footer: {collapsed:?}");

    // Ctrl+O expand → every line visible, including the head.
    assert!(view.toggle_tool_expanded());
    let expanded = render(&mut view, 70, 30);
    assert!(expanded.contains("ln01"), "all lines visible when expanded: {expanded:?}");
}

/// Pi renders the shell call with `config.prompt` (`formatShellCall(args, config.prompt)`,
/// bash.ts:488): `$` for bash (bash.ts:523) and `PS>` for PowerShell (powershell.ts:43). Everything
/// else about the row — timeout suffix, tail, expand, `Took …` — is the same renderer.
#[test]
fn powershell_call_renders_with_the_ps_prompt() {
    let mut view = TranscriptView::new();
    view.push_tool_start("powershell", json!({ "command": "Get-ChildItem", "timeout": 5 }));
    let body: String = (1..=25).map(|i| format!("ln{i:02}")).collect::<Vec<_>>().join("\n");
    view.push_tool_end("powershell", false, Some(text_result(&body, json!(null))));

    let collapsed = render(&mut view, 70, 20);
    assert!(
        collapsed.contains("PS> Get-ChildItem"),
        "PowerShell header uses `PS>`: {collapsed:?}"
    );
    assert!(
        !collapsed.contains("$ Get-ChildItem"),
        "never the bash prompt: {collapsed:?}"
    );
    assert!(collapsed.contains("(timeout 5s)"), "timeout suffix: {collapsed:?}");
    assert!(collapsed.contains("ln25") && collapsed.contains("ln21"), "5-line tail: {collapsed:?}");
    assert!(!collapsed.contains("ln20"), "earlier lines hidden: {collapsed:?}");
    assert!(collapsed.contains("Took"), "duration footer: {collapsed:?}");

    assert!(view.toggle_tool_expanded());
    let expanded = render(&mut view, 70, 30);
    assert!(expanded.contains("ln01"), "expand shows the head: {expanded:?}");
    assert!(expanded.contains("PS> Get-ChildItem"), "header survives expand: {expanded:?}");
}

#[test]
fn edit_result_renders_the_self_diff() {
    let mut view = TranscriptView::new();
    view.push_tool_start("edit", json!({ "path": "a.txt" }));
    // The diff lives in `details.diff` (edit.ts:359); the content text is a plain success message.
    view.push_tool_end(
        "edit",
        false,
        Some(text_result(
            "Successfully replaced 1 block(s) in a.txt.",
            json!({ "diff": "-1 old text\n+1 new text" }),
        )),
    );
    let text = render(&mut view, 70, 10);
    assert!(text.contains("edit a.txt"), "edit header: {text:?}");
    assert!(text.contains("-1 old text"), "removed diff line: {text:?}");
    assert!(text.contains("+1 new text"), "added diff line: {text:?}");
    // The raw success message is NOT shown — the diff is the surface (edit.ts:390-431).
    assert!(!text.contains("Successfully replaced"), "success text suppressed: {text:?}");
}

#[test]
fn edit_error_shows_error_text_not_diff() {
    let mut view = TranscriptView::new();
    view.push_tool_start("edit", json!({ "path": "a.txt" }));
    view.push_tool_end("edit", true, Some(text_result("permission denied", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("edit a.txt"), "edit header still shows: {text:?}");
    assert!(text.contains("permission denied"), "error body shown: {text:?}");
    assert!(!text.contains('✗'), "no cross glyph — state is the bg tint: {text:?}");
}

#[test]
fn grep_call_shows_pattern_and_path() {
    let mut view = TranscriptView::new();
    view.push_tool_start("grep", json!({ "pattern": "foo", "path": "src", "limit": 50 }));
    view.push_tool_end(
        "grep",
        false,
        Some(text_result("src/a.rs:1:foo()\nsrc/b.rs:2:foo!", json!(null))),
    );
    let text = render(&mut view, 70, 10);
    // `grep /<pattern>/ in <path>` + ` limit N` (grep.ts:68-86).
    assert!(text.contains("grep /foo/ in src"), "grep header: {text:?}");
    assert!(text.contains("limit 50"), "limit suffix: {text:?}");
    assert!(text.contains("src/a.rs:1:foo()"), "match line shown: {text:?}");
}

#[test]
fn list_tool_limit_survives_a_float_spelling() {
    // `JSON.parse` gives the same double for `50` and `50.0`, and each of the three headers
    // interpolates it with a template literal (`formatGrepCall` grep.ts:89, `formatFindCall`
    // find.ts:85, `formatLsCall` ls.ts:62), so both spellings render. `Value::as_i64` answered
    // `None` for the float and dropped the whole suffix.
    let mut view = TranscriptView::new();
    view.push_tool_start("grep", json!({ "pattern": "foo", "path": "src", "limit": 50.0 }));
    view.push_tool_end("grep", false, Some(text_result("src/a.rs:1:foo()", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("limit 50"), "grep float limit renders as `limit 50`: {text:?}");
    assert!(!text.contains("limit 50.0"), "no Rust `Debug` float spelling: {text:?}");

    let mut view = TranscriptView::new();
    view.push_tool_start("find", json!({ "pattern": "*.rs", "path": "src", "limit": 20.0 }));
    view.push_tool_end("find", false, Some(text_result("src/a.rs", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("(limit 20)"), "find float limit: {text:?}");

    let mut view = TranscriptView::new();
    view.push_tool_start("ls", json!({ "path": "src", "limit": 10.0 }));
    view.push_tool_end("ls", false, Some(text_result("a.rs", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("(limit 10)"), "ls float limit: {text:?}");
}

#[test]
fn list_tool_limit_zero_still_renders() {
    // `if (limit !== undefined)` is a PRESENCE test in grep.ts/find.ts/ls.ts, unlike
    // `formatShellCall`'s truthiness test on `timeout` — so `limit: 0` reaches the header.
    let mut view = TranscriptView::new();
    view.push_tool_start("grep", json!({ "pattern": "foo", "path": "src", "limit": 0 }));
    view.push_tool_end("grep", false, Some(text_result("", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("limit 0"), "presence test, not truthiness: {text:?}");

    let mut view = TranscriptView::new();
    view.push_tool_start("ls", json!({ "path": "src", "limit": 0 }));
    view.push_tool_end("ls", false, Some(text_result("", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("(limit 0)"), "ls presence test: {text:?}");

    // `0.0` is the same double as `0` after `JSON.parse`, and `String(-0) === "0"` — so the float
    // and negative-zero spellings render identically, with no `-0` and no `0.0`.
    let mut view = TranscriptView::new();
    view.push_tool_start("find", json!({ "pattern": "*.rs", "path": "src", "limit": -0.0 }));
    view.push_tool_end("find", false, Some(text_result("", json!(null))));
    let text = render(&mut view, 70, 8);
    assert!(text.contains("(limit 0)"), "find negative-zero float limit: {text:?}");
    assert!(!text.contains("(limit -0"), "`String(-0)` is `0`, never `-0`: {text:?}");
}

#[test]
fn ls_call_defaults_path_to_dot() {
    let mut view = TranscriptView::new();
    view.push_tool_start("ls", json!({}));
    view.push_tool_end("ls", false, Some(text_result("Cargo.toml\nsrc/", json!(null))));
    let text = render(&mut view, 60, 8);
    // `ls .` (renderToolPath emptyFallback ".", ls.ts:54).
    assert!(text.contains("ls ."), "ls defaults to `.`: {text:?}");
    assert!(text.contains("Cargo.toml"), "entries shown: {text:?}");
    assert!(text.contains("src/"), "dir entry shown: {text:?}");
}

#[test]
fn commit_moves_live_tools_to_scrollback() {
    let mut view = TranscriptView::new();
    view.push_tool_start("read", json!({ "path": "x" }));
    view.push_tool_end("read", false, None);
    assert_eq!(view.active_tools().len(), 1);
    view.commit_tools();
    assert_eq!(view.active_tools().len(), 0, "committed tools leave the live set");
    assert_eq!(view.pending().len(), 1, "committed as a scrollback entry");
}
