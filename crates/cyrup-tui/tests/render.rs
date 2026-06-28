//! Headless render tests against ratatui's `TestBackend` (R-10-010 / R-ARCH-TUI-010).
//!
//! Renders the app with a synthetic transcript + status to a virtual buffer and asserts the
//! expected text / themed cells appear: user text, assistant text, a streaming delta, the status
//! line, event-driven status updates, and theme colors reaching the rendered cells.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, UiTheme};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

/// Flatten the test backend's cell grid into rows of text for `contains` assertions.
fn buf_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// True if any non-blank cell carries the given foreground color.
fn has_fg(app: &App<TestBackend>, color: Color) -> bool {
    let buf = app.terminal().backend().buffer();
    buf.content().iter().any(|cell| cell.fg == color && cell.symbol() != " ")
}

#[test]
fn committed_entries_move_to_scrollback_and_only_active_turn_renders() {
    // R-ARCH-TUI-003: finalized entries flow to native scrollback (insert_before); the inline
    // viewport renders only the active streaming turn + editor + status.
    let mut app = App::new(TestBackend::new(50, 16), UiTheme::dark()).unwrap();
    app.transcript_mut().push_user("hello world");
    app.transcript_mut().commit_assistant(Some("hi there".to_string()));
    // A second, still-streaming assistant turn arrives as deltas (the active region).
    app.transcript_mut().push_assistant_delta("strea");
    app.transcript_mut().push_assistant_delta("ming…");
    app.draw().unwrap();

    // (a) Committed user + assistant text reached the scrollback accumulator (what insert_before got).
    let sb = app.scrollback_text();
    assert!(sb.contains("you: hello world"), "scrollback missing user line:\n{sb}");
    assert!(sb.contains("assistant: hi there"), "scrollback missing assistant line:\n{sb}");

    // (b) The viewport no longer contains the committed text, but DOES show the active streaming turn
    //     plus the editor + status line.
    let text = buf_text(&app);
    assert!(!text.contains("hello world"), "committed user leaked into viewport:\n{text}");
    assert!(!text.contains("hi there"), "committed assistant leaked into viewport:\n{text}");
    assert!(text.contains("streaming…"), "active streaming turn missing from viewport:\n{text}");
    assert!(text.contains("idle"), "status line missing from viewport:\n{text}");
}

#[test]
fn renders_status_line_with_model_and_state() {
    let mut app = App::new(TestBackend::new(60, 12), UiTheme::dark()).unwrap();
    app.status_mut().set_model("anthropic/claude");
    app.status_mut().set_streaming(true);
    app.status_mut().set_tokens(1234);
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("anthropic/claude"), "missing model:\n{text}");
    assert!(text.contains("streaming"), "missing state:\n{text}");
    assert!(text.contains("1234 tokens"), "missing tokens:\n{text}");
}

#[test]
fn ingest_events_drive_status_and_transcript() {
    let mut app = App::new(TestBackend::new(60, 14), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.ingest_event(&AgentSessionEvent::ModelChanged {
        provider: "openai".to_string(),
        model: "gpt".to_string(),
    });
    app.ingest_event(&AgentSessionEvent::QueueUpdate {
        steering: vec!["one".to_string()],
        follow_up: vec!["two".to_string(), "three".to_string()],
    });
    app.draw().unwrap();

    let text = buf_text(&app);
    assert!(text.contains("openai/gpt"), "model not reflected:\n{text}");
    assert!(text.contains("streaming"), "streaming not set:\n{text}");
    assert!(text.contains("3 queued"), "queue depth not reflected:\n{text}");
    // The model-change notification is a committed entry: it lives in scrollback, not the viewport.
    let sb = app.scrollback_text();
    assert!(sb.contains("model → openai/gpt"), "model change not logged to scrollback:\n{sb}");
    assert!(!text.contains("model → openai/gpt"), "status entry leaked into viewport:\n{text}");

    // AgentEnd clears the streaming state.
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![] });
    app.draw().unwrap();
    assert!(buf_text(&app).contains("idle"), "idle not set after agent_end");
}

#[test]
fn theme_colors_reach_rendered_cells() {
    // Dark theme accent (#569cd6) styles the active streaming label; assert it lands on real
    // viewport cells (an in-flight turn stays in the viewport, unlike committed entries).
    let accent = Color::Rgb(0x56, 0x9c, 0xd6);
    let mut app = App::new(TestBackend::new(40, 12), UiTheme::dark()).unwrap();
    app.transcript_mut().push_assistant_delta("colored");
    app.draw().unwrap();
    assert!(has_fg(&app, accent), "dark accent color did not reach any cell");

    // A different theme yields a different accent on the cells.
    let light_accent = Color::Rgb(0x00, 0x00, 0xff);
    let mut light = App::new(TestBackend::new(40, 12), UiTheme::light()).unwrap();
    light.transcript_mut().push_assistant_delta("colored");
    light.draw().unwrap();
    assert!(has_fg(&light, light_accent), "light accent color did not reach any cell");
}

#[test]
fn finalized_turn_via_events_flows_to_scrollback_and_clears_viewport() {
    // Drive a full turn through the neutral push API + AgentSessionEvents, exactly as the run loop
    // does, and prove the committed turn lands in scrollback while the viewport empties.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.transcript_mut().push_user("question?");
    app.transcript_mut().push_assistant_delta("ans");
    app.transcript_mut().push_assistant_delta("wer");

    // Mid-turn: the user line has committed to scrollback; the assistant is still active (viewport).
    app.draw().unwrap();
    let mid = buf_text(&app);
    assert!(mid.contains("answer"), "active streaming turn missing from viewport mid-turn:\n{mid}");
    assert!(!mid.contains("question?"), "committed user leaked into viewport:\n{mid}");
    assert!(app.scrollback_text().contains("you: question?"), "user not flushed to scrollback");

    // AgentEnd finalizes the streaming assistant turn.
    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![] });
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("you: question?"), "user missing from scrollback:\n{sb}");
    assert!(sb.contains("assistant: answer"), "finalized assistant missing from scrollback:\n{sb}");

    let view = buf_text(&app);
    assert!(!view.contains("answer"), "finalized assistant still in viewport:\n{view}");
    assert!(view.contains("idle"), "status line missing from viewport after finalize:\n{view}");
}

#[test]
fn committed_entries_flush_exactly_once() {
    // A flushed entry must not reappear in scrollback on subsequent draws (insert_before once).
    let mut app = App::new(TestBackend::new(50, 12), UiTheme::dark()).unwrap();
    app.transcript_mut().push_user("only once");
    app.draw().unwrap();
    app.draw().unwrap();
    app.draw().unwrap();

    let occurrences = app.scrollback_text().matches("you: only once").count();
    assert_eq!(occurrences, 1, "committed entry flushed more than once");
    assert!(app.state().transcript.pending().is_empty(), "pending buffer not drained");
}

#[test]
fn builtin_themes_resolve_known_colors() {
    let dark = UiTheme::builtin("dark");
    assert_eq!(dark.name, "dark");
    assert_eq!(dark.accent, Some(Color::Rgb(0x56, 0x9c, 0xd6)));
    let light = UiTheme::builtin("light");
    assert_eq!(light.name, "light");
    // Unknown names fall back to the dark palette (never panics).
    let fallback = UiTheme::builtin("does-not-exist");
    assert_eq!(fallback.accent, dark.accent);
}
