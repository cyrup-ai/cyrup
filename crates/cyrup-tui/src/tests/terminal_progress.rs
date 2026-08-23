//! **G66** — the OSC 9;4 taskbar progress indicator, end to end.
//!
//! Ports pi v0.83.0 `ProcessTerminal.setProgress` (`packages/tui/src/terminal.ts:11-13`, `:509-523`,
//! `:407-409`) driven from `interactive-mode.ts`'s five call sites (`:2865`, `:3057`, `:3076`,
//! `:3090`, `:6041`).
//!
//! # What was broken
//!
//! The `/settings` row existed and the setting was read; nothing was behind either. Cyrup contained
//! no `ESC ] 9 ; 4` sequence anywhere, so "Terminal progress" was a switch wired to a mechanism that
//! had never been built. These tests drive the two REAL user actions — cycling the `/settings` row
//! with the keyboard, and submitting a prompt — and assert the indicator arms and clears.
//!
//! **No network.** The session runs on `FauxProvider`, an in-process canned-response provider.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, AgentSessionEvent, SessionBuilder, SessionConfig};
use crate::crossterm::event::KeyCode;
use crate::{
    App, AppAction, AppCommand, SelectorKind, TerminalProgress, UiTheme,
    TERMINAL_PROGRESS_ACTIVE_SEQUENCE, TERMINAL_PROGRESS_CLEAR_SEQUENCE,
    TERMINAL_PROGRESS_KEEPALIVE,
};
use ratatui::backend::TestBackend;
use tokio_stream::StreamExt;
use tempfile::TempDir;
use super::harness::*;

struct Fixture {
    _tmp: TempDir,
    session: Arc<AgentSession>,
}

/// A real session on the faux provider, with one canned assistant turn queued.
async fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    Fixture {
        _tmp: tmp,
        session: Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap()),
    }
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

/// Open `/settings`, fuzzy-filter to the "Terminal progress" row and press Enter — the exact
/// keystroke sequence a user performs — and return the command the app decided to run.
///
/// Deliberately goes through `handle_input`, not through a hand-built `AppCommand`: the point of
/// this helper is that the row a human can see is connected to the command the app dispatches.
async fn cycle_terminal_progress_row(
    app: &mut App<TestBackend>,
    session: &Arc<AgentSession>,
) -> AppCommand {
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Settings), session, None)
        .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::Settings),
        "`/settings` must open the settings grid"
    );
    for c in "Terminal progress".chars() {
        app.handle_input(&key(KeyCode::Char(c)));
    }
    match app.handle_input(&key(KeyCode::Enter)) {
        AppAction::Command(cmd) => cmd,
        other => panic!("cycling the row must dispatch a command, got {other:?}"),
    }
}

// ================================================================ the proof

/// **The end-to-end.** The user turns "Terminal progress" on in `/settings`, then submits a prompt.
/// The indicator must arm on the turn's `agent_start` and clear on its `agent_end`.
///
/// FAILS before the fix in two independent ways: there was no `terminal_progress` state for the
/// `ApplySetting` arm to write, and no session-event arm ever recorded a transition — the whole
/// mechanism the visible row describes did not exist.
#[tokio::test]
async fn settings_row_then_a_real_prompt_arms_and_clears_the_indicator() {
    let fx = fixture().await;
    let mut app = app();

    // --- user action 1: `/settings` → "Terminal progress" → Enter.
    let cmd = cycle_terminal_progress_row(&mut app, &fx.session).await;
    assert!(
        matches!(&cmd, AppCommand::ApplySetting { id, value }
            if id == "terminal.showTerminalProgress" && value == "true"),
        "the visible row must cycle the real setting id, got {cmd:?}"
    );
    app.execute_command(cmd, &fx.session, None).await;
    assert!(
        app.state().terminal_progress.enabled(),
        "the row the user just cycled must reach the live gate, not only settings.json"
    );
    assert!(
        !app.state().terminal_progress.is_active(),
        "turning the row on must not itself light the taskbar — pi arms only from agent_start"
    );

    // --- user action 2: type a message and press enter.
    let mut events = fx.session.subscribe();
    let _ = fx.session.prompt("hi").await.unwrap();
    fx.session.wait_for_idle().await;

    // Fold the events the session ACTUALLY emitted, exactly as the run loop's `events.next()` arm
    // does, checking the indicator at each of pi's two transition points.
    let (armed_at_start, kept_through_the_turn, cleared_at_end) =
        tokio::time::timeout(Duration::from_secs(5), async {
            let (mut armed, mut kept, mut cleared) = (false, true, false);
            while let Some(ev) = events.next().await {
                let is_start = matches!(ev, AgentSessionEvent::AgentStart);
                let is_end = matches!(ev, AgentSessionEvent::AgentEnd { .. });
                app.ingest_session_event(&ev, &fx.session).await;
                if is_start {
                    armed = app.state().terminal_progress.is_active();
                    assert_eq!(
                        app.state_mut().terminal_progress.take_pending(),
                        Some(true),
                        "agent_start must park the ACTIVE sequence for the run loop to write"
                    );
                } else if is_end {
                    cleared = !app.state().terminal_progress.is_active();
                    assert_eq!(
                        app.state_mut().terminal_progress.take_pending(),
                        Some(false),
                        "agent_end must park the CLEAR sequence"
                    );
                    break;
                } else if armed {
                    // Every mid-turn event (message updates, tool rows) leaves it armed, and none of
                    // them re-writes: pi's interval is the only thing that repeats.
                    kept &= app.state().terminal_progress.is_active();
                    kept &= app.state_mut().terminal_progress.take_pending().is_none();
                }
            }
            (armed, kept, cleared)
        })
        .await
        .expect("the session's event stream should settle");

    assert!(armed_at_start, "agent_start must light the indicator (interactive-mode.ts:2865-2867)");
    assert!(
        kept_through_the_turn,
        "the indicator must stay lit, and stay un-rewritten, for the whole turn"
    );
    assert!(cleared_at_end, "agent_end must clear it (interactive-mode.ts:3057-3059)");
}

/// The gate really gates. With the row left OFF — the default — a real turn must produce no
/// transition at all, because pi never reaches `ui.terminal.setProgress`.
///
/// This is the assertion that would catch a "fix" that simply always emits the sequence.
#[tokio::test]
async fn with_the_row_off_a_real_turn_emits_nothing() {
    let fx = fixture().await;
    let mut app = app();
    assert!(!app.state().terminal_progress.enabled(), "default is off (settings.rs:779-783)");

    let mut events = fx.session.subscribe();
    let _ = fx.session.prompt("hi").await.unwrap();
    fx.session.wait_for_idle().await;

    let clean = tokio::time::timeout(Duration::from_secs(5), async {
        let mut clean = true;
        while let Some(ev) = events.next().await {
            let is_end = matches!(ev, AgentSessionEvent::AgentEnd { .. });
            app.ingest_session_event(&ev, &fx.session).await;
            clean &= app.state_mut().terminal_progress.take_pending().is_none();
            clean &= !app.state().terminal_progress.is_active();
            if is_end {
                break;
            }
        }
        clean
    })
    .await
    .expect("the session's event stream should settle");

    assert!(clean, "with the setting off no event may produce an OSC 9;4 write");
}

/// A `/compact` is its own progress window (`interactive-mode.ts:3076-3078` / `:3090-3092`) — the
/// only one with no `agent_start` around it, so it is the case a start/end-only implementation
/// silently misses.
#[tokio::test]
async fn a_compaction_is_its_own_progress_window() {
    let fx = fixture().await;
    let mut app = app();
    app.execute_command(
        AppCommand::ApplySetting {
            id: "terminal.showTerminalProgress".to_string(),
            value: "true".to_string(),
        },
        &fx.session,
        None,
    )
    .await;

    app.ingest_event(&AgentSessionEvent::CompactionStart {
        reason: cyrup_session_svc::CompactionReason::Manual,
    });
    assert!(app.state().terminal_progress.is_active(), "compaction_start arms");
    assert_eq!(app.state_mut().terminal_progress.take_pending(), Some(true));

    app.ingest_event(&AgentSessionEvent::CompactionEnd {
        reason: cyrup_session_svc::CompactionReason::Manual,
        aborted: false,
        result: None,
        will_retry: false,
        error_message: None,
    });
    assert!(!app.state().terminal_progress.is_active(), "compaction_end clears");
    assert_eq!(app.state_mut().terminal_progress.take_pending(), Some(false));
}

/// Turning the row back OFF while a turn is running takes the taskbar down with it — the documented
/// `[CYRUP-DELTA]`. Pi leaves it lit until process exit here, because its gate makes its own
/// `agent_end` clear unreachable; clearing on the disabling edge is the same sequence sent sooner.
#[tokio::test]
async fn turning_the_row_off_mid_turn_takes_the_indicator_down() {
    let fx = fixture().await;
    let mut app = app();
    let on = cycle_terminal_progress_row(&mut app, &fx.session).await;
    app.execute_command(on, &fx.session, None).await;

    app.ingest_event(&AgentSessionEvent::AgentStart);
    assert!(app.state().terminal_progress.is_active());
    app.state_mut().terminal_progress.take_pending();

    // The off half is dispatched as the command rather than harvested from a second visit to the
    // row. `settings_rows` seeds each row from `settings.effective()`, which does not pick up a
    // just-persisted write until a `/reload` (the `C::ApplySetting` arm says so in as many words),
    // so re-opening the selector would hand back the *stale* `false` row and cycle it to `true`
    // again. That staleness is a separate, pre-existing gap; the command below is byte-identical to
    // the one the row emits once it is fresh, and it is the same value the run loop dispatches.
    let off = AppCommand::ApplySetting {
        id: "terminal.showTerminalProgress".to_string(),
        value: "false".to_string(),
    };
    app.execute_command(off, &fx.session, None).await;

    assert!(!app.state().terminal_progress.enabled());
    assert!(!app.state().terminal_progress.is_active(), "the lit indicator must be taken down");
    assert_eq!(
        app.state_mut().terminal_progress.take_pending(),
        Some(false),
        "and the clear must actually be parked for the run loop to write"
    );
}

// ================================================================ mirrors

/// MIRROR — the bytes. A terminal parses OSC 9;4 positionally, so a wrong parameter is not a
/// cosmetic difference: `9;4;3` is indeterminate progress and `9;4;0` removes it. Checked against
/// `pi/packages/tui/src/terminal.ts:11-13` at v0.84.1, whose only change to that file over v0.83.0
/// was dropping the stray `;` this asserts is absent.
#[test]
fn mirror_the_wire_sequences_are_pis() {
    assert_eq!(TERMINAL_PROGRESS_ACTIVE_SEQUENCE.as_bytes(), b"\x1b]9;4;3\x07");
    assert_eq!(TERMINAL_PROGRESS_CLEAR_SEQUENCE.as_bytes(), b"\x1b]9;4;0\x07");
    assert!(
        !TERMINAL_PROGRESS_CLEAR_SEQUENCE.contains(";0;"),
        "v0.83.0's trailing `;` was removed at v0.84.1"
    );
    assert_eq!(
        TERMINAL_PROGRESS_KEEPALIVE,
        Duration::from_millis(1000),
        "pi TERMINAL_PROGRESS_KEEPALIVE_MS"
    );
}

/// MIRROR — the keepalive gate. Pi's `setInterval` exists only while progress is armed
/// (`terminal.ts:514-516` / `:525-531`), which is what the run loop's `if`-gated ticker arm
/// reproduces. An always-on ticker would write an escape to every user's terminal once a second
/// forever.
#[test]
fn mirror_the_keepalive_runs_only_between_the_two_transitions() {
    let mut p = TerminalProgress::with_enabled(true);
    assert!(!p.keepalive(), "idle");
    p.set(true);
    assert!(p.keepalive(), "armed");
    p.set(false);
    assert!(!p.keepalive(), "cleared");

    let mut off = TerminalProgress::default();
    off.set(true);
    assert!(!off.keepalive(), "the setting gates the keepalive too");
}
