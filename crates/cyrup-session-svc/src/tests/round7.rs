//! Round-7 L5-consolidation parity tests (gap-09). Each closes a genuinely-open closeable item the
//! round-3 re-derivation surfaced, wiring the just-landed sibling-crate seams through the facade:
//!
//!   * #13c — the `input` extension event delivers `source` + `streamingBehavior` to handlers
//!     (Pi `emitInput`, runner.ts:1095-1114 from agent-session.ts:1019-1024). cyrup's
//!     `HostEvent::Input` now carries both; `emit_input_event` forwards the mapped `ui.source` and
//!     the in-flight streaming behavior, so a handler can branch on interactive-vs-queued.
//!   * #13 (native residue) — slash `_tryExecuteExtensionCommand` *exec* dispatch for NATIVE
//!     extensions (Pi agent-session.ts:1004-1013,1148-1172): a `/<cmd>` matching a registered
//!     native command runs its command-tier handler and short-circuits (no prompt sent).
//!   * §08 ledger row — `LiveHostServices` injected as the live capability backend: a loaded
//!     extension's `control` capability (Pi `createCommandContext`, agent-session.ts:1158) reaches
//!     a REAL session effect via the command-tier control channel.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{Content, ExtensionId, Message, StopReason};
use cyrup_ext::{
    CommandDescriptor, ControlOp, EventKind, HostCtx, HostEvent, HookOutcome, HostServices, InitApi,
    InputEventSource, InputStreamingBehavior, NativeExtension,
};
use cyrup_ext::ExtError;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use crate::{InputSource, SessionBuilder, SessionConfig, UserInput};
use tempfile::TempDir;

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
    cfg
}

fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

fn user_texts(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .filter_map(|m| match m {
            Message::User { content, .. } => Some(
                content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.to_string()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

// =============================================================== #13c input event source/behavior ==

type Probe = Arc<Mutex<Vec<(InputEventSource, Option<InputStreamingBehavior>)>>>;

/// A native `input` handler that records the `source` + `streamingBehavior` it was delivered, then
/// passes through (no transform), proving the payload reaches the handler (#13c).
struct InputProbe(Probe);
#[async_trait::async_trait]
impl NativeExtension for InputProbe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("input-probe")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::Input { source, streaming_behavior, .. } = ev {
            self.0.lock().unwrap().push((*source, *streaming_behavior));
        }
        HookOutcome::Noop
    }
}

/// gap #13c: an idle `rpc` submission is delivered to the `input` handler as `source=rpc`,
/// `streamingBehavior=None`; a default submission maps to `source=interactive`.
#[tokio::test]
async fn input_event_delivers_source_and_streaming_behavior() {
    let fx = fixture();
    let probe: Probe = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(InputProbe(probe.clone())))
        .build()
        .await
        .unwrap();

    // An RPC-sourced submission while idle.
    let _ = session.prompt(UserInput::text("hi", InputSource::Rpc)).await.unwrap();
    session.wait_for_idle().await;

    let seen = probe.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the input handler fired exactly once");
    assert_eq!(
        seen[0],
        (InputEventSource::Rpc, None),
        "rpc source forwarded; streamingBehavior is None while idle (Pi `this.isStreaming ? ... : undefined`)"
    );

    // A default (Sdk/Cli/Tui/Stdin) submission collapses onto Pi's `interactive`.
    let _ = session.prompt("again").await.unwrap();
    session.wait_for_idle().await;
    let seen = probe.lock().unwrap().clone();
    assert_eq!(seen[1].0, InputEventSource::Interactive, "non-rpc source -> interactive");
}

// =============================================================== #13 native slash command exec ====

/// A native extension registering a `/greet` command whose handler records its args + returns text.
struct GreetCommand(Arc<Mutex<Vec<String>>>);
#[async_trait::async_trait]
impl NativeExtension for GreetCommand {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("greet-command")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "greet",
            CommandDescriptor { description: "greet someone".into(), completions: vec![] },
        );
        Ok(())
    }
    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }
    async fn execute_command(
        &self,
        _name: &str,
        args: &str,
        ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        // A command runs command-tier (session mutation allowed): the deadlock guard must pass.
        ctx.require_command_tier()?;
        self.0.lock().unwrap().push(args.to_string());
        Ok(Some(format!("hello {args}")))
    }
}

/// gap #13 (native residue): a `/<cmd>` matching a registered native command runs its handler and
/// short-circuits — no user message is sent/persisted (Pi `_tryExecuteExtensionCommand` returns
/// `true` -> the prompt is consumed).
#[tokio::test]
async fn native_slash_command_executes_and_short_circuits_the_prompt() {
    let fx = fixture();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(GreetCommand(calls.clone())))
        .build()
        .await
        .unwrap();

    let _ = session.prompt("/greet world").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        calls.lock().unwrap().clone(),
        vec!["world".to_string()],
        "the native command handler ran with the parsed args"
    );
    assert!(
        user_texts(&session.messages().await).iter().all(|t| !t.contains("/greet")),
        "the slash command was consumed — no user message was sent to the model"
    );

    // A `/unknown` command (no native owner) is NOT consumed: it falls through to a normal prompt.
    let _ = session.prompt("/unknown stuff").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("/unknown stuff")),
        "an unmatched slash command falls through to normal prompt handling"
    );
}

// ==================================================== §08 LiveHostServices control -> session ====

/// §08 ledger row: `LiveHostServices` (the live capability backend a wasm host load injects) routes
/// a guest `control` capability call to a REAL session effect. Here we exercise the SYNC `control()`
/// path a guest would hit (the guest is wasm-suspended), then drain + apply at the command-tier-safe
/// point — proving the backend reaches the running session without needing the gated guest E2E.
#[tokio::test]
async fn live_host_services_control_reaches_a_real_session_effect() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_with_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .unwrap();

    // Simulate the guest's `session.sendUserMessage("from-extension")` capability call.
    session
        .services()
        .host_services
        .control(ControlOp::SendUserMessage {
            content: "from-extension".into(),
            opts: serde_json::Value::Null,
        })
        .expect("control routes to the wired channel (LiveHostServices::wire_control_channel)");

    // The runtime drains + applies queued control ops at a command-tier-safe point.
    session.apply_pending_control().await;
    session.wait_for_idle().await;

    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("from-extension")),
        "the extension-driven control op produced a real session effect (the user message ran)"
    );

    // SEAM-003 rewrite of this assertion. It used to read:
    //
    //     let deferred = session.apply_pending_control().await;
    //     assert_eq!(deferred.len(), 1, "Reload is runtime-tier — handed back to the runtime");
    //
    // which encoded the defect: `apply_pending_control` returned the runtime-tier ops "for the
    // runtime to act on", and its only production caller did `let _deferred = …`. Nothing ever
    // acted. `apply_pending_control` is now a SINK — a runtime-tier op is routed to the installed
    // `RuntimeActions` (see `tests/control_ops.rs` for the positive, end-to-end proof), and on a
    // BARE session like this one — built straight from `SessionBuilder`, never installed into an
    // `AgentSessionRuntime` — there is no host to route to, so the op is REPORTED (a `tracing::warn`
    // naming `SessionServiceError::NoRuntimeHost("reload")`) rather than silently dropped. What is
    // asserted here is that the drain consumes it and leaves the session healthy.
    session
        .services()
        .host_services
        .control(ControlOp::Reload)
        .expect("control routes to the channel");
    session.apply_pending_control().await;
    assert!(
        session.services().host_services.take_pending_control().is_empty(),
        "the drain CONSUMED the runtime-tier op (it is not left queued for a caller that never comes)"
    );
    // The session is unharmed by a runtime-tier op it cannot service.
    let _ = session.prompt("still alive").await.unwrap();
    session.wait_for_idle().await;
    assert!(
        user_texts(&session.messages().await).iter().any(|t| t.contains("still alive")),
        "a runtime-tier op with no runtime host installed degrades cleanly"
    );
}
