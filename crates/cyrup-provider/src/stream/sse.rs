//! Direct-wire HTTP + SSE transport (arch-01 §7.1: `reqwest` + `rustls`, no native-tls, +
//! `eventsource-stream`).
//!
//! [`open_sse`] opens a request, exposes `on_request`/`on_response` observability hooks, maps a
//! non-2xx response or a transport failure to a typed [`ProviderError`] (the caller turns it into a
//! terminal `StreamEvent::Error`), and yields decoded SSE frames as an async stream that honors the
//! [`CancelToken`] (cancellation yields a single [`ProviderError::Aborted`] then ends).

use crate::error::ProviderError;
use crate::HeaderMap;
use bytes::Bytes;
use cyrup_core::CancelToken;
use eventsource_stream::{Event as EsEvent, EventStreamError, Eventsource};
use futures::{Stream, StreamExt};
use std::pin::Pin;
use std::sync::Arc;

/// Inspect (and log/route) the outbound request before send (func-01 R-01-048-adjacent).
pub type OnRequest = Arc<dyn Fn(&SseRequest) + Send + Sync>;
/// Inspect HTTP status + headers once the response opens (func-01 R-01-049).
pub type OnResponse = Arc<dyn Fn(u16, &reqwest::header::HeaderMap) + Send + Sync>;

/// An outbound SSE request description.
#[derive(Clone, Debug)]
pub struct SseRequest {
    pub method: reqwest::Method,
    pub url: String,
    /// Request headers. A `None` value suppresses a would-be default header (func-01 §4.1).
    pub headers: HeaderMap,
    pub body: Option<serde_json::Value>,
}

impl SseRequest {
    /// A `POST` with a JSON body (the common vendor case).
    pub fn post_json(url: impl Into<String>, body: serde_json::Value) -> Self {
        SseRequest {
            method: reqwest::Method::POST,
            url: url.into(),
            headers: HeaderMap::new(),
            body: Some(body),
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.insert(name.into(), Some(value.into()));
        self
    }
}

/// One decoded SSE frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseFrame {
    /// The `event:` field (`"message"` when unspecified, per the SSE spec).
    pub event: String,
    /// The `data:` payload.
    pub data: String,
}

/// Build the shared HTTP client (arch-01 §7.1: rustls-tls, no native-tls).
pub fn build_client() -> Result<reqwest::Client, ProviderError> {
    reqwest::Client::builder()
        .build()
        .map_err(|e| ProviderError::Transport(Box::new(e)))
}

type FrameStream = Pin<Box<dyn Stream<Item = Result<SseFrame, ProviderError>> + Send>>;

type EsInner =
    Pin<Box<dyn Stream<Item = Result<EsEvent, EventStreamError<reqwest::Error>>> + Send>>;

struct SseState {
    es: EsInner,
    cancel: CancelToken,
    done: bool,
}

/// Open the request and return a cancel-aware stream of decoded SSE frames.
///
/// Errors before the stream opens (transport failure, non-2xx HTTP, cancellation during connect)
/// are returned as `Err`; errors *during* streaming arrive as `Err` items inside the stream. In
/// both cases the caller converts them to a terminal `StreamEvent::Error` (func-01 R-01-018/045).
pub async fn open_sse(
    client: &reqwest::Client,
    req: SseRequest,
    cancel: CancelToken,
    on_request: Option<OnRequest>,
    on_response: Option<OnResponse>,
) -> Result<FrameStream, ProviderError> {
    if let Some(cb) = &on_request {
        cb(&req);
    }

    let mut builder = client.request(req.method.clone(), &req.url);
    for (name, value) in &req.headers {
        // A `None` value means "suppress a default"; on a fresh request there is nothing to
        // suppress, so only present values are applied.
        if let Some(value) = value {
            builder = builder.header(name.as_str(), value.as_str());
        }
    }
    if let Some(body) = &req.body {
        builder = builder.json(body);
    }

    // Race the connect against cancellation (R-01-044).
    let resp = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(ProviderError::Aborted),
        sent = builder.send() => sent.map_err(|e| ProviderError::Transport(Box::new(e)))?,
    };

    let status = resp.status();
    if let Some(cb) = &on_response {
        cb(status.as_u16(), resp.headers());
    }

    if !status.is_success() {
        let code = status.as_u16();
        let message = resp.text().await.unwrap_or_default();
        return Err(ProviderError::Http { status: code, message });
    }

    let es: EsInner = Box::pin(resp.bytes_stream().eventsource());
    let state = SseState { es, cancel, done: false };

    let stream = futures::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        tokio::select! {
            biased;
            _ = state.cancel.cancelled() => {
                state.done = true;
                Some((Err(ProviderError::Aborted), state))
            }
            next = state.es.next() => match next {
                None => None,
                Some(Ok(ev)) => {
                    Some((Ok(SseFrame { event: ev.event, data: ev.data }), state))
                }
                Some(Err(e)) => {
                    state.done = true;
                    Some((Err(ProviderError::Decode(e.to_string())), state))
                }
            },
        }
    });

    Ok(Box::pin(stream))
}

/// Decode raw SSE bytes into frames (no network) — useful for replaying recorded vendor fixtures
/// (arch-01 §11). Errors during decode arrive as `Err` items.
pub fn decode_sse_bytes(bytes: impl Into<Bytes>) -> FrameStream {
    let bytes = bytes.into();
    let byte_stream =
        futures::stream::once(async move { Ok::<Bytes, std::io::Error>(bytes) });
    let es = byte_stream.eventsource();
    let stream = es.map(|ev| match ev {
        Ok(ev) => Ok(SseFrame { event: ev.event, data: ev.data }),
        Err(e) => Err(ProviderError::Decode(e.to_string())),
    });
    Box::pin(stream)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn decodes_frames_from_fixture_bytes() {
        let raw = "event: delta\ndata: hello\n\ndata: world\n\ndata: [DONE]\n\n";
        let frames: Vec<_> = decode_sse_bytes(raw.as_bytes().to_vec())
            .map(|r| r.unwrap())
            .collect()
            .await;
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0], SseFrame { event: "delta".into(), data: "hello".into() });
        assert_eq!(frames[1].data, "world");
        assert_eq!(frames[2].data, "[DONE]");
    }
}
