//! How a generation settled — the [`StopReason`] enum (func-01 §9).

/// How a generation settled — or, for [`StopReason::Pending`], that it has **not settled yet**
/// (func-01 §9; Pi `StopReason`, `ai/src/types.ts:391`, read in full:
/// `"pending" | "stop" | "length" | "toolUse" | "error" | "aborted"`).
///
/// # Provenance: this is a PORT-FORWARD, not a bug fix
///
/// `"pending"` reached the `partial` snapshots in Pi commit `f9a49869` — *"feat(ai): expose pending
/// stop reason while streaming"* (#7151, 2026-07-27), first released in **v0.83.0**. That postdates
/// cyrup's port baseline (HEAD 2026-07-10), so the absence of this variant was UNPORTED UPSTREAM
/// WORK, not a defect cyrup introduced. Before `f9a49869`, Pi's faux partial seed was
/// `{...message, content: []}` — it inherited the scripted message's settled reason — which is why
/// the real-Pi captures under `cyrup-test-support/fixtures/pi/*.pi-captured.events.jsonl` show
/// `"stop"` on their partials. Those fixtures are annotated; do not use them to argue this variant
/// away.
///
/// The truncated-stream half of the same gap-analysis id (PROV-010 / AGENT-014 / DRIFT-012) was a
/// genuine defect and is fixed separately in `cyrup_provider::StreamEvent::end_of_stream`.
///
/// # `Pending` is the in-flight sentinel, never an outcome
///
/// `AssistantMessage.stop_reason` is a required non-`Option` field (Pi types.ts:386-397), so a
/// message that is still being streamed needs a value. Pi spells that value `"pending"`: every
/// stream function seeds `output.stopReason = "pending"` and attaches that same mutable `output` to
/// each non-terminal event as `partial` (`anthropic-messages.ts:509`, `google-generative-ai.ts:73`,
/// `openai-responses.ts:124`, `openai-completions.ts:218`, `mistral-conversations.ts:153`,
/// `faux.ts:316`; `agent/src/proxy.ts:121-137` seeds the client-rebuilt partial the same way).
/// Pi then makes it unobservable past the stream by THROWING if it survives to end of input —
/// `if (output.stopReason === "pending") throw new Error("… stream ended without a stop reason")`
/// (`anthropic-messages.ts:751-753`, `google-generative-ai.ts:266-268`,
/// `openai-responses.ts:170-172`, `openai-completions.ts:580-582`,
/// `mistral-conversations.ts:88-90`, `faux.ts:393-395`) — and the catch that receives that throw
/// sets `output.stopReason = "error"` before pushing the terminal event
/// (`anthropic-messages.ts:765-768`).
///
/// cyrup enforces the identical invariant **structurally**, in exactly two places, so no future
/// converter can forget it:
///
/// - `cyrup_provider::StreamEvent::end_of_stream` — the single end-of-input seam every wire-API
///   decoder funnels through; a `None`/`Pending` reason becomes the `error` terminal.
/// - `cyrup_provider::StreamEvent::terminal` — rewrites a `Pending` stop reason to
///   [`StopReason::Error`] and routes to the `error` terminal, mirroring Pi's catch. Backed by
///   `DoneReason::try_from(StopReason)`, which returns `Err(ErrorReason::Error)` for `Pending`, so
///   `Pending` cannot appear on a `done` event even by construction.
///
/// Consequently `Pending` is reachable in a `partial` snapshot (`message_start` /
/// `message_update`, `agent-loop.ts:314-341`) and nowhere else: not on a terminal event, not on the
/// agent loop's settled message, not in a session file (persistence happens on `message_end`, whose
/// message always comes from a terminal). Every settled-outcome consumer nonetheless handles it
/// explicitly rather than through a `_ =>` arm — see PROV-010 / AGENT-014 / DRIFT-012.
///
/// # No unknown-value fallback, by design
///
/// There is deliberately **no** `#[serde(other)]` catch-all. Pi's union is closed and enumerated in
/// full at types.ts:391 — unlike `ApiId`/`ProviderId`, which cyrup models as open newtype strings
/// precisely because upstream keeps extending them. A tolerant `Unknown` variant would (a) be lossy
/// on re-serialize, so `interop.rs`'s round-trip equivalence assertion (R-00-013) would silently
/// rewrite a future `"stopReason":"whatever"` to a different string on export — turning a loud
/// import failure into silent data corruption — and (b) land in the same `_ =>` success arms this
/// change just closed. A genuinely new upstream stop reason is a port decision (exit code,
/// compaction validity, truncated-tool-batch handling, TUI notice) and must be made deliberately,
/// not absorbed.
///
/// The cost of that strictness is that an unknown value fails `Deserialize`. On the session-load
/// path that failure is absorbed one level up rather than propagated: `cyrup_session::Entry`'s
/// `Deserialize` (`entry.rs:262-285`) falls back to `Entry::Unknown(Value)` when a known-tag line
/// does not fit the strict schema, so the line survives verbatim and `manager::load`'s `recovered`
/// flag is never raised for it. Nothing is destroyed — but the entry stops being interpretable
/// (`entries_have_assistant` answers `false` for it, `manager.rs:826-831`), so a genuinely new
/// upstream stop reason must still be added HERE to be understood. Widening the enum with a
/// tolerant catch-all would not fix that; it would only make the misunderstanding silent.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    /// In-flight: the provider has not delivered a terminal stop reason yet (Pi `"pending"`).
    /// Valid ONLY on a non-terminal event's `partial`; see the type docs.
    Pending,
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
    /// The provider accepted the request and returned a durable handle instead of a completed turn
    /// (Pi `"deferred"`, added in v0.84.0 — `v0.84.1 ai/src/types.ts:391`; absent at v0.83.0's
    /// otherwise identical `types.ts:391`).
    ///
    /// It is a **success** terminal, not an error: Pi narrows the `done` event's reason to
    /// `Extract<StopReason, "stop" | "length" | "toolUse" | "deferred">`
    /// (`v0.84.1 ai/src/types.ts:527-531`), so [`crate::StopReason::Deferred`] maps to a `done`
    /// event carrying an assistant message whose `content` is empty and whose payload is the
    /// [`crate::DeferredHandle`] in [`crate::AssistantMessage::deferred`].
    ///
    /// cyrup does not yet PRODUCE one — no cyrup wire api implements Pi's optional
    /// `fetchDeferred`/`cancelDeferred` (`v0.84.1 ai/src/types.ts:271-276`), and upstream's only
    /// producer is the faux test provider (`v0.84.1 ai/src/providers/faux.ts:293-305,524`), every
    /// real provider throwing `"Provider ${model.provider} does not support deferred responses"`
    /// (`v0.84.1 ai/src/models.ts:714,728`). The variant exists so a Pi-written session containing
    /// one loads, retains its handle, and re-exports unchanged (R-00-013).
    Deferred,
}

impl StopReason {
    /// Whether this reason represents a **settled** turn, i.e. anything but [`Self::Pending`].
    ///
    /// Use this instead of `!matches!(r, Error | Aborted)` when asking "did this turn produce a
    /// usable result": a `Pending` message is an unfinished stream, not a success.
    pub fn is_settled(self) -> bool {
        !matches!(self, StopReason::Pending)
    }
}
