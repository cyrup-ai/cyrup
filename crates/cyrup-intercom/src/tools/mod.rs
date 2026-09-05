//! The two intercom tools registered via `InitApi::register_tool`:
//!
//! - [`intercom::IntercomTool`] (`intercom`) — always registered (`index.ts:1425-1806`).
//! - [`contact_supervisor::ContactSupervisorTool`] (`contact_supervisor`) — registered ONLY when
//!   child-orchestrator metadata is present (`index.ts:1162-1163`).
//!
//! Both are `cyrup_core::Tool` impls backed by the shared [`crate::session_state::SharedIntercomState`]
//! (its live [`crate::transport::client::IntercomClient`], [`crate::reply_tracker`] state, and the
//! outbound single-slot waiter).

pub mod contact_supervisor;
pub mod intercom;
pub(crate) mod render;

use cyrup_core::{Content, TerminateHint, ToolResult};

/// Build a plain-text [`ToolResult`] carrying pi's **empty** `details: {}`.
///
/// Upstream never returns a result with the `details` key ABSENT from either tool — every arm of
/// `intercom` and `contact_supervisor` sets it, and the arms with nothing structured to report set
/// it to `{}` (`v0.10.1 index.ts:1880`, `:1934`, `:1996`, `:2021`, `:2183`, `:2256`, `:2268`,
/// `:2281`). This used to be `details: None`, which is upstream's `undefined`, i.e. a state pi's
/// own renderers are written against but pi itself cannot produce.
pub(crate) fn text_result(text: impl Into<String>) -> ToolResult {
    detailed_result(text, serde_json::json!({}))
}

/// Build a plain-text [`ToolResult`] carrying the structured `details` pi attaches to the arms that
/// have something to report — `messageId`, `delivered`, `reason`, `replyTo`, `structuredReply`.
///
/// **`details.messageId` is load-bearing, not decoration.** `intercom({action:"reply", replyTo})`
/// takes a MESSAGE id, and `render_result` prints `` (${messageId.slice(0,8)}) `` for exactly that
/// reason (`v0.10.1 index.ts:2325-2327`) — so a `send`/`ask` whose result carried no `details` gave
/// the model no way to learn the id it is later asked to quote back.
pub(crate) fn detailed_result(text: impl Into<String>, details: serde_json::Value) -> ToolResult {
    ToolResult {
        content: vec![Content::text(text.into())],
        details: Some(details),
        terminate: TerminateHint::Unspecified,
        ..Default::default()
    }
}

/// `deliveryDetails(result)` (`v0.13.0 index.ts:116-126`) — the ack's full outcome, spread into a
/// tool result's `details`.
///
/// ICOM-054. `code` and `reason` are OMITTED when absent rather than emitted as `null`, because
/// upstream spreads them conditionally; the other four are always present. This is what puts
/// `delivery: "queued"` in front of the model when a peer was offline, where the result previously
/// said only `delivered: true`.
pub(crate) fn delivery_details(result: &crate::transport::client::SendResult) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    map.insert("messageId".to_string(), serde_json::json!(result.id));
    map.insert("delivered".to_string(), serde_json::json!(result.delivered));
    map.insert("delivery".to_string(), serde_json::json!(result.delivery));
    map.insert("retryable".to_string(), serde_json::json!(result.retryable));
    map.insert(
        "outcomeKnown".to_string(),
        serde_json::json!(result.outcome_known),
    );
    if let Some(code) = &result.code {
        map.insert("code".to_string(), serde_json::json!(code));
    }
    if let Some(reason) = &result.reason {
        map.insert("reason".to_string(), serde_json::json!(reason));
    }
    serde_json::Value::Object(map)
}
