//! Proxy `StreamFn` for apps that route LLM calls through an auth-managing server (1:1 port of Pi
//! `packages/agent/src/proxy.ts`, exported via `index.ts:42`).
//!
//! The server proxies the model call and streams back [`ProxyAssistantMessageEvent`]s with the heavy
//! `partial` snapshot **stripped** to save bandwidth (proxy.ts:33-34,84). The client rebuilds the
//! growing [`AssistantMessage`](cyrup_core::AssistantMessage) locally — including streaming
//! tool-call argument JSON via [`cyrup_provider::parse_streaming_json_object`] (Pi
//! `parseStreamingJson`, proxy.ts:324) — and re-emits the full `cyrup_provider::StreamEvent` stream
//! the agent loop already consumes.
//!
//! Transport reuses cyrup-provider's existing SSE client ([`cyrup_provider::open_sse`],
//! arch-01 §7.1) — the same `reqwest`+`rustls` path every direct provider uses, framed by
//! cyrup-provider's in-tree SSE framer — so no new dependency is introduced. `POST {proxyUrl}/api/stream` with `Authorization: Bearer`
//! and the `{ model, context, options }` body matches Pi `streamProxy` (proxy.ts:152-164); the
//! `cancel` token drives the abort that Pi performs via `reader.cancel` (proxy.ts:141-145).

mod builder;
mod http_status;
mod options;
mod stream_fn;
mod transport;
mod wire;

pub use builder::ProxyMessageBuilder;
pub use options::ProxyStreamOptions;
pub use stream_fn::ProxyStreamFn;
pub use transport::stream_proxy;
pub use wire::ProxyAssistantMessageEvent;

// `crate::proxy::proxy_error_message` — the path `src/tests/area02_backlog.rs` asserts against;
// keep it resolving.
pub(crate) use http_status::proxy_error_message;

// The three fixtures the split test modules share (verbatim from the pre-split `mod tests`). They
// sit here, private to `proxy`, rather than in a test-util module of their own — three trivial
// constructors do not justify one.
#[cfg(test)]
fn model() -> cyrup_core::ModelRef {
    cyrup_core::ModelRef {
        provider: "anthropic".into(),
        api: Some("anthropic-messages".into()),
        model: "claude".into(),
    }
}

#[cfg(test)]
fn usage_json() -> serde_json::Value {
    serde_json::json!({
        "input": 10, "output": 20, "cacheRead": 0, "cacheWrite": 0,
        "totalTokens": 30,
        "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0 }
    })
}

#[cfg(test)]
#[allow(clippy::expect_used)]
fn ev(json: serde_json::Value) -> ProxyAssistantMessageEvent {
    serde_json::from_value(json).expect("proxy event must deserialize")
}
