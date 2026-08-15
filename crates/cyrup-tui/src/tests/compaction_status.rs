//! TUI-055 — the compaction status band must actually reach the screen.
//!
//! **The band itself was never the bug.** `tests/status_indicator.rs` already pins that a
//! `compaction_start` event arms `IndicatorKind::Compaction` and that the band paints
//! `Compacting context...`, and it has been green throughout. Yet a manual `/compact` measured live
//! on 2026-08-13 — sampled every 200 ms across a 10.5 s compaction, no keys sent — showed an
//! **empty** status band in every single sample (`docs/gap-analysis/REPRO-LOG.md`).
//!
//! The cause was one layer up, in the run loop rather than in any component:
//!
//! ```text
//! AppAction::Command(cmd) => { self.execute_command(cmd, &session, runtime).await; }
//! ```
//!
//! `C::Compact` awaited `session.compact(...)` — a 10–20 s provider call — inside that `select!`
//! arm. A single tokio task cannot reach a sibling arm while one arm is pending, so for the whole
//! operation the `compaction_start` event sat unread in `events`, `IndicatorKind::Compaction` was
//! never armed, and the 80 ms spinner arm never fired. Pi keeps its `CompactionStatusIndicator` on
//! screen for the entire operation (`interactive-mode.ts:3075-3087`) because its await yields to the
//! same event loop that renders.
//!
//! So the property this file pins is the one the live run measured: **`/compact` must not be awaited
//! on the run loop's task.** With the channel installed, `execute_command` returns having mutated
//! nothing, and the outcome arrives separately — which is what leaves every other `select!` arm,
//! including the event stream and the spinner, free to run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;

use cyrup_provider::faux::FauxProvider;
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, AgentSessionEvent, CompactionReason, SessionBuilder, SessionConfig};
use crate::{App, AppCommand, UiTheme};
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap()
}

fn screen(app: &App<TestBackend>) -> String {
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

async fn session(dir: &std::path::Path) -> Arc<AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    Arc::new(SessionBuilder::new(faux, cfg).build().await.unwrap())
}

/// RED before this change: `execute_command` awaited the compaction inline, so by the time it
/// returned the outcome had ALREADY been rendered into the transcript — and, in the live binary,
/// every other `select!` arm had been starved for the whole call.
#[tokio::test]
async fn compact_does_not_run_on_the_run_loops_task() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path()).await;
    let mut app = new_app();
    // What `App::run` does once at startup.
    let mut compact_rx = app.install_compact_channel();

    app.execute_command(AppCommand::Compact(None), &session, None).await;

    // The spawned task holds no reference to `AppState`, so this is deterministic rather than a
    // race: nothing the compaction produces can have been applied yet.
    app.draw().unwrap();
    let immediately = screen(&app);
    assert!(
        !immediately.contains("Compaction failed") && !immediately.contains("Compacted from"),
        "the outcome must NOT have been applied by `execute_command` — that is the inline await \
         this item is about:\n{immediately}"
    );

    // …and it arrives over the channel the run loop services, alongside every other arm.
    let outcome = compact_rx.recv().await.expect("the spawned compaction must answer");
    app.apply_compact_outcome(outcome);
    app.draw().unwrap();
    let after = screen(&app);
    assert!(
        after.contains("Compaction failed") || after.contains("Compacted from"),
        "applying the outcome renders exactly what the inline path used to:\n{after}"
    );
}

/// The other half of the item: with the loop free, the `compaction_start` event that the loop can
/// now read arms the band, and it stays armed until `compaction_end`.
#[tokio::test]
async fn the_band_is_armed_for_the_whole_compaction_window() {
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::CompactionStart { reason: CompactionReason::Manual });
    // Every frame in the window — the live run sampled 50 of them and found the band empty in all.
    for _ in 0..5 {
        app.draw().unwrap();
        let s = screen(&app);
        assert!(s.contains("Compacting context..."), "band missing mid-compaction:\n{s}");
        // SESS-040's Verify line: the band does not merely appear, it must carry the affordance it
        // advertises — `Compacting context... (${keyText("app.interrupt")} to cancel)`
        // (`status-indicator.ts:78-82`), built from the LIVE keymap. Asserting only the message
        // would pass while the advertised half was missing, which is the failure mode this whole
        // item is about.
        assert!(
            s.contains("Compacting context... (escape to cancel)"),
            "the band must advertise the cancel key it now actually honours:\n{s}"
        );
    }
    app.ingest_event(&AgentSessionEvent::CompactionEnd {
        reason: CompactionReason::Manual,
        result: None,
        aborted: false,
        will_retry: false,
        error_message: None,
    });
    app.draw().unwrap();
    assert!(!screen(&app).contains("Compacting context..."), "the band clears at compaction_end");
}

/// The no-run-loop fallback stays correct: an embedder (or a test) driving `execute_command`
/// directly with no channel installed still gets the outcome rendered, inline, exactly as before.
#[tokio::test]
async fn without_a_channel_the_outcome_is_still_applied_inline() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path()).await;
    let mut app = new_app();

    app.execute_command(AppCommand::Compact(None), &session, None).await;

    app.draw().unwrap();
    let s = screen(&app);
    assert!(
        s.contains("Compaction failed") || s.contains("Compacted from"),
        "the inline fallback must still render the outcome:\n{s}"
    );
}

/// TUI-054 — a failed or cancelled compaction must never be announced as a success.
///
/// RED at HEAD: the arm was `AgentSessionEvent::CompactionEnd { .. }` — every field discarded —
/// ending in an unconditional `push_status("compaction complete")`, so all three of these frames
/// contained that line. Observed live three times, twice immediately after an error blob
/// (`compact error: Nothing to compact (session too small)` and an `http 400` from the
/// summarization provider) — see `docs/gap-analysis/REPRO-LOG.md`.
///
/// pi's `case "compaction_end"` (`interactive-mode.ts:3089-3123` @v0.83.0) branches on `aborted`,
/// `result` and `errorMessage` and states success in words nowhere.
#[tokio::test]
async fn a_failed_or_cancelled_compaction_is_not_announced_as_complete() {
    // (a) An automatic compaction that was cancelled reads pi's `Auto-compaction cancelled`.
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::CompactionEnd {
        reason: CompactionReason::Threshold,
        result: None,
        aborted: true,
        will_retry: false,
        error_message: None,
    });
    app.draw().unwrap();
    let s = screen(&app);
    assert!(!s.contains("compaction complete"), "a cancelled compaction claimed success:\n{s}");
    assert!(s.contains("Auto-compaction cancelled"), "pi's cancel copy is missing:\n{s}");

    // (b) A failure renders its own message, not a success line.
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::CompactionEnd {
        reason: CompactionReason::Overflow,
        result: None,
        aborted: false,
        will_retry: false,
        error_message: Some("summarization failed: http 400".to_string()),
    });
    app.draw().unwrap();
    let s = screen(&app);
    assert!(!s.contains("compaction complete"), "a failed compaction claimed success:\n{s}");
    assert!(s.contains("summarization failed"), "the failure message is not rendered:\n{s}");

    // (c) A manual compaction is rendered by the command path (`apply_compact_outcome`), so the
    // event arm must stay silent rather than adding a second, contradictory line.
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::CompactionEnd {
        reason: CompactionReason::Manual,
        result: None,
        aborted: false,
        will_retry: false,
        error_message: Some("Nothing to compact (session too small)".to_string()),
    });
    app.draw().unwrap();
    let s = screen(&app);
    assert!(!s.contains("compaction complete"), "manual failure claimed success:\n{s}");

    // (d) SESS-040 — and the manual path that DOES render must not call the user's own cancel an
    // error. pi's manual branches are `showError("Compaction cancelled")` when `aborted`
    // (`interactive-mode.ts:3099-3100`) and `showError(errorMessage)` otherwise (`:3116-3117`),
    // where `errorMessage` is the `Compaction failed: …` its catch builds
    // (`agent-session.ts:1908-1917`). cyrup rendered both as the dim status line
    // `compact error: …`, so pressing the Escape the band advertises reported the cancel as an
    // error, in a channel pi never uses for either branch.
    let mut app = new_app();
    app.apply_compact_outcome(Err("Compaction cancelled".to_string()));
    app.draw().unwrap();
    let s = screen(&app);
    assert!(s.contains("Compaction cancelled"), "pi's bare cancel copy is missing:\n{s}");
    assert!(
        !s.contains("compact error"),
        "a deliberate cancel must not be prefixed as an error:\n{s}"
    );

    // …and a genuine manual failure keeps pi's wrapper, which cyrup already emits verbatim on the
    // event — this path was the one that disagreed with it.
    let mut app = new_app();
    app.apply_compact_outcome(Err("Nothing to compact (session too small)".to_string()));
    app.draw().unwrap();
    let s = screen(&app);
    assert!(
        s.contains("Compaction failed: Nothing to compact (session too small)"),
        "pi's manual failure copy (agent-session.ts:1908-1917) is missing:\n{s}"
    );
    assert!(!s.contains("compact error"), "the cyrup-only prefix is still rendered:\n{s}");
}
