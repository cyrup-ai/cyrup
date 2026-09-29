//! TOOL-050 — the bash `Took …` footer's duration text (`formatDuration`, renderers/bash.ts:32-42
//! @v0.87.1). pi v0.86.0 (#9628) switched a minute or more from `600.0s` to `10m 0s`, and an hour
//! or more to `Xh Ym Zs`; under a minute it is still `toFixed(1)`.

#![allow(clippy::unwrap_used, clippy::panic)]

use crate::transcript::*;

/// Render a settled `bash` block whose frozen duration is `ms` and return its `Took …` row.
fn took_row(ms: u64) -> String {
    let mut view = TranscriptView::new();
    view.push_tool_start("bash", serde_json::json!({ "command": "make" }));
    view.push_tool_end(
        "bash",
        false,
        Some(serde_json::json!({ "content": [{ "type": "text", "text": "ok" }] })),
    );
    view.commit_tools();
    let mut entry = view.drain_committed().into_iter().next().unwrap();
    let Entry::Tool(run) = &mut entry else {
        panic!("a committed bash call is a tool entry");
    };
    run.duration_ms = Some(ms);
    let lines = entry_lines(&entry, &UiTheme::dark(), 60, 1, ImageOpts::default());
    lines
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .find(|r| r.contains("Took "))
        .unwrap()
        .trim()
        .to_string()
}

#[test]
fn a_minute_or_more_renders_minutes_and_whole_seconds() {
    assert_eq!(took_row(60_000), "Took 1m 0s");
    assert_eq!(took_row(600_000), "Took 10m 0s");
    // `Math.floor`, not rounding: 125.999 s is 2m 5s.
    assert_eq!(took_row(125_999), "Took 2m 5s");
    assert_eq!(took_row(3_599_999), "Took 59m 59s");
}

#[test]
fn an_hour_or_more_renders_hours_minutes_and_seconds() {
    assert_eq!(took_row(3_600_000), "Took 1h 0m 0s");
    assert_eq!(took_row(3_725_000), "Took 1h 2m 5s");
    assert_eq!(took_row(90_061_000), "Took 25h 1m 1s");
}

/// MIRROR: under a minute the text is still `toFixed(1)`, including the boundary value that
/// rounds UP to `60.0s` because the branch is taken on the unrounded seconds.
#[test]
fn under_a_minute_keeps_one_decimal() {
    assert_eq!(took_row(0), "Took 0.0s");
    assert_eq!(took_row(1_234), "Took 1.2s");
    assert_eq!(took_row(59_949), "Took 59.9s");
    assert_eq!(took_row(59_950), "Took 60.0s");
}

/// `toFixed` breaks an exact tie toward the larger digit; Rust's `{:.1}` toward the even one.
/// `250 / 1000` and `2_250 / 1000` are exact quarters, so these are the values where they differ.
#[test]
fn exact_ties_round_up_as_to_fixed_does() {
    assert_eq!(took_row(250), "Took 0.3s");
    assert_eq!(took_row(1_250), "Took 1.3s");
    assert_eq!(took_row(2_250), "Took 2.3s");
    assert_eq!(took_row(750), "Took 0.8s");
    // A near-tie that is not exact in binary follows the stored value, as `toFixed` does:
    // 1.15 is 1.149999…, so it rounds down.
    assert_eq!(took_row(1_150), "Took 1.1s");
}
