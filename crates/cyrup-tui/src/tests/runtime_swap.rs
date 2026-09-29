//! Runtime session-swap re-binding (gap #2 / arch-11 §3.4): the TUI run loop holds an
//! `AgentSessionRuntime` and drives the session-lifecycle commands through it, then re-binds the UI
//! to the freshly-installed session. These tests prove the **driving** side (a `/new` or `/fork`
//! command calls the matching runtime op and bumps the replacement generation, invalidating the old
//! subscription with a terminal `SessionReplaced`) and the **re-bind** side (`App::rebind_session`
//! installs the new session's UI state). Mirrors Pi's interactive session-swap
//! (`agent-session-runtime.ts` `newSession`/`fork` + the run-loop re-subscribe).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;
use std::time::Duration;

use crate::{App, AppCommand, SelectorKind, UiTheme};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{
    AgentSessionEvent, AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget,
};
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
    // Two credentialed providers with embedded models, so every session this fixture builds has an
    // auth-filtered catalog spanning more than one provider — supplied here, not by the host.
    std::fs::write(
        agent_dir.join("auth.json"),
        r#"{"deepseek":{"type":"api_key","key":"k"},"groq":{"type":"api_key","key":"k"}}"#,
    )
    .unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    Fixture { _tmp: tmp, config }
}

async fn runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    // The ambient env tier pinned EMPTY, so the provider set is exactly the fixture's `auth.json`
    // and never the host's exported credentials.
    let auth = Arc::new(
        cyrup_config::AuthStore::at(fx.config.agent_dir.join("auth.json"))
            .with_ambient_env(std::collections::HashMap::new()),
    );
    let factory = Arc::new(SessionFactory::new(provider, fx.config.clone()).auth(auth));
    AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap()
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
    app.execute_command(AppCommand::NewSession, &session0, Some(&rt))
        .await;

    // The runtime swapped the active session.
    assert_eq!(
        rt.generation().await,
        1,
        "/new bumps the replacement generation"
    );
    let session1 = rt.session().await;
    assert_ne!(
        session1.session_id().to_string(),
        id0,
        "a fresh session is installed"
    );

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
    assert!(
        replaced,
        "the prior subscription must receive a terminal SessionReplaced"
    );

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
    assert!(
        app.active_selector_kind().is_none(),
        "rebind clears any open selector"
    );
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
    assert!(
        !anchors.is_empty(),
        "a user message anchor exists to fork from"
    );
    let entry = anchors[0].entry_id.to_string();
    let anchor_text = anchors[0].text.clone();
    assert_eq!(rt.generation().await, 0);

    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::UserMessage,
            value: entry,
        },
        &session0,
        Some(&rt),
    )
    .await;

    assert_eq!(
        rt.generation().await,
        1,
        "/fork bumps the replacement generation"
    );
    let session1 = rt.session().await;
    assert_ne!(
        session1.session_id().to_string(),
        session0.session_id().to_string(),
        "fork installs a distinct branched session"
    );
    // `position:"before"` re-seeds the editor with the anchor text for re-editing.
    assert_eq!(
        app.editor_mut().text(),
        anchor_text,
        "fork before re-seeds the editor"
    );

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
    app.execute_command(AppCommand::NewSession, &session0, None)
        .await;
    app.draw().unwrap();
    assert!(app.scrollback_text().contains("starting new session"));
    assert_eq!(rt.generation().await, 0, "no runtime ⇒ no replacement");
}

/// **TUI-105 — the SESSION-REBIND leg of the provider recount.**
///
/// Upstream `rebindCurrentSession` ends with
/// `await this.updateAvailableProviderCount(); this.updateEditorBorderColor(); this.updateTerminalTitle();`
/// (`interactive-mode.ts:2040-2043` @v0.87.1). That function is the hook `setRebindSession` registers
/// (`:573-574`), so the recount happens on EVERY `/reload`, `/new`, `/resume`, `/fork` and `/import` —
/// not only on a login, a `/logout`, a `/model` or a `/scoped-models`.
///
/// The count is a property of the SWAPPED-IN session: `/resume` of a session recorded under a
/// different agent dir reads a different `auth.json` (the reason the arm already re-reads the auth
/// snapshot) and `/reload` re-reads `scopedModels` from rebuilt settings. So the arm is DRIVEN here
/// rather than source-read: the two existing `session_swapped` tests
/// (`tests/startup_resources_panel.rs`, `tests/theme_reapply_on_reload.rs`) assert its ordering out
/// of `include_str!`, which cannot distinguish a recount of the new session's providers from a
/// recount of the old one's.
///
/// The pre-swap count is seeded through `refresh_provider_count` against the OUTGOING session, whose
/// scoped set pins it to one provider (`:5094-5095`, the scoped branch); the swapped-in session
/// carries no scoped set, so its own count is the auth-filtered snapshot's
/// (`getAvailableSnapshot()`, `:5096`) and the two differ.
///
/// **Red without the change:** `App::on_session_swapped` never called `refresh_provider_count`, so
/// the footer's `provider_count > 1` gate (`status.rs:597`) — the one that decides whether the model
/// is prefixed with `(provider)` — kept answering from the outgoing session's provider set for the
/// rest of the process.
#[tokio::test]
async fn tui105_a_session_swap_recounts_providers_from_the_swapped_in_session() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = inline_app();

    let session0 = rt.session().await;
    // Pin the OUTGOING session to a single provider through pi's scoped branch, so the pre-swap
    // count is a real count of a real session and not a sentinel.
    let one = session0
        .available_model_catalog()
        .into_iter()
        .next()
        .expect("the fixture session has a model catalog");
    session0.set_scoped_models(vec![cyrup_session_svc::ScopedModel {
        model: one,
        thinking_level: None,
    }]);
    app.refresh_provider_count(&session0);
    let before = app.state().status.provider_count;
    assert_eq!(before, 1, "one scoped provider ⇒ one (`:5094-5095`)");

    // Swap: `/new` installs a fresh session, which carries no scoped set.
    app.execute_command(AppCommand::NewSession, &session0, Some(&rt))
        .await;
    let session1 = rt.session().await;
    assert!(
        session1.scoped_models().is_empty(),
        "the swapped-in session must fall to the auth-filtered snapshot branch"
    );
    let expected: std::collections::BTreeSet<String> = session1
        .available_model_catalog()
        .iter()
        .map(|m| m.provider.as_str().to_string())
        .collect();
    assert_ne!(
        expected.len(),
        before,
        "the fixture must make the swap observable: the swapped-in session's provider set has to \
         differ from the outgoing session's scoped one"
    );

    // Drive the run loop's `session_swapped` arm exactly as `App::run`'s `select!` does.
    let mut events = session0.subscribe();
    let mut ctx = run_ctx(session0, &rt);
    app.on_session_swapped(&mut ctx, &mut events)
        .await
        .expect("the swap arm must not fail");

    assert_eq!(
        app.state().status.provider_count,
        expected.len(),
        "`rebindCurrentSession` recounts from the swapped-in session (`interactive-mode.ts:2042`); \
         without that call the footer keeps the outgoing session's {before}"
    );
}

/// `App::run`'s own backend. The `session_swapped` arm is `impl App<InlineBackend<TuiStdout>>`, so
/// the arm cannot be driven through the `TestBackend` the rest of this file uses.
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

/// The run loop's context, built exactly as `App::run` builds it: every field is a channel, a timer
/// or a handle the arm under test needs live. Its receivers are dropped with the returned value's
/// senders, which is fine — the arm only ever SENDS on them.
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
