//! SUBA-143 — the REACHABILITY proof for the runtime-agent registration EVENT bridge (pi
//! `src/agents/runtime-agent-events.ts` @v0.71.0, installed at `src/extension/index.ts:894`).
//!
//! The ledger row's Verify, literally: *"A guest extension registers an agent through the topic,
//! sees `{ ok: true }`, and the agent appears in discovery; a malformed definition returns
//! `{ ok: false }` with upstream's message."* Plus the half upstream gets from its returned
//! object and cyrup has to earn — disposal, and a session teardown that leaves no live token.
//!
//! Every hop is production code; the only test-authored objects are the `HostServices` impl (whose
//! `emit_event` body is the live backend's single line, `cyrup-session-svc/src/host_services.rs`)
//! and a sibling extension that records what the bus hands it.
//!
//! ```text
//! ExtensionHost::new(HostConfig{ mode, has_ui, cwd })                    (cyrup-ext/src/facade.rs)
//!   → host.load_native_with_services(SubagentsExtension, services)                        REAL
//!       → NativeExtension::init, RegistrationMode::Full                  (native_impl.rs)  REAL
//!           → InitApi::subscribe_bus(RUNTIME_AGENT_REGISTER_EVENT)                        REAL
//!           → InitApi::subscribe_bus(RUNTIME_AGENT_DISPOSE_EVENT)                         REAL
//!   → host.bus().subscribe(<client>, runtime_agent_register_reply_event(id))  — client attach
//!   → host.bus().emit(RUNTIME_AGENT_REGISTER_EVENT, runtime_agent_register_request(..))   REAL
//!   → host.deliver_bus_events(&CancelToken::new())                                        REAL
//!       → SubagentsExtension::on_bus_event                                          THE FEATURE
//!           → RuntimeAgentEventBridge::dispatch → RuntimeAgentRegistry::register_value    REAL
//!           → HostServices::emit_event → SharedBus::emit                                  REAL
//!       → the next drain round delivers the reply to the client                           REAL
//!   → SubagentExecutor::resolve_agent finds the agent                                     REAL
//! ```
//!
//! No sleeps: `BusFanout::drain_bus` loops rounds inside the ONE `deliver_bus_events(..)` await, so
//! the reply arrives in the same call that carried the request.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use cyrup_core::{CancelToken, ExtensionId};
use cyrup_ext::native::{HostCtx, InitApi, NativeExtension};
use cyrup_ext::{ExtError, ExtMode, ExtensionHost, HookOutcome, HostConfig, HostEvent};

use crate::discovery::runtime_agent_events::{
    RUNTIME_AGENT_DISPOSE_EVENT, RUNTIME_AGENT_REGISTER_EVENT, read_disposal_reply,
    read_registration_reply, runtime_agent_dispose_reply_event, runtime_agent_dispose_request,
    runtime_agent_register_reply_event, runtime_agent_register_request,
};
use crate::discovery::types::{AgentReadScope, AgentSource};
use crate::extension::{RegistrationMode, SubagentsExtension};
use crate::paths::Roots;
use crate::registration::SubagentExtensionConfig;

const TEST_SESSION_ID: &str = "suba143sess";

/// The live capability backend reduced to what this surface reads. `emit_event`'s body is
/// production's, verbatim.
struct BusForwardingServices {
    bus: Arc<cyrup_ext::bus::SharedBus>,
    session_file: PathBuf,
}

impl cyrup_ext::host::HostServices for BusForwardingServices {
    fn emit_event(&self, topic: &str, payload: &Value) {
        self.bus.emit(topic.to_string(), payload.clone());
    }

    fn session_id(&self) -> Option<String> {
        Some(TEST_SESSION_ID.to_string())
    }

    fn session_file(&self) -> Option<PathBuf> {
        Some(self.session_file.clone())
    }
}

/// The sibling extension on the far end — i.e. exactly the caller upstream's
/// `registerAgentViaEvents` serves.
struct RecordingClient {
    id: ExtensionId,
    seen: Arc<Mutex<Vec<(String, Value)>>>,
}

impl RecordingClient {
    fn new() -> Self {
        Self {
            id: ExtensionId::from("suba143-client"),
            seen: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// The one delivery on `topic`, or `None` — which is upstream's `result === undefined`, the
    /// input [`read_registration_reply`] answers with pi's "not installed" sentence.
    fn one(&self, topic: &str) -> Option<Value> {
        let seen = self.seen.lock().unwrap().clone();
        let matching: Vec<&(String, Value)> = seen.iter().filter(|(t, _)| t == topic).collect();
        assert!(
            matching.len() <= 1,
            "expected at most one delivery on {topic}, saw: {seen:?}"
        );
        matching.first().map(|(_, value)| value.clone())
    }

    fn topics(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .map(|(topic, _)| topic.clone())
            .collect()
    }
}

#[async_trait::async_trait]
impl NativeExtension for RecordingClient {
    fn id(&self) -> ExtensionId {
        self.id.clone()
    }

    async fn init(&self, _api: &mut InitApi) -> Result<(), ExtError> {
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn on_bus_event(
        &self,
        topic: &str,
        payload: &Value,
        _ctx: &HostCtx,
    ) -> Result<(), ExtError> {
        self.seen
            .lock()
            .unwrap()
            .push((topic.to_string(), payload.clone()));
        Ok(())
    }
}

struct Harness {
    host: Arc<ExtensionHost>,
    client: Arc<RecordingClient>,
    extension: Arc<SubagentsExtension>,
    cwd: PathBuf,
    roots: Roots,
    _home: tempfile::TempDir,
    _dir: tempfile::TempDir,
}

impl Harness {
    async fn start(mode: RegistrationMode) -> Self {
        let home = tempfile::tempdir().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_path_buf();
        let roots = Roots::sandboxed(home.path());
        let config = SubagentExtensionConfig {
            roots: roots.clone(),
            ..SubagentExtensionConfig::default()
        };

        let host = Arc::new(ExtensionHost::new(HostConfig {
            mode: ExtMode::Tui,
            has_ui: true,
            cwd: cwd.clone(),
        }));
        let services = Arc::new(BusForwardingServices {
            bus: Arc::clone(host.bus()),
            session_file: home.path().join("session.jsonl"),
        });
        let extension = Arc::new(SubagentsExtension::with_mode(config, cwd.clone(), mode));
        let backend: Arc<dyn cyrup_ext::host::HostServices> = Arc::clone(&services) as _;
        host.load_native_with_services(extension.clone(), backend)
            .await
            .expect("the subagents extension loads");

        let client = Arc::new(RecordingClient::new());
        host.load_native(client.clone())
            .await
            .expect("the client extension loads");

        Self {
            host,
            client,
            extension,
            cwd,
            roots,
            _home: home,
            _dir: dir,
        }
    }

    async fn full() -> Self {
        Self::start(RegistrationMode::Full).await
    }

    fn listen(&self, topic: &str) {
        self.host
            .bus()
            .subscribe(self.client.id.clone(), topic.to_string());
    }

    async fn emit(&self, topic: &str, envelope: Value) {
        self.host.bus().emit(topic.to_string(), envelope);
        self.host.deliver_bus_events(&CancelToken::new()).await;
    }

    /// Register through the bus exactly as a sibling extension would, and read the reply back with
    /// the ported client helper. `Ok` carries the `registrationId`.
    async fn register(
        &self,
        request_id: &str,
        name: &str,
        definition: &Value,
    ) -> Result<String, String> {
        let reply_topic = runtime_agent_register_reply_event(request_id);
        self.listen(&reply_topic);
        self.emit(
            RUNTIME_AGENT_REGISTER_EVENT,
            runtime_agent_register_request(request_id, name, definition),
        )
        .await;
        read_registration_reply(self.client.one(&reply_topic).as_ref())
    }

    async fn dispose(&self, request_id: &str, registration_id: &str) -> Result<(), String> {
        let reply_topic = runtime_agent_dispose_reply_event(request_id);
        self.listen(&reply_topic);
        self.emit(
            RUNTIME_AGENT_DISPOSE_EVENT,
            runtime_agent_dispose_request(request_id, registration_id),
        )
        .await;
        read_disposal_reply(self.client.one(&reply_topic).as_ref())
    }

    /// The executor's OWN resolution path — the seam the `subagent` tool selects an agent through.
    fn resolves(&self, name: &str) -> bool {
        self.extension
            .executor()
            .resolve_agent(&self.cwd, name, AgentReadScope::Both, &self.roots)
            .is_ok()
    }
}

fn definition() -> Value {
    json!({
        "description": "Scout at runtime.",
        "systemPrompt": "Scout.",
        "aliases": ["rscout"],
    })
}

/// A second definition that shares no identity key with [`definition`] — registering two agents
/// that both claim the alias `rscout` is a genuine collision, and the registry refuses it.
fn other_definition() -> Value {
    json!({
        "description": "Another agent at runtime.",
        "systemPrompt": "Other.",
    })
}

// =================================================================================================
// The row's Verify
// =================================================================================================

/// A sibling extension with NOTHING but the bus registers an agent, sees `{ok: true}`, and the
/// agent is selectable by the executor's own resolution path.
#[tokio::test]
async fn a_sibling_extension_registers_an_agent_over_the_bus_and_discovery_finds_it() {
    let harness = Harness::full().await;
    assert!(!harness.resolves("runtime-scout"));

    let registration_id = harness
        .register("suba143-1", "runtime-scout", &definition())
        .await
        .expect("the bridge answers ok");
    assert!(
        !registration_id.is_empty(),
        "the reply must carry a redeemable registrationId"
    );

    // The registry the tool's discovery reads.
    let listed = harness.extension.executor().runtime_agents().list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].name, "runtime-scout");
    assert_eq!(listed[0].source, AgentSource::Runtime);
    assert_eq!(listed[0].file_path, PathBuf::from("runtime:runtime-scout"));

    // … and the executor's own resolution, by name AND by the registered alias.
    assert!(harness.resolves("runtime-scout"));
    assert!(harness.resolves("rscout"));

    // The reply landed on the requester's topic and NOWHERE else.
    assert_eq!(
        harness.client.topics(),
        vec![runtime_agent_register_reply_event("suba143-1")]
    );
}

/// The other half of the row's Verify: a malformed definition returns `{ok: false}` carrying the
/// registry's own — i.e. upstream's — sentence, and registers nothing.
#[tokio::test]
async fn a_malformed_definition_is_refused_over_the_bus_with_upstreams_message() {
    let harness = Harness::full().await;
    assert_eq!(
        harness
            .register(
                "suba143-bad",
                "runtime-scout",
                &json!({"description": "d", "systemPrompt": "p", "teleport": true}),
            )
            .await
            .expect_err("refused"),
        "Runtime agent definition has unknown fields: teleport."
    );
    assert!(harness.extension.executor().runtime_agents().is_empty());
    assert!(harness.extension.runtime_agent_bridge().is_empty());
    assert!(!harness.resolves("runtime-scout"));
}

/// Upstream returns an object whose `dispose()` removes the agent. The token does the same job,
/// over the bus, and is idempotent the way `dispose(): void` is (`runtime-agent-registry.ts:389`).
#[tokio::test]
async fn the_registration_token_disposes_the_agent_and_is_idempotent() {
    let harness = Harness::full().await;
    let first = harness
        .register("suba143-1", "runtime-scout", &definition())
        .await
        .expect("ok");
    let second = harness
        .register("suba143-2", "runtime-other", &other_definition())
        .await
        .expect("ok");
    assert_ne!(first, second, "each registration gets its own token");
    assert_eq!(harness.extension.runtime_agent_bridge().len(), 2);

    harness.dispose("suba143-d1", &first).await.expect("ok");
    assert!(!harness.resolves("runtime-scout"));
    assert!(
        harness.resolves("runtime-other"),
        "one token must remove exactly one record"
    );

    // Second call: upstream's `if (disposed) return;`.
    harness.dispose("suba143-d2", &first).await.expect("ok");
    assert!(harness.resolves("runtime-other"));
    assert_eq!(harness.extension.runtime_agent_bridge().len(), 1);
}

/// `SessionShutdown` runs pi's `clearRuntimeAgentsForPi(pi)` (`extension/index.ts:1042`). The
/// bridge's tokens must not survive it — a stale token reaching into a rebuilt session would
/// remove an agent the new session registered.
#[tokio::test]
async fn session_shutdown_clears_the_registry_and_the_outstanding_tokens() {
    let harness = Harness::full().await;
    let token = harness
        .register("suba143-1", "runtime-scout", &definition())
        .await
        .expect("ok");
    assert!(harness.resolves("runtime-scout"));

    harness
        .extension
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "test".to_string(),
                target_session_file: None,
            },
            &HostCtx::event(ExtMode::Json, false, harness.cwd.clone()),
        )
        .await;

    assert!(harness.extension.executor().runtime_agents().is_empty());
    assert!(harness.extension.runtime_agent_bridge().is_empty());
    assert!(!harness.resolves("runtime-scout"));

    // A rebuilt session takes the name again; the stale token is a no-op against it.
    harness
        .register("suba143-2", "runtime-scout", &definition())
        .await
        .expect("ok");
    harness.dispose("suba143-d1", &token).await.expect("ok");
    assert!(
        harness.resolves("runtime-scout"),
        "a token from the previous session must not reach into this one"
    );
}

/// A `requestId` carrying a newline would let a caller name a reply topic it was never given. The
/// refusal goes to `…:unknown`, and NOTHING is delivered on the forged topic.
#[tokio::test]
async fn a_forged_reply_topic_is_never_written_to() {
    let harness = Harness::full().await;
    let forged = runtime_agent_register_reply_event("victim");
    harness.listen(&forged);
    harness.listen(&runtime_agent_register_reply_event("unknown"));

    harness
        .emit(
            RUNTIME_AGENT_REGISTER_EVENT,
            json!({
                "version": 1,
                "requestId": format!("attacker\n{forged}"),
                "name": "runtime-scout",
                "definition": definition(),
            }),
        )
        .await;

    assert!(
        harness.client.one(&forged).is_none(),
        "the forged topic must carry nothing"
    );
    assert_eq!(
        read_registration_reply(
            harness
                .client
                .one(&runtime_agent_register_reply_event("unknown"))
                .as_ref()
        )
        .expect_err("refused"),
        "Runtime agent event requestId must be a non-empty string without newlines."
    );
    assert!(harness.extension.executor().runtime_agents().is_empty());
}

/// A `ChildSafe` fanout child registers no orchestrator surface, so it subscribes to neither
/// topic: the request reaches nobody and the client's reader sees upstream's "not installed"
/// sentence — which is exactly what upstream's `registerAgentViaEvents` throws when no owner
/// answered (`runtime-agent-events.ts:36-38`).
#[tokio::test]
async fn a_child_safe_registration_answers_no_runtime_agent_event_at_all() {
    let harness = Harness::start(RegistrationMode::ChildSafe).await;
    assert_eq!(
        harness
            .register("suba143-1", "runtime-scout", &definition())
            .await
            .expect_err("no owner answered"),
        crate::discovery::runtime_agent_events::RUNTIME_AGENT_BRIDGE_ABSENT_MESSAGE
    );
    assert!(harness.extension.executor().runtime_agents().is_empty());
    assert!(harness.client.topics().is_empty());
}
