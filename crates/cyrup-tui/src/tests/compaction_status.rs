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
        !immediately.contains("compact error") && !immediately.contains("Compacted from"),
        "the outcome must NOT have been applied by `execute_command` — that is the inline await \
         this item is about:\n{immediately}"
    );

    // …and it arrives over the channel the run loop services, alongside every other arm.
    let outcome = compact_rx.recv().await.expect("the spawned compaction must answer");
    app.apply_compact_outcome(outcome);
    app.draw().unwrap();
    let after = screen(&app);
    assert!(
        after.contains("compact error") || after.contains("Compacted from"),
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
        s.contains("compact error") || s.contains("Compacted from"),
        "the inline fallback must still render the outcome:\n{s}"
    );
}
