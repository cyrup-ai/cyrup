//! The faux provider — scripted, deterministic, no network, no tokens (func-01 §15 / R-01-051..053).
//!
//! 1:1 port of Pi's faux provider core (`pi/packages/ai/src/providers/faux.ts`): scripted response
//! steps (static messages **or** dynamic `(context, state, model) → AssistantMessage` factories,
//! faux.ts:96-103), token-level chunked streaming (`splitStringByTokenSize`, faux.ts:253-263),
//! `tokensPerSecond` pacing (`scheduleChunk`, faux.ts:300-306), mid-stream abort handling
//! (faux.ts:316-391), per-session prompt-cache estimation via common-prefix accounting
//! (`withUsageEstimate`/`commonPrefixLength`, faux.ts:204-251), an `on_response` callback
//! (faux.ts:449), and multiple model definitions + id lookup (faux.ts:416-440,481-488).
//!
//! Used by tests/demos across the workspace (agent loop, sessions, compaction, tools, hooks) so
//! they run without real provider APIs (func-00 R-00-011). Available to this crate's own tests and
//! behind the `faux` feature for downstream consumers.
//!
//! ## Deferred turns (pi v0.84.x)
//!
//! pi's faux gained a deferred half in v0.84.x — v0.83.0's `faux.ts` contains the string `deferred`
//! zero times. cyrup ports the part that stands alone: a scripted turn may carry a
//! [`cyrup_core::DeferredHandle`] ([`FauxMessageOptions::deferred`], pi `:80`/`:94`), and
//! [`faux_deferred_message`] builds pi's exact `createDeferredMessage` receipt (pi `:293-305`), so
//! the deferred READ path is exercisable offline against a produced `stopReason: "deferred"` turn.
//!
//! What is NOT ported, and precisely what it is blocked on — none of it can be written without
//! first inventing public API that does not exist in cyrup:
//!
//! * **`streamOptions.deferred` submission** (pi `:524-550`) needs `StreamOptions.deferred`
//!   (pi `v0.84.1 ai/src/types.ts:307`). [`crate::stream::StreamOptions`] has no such field, and no
//!   cyrup wire api reads one.
//! * **`fetchDeferred` / `cancelDeferred`** (pi `:567-642`, exposed on the provider at `:694-695`)
//!   need `Api.fetchDeferred`/`Api.cancelDeferred` (pi `v0.84.1 ai/src/types.ts:271-276`) plus
//!   `DeferredFetchOptions`/`DeferredCancelOptions` (pi `:223-232`). [`crate::provider::Provider`]
//!   declares neither method and neither options type exists in the workspace.
//! * Consequently the `deferredResponses` registry, `RegisterFauxProviderOptions.deferred`
//!   (`pendingFetches`/`pollAfterMs`, pi `:120-124`) and
//!   `FauxProviderState.{deferredFetchCount,cancelledDeferred}` (pi `:103-104`) have nothing to
//!   drive them and are deliberately absent rather than dead-coded.

use crate::context::Context;
use crate::model::{Modality, Model, ModelCost};
use crate::provider::Provider;
use crate::stream::{ErrorReason, StreamEvent, StreamOptions};
use cyrup_core::{
    ApiId, AssistantMessage, Content, Cost, DeferredHandle, EventStream, Message, ProviderId,
    SharedStr, StopReason, ToolCall, ToolCallId, Usage,
};
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

static FAUX_CALL_SEQ: AtomicUsize = AtomicUsize::new(0);

const DEFAULT_PROVIDER: &str = "faux";
const DEFAULT_API: &str = "faux";
const DEFAULT_MODEL_ID: &str = "faux-1";
const DEFAULT_MODEL_NAME: &str = "Faux Model";
const DEFAULT_MIN_TOKEN_SIZE: usize = 3;
const DEFAULT_MAX_TOKEN_SIZE: usize = 5;
const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;
const DEFAULT_MAX_TOKENS: u64 = 16_384;

/// Rough usage estimate: ~1 token per 4 characters (func-01 R-01-052; Pi `estimateTokens`,
/// faux.ts:140-142).
fn estimate_tokens(s: &str) -> u64 {
    (s.chars().count() as u64).div_ceil(4)
}

fn estimate_output(content: &[Content]) -> u64 {
    estimate_tokens(&assistant_content_to_text(content))
}

// ---- declarative model definitions (Pi `FauxModelDefinition`, faux.ts:37-45) ----

/// A faux model definition — the fields Pi exposes on `FauxModelDefinition` (faux.ts:37-45). Unset
/// fields take Pi's defaults via [`FauxModelDefinition::new`].
#[derive(Clone, Debug, PartialEq)]
pub struct FauxModelDefinition {
    pub id: String,
    pub name: Option<String>,
    pub reasoning: bool,
    pub input: Vec<Modality>,
    pub cost: ModelCost,
    pub context_window: u64,
    pub max_tokens: u64,
}

impl FauxModelDefinition {
    /// A definition for `id` with Pi's defaults (name=id, reasoning=false, input=`[text,image]`,
    /// zero cost, contextWindow 128000, maxTokens 16384).
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: None,
            reasoning: false,
            input: vec![Modality::Text, Modality::Image],
            cost: ModelCost::default(),
            context_window: DEFAULT_CONTEXT_WINDOW,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }

    fn into_model(self, api: &ApiId, provider: &ProviderId) -> Model {
        let name = self.name.unwrap_or_else(|| self.id.clone());
        Model {
            id: self.id.into(),
            name,
            api: api.clone(),
            provider: provider.clone(),
            base_url: "http://localhost:0".into(),
            reasoning: self.reasoning,
            input: self.input,
            cost: self.cost,
            context_window: self.context_window,
            max_tokens: self.max_tokens,
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }
}

/// A snapshot of the provider's call state, handed to a [`FauxResponseFactory`] (Pi
/// `state: { callCount }`, faux.ts:99).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FauxCallState {
    pub call_count: usize,
}

/// A dynamic response: `(context, options, state, model) → AssistantMessage` (Pi
/// `FauxResponseFactory`, faux.ts:96-101). 1:1 with Pi's four-arg signature: the factory sees the
/// request context, the resolved [`StreamOptions`] (so it can branch on `session_id`/`cancel`/
/// `cache_retention`), the call state, and the model. The async form is [`FauxAsyncResponseFactory`].
pub type FauxResponseFactory =
    Arc<dyn Fn(&Context, &StreamOptions, FauxCallState, &Model) -> AssistantMessage + Send + Sync>;

/// The async form of [`FauxResponseFactory`] (Pi `FauxResponseFactory` returning
/// `Promise<AssistantMessage>`, faux.ts:96-101,463-464). Resolved lazily inside the returned stream
/// (mirroring Pi's `queueMicrotask`), so it may `.await` network/timer work.
pub type FauxAsyncResponseFactory = Arc<
    dyn Fn(
            Context,
            StreamOptions,
            FauxCallState,
            Model,
        ) -> Pin<Box<dyn Future<Output = AssistantMessage> + Send>>
        + Send
        + Sync,
>;

/// One scripted step: a static message, a sync factory, or an async factory (Pi `FauxResponseStep`,
/// faux.ts:103 — a `AssistantMessage | FauxResponseFactory` where the factory may be async).
#[derive(Clone)]
pub enum FauxResponseStep {
    Message(Box<AssistantMessage>),
    Factory(FauxResponseFactory),
    AsyncFactory(FauxAsyncResponseFactory),
}

impl From<AssistantMessage> for FauxResponseStep {
    fn from(m: AssistantMessage) -> Self {
        FauxResponseStep::Message(Box::new(m))
    }
}

impl FauxResponseStep {
    /// A dynamic step from a sync closure (Pi sync factory step). The closure now receives the
    /// resolved [`StreamOptions`] as its second argument (Pi `(context, options, state, model)`).
    pub fn factory<F>(f: F) -> Self
    where
        F: Fn(&Context, &StreamOptions, FauxCallState, &Model) -> AssistantMessage
            + Send
            + Sync
            + 'static,
    {
        FauxResponseStep::Factory(Arc::new(f))
    }

    /// A dynamic step from an async closure (Pi async factory step, `Promise<AssistantMessage>`).
    /// The closure receives owned `(context, options, state, model)` and returns a future.
    pub fn async_factory<F, Fut>(f: F) -> Self
    where
        F: Fn(Context, StreamOptions, FauxCallState, Model) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = AssistantMessage> + Send + 'static,
    {
        FauxResponseStep::AsyncFactory(Arc::new(move |c, o, s, m| Box::pin(f(c, o, s, m))))
    }
}

/// The synthetic response metadata handed to [`OnResponse`] (Pi `{ status, headers }`, faux.ts:449).
/// The faux core always reports `status: 200` and empty `headers`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FauxResponseMeta {
    pub status: u16,
    pub headers: HashMap<String, String>,
}

/// Invoked with the synthetic response metadata + the request model before each stream begins (Pi
/// `streamOptions.onResponse({status:200, headers:{}}, requestModel)`, faux.ts:449). 1:1 with Pi:
/// the callback sees both the `{status, headers}` envelope and the resolved request [`Model`].
pub type OnResponse = Arc<dyn Fn(&FauxResponseMeta, &Model) + Send + Sync>;

/// Construction-time configuration mirroring Pi `RegisterFauxProviderOptions` (faux.ts:105-114).
#[derive(Clone)]
pub struct FauxConfig {
    pub api: ApiId,
    pub provider: ProviderId,
    pub models: Vec<FauxModelDefinition>,
    pub tokens_per_second: Option<f64>,
    pub min_token_size: usize,
    pub max_token_size: usize,
    pub on_response: Option<OnResponse>,
}

impl Default for FauxConfig {
    fn default() -> Self {
        Self {
            api: ApiId::from(DEFAULT_API),
            provider: ProviderId::from(DEFAULT_PROVIDER),
            models: vec![{
                let mut d = FauxModelDefinition::new(DEFAULT_MODEL_ID);
                d.name = Some(DEFAULT_MODEL_NAME.into());
                d
            }],
            tokens_per_second: None,
            min_token_size: DEFAULT_MIN_TOKEN_SIZE,
            max_token_size: DEFAULT_MAX_TOKEN_SIZE,
            on_response: None,
        }
    }
}

/// A scripted provider whose responses are consumed from a queue in request order (func-01 §15).
pub struct FauxProvider {
    id: ProviderId,
    api: ApiId,
    default_model: Model,
    models: Vec<Model>,
    queue: Mutex<VecDeque<FauxResponseStep>>,
    call_count: AtomicUsize,
    min_token_size: usize,
    max_token_size: usize,
    tokens_per_second: Option<f64>,
    on_response: Option<OnResponse>,
    /// Per-session serialized prompt, for common-prefix cache accounting (Pi `promptCache`,
    /// faux.ts:414). `Arc` so a lazily-resolved async factory can carry it into the returned stream.
    prompt_cache: Arc<Mutex<HashMap<String, String>>>,
}

impl Default for FauxProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FauxProvider {
    /// A provider with Pi's defaults: one `faux-1` model, single-block streaming chunking,
    /// no pacing (func-01 R-01-051).
    pub fn new() -> Self {
        Self::with_config(FauxConfig::default())
    }

    /// A provider configured exactly like Pi `createFauxCore(options)` (faux.ts:403-440).
    pub fn with_config(config: FauxConfig) -> Self {
        let id = config.provider;
        let api = config.api;
        let min_token_size = config
            .min_token_size
            .max(1)
            .min(config.max_token_size.max(1));
        let max_token_size = config.max_token_size.max(min_token_size);
        let defs = if config.models.is_empty() {
            FauxConfig::default().models
        } else {
            config.models
        };
        let models: Vec<Model> = defs.into_iter().map(|d| d.into_model(&api, &id)).collect();
        // `with_config` always yields ≥1 model (defaults fill the empty case).
        let default_model = models
            .first()
            .cloned()
            .unwrap_or_else(|| FauxModelDefinition::new(DEFAULT_MODEL_ID).into_model(&api, &id));
        Self {
            id,
            api,
            default_model,
            models,
            queue: Mutex::new(VecDeque::new()),
            call_count: AtomicUsize::new(0),
            min_token_size,
            max_token_size,
            tokens_per_second: config.tokens_per_second,
            on_response: config.on_response,
            prompt_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The default faux model (convenience for tests). Pi `getModel()` with no id (faux.ts:484).
    pub fn model(&self) -> &Model {
        &self.default_model
    }

    /// Replace the remaining response queue with static messages (func-01 R-01-052).
    pub fn set_responses(&self, responses: Vec<AssistantMessage>) {
        self.set_response_steps(responses.into_iter().map(FauxResponseStep::from).collect());
    }

    /// Append static messages to the queue.
    pub fn append_responses(&self, responses: Vec<AssistantMessage>) {
        self.append_response_steps(responses.into_iter().map(FauxResponseStep::from).collect());
    }

    /// Replace the remaining queue with scripted steps (Pi `setResponses`, faux.ts:498).
    pub fn set_response_steps(&self, steps: Vec<FauxResponseStep>) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
            q.extend(steps);
        }
    }

    /// Append scripted steps (Pi `appendResponses`, faux.ts:501).
    pub fn append_response_steps(&self, steps: Vec<FauxResponseStep>) {
        if let Ok(mut q) = self.queue.lock() {
            q.extend(steps);
        }
    }

    /// Pi `getPendingResponseCount` (faux.ts:504).
    pub fn pending_count(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    /// Pi `state.callCount` (faux.ts:413).
    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }

    fn chunk_config(&self) -> ChunkConfig {
        ChunkConfig {
            min_token_size: self.min_token_size,
            max_token_size: self.max_token_size,
            tokens_per_second: self.tokens_per_second,
            // Vary the deterministic chunk boundaries per call (like Pi's per-call `Math.random`),
            // while staying reproducible across runs ([CYRUP-DELTA]).
            seed: 0x9E37_79B9_7F4A_7C15 ^ (self.call_count() as u64).wrapping_mul(0x0100_0000_01B3),
        }
    }
}

/// Chunking + pacing knobs for [`faux_event_stream`] (Pi `tokenSize`/`tokensPerSecond`,
/// faux.ts:106-114).
#[derive(Clone, Copy, Debug)]
pub struct ChunkConfig {
    pub min_token_size: usize,
    pub max_token_size: usize,
    pub tokens_per_second: Option<f64>,
    /// Seed for the deterministic chunk-size PRNG ([CYRUP-DELTA] replacement for `Math.random`).
    pub seed: u64,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            min_token_size: DEFAULT_MIN_TOKEN_SIZE,
            max_token_size: DEFAULT_MAX_TOKEN_SIZE,
            tokens_per_second: None,
            seed: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

/// Pick a deterministic chunk token-size in `[min, max]` ([CYRUP-DELTA] replacement for Pi's
/// `Math.random`, faux.ts:257). Advances `rng` in place (xorshift64*).
fn next_token_size(rng: &mut u64, min: usize, max: usize) -> usize {
    let mut x = *rng;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *rng = x;
    let span = (max - min + 1) as u64;
    min + (x.wrapping_mul(0x2545_F491_4F6C_DD1D) % span) as usize
}

/// Split `text` into chunks of randomized 3–5-token (12–20-char) size (Pi `splitStringByTokenSize`,
/// faux.ts:253-263).
fn split_by_token_size(text: &str, rng: &mut u64, min: usize, max: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut chunks = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let char_size = (next_token_size(rng, min, max) * 4).max(1);
        let end = (i + char_size).min(chars.len());
        let chunk: String = chars
            .get(i..end)
            .map(|s| s.iter().collect())
            .unwrap_or_default();
        chunks.push(chunk);
        i = end;
    }
    if chunks.is_empty() {
        chunks.push(String::new());
    }
    chunks
}

/// Build the chunked event list for a final `message` (Pi `streamWithDeltas`, faux.ts:308-401),
/// without abort/pacing (those are applied by [`faux_event_stream`]).
fn build_events(message: &AssistantMessage, chunk: &ChunkConfig) -> Vec<StreamEvent> {
    let mut rng = chunk.seed | 1;
    let (min, max) = (chunk.min_token_size, chunk.max_token_size);
    // Pi builds the partial prototype as `{ ...message, content: [], stopReason: "pending" }`
    // (`v0.84.1 ai/src/providers/faux.ts:346`) — a SPREAD, so every other field of the scripted
    // message rides along and only `content`/`stopReason` are overridden. This used to be a
    // hand-written literal that dropped `diagnostics`, `deferred`, `errorMessage` and
    // `rawStopReason` on the floor; a scripted `stopReason: "deferred"` turn then streamed partials
    // with no handle on them. Cloning is the spread, verbatim.
    //
    // The `errorMessage` half moves one stored snapshot:
    // `cyrup-test-support/fixtures/pi/error-stream.events.jsonl` now carries `"errorMessage":"boom"`
    // on its `start` partial, because pi's spread carries it there. That file is a cyrup
    // self-recording (no `_note` pi-capture header), so it recorded the divergence, not pi; it was
    // regenerated with `CYRUP_UPDATE_GOLDEN=1`. No non-test consumer reads a `start` partial's
    // `error_message` — `agent.rs:802` and `modes/json_event.rs:127` both match `Start { .. }`.
    let proto = {
        let mut p = message.clone();
        p.content = Vec::new();
        p.stop_reason = StopReason::Pending;
        p
    };
    let mk = |content: Vec<Content>| {
        let mut p = proto.clone();
        p.content = content;
        Arc::new(p)
    };

    let mut events = vec![StreamEvent::Start {
        partial: mk(Vec::new()),
    }];
    let mut acc: Vec<Content> = Vec::new();

    for (i, block) in message.content.iter().enumerate() {
        match block {
            Content::Text { text, .. } => {
                let mut cur = acc.clone();
                cur.push(Content::text(String::new()));
                events.push(StreamEvent::TextStart {
                    content_index: i,
                    partial: mk(cur.clone()),
                });
                let mut grown = String::new();
                for chunk in split_by_token_size(text, &mut rng, min, max) {
                    grown.push_str(&chunk);
                    if let Some(Content::Text { text: t, .. }) = cur.get_mut(i) {
                        *t = SharedStr::from(&grown);
                    }
                    events.push(StreamEvent::TextDelta {
                        content_index: i,
                        delta: chunk,
                        partial: mk(cur.clone()),
                    });
                }
                if let Some(Content::Text { text: t, .. }) = cur.get_mut(i) {
                    *t = text.clone();
                }
                events.push(StreamEvent::TextEnd {
                    content_index: i,
                    content: text.to_string(),
                    partial: mk(cur.clone()),
                });
                acc = cur;
            }
            Content::Thinking { thinking, .. } => {
                let mut cur = acc.clone();
                cur.push(Content::thinking(String::new()));
                events.push(StreamEvent::ThinkingStart {
                    content_index: i,
                    partial: mk(cur.clone()),
                });
                let mut grown = String::new();
                for chunk in split_by_token_size(thinking, &mut rng, min, max) {
                    grown.push_str(&chunk);
                    if let Some(Content::Thinking { thinking: t, .. }) = cur.get_mut(i) {
                        *t = SharedStr::from(&grown);
                    }
                    events.push(StreamEvent::ThinkingDelta {
                        content_index: i,
                        delta: chunk,
                        partial: mk(cur.clone()),
                    });
                }
                if let Some(Content::Thinking { thinking: t, .. }) = cur.get_mut(i) {
                    *t = thinking.clone();
                }
                events.push(StreamEvent::ThinkingEnd {
                    content_index: i,
                    content: thinking.to_string(),
                    partial: mk(cur.clone()),
                });
                acc = cur;
            }
            Content::ToolCall(tc) => {
                let mut cur = acc.clone();
                // Pi keeps `arguments: {}` in the partial during streaming (faux.ts:377).
                cur.push(Content::ToolCall(ToolCall {
                    id: tc.id.clone(),
                    name: tc.name.clone(),
                    arguments: serde_json::Map::new().into(),
                    thought_signature: None,
                }));
                events.push(StreamEvent::ToolCallStart {
                    content_index: i,
                    partial: mk(cur.clone()),
                });
                let args_json = serde_json::to_string(&tc.arguments).unwrap_or_default();
                for chunk in split_by_token_size(&args_json, &mut rng, min, max) {
                    events.push(StreamEvent::ToolCallDelta {
                        content_index: i,
                        delta: chunk,
                        partial: mk(cur.clone()),
                    });
                }
                // The final toolcall partial carries the real parsed arguments (faux.ts:389).
                if let Some(Content::ToolCall(c)) = cur.get_mut(i) {
                    c.arguments = tc.arguments.clone();
                }
                events.push(StreamEvent::ToolCallEnd {
                    content_index: i,
                    tool_call: tc.clone(),
                    partial: mk(cur.clone()),
                });
                acc = cur;
            }
            // Images are carried only in the terminal message (faux does not chunk them).
            Content::Image { .. } => {}
        }
    }

    // Pi's own truncation guard: a scripted response whose `stopReason` is still `"pending"` makes
    // `streamWithDeltas` throw `"Faux response ended without a stop reason"` (faux.ts:393-395),
    // which its catch re-emits as `{type:"error", reason:"error"}`. Route through the same
    // `end_of_stream` seam as the five real wire APIs so the faux provider — which nine crates use
    // as their offline oracle — cannot disagree with them about what a truncated stream means.
    events.push(StreamEvent::end_of_stream(
        message.clone(),
        Some(message.stop_reason),
        "Faux response ended without a stop reason",
    ));
    events
}

/// State driving the abort/pacing unfold (Pi's `signal` checks + `scheduleChunk`).
struct StreamState {
    events: std::vec::IntoIter<StreamEvent>,
    cancel: Option<cyrup_core::CancelToken>,
    tokens_per_second: Option<f64>,
    prev_partial: Arc<AssistantMessage>,
    done: bool,
}

fn aborted_message(partial: &AssistantMessage) -> AssistantMessage {
    let mut m = partial.clone();
    m.stop_reason = StopReason::Aborted;
    m.error_message = Some("Request was aborted".into());
    m
}

/// Stream a finalized `message` as a chunked [`StreamEvent`] sequence (Pi `streamWithDeltas`,
/// faux.ts:308-401): a `start`, then per-block `*_start → (*_delta)* → *_end` with token-level
/// chunking, then the matching terminal — honoring `options.cancel` (mid-stream abort →
/// `error{reason:aborted}`) and `chunk.tokens_per_second` pacing. The `message` is streamed AS-IS
/// (no identity stamping / usage estimate); callers prepare it first. This is the shared engine
/// behind both [`FauxProvider`] and the test-support scripted stream fn.
pub fn faux_event_stream(
    message: AssistantMessage,
    options: &StreamOptions,
    chunk: ChunkConfig,
) -> EventStream<StreamEvent> {
    let events = build_events(&message, &chunk);
    let proto_empty = {
        let mut p = message;
        p.content = Vec::new();
        // The abort fallback is a PARTIAL (it is only read before the first event lands), so it
        // carries Pi's `"pending"` seed, not a settled reason. Same spread as `build_events`
        // (`v0.84.1 ai/src/providers/faux.ts:346`): `content` and `stopReason` are the ONLY
        // overrides — `errorMessage` used to be cleared here, which the spread does not do.
        p.stop_reason = StopReason::Pending;
        p
    };
    let state = StreamState {
        events: events.into_iter(),
        cancel: options.cancel.clone(),
        tokens_per_second: chunk.tokens_per_second,
        prev_partial: Arc::new(proto_empty),
        done: false,
    };
    Box::pin(futures::stream::unfold(state, |mut st| async move {
        if st.done {
            return None;
        }
        // Abort check before each event (Pi checks `signal.aborted` before start, before each block,
        // and before each chunk; faux.ts:317-388).
        if let Some(cancel) = &st.cancel
            && cancel.is_cancelled()
        {
            st.done = true;
            let aborted = Arc::new(aborted_message(&st.prev_partial));
            return Some((
                StreamEvent::Error {
                    reason: ErrorReason::Aborted,
                    error: aborted,
                },
                st,
            ));
        }
        let ev = match st.events.next() {
            Some(e) => e,
            None => return None,
        };
        // `tokensPerSecond` pacing for delta events (Pi `scheduleChunk`, faux.ts:300-306).
        if let Some(tps) = st.tokens_per_second
            && tps > 0.0
            && let Some(delta) = delta_text(&ev)
        {
            let secs = estimate_tokens(delta) as f64 / tps;
            if secs > 0.0 {
                tokio::time::sleep(Duration::from_secs_f64(secs)).await;
            }
        }
        if let Some(p) = ev.partial() {
            st.prev_partial = p.clone();
        }
        Some((ev, st))
    }))
}

impl Provider for FauxProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    fn stream(
        &self,
        request_model: &Model,
        context: &Context,
        options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        let call_count = self.call_count.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(cb) = &self.on_response {
            // Pi `onResponse({status, headers}, requestModel)` (faux.ts:449).
            cb(
                &FauxResponseMeta {
                    status: 200,
                    headers: HashMap::new(),
                },
                request_model,
            );
        }

        let step = self.queue.lock().ok().and_then(|mut q| q.pop_front());
        let state = FauxCallState { call_count };
        let api = self.api.clone();
        let id = self.id.clone();
        let model_id = request_model.id.as_str().to_string();
        let chunk = self.chunk_config();

        // Resolve the response message. An async factory is resolved lazily inside the returned
        // stream (Pi resolves the step in a `queueMicrotask`, faux.ts:447-467), so `await` work is
        // allowed; every other step kind resolves eagerly.
        let mut message = match step {
            Some(FauxResponseStep::AsyncFactory(f)) => {
                let cache = self.prompt_cache.clone();
                let ctx = context.clone();
                let opts = options.clone();
                let model = request_model.clone();
                let fut = async move {
                    let mut message = f(ctx.clone(), opts.clone(), state, model).await;
                    // Pi `cloneMessage`: stamp the request identity (faux.ts:265-275).
                    message.api = api;
                    message.provider = id;
                    message.model = model_id;
                    apply_usage_estimate(&cache, &mut message, &ctx, &opts);
                    faux_event_stream(message, &opts, chunk)
                };
                use futures::StreamExt as _;
                return Box::pin(futures::stream::once(fut).flatten());
            }
            Some(FauxResponseStep::Message(m)) => *m,
            Some(FauxResponseStep::Factory(f)) => f(context, options, state, request_model),
            None => AssistantMessage::errored(
                self.id.clone(),
                request_model.id.as_str(),
                Some(self.api.clone()),
                StopReason::Error,
                "No more faux responses queued",
            ),
        };

        // Pi `cloneMessage`: stamp the request identity onto the response (faux.ts:265-275).
        message.api = self.api.clone();
        message.provider = self.id.clone();
        message.model = model_id;
        apply_usage_estimate(&self.prompt_cache, &mut message, context, options);

        faux_event_stream(message, options, self.chunk_config())
    }
}

fn delta_text(ev: &StreamEvent) -> Option<&str> {
    match ev {
        StreamEvent::TextDelta { delta, .. }
        | StreamEvent::ThinkingDelta { delta, .. }
        | StreamEvent::ToolCallDelta { delta, .. } => Some(delta),
        _ => None,
    }
}

/// Common-prefix prompt-cache estimate (Pi `withUsageEstimate`, faux.ts:213-251). Free function so
/// both the eager path and the lazily-resolved async-factory path (which carries a cloned
/// `Arc<Mutex<_>>` cache into the returned stream) share one implementation.
fn apply_usage_estimate(
    prompt_cache: &Mutex<HashMap<String, String>>,
    message: &mut AssistantMessage,
    context: &Context,
    options: &StreamOptions,
) {
    message.usage = usage_estimate(prompt_cache, &message.content, context, options);
}

/// Compute the prompt-cache usage estimate (Pi `withUsageEstimate`, faux.ts:213-251) for an
/// assistant message's `content` against `context`/`options`, mutating `prompt_cache` exactly as Pi
/// mutates its per-provider `promptCache`. Exposed so an out-of-band caller — e.g. the scripted
/// harness's queue-exhaustion error terminal (Pi `createErrorMessage` → `withUsageEstimate`,
/// faux.ts:451-461) — can stamp the same usage object Pi does (`output:0` for the empty-content error
/// message, `input` = the serialized-context estimate) rather than the fixed `buildUsage` defaults.
pub fn usage_estimate(
    prompt_cache: &Mutex<HashMap<String, String>>,
    content: &[Content],
    context: &Context,
    options: &StreamOptions,
) -> Usage {
    let prompt_text = serialize_context(context);
    let prompt_tokens = estimate_tokens(&prompt_text);
    let output_tokens = estimate_output(content);
    let mut input = prompt_tokens;
    let mut cache_read = 0u64;
    let mut cache_write = 0u64;

    let cache_active = options.cache_retention != Some(crate::stream::CacheRetention::None);
    if let (Some(session_id), true) = (&options.session_id, cache_active) {
        let key = session_id.as_str().to_string();
        let mut cache = prompt_cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(previous) = cache.get(&key) {
            let cached_chars = common_prefix_len(previous, &prompt_text);
            cache_read = estimate_tokens(slice_chars(previous, 0, cached_chars));
            cache_write = estimate_tokens(slice_chars(&prompt_text, cached_chars, usize::MAX));
            input = prompt_tokens.saturating_sub(cache_read);
        } else {
            cache_write = prompt_tokens;
        }
        cache.insert(key, prompt_text);
    }

    Usage {
        input,
        output: output_tokens,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: input + output_tokens + cache_read + cache_write,
        cost: Cost::default(),
    }
}

// ---- context serialization for the prompt-cache estimate (Pi serializeContext, faux.ts:190-202) ----

fn slice_chars(s: &str, start: usize, end: usize) -> &str {
    // Byte-accurate prefix slicing is fine here: `commonPrefixLength` counts chars but the cached
    // text is reused verbatim, so a char-index → byte-range conversion keeps the estimate stable.
    let bytes_start = s
        .char_indices()
        .nth(start)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let bytes_end = if end == usize::MAX {
        s.len()
    } else {
        s.char_indices().nth(end).map(|(i, _)| i).unwrap_or(s.len())
    };
    s.get(bytes_start..bytes_end).unwrap_or("")
}

fn common_prefix_len(a: &str, b: &str) -> usize {
    a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count()
}

fn content_to_text(content: &[Content]) -> String {
    content
        .iter()
        .map(|b| match b {
            Content::Text { text, .. } => text.to_string(),
            Content::Image {
                mime_type, data, ..
            } => {
                format!("[image:{mime_type}:{}]", data.len())
            }
            _ => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn assistant_content_to_text(content: &[Content]) -> String {
    content
        .iter()
        .map(|b| match b {
            Content::Text { text, .. } => text.to_string(),
            Content::Thinking { thinking, .. } => thinking.to_string(),
            Content::ToolCall(tc) => {
                format!(
                    "{}:{}",
                    tc.name,
                    serde_json::to_string(&tc.arguments).unwrap_or_default()
                )
            }
            Content::Image { .. } => String::new(),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn message_to_text(message: &Message) -> String {
    match message {
        Message::User { content, .. } => content_to_text(content),
        Message::Assistant(a) => assistant_content_to_text(&a.content),
        Message::ToolResult {
            tool_name, content, ..
        } => {
            let mut parts = vec![tool_name.clone()];
            parts.push(content_to_text(content));
            parts.join("\n")
        }
    }
}

fn role_label(message: &Message) -> &'static str {
    match message {
        Message::User { .. } => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult { .. } => "toolResult",
    }
}

fn serialize_context(context: &Context) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(sp) = &context.system_prompt {
        parts.push(format!("system:{sp}"));
    }
    for message in &context.messages {
        parts.push(format!(
            "{}:{}",
            role_label(message),
            message_to_text(message)
        ));
    }
    if !context.tools.is_empty() {
        parts.push(format!(
            "tools:{}",
            serde_json::to_string(&context.tools).unwrap_or_default()
        ));
    }
    parts.join("\n\n")
}

// ---- Scripting helpers (func-01 §15; Pi faux.ts:49-94) ----

pub fn faux_text(s: impl Into<SharedStr>) -> Content {
    Content::text(s)
}

pub fn faux_thinking(s: impl Into<SharedStr>) -> Content {
    Content::thinking(s)
}

pub fn faux_tool_call(name: impl Into<String>, arguments: serde_json::Value) -> Content {
    faux_tool_call_with_id(name, arguments, None)
}

/// [`faux_tool_call`] with an optional explicit id (Pi `fauxToolCall(name, args, { id })`,
/// faux.ts:57-64). An absent id is minted as `tool:<ts>:<rand>` (Pi `randomId("tool")`, faux.ts:144)
/// with the timestamp zeroed and the random segment replaced by a monotonic counter so snapshots are
/// deterministic ([CYRUP-DELTA] for `Date.now()`/`Math.random()`).
pub fn faux_tool_call_with_id(
    name: impl Into<String>,
    arguments: serde_json::Value,
    id: Option<String>,
) -> Content {
    let id = id.unwrap_or_else(|| {
        let n = FAUX_CALL_SEQ.fetch_add(1, Ordering::SeqCst);
        format!("tool:0:{n}")
    });
    Content::ToolCall(ToolCall {
        id: ToolCallId::from(id),
        name: name.into(),
        // Pi `ToolCall.arguments` is always an object (types.ts:348); coerce a non-object to `{}`.
        arguments: match arguments {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        }
        .into(),
        thought_signature: None,
    })
}

/// Optional fields for [`faux_assistant_message_with`] (Pi `fauxAssistantMessage` options,
/// `v0.84.1 ai/src/providers/faux.ts:78-84`).
#[derive(Clone, Debug, Default)]
pub struct FauxMessageOptions {
    /// The deferred receipt handle (pi `deferred?: DeferredHandle`,
    /// `v0.84.1 ai/src/providers/faux.ts:80`, copied onto the message at `:94`). Pair it with
    /// [`StopReason::Deferred`] to script a turn that is a receipt rather than a reply.
    pub deferred: Option<DeferredHandle>,
    pub error_message: Option<String>,
    pub response_id: Option<String>,
    pub timestamp: Option<i64>,
}

/// Build a scripted assistant reply (func-01 §15). Usage output is estimated from `content`.
pub fn faux_assistant_message(content: Vec<Content>, stop_reason: StopReason) -> AssistantMessage {
    faux_assistant_message_with(content, stop_reason, FauxMessageOptions::default())
}

/// Build a scripted assistant reply with full options (Pi `fauxAssistantMessage(content, options)`,
/// faux.ts:73-94). `errorMessage`/`responseId`/`timestamp` mirror Pi's option struct.
pub fn faux_assistant_message_with(
    content: Vec<Content>,
    stop_reason: StopReason,
    options: FauxMessageOptions,
) -> AssistantMessage {
    let output = estimate_output(&content);
    AssistantMessage {
        content,
        provider: ProviderId::from(DEFAULT_PROVIDER),
        model: DEFAULT_MODEL_ID.to_string(),
        api: ApiId::from(DEFAULT_API),
        response_model: None,
        response_id: options.response_id,
        diagnostics: None,
        usage: Usage {
            output,
            total_tokens: output,
            ..Default::default()
        },
        stop_reason,
        deferred: options.deferred.map(Box::new),
        error_message: options.error_message,
        raw_stop_reason: None,
        timestamp: options.timestamp.unwrap_or(0),
    }
}

/// Build the deferred RECEIPT turn pi's faux provider emits when a submission is deferred (pi
/// `createDeferredMessage`, `v0.84.1 ai/src/providers/faux.ts:293-305`): empty content,
/// `stopReason: "deferred"`, the handle attached, and the request model's identity —
/// `api`/`provider`/`model` all taken from `model`, exactly as pi does at `:297-299`.
///
/// Version lag, not a port bug: pi's whole deferred half of `faux.ts` arrives in v0.84.x (`git diff
/// v0.83.0..v0.84.1 -- packages/ai/src/providers/faux.ts` is +178/-11 and v0.83.0's copy contains
/// zero occurrences of `deferred`).
///
/// Script it as a step and the faux provider streams it through the same `streamWithDeltas` engine
/// pi uses (`:541-548`), producing `start` → `done{reason:"deferred"}` — which is what makes the
/// deferred READ path (`AssistantMessage::deferred`, [`StopReason::Deferred`],
/// [`crate::stream::DoneReason::Deferred`]) exercisable offline.
///
/// [CYRUP-DELTA] `timestamp` is `0`, not `Date.now()` (`:303`), so snapshots reproduce — the same
/// substitution [`faux_tool_call_with_id`] already makes for `randomId`.
pub fn faux_deferred_message(model: &Model, handle: DeferredHandle) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.id.as_str().to_string(),
        api: model.api.clone(),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Deferred,
        deferred: Some(Box::new(handle)),
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
    }
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::stream::collect_message;
    use cyrup_core::CancelToken;
    use futures::StreamExt;

    #[tokio::test]
    async fn streams_scripted_text_and_done() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text("hello")],
            StopReason::Stop,
        )]);
        let model = faux.model().clone();
        let stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let msg = collect_message(stream).await;
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert_eq!(msg.content, vec![faux_text("hello")]);
        assert!(msg.usage.output > 0);
        assert!(msg.usage.total_tokens >= msg.usage.output);
        assert_eq!(faux.call_count(), 1);
        assert_eq!(faux.pending_count(), 0);
    }

    /// Nine crates use the faux provider as their offline oracle, so it must not disagree with the
    /// five real wire APIs about what an unfinished stream means. Pi's `streamWithDeltas` throws
    /// `"Faux response ended without a stop reason"` for a still-`"pending"` scripted response
    /// (faux.ts:393-395), and the catch re-emits `{type:"error", reason:"error"}`.
    #[tokio::test]
    async fn a_scripted_pending_response_is_an_error_terminal_not_a_done() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text("hello")],
            StopReason::Pending,
        )]);
        let model = faux.model().clone();
        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }

        // Every in-flight partial carries Pi's sentinel (faux.ts:316).
        for (i, p) in events.iter().filter_map(StreamEvent::partial).enumerate() {
            assert_eq!(p.stop_reason, StopReason::Pending, "partial #{i}");
        }

        match events.last() {
            Some(StreamEvent::Error { reason, error }) => {
                assert_eq!(*reason, crate::stream::ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(
                    error.error_message.as_deref(),
                    Some("Faux response ended without a stop reason")
                );
                // Pi's catch re-emits `output` with its accumulated blocks intact.
                assert_eq!(error.content, vec![faux_text("hello")]);
            }
            other => panic!("a pending scripted response must not settle cleanly, got {other:?}"),
        }
    }

    /// The in-flight `partial` of a perfectly normal faux stream must report `"pending"` on the
    /// wire, matching Pi's `{ ...message, content: [], stopReason: "pending" }` (faux.ts:316) — the
    /// terminal is unaffected.
    #[tokio::test]
    async fn faux_partials_report_pending_and_the_terminal_still_reports_stop() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text("hello world")],
            StopReason::Stop,
        )]);
        let model = faux.model().clone();
        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }
        let partials: Vec<_> = events.iter().filter_map(StreamEvent::partial).collect();
        assert!(!partials.is_empty());
        for p in &partials {
            assert_eq!(p.stop_reason, StopReason::Pending);
            assert_eq!(
                serde_json::to_value(*p).unwrap()["stopReason"],
                "pending",
                "wire spelling must be Pi's"
            );
        }
        match events.last() {
            Some(StreamEvent::Done { reason, message }) => {
                assert_eq!(*reason, crate::stream::DoneReason::Stop);
                assert_eq!(message.stop_reason, StopReason::Stop);
            }
            other => panic!("expected done/stop, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn empty_queue_yields_error_message() {
        let faux = FauxProvider::new();
        let model = faux.model().clone();
        let stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let msg = collect_message(stream).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.is_some());
    }

    #[tokio::test]
    async fn event_ordering_is_start_blocks_terminal() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![
                faux_thinking("t"),
                faux_tool_call("echo", serde_json::json!({"x": 1})),
            ],
            StopReason::ToolUse,
        )]);
        let model = faux.model().clone();
        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }
        assert!(matches!(events.first(), Some(StreamEvent::Start { .. })));
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ThinkingStart {
                content_index: 0,
                ..
            }
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            StreamEvent::ToolCallEnd {
                content_index: 1,
                ..
            }
        )));
    }

    #[tokio::test]
    async fn responses_consumed_in_order() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![
            faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
        ]);
        assert_eq!(faux.pending_count(), 2);
        let model = faux.model().clone();
        let m1 =
            collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        let m2 =
            collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(m1.content, vec![faux_text("first")]);
        assert_eq!(m2.content, vec![faux_text("second")]);
        assert_eq!(faux.call_count(), 2);
    }

    #[tokio::test]
    async fn chunked_streaming_emits_multiple_deltas() {
        let faux = FauxProvider::new();
        // 60 chars ⇒ many 12–20-char deltas (>1).
        let long = "x".repeat(60);
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text(&long)],
            StopReason::Stop,
        )]);
        let model = faux.model().clone();
        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut deltas = 0;
        let mut reassembled = String::new();
        while let Some(ev) = stream.next().await {
            if let StreamEvent::TextDelta { delta, .. } = &ev {
                deltas += 1;
                reassembled.push_str(delta);
            }
        }
        assert!(deltas > 1, "expected chunked deltas, got {deltas}");
        assert_eq!(reassembled, long);
    }

    #[tokio::test]
    async fn dynamic_factory_sees_call_state() {
        let faux = FauxProvider::new();
        faux.set_response_steps(vec![FauxResponseStep::factory(
            |_ctx, _opts, state, _model| {
                faux_assistant_message(
                    vec![faux_text(format!("call-{}", state.call_count))],
                    StopReason::Stop,
                )
            },
        )]);
        let model = faux.model().clone();
        let msg =
            collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.content, vec![faux_text("call-1")]);
    }

    #[tokio::test]
    async fn sync_factory_sees_stream_options() {
        use cyrup_core::SessionId;
        let faux = FauxProvider::new();
        // The factory branches on the resolved StreamOptions (Pi's 2nd factory arg, faux.ts:96-101).
        faux.set_response_steps(vec![FauxResponseStep::factory(
            |_ctx, opts, _state, _model| {
                let sid = opts
                    .session_id
                    .as_ref()
                    .map(|s| s.as_str().to_string())
                    .unwrap_or_default();
                faux_assistant_message(vec![faux_text(format!("sid-{sid}"))], StopReason::Stop)
            },
        )]);
        let model = faux.model().clone();
        let opts = StreamOptions {
            session_id: Some(SessionId::from("s7")),
            ..Default::default()
        };
        let msg = collect_message(faux.stream(&model, &Context::default(), &opts)).await;
        assert_eq!(msg.content, vec![faux_text("sid-s7")]);
    }

    #[tokio::test]
    async fn async_factory_resolves_lazily() {
        let faux = FauxProvider::new();
        faux.set_response_steps(vec![FauxResponseStep::async_factory(
            |_ctx, _opts, state, _model| async move {
                // `await` work is allowed in the async factory (Pi `Promise<AssistantMessage>`).
                tokio::task::yield_now().await;
                faux_assistant_message(
                    vec![faux_text(format!("async-{}", state.call_count))],
                    StopReason::Stop,
                )
            },
        )]);
        let model = faux.model().clone();
        let msg =
            collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(msg.content, vec![faux_text("async-1")]);
    }

    #[tokio::test]
    async fn aborted_before_start_yields_aborted_error() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text("hi")],
            StopReason::Stop,
        )]);
        let model = faux.model().clone();
        let cancel = CancelToken::new();
        cancel.cancel();
        let opts = StreamOptions {
            cancel: Some(cancel),
            ..Default::default()
        };
        let mut stream = faux.stream(&model, &Context::default(), &opts);
        let first = stream.next().await.expect("an event");
        assert!(matches!(
            first,
            StreamEvent::Error {
                reason: ErrorReason::Aborted,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn on_response_callback_fires() {
        let hits = Arc::new(AtomicUsize::new(0));
        let h = hits.clone();
        let cfg = FauxConfig {
            on_response: Some(Arc::new(move |meta: &FauxResponseMeta, model: &Model| {
                // Pi `onResponse({status, headers}, requestModel)` (faux.ts:449).
                assert_eq!(meta.status, 200);
                assert!(meta.headers.is_empty());
                assert_eq!(model.id.as_str(), DEFAULT_MODEL_ID);
                h.fetch_add(1, Ordering::SeqCst);
            })),
            ..Default::default()
        };
        let faux = FauxProvider::with_config(cfg);
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text("hi")],
            StopReason::Stop,
        )]);
        let model = faux.model().clone();
        let _ =
            collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default()))
                .await;
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn prompt_cache_splits_on_repeat_session() {
        use cyrup_core::SessionId;
        let faux = FauxProvider::new();
        faux.set_responses(vec![
            faux_assistant_message(vec![faux_text("a")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("b")], StopReason::Stop),
        ]);
        let model = faux.model().clone();
        let sid = SessionId::from("s1");
        let opts = StreamOptions {
            session_id: Some(sid),
            ..Default::default()
        };
        let mut ctx = Context {
            system_prompt: Some("hello world prompt".into()),
            ..Default::default()
        };
        let m1 = collect_message(faux.stream(&model, &ctx, &opts)).await;
        // First call writes the cache, reads nothing.
        assert!(m1.usage.cache_write > 0);
        assert_eq!(m1.usage.cache_read, 0);
        // Append more to the same prompt prefix → second call reads the shared prefix.
        ctx.messages.push(Message::User {
            content: vec![Content::text("more")],
            timestamp: 0,
        });
        let m2 = collect_message(faux.stream(&model, &ctx, &opts)).await;
        assert!(
            m2.usage.cache_read > 0,
            "expected a cache read on the shared prefix"
        );
    }

    fn handle(model: &Model) -> DeferredHandle {
        DeferredHandle {
            provider: model.provider.as_str().to_string(),
            model_id: model.id.as_str().to_string(),
            api: model.api.as_str().to_string(),
            id: "deferred:0:1".to_string(),
            expires_at: None,
            poll_after_ms: Some(1500),
            data: Some(serde_json::json!({"batch": "b-7"})),
        }
    }

    /// Version lag (pi's deferred half of `faux.ts` lands in v0.84.x; v0.83.0's copy contains the
    /// string `deferred` zero times): pi can emit `createDeferredMessage`
    /// (`v0.84.1 ai/src/providers/faux.ts:293-305`) and cyrup could not, so no test anywhere could
    /// drive cyrup's deferred READ path from a produced deferred turn.
    #[tokio::test]
    async fn a_scripted_deferred_turn_streams_a_deferred_done_carrying_its_handle() {
        let faux = FauxProvider::new();
        let model = faux.model().clone();
        let h = handle(&model);
        faux.set_responses(vec![faux_deferred_message(&model, h.clone())]);

        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }

        // pi's receipt has empty content, so `streamWithDeltas` emits start → done only (`:356`
        // never enters the block loop).
        assert_eq!(events.len(), 2, "{events:?}");
        match events.last() {
            Some(StreamEvent::Done { reason, message }) => {
                assert_eq!(*reason, crate::stream::DoneReason::Deferred);
                assert_eq!(message.stop_reason, StopReason::Deferred);
                assert!(message.content.is_empty());
                assert_eq!(message.deferred.as_deref(), Some(&h));
                assert_eq!(
                    serde_json::to_value(message).unwrap()["stopReason"],
                    "deferred",
                    "wire spelling must be Pi's"
                );
            }
            other => panic!("expected done/deferred, got {other:?}"),
        }

        // pi's in-flight partial is `{ ...message, content: [], stopReason: "pending" }`
        // (`:346`) — a SPREAD, so the handle rides along on every partial. cyrup's hand-written
        // prototype used to drop it.
        for (i, p) in events.iter().filter_map(StreamEvent::partial).enumerate() {
            assert_eq!(p.stop_reason, StopReason::Pending, "partial #{i}");
            assert_eq!(p.deferred.as_deref(), Some(&h), "partial #{i}");
        }
    }

    /// The scripting-surface half: pi's `fauxAssistantMessage` takes `deferred` in its options bag
    /// (`v0.84.1 ai/src/providers/faux.ts:80`, copied at `:94`).
    ///
    /// MIRROR: an ordinary scripted reply carries no handle and no partial gains one, so the spread
    /// fix cannot leak a handle onto a non-deferred turn.
    #[tokio::test]
    async fn the_deferred_option_is_scriptable_and_absent_by_default() {
        let faux = FauxProvider::new();
        let model = faux.model().clone();
        let h = handle(&model);
        faux.set_responses(vec![
            faux_assistant_message_with(
                vec![faux_text("receipt")],
                StopReason::Deferred,
                FauxMessageOptions {
                    deferred: Some(h.clone()),
                    ..Default::default()
                },
            ),
            faux_assistant_message(vec![faux_text("ordinary")], StopReason::Stop),
        ]);

        let m1 = collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default()))
            .await;
        assert_eq!(m1.stop_reason, StopReason::Deferred);
        assert_eq!(m1.deferred.as_deref(), Some(&h));

        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }
        for p in events.iter().filter_map(StreamEvent::partial) {
            assert_eq!(p.deferred, None);
        }
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
    }

    #[tokio::test]
    async fn multiple_models_lookup_by_id() {
        let cfg = FauxConfig {
            models: vec![
                FauxModelDefinition::new("m-a"),
                FauxModelDefinition::new("m-b"),
            ],
            ..Default::default()
        };
        let faux = FauxProvider::with_config(cfg);
        assert_eq!(faux.model().id.as_str(), "m-a");
        assert!(faux.get_model("m-b").is_some());
        assert!(faux.get_model("nope").is_none());
    }
}
