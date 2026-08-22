#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::transcript::*;

fn tool_names(entries: &[Entry]) -> Vec<&str> {
    entries
        .iter()
        .filter_map(|e| match e {
            Entry::Tool(run) => Some(run.name.as_str()),
            _ => None,
        })
        .collect()
}

/// The primary SCREEN-FILL fix: finished tools commit progressively (leaving `active_tools`), so a
/// long multi-tool turn never stacks the whole turn in the live viewport. `content_height` (which
/// sizes the viewport) stays bounded to the running tail.
#[test]
fn finished_tools_commit_progressively_and_content_height_stays_bounded() {
    let theme = UiTheme::dark();
    let mut view = TranscriptView::new();
    let tall_before = {
        // Simulate 20 finished tool calls arriving one at a time (the reported storm).
        for i in 0..20u32 {
            let name = format!("read_{i}");
            view.push_tool_start(name.clone(), serde_json::json!({ "path": format!("file_{i}.md") }));
            view.push_tool_end(
                name,
                false,
                Some(format!("body of file {i}\nsecond line\nthird").into()),
            );
            // The app drains finished-leading tools after every ToolExecutionEnd.
            view.commit_finished_leading_tools();
        }
        view.content_height(80, &theme)
    };
    // All 20 committed to `pending` (headed for native scrollback), none left live.
    assert_eq!(view.active_tools().len(), 0, "finished tools must not accumulate live");
    assert_eq!(tool_names(view.pending()).len(), 20, "all finished tools should be committed");
    // The live region measured near-empty (no tail): bounded, not full-screen.
    assert!(tall_before <= 1, "content_height ballooned to {tall_before}; must stay bounded");
    // Commit order equals call order in scrollback.
    assert_eq!(tool_names(view.pending()).first().copied(), Some("read_0"));
    assert_eq!(tool_names(view.pending()).last().copied(), Some("read_19"));
}

/// Only the LEADING run of finished tools commits: a still-running earlier tool blocks committing a
/// finished later one ahead of it, so scrollback order = call order even when tools interleave.
#[test]
fn only_leading_finished_run_commits_running_tool_blocks() {
    let mut view = TranscriptView::new();
    view.push_tool_start("a", Value::Null); // will stay running
    view.push_tool_start("b", Value::Null);
    view.push_tool_start("c", Value::Null);
    // `b` finishes first, but `a` is still running ahead of it.
    view.push_tool_end("b", false, Some("b-result".into()));
    view.commit_finished_leading_tools();
    assert!(view.pending().is_empty(), "nothing commits while the leading tool `a` runs");
    assert_eq!(view.active_tools().len(), 3, "all three stay live until `a` finishes");

    // `a` finishes → the leading run `a`, `b` commits (in order), `c` stays live.
    view.push_tool_end("a", false, Some("a-result".into()));
    view.commit_finished_leading_tools();
    assert_eq!(tool_names(view.pending()), vec!["a", "b"], "leading finished run commits in order");
    assert_eq!(view.active_tools().len(), 1, "still-running `c` stays live");
    assert_eq!(view.active_tools()[0].name, "c");
}

/// The `streaming.is_none()` guard: a finished tool never commits ahead of uncommitted assistant
/// text of the same step (SCROLLBACK-ORDER safety).
#[test]
fn streaming_partial_blocks_tool_commit() {
    let mut view = TranscriptView::new();
    view.push_assistant_delta("thinking about the next step");
    view.push_tool_start("read", Value::Null);
    view.push_tool_end("read", false, Some("result".into()));
    view.commit_finished_leading_tools();
    assert!(view.pending().is_empty(), "tool must not commit while assistant text is streaming");
    assert_eq!(view.active_tools().len(), 1, "the finished tool stays live behind the stream");

    // Once the assistant text commits (streaming cleared), the tool is free to commit after it.
    view.commit_assistant(None);
    view.commit_finished_leading_tools();
    assert!(
        matches!(view.pending().first(), Some(Entry::Assistant(_))),
        "assistant text commits before the tool row"
    );
    assert_eq!(tool_names(view.pending()), vec!["read"], "the tool commits after the stream");
}
