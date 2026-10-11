//! The pi tool call a `tools/call` request serves, sent to the server as
//! `_meta["pi-mcp-adapter/toolCallId"]` — `4b7e310` (#674, first in v3.0.0), **MCP-609**.
//!
//! Upstream builds the key once per call from the `toolCallId` its tool's `execute` received
//! (`direct-tools.ts:321`, `proxy-modes.ts:1658` @v5.0.0) and sends it on every attempt of that
//! call, a session-recovery retry included. Here the id becomes an [`OriginatingCall`] at the same
//! two entry points ([`crate::dispatch::McpDispatch`]'s `call_direct` and [`super::McpTool`]'s
//! `execute`), rides [`super::execute_call`] to [`super::ProxyEnv::call_tool`], and is stamped on
//! the request in one place, inside the recovery closure, so each attempt carries it.
//!
//! **Not stamped, as upstream does not:** `resources/read` (`direct-tools.ts:291`,
//! `proxy-modes.ts:1625`), and the three upstream routes that call `executeCall` or `callTool`
//! with no id at all — `mcpScript` (`mcp-code.ts:206`; Cut 4), an MCP App's `/proxy/tools/call`
//! (`ui-server.ts:465-468`; Cut 2), and the runtime tool-call event (`index.ts:983`; unported,
//! `MCP-599`, which when it lands passes `None` here as upstream passes no id).

use cyrup_core::ToolCallId;
use rmcp::model::{CallToolRequestParams, RequestParamsMeta};
use serde_json::Value;

/// `TOOL_CALL_ID_REQUEST_META_KEY` (`utils.ts:350` @v5.0.0, unchanged through v5.2.0).
pub const TOOL_CALL_ID_META_KEY: &str = "pi-mcp-adapter/toolCallId";

/// The tool call a `tools/call` request is made for — the `toolCallId` upstream hands
/// `withToolCallIdMeta` (`utils.ts:352-358` @v5.0.0).
///
/// It exists only for a call with an id to be told apart by, so a request built without one cannot
/// carry an empty or invented key: the absence is `Option::None`, not a value to check later.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OriginatingCall(ToolCallId);

impl OriginatingCall {
    /// `if (!toolCallId) return meta;` (`utils.ts:356`) — the empty id a provider records for a
    /// tool call its stream named no id for is no id, and the request carries no key.
    ///
    /// [CYRUP-DELTA, upward] A call nested under such a caller (`/1`, `/1/2`, …; see
    /// [`ToolCallId::is_anonymous`]) is treated as having no id either. Its id is derived from a
    /// caller upstream itself treats as having none (`if (!toolCallId)`, `utils.ts:356`), and an id
    /// derived from no id is still no id. Upstream forwards it, because `"/1"` is truthy and the
    /// nested form is `` `${callerId}/${n}` `` whatever `callerId` is (pi
    /// `core/nested-tool-calls.ts:189` @f1b2e77f5). Every id with a real root is forwarded as
    /// upstream forwards it — a nested call under a real caller, and an id a provider adapter mints
    /// for a call its stream named none for (Mistral's `toolcall<index>`, which also repeats from
    /// turn to turn).
    #[must_use]
    pub fn of(call_id: &ToolCallId) -> Option<Self> {
        (!call_id.is_anonymous()).then(|| Self(call_id.clone()))
    }

    /// The id the server is sent.
    #[must_use]
    pub fn id(&self) -> &ToolCallId {
        &self.0
    }

    /// `{ ...meta, [TOOL_CALL_ID_REQUEST_META_KEY]: toolCallId }` (`utils.ts:357`): added to the
    /// `_meta` the params already carry, replacing only a same-named key.
    ///
    /// rmcp adds `progressToken` (and, for a client that negotiated request metadata, its
    /// `io.modelcontextprotocol/*` keys) at send time, beside this key and not over it
    /// (`rmcp` 3.1.4 `service.rs:877-888`).
    pub fn stamp(&self, params: &mut CallToolRequestParams) {
        params.meta_or_default().insert(
            TOOL_CALL_ID_META_KEY.to_string(),
            Value::String(self.0.as_str().to_string()),
        );
    }
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
    use rmcp::model::RequestMetaObject;

    fn meta_of(params: &CallToolRequestParams) -> Value {
        serde_json::to_value(params).unwrap()["_meta"].clone()
    }

    /// `__tests__/tool-call-id-meta.test.ts:32` — the key is upstream's, byte for byte.
    #[test]
    fn the_key_is_upstreams() {
        assert_eq!(TOOL_CALL_ID_META_KEY, "pi-mcp-adapter/toolCallId");
    }

    /// `withToolCallIdMeta(undefined, "call-1")` → `{ "pi-mcp-adapter/toolCallId": "call-1" }`:
    /// params with no `_meta` gain one holding exactly the key.
    #[test]
    fn a_call_with_an_id_puts_it_on_the_requests_meta() {
        let mut params = CallToolRequestParams::new("echo");
        OriginatingCall::of(&ToolCallId::from("call-1"))
            .unwrap()
            .stamp(&mut params);
        assert_eq!(
            meta_of(&params),
            serde_json::json!({ "pi-mcp-adapter/toolCallId": "call-1" })
        );
    }

    /// `{ ...meta, [KEY]: id }` (`utils.ts:357`) — every key already there survives
    /// (`tool-call-id-meta.test.ts:37-41`), and a same-named one is replaced by the call's id,
    /// because the spread puts the key last.
    #[test]
    fn the_id_joins_the_meta_already_there_and_wins_a_same_named_key() {
        let mut existing = RequestMetaObject::new();
        existing.insert("pi-mcp-adapter/stream-token".into(), "tok".into());
        existing.insert(TOOL_CALL_ID_META_KEY.into(), "stale".into());
        let mut params = CallToolRequestParams::new("echo");
        params.set_meta(existing);

        OriginatingCall::of(&ToolCallId::from("call-2"))
            .unwrap()
            .stamp(&mut params);

        assert_eq!(
            meta_of(&params),
            serde_json::json!({
                "pi-mcp-adapter/stream-token": "tok",
                "pi-mcp-adapter/toolCallId": "call-2",
            })
        );
    }

    /// `withToolCallIdMeta(undefined, "")` is `undefined` (`tool-call-id-meta.test.ts:47`): an empty
    /// id is no origin, so there is nothing to stamp. The request still carries the `_meta` rmcp adds
    /// (`progressToken`) but never this key, which `live.rs`'s
    /// `the_originating_call_rides_the_request_meta_and_no_origin_sends_no_key` checks on the wire.
    #[test]
    fn an_empty_id_is_no_origin() {
        assert_eq!(OriginatingCall::of(&ToolCallId::from("")), None);
    }

    /// [CYRUP-DELTA, upward] — a nested call under an id-less caller has no origin, at any depth. A
    /// nested call under a REAL caller, and any other id, is forwarded as upstream forwards it.
    #[test]
    fn a_nested_id_has_an_origin_exactly_when_its_caller_does() {
        let anonymous = ToolCallId::from("");
        let first = ToolCallId::nested(&anonymous, 1);
        assert_eq!(first.as_str(), "/1");
        assert_eq!(OriginatingCall::of(&first), None);
        assert_eq!(OriginatingCall::of(&ToolCallId::nested(&first, 2)), None);

        let real = ToolCallId::from("call_7");
        let nested = ToolCallId::nested(&real, 3);
        assert_eq!(
            OriginatingCall::of(&nested).unwrap().id().as_str(),
            "call_7/3"
        );

        // Only the form `nested` writes under an empty caller is anonymous: a provider id that merely
        // begins with `/` is a real id, and upstream forwards it.
        for provided in ["/calls/42", "/abc", "/", "/1/", "/1a"] {
            assert_eq!(
                OriginatingCall::of(&ToolCallId::from(provided))
                    .map(|call| call.id().as_str().to_string()),
                Some(provided.to_string()),
                "{provided:?} is forwarded"
            );
        }
    }
}
