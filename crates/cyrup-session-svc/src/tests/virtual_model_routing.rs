//! The virtual-model ROUTING STEP, end to end — pi `_installAgentRequestProjection`'s virtual arm
//! (`packages/coding-agent/src/core/agent-session.ts:775-831` @v1.0.4) and the `direct` arm of
//! `_getSummarizationRequestAuth` (`:560-581`).
//!
//! Every case here drives a REAL request through the real agent loop: a registered virtual model is
//! selected, a fake router answers, and the assertion is on what the faux PROVIDER was asked for and
//! on what the session PERSISTED. Nothing calls `route_request` directly, because the thing under
//! test is that routing happens on every request at all.
//!
//! Each test names the behaviour it pins and the neutering it is red against. The two marked
//! `REGRESSION GUARD` pass with the behaviour removed by construction and are not red proofs.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{
    AssistantMessage, Content, ExtensionId, ModelThinkingLevel, StopReason, Tool, ToolCallId,
    ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension};
use cyrup_provider::faux::{
    FauxConfig, FauxMessageOptions, FauxModelDefinition, FauxProvider, FauxResponseStep,
    faux_assistant_message, faux_assistant_message_with, faux_text, faux_tool_call,
};
use cyrup_provider::{
    Model, ModelRoute, ModelRouteError, ModelRouteReason, ModelRouteRequest, ModelRouter, Provider,
    VirtualModelDefinition, VirtualModelSpec, create_virtual_model,
};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, SessionBuilder, SessionConfig};

// ---------------------------------------------------------------------------- fixtures ----

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// `faux` with upstream's own two-model fixture shape (`test/suite/virtual-models.test.ts:43-62`):
/// a cheap `small` and a reasoning `large`. The windows are deliberately LARGE here so no case
/// below trips an incidental compaction; the compaction interactions have their own fixture.
fn two_model_faux() -> Arc<FauxProvider> {
    let mut small = FauxModelDefinition::new("small");
    small.context_window = 200_000;
    let mut large = FauxModelDefinition::new("large");
    large.context_window = 200_000;
    large.max_tokens = 4000;
    large.reasoning = true;
    Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![small, large],
        ..FauxConfig::default()
    }))
}

/// `router/auto`, pi's own example id (`docs/virtual-models.md`, "Register a virtual model").
fn router_spec() -> VirtualModelSpec {
    VirtualModelSpec {
        provider: "router".into(),
        id: "auto".into(),
        name: "Auto".to_string(),
        thinking_levels: Some(vec![
            ModelThinkingLevel::Off,
            ModelThinkingLevel::Low,
            ModelThinkingLevel::High,
        ]),
        context_window: None,
        max_tokens: None,
        input: None,
    }
}

/// One `route()` call, flattened to what the assertions need.
#[derive(Debug, Clone, PartialEq)]
struct Recorded {
    reason: ModelRouteReason,
    thinking_level: ModelThinkingLevel,
    /// `previous.model.id` and the level that turn was ASKED at.
    previous: Option<(String, Option<ModelThinkingLevel>)>,
    /// `failed.model.id` and the failed response's `error_message`.
    failed: Option<(String, Option<String>)>,
    state: Option<Value>,
}

/// What the router should answer for a call, given the call itself.
type Decide = Arc<
    dyn Fn(&Recorded) -> Result<(String, ModelThinkingLevel, Option<Value>), String> + Send + Sync,
>;

/// A fake router that records every call and answers by model ID, resolving it against the
/// physical catalog it was built with — which is what a real router does too, since
/// `resolve_model` re-resolves the answer against the catalog and returns the catalog's own row.
struct TestRouter {
    log: Arc<Mutex<Vec<Recorded>>>,
    catalog: Vec<Model>,
    decide: Decide,
}

#[async_trait::async_trait]
impl ModelRouter for TestRouter {
    async fn route(&self, request: ModelRouteRequest<'_>) -> Result<ModelRoute, ModelRouteError> {
        let recorded = Recorded {
            reason: request.reason,
            thinking_level: request.thinking_level,
            previous: request
                .previous
                .as_ref()
                .map(|p| (p.model.id.as_str().to_string(), p.thinking_level)),
            failed: request.failed.as_ref().map(|f| {
                (
                    f.model.id.as_str().to_string(),
                    f.message.error_message.clone(),
                )
            }),
            state: request.state.cloned(),
        };
        self.log.lock().unwrap().push(recorded.clone());
        let (id, level, state) = (self.decide)(&recorded).map_err(ModelRouteError::new)?;
        let model = self
            .catalog
            .iter()
            .find(|m| m.id.as_str() == id)
            .cloned()
            .ok_or_else(|| ModelRouteError::new(format!("test router: no model {id}")))?;
        Ok(ModelRoute {
            model,
            thinking_level: level,
            state,
        })
    }
}

/// Register `router/auto` on `session` with `decide`, and return the call log.
fn install_router(
    session: &AgentSession,
    physical: &[Model],
    decide: Decide,
) -> Arc<Mutex<Vec<Recorded>>> {
    let log = Arc::new(Mutex::new(Vec::new()));
    let router = Arc::new(TestRouter {
        log: Arc::clone(&log),
        catalog: physical.to_vec(),
        decide,
    });
    session
        .register_virtual_model(VirtualModelDefinition::new(router_spec(), router))
        .expect("register router/auto");
    log
}

/// Always route to `id` at `level`, carrying no state.
fn always(id: &'static str, level: ModelThinkingLevel) -> Decide {
    Arc::new(move |_| Ok((id.to_string(), level, None)))
}

/// One provider request as the faux provider saw it: the model id it was asked for and the
/// reasoning level on the options.
#[derive(Debug, Clone, PartialEq)]
struct SeenRequest {
    model: String,
    reasoning: ModelThinkingLevel,
}

/// What the scripted provider answers, per call.
#[derive(Clone)]
enum Reply {
    Text(&'static str),
    Call(&'static str),
    /// A terminal error whose text `is_retryable_assistant_error` matches.
    RetryableError(&'static str),
    /// A terminal error with arbitrary text — used for an OVERFLOW message, which
    /// `is_context_overflow` matches and `is_retryable_assistant_error` therefore does not.
    ErrorText(&'static str),
}

fn script(replies: Vec<Reply>, faux: &Arc<FauxProvider>) -> Arc<Mutex<Vec<SeenRequest>>> {
    let seen: Arc<Mutex<Vec<SeenRequest>>> = Arc::new(Mutex::new(Vec::new()));
    let steps: Vec<FauxResponseStep> = replies
        .into_iter()
        .map(|reply| {
            let seen = Arc::clone(&seen);
            FauxResponseStep::factory(move |_ctx, opts, _state, model| {
                seen.lock().unwrap().push(SeenRequest {
                    model: model.id.as_str().to_string(),
                    reasoning: opts.reasoning,
                });
                match reply {
                    Reply::Text(t) => {
                        faux_assistant_message(vec![faux_text(t.to_string())], StopReason::Stop)
                    }
                    Reply::Call(name) => faux_assistant_message(
                        vec![faux_tool_call(name.to_string(), json!({}))],
                        StopReason::ToolUse,
                    ),
                    Reply::RetryableError(text) | Reply::ErrorText(text) => {
                        faux_assistant_message_with(
                            Vec::new(),
                            StopReason::Error,
                            FauxMessageOptions {
                                error_message: Some(text.to_string()),
                                ..FauxMessageOptions::default()
                            },
                        )
                    }
                }
            })
        })
        .collect();
    faux.set_response_steps(steps);
    seen
}

/// A tool the scripted provider can call, so a run produces a CONTINUATION request.
struct Echo(Value);

#[async_trait::async_trait]
impl Tool for Echo {
    fn name(&self) -> &str {
        "echo"
    }
    fn parameters(&self) -> &Value {
        &self.0
    }
    fn description(&self) -> &str {
        "echo tool"
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: Value,
        _cancel: CancelTokenAlias,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("echoed")],
            ..Default::default()
        })
    }
}
type CancelTokenAlias = cyrup_core::CancelToken;

struct EchoExt;

#[async_trait::async_trait]
impl NativeExtension for EchoExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("virtual-routing-echo")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(
            Arc::new(Echo(json!({"type": "object", "properties": {}}))) as Arc<dyn Tool>
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
}

/// A native that answers `session_before_compact` with a compaction of its own — pi's
/// `pi.on("session_before_compact", … => ({ compaction: { summary, firstKeptEntryId,
/// tokensBefore } }))`, which is exactly the extension upstream's own
/// `"does not route compactions that an extension supplies"` test installs
/// (`test/suite/virtual-models.test.ts:346-366` @v1.0.4).
struct SuppliesCompactionExt;

#[async_trait::async_trait]
impl NativeExtension for SuppliesCompactionExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("virtual-routing-supplies-compaction")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::SessionBeforeCompact]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        let HostEvent::SessionBeforeCompact { preparation, .. } = ev else {
            return HookOutcome::Noop;
        };
        // Upstream reads `preparation.firstKeptEntryId` / `.tokensBefore` off the event and hands
        // them straight back, so the override is accepted rather than defaulted.
        HookOutcome::Mutate(cyrup_ext::EventPatch::CompactionOverride(json!({
            "summary": "extension summary",
            "firstKeptEntryId": preparation.get("firstKeptEntryId"),
            "tokensBefore": preparation.get("tokensBefore"),
        })))
    }
}

/// Fast, enabled auto-retry, so the retry case does not sleep for seconds.
fn fast_retry_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "retry",
        json!({"enabled": true, "maxRetries": 2, "baseDelayMs": 1}),
    )
    .unwrap();
    cli
}

/// Compaction budgets small enough that a two-turn session has something to compact.
fn compactable_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        json!({"enabled": true, "keepRecentTokens": 0, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

async fn prompt_and_settle(session: &AgentSession, text: &str) {
    let _ = session.prompt(text).await.expect("prompt");
    session.wait_for_idle().await;
}

/// Every `pi.virtual-model-state` entry's `data`, in branch order.
async fn state_entries(session: &AgentSession) -> Vec<Value> {
    session
        .entries_json()
        .await
        .into_iter()
        .filter(|e| e.get("customType").and_then(Value::as_str) == Some("pi.virtual-model-state"))
        .filter_map(|e| e.get("data").cloned())
        .collect()
}

/// The persisted assistant turns, newest last.
async fn assistants(session: &AgentSession) -> Vec<AssistantMessage> {
    session
        .messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_core::Message::Assistant(a) => Some(a),
            _ => None,
        })
        .collect()
}

// ------------------------------------------------------------------------------- tests ----

/// **The headline of this step.** Selecting a registered virtual model must actually route a real
/// request to a physical model: the provider is asked for `large`, the persisted assistant turn
/// names `faux/large`, and the SELECTION is still `router/auto`.
///
/// Pins pi `agent-session.ts:798-830` (the whole arm) end to end.
///
/// RED-PROVE: make `PolicyHooks::prepare_request` skip the routing step (return the inner update) —
/// the request then carries the VIRTUAL model, `ProviderStreamFn`'s guard answers the
/// `must be routed before streaming` terminal, the provider is never called, and all three
/// assertions fail.
#[tokio::test]
async fn a_registered_virtual_model_routes_a_real_request_to_a_physical_model() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(vec![Reply::Text("routed answer")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let log = install_router(
        &session,
        &physical,
        always("large", ModelThinkingLevel::High),
    );
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "hello").await;

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[SeenRequest {
            model: "large".to_string(),
            reasoning: ModelThinkingLevel::High,
        }],
        "the provider must be asked for the ROUTED physical model, at the routed level"
    );
    let turns = assistants(&session).await;
    assert_eq!(turns.len(), 1, "one assistant turn");
    assert_eq!(turns[0].provider.as_str(), "faux");
    assert_eq!(turns[0].model, "large");
    assert_eq!(turns[0].stop_reason, StopReason::Stop);
    assert_eq!(
        turns[0].thinking_level,
        Some(ModelThinkingLevel::High),
        "the loop records the level the request was MADE with (pi agent-loop.ts:409)"
    );
    let selection = session.model().expect("selection");
    assert_eq!(selection.provider.as_str(), "router");
    assert_eq!(selection.model.as_str(), "auto");
    assert_eq!(
        selection.api.as_ref().map(cyrup_core::ApiId::as_str),
        Some(cyrup_core::VIRTUAL_MODEL_API),
        "the selection stays virtual; only the request was overridden"
    );
    let calls = log.lock().unwrap();
    assert_eq!(calls.len(), 1, "exactly one route per request");
    assert_eq!(calls[0].reason, ModelRouteReason::User);
    assert_eq!(calls[0].previous, None, "nothing answered before this");
    assert_eq!(calls[0].failed, None);
    assert_eq!(calls[0].state, None, "no state stored yet");
}

/// Every request routes, and the reason distinguishes the user's turn from the tool-result
/// continuation inside the same run — pi `reason: failed ? "retry" : userTurn ? "user" :
/// "continuation"` (`agent-session.ts:807-811`). The continuation's `previous` must name the
/// physical model that answered the first request, with the level it was asked at.
///
/// RED-PROVE (reason): hardcode `ModelRouteReason::User` in `route_request` — the second reason
/// assertion fails. RED-PROVE (`previous`): build `previous` from the selection instead of
/// `find_latest_response` + `physical_model` — it comes back `Some(("auto", _))` or `None`.
/// RED-PROVE (`previous.thinking_level`): drop the `Settled::with_thinking_level` stamp in
/// `stream_assistant` — the level comes back `None`.
#[tokio::test]
async fn every_request_routes_and_a_tool_continuation_is_not_a_user_turn() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(vec![Reply::Call("echo"), Reply::Text("done")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = config(&fx);
    cfg.no_extensions = false;
    let session = SessionBuilder::new(provider, cfg)
        .with_native_extension(Arc::new(EchoExt))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let log = install_router(
        &session,
        &physical,
        always("large", ModelThinkingLevel::Low),
    );
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "use the tool").await;

    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "the tool result produced a second provider request"
    );
    let calls = log.lock().unwrap();
    assert_eq!(
        calls.iter().map(|c| c.reason).collect::<Vec<_>>(),
        vec![ModelRouteReason::User, ModelRouteReason::Continuation],
        "one route per request, and the continuation is not a user turn"
    );
    assert_eq!(calls[0].previous, None);
    assert_eq!(
        calls[1].previous,
        Some(("large".to_string(), Some(ModelThinkingLevel::Low))),
        "previous names the PHYSICAL model that answered, at the level it was asked at"
    );
}

/// A retry routes as `Retry` and the router is handed the FAILED physical request — which the
/// transcript can no longer supply, because `prepare_retry` dropped that message from it.
///
/// Pins pi's stash (`agent-session.ts:1852`), its consumption (`:778-779`) and `resolveModel`'s
/// mapping of `failed` through `getPhysicalModel` (`model-runtime.ts:1003`).
///
/// RED-PROVE: remove `stash_failed_response` from `handle_post_agent_run`'s retry arm — the second
/// reason comes back `User` and `failed` comes back `None`, while the first assertion still passes.
#[tokio::test]
async fn a_retry_routes_as_a_retry_and_reports_the_failed_physical_request() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(
        vec![
            Reply::RetryableError("overloaded_error"),
            Reply::Text("second attempt"),
        ],
        &faux,
    );
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .cli_settings(fast_retry_settings())
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let log = install_router(
        &session,
        &physical,
        always("large", ModelThinkingLevel::Off),
    );
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "hard question").await;

    assert_eq!(seen.lock().unwrap().len(), 2, "the run retried once");
    let calls = log.lock().unwrap();
    assert_eq!(
        calls.iter().map(|c| c.reason).collect::<Vec<_>>(),
        vec![ModelRouteReason::User, ModelRouteReason::Retry]
    );
    assert_eq!(
        calls[1].failed,
        Some(("large".to_string(), Some("overloaded_error".to_string()))),
        "the router is handed the failed PHYSICAL request and its error"
    );
}

/// Router state round-trips through the branch: it is appended as a `pi.virtual-model-state`
/// custom entry with pi's camelCase `{provider, modelId, state}`, handed back to the NEXT request,
/// and NOT re-appended when the router returns it unchanged.
///
/// Pins pi `:809` (read), `:817-823` (write, "`route.state !== undefined && route.state !==
/// state`") and `virtual-models.ts:36-40` (the data shape the already-landed
/// `cyrup_session::virtual_models::virtual_model_state` reader matches on).
///
/// RED-PROVE (append): make `persist_router_state` append whenever the router returned ANY state —
/// a second entry appears for the continuation that returned the same state and the
/// `state_entries().len() == 1` assertion fails. RED-PROVE (read): pass `state: None` into
/// `RouteOptions` — the second call's `state` comes back `None`.
#[tokio::test]
async fn router_state_is_stored_on_the_branch_and_handed_to_the_next_request() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    script(vec![Reply::Call("echo"), Reply::Text("done")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = config(&fx);
    cfg.no_extensions = false;
    let session = SessionBuilder::new(provider, cfg)
        .with_native_extension(Arc::new(EchoExt))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    // Store `{turns: 1}` on the user turn and then return the SAME state unchanged on the
    // continuation — upstream's own router shape, and the only one that distinguishes
    // "append when it differs" from "append whenever something came back".
    let decide: Decide = Arc::new(|call| {
        let state = match call.reason {
            ModelRouteReason::User => Some(json!({"turns": 1})),
            _ => call.state.clone(),
        };
        Ok(("large".to_string(), ModelThinkingLevel::Off, state))
    });
    let log = install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "use the tool").await;

    let calls = log.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[0].state, None,
        "nothing stored before the first route"
    );
    assert_eq!(
        calls[1].state,
        Some(json!({"turns": 1})),
        "the stored state is read back off the branch for the next request"
    );
    let entries = state_entries(&session).await;
    assert_eq!(
        entries,
        vec![json!({"provider": "router", "modelId": "auto", "state": {"turns": 1}})],
        "exactly one entry, in pi's camelCase shape; the unchanged state appends nothing"
    );
}

/// A routing failure ends the run with an error response that still names the VIRTUAL model, with
/// api `pi-virtual` — which is what `cyrup_session::virtual_models::branch_selection` skips when it
/// restores a selection, and what upstream documents ("Failed routing leaves the virtual model on
/// its message", `virtual-models.ts:104`).
///
/// The provider must never be reached.
///
/// RED-PROVE: swallow the routing error in `PolicyHooks::prepare_request` and fall through to the
/// inner update — the provider is then called once and the terminal turn names `faux/...` (or the
/// unrouted-stream error) instead of `router/auto` with the router's own text.
#[tokio::test]
async fn a_routing_failure_ends_the_run_on_the_virtual_model() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(vec![Reply::Text("never reached")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let decide: Decide = Arc::new(|_| Err("router exploded".to_string()));
    install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "hello").await;

    assert!(
        seen.lock().unwrap().is_empty(),
        "a routing failure must not reach the provider"
    );
    let turns = assistants(&session).await;
    let last = turns.last().expect("a terminal assistant turn");
    assert_eq!(last.provider.as_str(), "router");
    assert_eq!(last.model, "auto");
    assert_eq!(last.api.as_str(), cyrup_core::VIRTUAL_MODEL_API);
    assert_eq!(last.stop_reason, StopReason::Error);
    assert!(
        last.error_message
            .as_deref()
            .is_some_and(|m| m.contains("router exploded")),
        "the router's own message reaches the user: {:?}",
        last.error_message
    );
}

/// A route to another VIRTUAL model is refused, with pi's exact text — upstream's "a router cannot
/// route to another virtual model" (`model-runtime.ts:1019-1020`).
///
/// RED-PROVE: drop the `is_virtual_model` rejection from `AgentSession::physical_model` — the route
/// is accepted, the virtual model reaches the provider swap, and the terminal error becomes the
/// unrouted-stream one instead of `is not a physical model`.
#[tokio::test]
async fn a_route_to_another_virtual_model_is_refused() {
    let fx = fixture();
    let faux = two_model_faux();
    let mut physical = faux.models().to_vec();
    // A SECOND virtual model, in the catalog as a routing target.
    let second = create_virtual_model(&VirtualModelSpec {
        provider: "router".into(),
        id: "second".into(),
        name: "Second".to_string(),
        thinking_levels: None,
        context_window: None,
        max_tokens: None,
        input: None,
    });
    physical.push(second.clone());
    let seen = script(vec![Reply::Text("never reached")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let log = install_router(
        &session,
        &physical,
        always("second", ModelThinkingLevel::Off),
    );
    // Register the target too, so it is a real virtual catalog row and not merely an unknown id.
    session
        .register_virtual_model(VirtualModelDefinition::new(
            VirtualModelSpec {
                provider: "router".into(),
                id: "second".into(),
                name: "Second".to_string(),
                thinking_levels: None,
                context_window: None,
                max_tokens: None,
                input: None,
            },
            Arc::new(TestRouter {
                log: Arc::new(Mutex::new(Vec::new())),
                catalog: Vec::new(),
                decide: always("small", ModelThinkingLevel::Off),
            }),
        ))
        .expect("register router/second");
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "hello").await;

    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(log.lock().unwrap().len(), 1, "the router did answer");
    let turns = assistants(&session).await;
    let last = turns.last().expect("a terminal assistant turn");
    assert_eq!(last.stop_reason, StopReason::Error);
    assert_eq!(
        last.error_message.as_deref(),
        Some("Virtual model router/auto routed to router/second, which is not a physical model."),
        "pi's exact refusal text (model-runtime.ts:1019-1020)"
    );
}

/// A manual `/compact` under a virtual selection routes its summarization request as `Direct`,
/// reads no state and stores none — pi `_getSummarizationRequestAuth` (`agent-session.ts:560-581`,
/// "Route a virtual model first: summaries size their input and output from the model they get")
/// and the `direct` rule "direct requests have no state, and Pi ignores state they return".
///
/// RED-PROVE: hand the summarizer the SELECTION instead of `summarization_model()`'s answer — the
/// summarization request is made for `router/auto`, `ProviderStreamFn`'s guard answers the
/// unrouted-stream error, the compaction fails and `compact()` returns `Err`.
#[tokio::test]
async fn a_compaction_summary_routes_as_direct_and_keeps_no_state() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(
        vec![
            Reply::Text("an answer"),
            Reply::Text("another answer"),
            Reply::Text("CONTEXT SUMMARY"),
            // Slack, so the compaction never starves whichever prompt it sizes itself from; the
            // assertions below count ROUTES, not provider calls, which is the point — upstream
            // records ONE `direct` route however many summary calls it makes.
            Reply::Text("EXTRA SUMMARY"),
            Reply::Text("EXTRA SUMMARY"),
        ],
        &faux,
    );
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .cli_settings(compactable_settings())
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    // Return state on every call, so "direct state is discarded" is observable rather than vacuous.
    let decide: Decide = Arc::new(|call| {
        Ok((
            "large".to_string(),
            ModelThinkingLevel::Off,
            Some(json!({"seen": call.reason.as_str()})),
        ))
    });
    let log = install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "hello").await;
    prompt_and_settle(&session, "and again").await;
    session.compact(None).await.expect("compact");

    let requests = seen.lock().unwrap().clone();
    assert!(
        requests.len() >= 3,
        "two turns and at least one summary call: {requests:?}"
    );
    assert!(
        requests.iter().all(|r| r.model == "large"),
        "every request — the turns AND the summary — went to the routed physical model, never to \
         `router/auto`: {requests:?}"
    );
    let calls = log.lock().unwrap().clone();
    assert_eq!(
        calls.iter().map(|c| c.reason).collect::<Vec<_>>(),
        vec![
            ModelRouteReason::User,
            ModelRouteReason::User,
            ModelRouteReason::Direct
        ],
        "one route per turn, and one `direct` route for the compaction"
    );
    assert_eq!(
        calls[2].state, None,
        "a direct request is handed no state (pi never reads the branch for one)"
    );
    let entries = state_entries(&session).await;
    assert_eq!(
        entries,
        vec![json!({"provider": "router", "modelId": "auto", "state": {"seen": "user"}})],
        "the direct route's returned state is DISCARDED, and the second turn returned the same \
         state as the first, so exactly one entry stands"
    );
}

/// A context OVERFLOW on the routed physical model compacts and retries, and the retry is routed
/// as `Retry` — which requires `same_model` to be computed from pi's `_modelForMessage`
/// (`agent-session.ts:2949-2951`) rather than from the SELECTION.
///
/// Under a virtual selection the selection never equals the response's model, so comparing against
/// it makes `check_compaction`'s whole overflow arm unreachable: no compaction, no retry, and a
/// user left staring at an overflow error on a session that could have recovered.
///
/// RED-PROVE (`_modelForMessage`): put `same_model` back to comparing the assistant against the
/// SELECTION — no compaction runs at all and the reasons come back `[User]`.
/// RED-PROVE (the stash): remove `stash_failed_response` from `check_compaction`'s overflow arm —
/// the compaction still runs but the retried request is routed as `User` instead of `Retry`, which
/// upstream's own comment rules out ("Compaction may fold the prompt into the summary, so the retry
/// is not a new user turn").
#[tokio::test]
async fn an_overflow_on_the_routed_model_compacts_and_retries_as_a_retry() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(
        vec![
            // One ordinary turn first, so the overflow recovery has something to compact.
            Reply::Text("first answer"),
            // `prompt is too long` is an OVERFLOW pattern, so this is not a retryable transient
            // error: `is_retryable_error` declines it and it reaches `check_compaction`'s
            // overflow arm instead.
            Reply::ErrorText("prompt is too long"),
            Reply::Text("CONTEXT SUMMARY"),
            Reply::Text("second attempt"),
            Reply::Text("EXTRA SUMMARY"),
            Reply::Text("EXTRA SUMMARY"),
        ],
        &faux,
    );
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .cli_settings(compactable_settings())
        .build()
        .await
        .expect("build")
        .into_shared();
    let log = install_router(
        &session,
        &physical,
        always("large", ModelThinkingLevel::Off),
    );
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "a short prompt").await;
    prompt_and_settle(&session, "a long prompt").await;

    assert!(
        seen.lock().unwrap().len() >= 3,
        "the overflow was recovered from, so a third request was made: {:?}",
        seen.lock().unwrap()
    );
    let calls = log.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .map(|c| c.reason)
            // The compaction's own summarization route is `Direct`; this assertion is about the
            // AGENT-LOOP requests either side of it.
            .filter(|r| *r != ModelRouteReason::Direct)
            .collect::<Vec<_>>(),
        vec![
            ModelRouteReason::User,
            ModelRouteReason::User,
            ModelRouteReason::Retry
        ],
        "the compact-and-retry request is a retry, not a new user turn"
    );
}

/// REGRESSION GUARD, not a red proof (it passes with the routing step removed, by construction).
/// A session with a PHYSICAL selection must behave exactly as it did before this feature existed:
/// the router is never consulted, no state entry is ever written, and the request goes to the
/// selected model.
#[tokio::test]
async fn a_physical_selection_routes_nothing() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(vec![Reply::Text("plain answer")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let log = install_router(
        &session,
        &physical,
        always("large", ModelThinkingLevel::High),
    );
    session.set_model("faux/small").await.expect("select");

    prompt_and_settle(&session, "hello").await;

    assert_eq!(
        seen.lock().unwrap()[0].model,
        "small",
        "the selected physical model is what the provider sees"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "no router is consulted for a physical selection"
    );
    assert!(state_entries(&session).await.is_empty());
}

/// REGRESSION GUARD, not a red proof. A registered virtual model is LISTED and SELECTABLE: it
/// appears in the composed catalog under a provider id nothing else defines, and
/// `available_model_catalog` offers it even though `router` has no credentials — pi's provisional
/// `{type: "api_key", source: "virtual"}` marking (`model-runtime.ts:962-968`).
#[tokio::test]
async fn a_virtual_only_provider_is_listed_and_available() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    assert!(
        !session
            .full_model_catalog()
            .iter()
            .any(|m| m.provider.as_str() == "router"),
        "precondition: nothing defines `router` before registration"
    );
    install_router(
        &session,
        &physical,
        always("large", ModelThinkingLevel::Off),
    );
    assert!(
        session
            .full_model_catalog()
            .iter()
            .any(|m| m.provider.as_str() == "router" && m.id.as_str() == "auto"),
        "the registration reaches the composed catalog (the fourth cache key)"
    );
    assert!(
        session
            .available_model_catalog()
            .iter()
            .any(|m| m.provider.as_str() == "router" && m.id.as_str() == "auto"),
        "a virtual-only provider counts as configured"
    );
}

/// Upstream's own pin, ported: **"does not route compactions that an extension supplies"**
/// (`test/suite/virtual-models.test.ts:346-366` @v1.0.4), whose router THROWS on `direct` and
/// whose assertion is `reasons() == ["user", "user"]`.
///
/// Why it must hold: upstream routes inside `_runDefaultCompaction`
/// (`core/agent-session.ts:2704-2727`), which is reached ONLY from the `else` arm of
/// `if (extensionCompaction)` (`:2806-2815`). An extension-supplied compaction makes no model call
/// at all, so there is nothing to route, and a router that is unavailable for `direct` requests
/// must not break a compaction that never needed it.
///
/// RED-PROVE: this test was written against the tree that called `summarization_model()` at the
/// TOP of `AgentSession::compact` (`session/compaction.rs:74`, before the `session_before_compact`
/// hook and before the nothing-to-compact check). It failed with
/// `compact: VirtualModelRouting("the router is unavailable for direct requests")` — the router
/// WAS asked, and its refusal failed a compaction the extension had already supplied.
#[tokio::test]
async fn an_extension_supplied_compaction_is_not_routed() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(
        vec![Reply::Text("first answer"), Reply::Text("second answer")],
        &faux,
    );
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .cli_settings(compactable_settings())
        .with_native_extension(Arc::new(SuppliesCompactionExt))
        .build()
        .await
        .expect("build")
        .into_shared();
    // Upstream's router verbatim: `if (request.reason === "direct") throw new Error("router
    // unavailable")`.
    let decide: Decide = Arc::new(|call| {
        if call.reason == ModelRouteReason::Direct {
            return Err("the router is unavailable for direct requests".to_string());
        }
        Ok(("large".to_string(), ModelThinkingLevel::Off, None))
    });
    let log = install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "first").await;
    prompt_and_settle(&session, "second").await;

    let result = session
        .compact(None)
        .await
        .expect("an extension-supplied compaction needs no route");

    assert_eq!(result.summary, "extension summary");
    assert_eq!(
        log.lock()
            .unwrap()
            .iter()
            .map(|c| c.reason)
            .collect::<Vec<_>>(),
        vec![ModelRouteReason::User, ModelRouteReason::User],
        "one route per turn and NO `direct` route: the extension supplied the summary"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        2,
        "two turns and no summarization call at all"
    );
}

/// A session with nothing to compact must not invoke the router either — upstream's
/// `prepareCompaction` check (`core/agent-session.ts:2765-2773` manual, `:3096-3100` auto) sits
/// ahead of `_runDefaultCompaction`, so the `direct` route is never reached.
///
/// RED-PROVE: against the same pre-fix tree this failed with
/// `compact: VirtualModelRouting("the router is unavailable for direct requests")` where the fixed
/// tree answers `NothingToCompact` — the router was asked before the session was even checked for
/// anything to summarize, so a `direct`-refusing router turned "nothing to compact" into a routing
/// failure and reported the wrong reason to the user.
#[tokio::test]
async fn a_session_with_nothing_to_compact_does_not_invoke_the_router() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let provider: Arc<dyn Provider> = faux;
    // Default settings, NOT `compactable_settings()`: a one-turn session is far below the budget.
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();
    let decide: Decide = Arc::new(|call| {
        if call.reason == ModelRouteReason::Direct {
            return Err("the router is unavailable for direct requests".to_string());
        }
        Ok(("large".to_string(), ModelThinkingLevel::Off, None))
    });
    let log = install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");

    let err = session
        .compact(None)
        .await
        .expect_err("a session this small has nothing to compact");
    assert!(
        matches!(
            err,
            crate::error::SessionServiceError::NothingToCompact
                | crate::error::SessionServiceError::AlreadyCompacted
        ),
        "the refusal is about the transcript, not about the router: {err:?}"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "the router was never asked: {:?}",
        log.lock().unwrap()
    );
}

/// The AUTO twin of [`an_extension_supplied_compaction_is_not_routed`]: an overflow recovery that
/// an extension supplies takes no route either — pi's auto path reaches
/// `_runDefaultCompaction` only from its own `else` arm (`core/agent-session.ts:3140-3156`
/// @v1.0.4), the same shape as the manual one.
///
/// RED-PROVE: against the tree that called `summarization_model()` at the top of
/// `run_auto_compaction` (`session/auto_compaction.rs:263`, before `compaction_start` and before
/// the hook), the `direct`-refusing router was asked and `?`-propagated its refusal out of
/// `run_auto_compaction`, so the overflow recovery never ran: the log carried a `Direct` reason
/// and the extension's summary never landed.
#[tokio::test]
async fn an_extension_supplied_auto_compaction_is_not_routed() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let seen = script(
        vec![
            Reply::Text("first answer"),
            Reply::ErrorText("prompt is too long"),
            Reply::Text("second attempt"),
        ],
        &faux,
    );
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .cli_settings(compactable_settings())
        .with_native_extension(Arc::new(SuppliesCompactionExt))
        .build()
        .await
        .expect("build")
        .into_shared();
    let decide: Decide = Arc::new(|call| {
        if call.reason == ModelRouteReason::Direct {
            return Err("the router is unavailable for direct requests".to_string());
        }
        Ok(("large".to_string(), ModelThinkingLevel::Off, None))
    });
    let log = install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");

    prompt_and_settle(&session, "a short prompt").await;
    prompt_and_settle(&session, "a long prompt").await;

    let calls = log.lock().unwrap().clone();
    assert!(
        calls.iter().all(|c| c.reason != ModelRouteReason::Direct),
        "the extension supplied the compaction, so no `direct` route was taken: {calls:?}"
    );
    assert!(
        calls.iter().any(|c| c.reason == ModelRouteReason::Retry),
        "the overflow recovery still happened and its retry is a `Retry`: {calls:?}"
    );
    let summaries: Vec<String> = session
        .entries_json()
        .await
        .into_iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("compaction"))
        .filter_map(|e| {
            e.get("summary")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .collect();
    assert_eq!(
        summaries,
        vec!["extension summary".to_string()],
        "the extension's summary is what landed, and no summarization call was made"
    );
    let requests = seen.lock().unwrap().clone();
    assert!(
        requests.iter().all(|r| r.model == "large"),
        "no request ever named `router/auto`: {requests:?}"
    );
}
