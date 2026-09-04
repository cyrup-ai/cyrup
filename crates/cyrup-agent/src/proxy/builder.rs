//! Client-side partial reconstruction (Pi `processProxyEvent` + `partial`, proxy.ts:121-367).

use super::wire::ProxyAssistantMessageEvent;
use cyrup_core::{
    AssistantMessage, Content, LazyArgs, ModelRef, SharedStr, ToolCall, ToolCallId, Usage,
};
use cyrup_provider::StreamEvent;
use serde_json::Map;
use std::collections::HashMap;
use std::sync::Arc;

/// Rebuilds the streaming [`AssistantMessage`] from bandwidth-reduced proxy events (Pi keeps the
/// `partial` object + a per-tool-call `partialJson` side-field, proxy.ts:121-137,323-324). cyrup
/// holds the streaming tool-call arg JSON in a side map keyed by content index rather than mutating
/// the typed [`ToolCall`] (which has no `partialJson` field) — observably identical: the block's
/// `arguments` track that buffer on every delta exactly as Pi's do. What differs is only when the
/// map behind them is built; see [`LazyArgs`] (PERF-001).
pub struct ProxyMessageBuilder {
    partial: AssistantMessage,
    /// Shared with every snapshot taken from it, so attaching the arguments to a block is a
    /// refcount bump and the `Map` is recovered only if something reads it (PERF-001). This
    /// builder is the CLIENT half of the proxy and carried the same per-delta whole-buffer
    /// re-parse the decoders did.
    tool_json: HashMap<usize, SharedStr>,
}

impl ProxyMessageBuilder {
    /// Seed the empty partial from the model identity (Pi `partial: AssistantMessage = {...}`,
    /// proxy.ts:121-137). `stopReason` starts at `pending`; `usage` is zeroed; content is empty.
    pub fn new(model: &ModelRef) -> Self {
        Self {
            partial: empty_partial(model),
            tool_json: HashMap::new(),
        }
    }

    /// The message assembled so far.
    pub fn partial(&self) -> &AssistantMessage {
        &self.partial
    }

    /// The message assembled so far, as the shared handle every [`StreamEvent`] now carries.
    ///
    /// This builder keeps an OWNED partial and mutates it in place, so this is exactly the one copy
    /// per event it always made: holding an `Arc` here instead would force `Arc::make_mut` to copy
    /// on the next mutation, because the event just emitted still holds a reference (PERF-001).
    /// That copy is now O(blocks) rather than O(bytes accumulated), because a block's text is a
    /// [`SharedStr`] and its tool arguments a [`LazyArgs`].
    fn shared(&self) -> Arc<AssistantMessage> {
        Arc::new(self.partial.clone())
    }

    /// Process one proxy event, mutating the partial and returning the reconstructed
    /// [`StreamEvent`] to forward (Pi `processProxyEvent`, proxy.ts:238-367). Returns `Ok(None)` for
    /// a `toolcall_end` whose content slot is not a tool call (Pi returns `undefined`,
    /// proxy.ts:347). Returns `Err(msg)` for a delta/end whose content slot has the wrong type — Pi
    /// `throw`s the identical message (proxy.ts:261,275,293,307,333), which its outer loop turns into
    /// a terminal `error` event.
    pub fn process(
        &mut self,
        event: ProxyAssistantMessageEvent,
    ) -> Result<Option<StreamEvent>, String> {
        match event {
            ProxyAssistantMessageEvent::Start => Ok(Some(StreamEvent::Start {
                partial: self.shared(),
            })),

            ProxyAssistantMessageEvent::TextStart { content_index } => {
                self.set_content(content_index, Content::text(""));
                Ok(Some(StreamEvent::TextStart {
                    content_index,
                    partial: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::TextDelta {
                content_index,
                delta,
            } => {
                match self.partial.content.get_mut(content_index) {
                    Some(Content::Text { text, .. }) => text.push_str(&delta),
                    _ => return Err("Received text_delta for non-text content".into()),
                }
                Ok(Some(StreamEvent::TextDelta {
                    content_index,
                    delta,
                    partial: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::TextEnd {
                content_index,
                content_signature,
            } => {
                let text = match self.partial.content.get_mut(content_index) {
                    Some(Content::Text {
                        text,
                        text_signature,
                    }) => {
                        *text_signature = content_signature;
                        text.to_string()
                    }
                    _ => return Err("Received text_end for non-text content".into()),
                };
                Ok(Some(StreamEvent::TextEnd {
                    content_index,
                    content: text,
                    partial: self.shared(),
                }))
            }

            ProxyAssistantMessageEvent::ThinkingStart { content_index } => {
                self.set_content(content_index, Content::thinking(""));
                Ok(Some(StreamEvent::ThinkingStart {
                    content_index,
                    partial: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::ThinkingDelta {
                content_index,
                delta,
            } => {
                match self.partial.content.get_mut(content_index) {
                    Some(Content::Thinking { thinking, .. }) => thinking.push_str(&delta),
                    _ => return Err("Received thinking_delta for non-thinking content".into()),
                }
                Ok(Some(StreamEvent::ThinkingDelta {
                    content_index,
                    delta,
                    partial: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::ThinkingEnd {
                content_index,
                content_signature,
            } => {
                let thinking = match self.partial.content.get_mut(content_index) {
                    Some(Content::Thinking {
                        thinking,
                        thinking_signature,
                        ..
                    }) => {
                        *thinking_signature = content_signature;
                        thinking.to_string()
                    }
                    _ => return Err("Received thinking_end for non-thinking content".into()),
                };
                Ok(Some(StreamEvent::ThinkingEnd {
                    content_index,
                    content: thinking,
                    partial: self.shared(),
                }))
            }

            ProxyAssistantMessageEvent::ToolCallStart {
                content_index,
                id,
                tool_name,
            } => {
                self.set_content(
                    content_index,
                    Content::ToolCall(ToolCall {
                        id: ToolCallId::from(id),
                        name: tool_name,
                        arguments: Map::new().into(),
                        thought_signature: None,
                    }),
                );
                self.tool_json.insert(content_index, SharedStr::new());
                Ok(Some(StreamEvent::ToolCallStart {
                    content_index,
                    partial: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::ToolCallDelta {
                content_index,
                delta,
            } => {
                let arguments = match self.partial.content.get(content_index) {
                    Some(Content::ToolCall(_)) => {
                        // Pi re-parses `content.partialJson` on every delta
                        // (`parseStreamingJson(content.partialJson) || {}`, proxy.ts:324). The
                        // recovered value is identical; only the cost differs — the block is handed
                        // a HANDLE on the buffer, so that parse runs only if something reads the
                        // arguments (PERF-001).
                        let buf = self.tool_json.entry(content_index).or_default();
                        buf.push_str(&delta);
                        LazyArgs::streaming(buf.clone())
                    }
                    _ => return Err("Received toolcall_delta for non-toolCall content".into()),
                };
                if let Some(Content::ToolCall(tc)) = self.partial.content.get_mut(content_index) {
                    tc.arguments = arguments;
                }
                Ok(Some(StreamEvent::ToolCallDelta {
                    content_index,
                    delta,
                    partial: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::ToolCallEnd { content_index } => {
                // Drop the streaming-JSON side buffer (Pi `delete content.partialJson`, proxy.ts:339).
                self.tool_json.remove(&content_index);
                match self.partial.content.get(content_index) {
                    Some(Content::ToolCall(tc)) => {
                        let tool_call = tc.clone();
                        Ok(Some(StreamEvent::ToolCallEnd {
                            content_index,
                            tool_call,
                            partial: self.shared(),
                        }))
                    }
                    // Pi returns `undefined` (no throw) for a non-toolCall slot (proxy.ts:347).
                    _ => Ok(None),
                }
            }

            ProxyAssistantMessageEvent::Done { reason, usage } => {
                self.partial.stop_reason = reason.into();
                self.partial.usage = usage;
                Ok(Some(StreamEvent::Done {
                    reason,
                    message: self.shared(),
                }))
            }
            ProxyAssistantMessageEvent::Error {
                reason,
                error_message,
                usage,
            } => {
                self.partial.stop_reason = reason.into();
                self.partial.error_message = error_message;
                self.partial.usage = usage;
                Ok(Some(StreamEvent::Error {
                    reason,
                    error: self.shared(),
                }))
            }
        }
    }

    /// Assign `content` at `index`, growing the content vector with empty-text fillers if the server
    /// skips ahead (Pi relies on JS sparse-array assignment, proxy.ts:247). In practice the server
    /// emits contiguous indices, so no filler is observable.
    fn set_content(&mut self, index: usize, content: Content) {
        if index >= self.partial.content.len() {
            self.partial.content.resize(index + 1, Content::text(""));
        }
        if let Some(slot) = self.partial.content.get_mut(index) {
            *slot = content;
        }
    }
}

fn empty_partial(model: &ModelRef) -> AssistantMessage {
    AssistantMessage {
        content: Vec::new(),
        provider: model.provider.clone(),
        model: model.model.to_string(),
        api: model
            .api
            .clone()
            .unwrap_or_else(|| cyrup_core::ApiId::from(cyrup_core::UNRESOLVED_API)),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        // Pi seeds the client-rebuilt partial with `stopReason: "pending"` verbatim
        // (proxy.ts:121-137, specifically `:123`). This is the client side of a Pi-server wire, so
        // the seed is directly observable interop, not an internal detail.
        stop_reason: cyrup_core::StopReason::Pending,
        deferred: None,
        error_message: None,
        raw_stop_reason: None,
        timestamp: 0,
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
    use crate::proxy::{ev, model, usage_json};
    use cyrup_core::StopReason;
    use cyrup_provider::stream::{DoneReason, ErrorReason};
    use serde_json::Value;

    /// Pi seeds the client-rebuilt partial with `stopReason: "pending"` verbatim (proxy.ts:123),
    /// and every non-terminal event re-emits that same object as `partial`. Seeding `stop` told
    /// anyone watching the reconstructed stream that the turn had completed before the first token
    /// landed — and this is a Pi-SERVER wire, so the seed is directly observable interop.
    #[test]
    fn rebuilt_partial_is_seeded_pending_and_stays_pending_until_the_terminal() {
        let mut b = ProxyMessageBuilder::new(&model());
        assert_eq!(b.partial().stop_reason, StopReason::Pending);
        assert_eq!(
            serde_json::to_value(b.partial()).unwrap()["stopReason"],
            "pending",
            "wire spelling must be Pi's"
        );

        for e in [
            serde_json::json!({"type": "start"}),
            serde_json::json!({"type": "text_start", "contentIndex": 0}),
            serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "Hi"}),
            serde_json::json!({"type": "text_end", "contentIndex": 0}),
        ] {
            let out = b.process(ev(e)).unwrap();
            if let Some(forwarded) = out.as_ref().and_then(StreamEvent::partial) {
                assert_eq!(
                    forwarded.stop_reason,
                    StopReason::Pending,
                    "a non-terminal partial must not claim a settled outcome"
                );
            }
            assert_eq!(b.partial().stop_reason, StopReason::Pending);
        }

        // The terminal settles it — `Pending` never escapes past here.
        let done = b
            .process(ev(
                serde_json::json!({"type": "done", "reason": "stop", "usage": usage_json()}),
            ))
            .unwrap();
        match done {
            Some(StreamEvent::Done { reason, message }) => {
                assert_eq!(reason, DoneReason::Stop);
                assert_eq!(message.stop_reason, StopReason::Stop);
            }
            other => panic!("expected done/stop, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_text_block_across_start_delta_end() {
        let mut b = ProxyMessageBuilder::new(&model());
        assert!(matches!(
            b.process(ev(serde_json::json!({"type": "start"}))).unwrap(),
            Some(StreamEvent::Start { .. })
        ));
        b.process(ev(
            serde_json::json!({"type": "text_start", "contentIndex": 0}),
        ))
        .unwrap();
        b.process(ev(
            serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "Hel"}),
        ))
        .unwrap();
        b.process(ev(
            serde_json::json!({"type": "text_delta", "contentIndex": 0, "delta": "lo"}),
        ))
        .unwrap();
        let end = b.process(ev(serde_json::json!({"type": "text_end", "contentIndex": 0, "contentSignature": "sig"}))).unwrap();
        match end {
            Some(StreamEvent::TextEnd { content, .. }) => assert_eq!(content, "Hello"),
            other => panic!("expected text_end, got {other:?}"),
        }
        match b.partial().content.first() {
            Some(Content::Text {
                text,
                text_signature,
            }) => {
                assert_eq!(text, "Hello");
                assert_eq!(text_signature.as_deref(), Some("sig"));
            }
            other => panic!("expected text content, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_thinking_block_with_signature() {
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(
            serde_json::json!({"type": "thinking_start", "contentIndex": 0}),
        ))
        .unwrap();
        b.process(ev(
            serde_json::json!({"type": "thinking_delta", "contentIndex": 0, "delta": "ponder"}),
        ))
        .unwrap();
        b.process(ev(serde_json::json!({"type": "thinking_end", "contentIndex": 0, "contentSignature": "ts"}))).unwrap();
        match b.partial().content.first() {
            Some(Content::Thinking {
                thinking,
                thinking_signature,
                ..
            }) => {
                assert_eq!(thinking, "ponder");
                assert_eq!(thinking_signature.as_deref(), Some("ts"));
            }
            other => panic!("expected thinking content, got {other:?}"),
        }
    }

    #[test]
    fn rebuilds_tool_call_args_from_streaming_json() {
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(serde_json::json!({"type": "toolcall_start", "contentIndex": 0, "id": "call_1", "toolName": "read_file"}))).unwrap();
        // Stream the arguments JSON in fragments; each delta re-parses the accumulated buffer.
        b.process(ev(serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "{\"path\":\"a."}))).unwrap();
        // Mid-stream the (truncated) JSON is recovered as much as possible (Pi parseStreamingJson).
        if let Some(Content::ToolCall(tc)) = b.partial().content.first() {
            assert_eq!(tc.arguments.get("path").and_then(Value::as_str), Some("a."));
        } else {
            panic!("expected tool call");
        }
        b.process(ev(
            serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "txt\"}"}),
        ))
        .unwrap();
        let end = b
            .process(ev(
                serde_json::json!({"type": "toolcall_end", "contentIndex": 0}),
            ))
            .unwrap();
        match end {
            Some(StreamEvent::ToolCallEnd { tool_call, .. }) => {
                assert_eq!(tool_call.id.as_str(), "call_1");
                assert_eq!(tool_call.name, "read_file");
                assert_eq!(
                    tool_call.arguments.get("path").and_then(Value::as_str),
                    Some("a.txt")
                );
            }
            other => panic!("expected toolcall_end, got {other:?}"),
        }
    }

    #[test]
    fn content_type_mismatch_returns_err_like_pi_throw() {
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(
            serde_json::json!({"type": "text_start", "contentIndex": 0}),
        ))
        .unwrap();
        // A toolcall_delta against a text slot: Pi throws; cyrup returns Err with the same message.
        let r = b.process(ev(
            serde_json::json!({"type": "toolcall_delta", "contentIndex": 0, "delta": "x"}),
        ));
        assert_eq!(
            r,
            Err("Received toolcall_delta for non-toolCall content".to_string())
        );
    }

    #[test]
    fn toolcall_end_on_non_toolcall_slot_returns_none() {
        // Pi returns `undefined` (no throw) for this case (proxy.ts:347).
        let mut b = ProxyMessageBuilder::new(&model());
        b.process(ev(
            serde_json::json!({"type": "text_start", "contentIndex": 0}),
        ))
        .unwrap();
        assert_eq!(
            b.process(ev(
                serde_json::json!({"type": "toolcall_end", "contentIndex": 0})
            ))
            .unwrap(),
            None
        );
    }

    #[test]
    fn done_event_sets_stop_reason_and_usage() {
        let mut b = ProxyMessageBuilder::new(&model());
        let done = b
            .process(ev(
                serde_json::json!({"type": "done", "reason": "stop", "usage": usage_json()}),
            ))
            .unwrap();
        match done {
            Some(StreamEvent::Done { reason, message }) => {
                assert_eq!(reason, DoneReason::Stop);
                assert_eq!(message.stop_reason, StopReason::Stop);
                assert_eq!(message.usage.total_tokens, 30);
            }
            other => panic!("expected done, got {other:?}"),
        }
    }

    #[test]
    fn error_event_maps_reason_and_message() {
        let mut b = ProxyMessageBuilder::new(&model());
        let e = b.process(ev(serde_json::json!({"type": "error", "reason": "error", "errorMessage": "boom", "usage": usage_json()}))).unwrap();
        match e {
            Some(StreamEvent::Error { reason, error }) => {
                assert_eq!(reason, ErrorReason::Error);
                assert_eq!(error.stop_reason, StopReason::Error);
                assert_eq!(error.error_message.as_deref(), Some("boom"));
            }
            other => panic!("expected error, got {other:?}"),
        }
    }
}
