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
    // `defaultWorkingMessage = "Working..."` — ASCII, v0.84.1 `interactive-mode.ts:420`.
    assert!(msg.contains("Working..."), "working message missing: [{msg}]");
    // `WorkingStatusIndicator` passes the message straight through and appends NOTHING
    // (`status-indicator.ts:29-40`, `interactive-mode.ts:2074-2080`); only retry/compaction/branch
    // bake a `(<key> to cancel)` into their own copy. The suffix was 18 stray columns on every turn.
    assert!(!msg.contains("to cancel"), "Working must carry no cancel suffix: [{msg}]");
    // The line is inset one column — `Loader extends Text` with `paddingX 1` (`loader.ts:35`,
    // `text.ts:70,76`) — and the glyph after it is one of the Braille spinner frames.
    assert!(msg.starts_with(' '), "band not inset one column: [{msg}]");
    assert!(
        SPINNER_FRAMES.iter().any(|f| msg.trim_start().starts_with(f)),
        "spinner glyph missing: [{msg}]"
    );
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
    ind.set(IndicatorKind::Retry, Some("Retrying (1/3)...".to_string()));
    let lines = ind.lines_at(Duration::ZERO, &theme, Some("esc"));
    let spinner_span = &lines[1].spans[0];
    assert_eq!(spinner_span.style.fg, theme.warning_style().fg, "retry spinner not warning-colored");
    assert!(line_text(&lines[1]).contains("Retrying (1/3)..."), "retry message missing");
}

#[test]
fn agent_start_renders_band_and_agent_end_clears_it() {
    // gap 11: AgentStart activates the working band in the live region; AgentEnd returns it to idle
    // (blank) — the band is rendered between the active turn and the editor.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AgentStart);
    app.draw().unwrap();
    let working = buf_text(&app);
    assert!(working.contains("Working..."), "working band not rendered after AgentStart:\n{working}");
    assert!(
        !working.contains("to cancel"),
        "Working carries no cancel suffix (status-indicator.ts:29-40):\n{working}"
    );
    assert!(
        SPINNER_FRAMES.iter().any(|f| working.contains(f)),
        "spinner glyph not rendered:\n{working}"
    );

    app.ingest_event(&AgentSessionEvent::AgentEnd { messages: vec![], will_retry: false });
    app.draw().unwrap();
    let idle = buf_text(&app);
    assert!(!idle.contains("Working..."), "working band lingered after AgentEnd:\n{idle}");
}

#[test]
fn auto_retry_event_renders_pi_exact_copy_with_delay_seconds() {
    // Item #9 — the retry band copy is Pi's exact "Retrying (a/max) in Ns..." (status-indicator.ts:47),
    // driven end-to-end through `ingest_event` from the `AutoRetryStart` event (delay in whole
    // seconds, rounded up), with the live-keymap cancel hint appended by the band.
    let mut app = App::new(TestBackend::new(70, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::AutoRetryStart {
        attempt: 2,
        max_attempts: 5,
        delay_ms: 1500,
        error_message: "429".into(),
    });
    app.draw().unwrap();
    let out = buf_text(&app);
    assert!(out.contains("Retrying (2/5) in 2s"), "retry copy missing/incorrect:\n{out}");
    assert!(out.contains("to cancel"), "cancel hint missing:\n{out}");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Retry);
}

#[test]
fn overflow_compaction_event_prefixes_context_overflow_copy() {
    // Item #9 — an OVERFLOW auto-compaction reads "Context overflow detected, Auto-compacting..."
    // (status-indicator.ts:82), while a MANUAL one reads "Compacting context...".
    use cyrup_session_svc::CompactionReason;
    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::CompactionStart { reason: CompactionReason::Overflow });
    app.draw().unwrap();
    assert!(
        buf_text(&app).contains("Context overflow detected, Auto-compacting..."),
        "overflow compaction copy missing:\n{}",
        buf_text(&app)
    );

    let mut app2 = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    app2.ingest_event(&AgentSessionEvent::CompactionStart { reason: CompactionReason::Manual });
    app2.draw().unwrap();
    let m = buf_text(&app2);
    assert!(m.contains("Compacting context..."), "manual compaction copy missing:\n{m}");
    assert!(!m.contains("Auto-compacting"), "manual must not say Auto-compacting:\n{m}");
}

#[test]
fn compaction_band_renders_its_message() {
    // The compaction indicator (driven by `CompactionStart` in `ingest_event`) renders its default
    // message in the band. Exercised via the public indicator API to avoid a session-crate dev-dep.
    let mut app = App::new(TestBackend::new(60, 16), UiTheme::dark()).unwrap();
    app.state_mut().indicator.set(IndicatorKind::Compaction, None);
    app.draw().unwrap();
    assert!(buf_text(&app).contains("Compacting context..."), "compaction band missing");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Compaction);

    app.state_mut().indicator.idle();
    app.draw().unwrap();
    assert!(!buf_text(&app).contains("Compacting context..."), "band should clear when idle");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Idle);
}

/// DRIFT-006 — a summarization retry must be VISIBLE and must restore the underlying indicator.
///
/// Pi `interactive-mode.ts:3222-3245`: `summarization_retry_scheduled` shows the retry countdown,
/// `summarization_retry_attempt_start` clears it and RECREATES the compaction/branch indicator from
/// the event's `source`, `summarization_retry_finished` clears the retry indicator. Before this
/// change the events did not exist, so a compacting session showed "Compacting context..." frozen for
/// the whole backoff with no indication anything was wrong.
#[test]
fn a_summarization_retry_shows_the_countdown_then_restores_the_compaction_band() {
    use cyrup_session_svc::{CompactionReason, SummarizationRetrySource};
    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();

    app.ingest_event(&AgentSessionEvent::CompactionStart { reason: CompactionReason::Threshold });
    app.draw().unwrap();
    assert!(buf_text(&app).contains("Auto-compacting..."), "sanity: the compaction band is up");

    app.ingest_event(&AgentSessionEvent::SummarizationRetryScheduled {
        attempt: 1,
        max_attempts: 3,
        delay_ms: 2000,
        error_message: "terminated".into(),
    });
    app.draw().unwrap();
    let out = buf_text(&app);
    assert!(out.contains("Retrying (1/3) in 2s"), "retry countdown missing during backoff:\n{out}");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Retry);

    app.ingest_event(&AgentSessionEvent::SummarizationRetryAttemptStart {
        source: SummarizationRetrySource::Compaction { reason: CompactionReason::Threshold },
    });
    app.draw().unwrap();
    let out = buf_text(&app);
    assert_eq!(
        app.state().indicator.kind(),
        IndicatorKind::Compaction,
        "the underlying indicator is recreated from `source` (interactive-mode.ts:3233-3238)"
    );
    assert!(out.contains("Auto-compacting..."), "compaction band not restored:\n{out}");
}

/// The branch-summary arm of the same event picks a DIFFERENT indicator — that is the whole reason
/// Pi's `summarization_retry_attempt_start` carries a discriminated `source`.
#[test]
fn a_branch_summary_retry_restores_the_branch_summary_band_not_the_compaction_one() {
    use cyrup_session_svc::SummarizationRetrySource;
    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();

    app.ingest_event(&AgentSessionEvent::SummarizationRetryScheduled {
        attempt: 3,
        max_attempts: 3,
        delay_ms: 8000,
        error_message: "terminated".into(),
    });
    app.ingest_event(&AgentSessionEvent::SummarizationRetryAttemptStart {
        source: SummarizationRetrySource::BranchSummary,
    });
    app.draw().unwrap();
    assert_eq!(app.state().indicator.kind(), IndicatorKind::BranchSummary);
    assert!(
        buf_text(&app).contains("Summarizing branch..."),
        "branch-summary band not restored:\n{}",
        buf_text(&app)
    );
}

/// `summarization_retry_finished` clears the retry band when the loop ended DURING a backoff
/// (exhausted / aborted) — the only case where the retry indicator is still live, since a
/// successful retried call already restored its own band via `attempt_start`.
#[test]
fn summarization_retry_finished_clears_a_still_live_retry_band() {
    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::SummarizationRetryScheduled {
        attempt: 3,
        max_attempts: 3,
        delay_ms: 8000,
        error_message: "terminated".into(),
    });
    app.draw().unwrap();
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Retry);

    app.ingest_event(&AgentSessionEvent::SummarizationRetryFinished);
    app.draw().unwrap();
    assert_ne!(
        app.state().indicator.kind(),
        IndicatorKind::Retry,
        "an exhausted retry must not leave the countdown spinning forever"
    );
}

/// X18 — `interactive-mode.ts:3286-3298` (`compaction_start`) and `:3339-3347`
/// (`auto_retry_start`) call `showStatusIndicator(...)` and nothing else. A `StatusIndicator`
/// extends `Loader` (`status-indicator.ts:9-27`) and lives in the fixed status slot, so it vanishes
/// on `clearStatusIndicator`. Nothing about a compaction or a retry is ever written to the message
/// list — `showStatus` (`interactive-mode.ts:3411-3428`) exists but is called for other things
/// (e.g. `"Auto-compaction cancelled"` at `:3313`), never for these two events.
///
/// cyrup was mirroring the SAME string into the transcript, which `insert_before` then froze into
/// native scrollback as a permanent dim `• Compacting context...` / `• Retrying (1/3) in 30s...`
/// row.
#[test]
fn compaction_and_retry_status_is_band_only_never_scrollback() {
    use cyrup_session_svc::CompactionReason;

    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&AgentSessionEvent::CompactionStart { reason: CompactionReason::Manual });
    app.ingest_event(&AgentSessionEvent::AutoRetryStart {
        attempt: 1,
        max_attempts: 3,
        delay_ms: 30_000,
        error_message: "429".into(),
    });
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(!sb.contains("Compacting context"), "compaction copy leaked to scrollback:\n{sb}");
    assert!(!sb.contains("Retrying"), "retry copy leaked to scrollback:\n{sb}");
    assert!(!sb.contains('•'), "status bullet row leaked to scrollback:\n{sb}");

    // MIRROR: the BAND still shows it — removing the mirror must not remove the affordance.
    let live = buf_text(&app);
    assert!(live.contains("Retrying (1/3) in 30s"), "retry band missing:\n{live}");
    assert_eq!(app.state().indicator.kind(), IndicatorKind::Retry);
}

/// X18, second half: `summarization_retry_scheduled` (`interactive-mode.ts:3367-3374`) is
/// `showError(event.errorMessage)` **then** `showStatusIndicator(new RetryStatusIndicator(...))` —
/// the error text belongs in the chat, the countdown belongs in the band.
#[test]
fn summarization_retry_writes_the_error_but_not_the_countdown() {
    use cyrup_session_svc::AgentSessionEvent as Ev;

    let mut app = App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap();
    app.ingest_event(&Ev::SummarizationRetryScheduled {
        attempt: 2,
        max_attempts: 3,
        delay_ms: 5_000,
        error_message: "summarizer overloaded".into(),
    });
    app.draw().unwrap();

    let sb = app.scrollback_text();
    assert!(sb.contains("summarizer overloaded"), "the error must still reach the chat:\n{sb}");
    assert!(!sb.contains("Retrying"), "countdown leaked to scrollback:\n{sb}");
    assert!(buf_text(&app).contains("Retrying (2/3)"), "retry band missing");
}
