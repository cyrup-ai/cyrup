//! The faux provider — scripted, deterministic, no network, no tokens (func-01 §15 / R-01-051..053).
//!
//! Used by tests/demos across the workspace (agent loop, sessions, compaction, tools, hooks) so
//! they run without real provider APIs (func-00 R-00-011). Available to this crate's own tests and
//! behind the `faux` feature for downstream consumers.

use crate::context::Context;
use crate::model::{Modality, Model, ModelCost};
use crate::provider::Provider;
use crate::stream::{StreamEvent, StreamOptions};
use cyrup_core::{
    AssistantMessage, Content, EventStream, ProviderId, StopReason, ToolCall, ToolCallId, Usage,
};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static FAUX_CALL_SEQ: AtomicUsize = AtomicUsize::new(0);

/// Rough usage estimate: ~1 token per 4 characters (func-01 R-01-052).
fn estimate_tokens(s: &str) -> u64 {
    (s.chars().count() as u64).div_ceil(4)
}

fn estimate_output(content: &[Content]) -> u64 {
    content
        .iter()
        .map(|b| match b {
            Content::Text { text, .. } => estimate_tokens(text),
            Content::Thinking { thinking, .. } => estimate_tokens(thinking),
            Content::ToolCall(tc) => {
                estimate_tokens(&tc.name)
                    + serde_json::to_string(&tc.arguments)
                        .map(|s| estimate_tokens(&s))
                        .unwrap_or(0)
            }
            Content::Image { .. } => 0,
        })
        .sum()
}

/// A scripted provider whose responses are consumed from a queue in request order.
pub struct FauxProvider {
    id: ProviderId,
    default_model: Model,
    models: Vec<Model>,
    queue: Mutex<VecDeque<AssistantMessage>>,
    call_count: AtomicUsize,
}

impl Default for FauxProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FauxProvider {
    pub fn new() -> Self {
        let id = ProviderId::from("faux");
        let default_model = Model {
            id: "faux-1".into(),
            name: "Faux".into(),
            api: "faux".into(),
            provider: id.clone(),
            base_url: None,
            reasoning: true,
            input: vec![Modality::Text],
            output: Vec::new(),
            cost: ModelCost::default(),
            context_window: 200_000,
            max_tokens: 8192,
            thinking_level_map: None,
            compat: None,
        };
        Self {
            id,
            models: vec![default_model.clone()],
            default_model,
            queue: Mutex::new(VecDeque::new()),
            call_count: AtomicUsize::new(0),
        }
    }

    /// The default faux model (convenience for tests).
    pub fn model(&self) -> &Model {
        &self.default_model
    }

    /// Replace the remaining response queue (func-01 R-01-052).
    pub fn set_responses(&self, responses: Vec<AssistantMessage>) {
        if let Ok(mut q) = self.queue.lock() {
            q.clear();
            q.extend(responses);
        }
    }

    /// Append more responses to the queue.
    pub fn append_responses(&self, responses: Vec<AssistantMessage>) {
        if let Ok(mut q) = self.queue.lock() {
            q.extend(responses);
        }
    }

    pub fn pending_count(&self) -> usize {
        self.queue.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn call_count(&self) -> usize {
        self.call_count.load(Ordering::SeqCst)
    }
}

impl Provider for FauxProvider {
    fn id(&self) -> &ProviderId {
        &self.id
    }

    fn models(&self) -> &[Model] {
        &self.models
    }

    fn stream(
        &self,
        _model: &Model,
        context: &Context,
        _options: &StreamOptions,
    ) -> EventStream<StreamEvent> {
        self.call_count.fetch_add(1, Ordering::SeqCst);

        let popped = self.queue.lock().ok().and_then(|mut q| q.pop_front());
        let mut message = popped.unwrap_or_else(|| {
            AssistantMessage::errored(
                self.id.clone(),
                self.default_model.id.as_str(),
                StopReason::Error,
                "No more faux responses queued",
            )
        });

        // Estimate input from the serialized context; simulate cache only when configured (R-01-052).
        let input = serde_json::to_string(context).map(|s| estimate_tokens(&s)).unwrap_or(0);
        message.usage.input = input;
        message.usage.total_tokens = message.usage.input
            + message.usage.output
            + message.usage.cache_read
            + message.usage.cache_write;

        Box::pin(futures::stream::iter(events_for(message)))
    }
}

fn events_for(message: AssistantMessage) -> Vec<StreamEvent> {
    let mut events = vec![StreamEvent::Start];
    for (i, block) in message.content.iter().enumerate() {
        match block {
            Content::Text { text, .. } => {
                events.push(StreamEvent::TextStart { content_index: i });
                events.push(StreamEvent::TextDelta { content_index: i, delta: text.clone() });
                events.push(StreamEvent::TextEnd { content_index: i, content: text.clone() });
            }
            Content::Thinking { thinking, .. } => {
                events.push(StreamEvent::ThinkingStart { content_index: i });
                events
                    .push(StreamEvent::ThinkingDelta { content_index: i, delta: thinking.clone() });
                events.push(StreamEvent::ThinkingEnd { content_index: i, content: thinking.clone() });
            }
            Content::ToolCall(tc) => {
                events.push(StreamEvent::ToolCallStart { content_index: i });
                let delta = serde_json::to_string(&tc.arguments).unwrap_or_default();
                events.push(StreamEvent::ToolCallDelta { content_index: i, delta });
                events.push(StreamEvent::ToolCallEnd { content_index: i, tool_call: tc.clone() });
            }
            // Images are carried only in the terminal message (faux does not chunk them).
            Content::Image { .. } => {}
        }
    }
    let is_error = matches!(message.stop_reason, StopReason::Error | StopReason::Aborted);
    if is_error {
        events.push(StreamEvent::Error { message });
    } else {
        events.push(StreamEvent::Done { message });
    }
    events
}

// ---- Scripting helpers (func-01 §15) ----

pub fn faux_text(s: impl Into<String>) -> Content {
    Content::text(s)
}

pub fn faux_thinking(s: impl Into<String>) -> Content {
    Content::thinking(s)
}

pub fn faux_tool_call(name: impl Into<String>, arguments: serde_json::Value) -> Content {
    let n = FAUX_CALL_SEQ.fetch_add(1, Ordering::SeqCst);
    Content::ToolCall(ToolCall {
        id: ToolCallId::from(format!("faux-call-{n}")),
        name: name.into(),
        arguments,
        thought_signature: None,
    })
}

/// Build a scripted assistant reply (func-01 §15). Usage output is estimated from `content`.
pub fn faux_assistant_message(content: Vec<Content>, stop_reason: StopReason) -> AssistantMessage {
    let output = estimate_output(&content);
    AssistantMessage {
        content,
        provider: ProviderId::from("faux"),
        model: "faux-1".to_string(),
        api: Some("faux".into()),
        response_model: None,
        response_id: None,
        usage: Usage { output, total_tokens: output, ..Default::default() },
        stop_reason,
        error_message: None,
        timestamp: 0,
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::stream::collect_message;
    use futures::StreamExt;

    #[tokio::test]
    async fn streams_scripted_text_and_done() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(vec![faux_text("hello")], StopReason::Stop)]);
        let model = faux.model().clone();
        let stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let msg = collect_message(stream).await;
        assert_eq!(msg.stop_reason, StopReason::Stop);
        assert_eq!(msg.content, vec![faux_text("hello")]);
        assert!(msg.usage.output > 0);
        assert!(msg.usage.total_tokens >= msg.usage.output);
        assert_eq!(faux.call_count(), 1);
        assert_eq!(faux.pending_count(), 0);
    }

    #[tokio::test]
    async fn empty_queue_yields_error_message() {
        let faux = FauxProvider::new();
        let model = faux.model().clone();
        let stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let msg = collect_message(stream).await;
        assert_eq!(msg.stop_reason, StopReason::Error);
        assert!(msg.error_message.is_some());
    }

    #[tokio::test]
    async fn event_ordering_is_start_blocks_terminal() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![faux_assistant_message(
            vec![faux_thinking("t"), faux_tool_call("echo", serde_json::json!({"x": 1}))],
            StopReason::ToolUse,
        )]);
        let model = faux.model().clone();
        let mut stream = faux.stream(&model, &Context::default(), &StreamOptions::default());
        let mut events = Vec::new();
        while let Some(ev) = stream.next().await {
            events.push(ev);
        }
        assert!(matches!(events.first(), Some(StreamEvent::Start)));
        assert!(matches!(events.last(), Some(StreamEvent::Done { .. })));
        // thinking block before tool-call block, each start→delta→end
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ThinkingStart { content_index: 0 })));
        assert!(events.iter().any(|e| matches!(e, StreamEvent::ToolCallEnd { content_index: 1, .. })));
    }

    #[tokio::test]
    async fn responses_consumed_in_order() {
        let faux = FauxProvider::new();
        faux.set_responses(vec![
            faux_assistant_message(vec![faux_text("first")], StopReason::Stop),
            faux_assistant_message(vec![faux_text("second")], StopReason::Stop),
        ]);
        assert_eq!(faux.pending_count(), 2);
        let model = faux.model().clone();
        let m1 = collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default())).await;
        let m2 = collect_message(faux.stream(&model, &Context::default(), &StreamOptions::default())).await;
        assert_eq!(m1.content, vec![faux_text("first")]);
        assert_eq!(m2.content, vec![faux_text("second")]);
        assert_eq!(faux.call_count(), 2);
    }
}
