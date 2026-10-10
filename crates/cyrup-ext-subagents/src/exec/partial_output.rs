//! SUBA-195 — unfinished assistant text recovered when a child ends abnormally. Port of pi
//! `src/runs/shared/partial-output.ts` @ad11b7ab (`9bc8f2d1` / #2653).
//!
//! Final output is built from completed `message_end` messages
//! ([`crate::exec::output::extract_final_output`]), so text that was still streaming when a child
//! timed out or exited on an error is otherwise lost. The tracker holds the latest unfinished
//! assistant text of one attempt; [`crate::exec::run_sync`] reads it only when the attempt ended
//! abnormally. Nothing is persisted.
//!
//! Upstream's tracker, verbatim in its decision:
//!
//! ```ts
//! observe(event) {
//!     if ((event.type !== "message_update" && event.type !== "message_end") || !isAssistant(event.message)) return;
//!     // A completed reply, including a tool-only one, is already part of the final output.
//!     // A provider-error message is skipped there, so its text is the newest unfinished text.
//!     latest = event.type === "message_update" || isErroredAssistant(event.message) ? event.message : undefined;
//! },
//! text() {
//!     const text = extractTextFromContent(latest?.content);
//!     return text.trim() ? text : undefined;
//! },
//! ```
//!
//! **[CYRUP-DELTA] in mechanism, not in decision.** Upstream reads the cumulative `message`
//! snapshot every in-process `message_update` carries. A cyrup child speaks the json-mode wire,
//! whose `message_update` is pi's `toJsonEvent` projection: no `message`, and no
//! `assistantMessageEvent.partial` (see [`crate::exec::ndjson::SubagentEvent::MessageUpdate`]).
//! The snapshot's text is therefore rebuilt from the deltas: `text_delta`s accumulate per
//! `contentIndex`, a `text_end` replaces its block with the authoritative `content`, and an
//! assistant `message_start` begins a fresh accumulation. The text extracted at the end is the one
//! upstream's `extractTextFromContent` would give for the same message: its text blocks, in
//! content order, joined by `"\n"` (`src/shared/utils.ts:579-603` @ad11b7ab). The
//! `message_end` arms read the completed message itself, exactly as upstream does.

use std::collections::BTreeMap;

use crate::exec::ndjson::SubagentEvent;

/// pi `PartialOutputCause` — why a child's streamed text is being kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialOutputCause {
    /// The run deadline or a per-tool deadline fired.
    Timeout,
    /// The child process ended on an error of its own (see `run_sync`'s gate for what counts).
    ChildError,
}

impl PartialOutputCause {
    fn label(self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::ChildError => "child error",
        }
    }
}

/// pi `formatPartialOutput(text, cause)`: `Partial output before ${cause}:\n${text}`.
#[must_use]
pub fn format_partial_output(text: &str, cause: PartialOutputCause) -> String {
    format!("Partial output before {}:\n{text}", cause.label())
}

/// What upstream's `latest` reference points at.
#[derive(Debug, Clone, Default)]
enum Latest {
    /// Nothing unfinished: no assistant text yet, or the last assistant message completed cleanly.
    #[default]
    None,
    /// The message currently streaming — its text is [`PartialOutputTracker::streaming`].
    Streaming,
    /// A fixed text: an errored assistant `message_end`'s content, or a streaming message that a
    /// new `message_start` superseded before its own first update arrived (upstream's reference
    /// keeps pointing at the old message until that update).
    Fixed(String),
}

/// pi `PartialOutputTracker`, one per attempt (it lives on [`crate::exec::AgentProgress`], which
/// is per attempt, as upstream creates one per `runSingleAttempt`).
#[derive(Debug, Clone, Default)]
pub struct PartialOutputTracker {
    /// The streaming assistant message's text blocks, by `contentIndex`.
    streaming: BTreeMap<u64, String>,
    latest: Latest,
}

impl PartialOutputTracker {
    /// pi `observe(event)`.
    pub fn observe(&mut self, event: &SubagentEvent) {
        match event {
            SubagentEvent::MessageStart { message } if is_assistant(message) => {
                if matches!(self.latest, Latest::Streaming) {
                    self.latest = Latest::Fixed(self.streaming_text());
                }
                self.streaming.clear();
            }
            // The json-mode projection emits `message_update` for assistant messages only
            // (`toJsonEvent` throws on any other role), so upstream's role check has nothing left
            // to reject here.
            SubagentEvent::MessageUpdate {
                assistant_message_event,
            } => {
                self.apply_delta(assistant_message_event);
                self.latest = Latest::Streaming;
            }
            SubagentEvent::MessageEnd { message } if is_assistant(message) => {
                self.streaming.clear();
                // A completed reply, including a tool-only one, is already part of the final
                // output. A provider-error message is skipped there, so its text is the newest
                // unfinished text.
                self.latest = if is_errored_assistant(message) {
                    Latest::Fixed(
                        message
                            .get("content")
                            .map(crate::tui::events::extract_event_text)
                            .unwrap_or_default(),
                    )
                } else {
                    Latest::None
                };
            }
            _ => {}
        }
    }

    /// pi `text()`: the latest unfinished assistant text that is newer than the last completed
    /// reply, or `None` when it is blank.
    #[must_use]
    pub fn text(&self) -> Option<String> {
        let text = match &self.latest {
            Latest::None => return None,
            Latest::Streaming => self.streaming_text(),
            Latest::Fixed(text) => text.clone(),
        };
        (!text.trim().is_empty()).then_some(text)
    }

    fn streaming_text(&self) -> String {
        self.streaming
            .values()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Fold one projected `assistantMessageEvent` into the streaming message's text blocks. Only
    /// the three text events carry text; thinking and tool-call blocks are not text upstream's
    /// extraction would return.
    fn apply_delta(&mut self, event: &serde_json::Value) {
        let Some(index) = event
            .get("contentIndex")
            .and_then(serde_json::Value::as_u64)
        else {
            return;
        };
        let field = |name: &str| event.get(name).and_then(serde_json::Value::as_str);
        match event.get("type").and_then(serde_json::Value::as_str) {
            Some("text_start") => {
                self.streaming.entry(index).or_default();
            }
            Some("text_delta") => {
                if let Some(delta) = field("delta") {
                    self.streaming.entry(index).or_default().push_str(delta);
                }
            }
            Some("text_end") => {
                if let Some(content) = field("content") {
                    self.streaming.insert(index, content.to_string());
                }
            }
            _ => {}
        }
    }
}

/// pi `isAssistant(message)`.
fn is_assistant(message: &serde_json::Value) -> bool {
    message.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
}

/// pi `isErroredAssistant(message)`: `stopReason === "error"`, or a non-empty string
/// `errorMessage`. Deliberately NOT [`SubagentEvent::is_error_or_aborted_message`]: an `aborted`
/// stop is not an error to upstream's tracker, so an aborted reply clears it like any other
/// completed one.
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    //! Ports of upstream's `test/unit/partial-output.test.ts` (`9bc8f2d1`), restated over the
    //! delta wire a cyrup child actually emits, plus the delta-specific cases.

    use super::*;
    use serde_json::json;

    fn start() -> SubagentEvent {
        SubagentEvent::MessageStart {
            message: json!({"role": "assistant", "content": []}),
        }
    }

    fn delta(index: u64, text: &str) -> SubagentEvent {
        SubagentEvent::MessageUpdate {
            assistant_message_event: json!({"type": "text_delta", "contentIndex": index, "delta": text}),
        }
    }

    fn end(text: &str, extra: serde_json::Value) -> SubagentEvent {
        let mut message = json!({"role": "assistant", "content": [{"type": "text", "text": text}]});
        if let (Some(target), Some(extra)) = (message.as_object_mut(), extra.as_object()) {
            target.extend(extra.clone());
        }
        SubagentEvent::MessageEnd { message }
    }

    #[test]
    fn keeps_the_latest_streamed_assistant_text() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&delta(0, "one"));
        tracker.observe(&delta(0, " two"));
        assert_eq!(tracker.text().as_deref(), Some("one two"));
    }

    #[test]
    fn drops_the_text_once_the_assistant_message_completes() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&delta(0, "one two"));
        tracker.observe(&end("one two", json!({"stopReason": "stop"})));
        assert_eq!(tracker.text(), None);
    }

    /// Upstream's "clear partial output on completed tool-only replies" half of `9bc8f2d1`.
    #[test]
    fn a_completed_tool_only_reply_clears_an_earlier_failed_messages_text() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&end(
            "cut off",
            json!({"stopReason": "error", "errorMessage": "overloaded"}),
        ));
        tracker.observe(&SubagentEvent::MessageEnd {
            message: json!({"role": "assistant", "stopReason": "toolUse",
                "content": [{"type": "toolCall", "id": "c1", "name": "bash", "arguments": {}}]}),
        });
        assert_eq!(tracker.text(), None);
    }

    #[test]
    fn ignores_user_and_tool_result_messages() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&delta(0, "streaming"));
        tracker.observe(&SubagentEvent::MessageEnd {
            message: json!({"role": "user", "content": [{"type": "text", "text": "hi"}]}),
        });
        tracker.observe(&SubagentEvent::MessageEnd {
            message: json!({"role": "toolResult", "content": [{"type": "text", "text": "out"}]}),
        });
        tracker.observe(&SubagentEvent::MessageStart {
            message: json!({"role": "user", "content": []}),
        });
        assert_eq!(tracker.text().as_deref(), Some("streaming"));
    }

    #[test]
    fn keeps_the_text_of_a_failed_provider_message_which_final_output_skips() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&end(
            "cut off",
            json!({"stopReason": "error", "errorMessage": "overloaded"}),
        ));
        assert_eq!(tracker.text().as_deref(), Some("cut off"));
    }

    /// `isErroredAssistant` is `stopReason === "error"` or a non-empty `errorMessage`; an aborted
    /// stop is neither.
    #[test]
    fn an_aborted_reply_clears_like_a_completed_one() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&delta(0, "half"));
        tracker.observe(&end("half", json!({"stopReason": "aborted"})));
        assert_eq!(tracker.text(), None);
    }

    #[test]
    fn labels_the_cause() {
        assert_eq!(
            format_partial_output("text", PartialOutputCause::Timeout),
            "Partial output before timeout:\ntext"
        );
        assert_eq!(
            format_partial_output("text", PartialOutputCause::ChildError),
            "Partial output before child error:\ntext"
        );
    }

    /// The delta reconstruction: blocks in content order joined by `"\n"` (upstream's
    /// `extractTextFromContent`), `text_end` authoritative, thinking and tool-call deltas ignored.
    #[test]
    fn rebuilds_the_snapshot_text_from_projected_deltas() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&SubagentEvent::MessageUpdate {
            assistant_message_event: json!({"type": "thinking_delta", "contentIndex": 0, "delta": "hmm"}),
        });
        tracker.observe(&SubagentEvent::MessageUpdate {
            assistant_message_event: json!({"type": "text_start", "contentIndex": 1}),
        });
        tracker.observe(&delta(1, "first bl"));
        tracker.observe(&SubagentEvent::MessageUpdate {
            assistant_message_event: json!({"type": "text_end", "contentIndex": 1, "content": "first block"}),
        });
        tracker.observe(&delta(3, "second"));
        tracker.observe(&SubagentEvent::MessageUpdate {
            assistant_message_event: json!({"type": "toolcall_delta", "contentIndex": 2, "delta": "{\"a\""}),
        });
        assert_eq!(tracker.text().as_deref(), Some("first block\nsecond"));
    }

    /// A thinking-only stream has no text: upstream's `text()` is `undefined` for it.
    #[test]
    fn a_blank_stream_is_no_partial_output() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&SubagentEvent::MessageUpdate {
            assistant_message_event: json!({"type": "thinking_delta", "contentIndex": 0, "delta": "hmm"}),
        });
        tracker.observe(&delta(1, "  \n"));
        assert_eq!(tracker.text(), None);
    }

    /// A fresh `message_start` begins a new accumulation, and until that message's first update the
    /// previous streaming text stays the newest (upstream's reference has not moved yet).
    #[test]
    fn a_new_message_starts_a_fresh_accumulation() {
        let mut tracker = PartialOutputTracker::default();
        tracker.observe(&start());
        tracker.observe(&delta(0, "old"));
        tracker.observe(&start());
        assert_eq!(tracker.text().as_deref(), Some("old"));
        tracker.observe(&delta(0, "new"));
        assert_eq!(tracker.text().as_deref(), Some("new"));
    }
}
