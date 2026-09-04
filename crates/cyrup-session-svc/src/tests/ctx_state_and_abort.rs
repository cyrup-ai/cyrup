//! EXT-005 END TO END — `ctx.abort()` must stop the run that is IN FLIGHT, and the `ctx-state`
//! accessors must answer from the LIVE session.
//!
//! Pi binds both on the BASE extension context, available from any handler
//! (`extensions/types.ts:329-346`), to real session state (agent-session.ts:2409-2419):
//!
//! ```text
//! isIdle:             () => this.isIdle
//! isProjectTrusted:   () => this.settingsManager.isProjectTrusted()
//! hasPendingMessages: () => this.pendingMessageCount > 0
//! getSystemPrompt:    () => this.systemPrompt
//! abort:              () => { … void this.abort() }          // synchronous, from the handler
//! ```
//!
//! cyrup queued `ControlOp::Abort` onto the command-tier control channel, which drains only at turn
//! BOUNDARIES (`apply_pending_agent_control` runs after `handle.finished()`), so an abort asked for
//! from a mid-run `tool_call` handler fired after the run it was meant to stop had already ended —
//! it aborted nothing. And `LiveHostServices` overrode none of the four state accessors, so every
//! guest in a real session read the trait defaults: idle even mid-run, and untrusted in a project
//! cyrup had just decided IS trusted.
//!
//! Both tests assert an OBSERVABLE effect — a truncated provider conversation, and the values a
//! handler actually read back — never that `HostServices::control(...)` returned `Ok`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::{SessionBuilder, SessionConfig};
use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    ControlOp, ExtError, HookOutcome, HostCtx, HostEvent, HostServices, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use serde_json::json;
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

// =============================================================================================
// `ctx.abort()` from a MID-RUN event handler
// =============================================================================================

/// Subscribes to `tool_call` — the mid-run seam Pi's own docs cite for `ctx.abort()` — and asks the
/// host to abort the moment the model calls a tool.
#[derive(Default)]
struct AbortOnToolCall {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    fired: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for AbortOnToolCall {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("abort-on-tool-call")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::ToolCall]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::ToolCall { .. })
            && let Ok(g) = self.services.lock()
            && let Some(svc) = g.clone()
        {
            self.fired.fetch_add(1, Ordering::SeqCst);
            let _ = svc.control(ControlOp::Abort);
        }
        HookOutcome::Noop
    }
}

/// A provider whose FIRST response calls a tool and whose SECOND would continue the conversation.
/// Reaching the second response means the run was NOT aborted.
fn faux_tool_then_more(turns: &Arc<AtomicUsize>) -> Arc<FauxProvider> {
    let a = turns.clone();
    let first = FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
        a.fetch_add(1, Ordering::SeqCst);
        faux_assistant_message(
            // `read` rather than `ls`: these tests are about ctx abort/state, and `ls` is not in
            // pi's default-active set (`system-prompt.ts:80`), so a call to it never dispatches.
            vec![faux_tool_call("read", json!({ "path": "." }))],
            StopReason::ToolUse,
        )
    });
    let b = turns.clone();
    let rest = FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
        b.fetch_add(1, Ordering::SeqCst);
        faux_assistant_message(
            vec![faux_text("continued after the tool")],
            StopReason::Stop,
        )
    });
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![first, rest.clone(), rest]);
    faux
}

/// THE EXT-005 ABORT PROOF: an extension calls `ctx.abort()` from a `tool_call` handler; the run
/// stops there. Before the fix the op sat on the control queue until the turn boundary — the model
/// got its tool result, the conversation continued, and only then did a no-op abort run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctx_abort_from_an_event_handler_stops_the_in_flight_run() {
    let fx = fixture();
    let turns = Arc::new(AtomicUsize::new(0));
    let ext = Arc::new(AbortOnToolCall::default());
    let fired = ext.fired.clone();

    let session = SessionBuilder::new(
        faux_tool_then_more(&turns) as Arc<dyn Provider>,
        base_config(&fx),
    )
    .with_native_extension(ext as Arc<dyn NativeExtension>)
    .build()
    .await
    .unwrap()
    .into_shared();

    let _ = session.prompt("use a tool please").await.unwrap();
    session.wait_for_idle().await;

    assert_eq!(
        fired.load(Ordering::SeqCst),
        1,
        "the tool_call handler ran exactly once"
    );
    assert!(
        turns.load(Ordering::SeqCst) >= 1,
        "the model was asked at least once"
    );

    let transcript = session
        .messages()
        .await
        .iter()
        .map(|m| serde_json::to_string(m).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n");

    // (1) The TOOL never ran: the abort landed between the `tool_call` hook and execution, so the
    //     agent recorded the cancellation instead of a directory listing (agent.rs `Prep::Immediate
    //     (self.immediate_error(call, "Operation aborted"))`). Before the fix the op was still on
    //     the control queue here and `ls` executed normally.
    assert!(
        transcript.contains("Operation aborted"),
        "the tool call was cancelled rather than executed: {transcript}"
    );
    // (2) The CONVERSATION never continued: the second scripted assistant turn — the one that only
    //     exists if the loop kept going past the tool batch — is absent, and the run's terminal
    //     message carries the aborted stop reason.
    assert!(
        !transcript.contains("continued after the tool"),
        "the aborted run never reached the next assistant turn: {transcript}"
    );
    assert!(
        transcript.contains("\"stopReason\":\"aborted\""),
        "the run terminated through the ABORT path, not a clean stop: {transcript}"
    );
    assert!(session.is_idle(), "the session settled after the abort");
}

// =============================================================================================
// the four `ctx-state` accessors
// =============================================================================================

/// What a handler read back from the base context, captured at a MID-RUN dispatch.
type Observed = Arc<Mutex<Option<(bool, bool, bool, String)>>>;

#[derive(Default)]
struct StateProbe {
    services: Arc<Mutex<Option<Arc<dyn HostServices>>>>,
    observed: Observed,
}

#[async_trait::async_trait]
impl NativeExtension for StateProbe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ctx-state-probe")
    }

    fn set_host_services(&self, services: Arc<dyn HostServices>) {
        if let Ok(mut g) = self.services.lock() {
            *g = Some(services);
        }
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::ToolCall]);
        Ok(())
    }

    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if matches!(ev, HostEvent::ToolCall { .. })
            && let Ok(g) = self.services.lock()
            && let Some(svc) = g.clone()
            && let Ok(mut slot) = self.observed.lock()
        {
            *slot = Some((
                svc.is_idle(),
                svc.has_pending_messages(),
                svc.is_project_trusted(),
                svc.system_prompt().unwrap_or_default(),
            ));
        }
        HookOutcome::Noop
    }
}

/// THE EXT-005 STATE PROOF: a handler running INSIDE a live run reads the session's real state.
/// Every value asserted here is the OPPOSITE of the `HostServices` trait default it used to get
/// (`is_idle` defaulted to `true`, `is_project_trusted` to `false`, `system_prompt` to `None`), so
/// the assertions cannot be satisfied by the pre-fix constants.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_ctx_state_accessors_answer_from_the_live_session() {
    let fx = fixture();
    let turns = Arc::new(AtomicUsize::new(0));
    let ext = Arc::new(StateProbe::default());
    let observed = ext.observed.clone();

    let session = SessionBuilder::new(
        faux_tool_then_more(&turns) as Arc<dyn Provider>,
        base_config(&fx),
    )
    .with_native_extension(ext as Arc<dyn NativeExtension>)
    .build()
    .await
    .unwrap()
    .into_shared();

    // A follow-up queued BEFORE the run makes `hasPendingMessages` observably true mid-run
    // (Pi `pendingMessageCount > 0`).
    let _ = session
        .follow_up("queued while the run is going")
        .await
        .unwrap();
    let _ = session.prompt("use a tool please").await.unwrap();
    session.wait_for_idle().await;

    let (is_idle, has_pending, trusted, prompt) = observed
        .lock()
        .unwrap()
        .clone()
        .expect("the tool_call handler observed the ctx state");

    assert!(
        !is_idle,
        "a handler dispatched mid-run must observe the session as NOT idle"
    );
    assert!(
        has_pending,
        "the queued follow-up is visible as a pending message"
    );
    assert!(
        trusted,
        "this session was built with trust_override = Some(true)"
    );
    assert!(
        prompt.contains("coding assistant"),
        "the handler read the session's REAL system prompt, not an empty default: {prompt:?}"
    );

    // …and the same reads are correct once the run has settled: `is_idle` flips back.
    let svc = session.services().host_services.clone();
    assert!(
        HostServices::is_idle(&*svc),
        "the accessor is LIVE, not a mirror — it reports idle once the run settles"
    );
}
