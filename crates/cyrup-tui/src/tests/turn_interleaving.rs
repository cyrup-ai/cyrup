//! One WHOLE turn — user → assistant text → tool → assistant text — driven through the real
//! `App::ingest_event` seam, asserted on ORDER **and on painted cells**.
//!
//! Why this file exists: every other TUI test here checks one component's rows in isolation against
//! one Pi component. None of them rendered an interleaved turn, so two user-visible defects lived
//! in the product under a fully green suite:
//!
//! 1. **Ordering.** `cyrup-agent` `break 'consume`s the instant a stream yields its terminal
//!    (`agent.rs:813-820`), so `StreamEvent::Done` is NEVER re-emitted as a `MessageUpdate` — the
//!    terminal reaches the TUI only as `MessageEnd`, whose arm was empty. Assistant text therefore
//!    never committed until `agent_end`, every step's text concatenated into one block, and
//!    `commit_finished_leading_tools` (which refuses to commit a tool ahead of uncommitted
//!    assistant text, `transcript.rs:865-868`) never fired — so every tool of the turn landed
//!    *after* all of the text. Pi finalizes at `message_end` (`interactive-mode.ts:3180-3216`),
//!    which is what makes its `chatContainer` read
//!    `[AssistantMessage][Tool][Tool][AssistantMessage]` in call order.
//!
//! 2. **The green slab.** `entry_lines` rendered every COMMITTED tool with a hardcoded
//!    `expanded = true`. Pi seeds each `ToolExecutionComponent` from `this.toolOutputExpanded`
//!    (`interactive-mode.ts:3165`, `:3239`, `:3437`, `:3486`, `:3602`), which defaults to **false**
//!    (`:442`) and is only ever changed by `setToolsExpanded`'s broadcast (`:4032-4046`). A
//!    collapsed `read`'s `renderResult` returns `""` upstream (`read.ts:178-180`), so Pi's committed
//!    block is three rows — top pad, header, bottom pad. Forcing `true` dumped the entire file
//!    inside the full-width state-tinted `Box`, so one `read` painted hundreds of rows of solid
//!    tool background over the conversation. On a 256-colour terminal that background is
//!    `Indexed(22)` — a vivid `#005f00` — which is the "hideous solid green" in the report.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::AgentMessage;
use cyrup_core::{AssistantMessage, Content, ProviderId, StopReason};
use cyrup_provider::StreamEvent;
use cyrup_session_svc::AgentSessionEvent;
use crate::{App, ColorMode, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

/// `dark.json:18` `"toolSuccessBg": "#283228"`, resolved through `dark.json:43`.
const DARK_TOOL_SUCCESS_BG: Color = Color::Rgb(0x28, 0x32, 0x28);
/// `dark.json:17` `"toolPendingBg": "#282832"`.
const DARK_TOOL_PENDING_BG: Color = Color::Rgb(0x28, 0x28, 0x32);

fn assistant(text: &str, tool: Option<(&str, &str, serde_json::Value)>) -> AssistantMessage {
    let mut m = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude",
        None,
        StopReason::Stop,
        "",
    );
    let mut content = vec![Content::text(text)];
    if let Some((id, name, args)) = tool {
        let arguments = match args {
            serde_json::Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        content.push(Content::ToolCall(cyrup_core::ToolCall {
            id: cyrup_core::ToolCallId::from(id),
            name: name.to_string(),
            arguments,
            thought_signature: None,
        }));
    }
    m.content = content;
    m
}

/// One assistant step **exactly as `cyrup-agent` emits it**: `MessageStart` on the stream's `Start`
/// frame (`agent.rs:802-808`), text deltas as `MessageUpdate` (`:827-831`), then `MessageEnd` with
/// the authoritative message (`:854`). No `MessageUpdate` ever carries a terminal event — the loop
/// breaks on one (`:813-820`) — which is precisely what this file's first defect turned on.
fn assistant_step(
    app: &mut App<TestBackend>,
    text: &str,
    tool: Option<(&str, &str, serde_json::Value)>,
) {
    let partial = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude",
        None,
        StopReason::Stop,
        "",
    );
    app.ingest_event(&AgentSessionEvent::MessageStart {
        message: AgentMessage::Assistant(partial.clone()),
    });
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(partial.clone()),
        assistant_message_event: Box::new(StreamEvent::TextDelta {
            content_index: 0,
            delta: text.to_string(),
            partial,
        }),
    });
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: AgentMessage::Assistant(assistant(text, tool)),
    });
}

fn run_tool(app: &mut App<TestBackend>, id: &str, name: &str, args: serde_json::Value, out: &str) {
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: cyrup_core::ToolCallId::from(id),
        tool_name: name.into(),
        args: args.clone(),
    });
    app.ingest_event(&AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: cyrup_core::ToolCallId::from(id),
        tool_name: name.into(),
        is_error: false,
        result: serde_json::json!({ "content": [{ "type": "text", "text": out }] }),
    });
}

/// The turn the bug report describes, start to finish.
fn drive_turn(app: &mut App<TestBackend>) {
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.transcript_mut().push_user("read main.rs please");
    app.draw().unwrap();
    assistant_step(
        app,
        "I'll check the file.",
        Some(("call_1", "read", serde_json::json!({ "file_path": "/src/main.rs" }))),
    );
    app.draw().unwrap();
    run_tool(
        app,
        "call_1",
        "read",
        serde_json::json!({ "file_path": "/src/main.rs" }),
        "fn main() {}",
    );
    app.draw().unwrap();
    assistant_step(app, "Done - it is a stub.", None);
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();
}

/// The row index of the first scrollback line whose text contains `needle`.
fn row_of(app: &App<TestBackend>, needle: &str) -> usize {
    app.scrollback_lines()
        .iter()
        .position(|l| {
            l.spans.iter().map(|s| s.content.as_ref()).collect::<String>().contains(needle)
        })
        .unwrap_or_else(|| {
            panic!(
                "{needle:?} is not in scrollback:\n{}",
                app.scrollback_lines()
                    .iter()
                    .enumerate()
                    .map(|(i, l)| format!(
                        "[{i}] bg={:?} {}",
                        l.style.bg,
                        l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()
                    ))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })
}

/// ORDER. `chatContainer` is a flat, append-only list upstream: the assistant component goes in at
/// `message_start` (`interactive-mode.ts:3139`), each `ToolExecutionComponent` after it as the call
/// appears (`:3166`/`:3240`), and the NEXT step's assistant component after those. Committed
/// scrollback must read in exactly that order.
#[test]
fn a_turn_interleaves_user_text_tool_text_in_that_order() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    drive_turn(&mut app);

    let user = row_of(&app, "read main.rs please");
    let first_text = row_of(&app, "I'll check the file.");
    let tool = row_of(&app, "/src/main.rs");
    let second_text = row_of(&app, "Done - it is a stub.");

    assert!(user < first_text, "user echo must precede the answer (user={user}, text={first_text})");
    assert!(
        first_text < tool,
        "the tool block must follow the text that requested it (text={first_text}, tool={tool})"
    );
    assert!(
        tool < second_text,
        "the tool block must precede the NEXT step's text (tool={tool}, text={second_text})"
    );
}

/// The two steps' texts must stay two blocks. While `MessageEnd` was a no-op both deltas piled into
/// one streaming buffer and `agent_end` committed them fused: `I'll check the file.Done - it is a
/// stub.` on one row.
#[test]
fn two_assistant_steps_do_not_fuse_into_one_block() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    drive_turn(&mut app);
    assert!(
        !app.scrollback_text().contains("I'll check the file.Done"),
        "consecutive assistant steps fused into one block:\n{}",
        app.scrollback_text()
    );
    assert_ne!(
        row_of(&app, "I'll check the file."),
        row_of(&app, "Done - it is a stub."),
        "the two steps landed on the same row"
    );
}

/// CELLS, not rows. Each committed line is asserted for the background it actually paints:
/// the tool block's rows carry `toolSuccessBg` across the full width (Pi `Box.applyBg`,
/// `box.ts:127-136`, via `updateDisplay`'s `bgFn`, `tool-execution.ts:253-258`); the assistant text
/// rows carry **no** background at all (`AssistantMessageComponent` is a bare `Markdown` child,
/// `assistant-message.ts:104-114` — no `Box`, no fill).
#[test]
fn painted_cells_tint_the_tool_block_only_and_never_the_prose() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    drive_turn(&mut app);

    let tool = row_of(&app, "/src/main.rs");
    let lines = app.scrollback_lines();

    // The header row and its two `paddingY` rows are the tinted box, painted edge to edge — the
    // whole point of `applyBg` padding to `width` before it paints.
    for row in [tool - 1, tool, tool + 1] {
        assert_eq!(
            lines[row].style.bg,
            Some(DARK_TOOL_SUCCESS_BG),
            "tool box row {row} is not painted toolSuccessBg"
        );
        let painted: usize = lines[row].spans.iter().map(|s| s.content.chars().count()).sum();
        assert!(painted >= 99, "tool box row {row} only painted {painted} of 100 columns");
        for span in &lines[row].spans {
            assert!(
                span.style.bg.is_none() || span.style.bg == Some(DARK_TOOL_SUCCESS_BG),
                "a span inside the tool box overrides the box tint: {span:?}"
            );
        }
    }
    // The prose rows on both sides are untinted.
    for needle in ["I'll check the file.", "Done - it is a stub."] {
        let row = row_of(&app, needle);
        assert_eq!(
            lines[row].style.bg, None,
            "assistant prose row {row} ({needle:?}) was painted a background"
        );
        for span in &lines[row].spans {
            assert_eq!(span.style.bg, None, "assistant prose span carries a background: {span:?}");
        }
    }
    // The tint is a background only — the header keeps its own foreground, so nothing is buried.
    assert!(
        lines[tool].spans.iter().any(|s| s.style.fg.is_some()),
        "the tool header lost its foreground and reads as bare background"
    );
}

/// The GREEN SLAB. A committed tool renders at the live `toolOutputExpanded`, which is `false` by
/// default (`interactive-mode.ts:442`), so a collapsed `read` commits as Pi's three-row box —
/// top pad, header, bottom pad — and not as the whole file wrapped in tool background.
#[test]
fn a_committed_read_stays_collapsed_instead_of_painting_the_whole_file() {
    let body: String = (1..=60).map(|i| format!("let x{i} = {i};\n")).collect();
    let mut app = App::new(TestBackend::new(70, 30), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    assistant_step(
        &mut app,
        "reading",
        Some(("c1", "read", serde_json::json!({ "file_path": "/big.rs" }))),
    );
    run_tool(&mut app, "c1", "read", serde_json::json!({ "file_path": "/big.rs" }), &body);
    assistant_step(&mut app, "done reading", None);
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();

    let tinted =
        app.scrollback_lines().iter().filter(|l| l.style.bg == Some(DARK_TOOL_SUCCESS_BG)).count();
    assert_eq!(
        tinted,
        3,
        "a collapsed `read` must commit as Pi's 3-row Box(1,1) (`read.ts:178-180` returns \"\" \
         collapsed), got {tinted} tinted rows:\n{}",
        app.scrollback_text()
    );
    assert!(
        !app.scrollback_text().contains("let x60"),
        "the collapsed block dumped the file body into scrollback:\n{}",
        app.scrollback_text()
    );

    // `Ctrl+O` before the flush opens it, exactly as `setToolsExpanded`'s broadcast reaches every
    // `chatContainer` child (`:4032-4046`) — the flag is read at paint time, never frozen.
    let mut app = App::new(TestBackend::new(70, 30), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.transcript_mut().toggle_tool_expanded();
    assistant_step(
        &mut app,
        "reading",
        Some(("c1", "read", serde_json::json!({ "file_path": "/big.rs" }))),
    );
    run_tool(&mut app, "c1", "read", serde_json::json!({ "file_path": "/big.rs" }), &body);
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("let x60"),
        "expanded tool output did not reach scrollback:\n{}",
        app.scrollback_text()
    );
}

/// The live viewport paints the same way, per row: a running tool is `toolPendingBg`
/// (`tool-execution.ts:254-255`, keyed on `isPartial`) and the rows around it stay at the terminal
/// default. Read off the real `TestBackend` cells.
#[test]
fn live_viewport_cells_tint_only_the_running_tool_rows() {
    let mut app = App::new(TestBackend::new(70, 24), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    assistant_step(
        &mut app,
        "checking",
        Some(("c1", "read", serde_json::json!({ "file_path": "/m.rs" }))),
    );
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: cyrup_core::ToolCallId::from("c1"),
        tool_name: "read".into(),
        args: serde_json::json!({ "file_path": "/m.rs" }),
    });
    app.draw().unwrap();

    let buf = app.terminal().backend().buffer().clone();
    let area = *buf.area();
    let mut pending_rows = 0usize;
    for y in 0..area.height {
        let bgs: Vec<Color> = (0..area.width).map(|x| buf[(x, y)].bg).collect();
        if bgs.contains(&DARK_TOOL_PENDING_BG) {
            assert!(
                bgs.iter().all(|b| *b == DARK_TOOL_PENDING_BG),
                "row {y} is only PARTLY tinted — `applyBg` pads to the full width \
                 (`box.ts:127-136`)"
            );
            pending_rows += 1;
        }
    }
    assert_eq!(
        pending_rows, 3,
        "a pending `read` must be Pi's 3-row Box(1,1) in toolPendingBg, got {pending_rows}"
    );
    // Nothing is painted toolSuccessBg while the call is still running.
    assert!(
        (0..area.height)
            .flat_map(|y| (0..area.width).map(move |x| (x, y)))
            .all(|(x, y)| buf[(x, y)].bg != DARK_TOOL_SUCCESS_BG),
        "a still-running tool was painted the success tint"
    );
}

/// The 256-colour projection is Pi's, exactly. `#283228` has channel spread `10`, so `rgbTo256`'s
/// `spread < 10` grayscale escape does NOT apply (`theme.ts:243-251`) and the 6×6×6 cube wins:
/// `40→0`, `50→95`, `40→0` ⇒ `16 + 36*0 + 6*1 + 0 = 22`. Index 22 is `#005f00`, a vivid green —
/// which is what a 256-colour terminal shows for the tool background in cyrup **and in Pi**. This
/// test pins the parity so the tint is never "fixed" away from upstream; the defect the report
/// describes was the SIZE of the painted region, not its colour.
#[test]
fn ansi256_tool_tints_quantise_exactly_as_pi_does() {
    let theme = UiTheme::dark().with_color_mode(ColorMode::Ansi256);
    let mut app = App::new(TestBackend::new(70, 24), theme).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    assistant_step(
        &mut app,
        "checking",
        Some(("c1", "read", serde_json::json!({ "file_path": "/m.rs" }))),
    );
    run_tool(&mut app, "c1", "read", serde_json::json!({ "file_path": "/m.rs" }), "fn main() {}");
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();

    let tinted: Vec<Color> = app
        .scrollback_lines()
        .iter()
        .filter_map(|l| l.style.bg)
        .filter(|b| *b != Color::Reset)
        .collect();
    assert!(!tinted.is_empty(), "no tinted rows at all under Ansi256");
    for bg in &tinted {
        assert_eq!(*bg, Color::Indexed(22), "toolSuccessBg must quantise to Pi's cube index 22");
    }
    // And the block is still three rows, not the file.
    assert_eq!(tinted.len(), 3, "the Ansi256 block grew past Pi's 3-row Box(1,1)");
}

/// Idempotence of the finalize. `message_end` is guarded on the open-message bit — Pi's
/// `if (this.streamingComponent && ...)` (`interactive-mode.ts:3182`), cleared at `:3213` — so a
/// producer that DOES forward a terminal `StreamEvent::Done` inside `message_update` cannot have
/// its message committed twice when `MessageEnd` follows.
#[test]
fn a_forwarded_terminal_then_message_end_commits_the_text_once() {
    let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
    let partial = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude",
        None,
        StopReason::Stop,
        "",
    );
    let final_msg = assistant("only once", None);
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.ingest_event(&AgentSessionEvent::MessageStart {
        message: AgentMessage::Assistant(partial),
    });
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(final_msg.clone()),
        assistant_message_event: Box::new(StreamEvent::terminal(final_msg.clone())),
    });
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: AgentMessage::Assistant(final_msg),
    });
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();

    let hits = app.scrollback_text().matches("only once").count();
    assert_eq!(hits, 1, "the message committed {hits} times:\n{}", app.scrollback_text());
}
