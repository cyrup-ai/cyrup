//! The extension `input` event — the seam an extension uses to see, rewrite, or answer a
//! submission BEFORE it becomes a turn (Pi `InputEvent` / `emitInputEvent`,
//! `extensions/types.ts` + `agent-session.ts`).
//!
//! Three arms, one per test below, each closing a gap-09 item:
//!
//! * `#13` HANDLED — a handler that answers the submission short-circuits the prompt entirely: no
//!   user message is recorded and the provider is never called.
//! * `#13b` TRANSFORM — a handler returning `EventPatch::Input` (Pi `action:"transform"`) rewrites
//!   the submission text, and the REWRITTEN text is what lands in the transcript.
//! * `#13c` SOURCE + BEHAVIOR — the event carries the submission's `InputEventSource` and
//!   `InputStreamingBehavior` so a handler can tell an SDK call from a typed line and route on it.

use std::sync::{Arc, Mutex};

use cyrup_core::{Content, ExtensionId, Message, StopReason};
use cyrup_ext::{
    EventKind, EventPatch, ExtError, HandledValue, HostCtx, HostEvent, HookOutcome, InitApi,
    InputEventSource, InputStreamingBehavior, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, fixture};
use crate::{InputSource, PromptAccepted, PromptOptions, SessionBuilder, UserInput};

/// A faux provider scripted with a single `ok` answer — enough for a run that must NOT happen.
fn faux_with_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// The text of the first user message in `messages`, or `None` when the run recorded none.
fn first_user_text(messages: &[Message]) -> Option<String> {
    messages.iter().find_map(|m| match m {
        Message::User { content, .. } => Some(
            content
                .iter()
                .filter_map(|c| match c {
                    Content::Text { text, .. } => Some(text.clone()),
                    _ => None,
                })
                .collect::<String>(),
        ),
        _ => None,
    })
}

// ============================================================== #13 the HANDLED arm ====

/// A native extension that fully services every `input` event (Pi `action:"handled"`).
struct InputHandler;
#[async_trait::async_trait]
impl NativeExtension for InputHandler {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("input-handler")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { .. } => HookOutcome::Handled(HandledValue(serde_json::json!({
                "action": "handled"
            }))),
            _ => HookOutcome::Noop,
        }
    }
}

/// gap #13: an `input` handler that returns `handled` short-circuits the prompt — no run starts and
/// nothing is persisted (Pi agent-session.ts:1018-1028).
#[tokio::test]
async fn input_event_handled_short_circuits_prompt() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx))
        .with_native_extension(Arc::new(InputHandler))
        .build()
        .await
        .unwrap();

    let accepted = session.prompt_with("anything", PromptOptions::default()).await.unwrap();
    assert_eq!(accepted, PromptAccepted::Handled, "the input handler serviced the submission");
    assert!(!session.is_streaming().await, "no run was started");
    assert!(session.messages().await.is_empty(), "nothing was persisted");
}

// ======================================================== #13b the TRANSFORM arm ====

/// A native `input` handler that rewrites the submission text to upper-case via the
/// `EventPatch::Input` mutate arm (Pi `action:"transform"`).
struct UppercaseInput;
#[async_trait::async_trait]
impl NativeExtension for UppercaseInput {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("uppercase-input")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::Input]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        match ev {
            HostEvent::Input { text, .. } => {
                HookOutcome::Mutate(EventPatch::Input { text: text.to_uppercase(), images: None })
            }
            _ => HookOutcome::Noop,
        }
    }
}

/// gap-09 #13b — the `input` extension event **transform** arm (Pi agent-session.ts:1029-1032 /
/// runner.ts:1116-1119): a handler rewrites the submission text/images via `EventPatch::Input`, and
/// the rewritten content is what is persisted + actually runs.
#[tokio::test]
async fn input_event_transform_rewrites_submission_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx))
        .with_native_extension(Arc::new(UppercaseInput))
        .build()
        .await
        .unwrap();

    let _ = session.prompt("hello world").await.unwrap();
    session.wait_for_idle().await;

    let messages = session.messages().await;
    assert_eq!(
        first_user_text(&messages).as_deref(),
        Some("HELLO WORLD"),
        "the transform handler's rewritten text is what runs + persists"
    );
}

// ============================================== #13c the SOURCE + BEHAVIOR payload ====

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

/// gap-09 #13c — the `input` extension event delivers `source` + `streamingBehavior` to handlers
/// (Pi `emitInput`, runner.ts:1095-1114 from agent-session.ts:1019-1024). cyrup's `HostEvent::Input`
/// carries both, and `emit_input_event` forwards the mapped `ui.source` plus the in-flight streaming
/// behavior, so a handler can branch on interactive-vs-queued: an idle `rpc` submission arrives as
/// `source=rpc`, `streamingBehavior=None`; a default submission maps to `source=interactive`.
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
