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
            } => indexed_str(serializer, "thinking_end", *content_index, "content", content),
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
