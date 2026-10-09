//! Extension model calls through the session's providers — pi `ctx.modelRegistry.complete()`,
//! `stream()` and `streamSimple()` (EXT-086).
//!
//! Upstream (`packages/coding-agent/src/core/model-registry.ts` @v1.0.4) the three are one-line
//! delegations to the model runtime: `stream` and `complete` to `ModelRuntime.stream` (whose
//! `prepareRequest` resolves the provider's request auth per call), `streamSimple` to
//! `ModelRuntime.streamSimple`, the only one of the three with a virtual-model arm
//! (`core/model-runtime.ts:691-741`). `complete` is `stream(…).result()` (`:707-713`). This module
//! holds the parts of that which belong to the EXTENSION side of the seam and are the same for every
//! backend:
//!
//! * [`ModelCall`] — one call as values: the model ADDRESS, pi's `Context`, and the subset of pi's
//!   stream options that can cross a component boundary ([`ModelCallOptions`]);
//! * [`settle`] — pi's `.result()`, the fold `complete` is defined as;
//! * [`complete_bounded`] — `complete` for a caller that must not hang: the provider work runs on
//!   its own task, aborted when the caller goes away or a ceiling elapses;
//! * [`ModelStreams`] — the guest-owned stream table behind `models.stream` /
//!   `models.stream-simple` / `models.poll-stream` / `models.close-stream`.
//!
//! The BACKEND — which provider serves a model, with what auth, and whether a virtual model is
//! routed — is [`crate::host::HostServices::model_stream`], which the session implements.
//!
//! # Why a stream is a handle the guest polls
//!
//! The same reason `http-client.request-stream` is one (`wit/world.wit`, the request/poll bridge): a
//! Component Model guest cannot hold a live host `Stream`, and the host cannot push into a guest
//! that is suspended in a call — re-entering its single-instance store is exactly what
//! [`crate::host::GuestReentry`] exists to refuse. So the HOST owns the provider stream, a pump
//! task drains it into a bounded buffer, and the guest asks for whatever has arrived. Unlike the
//! HTTP precedent, a poll never parks the guest's store on an open-ended wait: it waits at most
//! [`MODEL_STREAM_POLL_WAIT`] for the FIRST event and returns every event already buffered (up to
//! [`MAX_EVENTS_PER_POLL`]) — possibly none, which is NOT end-of-stream. One round trip per batch,
//! not per token.
//!
//! # What a hung, panicking or abandoned caller can and cannot do
//!
//! * The provider stream lives on a pump task whose [`tokio::task::AbortHandle`] the table holds.
//!   `close`, dropping the table (the guest instance going away) and the abandonment reaper all
//!   ABORT that task, which drops the provider stream and with it the in-flight HTTP request.
//! * A guest that stops polling for [`MODEL_STREAM_ABANDON_AFTER`] is treated as gone: the pump
//!   stops on its own and the handle answers a typed error. A wedged or trapped guest therefore
//!   cannot keep a paid request running.
//! * A pump that panics (a provider bug) closes its buffer without a terminal; the next poll turns
//!   that into one synthesized `error` terminal, so the guest always sees exactly one terminal and
//!   then end-of-stream.
//! * `complete` runs the same way and is bounded by [`MODEL_COMPLETE_TIMEOUT`]; its task is aborted
//!   if the awaiting caller is dropped (a cancelled tool call, an epoch trap).
//!
//! No task this module spawns is awaited by anything that keeps the process alive: each one is
//! owned by an abort handle and dies with the runtime.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::{
    AssistantMessage, CancelToken, EventStream, ModelThinkingLevel, ProviderId, StopReason,
};
use cyrup_provider::{
    CacheRetention, Context, HeaderMap, SimpleStreamOptions, StreamEvent, StreamOptions,
};
use futures::StreamExt;
use serde_json::Value;

/// How long ONE `poll-stream` waits for the first event before answering with an empty batch.
/// Short on purpose: the guest's store is suspended for the whole wait, so this bounds how long a
/// poll can hold the instance, while still letting a guest that polls in a loop spend its time
/// suspended rather than spinning.
pub const MODEL_STREAM_POLL_WAIT: Duration = Duration::from_secs(1);

/// The most events one poll returns. A provider that bursts thousands of deltas is drained over
/// several polls rather than serialized into one unbounded host→guest copy.
pub const MAX_EVENTS_PER_POLL: usize = 256;

/// How many events the pump buffers ahead of the guest. When the guest falls this far behind, the
/// pump stops reading the provider — backpressure onto the HTTP body instead of host memory.
pub const MODEL_STREAM_BUFFER: usize = 1024;

/// A stream whose guest has not polled for this long is abandoned: its pump aborts the provider
/// request and the handle answers an error. Generous, because a guest may legitimately open a
/// stream in one handler and drain it from another; finite, because a guest that trapped, was
/// wedged or simply forgot the handle must not keep spending the user's money.
pub const MODEL_STREAM_ABANDON_AFTER: Duration = Duration::from_secs(120);

/// The most model streams ONE extension may hold open at once. Each is a live provider request.
pub const MAX_OPEN_MODEL_STREAMS: usize = 16;

/// Ceiling on a guest's `models.complete`, which suspends the guest's store for its whole length.
/// pi has no ceiling (a JS `await` suspends nothing else); here an unbounded call would hold the
/// extension instance — every one of its handlers — for as long as a provider chose to stall.
pub const MODEL_COMPLETE_TIMEOUT: Duration = Duration::from_secs(600);

/// Which of pi's request-time verbs a [`ModelCall`] is. `complete` is not a third variant: it is
/// `stream(…).result()` upstream (`core/model-runtime.ts:707-713` @v1.0.4), so it is a
/// [`Self::Stream`] call folded by [`settle`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelCallVerb {
    /// pi `stream(model, context, options)` — provider-specific options, and NO virtual-model
    /// routing: a virtual model settles as pi's "must be routed before streaming" error.
    Stream,
    /// pi `streamSimple(model, context, options)` — the provider-neutral options (`reasoning`), and
    /// the one verb that routes a virtual model (reason `direct`, `model-runtime.ts:717-734`).
    StreamSimple,
}

/// The subset of pi's `ModelsApiStreamOptions` / `ModelsSimpleStreamOptions` a guest can send as
/// JSON (camelCase, pi's own key names). Unknown keys are ignored, as a JS options bag ignores
/// them.
///
/// `[CYRUP-DELTA]` Four groups of pi's options do not cross, each for a stated reason:
///
/// * **`apiKey`, `env`** — the credential plane. pi's `prepareRequest` lets an explicit caller key
///   win over the resolved one (`model-runtime.ts:681`); a sandboxed guest has no key of the user's
///   to supply, and an `env` overlay could redirect a provider's endpoint (`LLAMA_BASE_URL`) with
///   the user's credential attached. The call always uses the session's resolved auth.
/// * **`signal`** — a closure-shaped `AbortSignal`. Cancelling a stream is `close-stream`; a
///   native passes a real [`CancelToken`] on [`ModelCall::cancel`].
/// * **`onPayload` / `onResponse` / `onProviderStreamEvent` / `transformHeaders`** — callbacks,
///   which cannot cross a component boundary (ADR-0002).
/// * **`transport`, `websocketConnectTimeoutMs`** — transport selection the port does not
///   implement (see `StreamOptions::websocket_connect_timeout_ms`).
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCallOptions {
    /// pi `maxTokens`.
    #[serde(default)]
    pub max_tokens: Option<u64>,
    /// pi `temperature`.
    #[serde(default)]
    pub temperature: Option<f32>,
    /// pi `reasoning` — read by `streamSimple` only, as upstream (`stream` takes the provider's own
    /// options). `"off"` is accepted and means none, which is what pi's virtual arm passes
    /// (`options?.reasoning ?? "off"`).
    #[serde(default)]
    pub reasoning: Option<ModelThinkingLevel>,
    /// pi `sessionId` — the provider-side cache/affinity key. Absent means none, NOT the
    /// conversation's: an extension call is not the session's request.
    #[serde(default)]
    pub session_id: Option<String>,
    /// pi `cacheRetention`: `"none" | "short" | "long"`.
    #[serde(default, deserialize_with = "de_cache_retention")]
    pub cache_retention: Option<CacheRetention>,
    /// pi `headers` — merged over the provider's and the model's, a `null` value suppressing one.
    #[serde(default)]
    pub headers: Option<HeaderMap>,
    /// pi `metadata`.
    #[serde(default)]
    pub metadata: Option<serde_json::Map<String, Value>>,
    /// pi `samplingParams`.
    #[serde(default)]
    pub sampling_params: Option<serde_json::Map<String, Value>>,
    /// pi `timeoutMs`.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// pi `maxRetries`.
    #[serde(default)]
    pub max_retries: Option<u32>,
    /// pi `maxRetryDelayMs`.
    #[serde(default)]
    pub max_retry_delay_ms: Option<u64>,
}

fn de_cache_retention<'de, D>(d: D) -> Result<Option<CacheRetention>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<String> = serde::Deserialize::deserialize(d)?;
    match raw.as_deref() {
        None => Ok(None),
        Some("none") => Ok(Some(CacheRetention::None)),
        Some("short") => Ok(Some(CacheRetention::Short)),
        Some("long") => Ok(Some(CacheRetention::Long)),
        Some(other) => Err(serde::de::Error::custom(format!(
            "cacheRetention must be \"none\", \"short\" or \"long\", not {other:?}"
        ))),
    }
}

impl ModelCallOptions {
    /// Parse a guest's `opts-json`. Empty, `null` and `{}` are all "no options".
    ///
    /// # Errors
    ///
    /// The JSON is not an object, or a known key has the wrong type.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed == "null" {
            return Ok(Self::default());
        }
        serde_json::from_str(trimmed)
            .map_err(|e| format!("opts-json is not a stream options object: {e}"))
    }

    /// The provider-level [`StreamOptions`] these options lower to. `cancel` is pi's `signal`.
    #[must_use]
    pub fn stream_options(&self, cancel: &CancelToken) -> StreamOptions {
        StreamOptions {
            cancel: Some(cancel.clone()),
            session_id: self.session_id.as_deref().map(Into::into),
            cache_retention: self.cache_retention,
            temperature: self.temperature,
            sampling_params: self.sampling_params.clone(),
            max_tokens: self.max_tokens,
            headers: self.headers.clone(),
            timeout_ms: self.timeout_ms,
            max_retries: self.max_retries,
            max_retry_delay_ms: self.max_retry_delay_ms,
            metadata: self.metadata.clone(),
            ..StreamOptions::default()
        }
    }

    /// The [`SimpleStreamOptions`] a `streamSimple` call lowers to: the same base plus the unified
    /// `reasoning` on-level (`off` is none).
    #[must_use]
    pub fn simple_options(&self, cancel: &CancelToken) -> SimpleStreamOptions {
        SimpleStreamOptions {
            base: self.stream_options(cancel),
            reasoning: self.reasoning.and_then(ModelThinkingLevel::level),
            thinking_budgets: None,
        }
    }
}

/// One model call: pi's `(model, context, options)` triple, as values, plus the verb and the
/// cancellation that stands in for pi's `signal`.
///
/// The model is an ADDRESS — `(provider, model_id)` — and never the model object a caller sends.
/// `[CYRUP-DELTA]` pi takes a whole `Model` and streams against whatever `baseUrl` and `headers`
/// it carries, with the provider's resolved credential attached (`prepareRequest`,
/// `model-runtime.ts:658-688` @v1.0.4). For a JS extension with ambient authority that is no
/// widening; for a sandboxed guest it would be a way to send the user's key to an endpoint of the
/// guest's choosing. So the backend re-resolves the address against the session's own catalog and
/// uses that row, exactly as [`crate::host::GuestModelRouter`] treats a router's answer.
#[derive(Clone)]
pub struct ModelCall {
    /// Which verb.
    pub verb: ModelCallVerb,
    /// The model's provider id (pi `model.provider`).
    pub provider: String,
    /// The model's id (pi `model.id`).
    pub model_id: String,
    /// pi's `Context`: `{ systemPrompt?, messages, tools? }`.
    pub context: Context,
    /// The crossable options.
    pub options: ModelCallOptions,
    /// pi's `options.signal`. Firing it aborts the provider request; the stream then ends with
    /// pi's `aborted` terminal.
    pub cancel: CancelToken,
}

impl std::fmt::Debug for ModelCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelCall")
            .field("verb", &self.verb)
            .field("provider", &self.provider)
            .field("model_id", &self.model_id)
            .field("messages", &self.context.messages.len())
            .field("options", &self.options)
            .finish_non_exhaustive()
    }
}

impl ModelCall {
    /// Decode a guest's three JSON arguments.
    ///
    /// `model_json` is any object carrying string `provider` and `id` keys — the whole row a guest
    /// read from `models.list-models` qualifies, and so does a bare `{provider, id}`; everything
    /// else in it is ignored (see the type's `[CYRUP-DELTA]`).
    ///
    /// # Errors
    ///
    /// A malformed argument. These are "the call could not be made" — a model that is merely
    /// unknown is NOT one: like pi, that settles as an error message from the backend.
    pub fn from_json(
        verb: ModelCallVerb,
        model_json: &str,
        context_json: &str,
        opts_json: &str,
    ) -> Result<Self, String> {
        let model: Value =
            serde_json::from_str(model_json).map_err(|e| format!("model-json is not JSON: {e}"))?;
        let field = |key: &str| {
            model
                .get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        };
        let (Some(provider), Some(model_id)) = (field("provider"), field("id")) else {
            return Err(
                "model-json must be a model object with non-empty string `provider` and `id`"
                    .to_string(),
            );
        };
        let context: Context = serde_json::from_str(context_json)
            .map_err(|e| format!("context-json is not a model context: {e}"))?;
        Ok(Self {
            verb,
            provider,
            model_id,
            context,
            options: ModelCallOptions::from_json(opts_json)?,
            cancel: CancelToken::new(),
        })
    }

    /// The settled error message for this call — what a backend answers instead of streaming when
    /// it cannot serve the call. pi delivers these through the stream, never as a rejection
    /// (`lazyStream`'s failure arm, `packages/ai/src/api/lazy.ts`), so `complete` resolves with one.
    #[must_use]
    pub fn errored(&self, message: impl Into<String>) -> AssistantMessage {
        AssistantMessage::errored(
            ProviderId::from(self.provider.as_str()),
            self.model_id.as_str(),
            None,
            StopReason::Error,
            message,
        )
    }

    /// A one-event stream carrying [`Self::errored`] as its terminal.
    #[must_use]
    pub fn errored_stream(&self, message: impl Into<String>) -> EventStream<StreamEvent> {
        terminal_only(self.errored(message))
    }
}

/// A stream of exactly one terminal event.
#[must_use]
pub fn terminal_only(message: AssistantMessage) -> EventStream<StreamEvent> {
    Box::pin(futures::stream::iter(vec![StreamEvent::terminal(message)]))
}

/// pi `AssistantMessageEventStream.result()` — the message the stream's terminal carries.
///
/// A stream that ends WITHOUT a terminal breaks the provider contract (exactly one terminal closes
/// every stream); it settles as an error naming that, never as a fabricated success.
pub async fn settle(
    mut stream: EventStream<StreamEvent>,
    provider: &str,
    model_id: &str,
) -> AssistantMessage {
    while let Some(event) = stream.next().await {
        if let Some(message) = event.terminal_message() {
            return Arc::unwrap_or_clone(Arc::clone(message));
        }
    }
    AssistantMessage::errored(
        ProviderId::from(provider),
        model_id,
        None,
        StopReason::Error,
        "the model stream ended without a terminal event",
    )
}

/// Aborts a task and fires its call's [`CancelToken`] when dropped — the guard that ties a spawned
/// provider request to its owner. Both, because dropping a provider stream is not always enough to
/// stop it: a provider that produces from a background task of its own stops at its next send,
/// whereas the token is pi's `signal`, which every provider observes.
struct AbortOnDrop(tokio::task::AbortHandle, CancelToken);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
        self.1.cancel();
    }
}

/// Start `call` on `provider` — pi's `provider.stream` / `provider.streamSimple` dispatch at the end
/// of `ModelRuntime.stream` / `.streamSimple` (`core/model-runtime.ts:703`, `:739` @v1.0.4), with
/// `call`'s options lowered for its verb and its [`CancelToken`] as the request's `signal`.
#[must_use]
pub fn dispatch(
    provider: &dyn cyrup_provider::Provider,
    model: &cyrup_provider::Model,
    call: &ModelCall,
) -> EventStream<StreamEvent> {
    // Timed here as well as by any timer inside the provider (that one wins): a provider that does
    // not time its own responses — a faux, local or extension provider — still answers with
    // `durationMs`, as every pi `AssistantMessageEventStream` does (`utils/event-stream.ts` @v1.1.0).
    cyrup_provider::timing::timed(match call.verb {
        ModelCallVerb::Stream => provider.stream(
            model,
            &call.context,
            &call.options.stream_options(&call.cancel),
        ),
        ModelCallVerb::StreamSimple => provider.stream_simple(
            model,
            &call.context,
            &call.options.simple_options(&call.cancel),
        ),
    })
}

/// pi `complete`, for a caller that must not hang: [`settle`] runs on its OWN task, so the provider
/// work never runs inside the caller's frame, and
///
/// * if the caller's future is dropped (its call was cancelled, its guest trapped), the task is
///   aborted and `cancel` fired, which ends the provider request;
/// * if `timeout` elapses first, the same happens and the call settles as an error;
/// * if the task panics, the call settles as an error naming that — the panic never reaches the
///   caller.
///
/// Every outcome is a settled [`AssistantMessage`], as pi's `complete` never rejects for a
/// request-time failure.
pub async fn complete_bounded(
    stream: EventStream<StreamEvent>,
    cancel: CancelToken,
    provider: &str,
    model_id: &str,
    timeout: Duration,
) -> AssistantMessage {
    let errored = |message: String| {
        AssistantMessage::errored(
            ProviderId::from(provider),
            model_id,
            None,
            StopReason::Error,
            message,
        )
    };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return errored("no async runtime is available to run the model call".into());
    };
    let (p, m) = (provider.to_string(), model_id.to_string());
    let task = runtime.spawn(async move { settle(stream, &p, &m).await });
    let _guard = AbortOnDrop(task.abort_handle(), cancel);
    match tokio::time::timeout(timeout, task).await {
        Ok(Ok(message)) => message,
        Ok(Err(join)) if join.is_panic() => errored("the model call panicked".into()),
        Ok(Err(_)) => errored("the model call was cancelled".into()),
        Err(_) => errored(format!("the model call did not settle within {timeout:?}")),
    }
}

/// One open stream in [`ModelStreams`].
struct Slot {
    /// The pump's buffer. A `tokio` mutex because a poll holds it across its bounded wait; a
    /// second, concurrent poll of the same handle is refused rather than queued.
    rx: tokio::sync::Mutex<tokio::sync::mpsc::Receiver<StreamEvent>>,
    /// When the guest last polled — read by the pump's abandonment reaper.
    last_poll: Arc<Mutex<Instant>>,
    /// Set by the pump when it gave up on an abandoned stream.
    abandoned: Arc<AtomicBool>,
    /// Set once a terminal event has been handed to the guest; from then on an empty, closed
    /// buffer is end-of-stream.
    terminal_delivered: AtomicBool,
    /// The pump task. Aborted on close, on table drop, and — from inside — on abandonment.
    pump: tokio::task::AbortHandle,
    /// The call's `signal`, fired with every abort.
    cancel: CancelToken,
    /// For the synthesized terminal of a pump that died without one.
    provider: String,
    model_id: String,
    /// The window this stream was opened with, for the abandonment error's text.
    abandon_after: Duration,
}

impl Slot {
    fn stop(&self) {
        self.pump.abort();
        self.cancel.cancel();
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The open model streams of ONE extension instance — the host half of `models.stream`,
/// `models.stream-simple`, `models.poll-stream` and `models.close-stream`.
///
/// Owned by the extension's [`crate::host::GuestState`]. Handles are local to the table, so one
/// extension can neither read nor close another's stream. Every stream is stopped when the table is
/// dropped, when the guest traps (`LiveExtension::fault`), and when its [`crate::ExtensionHost`] is
/// dropped — the last one explicitly, because a loaded guest is not freed with its host (see that
/// `Drop`), so waiting for this table's own drop would leave the requests to the abandonment window.
pub struct ModelStreams {
    next: AtomicU32,
    slots: Mutex<HashMap<u32, Arc<Slot>>>,
    poll_wait: Duration,
    /// Read when a stream OPENS, so [`Self::set_abandon_after`] governs the streams opened after it.
    abandon_after: Mutex<Duration>,
    max_open: usize,
}

impl Default for ModelStreams {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ModelStreams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelStreams")
            .field("open", &self.open_count())
            .finish_non_exhaustive()
    }
}

impl ModelStreams {
    /// A table with the production bounds.
    #[must_use]
    pub fn new() -> Self {
        Self::with_bounds(
            MODEL_STREAM_POLL_WAIT,
            MODEL_STREAM_ABANDON_AFTER,
            MAX_OPEN_MODEL_STREAMS,
        )
    }

    /// A table with explicit bounds, so a test can pin abandonment and the poll wait without
    /// waiting the production durations.
    #[must_use]
    pub fn with_bounds(poll_wait: Duration, abandon_after: Duration, max_open: usize) -> Self {
        Self {
            next: AtomicU32::new(1),
            slots: Mutex::new(HashMap::new()),
            poll_wait,
            abandon_after: Mutex::new(abandon_after),
            max_open,
        }
    }

    /// Change how long a stream may go unpolled before it is abandoned
    /// ([`MODEL_STREAM_ABANDON_AFTER`] by default), for the streams opened from now on — the knob
    /// an embedder (or a test that cannot wait two minutes) turns on a live extension's table.
    pub fn set_abandon_after(&self, abandon_after: Duration) {
        *self
            .abandon_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = abandon_after;
    }

    fn abandon_after(&self) -> Duration {
        *self
            .abandon_after
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<u32, Arc<Slot>>> {
        self.slots
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// How many streams are open (live, finished-but-unclosed, or abandoned-but-unclosed).
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.lock().len()
    }

    /// Take ownership of `stream` and answer its handle. The stream starts being drained at once.
    ///
    /// # Errors
    ///
    /// The extension already holds [`MAX_OPEN_MODEL_STREAMS`] streams, or there is no async runtime
    /// to drain it on. Either way `stream` is dropped and `cancel` fired, which ends its request.
    pub fn open(
        &self,
        stream: EventStream<StreamEvent>,
        cancel: CancelToken,
        provider: &str,
        model_id: &str,
    ) -> Result<u32, String> {
        let refuse = |message: String| {
            cancel.cancel();
            Err(message)
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return refuse("no async runtime is available to run the model stream".into());
        };
        let abandon_after = self.abandon_after();
        let mut slots = self.lock();
        if slots.len() >= self.max_open {
            return refuse(format!(
                "this extension already has {} model streams open; close one first",
                self.max_open
            ));
        }
        let (tx, rx) = tokio::sync::mpsc::channel(MODEL_STREAM_BUFFER);
        let last_poll = Arc::new(Mutex::new(Instant::now()));
        let abandoned = Arc::new(AtomicBool::new(false));
        let pump = runtime.spawn(pump(
            stream,
            tx,
            Arc::clone(&last_poll),
            Arc::clone(&abandoned),
            cancel.clone(),
            abandon_after,
        ));
        let handle = self.next.fetch_add(1, Ordering::Relaxed);
        slots.insert(
            handle,
            Arc::new(Slot {
                rx: tokio::sync::Mutex::new(rx),
                last_poll,
                abandoned,
                terminal_delivered: AtomicBool::new(false),
                pump: pump.abort_handle(),
                cancel,
                provider: provider.to_string(),
                model_id: model_id.to_string(),
                abandon_after,
            }),
        );
        Ok(handle)
    }

    /// The events that have arrived since the last poll.
    ///
    /// * `Ok(Some(events))` — the stream is live; `events` may be EMPTY when nothing arrived within
    ///   the poll wait. The batch that carries the terminal (`done` / `error`) is the last
    ///   non-empty one.
    /// * `Ok(None)` — end of stream: the terminal was delivered by an earlier poll.
    ///
    /// # Errors
    ///
    /// No open stream has `handle` (never opened, or closed); the stream was abandoned; or another
    /// poll of the same handle is in flight.
    pub async fn poll(&self, handle: u32) -> Result<Option<Vec<StreamEvent>>, String> {
        let slot = self
            .lock()
            .get(&handle)
            .cloned()
            .ok_or_else(|| format!("no open model stream for handle {handle}"))?;
        *slot
            .last_poll
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Instant::now();
        let mut rx = slot
            .rx
            .try_lock()
            .map_err(|_| format!("model stream {handle} is already being polled"))?;
        let first = match tokio::time::timeout(self.poll_wait, rx.recv()).await {
            Err(_) => return Ok(Some(Vec::new())),
            Ok(first) => first,
        };
        let Some(first) = first else {
            return self.closed_buffer(handle, &slot);
        };
        let mut events = vec![first];
        while events.len() < MAX_EVENTS_PER_POLL {
            match rx.try_recv() {
                Ok(event) => events.push(event),
                Err(_) => break,
            }
        }
        if events.iter().any(|e| e.terminal_message().is_some()) {
            slot.terminal_delivered.store(true, Ordering::SeqCst);
        }
        Ok(Some(events))
    }

    /// A poll found the pump's buffer closed and empty.
    fn closed_buffer(&self, handle: u32, slot: &Slot) -> Result<Option<Vec<StreamEvent>>, String> {
        if slot.terminal_delivered.load(Ordering::SeqCst) {
            return Ok(None);
        }
        if slot.abandoned.load(Ordering::SeqCst) {
            return Err(format!(
                "model stream {handle} was abandoned: it was not polled for {:?}, so its request \
                 was cancelled",
                slot.abandon_after
            ));
        }
        // The pump ended without a terminal: it panicked, or the provider broke the one-terminal
        // contract. Hand the guest the one terminal it is owed, then end-of-stream.
        slot.terminal_delivered.store(true, Ordering::SeqCst);
        let message = AssistantMessage::errored(
            ProviderId::from(slot.provider.as_str()),
            slot.model_id.as_str(),
            None,
            StopReason::Error,
            "the model stream ended without a terminal event",
        );
        Ok(Some(vec![StreamEvent::terminal(message)]))
    }

    /// Close a stream: abort its pump (cancelling the provider request if it is still running) and
    /// forget the handle. An unknown handle is a no-op, as `http-client.close-stream`'s is.
    pub fn close(&self, handle: u32) {
        let slot = self.lock().remove(&handle);
        if let Some(slot) = slot {
            slot.stop();
        }
    }

    /// Close every stream. Called when the instance can no longer poll any of them: it is being
    /// dropped, or it trapped — a trap poisons a component store for good (wasmtime refuses to
    /// enter it again), so a stream it opened would otherwise run, and bill, until abandoned.
    pub fn close_all(&self) {
        let slots: Vec<_> = self.lock().drain().map(|(_, s)| s).collect();
        for slot in slots {
            slot.stop();
        }
    }
}

impl Drop for ModelStreams {
    fn drop(&mut self) {
        self.close_all();
    }
}

/// Resolves once `last_poll` is older than `abandon_after`.
async fn abandonment(last_poll: &Mutex<Instant>, abandon_after: Duration) {
    loop {
        let since = last_poll
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .elapsed();
        if since >= abandon_after {
            return;
        }
        tokio::time::sleep(abandon_after.saturating_sub(since)).await;
    }
}

/// Drain the provider stream into the guest's buffer until its terminal, the guest closing the
/// buffer, or the guest abandoning the stream. Returning drops `stream`, which cancels its request.
async fn pump(
    mut stream: EventStream<StreamEvent>,
    tx: tokio::sync::mpsc::Sender<StreamEvent>,
    last_poll: Arc<Mutex<Instant>>,
    abandoned: Arc<AtomicBool>,
    cancel: CancelToken,
    abandon_after: Duration,
) {
    loop {
        let event = tokio::select! {
            event = stream.next() => event,
            () = abandonment(&last_poll, abandon_after) => {
                abandoned.store(true, Ordering::SeqCst);
                cancel.cancel();
                return;
            }
        };
        let Some(event) = event else { return };
        let terminal = event.terminal_message().is_some();
        tokio::select! {
            sent = tx.send(event) => if sent.is_err() { return },
            () = abandonment(&last_poll, abandon_after) => {
                abandoned.store(true, Ordering::SeqCst);
                cancel.cancel();
                return;
            }
        }
        if terminal {
            return;
        }
    }
}

/// The JSON a guest receives for one event: pi's `AssistantMessageEvent`, byte-for-byte the shape
/// `StreamEvent`'s serde already produces for the json/rpc wire.
#[must_use]
pub fn event_json(event: &StreamEvent) -> String {
    serde_json::to_string(event).unwrap_or_else(|e| {
        serde_json::json!({ "type": "error", "reason": "error", "error": { "errorMessage": e.to_string() } })
            .to_string()
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
    use serde_json::json;

    /// Set when the stream that owns it is dropped — the observable "the provider request is gone".
    struct DropFlag(Arc<AtomicBool>);

    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A provider stream that never yields — a hung provider — and reports its own drop.
    fn hung_stream() -> (EventStream<StreamEvent>, Arc<AtomicBool>) {
        let dropped = Arc::new(AtomicBool::new(false));
        let flag = DropFlag(Arc::clone(&dropped));
        let stream = futures::stream::pending::<StreamEvent>().map(move |event| {
            let _owned = &flag;
            event
        });
        (Box::pin(stream), dropped)
    }

    /// A provider stream whose first poll panics — a provider bug.
    fn panicking_stream() -> EventStream<StreamEvent> {
        Box::pin(futures::stream::poll_fn(
            |_| -> std::task::Poll<Option<StreamEvent>> { panic!("provider bug") },
        ))
    }

    fn faux_stream(text: &str) -> (EventStream<StreamEvent>, CancelToken) {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_text(text)],
            StopReason::Stop,
        )]);
        let call = ModelCall::from_json(
            ModelCallVerb::Stream,
            &json!({"provider": "faux", "id": faux.model().id.as_str()}).to_string(),
            r#"{"messages":[{"role":"user","content":"hi","timestamp":1}]}"#,
            "",
        )
        .unwrap();
        let cancel = call.cancel.clone();
        (dispatch(&faux, &faux.model().clone(), &call), cancel)
    }

    /// Wait (briefly) for a flag another task sets.
    async fn eventually(flag: &AtomicBool) -> bool {
        for _ in 0..200 {
            if flag.load(Ordering::SeqCst) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        false
    }

    fn text_of(message: &AssistantMessage) -> String {
        message
            .content
            .iter()
            .filter_map(|block| match block {
                cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
                _ => None,
            })
            .collect()
    }

    fn quick_table() -> ModelStreams {
        ModelStreams::with_bounds(Duration::from_millis(30), Duration::from_secs(60), 2)
    }

    #[test]
    fn a_call_reads_the_model_address_and_nothing_else_from_the_model_object() {
        let call = ModelCall::from_json(
            ModelCallVerb::StreamSimple,
            &json!({"provider": "p", "id": "m", "baseUrl": "https://attacker.invalid", "api": "x"})
                .to_string(),
            r#"{"systemPrompt":"s","messages":[{"role":"user","content":"hi","timestamp":1}]}"#,
            r#"{"maxTokens":64,"reasoning":"off","cacheRetention":"none","apiKey":"k","signal":{}}"#,
        )
        .unwrap();
        assert_eq!((call.provider.as_str(), call.model_id.as_str()), ("p", "m"));
        assert_eq!(call.context.system_prompt.as_deref(), Some("s"));
        assert_eq!(call.context.messages.len(), 1);
        assert_eq!(call.options.max_tokens, Some(64));
        assert_eq!(call.options.cache_retention, Some(CacheRetention::None));
        // `reasoning: "off"` is pi's "none" for streamSimple.
        assert_eq!(call.options.simple_options(&call.cancel).reasoning, None);
        // The credential plane does not cross: there is no field for `apiKey` to land in.
        assert!(call.options.stream_options(&call.cancel).api_key.is_none());
    }

    #[test]
    fn a_malformed_argument_is_an_err_not_a_settled_message() {
        let ctx = r#"{"messages":[]}"#;
        assert!(ModelCall::from_json(ModelCallVerb::Stream, r#"{"id":"m"}"#, ctx, "").is_err());
        assert!(
            ModelCall::from_json(
                ModelCallVerb::Stream,
                r#"{"provider":"p","id":""}"#,
                ctx,
                ""
            )
            .is_err()
        );
        assert!(
            ModelCall::from_json(
                ModelCallVerb::Stream,
                r#"{"provider":"p","id":"m"}"#,
                "[]",
                ""
            )
            .is_err()
        );
        assert!(
            ModelCall::from_json(
                ModelCallVerb::Stream,
                r#"{"provider":"p","id":"m"}"#,
                ctx,
                r#"{"cacheRetention":"forever"}"#
            )
            .is_err()
        );
        assert!(
            ModelCall::from_json(
                ModelCallVerb::Stream,
                r#"{"provider":"p","id":"m"}"#,
                ctx,
                "null"
            )
            .is_ok()
        );
    }

    #[tokio::test]
    async fn settle_answers_the_message_the_terminal_carries() {
        let (stream, _) = faux_stream("the faux reply");
        let message = settle(stream, "faux", "faux-1").await;
        assert_eq!(message.stop_reason, StopReason::Stop);
        assert_eq!(text_of(&message), "the faux reply");
    }

    #[tokio::test]
    async fn settle_turns_a_stream_without_a_terminal_into_an_error() {
        let message = settle(Box::pin(futures::stream::empty()), "p", "m").await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(
            message
                .error_message
                .as_deref()
                .unwrap()
                .contains("without a terminal")
        );
    }

    #[tokio::test]
    async fn a_hung_complete_is_cut_off_and_its_provider_request_dropped() {
        let (stream, dropped) = hung_stream();
        let cancel = CancelToken::new();
        let message =
            complete_bounded(stream, cancel.clone(), "p", "m", Duration::from_millis(40)).await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(
            message
                .error_message
                .as_deref()
                .unwrap()
                .contains("did not settle")
        );
        assert!(
            eventually(&dropped).await,
            "the hung provider stream was not dropped"
        );
        assert!(cancel.is_cancelled(), "the call's signal was not fired");
    }

    #[tokio::test]
    async fn a_complete_whose_caller_goes_away_drops_its_provider_request() {
        let (stream, dropped) = hung_stream();
        let cancel = CancelToken::new();
        let call = complete_bounded(stream, cancel.clone(), "p", "m", Duration::from_secs(600));
        // The caller gives up — a cancelled tool call, a trapped guest — and drops the future.
        assert!(
            tokio::time::timeout(Duration::from_millis(20), call)
                .await
                .is_err()
        );
        assert!(
            eventually(&dropped).await,
            "the provider stream outlived its caller"
        );
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn a_panicking_provider_settles_complete_as_an_error() {
        let message = complete_bounded(
            panicking_stream(),
            CancelToken::new(),
            "p",
            "m",
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(message.stop_reason, StopReason::Error);
        assert!(
            message
                .error_message
                .as_deref()
                .unwrap()
                .contains("panicked")
        );
    }

    #[tokio::test]
    async fn a_stream_drains_chunk_by_chunk_to_one_terminal_then_end() {
        let table = quick_table();
        let text = "a reply long enough to be cut into several faux deltas";
        let (stream, cancel) = faux_stream(text);
        let handle = table.open(stream, cancel, "faux", "faux-1").unwrap();
        let (mut deltas, mut terminals, mut joined) = (0, 0, String::new());
        loop {
            let Some(batch) = table.poll(handle).await.unwrap() else {
                break;
            };
            for event in batch {
                if let StreamEvent::TextDelta { delta, .. } = &event {
                    deltas += 1;
                    joined.push_str(delta);
                }
                if event.terminal_message().is_some() {
                    terminals += 1;
                }
            }
        }
        assert!(deltas > 1, "expected several deltas, got {deltas}");
        assert_eq!(joined, text);
        assert_eq!(terminals, 1);
        // End-of-stream is sticky until the guest closes the handle.
        assert_eq!(table.poll(handle).await.unwrap(), None);
        table.close(handle);
        assert!(table.poll(handle).await.is_err());
    }

    #[tokio::test]
    async fn a_silent_stream_polls_as_an_empty_batch_not_as_its_end() {
        let table = quick_table();
        let (stream, _dropped) = hung_stream();
        let handle = table.open(stream, CancelToken::new(), "p", "m").unwrap();
        assert_eq!(table.poll(handle).await.unwrap(), Some(Vec::new()));
    }

    #[tokio::test]
    async fn close_drops_the_provider_request_and_fires_the_signal() {
        let table = quick_table();
        let (stream, dropped) = hung_stream();
        let cancel = CancelToken::new();
        let handle = table.open(stream, cancel.clone(), "p", "m").unwrap();
        table.close(handle);
        assert!(eventually(&dropped).await);
        assert!(cancel.is_cancelled());
        assert_eq!(table.open_count(), 0);
    }

    #[tokio::test]
    async fn dropping_the_table_drops_every_provider_request() {
        let table = quick_table();
        let (a, a_dropped) = hung_stream();
        let (b, b_dropped) = hung_stream();
        table.open(a, CancelToken::new(), "p", "m").unwrap();
        table.open(b, CancelToken::new(), "p", "m").unwrap();
        drop(table);
        assert!(eventually(&a_dropped).await && eventually(&b_dropped).await);
    }

    #[tokio::test]
    async fn close_all_stops_every_stream_a_trapped_instance_left_open() {
        let table = quick_table();
        let (a, a_dropped) = hung_stream();
        let handle = table.open(a, CancelToken::new(), "p", "m").unwrap();
        table.close_all();
        assert!(eventually(&a_dropped).await);
        assert!(table.poll(handle).await.is_err());
    }

    #[tokio::test]
    async fn an_unpolled_stream_is_abandoned_and_its_request_dropped() {
        let table =
            ModelStreams::with_bounds(Duration::from_millis(30), Duration::from_secs(60), 2);
        table.set_abandon_after(Duration::from_millis(60));
        let (stream, dropped) = hung_stream();
        let cancel = CancelToken::new();
        let handle = table.open(stream, cancel.clone(), "p", "m").unwrap();
        // Nobody polls.
        assert!(
            eventually(&dropped).await,
            "an abandoned stream kept its request"
        );
        assert!(cancel.is_cancelled());
        let err = table.poll(handle).await.unwrap_err();
        assert!(err.contains("abandoned"), "{err}");
    }

    #[tokio::test]
    async fn a_stream_that_keeps_being_polled_is_not_abandoned() {
        let table =
            ModelStreams::with_bounds(Duration::from_millis(20), Duration::from_secs(60), 2);
        table.set_abandon_after(Duration::from_millis(80));
        let (stream, dropped) = hung_stream();
        let handle = table.open(stream, CancelToken::new(), "p", "m").unwrap();
        for _ in 0..10 {
            assert_eq!(table.poll(handle).await.unwrap(), Some(Vec::new()));
        }
        assert!(!dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_pump_that_panics_hands_the_guest_one_error_terminal_then_end() {
        let table = quick_table();
        let handle = table
            .open(panicking_stream(), CancelToken::new(), "p", "m")
            .unwrap();
        let mut terminals = Vec::new();
        loop {
            match table.poll(handle).await.unwrap() {
                None => break,
                Some(batch) => terminals.extend(batch),
            }
        }
        assert_eq!(terminals.len(), 1);
        let message = terminals[0].terminal_message().unwrap();
        assert_eq!(message.stop_reason, StopReason::Error);
    }

    #[tokio::test]
    async fn the_table_refuses_a_stream_past_its_cap_and_cancels_it() {
        let table = quick_table();
        table
            .open(hung_stream().0, CancelToken::new(), "p", "m")
            .unwrap();
        table
            .open(hung_stream().0, CancelToken::new(), "p", "m")
            .unwrap();
        let (third, dropped) = hung_stream();
        let cancel = CancelToken::new();
        assert!(table.open(third, cancel.clone(), "p", "m").is_err());
        assert!(dropped.load(Ordering::SeqCst));
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn an_event_crosses_as_pis_assistant_message_event_json() {
        let message =
            AssistantMessage::errored(ProviderId::from("p"), "m", None, StopReason::Error, "boom");
        let json: Value =
            serde_json::from_str(&event_json(&StreamEvent::terminal(message))).unwrap();
        assert_eq!(json["type"], "error");
        assert_eq!(json["reason"], "error");
        assert_eq!(json["error"]["errorMessage"], "boom");
    }
}
