//! Codex → Responses event mapping (pi :721-757).

use super::{CODEX_RESPONSE_STATUSES, FrameStream};
use crate::error::ProviderError;
use crate::stream::sse::SseFrame;
use futures::StreamExt;
use serde_json::{Value, json};

/// The outcome of mapping one Codex SSE event (pi `mapCodexEvents`, `:721-752`).
#[derive(Debug, PartialEq, Eq)]
pub(super) enum MappedCodexEvent {
    /// `if (!type) continue` — the event is dropped.
    Skip,
    /// A `CodexApiError` was thrown; the string is upstream's exact `Error.message`.
    Fail(String),
    /// Forwarded to the shared Responses decoder unchanged.
    Pass(Value),
    /// The rewritten `response.completed` terminal; upstream `return`s right after yielding it.
    Terminal(Value),
}

/// 1:1 port of pi `mapCodexEvents`'s per-event body (`openai-codex-responses.ts:722-751`) plus
/// `normalizeCodexStatus` (`:754-757`).
///
/// `request_service_tier` is `options?.serviceTier`; it is folded into the terminal event's
/// `response.service_tier` by [`resolve_codex_service_tier`] so the shared decoder's
/// `applyServiceTierPricing` — whose multiplier table is byte-identical to Codex's
/// `getServiceTierCostMultiplier` (`:598-610` vs `openai-responses.ts:281-293`) — prices the turn
/// exactly as upstream's `resolveServiceTier` hook would.
pub(super) fn map_codex_event(
    event: &Value,
    request_service_tier: Option<&str>,
) -> MappedCodexEvent {
    let Some(etype) = event.get("type").and_then(Value::as_str) else {
        return MappedCodexEvent::Skip;
    };

    if etype == "error" {
        let (code, message) = extract_codex_event_error(event);
        let detail = message
            .or(code)
            .unwrap_or_else(|| serde_json::to_string(event).unwrap_or_else(|_| "{}".to_string()));
        return MappedCodexEvent::Fail(format!("Codex error: {detail}"));
    }

    if etype == "response.failed" {
        let message = event
            .pointer("/response/error/message")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty());
        return MappedCodexEvent::Fail(message.unwrap_or("Codex response failed").to_string());
    }

    if matches!(
        etype,
        "response.done" | "response.completed" | "response.incomplete"
    ) {
        let mut mapped = event.clone();
        if let Some(obj) = mapped.as_object_mut() {
            obj.insert("type".to_string(), json!("response.completed"));
            if let Some(response) = obj.get_mut("response").and_then(Value::as_object_mut) {
                // `status: normalizeCodexStatus(response.status)` — an unknown status becomes
                // `undefined`, i.e. an absent key.
                let normalized = response
                    .get("status")
                    .and_then(Value::as_str)
                    .filter(|s| CODEX_RESPONSE_STATUSES.contains(s))
                    .map(str::to_string);
                match normalized {
                    Some(status) => {
                        response.insert("status".to_string(), json!(status));
                    }
                    None => {
                        response.remove("status");
                    }
                }
                let resolved = resolve_codex_service_tier(
                    response.get("service_tier").and_then(Value::as_str),
                    request_service_tier,
                );
                match resolved {
                    Some(tier) => {
                        response.insert("service_tier".to_string(), json!(tier));
                    }
                    None => {
                        response.remove("service_tier");
                    }
                }
            }
        }
        return MappedCodexEvent::Terminal(mapped);
    }

    MappedCodexEvent::Pass(event.clone())
}

/// 1:1 port of pi `extractCodexEventError` (`openai-codex-responses.ts:708-719`): the code/message
/// may sit on the event or inside a nested `error` object.
fn extract_codex_event_error(event: &Value) -> (Option<String>, Option<String>) {
    let field = |name: &str| {
        event
            .get(name)
            .and_then(Value::as_str)
            .or_else(|| {
                event
                    .pointer(&format!("/error/{name}"))
                    .and_then(Value::as_str)
            })
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    (field("code"), field("message"))
}

/// 1:1 port of pi `resolveCodexServiceTier` (`openai-codex-responses.ts:627-635`): the backend
/// reporting `"default"` does not override an explicitly requested `flex`/`priority` tier.
pub(super) fn resolve_codex_service_tier(
    response_service_tier: Option<&str>,
    request_service_tier: Option<&str>,
) -> Option<String> {
    if response_service_tier == Some("default")
        && matches!(request_service_tier, Some("flex") | Some("priority"))
    {
        return request_service_tier.map(str::to_string);
    }
    response_service_tier
        .or(request_service_tier)
        .map(str::to_string)
}

/// Streaming state for [`map_codex_frames`].
struct MapState {
    inner: FrameStream,
    done: bool,
    request_service_tier: Option<String>,
}

/// Apply [`map_codex_event`] across an SSE frame stream (pi's `mapCodexEvents` generator wrapped
/// around `parseSSE`, `:664`).
///
/// The generator's two control-flow effects are preserved: an untyped event is dropped, and the
/// terminal event ENDS the stream (upstream `return`s from the generator), so nothing after
/// `response.done` reaches the decoder.
///
/// **Error-text delta.** Upstream's `CodexApiError`/`CodexProtocolError` reach the outer catch,
/// which writes `error.message` verbatim into `errorMessage`. Here they travel as
/// [`ProviderError::Decode`], whose `Display` prefixes `"decode error: "`, because emitting an
/// unprefixed terminal from inside the *shared* decoder would mean changing
/// `openai_responses::decode_stream`. The message body is upstream's exact text.
pub(super) fn map_codex_frames(
    frames: FrameStream,
    request_service_tier: Option<String>,
) -> FrameStream {
    let state = MapState {
        inner: frames,
        done: false,
        request_service_tier,
    };
    Box::pin(futures::stream::unfold(state, |mut state| async move {
        if state.done {
            return None;
        }
        loop {
            let frame = match state.inner.next().await {
                // End of input: the shared decoder's own truncated-stream rule takes over.
                None => return None,
                Some(Err(e)) => {
                    state.done = true;
                    return Some((Err(e), state));
                }
                Some(Ok(frame)) => frame,
            };

            let data = frame.data.trim();
            // pi `parseSSE`: `if (data && data !== "[DONE]")`.
            if data.is_empty() || data == "[DONE]" {
                continue;
            }
            let event: Value = match serde_json::from_str(data) {
                Ok(v) => v,
                Err(e) => {
                    state.done = true;
                    // pi `CodexProtocolError(\`Invalid Codex SSE JSON: ${formatThrownValue(cause)}\`)`
                    // (:801).
                    return Some((
                        Err(ProviderError::Decode(format!(
                            "Invalid Codex SSE JSON: {e}"
                        ))),
                        state,
                    ));
                }
            };

            let tier = state.request_service_tier.clone();
            match map_codex_event(&event, tier.as_deref()) {
                MappedCodexEvent::Skip => continue,
                MappedCodexEvent::Fail(message) => {
                    state.done = true;
                    return Some((Err(ProviderError::Decode(message)), state));
                }
                MappedCodexEvent::Pass(value) => {
                    return Some((Ok(reframe(&frame, &value)), state));
                }
                MappedCodexEvent::Terminal(value) => {
                    state.done = true;
                    return Some((Ok(reframe(&frame, &value)), state));
                }
            }
        }
    }))
}

/// Re-serialize a mapped event back into an SSE frame for the shared decoder.
fn reframe(original: &SseFrame, value: &Value) -> SseFrame {
    SseFrame {
        event: original.event.clone(),
        data: value.to_string(),
    }
}
