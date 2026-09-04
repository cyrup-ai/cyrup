//! The `bash` tool's live elapsed timer — Pi `bash.ts:309-313` (`options.isPartial ? "Elapsed" :
//! "Took"`) plus the 1 s `setInterval(() => context.invalidate())` its `renderResult` arms while the
//! result is still partial (`bash.ts:471-479`).
//!
//! # What was broken
//!
//! cyrup keyed the duration footer on `ToolRun::duration_ms`, which is written only when the call
//! SETTLES. A running command therefore rendered no duration line at all — `grep -rn Elapsed
//! crates/cyrup-tui/src` had zero hits — so a ten-minute build showed nothing to say it was alive,
//! and the number appeared only after the fact.
//!
//! These tests drive the assembled `App` through the same transcript calls
//! `App::ingest_event_rendered_owned` makes for `ToolExecutionStart` / `ToolExecutionUpdate` /
//! `ToolExecutionEnd` (`app.rs`), and assert on the rendered frame.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{App, UiTheme};
use ratatui::backend::TestBackend;
use serde_json::json;

/// The whole rendered buffer as text: a settled tool block is flushed to scrollback, so asserting on
/// the live region alone would miss the `Took` frame.
fn all_text(app: &App<TestBackend>) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out.push_str(&app.scrollback_text());
    out
}

/// Start a `bash` call the way `ToolExecutionStart` does, then deliver bash's initial EMPTY update
/// (`bash.ts:384-385`, ported at `cyrup-tools/src/tools/bash.rs:170`) exactly as
/// `ToolExecutionUpdate` does.
fn start_running_bash(app: &mut App<TestBackend>, command: &str) {
    app.transcript_mut().push_tool_start_rendered(
        "bash",
        Some("call-1".to_string()),
        json!({ "command": command }),
        None,
    );
    app.transcript_mut().push_tool_update(
        Some("call-1"),
        Some(json!({ "content": [], "details": null })),
    );
}

/// THE regression: a still-running command shows a live `Elapsed …`, never `Took`.
#[test]
fn a_running_bash_call_renders_elapsed_not_took() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    start_running_bash(&mut app, "cargo build --release");
    app.draw().unwrap();

    let text = all_text(&app);
    assert!(
        text.contains("$ cargo build --release"),
        "bash header missing:\n{text}"
    );
    assert!(
        text.contains("Elapsed "),
        "no live elapsed timer while running:\n{text}"
    );
    assert!(
        !text.contains("Took "),
        "a running call claimed it had finished:\n{text}"
    );
}

/// …and the settled frame flips to `Took` and drops `Elapsed` — Pi's label is a pure function of
/// `isPartial`, so exactly one of the two is ever on screen for a given call.
#[test]
fn a_settled_bash_call_renders_took_not_elapsed() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    start_running_bash(&mut app, "echo hi");
    app.transcript_mut().push_tool_end_rendered(
        "bash",
        Some("call-1"),
        false,
        Some(json!({ "content": [{ "type": "text", "text": "hi" }] })),
        None,
    );
    app.draw().unwrap();

    let text = all_text(&app);
    assert!(
        text.contains("Took "),
        "settled call has no duration:\n{text}"
    );
    assert!(
        !text.contains("Elapsed "),
        "settled call still shows the live timer:\n{text}"
    );
}

/// MIRROR (green before and after the fix): the `$ command` header and the tool output are unrelated
/// to the duration line, so a failure of the two tests above cannot be a general rendering outage.
#[test]
fn the_bash_body_renders_independently_of_the_duration_line() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    start_running_bash(&mut app, "ls -la");
    app.transcript_mut().push_tool_update(
        Some("call-1"),
        Some(json!({ "content": [{ "type": "text", "text": "total 4\ndrwxr-xr-x" }] })),
    );
    app.draw().unwrap();

    let text = all_text(&app);
    assert!(text.contains("$ ls -la"), "bash header missing:\n{text}");
    assert!(
        text.contains("total 4"),
        "streamed partial output missing:\n{text}"
    );
}

/// The repaint driver: `App::run`'s elapsed tick is `if`-gated on this predicate, which is Pi's
/// interval-arming condition (`state.startedAt !== undefined && options.isPartial`, bash.ts:471) and
/// its clear (`:475-479`). Without it the `Elapsed` figure would only advance when some unrelated
/// event happened to redraw.
#[test]
fn the_elapsed_tick_is_armed_only_while_a_bash_call_runs() {
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    assert!(
        !app.transcript_mut().has_running_elapsed_tool(),
        "an idle session armed the elapsed tick"
    );

    // A non-bash tool must not arm it — Pi arms the interval from bash's OWN renderResult.
    app.transcript_mut().push_tool_start_rendered(
        "read",
        Some("call-r".to_string()),
        json!({ "file_path": "/tmp/x" }),
        None,
    );
    app.transcript_mut()
        .push_tool_update(Some("call-r"), Some(json!({ "content": [] })));
    assert!(
        !app.transcript_mut().has_running_elapsed_tool(),
        "a running `read` armed the bash elapsed tick"
    );

    start_running_bash(&mut app, "sleep 30");
    assert!(
        app.transcript_mut().has_running_elapsed_tool(),
        "a running bash call did not arm the elapsed tick"
    );

    app.transcript_mut().push_tool_end_rendered(
        "bash",
        Some("call-1"),
        false,
        Some(json!({ "content": [] })),
        None,
    );
    assert!(
        !app.transcript_mut().has_running_elapsed_tool(),
        "the elapsed tick stayed armed after the call settled"
    );
}
