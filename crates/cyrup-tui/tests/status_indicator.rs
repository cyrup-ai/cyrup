//! Working/idle status-band tests (spec/tui/01 §6; `components/status-indicator.ts`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::time::Duration;

use cyrup_session_svc::AgentSessionEvent;
use cyrup_tui::{App, IndicatorKind, StatusIndicator, UiTheme, SPINNER_FRAMES};
use ratatui::backend::TestBackend;
use ratatui::text::Line;

fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn buf_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    out
}

#[test]
fn idle_band_is_two_blank_lines() {
    let theme = UiTheme::dark();
    let ind = StatusIndicator::new();
    assert_eq!(ind.kind(), IndicatorKind::Idle);
    assert!(!ind.is_active());
    let lines = ind.lines_at(Duration::ZERO, &theme, Some("esc"));
    assert_eq!(lines.len(), 2, "idle band must reserve two rows");
    assert!(lines.iter().all(|l| line_text(l).trim().is_empty()), "idle band not blank");
}

#[test]
fn working_band_shows_spinner_message_and_cancel_hint() {
    let theme = UiTheme::dark();
    let mut ind = StatusIndicator::new();
    ind.working();
    assert!(ind.is_active());
    let lines = ind.lines_at(Duration::ZERO, &theme, Some("esc"));
    assert_eq!(lines.len(), 2);
    assert!(line_text(&lines[0]).trim().is_empty(), "first band line should be blank (loader spacer)");
    let msg = line_text(&lines[1]);
    assert!(msg.contains("Working…"), "working message missing: [{msg}]");
    assert!(msg.contains("(esc to cancel)"), "cancel hint missing: [{msg}]");
    // The leading glyph is one of the Braille spinner frames.
    assert!(SPINNER_FRAMES.iter().any(|f| msg.starts_with(f)), "spinner glyph missing: [{msg}]");
}

#[test]
fn spinner_advances_every_80ms() {
    assert_eq!(StatusIndicator::spinner_at(Duration::ZERO), SPINNER_FRAMES[0]);
    assert_eq!(StatusIndicator::spinner_at(Duration::from_millis(80)), SPINNER_FRAMES[1]);
    assert_eq!(StatusIndicator::spinner_at(Duration::from_millis(79)), SPINNER_FRAMES[0]);
    // Wraps after the full frame count (10 * 80 ms = 800 ms).
    assert_eq!(StatusIndicator::spinner_at(Duration::from_millis(800)), SPINNER_FRAMES[0]);
}

#[test]
fn retry_uses_warning_spinner_color() {
    let theme = UiTheme::dark();
    let mut ind = StatusIndicator::new();
    ind.set(IndicatorKind::Retry, Some("Retrying (1/3)…".to_string()));
    let lines = ind.lines_at(Duration::ZERO, &theme, Some("esc"));
    let spinner_span = &lines[1].spans[0];
    assert_eq!(spinner_span.style.fg, theme.warning_style().fg, "retry spinner not warning-colored");
    assert!(line_text(&lines[1]).contains("Retrying (1/3)…"), "retry message missing");
}

#[test]
fn agent_start_renders_band_and_agent_end_clears_it() {
    // gap 11: AgentStart activates the working band in the live region; AgentEnd returns it to idle
    // (blank) — the band is rendered between the active turn and the editor.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.draw().unwrap();
    let working = buf_text(&app);
    assert!(working.contains("Working…"), "working band not rendered after AgentStart:\n{working}");
    assert!(working.contains("esc to cancel"), "cancel hint not in band:\n{working}");
    assert!(
        SPINNER_FRAMES.iter().any(|f| working.contains(f)),
        "spinner glyph not rendered:\n{working}"
    );

    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![] });
    app.draw().unwrap();
    let idle = buf_text(&app);
    assert!(!idle.contains("Working…"), "working band lingered after AgentEnd:\n{idle}");
}

#[test]
fn compaction_band_renders_its_message() {
    // The compaction indicator (driven by `CompactionStart` in `ingest_event`) renders its default
    // message in the band. Exercised via the public indicator API to avoid a session-crate dev-dep.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.state_mut().indicator.set(IndicatorKind::Compaction, None);
    app.draw().unwrap();
    assert!(buf_text(&app).contains("Compacting context…"), "compaction band missing");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Compaction);

    app.state_mut().indicator.idle();
    app.draw().unwrap();
    assert!(!buf_text(&app).contains("Compacting context…"), "band should clear when idle");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Idle);
}
