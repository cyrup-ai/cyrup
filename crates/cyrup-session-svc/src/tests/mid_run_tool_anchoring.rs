//! DRIFT-001 — message-anchored deferred tool loading, the ANCHORING half.
//!
//! Pi landed this in `3d8f7435` ("feat(ai): support message-anchored tool loading"). The shape is:
//! a tool runs, calls `pi.setActiveTools([...])`, and the wrapper around every extension-registered
//! tool (`coding-agent/src/core/extensions/wrapper.ts` `wrapRegisteredTool`) snapshots
//! `runner.getActiveTools()` either side of `execute`. When the change is PURELY ADDITIVE it stamps
//! the difference onto the result as `addedToolNames` (`agent/src/types.ts:362-363`). Downstream,
//! `splitDeferredTools` (`ai/src/utils/deferred-tools.ts`) reads that field back off the transcript
//! to decide WHERE a tool definition is placed. The load-bearing precondition for all of it is that
//! the newly-active tool is genuinely callable FROM THAT TURN ONWARD — Pi guarantees it with
//! `_installAgentNextTurnRefresh` (`coding-agent/src/core/agent-session.ts:519-540`), whose
//! `prepareNextTurnWithContext` returns `context: {...previousContext, systemPrompt, tools:
//! this.agent.state.tools.slice()}` on EVERY turn.
//!
//! cyrup had no such refresh. `RunCtx` snapshots `st.tools` once at run start (`agent.rs`
//! `start_run`) and never re-reads it, and `TurnUpdate` carried no `tools`. `refresh_extension_tools`
//! (EXT-004) existed but its only turn-adjacent caller, `apply_pending_agent_control`, runs AFTER
//! `handle.finished()` — i.e. after the whole run. So a tool that added a tool mid-run produced an
//! anchor pointing at a tool the model could not call until the NEXT prompt.
//!
//! These tests assert the OBSERVABLE contract, from the provider request the agent actually built:
//! not before the anchor, yes from the anchor onward, actually executed, still permission-gated.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use cyrup_core::{CancelToken, Content, StopReason, Tool, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxProvider, FauxResponseStep,
};
use cyrup_provider::Provider;
use crate::{AgentSession, SessionBuilder, SessionConfig};
use cyrup_tools::{PermissionPolicy, Rule};
use tempfile::TempDir;

// ------------------------------------------------------------------------------ fixtures ----

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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

/// A late-bound weak self-reference, so a tool built BEFORE the session can reach the session that
/// ends up owning it — the same shape `SessionActivityHandle` uses inside the crate.
type SessionSlot = Arc<OnceLock<Weak<AgentSession>>>;

fn empty_schema() -> serde_json::Value {
    serde_json::json!({ "type": "object", "properties": {} })
}

/// The PRODUCER side of Pi's `wrapRegisteredTool`: a tool that widens the active set while it runs
/// and reports the difference as `addedToolNames`.
struct LoaderTool {
    slot: SessionSlot,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for LoaderTool {
    fn name(&self) -> &str {
        "loader"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        let session = self.slot.get().and_then(Weak::upgrade).ok_or_else(|| ToolError::new("no session"))?;
        // Pi's `pi.setActiveTools([...getActiveTools(), "late"])` — a purely ADDITIVE change, which
        // is the only kind `wrapRegisteredTool` records (a removal wipes the cache instead).
        let mut names = session.active_tool_names();
        if !names.iter().any(|n| n == "late") {
            names.push("late".to_string());
        }
        session.set_active_tools_by_name(&names).await;
        // NOTE: the tool does NOT stamp `added_tool_names` itself — upstream no tool ever does.
        // The host's registered-tool wrapper (Pi `wrapRegisteredTool`) derives it from the
        // active-set diff around this very `execute`.
        Ok(ToolResult { content: vec![Content::text("loaded")], ..Default::default() })
    }
}

/// The tool that only exists in the registry until `loader` activates it.
struct LateTool {
    ran: Arc<AtomicBool>,
    params: serde_json::Value,
}

#[async_trait::async_trait]
impl Tool for LateTool {
    fn name(&self) -> &str {
        "late"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    /// A distinctive marker so a test can tell whether the SYSTEM PROMPT the model was sent
    /// describes this tool (DRIFT-033) — only tools with a snippet reach the prompt's tool list
    /// (`prompt/builder.rs:198-209`).
    fn prompt_snippet(&self) -> Option<&str> {
        Some("LATE_TOOL_SNIPPET")
    }
    async fn execute(
        &self,
        _call_id: ToolCallId,
        _params: serde_json::Value,
        _cancel: CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.ran.store(true, Ordering::SeqCst);
        Ok(ToolResult { content: vec![Content::text("late-ran")], ..Default::default() })
    }
}

/// The tool names the agent offered the model, one entry per provider request.
type OfferedTools = Arc<Mutex<Vec<Vec<String>>>>;

/// Turn 1 calls `loader`; turn 2 calls `late`; turn 3 stops. Each step records the tool array the
/// agent handed the provider for THAT request — the only honest evidence of what was callable.
fn faux_three_turns(offered: &OfferedTools) -> Arc<FauxProvider> {
    let mk = |offered: &OfferedTools, reply: AssistantReply| {
        let cap = offered.clone();
        FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
            cap.lock().unwrap().push(ctx.tools.iter().map(|t| t.name.clone()).collect());
            match &reply {
                AssistantReply::Call(name) => faux_assistant_message(
                    vec![faux_tool_call(name.clone(), serde_json::json!({}))],
                    StopReason::ToolUse,
                ),
                AssistantReply::Text(t) => {
                    faux_assistant_message(vec![faux_text(t.clone())], StopReason::Stop)
                }
            }
        })
    };
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        mk(offered, AssistantReply::Call("loader".into())),
        mk(offered, AssistantReply::Call("late".into())),
        mk(offered, AssistantReply::Text("done".into())),
    ]);
    faux
}

#[derive(Clone)]
enum AssistantReply {
    Call(String),
    Text(String),
}

/// Build a bound session whose registry holds `loader` + `late`, with only `loader` active.
async fn session_with_loader(
    fx: &Fixture,
    faux: Arc<FauxProvider>,
    policy: PermissionPolicy,
) -> (Arc<AgentSession>, Arc<AtomicBool>) {
    let slot: SessionSlot = Arc::new(OnceLock::new());
    let late_ran = Arc::new(AtomicBool::new(false));
    let mut cfg = base_config(fx);
    cfg.permission_policy = policy;
    cfg.custom_tools = vec![
        Arc::new(LoaderTool { slot: slot.clone(), params: empty_schema() }) as Arc<dyn Tool>,
        Arc::new(LateTool { ran: late_ran.clone(), params: empty_schema() }) as Arc<dyn Tool>,
    ];
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::downgrade(&session));
    // Start from a known active set: `loader` only. `late` is registered-but-inactive, exactly the
    // state Pi's `dynamic-tools.ts` example leaves a not-yet-loaded tool in.
    session.set_active_tools_by_name(&["loader".to_string()]).await;
    (session, late_ran)
}

// ------------------------------------------------------------------------------- the proof ----

/// THE DRIFT-001 ANCHOR PROOF. A tool that adds a tool mid-run makes it callable from that point in
/// the transcript ONWARD — and it was NOT callable before.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_tool_added_mid_run_is_callable_from_that_turn_onward_and_not_before() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_three_turns(&offered);
    let (session, late_ran) = session_with_loader(&fx, faux, PermissionPolicy::new()).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let turns = offered.lock().unwrap().clone();
    assert!(turns.len() >= 2, "the run drove at least two turns against the provider: {turns:?}");

    // NOT BEFORE — the turn that produced the anchoring tool call could not see `late`.
    assert!(
        !turns[0].iter().any(|t| t == "late"),
        "turn 1 must NOT offer the not-yet-added tool: {:?}",
        turns[0]
    );
    assert!(turns[0].iter().any(|t| t == "loader"), "turn 1 offers the loader: {:?}", turns[0]);

    // FROM THIS POINT ONWARD — the very next turn of the SAME run offers it.
    assert!(
        turns[1].iter().any(|t| t == "late"),
        "turn 2 of the same run must offer the tool added by turn 1's tool result: {:?}",
        turns[1]
    );
    // Additive, not a replacement: Pi's refresh keeps the previously active names.
    assert!(turns[1].iter().any(|t| t == "loader"), "turn 2 kept the loader: {:?}", turns[1]);

    // CALLABLE, not merely advertised: the model's call actually reached the tool.
    assert!(
        late_ran.load(Ordering::SeqCst),
        "the added tool actually EXECUTED when the model called it"
    );

    // …and the loop did not synthesize a `Tool 'late' not found` error result instead.
    let messages = session.agent_messages().await;
    let errors: Vec<String> = messages
        .iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.is_error => Some(
                t.content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(errors.is_empty(), "no tool result errored: {errors:?}");
}

/// The system prompt the agent handed the provider, one entry per request.
type OfferedPrompts = Arc<Mutex<Vec<String>>>;

/// Same three-turn script as [`faux_three_turns`], but capturing the SYSTEM PROMPT of each request
/// instead of the tool array.
fn faux_three_turns_capturing_prompts(prompts: &OfferedPrompts) -> Arc<FauxProvider> {
    let mk = |prompts: &OfferedPrompts, reply: AssistantReply| {
        let cap = prompts.clone();
        FauxResponseStep::factory(move |ctx, _opts, _state, _model| {
            cap.lock().unwrap().push(ctx.system_prompt.clone().unwrap_or_default());
            match &reply {
                AssistantReply::Call(name) => faux_assistant_message(
                    vec![faux_tool_call(name.clone(), serde_json::json!({}))],
                    StopReason::ToolUse,
                ),
                AssistantReply::Text(t) => {
                    faux_assistant_message(vec![faux_text(t.clone())], StopReason::Stop)
                }
            }
        })
    };
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        mk(prompts, AssistantReply::Call("loader".into())),
        mk(prompts, AssistantReply::Call("late".into())),
        mk(prompts, AssistantReply::Text("done".into())),
    ]);
    faux
}

/// DRIFT-033 — the turn-boundary refresh must re-push the SYSTEM PROMPT beside the tool array, so a
/// tool added mid-run is DESCRIBED to the model in the same run it becomes callable in.
///
/// Pi returns both from one object literal: `context: { ...previousContext, systemPrompt:
/// this._systemPromptOverride ?? this._baseSystemPrompt, tools: this.agent.state.tools.slice() }`
/// (agent-session.ts:533-535 @v0.83.0). cyrup returned only `tools`, because it had a single prompt
/// slot and re-pushing it would have clobbered a `before_agent_start` sanitization; with pi's two
/// slots modelled that objection is gone.
///
/// RED before the fix: `RunCtx` copies the system prompt once at `start_run` and only a
/// `TurnUpdate::system_prompt` replaces it (`cyrup-agent/src/agent.rs:692-693`), so turn 2 was sent
/// the run-start prompt and `LATE_TOOL_SNIPPET` never appeared — even though `late` was in the tool
/// array that same request.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drift033_a_mid_run_tool_addition_reaches_the_system_prompt() {
    let fx = fixture();
    let prompts: OfferedPrompts = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_three_turns_capturing_prompts(&prompts);
    let (session, _late_ran) = session_with_loader(&fx, faux, PermissionPolicy::new()).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let seen = prompts.lock().unwrap().clone();
    assert!(seen.len() >= 2, "the run drove at least two turns: {}", seen.len());
    assert!(
        !seen[0].contains("LATE_TOOL_SNIPPET"),
        "turn 1 must not describe the not-yet-added tool"
    );
    assert!(
        seen[1].contains("LATE_TOOL_SNIPPET"),
        "turn 2 of the SAME run must describe the tool turn 1 added"
    );
}

/// DRIFT-033, the other half — a mid-run tool rebuild must not overwrite a `before_agent_start`
/// handler's replacement prompt, and that replacement must not survive its own run.
///
/// pi scopes it with two slots: `_systemPromptOverride` is assigned in `emitBeforeAgentStart`'s
/// caller (agent-session.ts:1247 @v0.83.0), every write of `agent.state.systemPrompt` resolves
/// `override ?? base` (`:534`, `:940`), and `_runAgentPrompt`'s `finally` clears it (`:1069`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drift033_a_start_hook_override_outranks_a_mid_run_rebuild_and_dies_with_its_run() {
    let fx = fixture();
    let prompts: OfferedPrompts = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_three_turns_capturing_prompts(&prompts);
    let (session, _late_ran) = session_with_loader(&fx, faux, PermissionPolicy::new()).await;

    // No handler ran, so nothing may be held in the override slot at any point of a plain run.
    assert_eq!(session.system_prompt_override(), None, "nothing overrides before a run");
    let base_before = session.base_system_prompt();
    assert_eq!(
        session.effective_system_prompt(),
        base_before,
        "with no override the effective prompt IS the base (pi's `override ?? base`)"
    );

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        session.system_prompt_override(),
        None,
        "`_runAgentPrompt`'s finally clears the override; a run may never leave one behind"
    );
    // The mid-run `set_active_tools_by_name` rebuilt the base, and with no override in force the
    // effective prompt follows it — the `late` snippet is now permanent for the session.
    let base_after = session.base_system_prompt();
    assert!(base_after.contains("LATE_TOOL_SNIPPET"), "the rebuild became the new base");
    assert_eq!(session.effective_system_prompt(), base_after);
}

/// The ANCHOR ITSELF rides on the tool result and is persisted to the append-only session JSONL, so
/// a resumed session can recompute the same placement (Pi recomputes `splitDeferredTools` from the
/// transcript on every request — there is no runtime state to restore).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_anchor_survives_the_transcript_and_the_session_file() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_three_turns(&offered);
    let (session, _late_ran) = session_with_loader(&fx, faux, PermissionPolicy::new()).await;

    let session_file = session.session_file().await.expect("the session persists to a file");
    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    // (1) In the live transcript.
    let anchored: Vec<cyrup_agent::ToolResultMessage> = session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if !t.added_tool_names.is_empty() => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(anchored.len(), 1, "exactly one tool result anchors a tool load");
    assert_eq!(anchored[0].tool_name, "loader");
    assert_eq!(anchored[0].added_tool_names, vec!["late".to_string()]);

    // (2) On disk, through `agent_message_to_core` → `SessionManager::append_message`.
    let raw = std::fs::read_to_string(&session_file).expect("read the session file");
    let anchor_line = raw
        .lines()
        .find(|l| l.contains("addedToolNames"))
        .unwrap_or_else(|| panic!("no anchored tool result in the session file:\n{raw}"));
    let v: serde_json::Value = serde_json::from_str(anchor_line).unwrap();
    assert_eq!(v["message"]["role"], "toolResult");
    assert_eq!(v["message"]["toolName"], "loader");
    assert_eq!(v["message"]["addedToolNames"], serde_json::json!(["late"]));

    // (3) And it comes back through the RESUME direction (`core_message_to_agent`).
    let mut resume_cfg = base_config(&fx);
    resume_cfg.target = crate::SessionTarget::Resume(session_file);
    let resumed =
        SessionBuilder::new(Arc::new(FauxProvider::new()) as Arc<dyn Provider>, resume_cfg)
            .build()
            .await
            .unwrap();
    let recovered: Vec<Vec<String>> = resumed
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if !t.added_tool_names.is_empty() => {
                Some(t.added_tool_names)
            }
            _ => None,
        })
        .collect();
    assert_eq!(recovered, vec![vec!["late".to_string()]], "the anchor survived resume");
}

/// PRIVILEGE ESCALATION GUARD. A tool that can add tools must not be able to add an UNGATED tool.
/// The gate keys on tool name + input at call time (`RunCtx::prepare` → `hooks.before_tool_call` →
/// `PolicyHooks` → the permission extension's `ToolCall` decision), never on how or when the tool
/// entered the registry — so a mid-run addition is evaluated exactly like a built-in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mid_run_added_tool_is_still_permission_gated() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_three_turns(&offered);
    let policy = PermissionPolicy::new()
        .with_rule(Rule::when(|tool, _| tool == "late").deny("late is denied by policy"));
    let (session, late_ran) = session_with_loader(&fx, faux, policy).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    // It became visible (the refresh still ran) …
    let turns = offered.lock().unwrap().clone();
    assert!(turns.len() >= 2, "{turns:?}");
    assert!(turns[1].iter().any(|t| t == "late"), "the added tool is offered: {:?}", turns[1]);

    // … and was BLOCKED when called, with the policy's reason as the tool result.
    assert!(
        !late_ran.load(Ordering::SeqCst),
        "the denied tool must NOT have executed, however it entered the tool set"
    );
    let blocked = session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.tool_name == "late" => Some(t),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 1, "the blocked call still produced a tool result");
    assert!(blocked[0].is_error, "a blocked call is an error result");
    let text: String = blocked[0]
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(text.contains("late is denied by policy"), "the policy reason reached the model: {text}");
    // A blocked call never ran, so it can never anchor a tool load (Pi's `createErrorToolResult`
    // carries no `addedToolNames`).
    assert!(blocked[0].added_tool_names.is_empty(), "an error result carries no anchor");
}

// ------------------------------------------------------- the gate the permission system uses ----

/// A native extension that denies one tool by NAME from its `tool_call` handler — a minimal
/// stand-in for `PermissionSystemExtension`, which subscribes to the same `EventKind::ToolCall` and
/// decides in `decide()` (cyrup-permission-system/src/extension/native.rs `init`: "ToolCall is the
/// deciding gate").
struct DenyByName(&'static str);

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for DenyByName {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("deny-by-name")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::ToolCall]);
        Ok(())
    }
    async fn on_event(
        &self,
        ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        match ev {
            cyrup_ext::HostEvent::ToolCall { name, .. } if name == self.0 => {
                cyrup_ext::HookOutcome::Block { reason: Some(format!("{name} denied by extension")), terminate: false }
            }
            _ => cyrup_ext::HookOutcome::Noop,
        }
    }
}

/// The same guard, one layer deeper: through the EXTENSION `tool_call` seam rather than the
/// in-process `PermissionPolicy`. This is the seam `cyrup-permission-system` actually occupies, and
/// `RunCtx::prepare` reaches it via `hooks.before_tool_call` for EVERY call — an unconditional call
/// with no branch on where the tool came from — so a tool that entered the set mid-run is decided on
/// identically to a built-in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mid_run_added_tool_is_gated_by_the_extension_tool_call_seam() {
    let fx = fixture();
    let offered: OfferedTools = Arc::new(Mutex::new(Vec::new()));
    let faux = faux_three_turns(&offered);

    let slot: SessionSlot = Arc::new(OnceLock::new());
    let late_ran = Arc::new(AtomicBool::new(false));
    let mut cfg = base_config(&fx);
    // A native extension needs the extension host live.
    cfg.no_extensions = false;
    cfg.custom_tools = vec![
        Arc::new(LoaderTool { slot: slot.clone(), params: empty_schema() }) as Arc<dyn Tool>,
        Arc::new(LateTool { ran: late_ran.clone(), params: empty_schema() }) as Arc<dyn Tool>,
    ];
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .with_native_extension(Arc::new(DenyByName("late")))
        .build()
        .await
        .unwrap()
        .into_shared();
    let _ = slot.set(Arc::downgrade(&session));
    session.set_active_tools_by_name(&["loader".to_string()]).await;

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;

    let turns = offered.lock().unwrap().clone();
    assert!(turns.len() >= 2, "{turns:?}");
    assert!(turns[1].iter().any(|t| t == "late"), "the added tool is offered: {:?}", turns[1]);
    assert!(!late_ran.load(Ordering::SeqCst), "the extension gate blocked the added tool");

    let blocked = session
        .agent_messages()
        .await
        .into_iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.tool_name == "late" => Some(t),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(blocked.len(), 1);
    assert!(blocked[0].is_error);
    let text: String = blocked[0]
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert!(text.contains("late denied by extension"), "the extension's reason reached the model: {text}");
}
