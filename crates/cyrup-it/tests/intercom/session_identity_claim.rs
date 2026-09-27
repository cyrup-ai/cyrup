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
//! cyrup's bus carries JSON and delivers after the emitting dispatch, so the claim is a reply on
//! `intercom:session-identity-claim` (`{ version: 1, stableId }`) — see
//! `cyrup_intercom::identity::INTERCOM_SESSION_IDENTITY_CLAIM_EVENT`.
//!
//! The first test is the whole production path: a real `AgentSession` whose host bus carries the
//! request from the intercom extension to a second native extension and the claim back, against a
//! real broker. The other two drive `IntercomExtension::on_bus_event` — the entry point the host bus
//! delivers through — to pin the late-claim re-register and the closed window.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::{broker_bin, registration, spawn_broker, within, write_broker_command};
use cyrup_core::ExtensionId;
use cyrup_ext::{
    ExtError, ExtMode, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_intercom::config::{config_path, load_config};
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::identity::{
    INTERCOM_SESSION_IDENTITY_CLAIM_EVENT, INTERCOM_SESSION_IDENTITY_EVENT,
};
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::spawn::wait_for_broker;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AppMode, SessionBuilder, SessionConfig};

/// A subagent-launcher stand-in: answers the identity request with a claim, exactly as upstream's
/// `pi.events.on(INTERCOM_SESSION_IDENTITY_EVENT, (request) => request.claim(" subagent-worker-run1-1 "))`.
#[derive(Default)]
struct Claimant {
    services: Mutex<Option<Arc<dyn HostServices>>>,
    requests: Mutex<Vec<serde_json::Value>>,
}

#[async_trait::async_trait]
impl NativeExtension for Claimant {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("identity-claimant")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        *self.services.lock().unwrap() = Some(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe_bus(INTERCOM_SESSION_IDENTITY_EVENT);
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &serde_json::Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        if topic == INTERCOM_SESSION_IDENTITY_EVENT {
            self.requests.lock().unwrap().push(payload.clone());
            let services = self.services.lock().unwrap().clone();
            if let Some(services) = services {
                services.emit_event(
                    INTERCOM_SESSION_IDENTITY_CLAIM_EVENT,
                    &serde_json::json!({ "version": 1, "stableId": " subagent-worker-run1-1 " }),
                );
            }
        }
        Ok(())
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

    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state
            .client()
            .and_then(|c| c.session_id())
            .is_some_and(|id| id == "subagent-worker-run1-1"))
        .await,
        "the session registers under the trimmed claim, not `process-wide-id`; got {:?}",
        state.client().and_then(|c| c.session_id())
    );
    assert_eq!(
        claimant.requests.lock().unwrap().clone(),
        vec![serde_json::json!({ "version": 1 })],
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

struct HostSession;

impl HostServices for HostSession {
    fn session_id(&self) -> Option<String> {
        Some(HOST_SESSION_ID.to_string())
    }
}

/// A session started through the production `SessionStart` arm and connected under its host id.
async fn started(agent_dir: &Path) -> (Arc<IntercomExtension>, HostCtx, tokio::process::Child) {
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
    ext.set_host_services(Arc::new(HostSession));
    let ctx = HostCtx::event(ExtMode::Print, false, agent_dir.to_path_buf());
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
            .and_then(|c| c.session_id())
            .is_some_and(|id| id == HOST_SESSION_ID))
        .await,
        "unclaimed, the session registers under its host session id"
    );
    (ext, ctx, broker)
}

async fn claim(ext: &IntercomExtension, ctx: &HostCtx, payload: serde_json::Value) {
    ext.on_bus_event(INTERCOM_SESSION_IDENTITY_CLAIM_EVENT, &payload, ctx)
        .await
        .expect("the claim listener never faults");
}

/// cyrup's bus delivers the claim after the `session_start` dispatch, so the startup connect can
/// register first. The claim still wins: the session re-registers under it and the host-assigned id
/// leaves the roster. A second claim does not move it again (`??=`: the first claim wins).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claim_that_lands_after_registration_re_registers_under_it() {
    let agent_dir = tempfile::tempdir().unwrap();
    let (ext, ctx, mut broker) = started(agent_dir.path()).await;
    let socket = broker_socket_path(&intercom_dir_path(agent_dir.path()));
    let peer = IntercomClient::connect(&socket, registration("peer"), Some("peer".into()))
        .await
        .expect("peer connects");

    // Not a V1 claim, then a blank one: both ignored.
    claim(&ext, &ctx, serde_json::json!({ "stableId": "no-version" })).await;
    claim(
        &ext,
        &ctx,
        serde_json::json!({ "version": 1, "stableId": "   " }),
    )
    .await;
    claim(
        &ext,
        &ctx,
        serde_json::json!({ "version": 1, "stableId": " late-claim " }),
    )
    .await;
    claim(
        &ext,
        &ctx,
        serde_json::json!({ "version": 1, "stableId": "second" }),
    )
    .await;

    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state
            .client()
            .filter(|c| c.is_connected())
            .and_then(|c| c.session_id())
            .is_some_and(|id| id == "late-claim"))
        .await,
        "re-registered under the first valid claim; got {:?}",
        state.client().and_then(|c| c.session_id())
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let ids = loop {
        let ids: Vec<String> = roster(&peer).await.into_iter().map(|(id, _)| id).collect();
        if !ids.iter().any(|id| id == HOST_SESSION_ID) || tokio::time::Instant::now() >= deadline {
            break ids;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    };
    assert!(ids.contains(&"late-claim".to_string()), "{ids:?}");
    assert!(
        !ids.iter()
            .any(|id| id == HOST_SESSION_ID || id == "second" || id == "no-version"),
        "the host id is gone and no later claim registered: {ids:?}"
    );
    // `buildPresenceIdentity(pi, currentIntercomSessionId ?? …)` (`index.ts:910,956,1657`): the
    // session is unnamed, so its alias is cut from the claimed id, not the host id.
    let sessions = roster(&peer).await;
    assert!(
        sessions.contains(&(
            "late-claim".to_string(),
            Some("subagent-chat-late-claim".to_string())
        )),
        "the unnamed claimed session advertises the claim's alias: {sessions:?}"
    );

    peer.disconnect();
    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// The window closes at the session's first `agent_start`: a claim after it changes nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_claim_after_the_first_run_started_is_ignored() {
    let agent_dir = tempfile::tempdir().unwrap();
    let (ext, ctx, mut broker) = started(agent_dir.path()).await;
    let _ = ext.on_event(&HostEvent::AgentStart, &ctx).await;
    claim(
        &ext,
        &ctx,
        serde_json::json!({ "version": 1, "stableId": "too-late" }),
    )
    .await;

    tokio::time::sleep(Duration::from_millis(500)).await;
    let client = ext.state().client().expect("still connected");
    assert!(client.is_connected());
    assert_eq!(client.session_id().as_deref(), Some(HOST_SESSION_ID));
    assert_eq!(ext.state().claimed_intercom_session_id(), None);

    client.disconnect();
    let _ = broker.kill().await;
}
