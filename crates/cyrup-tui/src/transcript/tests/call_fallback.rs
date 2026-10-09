#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! TUI-138: the call fallback (`createCallFallback`, `components/tool-execution.ts:142-144`
//! @ce950d78f) shows the tool's arguments through `formatToolCallWithArgs`
//! (`core/tools/render-utils.ts:78-97`) — a collapsed `key=value` preview on the title line and an
//! expanded `  key: value` block — rather than the bare bold name.

use crate::theme::UiTheme;
use crate::transcript::tool_builtin::{
    format_tool_call_with_args, js_json_stringify, render_call_fallback, truncate_utf16_preview,
};
use crate::transcript::tool_render::{ImageOpts, tool_block};
use crate::transcript::*;
use cyrup_core::ToolRenderKind;
use ratatui::text::Line;
use serde_json::{Value, json};

fn render(args: &Value, expanded: bool) -> Vec<Line<'static>> {
    let mut out = Vec::new();
    format_tool_call_with_args(
        "mcp__github__get",
        args,
        expanded,
        &UiTheme::dark(),
        &mut out,
    );
    out
}

fn text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn texts(lines: &[Line<'_>]) -> Vec<String> {
    lines.iter().map(text).collect()
}

/// The preview half of a collapsed header line: everything after `title + " "`.
fn preview(line: &Line<'_>) -> String {
    text(line)
        .strip_prefix("mcp__github__get ")
        .expect("collapsed header starts with the title and a space")
        .to_owned()
}

#[test]
fn collapsed_object_renders_key_value_pairs_after_the_name() {
    let theme = UiTheme::dark();
    let lines = render(&json!({ "owner": "a", "repo": "b" }), false);
    assert_eq!(
        texts(&lines),
        vec![r#"mcp__github__get owner="a" repo="b""#]
    );
    // Title span is the bold tool title; the separator and the preview are muted.
    let spans = &lines[0].spans;
    assert_eq!(spans[0].content, "mcp__github__get");
    assert_eq!(spans[0].style, theme.tool_title_style());
    assert!(spans[1..].iter().all(|s| s.style == theme.muted_style()));
}

#[test]
fn collapsed_preview_over_budget_is_cut_to_exactly_100_with_an_ellipsis() {
    let long = "x".repeat(300);
    let lines = render(&json!({ "body": long }), false);
    assert_eq!(lines.len(), 1);
    let p = preview(&lines[0]);
    assert!(p.ends_with("..."), "{p}");
    assert_eq!(p.encode_utf16().count(), 100);
    assert!(p.starts_with(r#"body=""#));
    // Exactly at the budget: untouched.
    let exact = format!("k={}", "y".repeat(98));
    assert_eq!(exact.len(), 100);
    assert_eq!(truncate_utf16_preview(&exact, 100), exact);
}

#[test]
fn expanded_puts_each_key_on_its_own_line_and_indents_continuations_four_spaces() {
    let theme = UiTheme::dark();
    let args = json!({
        "title": "Fix it",
        "body": "line one\r\nline\ttwo",
        "labels": { "kind": ["bug", 1.0] },
    });
    let lines = render(&args, true);
    assert_eq!(
        texts(&lines),
        vec![
            "mcp__github__get",
            "  title: Fix it",
            "  body: line one",
            "    line   two",
            "  labels: {",
            "      \"kind\": [",
            "        \"bug\",",
            "        1",
            "      ]",
            "    }",
        ]
    );
    assert_eq!(lines[0].style, Default::default());
    assert_eq!(lines[0].spans[0].style, theme.tool_title_style());
    for l in &lines[1..] {
        assert_eq!(l.style, theme.muted_style());
    }
}

#[test]
fn null_and_empty_args_render_the_bare_name() {
    let theme = UiTheme::dark();
    for args in [Value::Null, json!({})] {
        for expanded in [false, true] {
            let lines = render(&args, expanded);
            assert_eq!(texts(&lines), vec!["mcp__github__get"], "{args} {expanded}");
            assert_eq!(lines[0].spans.len(), 1);
            assert_eq!(lines[0].spans[0].style, theme.tool_title_style());
        }
    }
}

#[test]
fn scalar_and_array_args_become_a_single_args_entry() {
    assert_eq!(
        texts(&render(&json!("q"), false)),
        vec![r#"mcp__github__get args="q""#]
    );
    assert_eq!(
        texts(&render(&json!(42), false)),
        vec!["mcp__github__get args=42"]
    );
    assert_eq!(
        texts(&render(&json!([1, "two"]), false)),
        vec![r#"mcp__github__get args=[1,"two"]"#]
    );
    assert_eq!(
        texts(&render(&json!([]), false)),
        vec!["mcp__github__get args=[]"]
    );
    // Expanded, a string scalar is shown raw and an array pretty-printed.
    assert_eq!(
        texts(&render(&json!("q"), true)),
        vec!["mcp__github__get", "  args: q"]
    );
    assert_eq!(
        texts(&render(&json!([1]), true)),
        vec!["mcp__github__get", "  args: [", "      1", "    ]"]
    );
}

#[test]
fn key_order_is_preserved() {
    let args: Value = serde_json::from_str(r#"{"zeta":1,"alpha":2,"mid":3}"#).unwrap();
    assert_eq!(
        texts(&render(&args, false)),
        vec!["mcp__github__get zeta=1 alpha=2 mid=3"]
    );
    assert_eq!(
        texts(&render(&args, true)),
        vec!["mcp__github__get", "  zeta: 1", "  alpha: 2", "  mid: 3"]
    );
}

#[test]
fn array_index_keys_come_first_ascending_as_object_entries_lists_them() {
    // node: `Object.entries({"2":"b","1":"a","x":1,"01":0,"4294967295":0})` lists `1`, `2`, then
    // `x`, `01` and `4294967295` (not array indices) in insertion order.
    let args: Value =
        serde_json::from_str(r#"{"2":"b","1":"a","x":1,"01":0,"4294967295":0}"#).unwrap();
    assert_eq!(
        texts(&render(&args, false)),
        vec![r#"mcp__github__get 1="a" 2="b" x=1 01=0 4294967295=0"#]
    );
    // Nested objects follow `JSON.stringify`'s same order:
    // `JSON.stringify({a:{"2":1,"1":2}})` is `{"a":{"1":2,"2":1}}`.
    let nested: Value = serde_json::from_str(r#"{"a":{"2":1,"1":2}}"#).unwrap();
    assert_eq!(js_json_stringify(&nested, false), r#"{"a":{"1":2,"2":1}}"#);
}

#[test]
fn multibyte_text_near_the_cut_never_panics_and_counts_utf16_units() {
    // Sweep the padding so every multibyte char lands on and around the cut.
    for pad in 0..12 {
        for ch in ["é", "中", "😀", "e\u{301}"] {
            let s = format!("{}{}", "a".repeat(80 + pad), ch.repeat(20));
            let lines = render(&json!({ "q": s }), false);
            let p = preview(&lines[0]);
            assert!(p.ends_with("..."));
            assert_eq!(p.encode_utf16().count(), 100, "{pad} {ch}: {p}");
        }
    }
    // A cut between a surrogate pair's halves leaves JS a lone high surrogate, which reaches the
    // terminal as U+FFFD; the preview stays 100 code units.
    let s = format!("{}😀😀😀", "a".repeat(96));
    let cut = truncate_utf16_preview(&s, 100);
    assert_eq!(cut, format!("{}\u{fffd}...", "a".repeat(96)));
}

#[test]
fn values_stringify_as_javascript_does() {
    // `JSON.stringify(1.0) === "1"`, where serde writes `1.0`; JS exponent bands.
    assert_eq!(js_json_stringify(&json!(1.0), false), "1");
    assert_eq!(js_json_stringify(&json!(1e21), false), "1e+21");
    assert_eq!(js_json_stringify(&json!(0.5), false), "0.5");
    assert_eq!(
        js_json_stringify(&json!(18_446_744_073_709_551_615_u64), false),
        "18446744073709552000"
    );
    // String escapes: quote, backslash, short forms and lowercase `\u00xx`; non-ASCII raw.
    assert_eq!(
        js_json_stringify(&json!("a\"b\\c\n\t\u{1}é"), false),
        r#""a\"b\\c\n\t\u0001é""#
    );
    assert_eq!(
        js_json_stringify(
            &json!({ "a": [1, { "b": null }], "c": {} , "d": [] }),
            false
        ),
        r#"{"a":[1,{"b":null}],"c":{},"d":[]}"#
    );
    assert_eq!(
        js_json_stringify(&json!({ "a": [true], "c": {} }), true),
        "{\n  \"a\": [\n    true\n  ],\n  \"c\": {}\n}"
    );
}

/// The wiring: a defined tool with no `renderCall` reaches the fallback with the block's
/// expanded state.
#[test]
fn render_call_fallback_threads_args_and_expanded() {
    let mut view = TranscriptView::default();
    view.push_tool_start("tool_search", json!({ "query": "github" }));
    let run = view.active_tools().last().cloned().expect("a run");
    let theme = UiTheme::dark();
    let mut collapsed = Vec::new();
    render_call_fallback(&run, false, &theme, &mut collapsed);
    assert_eq!(texts(&collapsed), vec![r#"tool_search query="github""#]);
    let mut expanded = Vec::new();
    render_call_fallback(&run, true, &theme, &mut expanded);
    assert_eq!(texts(&expanded), vec!["tool_search", "  query: github"]);
}

/// The call site: `tool_block` hands the fallback the block's own expanded state
/// (`tool_render.rs`, `createCallFallback` at `tool-execution.ts:142-144` @ce950d78f), for a
/// defined, non-builtin tool with no rendered call.
#[test]
fn tool_block_passes_its_expanded_state_to_the_call_fallback() {
    let mut view = TranscriptView::default();
    view.push_tool_start_defined(
        "mcp_x",
        Some("id".to_owned()),
        json!({ "key": "v" }),
        None,
        Some(ToolRenderKind::Default),
    );
    let run = view.active_tools().last().cloned().expect("a run");
    let theme = UiTheme::dark();
    let rows = |expanded| -> Vec<String> {
        tool_block(&run, expanded, 80, 1, &theme, ImageOpts::default())
            .lines
            .iter()
            .map(text)
            .collect()
    };
    let collapsed = rows(false);
    assert!(
        collapsed.iter().any(|r| r.contains(r#"mcp_x key="v""#)),
        "{collapsed:#?}"
    );
    assert!(
        !collapsed.iter().any(|r| r.contains("  key: v")),
        "{collapsed:#?}"
    );
    let expanded = rows(true);
    assert!(
        expanded.iter().any(|r| r.contains("  key: v")),
        "{expanded:#?}"
    );
    assert!(
        !expanded.iter().any(|r| r.contains(r#"key="v""#)),
        "{expanded:#?}"
    );
}
