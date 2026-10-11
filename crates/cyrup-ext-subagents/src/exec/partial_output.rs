//! SUBA-195 — unfinished assistant text recovered when a child ends abnormally: pi-subagents'
//! partial-output tracker (`src/runs/shared/partial-output.ts` @ad11b7ab, `9bc8f2d1` / #2653).
//!
//! Upstream's module doc, verbatim:
//!
//! > Final output is built from completed `message_end` messages, so text that was still streaming
//! > when a child timed out or its session threw is otherwise lost. The tracker keeps one in-memory
//! > reference to the latest unfinished assistant message. Nothing is persisted.
//!
//! cyrup's final output is built the same way ([`crate::exec::output::extract_final_output`] reads
//! only `message_end`s), so the loss is the same and so is the fix: one tracker per attempt, fed
//! every parsed child event at the crate's one parse point
//! ([`crate::exec::drive_attempt`]'s `handle_child_line`, upstream `partialOutput.observe(evt)` in
//! `processEvent`, `src/runs/foreground/execution.ts` and `src/runs/background/run-child-session.ts`).
//!
//! # `[CYRUP-DELTA]` the wire carries deltas, not snapshots
//!
//! Upstream's `observe` keeps `event.message` off each `message_update` — the cumulative
//! partial-message snapshot an in-process child session hands it. cyrup's child is a
//! `cyrup --print --mode json` re-exec whose `message_update` is the DELTA-ONLY projection
//! (`crates/cyrup-modes/src/json_event.rs`, `JsonAgentSessionEvent`: `{type, usage,
//! assistantMessageEvent}`, with both the outer `message` and the inner `partial` dropped; see
//! [`crate::exec::ndjson::SubagentEvent::MessageUpdate`]). The projection's own contract
//! (`coding-agent/docs/rpc.md:952-956`) names the substitute: *"Clients that need a live partial
//! message must assemble it from `message_start` and subsequent events using `contentIndex`."*
//! So the in-flight message is ASSEMBLED here — seeded from the `message_start` message's text
//! blocks, `text_delta` appended per `contentIndex`, `text_end`'s `content` taken as that block's
//! final text — instead of being referenced. Only `text` blocks are kept, which is exactly what
//! upstream's `extractTextFromContent` reads off the snapshot (`type: "text"` parts; a thinking or
//! tool-call block carries no `text` key).
//!
//! Every other rule is upstream's, in upstream's order:
//!
//! * a `message_update` makes the in-flight assistant message the latest unfinished text;
//! * a completed assistant `message_end` — INCLUDING a tool-only one — CLEARS it ("A completed reply,
//!   including a tool-only one, is already part of the final output.");
//! * an ERRORED assistant `message_end` (`stopReason === "error"` or a non-empty `errorMessage`)
//!   becomes the latest instead ("A provider-error message is skipped there, so its text is the
//!   newest unfinished text.");
//! * a non-assistant message is ignored; text is extracted only when asked, and blank text is no
//!   text (`text.trim() ? text : undefined`).

use std::collections::BTreeMap;

use crate::exec::ndjson::SubagentEvent;

/// pi `PartialOutputCause` (`partial-output.ts`): why the child ended abnormally. Decides the label
/// [`format_partial_output`] writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialOutputCause {
    /// The run deadline or a per-tool deadline killed the child.
    Timeout,
    /// The child failed without settling — upstream's thrown child-session/prompt error. See
    /// [`crate::exec::drive_attempt`]'s `DriveState::outcome` for cyrup's mapping of "threw" onto a
    /// subprocess.
    ChildError,
}

impl PartialOutputCause {
    /// The literal upstream interpolates: `"timeout" | "child error"`.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ChildError => "child error",
        }
    }
}

/// pi `formatPartialOutput(text, cause)` (`partial-output.ts`):
/// `` `Partial output before ${cause}:\n${text}` ``.
#[must_use]
pub fn format_partial_output(text: &str, cause: PartialOutputCause) -> String {
    format!("Partial output before {}:\n{text}", cause.label())
}

/// What upstream's `latest` reference points at.
#[derive(Debug, Clone)]
enum Latest {
    /// The assistant message still streaming: its text blocks by `contentIndex`.
    Streaming(BTreeMap<u64, String>),
    /// An errored assistant `message_end`'s extracted text.
    Errored(String),
}

/// pi `createPartialOutputTracker()` (`partial-output.ts`). One per attempt; in memory only.
#[derive(Debug, Clone, Default)]
pub struct PartialOutputTracker {
    /// The text blocks of the assistant message whose `message_start` was the last one seen and
    /// whose first `message_update` has not yet arrived. Held apart from [`Self::latest`] because
    /// upstream only moves `latest` on `message_update`/`message_end`: a bare `message_start` must
    /// not discard an earlier errored message's text.
    started: Option<BTreeMap<u64, String>>,
    latest: Option<Latest>,
}

fn is_assistant(message: &serde_json::Value) -> bool {
    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
}

/// pi `isErroredAssistant`: `stopReason === "error" || (typeof errorMessage === "string" &&
/// errorMessage.length > 0)`.
fn is_errored_assistant(message: &serde_json::Value) -> bool {
    message
        .get("stopReason")
        .and_then(serde_json::Value::as_str)
        == Some("error")
        || message
            .get("errorMessage")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|text| !text.is_empty())
}

/// A message's `text` content blocks keyed by their position — the `contentIndex` every later
/// stream event addresses.
fn text_blocks(message: &serde_json::Value) -> BTreeMap<u64, String> {
    let Some(parts) = message.get("content").and_then(serde_json::Value::as_array) else {
        return BTreeMap::new();
    };
    (0_u64..)
        .zip(parts)
        .filter(|(_, part)| part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
        .filter_map(|(index, part)| {
            part.get("text")
                .and_then(serde_json::Value::as_str)
                .map(|text| (index, text.to_string()))
        })
        .collect()
}

impl PartialOutputTracker {
    /// A tracker that has seen nothing.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// pi `observe(event)`.
    pub fn observe(&mut self, event: &SubagentEvent) {
        match event {
            SubagentEvent::MessageStart { message } if is_assistant(message) => {
                self.started = Some(text_blocks(message));
            }
            // cyrup's json mode only ever emits `message_update` for an assistant message (the
            // projection refuses any other role, `json_event.rs`), so upstream's `isAssistant`
            // gate has nothing left to test on the delta-only record.
            SubagentEvent::MessageUpdate {
                assistant_message_event,
            } => self.apply_update(assistant_message_event),
            SubagentEvent::MessageEnd { message } if is_assistant(message) => {
                self.started = None;
                // `latest = isErroredAssistant(event.message) ? event.message : undefined`.
                self.latest = is_errored_assistant(message).then(|| {
                    Latest::Errored(crate::tui::events::extract_event_text(
                        message.get("content").unwrap_or(&serde_json::Value::Null),
                    ))
                });
            }
            _ => {}
        }
    }

    /// `latest = event.message` for a `message_update`, assembled from the delta.
    fn apply_update(&mut self, update: &serde_json::Value) {
        // The first update of a freshly started message replaces whatever `latest` held; every
        // later one extends the same message.
        if let Some(seed) = self.started.take() {
            self.latest = Some(Latest::Streaming(seed));
        }
        if !matches!(self.latest, Some(Latest::Streaming(_))) {
            self.latest = Some(Latest::Streaming(BTreeMap::new()));
        }
        let Some(Latest::Streaming(blocks)) = self.latest.as_mut() else {
            return;
        };
        let Some(index) = update
            .get("contentIndex")
            .and_then(serde_json::Value::as_u64)
        else {
            return;
        };
        let text = |key: &str| {
            update
                .get(key)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
        };
        match update.get("type").and_then(serde_json::Value::as_str) {
            Some("text_start") => {
                blocks.entry(index).or_default();
            }
            Some("text_delta") => blocks.entry(index).or_default().push_str(text("delta")),
            Some("text_end") => {
                blocks.insert(index, text("content").to_string());
            }
            _ => {}
        }
    }

    /// pi `text()`: the latest unfinished assistant text newer than the last completed reply, or
    /// `None` when there is none or it is blank. Text blocks join with `"\n"`, as
    /// `extractTextFromContent` joins them.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let text = match self.latest.as_ref()? {
            Latest::Streaming(blocks) => blocks.values().cloned().collect::<Vec<_>>().join("\n"),
            Latest::Errored(text) => text.clone(),
        };
        (!text.trim().is_empty()).then_some(text)
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
    //! Upstream's unit cases (`test/unit/partial-output.test.ts`, `9bc8f2d1`: keeps the latest
    //! streamed text, drops it on completion, ignores user/tool-result messages, keeps a failed
    //! provider message's text, labels the cause), re-expressed on cyrup's delta-only wire, plus the
    //! assembly rules that wire needs. The run-level behaviour is proven against real child
    //! processes in `crate::tests::partial_output_integration`.

    use super::*;

    fn event(json: serde_json::Value) -> SubagentEvent {
        crate::exec::ndjson::parse_line(&json.to_string()).expect("parses")
    }

    fn start() -> SubagentEvent {
        event(serde_json::json!({
            "type": "message_start",
            "message": { "role": "assistant", "content": [] }
        }))
    }

    fn delta(index: u64, text: &str) -> SubagentEvent {
        event(serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "contentIndex": index, "delta": text }
        }))
    }

    fn end(content: serde_json::Value, extra: serde_json::Value) -> SubagentEvent {
        let mut message = serde_json::json!({ "role": "assistant", "content": content });
        if let (Some(m), Some(e)) = (message.as_object_mut(), extra.as_object()) {
            m.extend(e.clone());
        }
        event(serde_json::json!({ "type": "message_end", "message": message }))
    }

    #[test]
    fn streamed_deltas_assemble_per_content_index() {
        let mut tracker = PartialOutputTracker::new();
        tracker.observe(&start());
        tracker.observe(&delta(0, "half "));
        tracker.observe(&delta(1, "second"));
        tracker.observe(&delta(0, "an answer"));
        assert_eq!(tracker.text().as_deref(), Some("half an answer\nsecond"));
    }

    #[test]
    fn a_completed_reply_clears_and_a_tool_only_one_does_too() {
        let mut tracker = PartialOutputTracker::new();
        tracker.observe(&start());
        tracker.observe(&delta(0, "draft"));
        tracker.observe(&end(
            serde_json::json!([{ "type": "toolCall", "id": "c1", "name": "bash" }]),
            serde_json::json!({ "stopReason": "toolUse" }),
        ));
        assert_eq!(tracker.text(), None);
    }

    #[test]
    fn user_and_tool_result_messages_are_ignored() {
        let mut tracker = PartialOutputTracker::new();
        tracker.observe(&start());
        tracker.observe(&delta(0, "streaming"));
        for role in ["user", "toolResult"] {
            tracker.observe(&event(serde_json::json!({
                "type": "message_end",
                "message": { "role": role, "content": [{ "type": "text", "text": "hi" }] }
            })));
        }
        assert_eq!(tracker.text().as_deref(), Some("streaming"));
    }

    #[test]
    fn an_errored_reply_keeps_its_text_until_the_next_update() {
        let mut tracker = PartialOutputTracker::new();
        tracker.observe(&end(
            serde_json::json!([{ "type": "text", "text": "provider died mid" }]),
            serde_json::json!({ "stopReason": "error", "errorMessage": "boom" }),
        ));
        assert_eq!(tracker.text().as_deref(), Some("provider died mid"));
        // A bare `message_start` does not move `latest` upstream.
        tracker.observe(&start());
        assert_eq!(tracker.text().as_deref(), Some("provider died mid"));
        tracker.observe(&delta(0, "retry text"));
        assert_eq!(tracker.text().as_deref(), Some("retry text"));
    }

    #[test]
    fn blank_text_is_no_text_and_text_end_is_authoritative() {
        let mut tracker = PartialOutputTracker::new();
        tracker.observe(&start());
        tracker.observe(&delta(0, "   "));
        assert_eq!(tracker.text(), None);
        tracker.observe(&event(serde_json::json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_end", "contentIndex": 0, "content": "final" }
        })));
        assert_eq!(tracker.text().as_deref(), Some("final"));
    }

    #[test]
    fn the_label_is_upstreams() {
        assert_eq!(
            format_partial_output("x", PartialOutputCause::ChildError),
            "Partial output before child error:\nx"
        );
        assert_eq!(
            format_partial_output("x", PartialOutputCause::Timeout),
            "Partial output before timeout:\nx"
        );
    }
}
