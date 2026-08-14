//! Regression proof for "id-based supervisor addressing is structurally dead: cyrup never registers
//! under its own session id" — driven through the REAL production entry point
//! (`IntercomExtension::on_event(SessionStart)` → `connect::ensure_connected` → `connect_once` →
//! `IntercomClient::connect`) against a REAL broker subprocess over a REAL Unix socket.
//!
//! Upstream (`pi-intercom` `git show v0.7.0:index.ts`):
//!   * `:945-946` `currentSessionId = ctx.sessionManager.getSessionId(); publishIntercomSessionId(…)`
//!   * `:833`     `await nextClient.connect(buildRegistration(), currentSessionId);`
//!
//! i.e. a session ALWAYS registers with the broker under its own agent session id.
//!
//! Pre-fix cyrup (`connect.rs:354-357`) instead offered
//! `std::env::var("CYRUP_INTERCOM_SESSION_ID").or_else(|| last_session_id())`. That env var has zero
//! writers anywhere in the workspace and `last_session_id()` is `None` before the first successful
//! connect, so the offered id was always `None` and the broker minted a random UUID
//! (`broker/mod.rs:319-320`; upstream `broker.ts:346-352`). Nothing in cyrup ever knew that UUID, so
//! no peer could address a session by the session id it actually has.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use cyrup_ext::{ExtMode, HostCtx, HostEvent, HostServices, NativeExtension};
use cyrup_intercom::config::load_config;
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::identity::presence_name;
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::spawn::wait_for_broker;
use crate::common::{spawn_broker, within, write_broker_command};

/// The live session id the scripted host reports — deliberately NOT UUID-shaped, so a broker-minted
/// UUID can never accidentally satisfy the assertion.
const LIVE_SESSION_ID: &str = "session-9f8e7d6c5b4a3210";

/// A `HostServices` backend that reports a live session id and nothing else — the in-crate analog of
/// the session backend the builder late-binds via `load_native_with_services` → `set_host_services`
/// (P-1 Route B). `session_name()` stays `None` so the presence label falls through to pi's
/// unnamed-session alias, which is derived from the SAME id (see the mirror assertion below).
struct LiveSessionServices;

impl HostServices for LiveSessionServices {
    fn session_id(&self) -> Option<String> {
        Some(LIVE_SESSION_ID.to_string())
    }
}

/// A host with NO session attached — `HostServices::session_id()`'s `None` default. This is the
/// headless/degraded session, and it is the control for the main test.
struct HeadlessServices;

impl HostServices for HeadlessServices {}

/// Bring a session up through the production `SessionStart` path and hand back its live client.
async fn connected_client(
    agent_dir: &Path,
    services: Arc<dyn HostServices>,
) -> (Arc<IntercomExtension>, Arc<IntercomClient>) {
    let intercom_dir = intercom_dir_path(agent_dir);
    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.to_path_buf(),
            PathBuf::from("/tmp/work"),
            load_config(&intercom_dir),
            None,
        )
        .expect("build the extension"),
    );
    // Exactly how the builder binds the live session backend: `set_host_services` BEFORE `init`.
    ext.set_host_services(services);

    let ctx = HostCtx::event(ExtMode::Print, false, agent_dir.to_path_buf());
    let _ = ext.on_event(&HostEvent::SessionStart { reason: "test".to_string() }, &ctx).await;

    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state.client().is_some_and(|c| c.is_connected())).await,
        "the session connects on SessionStart"
    );
    let client = state.client().expect("a live client");
    (ext, client)
}

/// THE REGRESSION. A session whose host reports a live session id must register with the broker
/// under THAT id (pi `index.ts:833,945`).
///
/// Against the pre-fix `connect_once` this fails on the first assertion: the broker mints a random
/// UUID, so `client.session_id()` is something like `"e3f1…-…"`, never `LIVE_SESSION_ID`.
///
/// The MIRROR assertion immediately after it is the one that stays green through the revert: the
/// registration NAME has always been derived from `HostServices::session_id()`
/// (`connect::build_registration` → `presence_name`), so pre-fix the broker listed a session whose
/// human-readable alias was `subagent-chat-9f8e7d6c` while its id was an unrelated UUID — the alias
/// is supposed to BE the readable form of the id it registers under. That mirror proves the scripted
/// host really is bound and its `session_id()` really is reachable from this code path, so the
/// failing assertion above is about the registration id and nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_start_registers_under_the_live_session_id() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);

    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(20)).await.expect("broker up");

    let (_ext, client) = connected_client(agent_dir.path(), Arc::new(LiveSessionServices)).await;

    assert_eq!(
        client.session_id().as_deref(),
        Some(LIVE_SESSION_ID),
        "pi `nextClient.connect(buildRegistration(), currentSessionId)` (index.ts:833): the broker \
         session id IS the live agent session id, not a broker-minted UUID"
    );

    // MIRROR (green before AND after the fix): the presence alias was always derived from the same
    // `HostServices::session_id()`.
    let expected_alias = presence_name(None, LIVE_SESSION_ID);
    assert_eq!(expected_alias, "subagent-chat-9f8e7d6c");

    // And the broker's OWN view agrees — the id is addressable by a peer through `list`.
    let sessions = client.list_sessions().await.expect("list sessions");
    let me = sessions
        .iter()
        .find(|s| s.name.as_deref() == Some(expected_alias.as_str()))
        .expect("our session is listed under its presence alias (mirror: true pre- and post-fix)");
    assert_eq!(
        me.id, LIVE_SESSION_ID,
        "a peer listing sessions sees the real session id, so id-based addressing can resolve it"
    );

    client.disconnect();
    let _ = broker.kill().await;
}

/// CONTROL: with no live session backend (`HostServices::session_id() == None` — the headless /
/// degraded session), the broker still mints its own id, exactly as upstream's absent-`sessionId`
/// branch does (`broker.ts:346-352`, `broker/mod.rs:319-320`). The fix must not have made a
/// session-less host register under an empty or bogus id.
///
/// This also keeps the main test honest: it shows `LIVE_SESSION_ID` is not something the broker
/// would have produced on its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_headless_session_still_gets_a_broker_minted_id() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);

    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(20)).await.expect("broker up");

    let (_ext, client) = connected_client(agent_dir.path(), Arc::new(HeadlessServices)).await;

    let id = client.session_id().expect("the broker assigned an id");
    assert!(!id.trim().is_empty(), "never a blank id");
    assert_ne!(id, LIVE_SESSION_ID, "no session id to adopt, so the broker minted one");

    client.disconnect();
    let _ = broker.kill().await;
}
