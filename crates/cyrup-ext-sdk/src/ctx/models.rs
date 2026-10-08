//! The `models` WIT import: the model registry view. Neither `set_model` nor `set_thinking_level`
//! is tier-gated (EXT-074 / GAP-11), so both live here: the host takes the ungated `guest_of` for
//! each and QUEUES the op, which is applied at the store-free turn-boundary drain
//! (`cyrup-ext/src/host/live.rs:565-577` for `set_model`, `:584-601` for `set_thinking_level`), so
//! an event-tier call takes effect on the SUBSEQUENT turn instead of being dropped or rejected.
//! This is the same position as [`Ctx::models`](super::Ctx::models) (`src/ctx/base.rs:58-62`) and
//! the `set-model` block in `wit/world.wit:778-786`. [`CommandCtx::set_model`] remains only as a
//! delegating wrapper, for source compatibility.
//!
//! [`CommandCtx::set_model`]: crate::CommandCtx::set_model

use serde::Serialize;
use serde_json::Value;

/// The model registry view (Pi `ctx.modelRegistry: ModelRegistry`, `extensions/types.ts:319`
/// @v0.83.0, plus `ctx.scopedModels` `:326`, `getContextUsage()` `:341`, `pi.setModel(model)`
/// `:1336` and `pi.setThinkingLevel(level)` `:1342`).
///
/// EXT-036: this cited `types.ts:1273-1279`, which is `registerMessageRenderer`/
/// `registerEntryRenderer` — a different surface entirely, not an off-by-N.
#[derive(Clone, Copy, Debug, Default)]
pub struct Models;

impl Models {
    /// The models scoped to this session — pi `ctx.scopedModels: readonly ScopedModel[]`
    /// (`extensions/types.ts:326` @v0.83.0): "Models scoped to this session (resolved from
    /// `--models` / `enabledModels` settings against the available catalogue). Same set the
    /// `/scoped-models` command shows. Empty when no scoping is configured (all available models
    /// are usable)." EXT-045 — without it a guest could not offer a model picker restricted to the
    /// session's scoped set.
    ///
    /// [`Value::Null`] — NOT the empty array that means "no scoping is configured" — when the host
    /// sent JSON this SDK could not parse (`super::parse_json`'s fallback). The two are different
    /// answers and a caller matching on `as_array()` must not treat them alike. The WIT import
    /// promises a JSON array (`wit/world.wit:776`).
    pub fn scoped(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::scoped_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }

    /// Every model in the registry, as the WIT `models.list-models` import's "json array of model
    /// refs" (`wit/world.wit:771`).
    ///
    /// [`Value::Null`] — NOT an empty array — when the host sent JSON this SDK could not parse
    /// (`super::parse_json`'s fallback), so a caller that treats a non-array as "no models" cannot
    /// tell the two apart. On the host (non-`wasm32`) target there is no host to ask and this is
    /// always an empty array.
    pub fn list(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::list_models());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Array(vec![])
    }
    /// One model by `(provider, id)` — pi `ctx.modelRegistry.find(provider, id)`, which
    /// `docs/virtual-models.md` tells a virtual-model router to use to name the physical model it
    /// is routing to.
    ///
    /// `None` when no row matches, when the host sent JSON this SDK could not parse, or on the host
    /// (non-`wasm32`) target, where there is no host to ask. Returned whole, because that is what a
    /// [`ModelRoute`](crate::ModelRoute) carries: the host re-resolves the address against its own
    /// catalog, but the full object is the answer that decodes on every path.
    pub fn find(&self, provider: &str, id: &str) -> Option<Value> {
        self.list()
            .as_array()?
            .iter()
            .find(|m| {
                m.get("provider").and_then(Value::as_str) == Some(provider)
                    && m.get("id").and_then(Value::as_str) == Some(id)
            })
            .cloned()
    }

    /// The model currently selected for this session (WIT `models.current`, `wit/world.wit:777`);
    /// `None` when the host has none to report, and always `None` on the host (non-`wasm32`)
    /// target, where there is no host to ask.
    pub fn current(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::current();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }
    /// The session's context-window usage, through the WIT `models.context-usage` import
    /// (`wit/world.wit:787`); see the [`Models`] type doc for the Pi surface this mirrors.
    ///
    /// [`Value::Null`] means ONLY "the host sent JSON this SDK could not parse"
    /// (`super::parse_json`'s fallback) — the host's own no-usage answer is the empty OBJECT `{}`
    /// (`cyrup-ext/src/host/live.rs:579`, and the `HostServices::context_usage` default at
    /// `cyrup-ext/src/host/services.rs:454-456`), which parses fine. On the host (non-`wasm32`)
    /// target there is no host to ask and this is always [`Value::Null`].
    pub fn context_usage(&self) -> Value {
        #[cfg(target_arch = "wasm32")]
        {
            return super::parse_json(crate::guest::bindings::cyrup::ext::models::context_usage());
        }
        #[cfg(not(target_arch = "wasm32"))]
        Value::Null
    }
    /// The session's current thinking level (WIT `models.thinking-level`, `wit/world.wit:788`) —
    /// the value [`Self::set_thinking_level`] writes; `None` when the host has none to report, and
    /// always `None` on the host (non-`wasm32`) target.
    pub fn thinking_level(&self) -> Option<String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::thinking_level();
        }
        #[cfg(not(target_arch = "wasm32"))]
        None
    }

    /// Set the thinking level (Pi `setThinkingLevel(level)`, `types.ts:1342` @v0.83.0; EXT-036
    /// corrected `:1288`; sdk gap #25 / GAP-11).
    ///
    /// Pi allows `setThinkingLevel` from ANY handler (factory-tier `pi.*`, `loader.ts:369-372` /
    /// `runner.ts:336`, no tier gate) and it takes effect. cyrup now matches this: the call is QUEUED
    /// as a control op and applied at the store-free turn-boundary drain
    /// (`AgentSession::apply_pending_control`), so its `thinking_level_select` re-emit
    /// (`agent-session.ts:1560-1567`) runs as a fresh top-level guest call after the event hook's wasm
    /// store guard is released — never a re-entry into the suspended single-instance store (the
    /// R-08-008 deadlock the old command-only gate guarded against is dissolved by deferral). So this
    /// returns `Ok(())` and the new level takes effect on the SUBSEQUENT turn, whether called from a
    /// command handler or an event handler.
    pub fn set_thinking_level(&self, level: &str) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::set_thinking_level(level);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = level;
            Ok(())
        }
    }

    /// Set the model (WIT `models.set-model`, `wit/world.wit:786`; EXT-074 / GAP-11).
    ///
    /// Callable from ANY tier, exactly like [`Self::set_thinking_level`]: the host's `set_model`
    /// (`cyrup-ext/src/host/live.rs:565-577`) opens with the ungated `guest_of` and QUEUES a
    /// `ControlOp::SetModel` applied at the store-free turn-boundary drain
    /// (`AgentSession::apply_pending_control`), so an event-tier call takes effect on the
    /// SUBSEQUENT turn rather than being dropped. The WIT import returns void, so the `Ok(())`
    /// here says only that the encoded ref reached the host, never that the model was accepted —
    /// the guest observes the EFFECT. [`CommandCtx::set_model`] delegates here.
    ///
    /// [`CommandCtx::set_model`]: crate::CommandCtx::set_model
    pub fn set_model(&self, model: impl Serialize) -> Result<(), String> {
        let m = serde_json::to_string(&model).unwrap_or_else(|_| "null".into());
        #[cfg(target_arch = "wasm32")]
        {
            crate::guest::bindings::cyrup::ext::models::set_model(&m);
            return Ok(());
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = m;
            Ok(())
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Model CALLS (EXT-086) — pi `ctx.modelRegistry.complete()` / `stream()` / `streamSimple()`.
// ---------------------------------------------------------------------------------------------

impl Models {
    /// pi `ctx.modelRegistry.complete(model, context, options)` (`core/model-registry.ts` @v1.0.4):
    /// one model call through the session's configured providers, with the user's credentials,
    /// settled to pi's `AssistantMessage` JSON.
    ///
    /// * `model` names the model by its `provider` and `id` keys — a row from [`Self::list`] or
    ///   [`Self::find`] works, and so does `json!({"provider": "…", "id": "…"})`. Only the address is
    ///   read: the host resolves it against its own catalog, so a `baseUrl` in it is ignored.
    /// * `context` is pi's `Context`: `{"systemPrompt"?, "messages": [...], "tools"?}`, each message
    ///   in pi's shape (`{"role": "user", "content": "…", "timestamp": 0}`).
    /// * `options` is pi's options bag (`maxTokens`, `temperature`, `sessionId`, `cacheRetention`,
    ///   `headers`, `metadata`, `samplingParams`, `timeoutMs`, `maxRetries`, `maxRetryDelayMs`);
    ///   `Value::Null` for none. Credentials and callbacks do not cross.
    ///
    /// Like pi's, a request-time failure is NOT an `Err`: an unknown model, a provider without
    /// credentials, a virtual model (which `complete` never routes) or a transport failure all
    /// come back as `Ok` with `"stopReason": "error"` and an `"errorMessage"`. `Err` means the call
    /// could not be made — the manifest does not declare `capabilities.modelCalls`, or an argument
    /// is malformed. [`message_text`] reads the reply's text.
    ///
    /// Suspends this extension for the length of the call (every handler of it waits), up to the
    /// host's ceiling. Prefer [`Self::stream`] for anything long.
    pub fn complete(
        &self,
        model: &Value,
        context: &Value,
        options: &Value,
    ) -> Result<Value, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let reply = crate::guest::bindings::cyrup::ext::models::complete(
                &model.to_string(),
                &context.to_string(),
                &options.to_string(),
            )?;
            return Ok(super::parse_json(reply));
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (model, context, options);
            Err(NO_HOST.into())
        }
    }

    /// pi `ctx.modelRegistry.stream(model, context, options)`: the same call as [`Self::complete`],
    /// streamed. Arguments as there. `stream` never routes a virtual model (its stream ends with
    /// pi's "must be routed before streaming" error); [`Self::stream_simple`] does.
    pub fn stream(
        &self,
        model: &Value,
        context: &Value,
        options: &Value,
    ) -> Result<ModelStream, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::stream(
                &model.to_string(),
                &context.to_string(),
                &options.to_string(),
            )
            .map(ModelStream::new);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (model, context, options);
            Err(NO_HOST.into())
        }
    }

    /// pi `ctx.modelRegistry.streamSimple(model, context, options)`: [`Self::stream`] with the
    /// provider-neutral options — `options.reasoning` is a thinking level (`"minimal"` …
    /// `"max"`, or `"off"`) — and the one verb that routes a VIRTUAL model, exactly as upstream:
    /// the router is asked with reason `"direct"`, and `maxTokens` is capped to the model it picks.
    pub fn stream_simple(
        &self,
        model: &Value,
        context: &Value,
        options: &Value,
    ) -> Result<ModelStream, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::models::stream_simple(
                &model.to_string(),
                &context.to_string(),
                &options.to_string(),
            )
            .map(ModelStream::new);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (model, context, options);
            Err(NO_HOST.into())
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
const NO_HOST: &str = "model calls are unavailable on the host target";

/// The text of a settled `AssistantMessage` (what [`Models::complete`] and
/// [`ModelStream::result`] return): its `text` blocks, concatenated in order. Empty for a message
/// with none — check `stopReason` / `errorMessage` to tell a failure from an empty reply.
pub fn message_text(message: &Value) -> String {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect()
}

/// A model stream the HOST holds open for this extension — pi's `AssistantMessageEventStream`,
/// drained by polling because a guest cannot hold a live host stream (the same request/poll bridge
/// as [`crate::HttpStreamResponse`]).
///
/// Each event is pi's `AssistantMessageEvent` JSON: `start`, `text_start` / `text_delta` /
/// `text_end`, the `thinking_*` and `toolcall_*` triples, then exactly ONE terminal — `done`
/// (`{"type": "done", "reason", "message"}`) or `error` (`{"type": "error", "reason", "error"}`) —
/// after which the stream is at its end.
///
/// Iterate it (`for event in stream { … }`), call [`Self::next_event`], or take whole batches with
/// [`Self::poll`]. Dropping it closes it, which cancels the provider request if it is still
/// running; so does [`Self::close`]. A stream the extension stops polling is closed by the host
/// after a while, and every stream is closed when the extension is unloaded.
#[derive(Debug)]
pub struct ModelStream {
    handle: u32,
    buffered: std::collections::VecDeque<Value>,
    ended: bool,
    closed: bool,
}

impl ModelStream {
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    fn new(handle: u32) -> Self {
        Self {
            handle,
            buffered: std::collections::VecDeque::new(),
            ended: false,
            closed: false,
        }
    }

    /// The host's handle for this stream.
    pub fn handle(&self) -> u32 {
        self.handle
    }

    /// One round trip to the host (`models.poll-stream`): every event that arrived since the last
    /// one, in order. `Ok(Some(empty))` means nothing arrived within the host's short wait — the
    /// stream is still live, poll again. `Ok(None)` is the end: the terminal was in an earlier
    /// batch. Events already buffered by [`Self::next_event`] are returned first.
    pub fn poll(&mut self) -> Result<Option<Vec<Value>>, String> {
        if !self.buffered.is_empty() {
            return Ok(Some(self.buffered.drain(..).collect()));
        }
        if self.ended {
            return Ok(None);
        }
        let batch = self.poll_host()?;
        if batch.is_none() {
            self.ended = true;
        }
        Ok(batch)
    }

    /// The next event, polling the host as often as it takes; `Ok(None)` after the terminal.
    pub fn next_event(&mut self) -> Result<Option<Value>, String> {
        loop {
            if let Some(event) = self.buffered.pop_front() {
                return Ok(Some(event));
            }
            if self.ended {
                return Ok(None);
            }
            match self.poll_host()? {
                Some(batch) => self.buffered.extend(batch),
                None => self.ended = true,
            }
        }
    }

    /// pi `AssistantMessageEventStream.result()`: drain to the terminal and return the message it
    /// carries (`done`'s `message`, `error`'s `error`).
    pub fn result(mut self) -> Result<Value, String> {
        while let Some(event) = self.next_event()? {
            match event.get("type").and_then(Value::as_str) {
                Some("done") => return Ok(event.get("message").cloned().unwrap_or(Value::Null)),
                Some("error") => return Ok(event.get("error").cloned().unwrap_or(Value::Null)),
                _ => {}
            }
        }
        Err("the model stream ended without a terminal event".into())
    }

    /// Close the stream now (`models.close-stream`), cancelling the provider request if it is still
    /// running. Dropping the stream does the same.
    pub fn close(mut self) {
        self.close_host();
    }

    fn poll_host(&mut self) -> Result<Option<Vec<Value>>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            let batch = crate::guest::bindings::cyrup::ext::models::poll_stream(self.handle)?;
            return Ok(batch.map(|events| events.into_iter().map(super::parse_json).collect()));
        }
        #[cfg(not(target_arch = "wasm32"))]
        Err(NO_HOST.into())
    }

    fn close_host(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        #[cfg(target_arch = "wasm32")]
        crate::guest::bindings::cyrup::ext::models::close_stream(self.handle);
    }
}

impl Drop for ModelStream {
    fn drop(&mut self) {
        self.close_host();
    }
}

impl Iterator for ModelStream {
    type Item = Result<Value, String>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_event() {
            Ok(Some(event)) => Some(Ok(event)),
            Ok(None) => None,
            Err(e) => {
                // An error ends iteration: yield it once, then stop.
                self.ended = true;
                Some(Err(e))
            }
        }
    }
}
