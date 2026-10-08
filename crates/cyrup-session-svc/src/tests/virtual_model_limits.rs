//! CONTEXT LIMITS AND COMPACTION UNDER A VIRTUAL SELECTION — the three behaviours
//! `packages/coding-agent/docs/virtual-models.md:29` states, read at **v1.0.4**:
//!
//! > Context usage uses the limits of the physical model that produced the latest response, even
//! > if that response came before switching to the virtual model; without such a response, the
//! > limits declared on the virtual model. Compaction uses the same limits, and also the limits of
//! > the model each request is routed to, compacting before sending if that window is too small
//! > while leaving the route as the router chose it.
//!
//! Upstream implements all three with four tiny pieces and NO change to the compaction algorithm:
//! `findLatestResponse` (`virtual-models.ts:109-118`), `get routedModel()`
//! (`agent-session.ts:1437-1443`), `_limitsModel()` (`:627-630`) and `_modelForMessage()`
//! (`:598-607`). Every reader that needs a window then goes through one of the last two:
//! `getContextUsage` (`:4222-4227`), `_checkCompaction` (`:2949-2951`), `_isRetryableError`
//! (`:3696`) and the routed-window check inside the routing step (`:754-762`, `:824-829`).
//!
//! **Why this matters at all**: a virtual model declares `contextWindow 0` by default
//! (`virtual-models.ts:184`), and cyrup computes its threshold as
//! `window.saturating_sub(reserve)`. Read the window off the SELECTION and a routed session either
//! auto-compacts on every single turn (declared window too small) or loses its context meter
//! entirely (declared window 0 reads as "unknown").
//!
//! The fixture is upstream's own (`test/suite/virtual-models.test.ts:40-62`): faux `small`
//! (window 1000) and `large` (window 50_000, maxTokens 4000, reasoning), plus a virtual
//! `router/auto` DECLARING window 1000 — so "the selection's window" and "the routed model's
//! window" are different numbers in every case below and no assertion can pass by coincidence.
//! The router is upstream's `defaultRoute`, ported literally.
//!
//! Each test names the behaviour it pins and the neutering it is red against. The one marked
//! `REGRESSION GUARD` passes with the behaviour removed, by construction, and is not a red proof.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    VirtualModelDefinition, VirtualModelSpec,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{AgentSession, AgentSessionEvent, CompactionReason, SessionBuilder, SessionConfig};

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

/// Upstream's fixture (`test/suite/virtual-models.test.ts:43-46`). The two windows are far apart
/// ON PURPOSE: 1000 is also the virtual model's declared window, so "the selection" and "the model
/// that answered" can never be confused for one another.
const SMALL_WINDOW: u64 = 1_000;
const LARGE_WINDOW: u64 = 50_000;

fn two_model_faux() -> Arc<FauxProvider> {
    let mut small = FauxModelDefinition::new("small");
    small.context_window = SMALL_WINDOW;
    let mut large = FauxModelDefinition::new("large");
    large.context_window = LARGE_WINDOW;
    large.max_tokens = 4000;
    large.reasoning = true;
    Arc::new(FauxProvider::with_config(FauxConfig {
        models: vec![small, large],
        ..FauxConfig::default()
    }))
}

/// `router/auto` declaring upstream's `contextWindow: 1000`
/// (`test/suite/virtual-models.test.ts:57`).
fn router_spec() -> VirtualModelSpec {
    VirtualModelSpec {
        provider: "router".into(),
        id: "auto".into(),
        name: "Auto".to_string(),
        thinking_levels: Some(vec![ModelThinkingLevel::Low, ModelThinkingLevel::High]),
        context_window: Some(SMALL_WINDOW),
        max_tokens: None,
        input: None,
    }
}

/// One `route()` call, flattened to what the port of `defaultRoute` reads.
#[derive(Debug, Clone, PartialEq)]
struct Recorded {
    reason: ModelRouteReason,
    thinking_level: ModelThinkingLevel,
    previous: Option<(String, Option<ModelThinkingLevel>)>,
    failed: Option<(String, Option<ModelThinkingLevel>)>,
}

type Decide = Arc<dyn Fn(&Recorded) -> Result<(String, ModelThinkingLevel), String> + Send + Sync>;

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
            failed: request
                .failed
                .as_ref()
                .map(|f| (f.model.id.as_str().to_string(), f.thinking_level)),
        };
        self.log.lock().unwrap().push(recorded.clone());
        let (id, level) = (self.decide)(&recorded).map_err(ModelRouteError::new)?;
        let model = self
            .catalog
            .iter()
            .find(|m| m.id.as_str() == id)
            .cloned()
            .ok_or_else(|| ModelRouteError::new(format!("test router: no model {id}")))?;
        Ok(ModelRoute {
            model,
            thinking_level: level,
            state: None,
        })
    }
}

/// Upstream's `defaultRoute` (`test/suite/virtual-models.test.ts:19-30`), ported literally:
/// *"new user turns pick by thinking level, everything else stays put"*.
fn default_route() -> Decide {
    Arc::new(|call: &Recorded| {
        if call.reason == ModelRouteReason::Direct {
            return Ok(("large".to_string(), ModelThinkingLevel::Low));
        }
        let sticky = call.failed.clone().or_else(|| call.previous.clone());
        if call.reason != ModelRouteReason::User
            && let Some((id, level)) = sticky
        {
            return Ok((id, level.unwrap_or(ModelThinkingLevel::High)));
        }
        if call.thinking_level == ModelThinkingLevel::High {
            Ok(("large".to_string(), ModelThinkingLevel::High))
        } else {
            Ok(("small".to_string(), ModelThinkingLevel::Off))
        }
    })
}

/// Upstream's continuation router (`test/suite/virtual-models.test.ts:261-264`): a CONTINUATION
/// goes to `small`, everything else follows `defaultRoute`.
fn continuations_to_small() -> Decide {
    let base = default_route();
    Arc::new(move |call: &Recorded| {
        if call.reason == ModelRouteReason::Continuation {
            return Ok(("small".to_string(), ModelThinkingLevel::Off));
        }
        base(call)
    })
}

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

/// One provider request as the faux provider saw it.
#[derive(Debug, Clone, PartialEq)]
struct SeenRequest {
    model: String,
    reasoning: ModelThinkingLevel,
}

#[derive(Clone)]
enum Reply {
    Text(&'static str),
    /// Text of a given length, for making the context genuinely large.
    Filler(char, usize),
    Call(&'static str),
    /// A terminal error with arbitrary text.
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
                match &reply {
                    Reply::Text(t) => {
                        faux_assistant_message(vec![faux_text((*t).to_string())], StopReason::Stop)
                    }
                    Reply::Filler(c, n) => faux_assistant_message(
                        vec![faux_text(std::iter::repeat_n(*c, *n).collect::<String>())],
                        StopReason::Stop,
                    ),
                    Reply::Call(name) => faux_assistant_message(
                        vec![faux_tool_call((*name).to_string(), json!({}))],
                        StopReason::ToolUse,
                    ),
                    Reply::ErrorText(text) => faux_assistant_message_with(
                        Vec::new(),
                        StopReason::Error,
                        FauxMessageOptions {
                            error_message: Some((*text).to_string()),
                            ..FauxMessageOptions::default()
                        },
                    ),
                }
            })
        })
        .collect();
    faux.set_response_steps(steps);
    seen
}

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
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::text("echoed")],
            ..Default::default()
        })
    }
}

struct EchoExt;

#[async_trait::async_trait]
impl NativeExtension for EchoExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("virtual-limits-echo")
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

/// `keepRecentTokens: 1, reserveTokens: 0` — upstream's own compaction settings for the two
/// routed-window cases (`test/suite/virtual-models.test.ts:230`, `:270`).
fn tight_compaction_settings() -> cyrup_config::Settings {
    let mut cli = cyrup_config::Settings::new();
    cli.set_field(
        "compaction",
        json!({"enabled": true, "keepRecentTokens": 1, "reserveTokens": 0}),
    )
    .unwrap();
    cli
}

/// Every `compaction_start` reason, in order, collected from a live subscription for the whole
/// test — upstream's `harness.eventsOfType("compaction_start")`.
fn collect_compactions(session: &Arc<AgentSession>) -> Arc<Mutex<Vec<CompactionReason>>> {
    let out = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&out);
    let mut stream = session.subscribe();
    tokio::spawn(async move {
        while let Some(ev) = stream.next().await {
            if let AgentSessionEvent::CompactionStart { reason } = ev {
                sink.lock().unwrap().push(reason);
            }
        }
    });
    out
}

async fn prompt_and_settle(session: &AgentSession, text: &str) {
    let _ = session.prompt(text).await.expect("prompt");
    session.wait_for_idle().await;
}

/// The persisted assistant turns as upstream's `dispatched()` spells them —
/// `provider/model:thinkingLevel` (`test/suite/virtual-models.test.ts:74-77`).
async fn dispatched(session: &AgentSession) -> Vec<String> {
    session
        .messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_core::Message::Assistant(a) => Some(format!(
                "{}/{}:{}",
                a.provider.as_str(),
                a.model,
                a.thinking_level
                    .map_or_else(|| "none".to_string(), crate::builder::thinking_level_to_str)
            )),
            _ => None,
        })
        .collect()
}

async fn last_assistant(session: &AgentSession) -> AssistantMessage {
    session
        .messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_core::Message::Assistant(a) => Some(a),
            _ => None,
        })
        .next_back()
        .expect("an assistant turn")
}

/// Build a routed session: the two-model faux provider, `router/auto` selected, level `high`.
async fn routed_session(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    decide: Decide,
    settings: Option<cyrup_config::Settings>,
) -> (Arc<AgentSession>, Arc<Mutex<Vec<Recorded>>>) {
    let physical = faux.models().to_vec();
    let provider: Arc<dyn Provider> = faux;
    let mut builder = SessionBuilder::new(provider, config(fx))
        .with_native_extension(Arc::new(EchoExt) as Arc<dyn NativeExtension>);
    if let Some(s) = settings {
        builder = builder.cli_settings(s);
    }
    let session = builder
        .build()
        .await
        .expect("build")
        // Bind the self-handle: `PolicyHooks` reaches the session through it, so an UNBOUND
        // by-value session routes nothing at all.
        .into_shared();
    let log = install_router(&session, &physical, decide);
    session.set_model("router/auto").await.expect("select");
    session
        .set_thinking_level(ModelThinkingLevel::High)
        .await
        .expect("level");
    (session, log)
}

// ------------------------------------------------------------------------------- tests ----

/// **Behaviour 1.** *"Context usage uses the limits of the physical model that produced the latest
/// response."* Upstream asserts exactly this at `test/suite/virtual-models.test.ts:99-100`:
/// `getContextUsage()?.contextWindow` is `50_000` — the `large` model's — and not the virtual
/// model's declared `1000`.
///
/// RED-PROVE: make `AgentSession::limits_model` answer the selection unconditionally (i.e. drop
/// the `routed_model()` arm, which is the pre-step behaviour) — the window comes back `1000`.
#[tokio::test]
async fn context_usage_reports_the_window_of_the_physical_model_that_answered() {
    let fx = fixture();
    let faux = two_model_faux();
    let seen = script(vec![Reply::Text("answer")], &faux);
    let (session, _log) = routed_session(&fx, faux, default_route(), None).await;

    prompt_and_settle(&session, "hello").await;

    assert_eq!(
        seen.lock().unwrap().as_slice(),
        &[SeenRequest {
            model: "large".to_string(),
            reasoning: ModelThinkingLevel::High,
        }],
        "the turn was routed to `large`, so `large` is the model whose limits apply"
    );
    let usage = session
        .stats_context_usage()
        .await
        .expect("a window is known once a physical model has answered");
    assert_eq!(
        usage.context_window, LARGE_WINDOW,
        "the limits follow the PHYSICAL model that answered, not the virtual selection's declared \
         window of {SMALL_WINDOW}"
    );
    // And the coarse reader must agree: SEAM-115 pins ONE producer for occupancy.
    assert_eq!(
        session.context_usage().await.context_window,
        LARGE_WINDOW,
        "`context_usage` and `stats_context_usage` must not disagree about the window"
    );
}

/// **Behaviour 1, the `??` arm.** *"…without such a response, the limits declared on the virtual
/// model."* That is pi's `_limitsModel() { return this.routedModel?.model ?? this.model; }`
/// (`agent-session.ts:629`) with `routedModel` undefined because no settled response exists yet.
///
/// RED-PROVE: drop the `?? this.model` arm (return `None` from `limits_model` when there is no
/// routed model) — the window reads 0, `stats_context_usage` returns `None`, and the session has
/// no context meter at all before its first answer.
#[tokio::test]
async fn before_the_first_response_the_virtual_models_declared_window_applies() {
    let fx = fixture();
    let faux = two_model_faux();
    let _seen = script(vec![Reply::Text("unused")], &faux);
    let (session, log) = routed_session(&fx, faux, default_route(), None).await;

    let usage = session
        .stats_context_usage()
        .await
        .expect("the declared window is a window");
    assert_eq!(
        usage.context_window, SMALL_WINDOW,
        "with no response yet, the limits are the ones DECLARED on the virtual model"
    );
    assert!(
        log.lock().unwrap().is_empty(),
        "reading the limits must not consult the router"
    );
}

/// **Behaviour 1, across a routing FAILURE.** Upstream `test/suite/virtual-models.test.ts:193-215`
/// with its own comment: *"The failed attempt names the virtual model, whose declared window is
/// 1k; the large model's 50k applies."*
///
/// The failure leaves an assistant message naming `router/auto` with api `pi-virtual` and
/// `stopReason: error` on the branch. Two independent guards have to hold for the window to stay
/// at 50k: `latest_branch_response` must skip an errored turn, and `physical_model` must refuse a
/// virtual row.
///
/// RED-PROVE (the stop-reason skip): drop the `StopReason::Error` arm from
/// `cyrup_session::virtual_models::latest_branch_response` — the walk returns the `router/auto`
/// message, `physical_model` then refuses it, `limits_model` falls back to the selection and the
/// window reads 1000.
/// RED-PROVE (the api refusal): drop the `.filter(|m| !is_virtual_model(m))` from
/// `AgentSession::physical_model` — with the skip ALSO removed the virtual row resolves and the
/// window reads 1000; with the skip in place this one alone is masked, which is why both are
/// listed.
#[tokio::test]
async fn the_limits_survive_a_routing_failure() {
    let fx = fixture();
    let faux = two_model_faux();
    let seen = script(vec![Reply::Text("answer")], &faux);
    let fail = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let base = default_route();
    let decide: Decide = {
        let fail = Arc::clone(&fail);
        Arc::new(move |call: &Recorded| {
            if fail.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("router unavailable".to_string());
            }
            base(call)
        })
    };
    let (session, _log) = routed_session(&fx, faux, decide, None).await;

    prompt_and_settle(&session, "hello").await;
    fail.store(true, std::sync::atomic::Ordering::SeqCst);
    prompt_and_settle(&session, "again").await;

    let last = last_assistant(&session).await;
    assert_eq!(last.provider.as_str(), "router");
    assert_eq!(last.model, "auto");
    assert_eq!(last.api.as_str(), cyrup_core::VIRTUAL_MODEL_API);
    assert_eq!(last.stop_reason, StopReason::Error);
    assert!(
        last.error_message
            .as_deref()
            .is_some_and(|m| m.contains("router unavailable")),
        "the router's own message reaches the user: {:?}",
        last.error_message
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "the failed routing never reached a provider"
    );
    let usage = session
        .stats_context_usage()
        .await
        .expect("the last physical response still fixes the window");
    assert_eq!(
        usage.context_window, LARGE_WINDOW,
        "a routing failure must not drop the window to the virtual model's declared {SMALL_WINDOW}"
    );
}

/// **Behaviour 2, the negative half.** Upstream *"checks compaction against the physical model that
/// produced the response"* (`test/suite/virtual-models.test.ts:217-226`): a context that exceeds
/// the virtual model's declared 1k window but FITS the large model's 50k must not compact.
///
/// `reserveTokens: 0` so the two thresholds are exactly the two windows, 1000 and 50_000, and the
/// assertion below pins the occupancy as strictly between them — otherwise this test could pass
/// because the session happened to be tiny, or fail because cyrup's system prompt and tool
/// definitions are larger than upstream's harness's (they are, which is why the prompt here is
/// ~2k tokens where upstream's is ~20k; the DISCRIMINATOR is unchanged, since the virtual model's
/// threshold is 1000 either way).
///
/// RED-PROVE: make `AgentSession::model_for_message` return `None` under a virtual selection —
/// `check_compaction` then falls back to the selection's window (the virtual model's declared
/// 1000) and a `threshold` compaction fires on the very first turn.
#[tokio::test]
async fn no_compaction_while_the_routed_models_window_fits() {
    let fx = fixture();
    let faux = two_model_faux();
    let _seen = script(
        vec![
            Reply::Text("short answer"),
            Reply::Text("long answer"),
            // Slack, so a stray compaction (the failure this test is red against) does not also
            // starve the script and turn a clean failure into a confusing one.
            Reply::Text("SUMMARY"),
            Reply::Text("SUMMARY"),
        ],
        &faux,
    );
    let (session, _log) = routed_session(
        &fx,
        faux,
        default_route(),
        Some(tight_compaction_settings()),
    )
    .await;
    let compactions = collect_compactions(&session);

    prompt_and_settle(&session, "hello").await;
    prompt_and_settle(&session, &"x".repeat(8_000)).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    let usage = session
        .stats_context_usage()
        .await
        .expect("a window is known once a physical model has answered");
    assert_eq!(usage.context_window, LARGE_WINDOW);
    let tokens = usage.tokens.expect("an occupancy");
    assert!(
        tokens > SMALL_WINDOW && tokens < LARGE_WINDOW,
        "the fixture only discriminates while the occupancy sits BETWEEN the two windows:          {tokens} tokens"
    );
    assert_eq!(
        compactions.lock().unwrap().as_slice(),
        &[] as &[CompactionReason],
        "the window that matters is `large`'s 50k, which {tokens} tokens fit"
    );
}

/// **Behaviour 3.** *"Compaction also uses the limits of the model each request is routed to,
/// compacting before sending if that window is too small, while leaving the route as the router
/// chose it."* Upstream `test/suite/virtual-models.test.ts:228-257`.
///
/// The first turn is routed to `large` and answers ~2k tokens, which `large`'s 50k window fits —
/// no compaction. The thinking level then drops to `low`, so the next USER turn is routed to
/// `small`, whose 1k window it does not fit: exactly one `threshold` compaction runs BEFORE that
/// request is sent, and the request still goes to `small`, at the level the router chose.
///
/// RED-PROVE: remove the `compact_before_routed_request` call from
/// `AgentSession::route_request` — no `compaction_start` fires at all and the oversized request
/// reaches `small` unshrunk. (The pairing matters too: adding the `selection_is_virtual` early
/// return to `compact_before_next_assistant_response` WITHOUT this check gives the same failure,
/// which is why the two halves landed together.)
#[tokio::test]
async fn a_request_routed_to_a_smaller_window_compacts_before_it_is_sent() {
    let fx = fixture();
    let faux = two_model_faux();
    let seen = script(
        vec![
            Reply::Filler('y', 8_000),
            Reply::Text("SUMMARY"),
            Reply::Text("small answer"),
            Reply::Text("SUMMARY"),
            Reply::Text("SUMMARY"),
        ],
        &faux,
    );
    let (session, _log) = routed_session(
        &fx,
        faux,
        default_route(),
        Some(tight_compaction_settings()),
    )
    .await;
    let compactions = collect_compactions(&session);

    prompt_and_settle(&session, "hello").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        compactions.lock().unwrap().as_slice(),
        &[] as &[CompactionReason],
        "`large`'s 50k window fits the first turn, so nothing compacts yet"
    );

    // `low` routes the next user turn to `small` (upstream's `defaultRoute`).
    session
        .set_thinking_level(ModelThinkingLevel::Low)
        .await
        .expect("level");
    prompt_and_settle(&session, "next").await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        compactions.lock().unwrap().as_slice(),
        &[CompactionReason::Threshold],
        "exactly one threshold compaction, triggered by the ROUTED model's 1k window"
    );
    let requests = seen.lock().unwrap().clone();
    let agent_requests: Vec<&SeenRequest> = requests
        .iter()
        .filter(|r| r.model == "small" || r.reasoning == ModelThinkingLevel::High)
        .collect();
    assert_eq!(
        agent_requests
            .last()
            .map(|r| (r.model.as_str(), r.reasoning)),
        Some(("small", ModelThinkingLevel::Off)),
        "the route STANDS: the compaction does not re-ask the router, and the request still goes \
         to `small` at the level it chose — {requests:?}"
    );
    assert_eq!(
        dispatched(&session).await.last().map(String::as_str),
        Some("faux/small:off"),
        "and that is the model the answer is recorded against"
    );
}

/// **Behaviour 3, between the turns of ONE run.** Upstream
/// `test/suite/virtual-models.test.ts:259-289`: the first turn is a tool call routed to `large`,
/// and the CONTINUATION is routed to `small`, so the compaction has to happen mid-run rather than
/// at a prompt boundary. This is the case `compact_before_next_assistant_response`'s virtual early
/// return would silently lose if the routed check did not exist.
///
/// RED-PROVE: remove the `compact_before_routed_request` call from `route_request` — no
/// `compaction_start` fires and the continuation reaches `small` unshrunk.
#[tokio::test]
async fn a_continuation_routed_to_a_smaller_window_compacts_between_turns() {
    let fx = fixture();
    let faux = two_model_faux();
    let seen = script(
        vec![
            Reply::Call("echo"),
            Reply::Text("SUMMARY"),
            Reply::Text("small answer"),
            Reply::Text("SUMMARY"),
            Reply::Text("SUMMARY"),
        ],
        &faux,
    );
    let (session, log) = routed_session(
        &fx,
        faux,
        continuations_to_small(),
        Some(tight_compaction_settings()),
    )
    .await;
    let compactions = collect_compactions(&session);

    // ~2k tokens: fits the `large` model of the first turn, not the `small` model of the second.
    prompt_and_settle(&session, &"x".repeat(8_000)).await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        compactions.lock().unwrap().as_slice(),
        &[CompactionReason::Threshold],
        "the continuation's routed window is what triggers, mid-run"
    );
    let reasons: Vec<ModelRouteReason> = log
        .lock()
        .unwrap()
        .iter()
        .map(|c| c.reason)
        .filter(|r| *r != ModelRouteReason::Direct)
        .collect();
    assert_eq!(
        reasons,
        vec![ModelRouteReason::User, ModelRouteReason::Continuation],
        "one user turn and one continuation"
    );
    let requests = seen.lock().unwrap().clone();
    assert!(
        requests
            .iter()
            .any(|r| r.model == "small" && r.reasoning == ModelThinkingLevel::Off),
        "the continuation still went to `small` at the router's level: {requests:?}"
    );
}

/// **The retry/overflow split follows the routed model too** — pi `_isRetryableError`'s
/// `isContextOverflow(message, (this._modelForMessage(message) ?? this.model)?.contextWindow ?? 0)`
/// (`agent-session.ts:3696`).
///
/// A response whose reported input tokens exceed the ROUTED model's window is an overflow, so it
/// is COMPACTED rather than retried. Read against the selection instead and the window is the
/// virtual model's declared 1000 — which `is_context_overflow` would also call an overflow here,
/// so this test is built the other way round to discriminate: the error text is a transient one,
/// and the assertion is that a usage-driven overflow against `large`'s 50k never fires.
///
/// REGRESSION GUARD, not a red proof: it holds whichever window is read, because both windows
/// agree on this message. It is here to fence the retry path against a future change that makes
/// `model_for_message` answer a model the response did not come from.
#[tokio::test]
async fn a_transient_error_on_the_routed_model_is_not_read_as_an_overflow() {
    let fx = fixture();
    let faux = two_model_faux();
    let seen = script(vec![Reply::ErrorText("overloaded_error")], &faux);
    let (session, _log) = routed_session(&fx, faux, default_route(), None).await;
    let compactions = collect_compactions(&session);

    prompt_and_settle(&session, "hello").await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        compactions.lock().unwrap().as_slice(),
        &[] as &[CompactionReason],
        "a transient error is not an overflow, so nothing compacts"
    );
    assert_eq!(seen.lock().unwrap().len(), 1);
    let last = last_assistant(&session).await;
    assert_eq!(
        last.model, "large",
        "the error is recorded against the ROUTED model"
    );
}

/// REGRESSION GUARD, not a red proof. A session with a PHYSICAL selection must report exactly the
/// numbers it reported before this feature existed: the window is the selection's own, whatever
/// model answered. This is the inverse proof — it must stay GREEN when the virtual arm is deleted
/// from `limits_model` and `model_for_message`, and a failure here means the physical path moved.
#[tokio::test]
async fn a_physical_selection_reports_its_own_window() {
    let fx = fixture();
    let faux = two_model_faux();
    let physical = faux.models().to_vec();
    let _seen = script(vec![Reply::Text("answer")], &faux);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, config(&fx))
        .build()
        .await
        .expect("build")
        .into_shared();
    let log = install_router(&session, &physical, default_route());
    session.set_model("faux/large").await.expect("select");
    let compactions = collect_compactions(&session);

    prompt_and_settle(&session, "hello").await;

    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        log.lock().unwrap().is_empty(),
        "a physical selection consults no router"
    );
    assert_eq!(
        session
            .stats_context_usage()
            .await
            .expect("a window")
            .context_window,
        LARGE_WINDOW,
        "the selection's own window, as before"
    );
    assert_eq!(
        compactions.lock().unwrap().as_slice(),
        &[] as &[CompactionReason]
    );
}
