//! ICOM-061 — `/alias` renames the session AND pushes the new name to intercom peers at once.
//!
//! Upstream (`pi-intercom` v0.14.0, added by `90e6ad4`, v0.13.0 #126):
//!
//! ```text
//! // index.ts:2911-2914
//! pi.registerCommand("alias", {
//!   description: "Set the current session alias (usage: /alias <name> or /alias menu)",
//!   handler: async (args, ctx) => setIntercomAlias(args, ctx),
//! });
//! ```
//!
//! `setIntercomAlias` (`index.ts:2779-2830`) calls `pi.setSessionName(alias)` and then
//! `syncPresenceIdentity(…)` — "Push the new identity directly so broker peers see the alias without
//! waiting for the idle name poll." Bare `/alias` / `/alias menu` open `ui.input` with a UI; without
//! one, bare `/alias` reports the current alias and `/alias menu` warns.
//!
//! Every test drives the real `NativeExtension::execute_command` against a real broker subprocess.
//! The rename test STOPS the name poll after `SessionStart`, so the only path that can carry the new
//! name to the peer is the command's own push — the immediacy is what is being proved.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::{registration, spawn_broker, within, write_broker_command};
use cyrup_ext::{DialogOptions, ExtMode, HostCtx, HostEvent, HostServices, NativeExtension};
use cyrup_intercom::config::load_config;
use cyrup_intercom::extension::{ALIAS_COMMAND, IntercomExtension};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::spawn::wait_for_broker;

const MY_SESSION_ID: &str = "session-a11a5a11a5a11a50";

/// The live session backend's rename seam: `session_name` reads back what `set_session_name` last
/// wrote (the live `LiveHostServices` routes both to the session tree), plus a scripted `input`
/// dialog and a notification recorder.
#[derive(Default)]
struct AliasHost {
    name: Mutex<Option<String>>,
    /// What the next `input` returns (`None` = the user dismissed the dialog).
    answer: Mutex<Option<String>>,
    /// Every `(title, placeholder)` the dialog was opened with.
    prompts: Mutex<Vec<(String, Option<String>)>>,
    notified: Mutex<Vec<(String, cyrup_ext::NotifyKind)>>,
    /// Drop every rename, as the live backend does when a turn holds the session manager
    /// (`LiveHostServices::set_session_name` → `with_manager`'s `try_lock` → "session busy").
    busy: AtomicBool,
}

impl AliasHost {
    fn named(name: Option<&str>) -> Arc<Self> {
        let host = Self::default();
        *host.name.lock().unwrap() = name.map(str::to_string);
        Arc::new(host)
    }
    fn answering(self: &Arc<Self>, answer: Option<&str>) {
        *self.answer.lock().unwrap() = answer.map(str::to_string);
    }
    fn name(&self) -> Option<String> {
        self.name.lock().unwrap().clone()
    }
}

impl HostServices for AliasHost {
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
    fn session_name(&self) -> Option<String> {
        self.name()
    }
    fn set_session_name(&self, name: &str) {
        if self.busy.load(Ordering::SeqCst) {
            return;
        }
        *self.name.lock().unwrap() = Some(name.to_string());
    }
    fn input(&self, prompt: &str, placeholder: Option<&str>, _: &DialogOptions) -> Option<String> {
        self.prompts
            .lock()
            .unwrap()
            .push((prompt.to_string(), placeholder.map(str::to_string)));
        self.answer.lock().unwrap().clone()
    }
    fn notify(&self, message: &str, kind: cyrup_ext::NotifyKind) {
        self.notified
            .lock()
            .unwrap()
            .push((message.to_string(), kind));
    }
}

struct Live {
    ext: Arc<IntercomExtension>,
    ctx: HostCtx,
    broker: tokio::process::Child,
    socket: PathBuf,
}

impl Live {
    async fn stop(mut self) {
        if let Some(c) = self.ext.state().client() {
            c.disconnect();
        }
        let _ = self.broker.kill().await;
    }
}

/// A real session on a real broker, with the name poll STOPPED once connected.
async fn live_session(agent_dir: &Path, host: Arc<AliasHost>, has_ui: bool) -> Live {
    let intercom_dir = intercom_dir_path(agent_dir);
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);
    let broker = spawn_broker(agent_dir);
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");

    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.to_path_buf(),
            PathBuf::from("/tmp/work"),
            load_config(&intercom_dir).expect("config loads"),
            None,
        )
        .expect("build the extension"),
    );
    ext.set_host_services(host);
    let mode = if has_ui { ExtMode::Tui } else { ExtMode::Print };
    let ctx = HostCtx::command(mode, has_ui, agent_dir.to_path_buf());
    let _ = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state
            .client()
            .is_some_and(|c| c.is_connected()))
        .await,
        "the session connects on SessionStart"
    );
    // Take the idle name poll (ICOM-006) out of the picture: from here on a rename reaches a peer
    // only if the command pushes it.
    state.stop_name_poll();
    Live {
        ext,
        ctx,
        broker,
        socket,
    }
}

/// The name a peer's `list` shows for [`MY_SESSION_ID`], polled until `expected` or the budget ends.
async fn peer_sees_name(peer: &IntercomClient, expected: &str, budget: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + budget;
    let mut last = None;
    loop {
        let sessions = peer.list_sessions().await.expect("peer list");
        last = sessions
            .iter()
            .find(|s| s.id == MY_SESSION_ID)
            .and_then(|s| s.name.clone())
            .or(last);
        if last.as_deref() == Some(expected) || tokio::time::Instant::now() >= deadline {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// THE FIX. `/alias Planner` renames the session and the peer sees `Planner` with the name poll
/// stopped. Pre-fix `execute_command` returned "native extension has no handler for command
/// `alias`" and the command was never registered.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn slash_alias_renames_the_session_and_pushes_the_name_to_peers_at_once() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let host = AliasHost::named(None);
    let live = live_session(agent_dir.path(), host.clone(), false).await;
    let peer = IntercomClient::connect(&live.socket, registration("peer"), None)
        .await
        .expect("peer connects");

    let reply = live
        .ext
        .execute_command(ALIAS_COMMAND, "  Planner  ", &live.ctx)
        .await
        .expect("`/alias` is a registered command with a handler");
    // `notifyAliasCommand(…, `Session alias set: ${alias}`, "info", …)` (`index.ts:2829`), on the
    // headless channel — the trimmed alias.
    assert_eq!(reply.as_deref(), Some("Session alias set: Planner"));
    // `pi.setSessionName(alias)` (`index.ts:2819`).
    assert_eq!(host.name().as_deref(), Some("Planner"));
    // `syncPresenceIdentity(…)` (`index.ts:2828`): with the poll stopped, only the push can do this.
    assert_eq!(
        peer_sees_name(&peer, "Planner", Duration::from_secs(5))
            .await
            .as_deref(),
        Some("Planner"),
        "the peer sees the alias without the name poll"
    );

    peer.disconnect();
    live.stop().await;
}

/// Without a UI, bare `/alias` REPORTS the alias and `/alias menu` warns; neither renames
/// (`index.ts:2786-2796`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bare_alias_without_a_ui_reports_and_the_menu_warns() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let host = AliasHost::named(Some("   "));
    let live = live_session(agent_dir.path(), host.clone(), false).await;
    let run = |args: &'static str| {
        let ext = live.ext.clone();
        let ctx = live.ctx.clone();
        async move {
            ext.execute_command(ALIAS_COMMAND, args, &ctx)
                .await
                .expect("dispatches")
        }
    };

    // A blank name is no alias (`pi.getSessionName()?.trim()` is falsy).
    assert_eq!(
        run("").await.as_deref(),
        Some("No session alias set. Use /alias <name>.")
    );
    assert_eq!(
        run(" MENU ").await.as_deref(),
        Some("The alias menu requires an interactive UI; use /alias <name>.")
    );
    assert_eq!(host.name().as_deref(), Some("   "), "neither form renames");

    *host.name.lock().unwrap() = Some(" worker: fix auth ".to_string());
    assert_eq!(
        run("").await.as_deref(),
        Some("Session alias: worker: fix auth")
    );
    assert!(
        host.prompts.lock().unwrap().is_empty(),
        "no dialog without a UI"
    );

    live.stop().await;
}

/// With a UI, bare `/alias` opens the input dialog (`index.ts:2798-2815`): its answer is trimmed and
/// applied, a dismissal ends silently, and a blank answer warns at `warning` level.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bare_alias_with_a_ui_asks_for_the_alias() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let host = AliasHost::named(Some("Planner"));
    let live = live_session(agent_dir.path(), host.clone(), true).await;
    let peer = IntercomClient::connect(&live.socket, registration("peer"), None)
        .await
        .expect("peer connects");

    host.answering(Some("  Reviewer "));
    let reply = live
        .ext
        .execute_command(ALIAS_COMMAND, "menu", &live.ctx)
        .await
        .expect("dispatches");
    assert_eq!(reply.as_deref(), Some("Session alias set: Reviewer"));
    assert_eq!(host.name().as_deref(), Some("Reviewer"));
    assert_eq!(
        host.prompts.lock().unwrap().clone(),
        vec![(
            "Set session alias".to_string(),
            Some("Current alias: Planner".to_string())
        )],
        "`ui.input(\"Set session alias\", currentAlias ? `Current alias: …` : …)`"
    );
    assert_eq!(
        peer_sees_name(&peer, "Reviewer", Duration::from_secs(5))
            .await
            .as_deref(),
        Some("Reviewer")
    );

    // Dismissed: `if (entered === undefined) return;` — nothing said, nothing renamed.
    host.answering(None);
    let reply = live
        .ext
        .execute_command(ALIAS_COMMAND, "", &live.ctx)
        .await
        .expect("dispatches");
    assert_eq!(reply, None);
    assert_eq!(host.name().as_deref(), Some("Reviewer"));

    // Blank: `"Session alias cannot be empty."` at `warning`, which a UI shows at its own level.
    host.answering(Some("   "));
    let reply = live
        .ext
        .execute_command(ALIAS_COMMAND, "", &live.ctx)
        .await
        .expect("dispatches");
    assert_eq!(reply, None, "a warning is notified, not returned at Info");
    assert_eq!(
        host.notified.lock().unwrap().clone(),
        vec![(
            "Session alias cannot be empty.".to_string(),
            cyrup_ext::NotifyKind::Warning
        )]
    );
    assert_eq!(host.name().as_deref(), Some("Reviewer"));

    peer.disconnect();
    live.stop().await;
}

/// A rename the session does not take is upstream's `catch` arm (`"Unable to set session alias:
/// …"`, `index.ts:2821-2826`), not a success: `set_session_name` has no result, and the live backend
/// drops a rename while a turn holds the session. Pre-fix the command answered "Session alias set"
/// and pushed the OLD name to peers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rename_the_session_does_not_take_is_reported_not_claimed() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let host = AliasHost::named(Some("Planner"));
    host.busy.store(true, Ordering::SeqCst);
    let live = live_session(agent_dir.path(), host.clone(), false).await;
    let peer = IntercomClient::connect(&live.socket, registration("peer"), None)
        .await
        .expect("peer connects");

    let reply = live
        .ext
        .execute_command(ALIAS_COMMAND, "Reviewer", &live.ctx)
        .await
        .expect("dispatches");
    assert_eq!(
        reply.as_deref(),
        Some(
            "Unable to set session alias: the session did not accept the rename (it may be busy); \
             try again"
        ),
        "headless, the error arm is the command's answer"
    );
    assert_eq!(host.name().as_deref(), Some("Planner"));
    assert_eq!(
        peer_sees_name(&peer, "Reviewer", Duration::from_millis(500))
            .await
            .as_deref(),
        Some("Planner"),
        "peers keep the name the session actually has"
    );

    // Once the session is free the same command lands.
    host.busy.store(false, Ordering::SeqCst);
    let reply = live
        .ext
        .execute_command(ALIAS_COMMAND, "Reviewer", &live.ctx)
        .await
        .expect("dispatches");
    assert_eq!(reply.as_deref(), Some("Session alias set: Reviewer"));

    peer.disconnect();
    live.stop().await;
}
