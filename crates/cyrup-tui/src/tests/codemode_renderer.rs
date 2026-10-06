//! CODE-011 — the `codemode` tool's own renderer, drawn through the real tool-row path.
//!
//! Upstream's `codemode-renderer.test.ts` (`ca-test/`, @v1.0.1) calls `renderResult` with a theme
//! and a width and strips the colours. Here the same four cases drive the production seam instead:
//! a [`cyrup_codemode_runtime::CodemodeExtension`] is loaded into an [`ExtensionHost`], session
//! events go through [`App::ingest_event_with_extensions`], and the assertions read the rendered
//! cells — the tool row's shell, padding and wrapping included, which is why every expectation is
//! taken after dropping the `codemode` title row the call side draws above the result.
//!
//! What this adds over the upstream file is what only the draw path can show: the nested-call list
//! growing as partial results stream in, the glyph colours, the collapsed script preview and the
//! expand toggle, and a resumed session drawing the same list.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;

use crate::{App, UiTheme};
use cyrup_codemode_runtime::CodemodeExtension;
use cyrup_codemode_runtime::tool::{CodemodeHostSlot, UnavailableSandboxFactory};
use cyrup_core::{
    ApiId, AssistantMessage, Content, Message, ProviderId, StopReason, ToolCall, ToolCallId,
};
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};
use cyrup_session_svc::agent_message::AgentMessage;
use cyrup_session_svc::{AgentSessionEvent, ReplayItem};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use serde_json::{Value, json};

const HEADER: &str = "Script completed\nWall time 0.1 seconds\nOutput:\n";

async fn host() -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: std::path::PathBuf::from("."),
    }));
    let extension =
        CodemodeExtension::new(CodemodeHostSlot::new(), Arc::new(UnavailableSandboxFactory));
    host.load_native(Arc::new(extension)).await.unwrap();
    host
}

fn app(width: u16, height: u16) -> App<TestBackend> {
    App::new(TestBackend::new(width, height), UiTheme::dark()).unwrap()
}

fn id() -> ToolCallId {
    ToolCallId::from("call-1")
}

fn start(code: &str) -> AgentSessionEvent {
    AgentSessionEvent::ToolExecutionStart {
        tool_call_id: id(),
        tool_name: "codemode".into(),
        args: json!({ "code": code }),
    }
}

fn update(calls: Value) -> AgentSessionEvent {
    AgentSessionEvent::ToolExecutionUpdate {
        tool_call_id: id(),
        tool_name: "codemode".into(),
        args: json!({ "code": "" }),
        partial_result: json!({ "content": [], "details": { "calls": calls } }),
    }
}

fn end(is_error: bool, result: Value) -> AgentSessionEvent {
    AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: id(),
        tool_name: "codemode".into(),
        is_error,
        result,
    }
}

/// Upstream's `render(result, isError, expanded, width)`, through the tool row: the `codemode`
/// title the call draws is dropped, the shell's padding column is stripped, and the rest is
/// trimmed exactly as the upstream helper trims.
///
/// `width` is the component width: the default shell is a `Box(1, 1)`, so the terminal is two
/// columns wider. A collapsed render is the default; `expanded` flips `Ctrl+O` first.
async fn render(result: Value, is_error: bool, expanded: bool, width: u16) -> String {
    let host = host().await;
    let mut app = app(width + 2, 40);
    if expanded {
        app.transcript_mut().set_tool_expanded(true);
    }
    app.ingest_event_with_extensions(&start(""), &host).await;
    app.ingest_event_with_extensions(&end(is_error, result), &host)
        .await;
    app.draw().unwrap();
    drop_title(&app.scrollback_text())
}

/// The block's text rows: shell padding stripped, trailing blanks trimmed, blank edges removed.
fn drop_title(scrollback: &str) -> String {
    let rows: Vec<String> = scrollback
        .lines()
        .map(|line| line.strip_prefix(' ').unwrap_or(line).trim_end().to_owned())
        .collect();
    let joined = rows.join("\n");
    let joined = joined.trim();
    joined
        .strip_prefix("codemode")
        .unwrap_or(joined)
        .trim()
        .to_owned()
}

fn cell_text(app: &App<TestBackend>) -> Vec<String> {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map_or(" ", |cell| cell.symbol()))
                .collect::<String>()
                .trim_end()
                .to_owned()
        })
        .collect()
}

/// The foreground of the first cell drawing `glyph`.
fn glyph_fg(app: &App<TestBackend>, glyph: &str) -> Option<Color> {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.height)
        .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
        .find_map(|(x, y)| {
            let cell = buf.cell((x, y))?;
            (cell.symbol() == glyph).then_some(cell.fg)
        })
}

// ---------------------------------------------------------------------------------------------
// `codemode-renderer.test.ts`, case for case.
// ---------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hides_the_script_header_and_shows_the_output() {
    let text = render(
        json!({
            "content": [
                { "type": "text", "text": HEADER },
                { "type": "text", "text": "hello" },
            ],
            "details": { "calls": [
                { "id": "call/1", "name": "read", "args": "{\"path\":\"a\"}", "status": "ok", "durationMs": 5 }
            ] },
        }),
        false,
        true,
        200,
    )
    .await;
    assert_eq!(text, "✓ read {\"path\":\"a\"} 5ms\n\nhello");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shows_the_cost_of_model_calls_and_their_total() {
    let call = |n: u32, cost: Option<f64>| {
        let mut call = json!({
            "id": format!("call/models.classify/{n}"),
            "name": "models.classify",
            "args": "scorer/judge",
            "status": "ok",
            "durationMs": 5,
        });
        if let Some(cost) = cost {
            call["cost"] = json!(cost);
        }
        call
    };
    let text = render(
        json!({
            "content": [{ "type": "text", "text": HEADER }],
            "details": { "calls": [
                call(1, Some(0.000_012_936)),
                call(2, Some(0.02)),
                call(3, None),
            ] },
        }),
        false,
        true,
        200,
    )
    .await;
    assert_eq!(
        text,
        [
            "✓ models.classify scorer/judge 5ms $0.000013",
            "✓ models.classify scorer/judge 5ms $0.02",
            "✓ models.classify scorer/judge 5ms",
            "Model calls: $0.02",
        ]
        .join("\n")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shows_results_without_a_header_such_as_rejected_options() {
    let text = render(
        json!({ "content": [
            { "type": "text", "text": "The @options line must be followed by JavaScript source" }
        ] }),
        true,
        true,
        200,
    )
    .await;
    assert_eq!(
        text,
        "The @options line must be followed by JavaScript source"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn limits_collapsed_output_to_wrapped_lines_not_logical_lines() {
    let text = render(
        json!({
            "content": [
                { "type": "text", "text": HEADER },
                { "type": "text", "text": "x".repeat(1000) },
            ],
            "details": { "calls": [], "fullOutputPath": "/tmp/out.txt" },
        }),
        false,
        false,
        50,
    )
    .await;
    let lines: Vec<&str> = text.split('\n').collect();
    assert_eq!(lines.len(), 7, "{text}");
    assert_eq!(lines[..5], vec!["x".repeat(50); 5][..], "{text}");
    assert!(lines[5].starts_with("... (15 more lines,"), "{}", lines[5]);
    assert_eq!(lines[6], "Full output: /tmp/out.txt");
}

// ---------------------------------------------------------------------------------------------
// What only the draw path shows.
// ---------------------------------------------------------------------------------------------

/// A nested call list that grows as the recorder publishes partial results, drawn while the
/// tool row is still live — then replaced by the final result without the partial's rows
/// lingering.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_calls_update_live_as_partial_results_stream_in() {
    let host = host().await;
    let mut app = app(100, 30);
    app.ingest_event_with_extensions(&start("const a = 1;"), &host)
        .await;
    app.draw().unwrap();
    assert!(
        !cell_text(&app).iter().any(|row| row.contains("read")),
        "no nested call has been reported yet"
    );

    let running = json!({ "id": "call-1/1", "name": "read", "args": "{\"path\":\"a\"}", "status": "running" });
    app.ingest_event_with_extensions(&update(json!([running])), &host)
        .await;
    app.draw().unwrap();
    let rows = cell_text(&app);
    assert!(
        rows.iter()
            .any(|row| row.contains("… read {\"path\":\"a\"}")),
        "the running call shows its `…` glyph:\n{}",
        rows.join("\n")
    );
    let theme = UiTheme::dark();
    assert_eq!(
        glyph_fg(&app, "…"),
        theme.warning_style().fg,
        "a running call's glyph is `warning`"
    );

    let done = json!({
        "id": "call-1/1", "name": "read", "args": "{\"path\":\"a\"}",
        "status": "ok", "durationMs": 1250,
    });
    let second = json!({ "id": "call-1/2", "name": "grep", "args": "{}", "status": "running" });
    app.ingest_event_with_extensions(&update(json!([done.clone(), second])), &host)
        .await;
    app.draw().unwrap();
    let rows = cell_text(&app);
    assert!(
        rows.iter()
            .any(|row| row.contains("✓ read {\"path\":\"a\"} 1.3s")),
        "the first call finished and shows its duration:\n{}",
        rows.join("\n")
    );
    assert!(
        rows.iter().any(|row| row.contains("… grep {}")),
        "the second call is running:\n{}",
        rows.join("\n")
    );
    assert_eq!(glyph_fg(&app, "✓"), theme.success_style().fg);

    app.ingest_event_with_extensions(
        &end(
            false,
            json!({
                "content": [{ "type": "text", "text": HEADER }, { "type": "text", "text": "done" }],
                "details": { "calls": [
                    done,
                    { "id": "call-1/2", "name": "grep", "args": "{}", "status": "cancelled" },
                ] },
            }),
        ),
        &host,
    )
    .await;
    app.draw().unwrap();
    let text = app.scrollback_text();
    assert!(
        text.contains("⊘ grep {}"),
        "the final result replaced the partial list:\n{text}"
    );
    assert!(
        !text.contains("… grep"),
        "no running row survives the final result:\n{text}"
    );
    assert!(
        text.contains("done"),
        "the output follows the list:\n{text}"
    );
}

/// `statusIcon`'s four glyphs in their four roles, and an error that shows only when expanded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_status_glyph_draws_in_its_role_colour_and_errors_show_when_expanded() {
    let calls = json!([
        { "id": "c/1", "name": "a", "args": "", "status": "running" },
        { "id": "c/2", "name": "b", "args": "", "status": "ok" },
        { "id": "c/3", "name": "c", "args": "", "status": "error", "error": "boom\nsecond" },
        { "id": "c/4", "name": "d", "args": "", "status": "cancelled" },
    ]);
    let host = host().await;
    let mut app = app(100, 30);
    app.ingest_event_with_extensions(&start(""), &host).await;
    app.ingest_event_with_extensions(&update(calls), &host)
        .await;
    app.draw().unwrap();
    let theme = UiTheme::dark();
    assert_eq!(glyph_fg(&app, "…"), theme.warning_style().fg);
    assert_eq!(glyph_fg(&app, "✓"), theme.success_style().fg);
    assert_eq!(glyph_fg(&app, "✗"), theme.error_style().fg);
    assert_eq!(glyph_fg(&app, "⊘"), theme.muted_style().fg);
    assert!(
        !cell_text(&app).iter().any(|row| row.contains("boom")),
        "collapsed, the error text is hidden"
    );

    app.transcript_mut().set_tool_expanded(true);
    app.draw().unwrap();
    let rows = cell_text(&app);
    let at = rows.iter().position(|row| row.contains("✗ c")).unwrap();
    assert_eq!(rows[at + 1].trim(), "boom");
    assert_eq!(rows[at + 2].trim(), "second");
    assert!(
        rows[at + 1].starts_with("     boom"),
        "the error is indented four columns under the call (plus the shell's padding): {:?}",
        rows[at + 1]
    );
}

/// The call row: the script collapsed to ten visual lines with the expand hint, whole when
/// expanded, drawn by the same component on the live row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_call_shows_the_script_collapsed_to_ten_lines_until_expanded() {
    let host = host().await;
    let mut app = app(100, 70);
    let script: String = (1..=40).map(|n| format!("const v{n} = {n};\n")).collect();
    app.ingest_event_with_extensions(&start(&script), &host)
        .await;
    app.draw().unwrap();
    let rows = cell_text(&app);
    assert!(
        rows.iter().any(|row| row.contains("codemode")),
        "the title row"
    );
    assert!(rows.iter().any(|row| row.contains("const v10 = 10;")));
    assert!(
        !rows.iter().any(|row| row.contains("const v11 = 11;")),
        "only ten lines show collapsed:\n{}",
        rows.join("\n")
    );
    assert!(
        rows.iter()
            .any(|row| row.contains("... (30 more lines, ctrl+o to expand)")),
        "the hint counts what is hidden:\n{}",
        rows.join("\n")
    );

    app.transcript_mut().set_tool_expanded(true);
    app.draw().unwrap();
    let rows = cell_text(&app);
    assert!(rows.iter().any(|row| row.contains("const v40 = 40;")));
    assert!(
        !rows.iter().any(|row| row.contains("more lines")),
        "expanded, nothing is hidden and no hint shows"
    );
}

/// A non-string `code` is reported where the script would be.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_non_string_script_is_an_invalid_arg() {
    let host = host().await;
    let mut app = app(100, 12);
    app.ingest_event_with_extensions(
        &AgentSessionEvent::ToolExecutionStart {
            tool_call_id: id(),
            tool_name: "codemode".into(),
            args: json!({ "code": 7 }),
        },
        &host,
    )
    .await;
    app.draw().unwrap();
    let rows = cell_text(&app);
    assert!(
        rows.iter()
            .any(|row| row.contains("codemode [invalid arg]")),
        "{}",
        rows.join("\n")
    );
}

/// A resumed session draws the same list from the persisted result's `details`
/// (`renderedPendingTools` replay, `interactive-mode.ts:3729-3775`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_resumed_session_replays_the_nested_call_list() {
    let host = host().await;
    let mut app = app(100, 30);
    let mut assistant = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        StopReason::Stop,
        String::new(),
    );
    assistant.error_message = None;
    assistant.content = vec![Content::ToolCall(ToolCall {
        id: id(),
        name: "codemode".into(),
        arguments: json!({ "code": "await tools.read({})" })
            .as_object()
            .cloned()
            .unwrap()
            .into(),
        thought_signature: None,
        namespace: None,
    })];
    let result = AgentMessage::Core(Message::ToolResult {
        tool_call_id: id(),
        tool_name: "codemode".into(),
        content: vec![
            Content::Text {
                text: HEADER.into(),
                text_signature: None,
            },
            Content::Text {
                text: "replayed output".into(),
                text_signature: None,
            },
        ],
        is_error: false,
        details: Some(json!({ "calls": [
            { "id": "call-1/1", "name": "read", "args": "{}", "status": "error", "durationMs": 7, "error": "nope" }
        ] })),
        timestamp: 0,
        usage: None,
        added_tool_names: Vec::new(),
        nested_calls: None,
    });
    app.replay_items_with_extensions(
        &[
            ReplayItem::Message(Box::new(AgentMessage::Core(Message::Assistant(assistant)))),
            ReplayItem::Message(Box::new(result)),
        ],
        &host,
    )
    .await;
    app.draw().unwrap();
    let text = app.scrollback_text();
    assert!(
        text.contains("await tools.read({})"),
        "the replayed call shows the script:\n{text}"
    );
    assert!(
        text.contains("✗ read {} 7ms"),
        "the replayed result lists the call:\n{text}"
    );
    assert!(
        text.contains("replayed output"),
        "and its output, without the header:\n{text}"
    );
    assert!(!text.contains("Wall time"), "{text}");
}

/// The foreground of the first scrollback span that contains `needle`.
fn scrollback_fg(app: &App<TestBackend>, needle: &str) -> Option<Color> {
    app.state()
        .scrollback
        .iter()
        .flat_map(|line| line.spans.iter())
        .find(|span| span.content.contains(needle))
        .and_then(|span| span.style.fg)
}

/// `context.isError` picks the output colour (`renderer.ts:133`): a failed script's text is drawn
/// in `error`, a completed one's in `toolOutput`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_result_draws_its_output_in_the_error_colour() {
    let theme = UiTheme::dark();
    let result = json!({ "content": [
        { "type": "text", "text": HEADER },
        { "type": "text", "text": "failure-text" },
    ] });
    for (is_error, expected) in [
        (true, theme.error_style().fg),
        (false, theme.tool_output_style().fg),
    ] {
        let host = host().await;
        let mut app = app(100, 20);
        app.ingest_event_with_extensions(&start(""), &host).await;
        app.ingest_event_with_extensions(&end(is_error, result.clone()), &host)
            .await;
        app.draw().unwrap();
        assert_eq!(
            scrollback_fg(&app, "failure-text"),
            expected,
            "is_error = {is_error}"
        );
    }
}

/// Collapsed, the list keeps the LAST eight calls behind an `earlier calls` line; the expand
/// toggle shows every call (`CALL_PREVIEW_COUNT`, `renderer.ts:18`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_last_eight_calls_show_until_the_row_is_expanded() {
    let calls: Vec<Value> = (1..=10)
        .map(|n| json!({ "id": format!("c/{n}"), "name": format!("tool{n:02}"), "args": "", "status": "ok" }))
        .collect();
    let host = host().await;
    let mut app = app(100, 40);
    app.ingest_event_with_extensions(&start(""), &host).await;
    app.ingest_event_with_extensions(&update(json!(calls)), &host)
        .await;
    app.draw().unwrap();
    let rows = cell_text(&app).join("\n");
    assert!(
        rows.contains("... (2 earlier calls, ctrl+o to expand)"),
        "{rows}"
    );
    assert!(
        !rows.contains("tool01") && !rows.contains("tool02"),
        "{rows}"
    );
    assert!(rows.contains("tool03") && rows.contains("tool10"), "{rows}");

    app.transcript_mut().set_tool_expanded(true);
    app.draw().unwrap();
    let rows = cell_text(&app).join("\n");
    assert!(rows.contains("tool01") && rows.contains("tool10"), "{rows}");
    assert!(!rows.contains("earlier calls"), "{rows}");
}
