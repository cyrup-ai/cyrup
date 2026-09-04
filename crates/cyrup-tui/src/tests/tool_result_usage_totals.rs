//! AGENT-005 accounting: a tool that reports usage for its own execution must reach the footer's
//! cumulative session totals.
//!
//! Pi's footer recomputes the totals from ALL session entries on every render
//! (`modes/interactive/components/footer.ts:88-104`), with an explicit `toolResult` branch:
//!
//! ```text
//! } else if (entry.type === "message" && entry.message.role === "toolResult" && entry.message.usage) {
//!     addUsageToTotals(usageTotals, entry.message.usage);
//! }
//! ```
//!
//! cyrup accumulates incrementally instead, and had exactly ONE `add_usage` call site — the
//! assistant `message_end` arm — so a tool's usage was billed but never displayed.
//!
//! Note the deliberate asymmetry, which these tests pin: upstream writes `latestCacheHitRate`
//! ONLY inside the assistant branch, so a `toolResult` must not restate the footer's `CH` segment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{App, UiTheme};
use cyrup_agent::{AgentMessage, ToolResultMessage};
use cyrup_core::{Content, ToolCallId, Usage};
use cyrup_session_svc::AgentSessionEvent;
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

fn usage(input: u64, output: u64, cache_read: u64) -> Usage {
    Usage {
        input,
        output,
        cache_read,
        total_tokens: input + output + cache_read,
        ..Usage::default()
    }
}

fn tool_result(usage: Option<Usage>) -> AgentMessage {
    AgentMessage::ToolResult(ToolResultMessage {
        tool_call_id: ToolCallId::from("c1"),
        tool_name: "summarize".to_string(),
        content: vec![Content::text("summarized")],
        is_error: false,
        details: None,
        timestamp: 0,
        usage,
        added_tool_names: Vec::new(),
    })
}

#[test]
fn a_tool_result_usage_joins_the_cumulative_footer_totals() {
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: tool_result(Some(usage(30, 7, 5))),
    });

    let u = &app.state().status.usage;
    assert_eq!(u.input, 30, "tool input tokens counted");
    assert_eq!(u.output, 7, "tool output tokens counted");
    assert_eq!(u.cache_read, 5, "tool cache-read tokens counted");
    assert_eq!(u.total_tokens, 42);
}

#[test]
fn tool_results_accumulate_across_calls() {
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: tool_result(Some(usage(10, 1, 0))),
    });
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: tool_result(Some(usage(20, 2, 0))),
    });
    assert_eq!(app.state().status.usage.input, 30);
    assert_eq!(app.state().status.usage.output, 3);
}

#[test]
fn a_tool_result_without_usage_changes_nothing() {
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: tool_result(None),
    });
    assert_eq!(
        app.state().status.usage,
        Usage::default(),
        "absent usage adds nothing"
    );
}

/// Upstream sets `latestCacheHitRate` only in the ASSISTANT branch. A tool result must leave the
/// footer's `CH` segment exactly as the last assistant turn set it — otherwise a tool with no
/// prompt tokens would blank a real cache-hit reading.
#[test]
fn a_tool_result_does_not_restate_the_latest_cache_hit_rate() {
    let mut app = new_app();
    // An assistant turn establishes the reading: 25 cache-read out of a 100-token prompt.
    app.status_mut().add_usage(&usage(75, 4, 25));
    let established = app.state().status.latest_cache_hit;
    assert_eq!(
        established,
        Some(25.0),
        "the assistant turn set the CH reading"
    );

    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: tool_result(Some(usage(0, 9, 0))),
    });

    assert_eq!(
        app.state().status.latest_cache_hit,
        established,
        "a tool result leaves the latest-turn cache-hit rate alone"
    );
    // …while still contributing to the cumulative totals.
    assert_eq!(app.state().status.usage.output, 13);
}

/// A non-`toolResult` message must not be mistaken for one by the serde projection.
#[test]
fn an_assistant_message_end_is_not_double_counted_by_the_tool_result_branch() {
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::MessageEnd {
        message: AgentMessage::user_text("hello"),
    });
    assert_eq!(app.state().status.usage, Usage::default());
}
