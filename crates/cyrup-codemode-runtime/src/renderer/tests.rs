//! `renderer.ts`'s formatting helpers and component trees, below the layout: the strings a tree
//! carries, with every role visible as `⟦role:text⟧`. What the user sees through the real draw path
//! (`codemode-renderer.test.ts`) is asserted in `cyrup-tui`.

use super::*;
use serde_json::json;

/// A theme that tags every styled run with its role instead of colouring it.
struct Tags;

impl RenderTheme for Tags {
    fn fg(&self, role: &str, text: &str) -> String {
        format!("⟦{role}:{text}⟧")
    }
    fn bold(&self, text: &str) -> String {
        format!("⟦bold:{text}⟧")
    }
    fn key_hint(&self, binding: &str, description: &str) -> String {
        format!("⟦key:{binding}|{description}⟧")
    }
    fn highlight_code(&self, code: &str, lang: &str) -> Vec<String> {
        code.split('\n')
            .map(|line| format!("⟦{lang}:{line}⟧"))
            .collect()
    }
}

fn call(status: CodemodeNestedCallStatus) -> CodemodeNestedCall {
    CodemodeNestedCall {
        id: "call/1".into(),
        name: "read".into(),
        args: r#"{"path":"a"}"#.into(),
        status,
        duration_ms: Some(5.0),
        error: None,
        cost: None,
    }
}

/// The rows of a tree with no wrapping and no truncation: a preview contributes its whole text.
fn flatten(node: &RenderNode<'_>, out: &mut Vec<String>) {
    match node {
        RenderNode::Text(text) => out.extend(text.split('\n').map(str::to_owned)),
        RenderNode::Spacer(rows) => out.extend(std::iter::repeat_n(String::new(), *rows)),
        RenderNode::Container(children) => {
            for child in children {
                flatten(child, out);
            }
        }
        RenderNode::VisualLinePreview { text, .. } => {
            out.extend(text.split('\n').map(str::to_owned));
        }
    }
}

fn rows(tree: &Arc<dyn RenderedTree>, expanded: bool) -> Vec<String> {
    let ctx = TreeCtx {
        expanded,
        theme: &Tags,
    };
    let mut out = Vec::new();
    flatten(&tree.tree(&ctx), &mut out);
    out
}

fn preview_hint(
    tree: &Arc<dyn RenderedTree>,
    hidden: usize,
) -> Option<(usize, PreviewKeep, String)> {
    fn find<'a, 'b>(node: &'b RenderNode<'a>) -> Option<&'b RenderNode<'a>> {
        match node {
            RenderNode::VisualLinePreview { .. } => Some(node),
            RenderNode::Container(children) => children.iter().find_map(find),
            _ => None,
        }
    }
    let ctx = TreeCtx {
        expanded: false,
        theme: &Tags,
    };
    let node = tree.tree(&ctx);
    match find(&node) {
        Some(RenderNode::VisualLinePreview {
            max_visual_lines,
            keep,
            hint,
            ..
        }) => Some((*max_visual_lines, *keep, hint(hidden))),
        _ => None,
    }
}

#[test]
fn format_duration_rounds_milliseconds_and_shows_tenths_of_a_second() {
    assert_eq!(format_duration(None), "");
    assert_eq!(format_duration(Some(0.0)), "0ms");
    assert_eq!(format_duration(Some(0.4)), "0ms");
    // `Math.round` sends a half up; `f64::round` would too, but a negative half differs.
    assert_eq!(format_duration(Some(2.5)), "3ms");
    assert_eq!(format_duration(Some(-0.5)), "0ms");
    assert_eq!(format_duration(Some(999.4)), "999ms");
    // `Math.round(999.5)` is 1000, still under the `ms < 1000` branch's input but printed in ms.
    assert_eq!(format_duration(Some(999.5)), "1000ms");
    assert_eq!(format_duration(Some(1000.0)), "1.0s");
    assert_eq!(format_duration(Some(1234.0)), "1.2s");
    // `toFixed` rounds the exact value half up: 1.25 is exactly representable, so `1.3`, where
    // Rust's `{:.1}` says `1.2`.
    assert_eq!(format_duration(Some(1250.0)), "1.3s");
    assert_eq!(format_duration(Some(61_000.0)), "61.0s");
}

#[test]
fn format_cost_uses_cents_above_a_cent_and_two_significant_digits_below() {
    assert_eq!(format_cost(0.02), "$0.02");
    assert_eq!(format_cost(0.01), "$0.01");
    assert_eq!(format_cost(1.5), "$1.50");
    assert_eq!(format_cost(12.0), "$12.00");
    // `toFixed(2)` of an exactly representable tie rounds up.
    assert_eq!(format_cost(0.125), "$0.13");
    assert_eq!(format_cost(0.000_012_936), "$0.000013");
    assert_eq!(format_cost(0.009), "$0.0090");
    // Rounding up into the next power of ten moves the exponent: 0.00996 → 0.010.
    assert_eq!(format_cost(0.00996), "$0.010");
    // `toPrecision` leaves fixed notation below 1e-6.
    assert_eq!(format_cost(0.000_000_5), "$5.0e-7");
    assert_eq!(format_cost(0.000_000_03), "$3.0e-8");
}

#[test]
fn each_status_has_its_glyph_and_role() {
    let theme: &dyn RenderTheme = &Tags;
    let glyph = |status| {
        format_call(&call(status), theme, false)
            .split(' ')
            .next()
            .map(str::to_owned)
    };
    assert_eq!(
        glyph(CodemodeNestedCallStatus::Running).as_deref(),
        Some("⟦warning:…⟧")
    );
    assert_eq!(
        glyph(CodemodeNestedCallStatus::Ok).as_deref(),
        Some("⟦success:✓⟧")
    );
    assert_eq!(
        glyph(CodemodeNestedCallStatus::Error).as_deref(),
        Some("⟦error:✗⟧")
    );
    assert_eq!(
        glyph(CodemodeNestedCallStatus::Cancelled).as_deref(),
        Some("⟦muted:⊘⟧")
    );
}

#[test]
fn a_call_line_is_glyph_name_args_duration_cost() {
    let mut priced = call(CodemodeNestedCallStatus::Ok);
    priced.name = "models.classify".into();
    priced.cost = Some(0.000_012_936);
    assert_eq!(
        format_call(&priced, &Tags, false),
        r#"⟦success:✓⟧ ⟦toolTitle:models.classify⟧ ⟦muted:{"path":"a"}⟧ ⟦dim:5ms⟧ ⟦dim:$0.000013⟧"#
    );
    // A running call has no duration yet, and a call with no args shows none.
    let mut running = call(CodemodeNestedCallStatus::Running);
    running.duration_ms = None;
    running.args = String::new();
    assert_eq!(
        format_call(&running, &Tags, false),
        "⟦warning:…⟧ ⟦toolTitle:read⟧"
    );
    // A zero cost is falsy upstream and shows nothing.
    let mut free = call(CodemodeNestedCallStatus::Ok);
    free.cost = Some(0.0);
    assert!(!format_call(&free, &Tags, false).contains('$'));
}

#[test]
fn collapsed_args_are_cut_to_eighty_characters_and_expanded_ones_are_whole() {
    let mut long = call(CodemodeNestedCallStatus::Ok);
    long.args = "a".repeat(100);
    let collapsed = format_call(&long, &Tags, false);
    assert!(
        collapsed.contains(&format!("⟦muted:{}...⟧", "a".repeat(77))),
        "{collapsed}"
    );
    let expanded = format_call(&long, &Tags, true);
    assert!(
        expanded.contains(&format!("⟦muted:{}⟧", "a".repeat(100))),
        "{expanded}"
    );
    // Exactly 80 is not cut.
    long.args = "a".repeat(80);
    assert!(format_call(&long, &Tags, false).contains(&format!("⟦muted:{}⟧", "a".repeat(80))));
}

#[test]
fn an_error_shows_only_when_expanded_and_is_indented_under_its_call() {
    let mut failed = call(CodemodeNestedCallStatus::Error);
    failed.error = Some("boom\nsecond line".into());
    assert!(!format_call(&failed, &Tags, false).contains("boom"));
    assert_eq!(
        format_call(&failed, &Tags, true),
        "⟦error:✗⟧ ⟦toolTitle:read⟧ ⟦muted:{\"path\":\"a\"}⟧ ⟦dim:5ms⟧\n    ⟦error:boom\n    second line⟧"
    );
}

#[test]
fn the_script_header_is_recognised_only_in_full() {
    assert!(is_script_header(
        "Script completed\nWall time 0.1 seconds\nOutput:\n"
    ));
    assert!(is_script_header(
        "Script failed\nWall time 12.34 seconds\nOutput:\n"
    ));
    assert!(!is_script_header(
        "Script completed\nWall time  seconds\nOutput:\n"
    ));
    assert!(!is_script_header(
        "Script completed\nWall time 0.1 seconds\nOutput:\nx"
    ));
    assert!(!is_script_header(
        "Script completed\nWall time 0.1 seconds\nOutput:"
    ));
    assert!(!is_script_header(
        "Script started\nWall time 0.1 seconds\nOutput:\n"
    ));
    assert!(!is_script_header(
        "Script completed\nWall time 1e3 seconds\nOutput:\n"
    ));
}

#[test]
fn only_the_last_eight_calls_show_collapsed_with_an_earlier_calls_line() {
    let calls: Vec<Value> = (0..10)
        .map(|n| json!({"id": format!("c/{n}"), "name": format!("t{n}"), "args": "", "status": "ok"}))
        .collect();
    let tree = render_result(
        &json!({"content": [], "details": {"calls": calls}}),
        &RenderOptions::default(),
    );
    let collapsed = rows(&tree, false);
    assert_eq!(
        collapsed.first().map(String::as_str),
        Some(""),
        "a spacer opens the list"
    );
    assert_eq!(
        collapsed.get(1).map(String::as_str),
        Some("⟦muted:... (2 earlier calls,⟧ ⟦key:app.tools.expand|to expand⟧⟦muted:)⟧")
    );
    assert_eq!(collapsed.len(), 1 + 1 + 8);
    assert!(collapsed.iter().any(|row| row.contains("t9")));
    assert!(
        !collapsed
            .iter()
            .any(|row| row.contains("t1 ") || row.ends_with("t1⟧"))
    );
    let expanded = rows(&tree, true);
    assert_eq!(
        expanded.len(),
        1 + 10,
        "every call, and no earlier-calls line"
    );
}

#[test]
fn the_model_calls_total_covers_every_call_not_only_the_shown_ones() {
    let mut calls: Vec<Value> = (0..9)
        .map(|n| json!({"id": format!("c/{n}"), "name": "models.classify", "args": "", "status": "ok", "cost": 0.5}))
        .collect();
    calls.push(json!({"id": "c/9", "name": "read", "args": "", "status": "ok"}));
    let tree = render_result(
        &json!({"content": [], "details": {"calls": calls}}),
        &RenderOptions::default(),
    );
    assert_eq!(
        rows(&tree, false).last().map(String::as_str),
        Some("⟦muted:Model calls: $4.50⟧")
    );
    // One priced call is not a total.
    let single = render_result(
        &json!({"content": [], "details": {"calls": [
            {"id": "c/0", "name": "models.classify", "args": "", "status": "ok", "cost": 0.5}
        ]}}),
        &RenderOptions::default(),
    );
    assert!(
        !rows(&single, false)
            .iter()
            .any(|row| row.contains("Model calls"))
    );
}

#[test]
fn the_output_follows_the_calls_without_the_script_header() {
    let result = json!({
        "content": [
            {"type": "text", "text": "Script completed\nWall time 0.1 seconds\nOutput:\n"},
            {"type": "text", "text": "  hello\tworld  \n"},
        ],
        "details": {"calls": []},
    });
    let tree = render_result(&result, &RenderOptions::default());
    assert_eq!(rows(&tree, true), vec!["", "⟦toolOutput:hello   world⟧"]);
    let errored = render_result(&result, &RenderOptions::default().errored(true));
    assert_eq!(rows(&errored, true), vec!["", "⟦error:hello   world⟧"]);
    let partial = render_result(&result, &RenderOptions::default().partial(true));
    assert_eq!(
        rows(&partial, true),
        Vec::<String>::new(),
        "no output while it runs"
    );
}

#[test]
fn a_result_without_a_header_keeps_its_first_block() {
    let tree = render_result(
        &json!({"content": [{"type": "text", "text": "The @options line must be followed by JavaScript source"}]}),
        &RenderOptions::default().errored(true),
    );
    assert_eq!(
        rows(&tree, true),
        vec![
            "",
            "⟦error:The @options line must be followed by JavaScript source⟧"
        ]
    );
}

#[test]
fn collapsed_output_is_a_five_line_preview_and_names_the_full_output_file() {
    let result = json!({
        "content": [{"type": "text", "text": "x"}],
        "details": {"calls": [], "fullOutputPath": "/tmp/out.txt"},
    });
    let tree = render_result(&result, &RenderOptions::default());
    assert_eq!(
        preview_hint(&tree, 15),
        Some((
            5,
            PreviewKeep::Start,
            "⟦muted:... (15 more lines,⟧ ⟦key:app.tools.expand|to expand⟧⟦muted:)⟧".to_owned()
        ))
    );
    assert_eq!(
        rows(&tree, false).last().map(String::as_str),
        Some("⟦muted:Full output: /tmp/out.txt⟧")
    );
    // Expanded, the notice at the end of the output itself is visible, so no file line.
    assert!(
        !rows(&tree, true)
            .iter()
            .any(|row| row.contains("Full output"))
    );
}

#[test]
fn the_call_shows_the_script_highlighted_and_collapsed_to_ten_lines() {
    let tree = render_call(&json!({"code": "// @options: {}\r\nconst a = 1;\t\n"}));
    assert_eq!(
        rows(&tree, true),
        vec![
            "⟦toolTitle:⟦bold:codemode⟧⟧",
            "⟦javascript:// @options: {}⟧",
            "⟦javascript:const a = 1;⟧",
        ]
    );
    assert_eq!(
        preview_hint(&tree, 30),
        Some((
            10,
            PreviewKeep::Start,
            "⟦muted:... (30 more lines,⟧ ⟦key:app.tools.expand|to expand⟧⟦muted:)⟧".to_owned()
        ))
    );
}

#[test]
fn a_call_without_code_is_just_the_title_and_a_non_string_is_an_invalid_arg() {
    for args in [json!({}), json!({"code": null}), json!({"code": ""})] {
        assert_eq!(
            rows(&render_call(&args), false),
            vec!["⟦toolTitle:⟦bold:codemode⟧⟧"]
        );
    }
    assert_eq!(
        rows(&render_call(&json!({"code": 7})), false),
        vec!["⟦toolTitle:⟦bold:codemode⟧⟧ ⟦error:[invalid arg]⟧"]
    );
}
