//! ICOM-064 — `intercom:session-identity`: another extension gives ONE session a fixed intercom id,
//! ahead of `CYRUP_INTERCOM_STABLE_ID` / `stableId`, while its name stays a readable label.
//!
//! Upstream (`pi-intercom` v0.14.0, `eae462a` #135): at `session_start` the extension emits
//! `INTERCOM_SESSION_IDENTITY_EVENT` with `{ version: 1, claim(stableId) }` on the session's own bus
//! (`index.ts:1645-1652`) and registers under `claimedIntercomSessionId ??
//! resolveConfiguredIntercomSessionId(…)` (`:1653`). Its test
//! (`intercom.integration.test.ts`, "a session identity claim sets the intercom id and keeps the
//! readable session name") claims `" subagent-worker-run1-1 "` over a process-wide stable id and
//! asserts the peer sees that id under the session's own name.
//!
//! cyrup's port is the same synchronous shape: the request is a typed
//! `IntercomSessionIdentityRequestV1` emitted with `HostServices::emit_typed_event`, which runs
//! every native claimant inline, so the id is settled before the session registers anywhere.
//!
//! The first test is the whole production path: a real `AgentSession` whose host bus carries the
//! request from the intercom extension to a second native extension, against a real broker. The
//! second drives `IntercomExtension::on_event(SessionStart)` over a backend whose typed emit claims,
//! to pin upstream's `??=` and the per-runtime reset.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::{broker_bin, registration, spawn_broker, within, write_broker_command};
use cyrup_core::ExtensionId;
use cyrup_ext::{
    ExtError, ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_intercom::config::{config_path, load_config};
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::identity::{INTERCOM_SESSION_IDENTITY_EVENT, IntercomSessionIdentityRequestV1};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::spawn::wait_for_broker;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AppMode, SessionBuilder, SessionConfig};

/// A subagent-launcher stand-in: claims inside the emit, exactly as upstream's
/// `pi.events.on(INTERCOM_SESSION_IDENTITY_EVENT, (request) => request.claim(" subagent-worker-run1-1 "))`.
#[derive(Default)]
struct Claimant {
    requests: Mutex<usize>,
}

#[async_trait::async_trait]
impl NativeExtension for Claimant {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("identity-claimant")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe_typed_bus(INTERCOM_SESSION_IDENTITY_EVENT);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    fn on_typed_bus_event(&self, topic: &str, event: &dyn std::any::Any) {
        if topic == INTERCOM_SESSION_IDENTITY_EVENT
            && let Some(request) = event.downcast_ref::<IntercomSessionIdentityRequestV1>()
        {
            *self.requests.lock().unwrap() += 1;
            request.claim(" subagent-worker-run1-1 ");
        }
    }
}

/// The peer's view of every session: `(id, name)`.
async fn roster(peer: &IntercomClient) -> Vec<(String, Option<String>)> {
    peer.list_sessions()
        .await
        .expect("peer list")
        .into_iter()
        .map(|s| (s.id, s.name))
        .collect()
}

/// THE FIX, end to end. A second extension's claim beats the configured `stableId`, and the
/// session keeps its readable name. Pre-fix nothing emits the request, so the claimant never runs
/// and the session registers as `process-wide-id`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claiming_extension_sets_the_intercom_id_over_the_stable_id() {
    let tmp = tempfile::tempdir().unwrap();
    let agent_dir = tmp.path().join("agent");
    let cwd = tmp.path().join("project");
    std::fs::create_dir_all(&cwd).unwrap();
    let intercom_dir = intercom_dir_path(&agent_dir);
    std::fs::create_dir_all(&intercom_dir).unwrap();
    // `process.env.PI_INTERCOM_STABLE_ID = "process-wide-id"` upstream; the config key is the same
    // tier (`resolveConfiguredIntercomSessionId`) and needs no process-global env write here.
    std::fs::write(
        config_path(&intercom_dir),
        serde_json::json!({
            "brokerCommand": broker_bin().to_string_lossy(),
            "brokerArgs": [],
            "stableId": "process-wide-id",
        })
        .to_string(),
    )
    .unwrap();
    let socket = broker_socket_path(&intercom_dir);
    let mut broker = spawn_broker(&agent_dir);
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");

    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.clone(),
            cwd.clone(),
            load_config(&intercom_dir).expect("config loads"),
            None,
        )
        .expect("build the intercom extension"),
    );
    let claimant = Arc::new(Claimant::default());
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.app_mode = AppMode::Print;
    let session = SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, cfg)
        .with_native_extension(ext.clone())
        .with_native_extension(claimant.clone())
        .build()
        .await
        .expect("build")
        .into_shared();
    // `createExtensionHarness("worker: fix auth refresh", …)` — the session's readable name.
    ext.state()
        .host_services()
        .expect("the live backend is bound before init")
        .set_session_name("worker: fix auth refresh");
    session.bind_extensions().await;

    // The claim is settled before the startup connect is even scheduled, so the FIRST client this
    // session ever publishes is already registered under it — there is no host-assigned
    // registration to replace.
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state.client().is_some()).await,
        "the session connects"
    );
    assert_eq!(
        state.client().and_then(|c| c.session_id()).as_deref(),
        Some("subagent-worker-run1-1"),
        "the session registers under the trimmed claim, not `process-wide-id`"
    );
    assert_eq!(
        *claimant.requests.lock().unwrap(),
        1,
        "exactly one V1 request per session start"
    );

    let peer = IntercomClient::connect(&socket, registration("peer"), Some("peer".into()))
        .await
        .expect("peer connects");
    let sessions = roster(&peer).await;
    assert!(
        sessions.contains(&(
            "subagent-worker-run1-1".to_string(),
            Some("worker: fix auth refresh".to_string())
        )),
        "`waitForSessionId(planner, \"subagent-worker-run1-1\")` with the readable name: {sessions:?}"
    );
    assert!(
        !sessions.iter().any(|(id, _)| id == "process-wide-id"),
        "no registration under the stable id: {sessions:?}"
    );

    peer.disconnect();
    session.dispose("quit").await;
    let _ = broker.kill().await;
}

const HOST_SESSION_ID: &str = "session-c1a1mc1a1mc1a1m0";

/// A backend whose typed bus has the listeners `claims` stands for: each entry is one listener's
/// `request.claim(…)` call, in listener order.
struct ClaimingHost {
    claims: Mutex<Vec<&'static str>>,
}

impl HostServices for ClaimingHost {
    fn session_id(&self) -> Option<String> {
        Some(HOST_SESSION_ID.to_string())
    }
    fn emit_typed_event(&self, topic: &str, event: &dyn std::any::Any) {
        if topic == INTERCOM_SESSION_IDENTITY_EVENT
            && let Some(request) = event.downcast_ref::<IntercomSessionIdentityRequestV1>()
        {
            for claim in self.claims.lock().unwrap().iter() {
                request.claim(claim);
            }
        }
    }
}

async fn start_session(ext: &IntercomExtension, ctx: &HostCtx) {
    let _ = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
                previous_session_file: None,
            },
            ctx,
        )
        .await;
}

async fn registered_as(ext: &IntercomExtension, expected: &str) -> bool {
    let state = ext.state().clone();
    within(Duration::from_secs(30), || {
        state
            .client()
            .filter(|c| c.is_connected())
            .and_then(|c| c.session_id())
            .is_some_and(|id| id == expected)
    })
    .await
}

/// `claimedIntercomSessionId ??= stableId.trim() || undefined` (`index.ts:1648-1650`): a blank
/// claim is no claim and the first real one wins over every later one. The slot is a fresh `let`
/// per `session_start` (`:1645`), so a runtime whose listeners claim nothing registers under its
/// host id again rather than inheriting the previous runtime's claim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_non_blank_claim_wins_and_does_not_outlive_its_runtime() {
    let agent_dir = tempfile::tempdir().unwrap();
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);
    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");
    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.path().to_path_buf(),
            PathBuf::from("/tmp/work"),
            load_config(&intercom_dir).expect("config loads"),
            None,
        )
        .expect("build the extension"),
    );
    let host = Arc::new(ClaimingHost {
        claims: Mutex::new(vec!["   ", " first-claim ", "second-claim"]),
    });
    ext.set_host_services(host.clone());
    let ctx = HostCtx::event(ExtMode::Print, false, agent_dir.path().to_path_buf());

    start_session(&ext, &ctx).await;
    assert!(
        registered_as(&ext, "first-claim").await,
        "registered under the first non-blank claim; got {:?}",
        ext.state().client().and_then(|c| c.session_id())
    );
    let peer = IntercomClient::connect(&socket, registration("peer"), Some("peer".into()))
        .await
        .expect("peer connects");
    // `buildPresenceIdentity(pi, currentIntercomSessionId ?? …)` (`index.ts:910,956,1657`): the
    // session is unnamed, so its alias is cut from the claimed id, not the host id.
    let sessions = roster(&peer).await;
    assert!(
        sessions.contains(&(
            "first-claim".to_string(),
            Some("subagent-chat-first-claim".to_string())
        )),
        "the unnamed claimed session advertises the claim's alias: {sessions:?}"
    );
    assert!(
        !sessions
            .iter()
            .any(|(id, _)| id == HOST_SESSION_ID || id == "second-claim"),
        "no registration under the host id or a later claim: {sessions:?}"
    );

    // The next runtime's listeners claim nothing.
    host.claims.lock().unwrap().clear();
    start_session(&ext, &ctx).await;
    assert!(
        registered_as(&ext, HOST_SESSION_ID).await,
        "an unclaimed runtime registers under its host id; got {:?}",
        ext.state().client().and_then(|c| c.session_id())
    );
    assert_eq!(ext.state().claimed_intercom_session_id(), None);

    peer.disconnect();
    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}
