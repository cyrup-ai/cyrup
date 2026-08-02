//! Assistant **thinking**/reasoning rendering (TUI-002; `assistant-message.ts:115-166`).
//!
//! Pi renders the `thinking` content blocks of an assistant turn as their own section: the run of
//! adjacent blocks is coalesced with `\n\n` (`:116-127`) and drawn as italic `thinkingText` markdown
//! (`:145-165`), or — when `hideThinkingBlock` is on — replaced by the single static
//! `hiddenThinkingLabel` (`"Thinking..."`, `:29` / `:139-143`). A blank spacer separates the
//! reasoning from the answer only when more visible content follows (`hasVisibleContentAfter`,
//! `:134-137`).
//!
//! These tests drive the real seams — `App::ingest_event` for the streaming
//! `StreamEvent::ThinkingDelta` and for the terminal message — and read what actually lands on
//! screen: the live inline viewport for the in-flight block, the committed `insert_before`
//! scrollback for the finished one.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::AgentMessage;
use cyrup_core::{ApiId, AssistantMessage, Content, ProviderId, StopReason};
use cyrup_provider::StreamEvent;
use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;

const REASONING: &str = "the user wants the parser rewritten";
const ANSWER: &str = "Here is the plan.";

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap()
}

fn thinking(t: &str) -> Content {
    Content::Thinking { thinking: t.to_string(), thinking_signature: None, redacted: false }
}

fn text(t: &str) -> Content {
    Content::Text { text: t.to_string(), text_signature: None }
}

/// An assistant message shell (no content) — the `partial` a streaming frame carries.
fn blank() -> AssistantMessage {
    let mut msg = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        StopReason::Stop,
        String::new(),
    );
    msg.error_message = None;
    msg
}

/// A clean terminal assistant message carrying `content`.
fn done(content: Vec<Content>) -> StreamEvent {
    let mut msg = blank();
    msg.content = content;
    StreamEvent::terminal(msg)
}

fn feed(app: &mut App<TestBackend>, ev: StreamEvent) {
    let message = ev.terminal_message().cloned().unwrap_or_else(blank);
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(message),
        assistant_message_event: Box::new(ev),
    });
}

/// The bottom `viewport_height` rows — the live inline region the app repaints each frame.
fn live_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let vh = app.viewport_height().min(area.height);
    let mut out = String::new();
    for y in (area.height - vh)..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// Whether some committed scrollback line containing `needle` is painted in the `thinkingText`
/// role, italic — Pi's `{color: theme.fg("thinkingText", …), italic: true}`.
fn styled_thinking(app: &App<TestBackend>, needle: &str) -> bool {
    let want = UiTheme::dark().thinking_text_style();
    app.scrollback_lines().iter().any(|line| {
        let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // Effective style = the span style patched over the line style (how ratatui paints it).
        let effective = line
            .spans
            .iter()
            .find(|s| s.content.contains(needle))
            .map(|s| line.style.patch(s.style));
        joined.contains(needle)
            && effective.is_some_and(|st| {
                st.fg == want.fg && st.add_modifier.contains(Modifier::ITALIC)
            })
    })
}

/// Streaming `ThinkingDelta` frames must grow a visible reasoning block in the LIVE viewport, not
/// vanish (`assistant-message.ts` renders thinking while the turn streams).
#[test]
fn streaming_thinking_deltas_render_in_the_live_viewport() {
    let mut app = new_app();
    for chunk in ["the user wants ", "the parser rewritten"] {
        feed(
            &mut app,
            StreamEvent::ThinkingDelta {
                content_index: 0,
                delta: chunk.to_string(),
                partial: blank(),
            },
        );
    }
    app.draw().unwrap();

    let live = live_text(&app);
    assert!(
        live.contains(REASONING),
        "streamed thinking must be visible in the live region; got:\n{live}"
    );
    assert_eq!(app.transcript_mut().thinking(), Some(REASONING));
}

/// A terminal message's `thinking` blocks commit to scrollback ABOVE the answer text, in the
/// italic `thinkingText` role.
#[test]
fn terminal_thinking_blocks_commit_above_the_answer() {
    let mut app = new_app();
    feed(&mut app, done(vec![thinking(REASONING), text(ANSWER)]));
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains(REASONING), "committed scrollback must carry the reasoning; got:\n{out}");
    assert!(out.contains(ANSWER), "committed scrollback must carry the answer; got:\n{out}");
    let think_at = out.find(REASONING).unwrap();
    let answer_at = out.find(ANSWER).unwrap();
    assert!(think_at < answer_at, "reasoning must precede the answer; got:\n{out}");
    assert!(styled_thinking(&app, REASONING), "reasoning must use the italic thinkingText role");
}

/// Adjacent thinking blocks are coalesced into one section joined by `\n\n`
/// (`assistant-message.ts:116-127` + `:145`).
#[test]
fn adjacent_thinking_blocks_are_coalesced() {
    let mut app = new_app();
    feed(&mut app, done(vec![thinking("first  "), thinking("  second"), text(ANSWER)]));
    app.draw().unwrap();

    let out = app.scrollback_text();
    let first = out.find("first").expect("first block rendered");
    let second = out.find("second").expect("second block rendered");
    assert!(first < second, "blocks keep their order; got:\n{out}");
    // One blank line between the two trimmed blocks (the `\n\n` join).
    let between = &out[first + "first".len()..second];
    assert_eq!(
        between.matches('\n').count(),
        2,
        "coalesced blocks join with a blank line; got {between:?} in:\n{out}"
    );
}

/// `hideThinkingBlock` replaces the body with Pi's single static `Thinking...` label
/// (`assistant-message.ts:29` / `:139-143`).
#[test]
fn hidden_thinking_renders_the_static_label_only() {
    let mut app = new_app();
    app.transcript_mut().set_hide_thinking_block(true);
    feed(&mut app, done(vec![thinking(REASONING), text(ANSWER)]));
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(out.contains("Thinking..."), "hidden reasoning shows the label; got:\n{out}");
    assert!(!out.contains(REASONING), "hidden reasoning must NOT show the body; got:\n{out}");
    assert!(out.contains(ANSWER), "the answer still renders; got:\n{out}");
    assert!(styled_thinking(&app, "Thinking..."), "the label uses the italic thinkingText role");
}

/// A turn with no reasoning commits no thinking entry — and whitespace-only reasoning is dropped
/// exactly as Pi skips a run whose trimmed blocks are all empty (`:128-130`).
#[test]
fn empty_reasoning_commits_nothing() {
    let mut app = new_app();
    feed(&mut app, done(vec![thinking("   \n  "), text(ANSWER)]));
    app.draw().unwrap();

    let out = app.scrollback_text();
    assert!(!out.contains("Thinking..."), "no label for empty reasoning; got:\n{out}");
    assert!(out.contains(ANSWER));
}

/// An interrupt drops the in-flight reasoning with the rest of the partial turn (R-10-030).
#[test]
fn interrupt_discards_the_live_reasoning() {
    let mut app = new_app();
    feed(
        &mut app,
        StreamEvent::ThinkingDelta {
            content_index: 0,
            delta: REASONING.to_string(),
            partial: blank(),
        },
    );
    app.transcript_mut().discard_streaming();
    app.draw().unwrap();

    assert_eq!(app.transcript_mut().thinking(), None);
    let live = live_text(&app);
    assert!(!live.contains(REASONING), "aborted reasoning must not linger; got:\n{live}");
}
