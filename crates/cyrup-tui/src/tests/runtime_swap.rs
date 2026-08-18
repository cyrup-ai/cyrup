//! Runtime session-swap re-binding (gap #2 / arch-11 §3.4): the TUI run loop holds an
//! `AgentSessionRuntime` and drives the session-lifecycle commands through it, then re-binds the UI
//! to the freshly-installed session. These tests prove the **driving** side (a `/new` or `/fork`
//! command calls the matching runtime op and bumps the replacement generation, invalidating the old
//! subscription with a terminal `SessionReplaced`) and the **re-bind** side (`App::rebind_session`
//! installs the new session's UI state). Mirrors Pi's interactive session-swap
//! (`agent-session-runtime.ts` `newSession`/`fork` + the run-loop re-subscribe).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use cyrup_session_svc::{
    AgentSessionEvent, AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget,
};
use crate::{App, AppCommand, SelectorKind, UiTheme};
use futures::StreamExt;
use ratatui::backend::TestBackend;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
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
    Fixture { _tmp: tmp, config }
}

async fn runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(SessionFactory::new(provider, fx.config.clone()));
    AgentSessionRuntime::create(factory, SessionTarget::New).await.unwrap()
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

/// `/new` drives `AgentSessionRuntime::new_session`, bumping the generation, invalidating the old
/// session's subscription (terminal `SessionReplaced`), and producing a distinct active session; the
/// run-loop re-bind then resets the transcript and surfaces the swap status line.
#[tokio::test]
async fn new_session_command_swaps_and_rebinds_the_ui() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();

    let session0 = rt.session().await;
    let id0 = session0.session_id().to_string();
    // The subscription the run loop holds before the swap; it must be invalidated.
    let mut old_sub = session0.subscribe();
    assert_eq!(rt.generation().await, 0);

    // Drive the command exactly as the run loop does (`AppAction::Command` arm).
    app.execute_command(AppCommand::NewSession, &session0, Some(&rt)).await;

    // The runtime swapped the active session.
    assert_eq!(rt.generation().await, 1, "/new bumps the replacement generation");
    let session1 = rt.session().await;
    assert_ne!(session1.session_id().to_string(), id0, "a fresh session is installed");

    // The old subscription is terminated with a `SessionReplaced` (R-11-021) — the run loop drops it.
    let replaced = tokio::time::timeout(Duration::from_secs(2), async {
        while let Some(ev) = old_sub.next().await {
            if matches!(ev, AgentSessionEvent::SessionReplaced { .. }) {
                return true;
            }
        }
        false
    })
    .await
    .expect("old subscription should terminate promptly");
    assert!(replaced, "the prior subscription must receive a terminal SessionReplaced");

    // Re-subscribing the new session yields a live stream (the run loop's new `events`).
    let _new_sub = session1.subscribe();

    // The run-loop re-bind installs the new session's UI state + the swap status line.
    app.rebind_session();
    app.draw().unwrap();
    let scrollback = app.scrollback_text();
    assert!(
        scrollback.contains("\u{2713} New session started"),
        "rebind surfaces the swap receipt: {scrollback}"
    );
    assert!(app.active_selector_kind().is_none(), "rebind clears any open selector");
}

/// `/fork` drives `AgentSessionRuntime::fork`, switching the runtime to the new branched session
/// (generation bump) and — for `position:"before"` — re-seeding the editor with the anchor text.
#[tokio::test]
async fn fork_command_swaps_to_the_branched_session() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();

    // Drive one turn so the session has a user-message anchor to fork at.
    let session0 = rt.session().await;
    let _ = session0.prompt("remember this").await.unwrap();
    session0.wait_for_idle().await;
    let anchors = session0.user_messages_for_forking().await;
    assert!(!anchors.is_empty(), "a user message anchor exists to fork from");
    let entry = anchors[0].entry_id.to_string();
    let anchor_text = anchors[0].text.clone();
    assert_eq!(rt.generation().await, 0);

    app.execute_command(
        AppCommand::ConfirmSelection { kind: SelectorKind::UserMessage, value: entry },
        &session0,
        Some(&rt),
    )
    .await;

    assert_eq!(rt.generation().await, 1, "/fork bumps the replacement generation");
    let session1 = rt.session().await;
    assert_ne!(
        session1.session_id().to_string(),
        session0.session_id().to_string(),
        "fork installs a distinct branched session"
    );
    // `position:"before"` re-seeds the editor with the anchor text for re-editing.
    assert_eq!(app.editor_mut().text(), anchor_text, "fork before re-seeds the editor");

    // The run-loop re-bind resets the transcript for the new session.
    app.rebind_session();
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("forked from message"),
        "rebind surfaces the fork status"
    );
}

/// Without a runtime threaded in (the SDK/embedder path), the session-lifecycle commands degrade to a
/// status line — the single fixed-session flow is unaffected (the `--print`/plain-interactive launch
/// regression guard).
#[tokio::test]
async fn no_runtime_keeps_the_single_session_flow() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();
    let session0 = rt.session().await;

    // `None` runtime → no swap, just a surfaced status line.
    app.execute_command(AppCommand::NewSession, &session0, None).await;
    app.draw().unwrap();
    assert!(app.scrollback_text().contains("starting new session"));
    assert_eq!(rt.generation().await, 0, "no runtime ⇒ no replacement");
}
