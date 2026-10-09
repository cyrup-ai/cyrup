//! The bash `Took …` footer from the RESULT's recorded duration (pi commit 36a686ee8, #10549):
//! *"A final result's recorded duration wins: it is monotonic and survives reloads. The renderer's
//! own clock is the fallback for live progress and for results stored without one."*
//! (`core/tools/renderers/bash.ts:102-106` @v1.1.0). pi's test drives a live result whose wall clock
//! jumped an hour and a restored one; both read `Took 4.2s`. The replayed half is
//! `tests::recorded_duration`.

#![allow(clippy::unwrap_used, clippy::panic)]

use crate::transcript::*;

fn rows(view: &mut TranscriptView) -> Vec<String> {
    view.commit_tools();
    let entry = view.drain_committed().into_iter().next().unwrap();
    entry_lines(&entry, &UiTheme::dark(), 60, 1, ImageOpts::default())
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect()
}

fn settle(view: &mut TranscriptView, recorded_ms: Option<u64>) {
    view.push_tool_start_rendered(
        "bash",
        Some("call-1".to_string()),
        serde_json::json!({ "command": "sleep 4" }),
        None,
    );
    view.push_tool_end_rendered(
        "bash",
        Some("call-1"),
        false,
        Some(serde_json::json!({ "content": [] })),
        None,
        recorded_ms,
    );
}

/// Live: the TUI saw the start, so its own clock measured a few microseconds — the recorded
/// 4.2 s is what shows.
#[test]
fn a_live_result_shows_its_recorded_duration_not_the_tuis_clock() {
    let mut view = TranscriptView::new();
    settle(&mut view, Some(4_200));
    let rows = rows(&mut view);
    assert!(rows.iter().any(|r| r.trim() == "Took 4.2s"), "{rows:#?}");
}

/// A result stored without a duration keeps the old behaviour: the TUI's own clock.
#[test]
fn a_result_without_one_falls_back_to_the_tuis_clock() {
    let mut view = TranscriptView::new();
    settle(&mut view, None);
    let rows = rows(&mut view);
    assert!(rows.iter().any(|r| r.trim() == "Took 0.0s"), "{rows:#?}");
}
