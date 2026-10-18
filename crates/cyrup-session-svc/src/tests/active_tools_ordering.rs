//! SEAM-175 — a restriction governs the tools registered BEFORE it, and never the ones registered
//! after it.
//!
//! pi's tool selection is a sequence. Every `registerTool` ends with `runtime.refreshTools()`
//! (`core/extensions/loader.ts:300` @f1b2e77f5), which merges the tool into the registry and
//! activates it ON TOP OF THE ACTIVE SET AS IT IS NOW (`_refreshToolRegistry` starts from
//! `previousActiveToolNames`, `core/agent-session.ts:3566`, and activates the names the registry
//! has not seen, `:3582-3589`). `setActiveTools(names)` is immediate (`setActiveToolsByName`,
//! `:1518-1526`): it replaces the active set at that moment and nothing re-applies it later. So a
//! tool registered after a restriction is active, and one registered before a restriction that does
//! not name it is not.
//!
//! cyrup cannot merge a registration at the moment it is made (the guest's store is borrowed, or
//! the registration comes from another task), so the merge is deferred to the next drain. The
//! restriction used to be deferred with it as a list of NAMES, replayed at the drain AFTER the merge.
//! That keeps the "registered before" half (a guest that registers a tool and restricts the set gets
//! what it asked for) and breaks the other half: a restriction made at `session_start` (the
//! subagents loader's, `tool_activation.rs`) was still queued when an MCP server connected and
//! registered its tools, so the drain merged them, activated them, and then replayed the old names
//! over them. The tools were registered and never active: absent from the next request and from a
//! `codemode` script's `tools`.
//!
//! The same replay undid an explicit `set_active_tools_by_name` made after the restriction, which in
//! pi is the later write and wins.
//!
//! Every assertion reads the request the PROVIDER received, or the facade's active set.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex, OnceLock};

use cyrup_core::{
    CancelToken, ExtensionId, Tool, ToolCallId, ToolError, ToolExposure, ToolResult, ToolUpdateSink,
};
use cyrup_ext::host::HostServices;
use cyrup_ext::{EventKind, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::Provider;
use serde_json::{Value, json};

use super::tool_transcript::{Fixture, Reply, Requests, ToolsExt, config, fixture, prompt, script};
use crate::{AgentSession, SessionBuilder, SessionTarget};

/// The owner `ToolsExt` registers its tools under, which a late registration reuses.
const OWNER: &str = "tool-transcript-ext";

// ------------------------------------------------------------------------------ doubles ----

/// A tool with a settable exposure, and a `default_active` that registration honours.
struct Named {
    name: &'static str,
    exposure: ToolExposure,
    default_active: bool,
    params: Value,
}

impl Named {
    /// Registered and active.
    fn direct(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name,
            exposure: ToolExposure::Direct,
            default_active: true,
            params: json!({"type": "object", "properties": {}}),
        })
    }

    /// Registered, and NOT activated by registration: only naming it activates it.
    fn spare(name: &'static str) -> Arc<dyn Tool> {
        Arc::new(Self {
            name,
            exposure: ToolExposure::Direct,
            default_active: false,
            params: json!({"type": "object", "properties": {}}),
        })
    }
}

#[async_trait::async_trait]
impl Tool for Named {
    fn name(&self) -> &str {
        self.name
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "ordering probe"
    }
    fn exposure(&self) -> ToolExposure {
        self.exposure
    }
    fn default_active(&self) -> bool {
        self.default_active
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::default())
    }
}

/// What the subagents loader does at `session_start`: restrict the active set through the
/// extension-facing `setActiveTools`, to the names it read a moment before.
struct RestrictAtStart {
    services: OnceLock<Arc<dyn HostServices>>,
    to: &'static [&'static str],
}

impl RestrictAtStart {
    fn arc(to: &'static [&'static str]) -> Arc<Self> {
        Arc::new(Self {
            services: OnceLock::new(),
            to,
        })
    }
}

#[async_trait::async_trait]
impl NativeExtension for RestrictAtStart {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("restrict-at-start")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        let _ = self.services.set(services);
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::SessionStart]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let (HostEvent::SessionStart { .. }, Some(services)) = (ev, self.services.get()) {
            let names: Vec<String> = self.to.iter().map(|n| (*n).to_string()).collect();
            services.set_active_tools(&names);
        }
        HookOutcome::Noop
    }
}

// ------------------------------------------------------------------------------ helpers ----

fn sorted(mut names: Vec<String>) -> Vec<String> {
    names.sort();
    names
}

fn strs(names: &[&str]) -> Vec<String> {
    names.iter().map(|n| (*n).to_string()).collect()
}

/// A session over `tools` (registered at build, active), plus the restricting extension when given,
/// whose provider answers once. `bind_extensions` has NOT run. The fixture's directories live as
/// long as the caller's `fx`.
async fn session_over(
    fx: &Fixture,
    requests: &Requests,
    tools: Vec<Arc<dyn Tool>>,
    restrict: Option<Arc<RestrictAtStart>>,
) -> Arc<AgentSession> {
    let mut builder = SessionBuilder::new(
        script(requests, vec![Reply::Text("done")]) as Arc<dyn Provider>,
        config(fx, SessionTarget::New),
    )
    .with_native_extension(Arc::new(ToolsExt(tools)));
    if let Some(restrict) = restrict {
        builder = builder.with_native_extension(restrict);
    }
    builder.build().await.unwrap().into_shared()
}

/// The tools the FIRST provider request declared, sorted.
fn first_request(requests: &Requests) -> Vec<String> {
    sorted(requests.lock().unwrap()[0].tools.clone())
}

fn services(session: &AgentSession) -> Arc<crate::host_services::LiveHostServices> {
    Arc::clone(&session.services().host_services)
}

// -------------------------------------------------------------------------------- tests ----

/// HEADLINE. A restriction made at `session_start` is still queued when a tool registers live (an
/// MCP server that connects after the session started). The tool is active in the next request.
///
/// Before the fix the drain merged the tool, activated it, and replayed the restriction's names
/// over it: the request declared `["early"]`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_registered_after_a_restriction_is_active_in_the_next_request() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early"), Named::direct("other")],
        Some(RestrictAtStart::arc(&["early"])),
    )
    .await;
    session.bind_extensions().await;
    assert_eq!(
        sorted(session.active_tool_names()),
        strs(&["early"]),
        "the facade reflects the restriction at once"
    );

    session
        .services()
        .ext_host
        .register_late_tool(ExtensionId::from(OWNER), Named::direct("late"))
        .unwrap();
    prompt(&session, "go").await;

    assert_eq!(
        first_request(&requests),
        strs(&["early", "late"]),
        "the restriction still holds (`other` is out) and the late tool is on top of it"
    );
}

/// A tool registered BEFORE a restriction that does not name it stays inactive: the restriction
/// governs everything registered when it was made, merged or not (pi merges at `registerTool`, so
/// the restriction that follows deactivates it). This is what the replay of names was for.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_registered_before_a_restriction_that_does_not_name_it_stays_inactive() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early"), Named::direct("other")],
        None,
    )
    .await;
    session.bind_extensions().await;

    session
        .services()
        .ext_host
        .register_late_tool(ExtensionId::from(OWNER), Named::direct("pre"))
        .unwrap();
    services(&session).set_active_tools(&strs(&["early"]));
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early"]));
}

/// …and one registered before a restriction that NAMES it is active, even though registration alone
/// would not activate it (`defaultActive: false`): naming a tool activates it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_registered_before_a_restriction_that_names_it_is_active() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(&fx, &requests, vec![Named::direct("early")], None).await;
    session.bind_extensions().await;

    session
        .services()
        .ext_host
        .register_late_tool(ExtensionId::from(OWNER), Named::spare("pre"))
        .unwrap();
    services(&session).set_active_tools(&strs(&["early", "pre"]));
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early", "pre"]));
}

/// A tool registered after a restriction that would not activate it on registration is not
/// activated: the restriction does not make it active, and nothing else does.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_spare_tool_registered_after_a_restriction_stays_inactive() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early")],
        Some(RestrictAtStart::arc(&["early"])),
    )
    .await;
    session.bind_extensions().await;

    session
        .services()
        .ext_host
        .register_late_tool(ExtensionId::from(OWNER), Named::spare("spare"))
        .unwrap();
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early"]));
}

/// An explicit `set_active_tools_by_name` made after an extension's restriction is the later write,
/// and wins. Before the fix the queued names were replayed over it at the next drain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_explicit_activation_after_a_restriction_wins() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early"), Named::spare("spare")],
        Some(RestrictAtStart::arc(&["early"])),
    )
    .await;
    session.bind_extensions().await;

    session
        .set_active_tools_by_name(&strs(&["early", "spare"]))
        .await;
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early", "spare"]));
}

/// The explicit write governs what was registered before it, as the extension-facing one does: a
/// tool registered, and not yet merged, when `set_active_tools_by_name` is called and not named by it
/// is not active (pi merged it at `registerTool`, so the write deactivated it). Before the fix the
/// merge activated it afterwards, and the request declared a tool the write had left out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_registered_before_an_explicit_write_that_does_not_name_it_stays_inactive() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early"), Named::direct("other")],
        None,
    )
    .await;
    session.bind_extensions().await;

    session
        .services()
        .ext_host
        .register_late_tool(ExtensionId::from(OWNER), Named::direct("pre"))
        .unwrap();
    session.set_active_tools_by_name(&strs(&["early"])).await;
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early"]));
}

/// Of two restrictions before a drain, the later one is the answer (pi: the last `setActiveTools`
/// wins).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_later_of_two_restrictions_wins() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early"), Named::direct("other")],
        None,
    )
    .await;
    session.bind_extensions().await;

    services(&session).set_active_tools(&strs(&["early"]));
    services(&session).set_active_tools(&strs(&["early", "other"]));
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early", "other"]));
}

/// The restriction reaches the agent the next request is built from, with nothing else changing: the
/// push no longer carries the names, so it must still happen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restriction_alone_narrows_the_next_request() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(
        &fx,
        &requests,
        vec![Named::direct("early"), Named::direct("other")],
        Some(RestrictAtStart::arc(&["early"])),
    )
    .await;
    session.bind_extensions().await;
    prompt(&session, "go").await;

    assert_eq!(first_request(&requests), strs(&["early"]));
}

// ------------------------------------------------------------------------- parked red-proves ----

/// SEAM-177, parked. pi's registry is current at `registerTool` (`loader.ts:300` @f1b2e77f5 refreshes
/// on every registration), so `getAllTools()` answers with a tool the moment it is registered. cyrup
/// answers from the session's dynamic-tool view, which merges at the next drain, so an extension that
/// reads the tool set to compute a restriction (the permission system does, at `before_agent_start`)
/// cannot see a tool registered since the last drain, and the restriction it makes then treats that
/// tool as one it never saw.
///
/// RED at the base of the change that filed it: the registered tool is not in the answer. Un-ignore
/// it with the fix.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "SEAM-177: `all_tool_names` answers from the merged view and misses a tool registered since the last drain"]
async fn a_tool_registered_live_is_visible_to_all_tool_names_at_once() {
    let fx = fixture();
    let requests: Requests = Arc::new(Mutex::new(Vec::new()));
    let session = session_over(&fx, &requests, vec![Named::direct("early")], None).await;
    session.bind_extensions().await;

    session
        .services()
        .ext_host
        .register_late_tool(ExtensionId::from(OWNER), Named::direct("late"))
        .unwrap();

    let all = services(&session).all_tool_names().expect("a live view");
    assert!(
        all.iter().any(|name| name == "late"),
        "registered, and not yet merged: {all:?}"
    );
}
