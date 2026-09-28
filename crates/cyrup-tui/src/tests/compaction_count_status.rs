//! TUI-003 — a resumed or swapped-in session reports how many times it was compacted.
//!
//! ```ts
//! // pi v0.87.1 coding-agent/src/modes/interactive/interactive-mode.ts:4056-4071
//! renderInitialMessages(): void {
//!     const entries = this.sessionManager.buildContextEntries();
//!     this.renderSessionEntries(entries, { updateFooter: true, populateHistory: true });
//!     this.renderProjectTrustWarningIfNeeded();
//!
//!     // Show compaction info if session was compacted
//!     const allEntries = this.sessionManager.getEntries();
//!     const compactionCount = allEntries.filter((e) => e.type === "compaction").length;
//!     if (compactionCount > 0) {
//!         const times = compactionCount === 1 ? "1 time" : `${compactionCount} times`;
//!         this.showStatus(`Session compacted ${times}`);
//!     }
//! }
//! ```
//!
//! cyrup's two `renderInitialMessages()` sites — the boot seed (`App::seed_session_ui`, after
//! `crates/cyrup/src/interactive.rs` has replayed a `--resume`/`--continue` branch) and the run
//! loop's `session_swapped` arm (`/resume`, `/fork`, `/import`, `/new`) — replayed and raised the
//! trust banner but never counted compactions. Both are driven here through their real entry points
//! and read off the transcript's pending entries, which is what the next frame commits.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use crate::transcript::Entry;
use crate::{App, AppCommand, UiTheme};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    config: SessionConfig,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    Fixture { tmp, config }
}

async fn runtime(fx: &Fixture, target: SessionTarget) -> Arc<AgentSessionRuntime> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let factory = Arc::new(SessionFactory::new(provider, fx.config.clone()));
    AgentSessionRuntime::create(factory, target).await.unwrap()
}

/// A pi session file whose CURRENT branch carries one compaction and whose ABANDONED branch carries
/// `abandoned` more — `getEntries()` counts every one of them, `buildContextEntries()` only the
/// first, so the status tells the two readings apart.
fn session_file(dir: &Path, cwd: &Path, abandoned: usize) -> PathBuf {
    let ts = "2026-01-01T00:00:00.000Z";
    let mut lines = vec![
        format!(
            r#"{{"type":"session","version":3,"id":"0199aaaa-bbbb-7ccc-8ddd-eeeeffff0001","timestamp":"{ts}","cwd":{cwd}}}"#,
            cwd = serde_json::to_string(&cwd.to_string_lossy()).unwrap()
        ),
        format!(
            r#"{{"type":"message","id":"u1","parentId":null,"timestamp":"{ts}","message":{{"role":"user","content":"first question","timestamp":1}}}}"#
        ),
    ];
    for i in 0..abandoned {
        lines.push(format!(
            r#"{{"type":"compaction","id":"x{i}","parentId":"u1","timestamp":"{ts}","summary":"abandoned summary {i}","firstKeptEntryId":"u1","tokensBefore":100}}"#
        ));
    }
    lines.push(format!(
        r#"{{"type":"compaction","id":"c1","parentId":"u1","timestamp":"{ts}","summary":"kept summary","firstKeptEntryId":"u1","tokensBefore":100}}"#
    ));
    lines.push(format!(
        r#"{{"type":"message","id":"u2","parentId":"c1","timestamp":"{ts}","message":{{"role":"user","content":"after the compaction","timestamp":2}}}}"#
    ));
    let path = dir.join("resumed.jsonl");
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}

/// `App::run`'s own backend: the boot seed and the swap arm are `impl App<InlineBackend<…>>`.
fn inline_app() -> App<crate::InlineBackend<crate::TuiStdout>> {
    App::new(
        crate::InlineBackend::with_anchor(
            crate::write_log::tui_stdout(),
            ratatui::layout::Position::ORIGIN,
        ),
        UiTheme::dark(),
    )
    .unwrap()
}

/// The run loop's context, built as `App::run` builds it (see `runtime_swap.rs`).
fn run_ctx(
    session: Arc<cyrup_session_svc::AgentSession>,
    rt: &Arc<AgentSessionRuntime>,
) -> crate::app::RunCtx {
    let mut spinner = tokio::time::interval(Duration::from_millis(80));
    spinner.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    crate::app::RunCtx {
        session,
        runtime: Some(Arc::clone(rt)),
        cancel: cyrup_core::CancelToken::new(),
        gen_rx: Some(rt.watch_generation()),
        spinner,
        overlay_tick: None,
        bash_rx: None,
        package_update_rx: None,
        ui_tx: tokio::sync::mpsc::unbounded_channel().0,
        ui_effect_tx: tokio::sync::mpsc::unbounded_channel().0,
        ext_error_tx: tokio::sync::mpsc::unbounded_channel().0,
        commands_changed_tx: tokio::sync::mpsc::unbounded_channel().0,
        overlay_tx: tokio::sync::mpsc::unbounded_channel().0,
        theme_switch_tx: tokio::sync::mpsc::unbounded_channel().0,
        shortcut_status_tx: tokio::sync::mpsc::unbounded_channel().0,
    }
}

fn statuses(app: &App<crate::InlineBackend<crate::TuiStdout>>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Status(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// Drive the `session_swapped` arm after `/resume`-ing `path`.
async fn resume_through_swap_arm(
    fx: &Fixture,
    path: &Path,
) -> App<crate::InlineBackend<crate::TuiStdout>> {
    let rt = runtime(fx, SessionTarget::New).await;
    let session0 = rt.session().await;
    rt.switch_session(path).await.unwrap();
    let mut app = inline_app();
    let mut events = session0.subscribe();
    let mut ctx = run_ctx(session0, &rt);
    app.on_session_swapped(&mut ctx, &mut events)
        .await
        .expect("the swap arm must not fail");
    app
}

/// The row's scenario on the `/resume` path, and the whole-file count: two compactions on the file,
/// one of them on an abandoned branch, is pi's `2 times` — not the `1` the branch projection holds.
#[tokio::test]
async fn a_resumed_session_reports_every_compaction_in_the_file() {
    let fx = fixture();
    let path = session_file(fx.tmp.path(), &fx.config.cwd, 1);
    let app = resume_through_swap_arm(&fx, &path).await;

    let pending = app.state().transcript.pending();
    let at = pending
        .iter()
        .position(|e| matches!(e, Entry::Status(s) if s == "Session compacted 2 times"))
        .unwrap_or_else(|| panic!("no `Session compacted 2 times` status: {pending:?}"));
    // `renderInitialMessages()` shows the status AFTER the replayed conversation.
    let replayed_user = pending
        .iter()
        .position(
            |e| matches!(e, Entry::User { text, .. } if text.contains("after the compaction")),
        )
        .unwrap_or_else(|| panic!("the resumed branch was not replayed: {pending:?}"));
    assert!(
        replayed_user < at,
        "the count follows the replay (`:4057-4069`): {pending:?}"
    );
}

/// `compactionCount === 1 ? "1 time" : …` (`:4068`) — the singular.
#[tokio::test]
async fn a_single_compaction_is_reported_in_the_singular() {
    let fx = fixture();
    let path = session_file(fx.tmp.path(), &fx.config.cwd, 0);
    let app = resume_through_swap_arm(&fx, &path).await;
    assert!(
        statuses(&app)
            .iter()
            .any(|s| s == "Session compacted 1 time"),
        "{:?}",
        statuses(&app)
    );
}

/// `if (compactionCount > 0)` (`:4067`) — `/new` swaps in a session with no compactions and says
/// nothing about them.
#[tokio::test]
async fn a_fresh_session_reports_no_compaction() {
    let fx = fixture();
    let rt = runtime(&fx, SessionTarget::New).await;
    let session0 = rt.session().await;
    let mut app = inline_app();
    app.execute_command(AppCommand::NewSession, &session0, Some(&rt))
        .await;
    let mut events = session0.subscribe();
    let mut ctx = run_ctx(session0, &rt);
    app.on_session_swapped(&mut ctx, &mut events)
        .await
        .expect("the swap arm must not fail");
    assert!(
        !statuses(&app)
            .iter()
            .any(|s| s.starts_with("Session compacted")),
        "{:?}",
        statuses(&app)
    );
}

/// The boot half: a `--resume` launch replays in `crates/cyrup/src/interactive.rs` and then
/// `App::run` seeds the UI — where `renderInitialMessages()`'s tail has to land.
#[tokio::test]
async fn a_resume_boot_reports_the_compaction_count() {
    let fx = fixture();
    let path = session_file(fx.tmp.path(), &fx.config.cwd, 2);
    let rt = runtime(&fx, SessionTarget::Resume(path)).await;
    let session = rt.session().await;
    let mut app = inline_app();
    app.seed_session_ui(&session, Some(&rt)).await;
    assert!(
        statuses(&app)
            .iter()
            .any(|s| s == "Session compacted 3 times"),
        "{:?}",
        statuses(&app)
    );
}
