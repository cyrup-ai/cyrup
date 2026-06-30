//! Scripted stream-fn provider (Pi `createFauxStreamFn` + `FauxStreamFnState`, test-harness.ts:266-314).
//!
//! Cycles through a list of declarative [`FauxResponse`]s in order, wrapping around when more calls
//! are made than responses (Pi `index = callCount % responses.length`). Records every request
//! [`Context`] and the call count for assertions, applies per-response `delayMs`, and streams via
//! the shared chunked engine [`cyrup_provider::faux::faux_event_stream`] (so chunking/pacing/abort
//! match the faux provider exactly). Honors per-response model identity overrides — unlike the faux
//! provider core, the scripted stream fn does NOT re-stamp the response identity (Pi parity).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{EventStream, ProviderId};
use cyrup_provider::faux::{faux_event_stream, usage_estimate, ChunkConfig};
use cyrup_provider::{Context, Model, Provider, StreamEvent, StreamOptions};
use futures::StreamExt;

use crate::response::{build_assistant_message, build_usage, faux_model, FauxResponse};

/// Inspectable state of a scripted stream fn (Pi `FauxStreamFnState`, test-harness.ts:266-271).
#[derive(Clone, Debug, Default)]
pub struct FauxStreamFnState {
    /// Number of times the stream fn has been called.
    pub call_count: usize,
    /// The request context passed to each call, in order.
    pub contexts: Vec<Context>,
}

/// A [`Provider`] that replays scripted [`FauxResponse`]s, advertising one **or more** models.
///
/// Two consumption flavours, matching Pi's two faux harnesses:
/// - **cycling** (default; Pi `createFauxStreamFn`, test-harness.ts:266-314): index =
///   `callCount % responses.len()`, wrapping around forever.
/// - **queue** (Pi `createFauxCore`/`registerFauxProvider`, faux.ts:444-506, the flavour
///   `suite/harness.ts` drives): responses are consumed from the front; once exhausted, every
///   further call streams the `"No more faux responses queued"` error terminal (faux.ts:451-461).
pub struct ScriptedProvider {
    id: ProviderId,
    models: Vec<Model>,
    responses: Mutex<Vec<FauxResponse>>,
    /// `true` ⇒ queue-consuming semantics; `false` ⇒ cycling (Pi's two flavours).
    queue_mode: bool,
    state: Arc<Mutex<FauxStreamFnState>>,
    /// Per-provider prompt cache backing the queue-exhaustion `withUsageEstimate` (Pi's per-provider
    /// `promptCache`, faux.ts:213-251,451-461). Shared across exhaustion calls so repeated drained
    /// turns with the same `sessionId` accumulate cacheRead/cacheWrite exactly as Pi's Map does.
    prompt_cache: Mutex<HashMap<String, String>>,
}

impl ScriptedProvider {
    /// Build a cycling scripted provider from a response sequence (Pi requires ≥1 response,
    /// test-harness.ts:285; cyrup defaults an empty list to a single `"ok"` so construction never
    /// fails).
    pub fn new(responses: Vec<FauxResponse>) -> Self {
        Self::with_model(responses, faux_model())
    }

    /// Build a cycling scripted provider that advertises a caller-supplied `model` (Pi
    /// `HarnessOptions.model` / `contextWindow` override, test-harness.ts:323-324,370). The session
    /// builder resolves the model from `provider.models()`, so overriding it here (e.g. a smaller
    /// `context_window`, or an entirely different api/provider/modalities) makes compaction-threshold
    /// and dynamic-provider scenarios reproducible through the harness.
    pub fn with_model(responses: Vec<FauxResponse>, model: Model) -> Self {
        Self::build(responses, vec![model], false)
    }

    /// Build a cycling scripted provider advertising **multiple** models (Pi `models?:
    /// FauxModelDefinition[]` + `harness.models`/`getModel(id)`, suite/harness.ts:64,82-84,201-202).
    /// The first model is the default ([`Provider::get_model`] looks the rest up by id). An empty
    /// list falls back to the single default faux model.
    pub fn with_models(responses: Vec<FauxResponse>, models: Vec<Model>) -> Self {
        Self::build(responses, models, false)
    }

    /// Build a **queue-consuming** scripted provider advertising the given models (Pi
    /// `registerFauxProvider`, the `suite/harness.ts` flavour). Responses are consumed in order;
    /// once exhausted, further calls stream the `"No more faux responses queued"` error terminal
    /// (Pi faux.ts:451-461). Unlike the cycling flavour, an empty list is left empty (Pi's suite
    /// harness starts with `setResponses([])` and `appendResponses` later, suite/harness.ts:105).
    pub fn queued(responses: Vec<FauxResponse>, models: Vec<Model>) -> Self {
        Self::build(responses, models, true)
    }

    fn build(responses: Vec<FauxResponse>, models: Vec<Model>, queue_mode: bool) -> Self {
        let models = if models.is_empty() { vec![faux_model()] } else { models };
        let id = models
            .first()
            .map(|m| m.provider.clone())
            .unwrap_or_else(|| ProviderId::from("faux"));
        // Cycling needs ≥1 response (an empty list defaults to `"ok"`); the queue flavour may start
        // empty and be fed via `append_responses` (Pi parity).
        let responses = if !queue_mode && responses.is_empty() {
            vec![FauxResponse::text("ok")]
        } else {
            responses
        };
        Self {
            id,
            models,
            responses: Mutex::new(responses),
            queue_mode,
            state: Arc::new(Mutex::new(FauxStreamFnState::default())),
            prompt_cache: Mutex::new(HashMap::new()),
        }
    }

    /// A shared handle to the inspectable call state (call count + captured contexts).
    pub fn state(&self) -> Arc<Mutex<FauxStreamFnState>> {
        self.state.clone()
    }

    /// Replace the response sequence (Pi `setResponses`). Cycling continues from the current call
    /// count; the queue flavour replaces the pending queue (and may be cleared to empty).
    pub fn set_responses(&self, responses: Vec<FauxResponse>) {
        if let Ok(mut r) = self.responses.lock() {
            *r = if !self.queue_mode && responses.is_empty() {
                vec![FauxResponse::text("ok")]
            } else {
                responses
            };
        }
    }

    /// Append responses to the pending sequence (Pi `appendResponses`, faux.ts:501-503;
    /// suite/harness.ts:204). Most meaningful in queue mode, where it extends the consumable queue.
    pub fn append_responses(&self, responses: Vec<FauxResponse>) {
        if let Ok(mut r) = self.responses.lock() {
            r.extend(responses);
        }
    }

    /// The number of pending (not-yet-consumed) responses (Pi `getPendingResponseCount`,
    /// faux.ts:504-506; suite/harness.ts:205). In cycling mode this is the configured list length.
    pub fn pending_count(&self) -> usize {
        self.responses.lock().map(|r| r.len()).unwrap_or(0)
    }

    fn lock_state(&self) -> std::sync::MutexGuard<'_, FauxStreamFnState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Provider for ScriptedProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    fn stream(
        &self,
        _model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let (resp, exhausted) = {
            let mut responses = self.responses.lock().unwrap_or_else(|e| e.into_inner());
            let mut state = self.lock_state();
            state.contexts.push(context.clone());
            let resolved = if self.queue_mode {
                // Queue flavour: consume from the front; exhaustion ⇒ Pi's error terminal.
                if responses.is_empty() {
                    (FauxResponse::error("No more faux responses queued"), true)
                } else {
                    (responses.remove(0), false)
                }
            } else {
                // Cycling flavour: wrap around (Pi `index = callCount % responses.length`).
                let index =
                    if responses.is_empty() { 0 } else { state.call_count % responses.len() };
                (responses.get(index).cloned().unwrap_or_else(|| FauxResponse::text("ok")), false)
            };
            state.call_count += 1;
            resolved
        };

        let mut message = build_assistant_message(&resp);
        if exhausted {
            // Queue-exhaustion terminal (Pi faux.ts:451-461): Pi runs the `createErrorMessage`
            // result (content `[]`) through `withUsageEstimate(message, context, streamOptions,
            // promptCache)`, NOT the fixed `buildUsage` defaults. So the stamped usage is
            // output:0 (empty content ⇒ `assistantContentToText([]) === ""`), input = the serialized
            // -context estimate, and cacheRead/cacheWrite per the session prompt-cache (faux.ts:213-251).
            message.usage =
                usage_estimate(&self.prompt_cache, &message.content, context, options);
        } else {
            // Normal scripted step: Pi `buildUsage` fixed defaults (input:100/output:50), NOT the
            // prompt-cache estimator (test-harness.ts:159).
            message.usage = build_usage(resp.usage.as_ref());
        }

        if exhausted {
            // Queue-exhaustion (no-step) path. Pi handles this OUT-OF-BAND of `streamWithDeltas`:
            // `outer.push({type:"error",reason:"error",error:message}); outer.end(message); return;`
            // (faux.ts:451-461) — it pushes a SINGLE `error` event and never emits a leading `start`.
            // Routing through `faux_event_stream` here would prepend `StreamEvent::Start`, yielding
            // `[start, error]` where Pi yields `[error]`. Emit just the terminal error event to match
            // Pi's exact event sequence. `StreamEvent::terminal` maps the `Error` stop reason onto
            // `StreamEvent::Error { reason: Error, error: message }` — the error event IS the terminal.
            return Box::pin(futures::stream::once(async move {
                StreamEvent::terminal(message)
            }));
        }

        let inner = faux_event_stream(message, options, ChunkConfig::default());

        match resp.delay_ms {
            Some(ms) if ms > 0 => {
                // Pi `setTimeout(emit, delayMs)` (test-harness.ts:304).
                let delayed = futures::stream::once(async move {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                    inner
                })
                .flatten();
                Box::pin(delayed)
            }
            _ => inner,
        }
    }
}

/// Build a scripted provider + a shared state handle (Pi `createFauxStreamFn` return shape,
/// test-harness.ts:281-314).
pub fn create_faux_stream_fn(
    responses: Vec<FauxResponse>,
) -> (Arc<ScriptedProvider>, Arc<Mutex<FauxStreamFnState>>) {
    let provider = Arc::new(ScriptedProvider::new(responses));
    let state = provider.state();
    (provider, state)
}

/// Build a scripted provider over a caller-supplied `model` + a shared state handle (Pi
/// `createFauxStreamFn` with a `model`/`contextWindow` override, test-harness.ts:370-372).
pub fn create_faux_stream_fn_with_model(
    responses: Vec<FauxResponse>,
    model: Model,
) -> (Arc<ScriptedProvider>, Arc<Mutex<FauxStreamFnState>>) {
    let provider = Arc::new(ScriptedProvider::with_model(responses, model));
    let state = provider.state();
    (provider, state)
}

/// Build a cycling scripted provider over **multiple** models + a shared state handle (Pi
/// `registerFauxProvider({ models })` resolved through the cycling stream fn, suite/harness.ts:64).
pub fn create_faux_stream_fn_with_models(
    responses: Vec<FauxResponse>,
    models: Vec<Model>,
) -> (Arc<ScriptedProvider>, Arc<Mutex<FauxStreamFnState>>) {
    let provider = Arc::new(ScriptedProvider::with_models(responses, models));
    let state = provider.state();
    (provider, state)
}

/// Build a **queue-consuming** scripted provider over the given models + a shared state handle (Pi
/// `registerFauxProvider`, the `suite/harness.ts` flavour: `setResponses`/`appendResponses`/
/// `getPendingResponseCount` + the exhaustion error).
pub fn create_faux_stream_fn_queued(
    responses: Vec<FauxResponse>,
    models: Vec<Model>,
) -> (Arc<ScriptedProvider>, Arc<Mutex<FauxStreamFnState>>) {
    let provider = Arc::new(ScriptedProvider::queued(responses, models));
    let state = provider.state();
    (provider, state)
}
