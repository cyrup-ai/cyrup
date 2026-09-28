//! EXT-083 — a guest `sendUserMessage`'s options bag must REACH the session.
//!
//! pi's `AgentSession.sendUserMessage` is a thin wrapper over `prompt`
//! (`core/agent-session.ts:2006-2035` @v0.87.1):
//!
//! ```ts
//! await this.prompt(text, {
//!     expandPromptTemplates: options?.expandPromptTemplates ?? false,
//!     streamingBehavior: options?.deliverAs,
//!     images,
//!     source: "extension",
//! });
//! ```
//!
//! and `expandPromptTemplates` is documented opt-IN at `:2004`: "Whether to dispatch extension
//! commands and expand skill commands and prompt templates. Default: false."
//!
//! cyrup carried the bag from the guest (`cyrup-ext-sdk`'s `SendUserMessageOptions` →
//! `cyrup-ext/src/host/live.rs::send_user_message` → `ControlOp::SendUserMessage { content, opts }`)
//! and then DROPPED it: `apply_pending_control` destructured `{ content, .. }` and called
//! `send_user_message(content, None)`, whose `String` → `UserInput` conversion sets
//! `expand_templates: true`. So a relayed `"/deploy"` ran the registered extension command instead of
//! being sent as text, and a `deliverAs: "followUp"` became a steer.
//!
//! Every assertion here is on an OBSERVABLE consequence of the bag — whether the command handler
//! ran, what text reached the transcript, which queue the message landed in — never on the decode.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{
    Content, ExtensionId, StopReason, TerminateHint, Tool, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_ext::{
    CommandDescriptor, ControlOp, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi,
    NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text, faux_tool_call};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Notify;

use crate::{
    InputSource, PromptOptions, SessionBuilder, SessionConfig, StreamingBehavior, UserInput,
};

// ------------------------------------------------------------------------------------ fixtures ----

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

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    cfg
}

// ------------------------------------------------------------------------------- the relay guest ----

/// A native built-in standing in for a WASM guest, over the SAME `HostServices::control` seam
/// `cyrup-ext/src/host/live.rs::send_user_message` reaches.
///
/// `/relay` queues one [`ControlOp::SendUserMessage`] carrying `content` and the `opts` bag under
/// test — exactly the op a guest's `ctx.send_user_message(content, SendUserMessageOptions{..})`
/// produces. `/deploy` is the relay's TARGET: a registered extension command whose handler sets a
/// flag, so "did the host dispatch the relayed text as a command?" is a fact and not an inference.
struct RelayExt {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    /// `(content, opts)` for the next `/relay`.
    payload: Arc<Mutex<(String, Value)>>,
    /// Set by the `/deploy` handler.
    deployed: Arc<AtomicBool>,
    /// A tool that parks, so a run can be held streaming (used by the `deliverAs` proof).
    tool: Arc<BlockTool>,
}

impl RelayExt {
    fn new(gate: Arc<Notify>) -> Self {
        Self {
            services: Arc::new(Mutex::new(None)),
            payload: Arc::new(Mutex::new((String::new(), Value::Null))),
            deployed: Arc::new(AtomicBool::new(false)),
            tool: Arc::new(BlockTool {
                gate,
                params: json!({"type": "object", "properties": {}}),
            }),
        }
    }
    fn arm(&self, content: &str, opts: Value) {
        *self.payload.lock().unwrap() = (content.to_string(), opts);
    }
}

#[async_trait::async_trait]
impl NativeExtension for RelayExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("relay-ext")
    }
    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_tool(self.tool.clone());
        for name in ["relay", "deploy"] {
            api.register_command(
                name,
                CommandDescriptor {
                    description: format!("EXT-083 {name}"),
                    completions: Vec::new(),
                },
            );
        }
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        if name == "deploy" {
            self.deployed.store(true, Ordering::SeqCst);
            return Ok(Some(String::new()));
        }
        let svc = self
            .services
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or_else(|| ExtError::Component("no host services".into()))?;
        let (content, opts) = self.payload.lock().unwrap().clone();
        svc.control(ControlOp::SendUserMessage { content, opts })
            .map_err(ExtError::Component)?;
        Ok(Some(String::new()))
    }
}

/// A tool whose execution parks on a gate, holding the run in a streaming state.
struct BlockTool {
    gate: Arc<Notify>,
    params: Value,
}
#[async_trait::async_trait]
impl Tool for BlockTool {
    fn name(&self) -> &str {
        "block"
    }
    fn parameters(&self) -> &Value {
        &self.params
    }
    fn description(&self) -> &str {
        "parks until released"
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.gate.notified().await;
        Ok(ToolResult {
            content: vec![Content::text("released")],
            details: None,
            terminate: TerminateHint::Unspecified,
            ..Default::default()
        })
    }
}

/// Every user-role text in the session transcript, in order.
async fn user_texts(session: &crate::AgentSession) -> Vec<String> {
    session
        .agent_messages()
        .await
        .iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(""),
            ),
            _ => None,
        })
        .collect()
}

/// Drive `/relay` with `content` + `opts` on an idle session and return the guest + session.
async fn relay_idle(content: &str, opts: Value) -> (Arc<RelayExt>, Arc<crate::AgentSession>) {
    let fx = fixture();
    let ext = Arc::new(RelayExt::new(Arc::new(Notify::new())));
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(ext.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();
    ext.arm(content, opts);
    // `/relay` is itself dispatched through `prepare`'s step 0; the op it queues is applied by the
    // drain at the end of the command route (`session/commands.rs`).
    let _ = session.prompt("/relay").await.expect("relay dispatched");
    session.wait_for_idle().await;
    (ext, session)
}

// =================================================================================== the proofs ====

/// With NO options, a relayed `"/deploy"` is SENT AS TEXT — the registered `/deploy` command is not
/// dispatched.
///
/// pi: `expandPromptTemplates: options?.expandPromptTemplates ?? false` (`:2031`), and `prompt` only
/// tries `_tryExecuteExtensionCommand` when `expandPromptTemplates && text.startsWith("/")`
/// (`:1618`). This is the row's headline: an extension that relays text — a chat bridge, a macro, a
/// subagent summary — must not have a leading `/` turn its message into a command.
#[tokio::test]
async fn a_relayed_slash_command_is_not_dispatched_without_expand_prompt_templates() {
    let (ext, session) = relay_idle("/deploy", json!({})).await;

    assert!(
        !ext.deployed.load(Ordering::SeqCst),
        "EXT-083: `/deploy` must NOT be dispatched — pi's `expandPromptTemplates` defaults to false \
         (`agent-session.ts:2031`), so the relayed text is a user message, not a command"
    );
    assert!(
        user_texts(&session).await.iter().any(|t| t == "/deploy"),
        "…and the literal text must reach the transcript as a user message: {:?}",
        user_texts(&session).await
    );
}

/// A `null` bag — what the SDK sends when a guest passes no options at all — behaves the same as `{}`.
///
/// `cyrup-ext/src/host/live.rs::send_user_message` stores `serde_json::from_str(&opts_json)
/// .unwrap_or(Value::Null)`, so `Value::Null` is a live wire shape and must decode to pi's
/// all-absent bag rather than to "expand everything".
#[tokio::test]
async fn a_null_options_bag_takes_pis_absent_key_defaults() {
    let (ext, session) = relay_idle("/deploy", Value::Null).await;

    assert!(
        !ext.deployed.load(Ordering::SeqCst),
        "a `null` bag is upstream's absent `options`, not an opt-in"
    );
    assert!(
        user_texts(&session).await.iter().any(|t| t == "/deploy"),
        "the literal text is what was sent: {:?}",
        user_texts(&session).await
    );
}

/// …and the opt-IN works: `{"expandPromptTemplates": true}` DOES dispatch the command.
///
/// Presence-before-absence for the two tests above — without this control they would pass on a host
/// that never dispatched anything.
#[tokio::test]
async fn expand_prompt_templates_true_dispatches_the_relayed_command() {
    let (ext, session) = relay_idle("/deploy", json!({"expandPromptTemplates": true})).await;

    assert!(
        ext.deployed.load(Ordering::SeqCst),
        "with `expandPromptTemplates: true` the relayed `/deploy` IS dispatched (pi `:1618`)"
    );
    assert!(
        !user_texts(&session).await.iter().any(|t| t == "/deploy"),
        "a dispatched command sends no prompt (pi `return`s after `handled`, `:1621-1624`): {:?}",
        user_texts(&session).await
    );
}

/// `deliverAs: "followUp"` lands in the FOLLOW-UP queue while the agent streams, not the steering
/// queue.
///
/// pi threads it on as `streamingBehavior` (`:2032`) and `prompt` picks the queue from it
/// (`:1660-1663`). cyrup dropped the bag and `send_user_message`'s `_ => steer` default made every
/// relayed message a steer — it interrupted the turn in flight instead of waiting for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deliver_as_follow_up_queues_behind_the_running_turn_instead_of_steering_it() {
    let fx = fixture();
    let gate = Arc::new(Notify::new());
    let ext = Arc::new(RelayExt::new(gate.clone()));
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("block", json!({}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("after tool")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("after follow-up")], StopReason::Stop),
    ]);
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(ext.clone() as Arc<dyn NativeExtension>)
        .build()
        .await
        .expect("build")
        .into_shared();

    let _stream = session
        .prompt(UserInput::text("kick off", InputSource::Sdk))
        .await
        .expect("prompt");

    // Park on the tool so the relay's op is drained with a LIVE run.
    let mut streaming = false;
    for _ in 0..400 {
        if session.is_streaming().await {
            streaming = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(streaming, "the block tool must hold the agent streaming");

    ext.arm("relayed while busy", json!({"deliverAs": "followUp"}));
    // `prepare`'s step 0 runs the extension command even while streaming (pi `:1618`, ahead of the
    // `isStreaming` queue branch at `:1653`), so `/relay` executes here and its op is drained with
    // `is_run_active()` true. A `streaming_behavior` is supplied for THIS submission because a
    // `/relay` that were NOT a command would have to be queued; the relayed message's own behaviour
    // comes from the op's bag alone.
    let _ = session
        .prompt_with(
            UserInput::text("/relay", InputSource::Tui),
            PromptOptions {
                streaming_behavior: Some(StreamingBehavior::Steer),
            },
        )
        .await
        .expect("relay dispatched mid-run");

    assert_eq!(
        session.follow_up_messages(),
        vec!["relayed while busy".to_string()],
        "EXT-083: `deliverAs: \"followUp\"` must reach the FOLLOW-UP queue (pi \
         `streamingBehavior: options?.deliverAs`, `agent-session.ts:2032`)"
    );
    assert!(
        session.steering_messages().is_empty(),
        "…and must not steer the turn in flight: {:?}",
        session.steering_messages()
    );

    gate.notify_one();
    session.wait_for_idle().await;
}
