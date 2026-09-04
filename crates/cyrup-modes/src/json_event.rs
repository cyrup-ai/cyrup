//! The json/rpc stdout wire projection (func-11 R-11-007/011; arch-11 §3.5).
//!
//! 1:1 port of Pi `coding-agent/src/modes/json-event.ts` (40 lines at v0.84.1; the file does NOT
//! exist at v0.83.0 — `git show v0.83.0:packages/coding-agent/src/modes/json-event.ts` → `fatal:
//! path … exists on disk, but not in 'v0.83.0'`). This is therefore VERSION LAG, not a port bug:
//! cyrup faithfully ported v0.83.0, whose `print-mode.ts` wrote `JSON.stringify(event)` and whose
//! `rpc-mode.ts` wrote `output(event)`.
//!
//! # What it projects
//!
//! Pi's `toJsonEvent` (json-event.ts:28-40) touches **only** `message_update` — every other event
//! type is returned by identity (`if (event.type !== "message_update") return event;`, :29-31). For
//! `message_update` it builds a *fresh two-key object* with no `...event` spread:
//!
//! ```ts
//! const assistantMessageEvent = event.assistantMessageEvent;
//! if (!("partial" in assistantMessageEvent)) {
//!     return { type: "message_update", assistantMessageEvent };
//! }
//! const { partial: _partial, ...deltaEvent } = assistantMessageEvent;
//! return { type: "message_update", assistantMessageEvent: deltaEvent };
//! ```
//!
//! So TWO fields leave the wire, not one:
//! - the outer `message` (the cumulative [`cyrup_session_svc::AgentSessionEvent::MessageUpdate`]
//!   `AgentMessage` snapshot) — dropped by the fresh-object construction, on BOTH branches;
//! - the inner `assistantMessageEvent.partial` (the cumulative `AssistantMessage` snapshot every
//!   non-terminal [`StreamEvent`] carries) — dropped by the rest-destructure.
//!
//! Pi's own regression test asserts both drops
//! (`coding-agent/test/suite/regressions/7290-json-stream-linear.test.ts:30-33`):
//! `expect(update).not.toHaveProperty("message")` and
//! `expect(update.assistantMessageEvent).not.toHaveProperty("partial")`, with the size assertion at
//! `:41-42` (`expect(largeBytes / smallBytes).toBeLessThan(2.2)` across a 2× longer response). Both
//! snapshots grow with the message, so re-emitting them on every delta makes the stream QUADRATIC in
//! the response length; dropping them makes it linear — the point of the change.
//!
//! The `!("partial" in …)` branch covers the `done`/`error` terminals, which carry no `partial`
//! (types.ts:527-532); they still lose the outer `message`, which is why both arms below route
//! through the same two-key construction.
//!
//! # It is a WIRE projection, never a type change
//!
//! Pi leaves the internal event untouched: `agent/src/types.ts:438` still declares
//! `{ type: "message_update"; message: AgentMessage; assistantMessageEvent: AssistantMessageEvent }`
//! and `agent.ts:550-552` still reduces `this._state.streamingMessage = event.message`. cyrup's
//! in-process consumers likewise depend on both fields — `cyrup-tui/src/app.rs:3803`,
//! `cyrup-agent/src/state.rs:147`, and `cyrup-ext/src/event.rs:429-430` (which maps the pair to
//! `HostEvent::MessageUpdate { message, delta }` for WASM guests) — so the projection is applied at
//! the two SERIALIZERS, exactly as Pi applies `toJsonEvent` at exactly two call sites and nowhere
//! else (`print-mode.ts:110`, `rpc/rpc-mode.ts:356`; verified with
//! `git grep -n toJsonEvent v0.84.1 -- packages/`).
//!
//! # Contract
//!
//! Pi ships this as a documented break, `coding-agent/docs/rpc.md:952-956` **@v0.84.1** (SEAM-085 —
//! the version tag is load-bearing: at the ported v0.83.0 baseline `docs/rpc.md:952-956` is the
//! *streaming example*, which shows `"message":{...}` and `"partial":{...}` on every delta, i.e. the
//! exact opposite of the contract quoted below):
//!
//! > `message_update` intentionally omits the former cumulative `message` field and
//! > `assistantMessageEvent.partial`. Clients that need a live partial message must assemble it from
//! > `message_start` and subsequent events using `contentIndex`. Treat `message_end.message` as
//! > authoritative. For tool calls, buffer `toolcall_delta.delta`; `toolcall_end.toolCall` contains
//! > the completed call.
//!
//! and retypes its own RPC client to the projected type (`rpc-client.ts:50`,
//! `RpcEventListener = (event: JsonAgentSessionEvent) => void`). cyrup's in-tree client of this wire
//! is `cyrup-ext-subagents`' `SubagentEvent` (`exec/ndjson.rs`), retyped in the same change.

use cyrup_session_svc::{AgentSessionEvent, StreamEvent};
use serde::ser::{Serialize, SerializeMap, Serializer};

/// Project one seam event into the shape the json/rpc stdout protocols emit (Pi `toJsonEvent`,
/// json-event.ts:28-40).
///
/// Borrows rather than clones: the returned value is a serialization view, so the dropped snapshots
/// are never visited by the serializer at all (Pi's rest-destructure is likewise free — its objects
/// are references). Every event type other than `message_update` serializes byte-identically to the
/// unprojected event.
#[must_use]
pub fn to_json_event(event: &AgentSessionEvent) -> JsonAgentSessionEvent<'_> {
    JsonAgentSessionEvent(event)
}

/// Whether `event` is one upstream's `session.subscribe(...)` can actually deliver — i.e. whether
/// it may be written to the json/rpc stdout stream at all.
///
/// pi's wire is `output(event)` over whatever `session.subscribe` hands it (`rpc-mode.ts:355`,
/// `print-mode.ts:74` @v0.83.0), and the listener type is `AgentSessionEventListener`, so the
/// stdout line set is EXACTLY the `AgentSessionEvent` union — `core/agent-session.ts:139-181`
/// @v0.83.0, re-read this pass. cyrup's [`AgentSessionEvent`] is a deliberate SUPER-set: it carries
/// four members upstream's union does not, and every one of them was reaching stdout as a protocol
/// line pi never writes.
///
/// * `session_replaced` — wholly cyrup-internal (R-11-021: the rebind signal that terminates a
///   subscription). It was already filtered at the two rpc write sites; that guard is generalized
///   here so the json mode, which never had one, is covered by the same rule.
/// * `model_changed` (SEAM-080) — pi has no such event. `git grep -n 'model_changed\|modelChanged'
///   v0.83.0 -- packages/` hits only `core/cache-stats.ts` (a boolean field on a cache-miss record)
///   and its one reader in `interactive-mode.ts`; nothing that is ever emitted. A `set_model` /
///   `cycle_model` over RPC therefore wrote a line no pi client can parse, IN ADDITION to its
///   `response`. It stays on the in-process fanout, which is where `cyrup-tui` reads it.
/// * `session_start` / `session_shutdown` (SEAM-081) — both names DO exist upstream, but as
///   EXTENSION-runner events, not session events. Re-derived at `v0.83.0`:
///   `interface SessionStartEvent` is `core/extensions/types.ts:561-567` and
///   `interface SessionShutdownEvent` is `:616-622`; the subscriptions are the
///   `on(event: "session_start", …)` / `on(event: "session_shutdown", …)` overloads at
///   `types.ts:1192` and `:1204`; and the emissions are a `sessionStartEvent:` config field
///   (`agent-session-runtime.ts:218`) and `emitSessionShutdownEvent(this._extensionRunner, …)`
///   (`agent-session.ts:2604`). Neither is in the `AgentSessionEvent`
///   union, so `session.subscribe` never sees them and `output(event)` can never write them. cyrup
///   emits each on BOTH tiers — `fanout_emit` **and** `dispatcher().dispatch_notify(HostEvent::…)`
///   (`cyrup-session-svc/src/session.rs:2706-2719` and `:2873-2894`) — so dropping the fanout copy
///   here removes the invented stdout line while leaving the extension tier, which is the tier that
///   upstream actually has, completely intact.
///
/// The decision recorded for SEAM-080/SEAM-081 is (a) — filter, do not document-and-keep — because
/// the whole value of this surface is that a client written against pi's `docs/rpc.md` works against
/// cyrup, and because both events already have a correctly-scoped delivery path for the consumers
/// that genuinely need them (the fanout for the TUI, the extension runner for guests).
///
/// The match is EXHAUSTIVE with no `_` arm on purpose: a new [`AgentSessionEvent`] variant must fail
/// to compile here rather than default onto a wire pi's clients parse.
#[must_use]
pub fn is_upstream_wire_event(event: &AgentSessionEvent) -> bool {
    match event {
        // cyrup-only members — never on pi's wire.
        AgentSessionEvent::SessionReplaced { .. }
        | AgentSessionEvent::ModelChanged { .. }
        | AgentSessionEvent::SessionStart { .. }
        | AgentSessionEvent::SessionShutdown { .. } => false,
        // Every member of pi's `AgentSessionEvent` union (`agent-session.ts:139-181` @v0.83.0),
        // including the `AgentEvent` members it re-exports.
        AgentSessionEvent::AgentStart
        | AgentSessionEvent::TurnStart
        | AgentSessionEvent::MessageStart { .. }
        | AgentSessionEvent::MessageUpdate { .. }
        | AgentSessionEvent::MessageEnd { .. }
        | AgentSessionEvent::ToolExecutionStart { .. }
        | AgentSessionEvent::ToolExecutionUpdate { .. }
        | AgentSessionEvent::ToolExecutionEnd { .. }
        | AgentSessionEvent::TurnEnd { .. }
        | AgentSessionEvent::AgentEnd { .. }
        | AgentSessionEvent::AgentSettled
        | AgentSessionEvent::QueueUpdate { .. }
        | AgentSessionEvent::CompactionStart { .. }
        | AgentSessionEvent::CompactionEnd { .. }
        | AgentSessionEvent::AutoRetryStart { .. }
        | AgentSessionEvent::AutoRetryEnd { .. }
        | AgentSessionEvent::SummarizationRetryScheduled { .. }
        | AgentSessionEvent::SummarizationRetryAttemptStart { .. }
        | AgentSessionEvent::SummarizationRetryFinished
        | AgentSessionEvent::BashExecutionUpdate { .. }
        | AgentSessionEvent::ThinkingLevelChanged { .. }
        | AgentSessionEvent::SessionInfoChanged { .. }
        | AgentSessionEvent::EntryAppended { .. } => true,
    }
}

/// The wire shape of an [`AgentSessionEvent`] (Pi `JsonAgentSessionEvent`, json-event.ts:16).
///
/// Construct via [`to_json_event`]. Serializing this is the ONLY sanctioned way to put a seam event
/// on the json or rpc stdout stream — serializing the [`AgentSessionEvent`] directly re-introduces
/// the quadratic snapshots.
#[derive(Clone, Copy, Debug)]
pub struct JsonAgentSessionEvent<'a>(&'a AgentSessionEvent);

impl Serialize for JsonAgentSessionEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        // Pi json-event.ts:29-31 — identity for every other event type.
        let AgentSessionEvent::MessageUpdate {
            assistant_message_event,
            ..
        } = self.0
        else {
            return self.0.serialize(serializer);
        };
        // Pi json-event.ts:35 and :39 — a fresh TWO-key object. The `..` above is the outer
        // `message` snapshot, which is deliberately not carried over.
        let mut map = serializer.serialize_map(Some(2))?;
        map.serialize_entry("type", "message_update")?;
        map.serialize_entry(
            "assistantMessageEvent",
            &DeltaOnly(assistant_message_event.as_ref()),
        )?;
        map.end()
    }
}

/// A [`StreamEvent`] serialized without its cumulative `partial` snapshot (Pi's
/// `WithoutPartial<T>` / the `const { partial: _partial, ...deltaEvent }` rest-destructure,
/// json-event.ts:3 and :38).
///
/// The match is EXHAUSTIVE with no `_` arm on purpose: a new [`StreamEvent`] variant must fail to
/// compile here rather than silently pick a default, so the projection cannot drift away from the
/// event enum it mirrors. Field emission order matches `StreamEvent`'s declaration order (which is
/// what its derived `Serialize` emits), so a projected record is byte-identical to the unprojected
/// one minus the `partial` key.
struct DeltaOnly<'a>(&'a StreamEvent);

impl Serialize for DeltaOnly<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            StreamEvent::Start { .. } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("type", "start")?;
                map.end()
            }
            StreamEvent::TextStart { content_index, .. } => {
                indexed(serializer, "text_start", *content_index)
            }
            StreamEvent::ThinkingStart { content_index, .. } => {
                indexed(serializer, "thinking_start", *content_index)
            }
            StreamEvent::ToolCallStart { content_index, .. } => {
                indexed(serializer, "toolcall_start", *content_index)
            }
            StreamEvent::TextDelta {
                content_index,
                delta,
                ..
            } => indexed_str(serializer, "text_delta", *content_index, "delta", delta),
            StreamEvent::ThinkingDelta {
                content_index,
                delta,
                ..
            } => indexed_str(serializer, "thinking_delta", *content_index, "delta", delta),
            StreamEvent::ToolCallDelta {
                content_index,
                delta,
                ..
            } => indexed_str(serializer, "toolcall_delta", *content_index, "delta", delta),
            StreamEvent::TextEnd {
                content_index,
                content,
                ..
            } => indexed_str(serializer, "text_end", *content_index, "content", content),
            StreamEvent::ThinkingEnd {
                content_index,
                content,
                ..
            } => indexed_str(
                serializer,
                "thinking_end",
                *content_index,
                "content",
                content,
            ),
            StreamEvent::ToolCallEnd {
                content_index,
                tool_call,
                ..
            } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "toolcall_end")?;
                map.serialize_entry("contentIndex", content_index)?;
                map.serialize_entry("toolCall", tool_call)?;
                map.end()
            }
            // The two terminals carry no `partial` — Pi's `!("partial" in assistantMessageEvent)`
            // branch (json-event.ts:34-36) returns them untouched, so delegate to the derived impl
            // and keep `done.message` / `error.error` (the AUTHORITATIVE final message, per
            // rpc.md:954) on the wire.
            terminal @ (StreamEvent::Done { .. } | StreamEvent::Error { .. }) => {
                terminal.serialize(serializer)
            }
        }
    }
}

/// `{type, contentIndex}` — the three `*_start` block events.
fn indexed<S: Serializer>(
    serializer: S,
    tag: &'static str,
    content_index: usize,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(2))?;
    map.serialize_entry("type", tag)?;
    map.serialize_entry("contentIndex", &content_index)?;
    map.end()
}

/// `{type, contentIndex, <key>}` — the three `*_delta` events (`key = "delta"`) and the two
/// text/thinking `*_end` events (`key = "content"`).
fn indexed_str<S: Serializer>(
    serializer: S,
    tag: &'static str,
    content_index: usize,
    key: &'static str,
    value: &str,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(3))?;
    map.serialize_entry("type", tag)?;
    map.serialize_entry("contentIndex", &content_index)?;
    map.serialize_entry(key, value)?;
    map.end()
}
