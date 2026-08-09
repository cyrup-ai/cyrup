//! Assistant stop-reason surfacing (`assistant-message.ts:175-201`).
//!
//! Pi's `AssistantMessageComponent` appends an `error`-styled line after the turn body when the
//! message did not finish cleanly:
//!
//! * `stopReason === "length"` → the max-output-token notice, rendered **always** (a length stop can
//!   land before a tool call completes, so it is surfaced even for tool turns);
//! * otherwise, only when the message carries NO `toolCall` content (the tool component reports the
//!   failure itself): `"aborted"` → `errorMessage` unless it is the internal `Request was aborted`
//!   sentinel, else `Operation aborted`; `"error"` → `Error: {errorMessage || "Unknown error"}`.
//!
//! These tests drive the real `App::ingest_event` seam with a terminal `StreamEvent` and read the
//! committed scrollback, so they assert what the user actually sees — text AND the `error` role
//! colour — not merely that a function exists.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_agent::AgentMessage;
use cyrup_core::{ApiId, AssistantMessage, Content, ProviderId, StopReason, ToolCall, ToolCallId};
use cyrup_provider::StreamEvent;
use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;

/// Pi v0.84.1 `coding-agent/src/modes/interactive/components/assistant-message.ts:180`, verbatim.
///
/// Changed from the v0.83.0 wording ("Error: Model stopped because it reached the maximum output
/// token limit. The response may be incomplete.", v0.83.0 `assistant-message.ts:153-161`) by
/// upstream `32850ef7c`: a `length` stop may now be a context overflow that pi compacts and
/// retries, so the notice no longer asserts the max-output-token cause. Upstream's own test moved
/// with it (`coding-agent/test/assistant-message.test.ts:63,73` — "renders length stops with
/// neutral truncation wording", expecting exactly this string).
const LENGTH_NOTICE: &str = "Response was truncated before completion.";

/// The v0.83.0 wording, asserted ABSENT so a silent regression to it is caught.
const OLD_LENGTH_NOTICE: &str = "maximum output token limit";

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap()
}

/// A terminal assistant message with the given stop reason / error text / content blocks.
fn message(
    stop_reason: StopReason,
    error_message: Option<&str>,
    content: Vec<Content>,
) -> AssistantMessage {
    let mut msg = AssistantMessage::errored(
        ProviderId::from("anthropic"),
        "claude-opus-4",
        Some(ApiId::from("anthropic-messages")),
        stop_reason,
        String::new(),
    );
    msg.error_message = error_message.map(str::to_string);
    msg.content = content;
    msg
}

fn text(t: &str) -> Content {
    Content::Text { text: t.to_string(), text_signature: None }
}

fn tool_call() -> Content {
    Content::ToolCall(ToolCall {
        id: ToolCallId::from("call_1"),
        name: "bash".to_string(),
        arguments: serde_json::Map::new(),
        thought_signature: None,
    })
}

/// Feed one terminal stream event through the public `ingest_event` seam and return the committed
/// scrollback the shell would hand to `Terminal::insert_before`.
fn scrollback_for(ev: StreamEvent) -> (App<TestBackend>, String) {
    let mut app = new_app();
    let message = ev.terminal_message().cloned().expect("terminal event carries a message");
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(message),
        assistant_message_event: Box::new(ev),
    });
    app.draw().unwrap();
    let out = app.scrollback_text();
    (app, out)
}

/// Whether some committed scrollback line containing `needle` is painted in the theme's `error`
/// colour — Pi wraps every one of these notices in `theme.fg("error", …)`.
fn styled_error(app: &App<TestBackend>, needle: &str) -> bool {
    let want = UiTheme::dark().error_style();
    app.scrollback_lines().iter().any(|line| {
        let joined: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        // Effective style = the span style patched over the line style (how ratatui paints it).
        let effective = line
            .spans
            .iter()
            .find(|s| s.content.contains(needle))
            .map(|s| line.style.patch(s.style));
        joined.contains(needle)
            && effective.is_some_and(|st| st.fg == want.fg && st.add_modifier == want.add_modifier)
    })
}

#[test]
fn error_stop_reason_surfaces_the_provider_error() {
    let (app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Error, Some("upstream 503"), vec![text("partial answer")])));
    assert!(out.contains("partial answer"), "partial body still committed:\n{out}");
    assert!(out.contains("Error: upstream 503"), "provider error surfaced:\n{out}");
    assert!(styled_error(&app, "Error: upstream 503"), "notice uses the error role:\n{out}");
}

#[test]
fn error_stop_reason_without_a_message_says_unknown_error() {
    let (_app, out) =
        scrollback_for(StreamEvent::terminal(message(StopReason::Error, None, vec![])));
    assert!(out.contains("Error: Unknown error"), "unknown-error fallback:\n{out}");
}

#[test]
fn aborted_stop_reason_surfaces_the_abort_message() {
    let (app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Aborted, Some("user pressed escape"), vec![])));
    assert!(out.contains("user pressed escape"), "abort message surfaced:\n{out}");
    assert!(!out.contains("Error: user pressed escape"), "abort is not `Error:`-prefixed:\n{out}");
    assert!(styled_error(&app, "user pressed escape"), "notice uses the error role:\n{out}");
}

#[test]
fn aborted_stop_reason_replaces_the_internal_sentinel() {
    // `Request was aborted` is Pi's internal sentinel; the user-facing wording is `Operation aborted`.
    let (_app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Aborted, Some("Request was aborted"), vec![])));
    assert!(out.contains("Operation aborted"), "sentinel replaced:\n{out}");
    assert!(!out.contains("Request was aborted"), "sentinel not shown verbatim:\n{out}");
}

#[test]
fn aborted_stop_reason_without_a_message_says_operation_aborted() {
    let (_app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Aborted, None, vec![])));
    assert!(out.contains("Operation aborted"), "abort fallback wording:\n{out}");
}

#[test]
fn length_stop_reason_surfaces_the_truncation_notice() {
    let (app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Length, None, vec![text("a truncated answer")])));
    assert!(out.contains("a truncated answer"), "truncated body committed:\n{out}");
    assert!(out.contains(LENGTH_NOTICE), "neutral truncation notice:\n{out}");
    assert!(styled_error(&app, LENGTH_NOTICE), "notice uses the error role:\n{out}");
    // The wording is neutral about the CAUSE (`32850ef7c`) — it must not claim a token limit.
    assert!(!out.contains(OLD_LENGTH_NOTICE), "v0.83.0 wording must be gone:\n{out}");
    // MIRROR: `assistant-message.ts:180` passes the bare sentence to `theme.fg("error", …)`; only
    // the `error` arm (`:193`) builds an `Error: `-prefixed string. A length stop is not prefixed.
    assert!(!out.contains("Error: Response was truncated"), "length notice is not prefixed:\n{out}");
}

#[test]
fn length_notice_is_shown_even_when_the_turn_carries_tool_calls() {
    // `assistant-message.ts:177` — the `length` branch is NOT gated on `hasToolCalls`.
    let (_app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Length, None, vec![text("calling out"), tool_call()])));
    assert!(out.contains(LENGTH_NOTICE), "length notice survives a tool turn:\n{out}");
}

#[test]
fn error_notice_is_suppressed_when_the_turn_carries_tool_calls() {
    // `assistant-message.ts:189` — the tool-execution component reports the failure instead.
    let (_app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Error, Some("tool blew up"), vec![tool_call()])));
    assert!(!out.contains("tool blew up"), "tool turns do not duplicate the error:\n{out}");
}

#[test]
fn aborted_notice_is_suppressed_when_the_turn_carries_tool_calls() {
    let (_app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Aborted, None, vec![tool_call()])));
    assert!(!out.contains("Operation aborted"), "tool turns do not duplicate the abort:\n{out}");
}

#[test]
fn a_clean_stop_adds_no_notice() {
    let (_app, out) = scrollback_for(StreamEvent::terminal(message(StopReason::Stop, None, vec![text("all done")])));
    assert!(out.contains("all done"), "body committed:\n{out}");
    assert!(!out.contains("Error:"), "no notice on a clean stop:\n{out}");
    assert!(!out.contains("aborted"), "no abort wording on a clean stop:\n{out}");
}


/// `Pending` is the in-flight sentinel (`cyrup_core::StopReason`), and Pi renders it like a clean
/// stop: its chain is `if ("length") … else if (!hasToolCalls) { if ("aborted") … else if
/// ("error") … }` (`assistant-message.ts:177-201`), and `"pending"` matches none of those, so no
/// notice is appended while a message is still streaming.
///
/// This asserts the arm added when `StopReason::Pending` was introduced. A `_ => None` would have
/// produced the same result today but would silently absorb the NEXT variant too; the match is
/// exhaustive so the compiler forces the decision. The rendering path is the same one the live
/// streaming partial takes, so a regression here would put an error banner under every in-flight
/// turn.
#[test]
fn a_pending_in_flight_message_adds_no_notice() {
    let msg = message(StopReason::Pending, None, vec![text("still typin")]);
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::MessageUpdate {
        message: AgentMessage::Assistant(msg.clone()),
        assistant_message_event: Box::new(StreamEvent::TextDelta {
            content_index: 0,
            delta: "still typin".to_string(),
            partial: msg,
        }),
    });
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(!out.contains("Error:"), "an in-flight turn must not show a notice:\n{out}");
    assert!(!out.contains("aborted"), "an in-flight turn must not show abort wording:\n{out}");
}
