//! `edit`'s **pre-execution** diff preview — Pi `computeEditsDiff` (edit-diff.ts:514-547) fired from
//! `edit`'s `renderCall` as soon as the streamed arguments are complete (edit.ts:377-386), rendered
//! by `buildEditCallComponent` (`:244-262`) and then superseded by the settled `details.diff`
//! (`setEditPreview` from `renderResult`, `:196-204`).
//!
//! What makes this a *pre*-execution preview and not a nicer post-hoc render is the assertion these
//! tests all make together: the diff is on screen while the file on disk is still untouched. cyrup
//! emits `ToolExecutionStart` before `prepare` — i.e. before the `before_tool_call` permission gate
//! (`cyrup-agent/src/agent.rs:1181/1334`) — so this is what a user reads while deciding whether to
//! approve the write.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use crate::{App, Component, TranscriptView, UiTheme};
use cyrup_core::ToolCallId;
use cyrup_session_svc::AgentSessionEvent;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use serde_json::json;

const BEFORE: &str = "alpha\nbravo\ncharlie\ndelta\n";

/// The live viewport — where a still-PENDING tool renders (ADR-0001: only the active region is
/// repainted each frame).
///
/// Deliberately NOT unioned with [`history`]: `insert_before` paints committed lines into the same
/// `TestBackend` grid on its way out, so a block that has scrolled off is readable in both places
/// and any "appears exactly once" assertion over the union counts it twice. Each assertion below
/// therefore reads the one surface that owns the block at that moment.
fn viewport(app: &App<TestBackend>) -> String {
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
    out
}

/// Native scrollback — where a FINISHED tool lands. `ToolExecutionEnd` runs
/// `commit_finished_leading_tools`, and the next `draw` flushes it out of the viewport.
fn history(app: &App<TestBackend>) -> String {
    app.scrollback_text()
}

fn start_edit(app: &mut App<TestBackend>, call_id: &str, args: serde_json::Value) {
    app.ingest_event(&AgentSessionEvent::ToolExecutionStart {
        tool_call_id: ToolCallId::from(call_id),
        tool_name: "edit".to_string(),
        args,
    });
    app.draw().unwrap();
}

fn end_edit(app: &mut App<TestBackend>, call_id: &str, is_error: bool, result: serde_json::Value) {
    app.ingest_event(&AgentSessionEvent::ToolExecutionEnd {
        tool_call_id: ToolCallId::from(call_id),
        tool_name: "edit".to_string(),
        result,
        is_error,
    });
    app.draw().unwrap();
}

fn text_result(text: &str, details: serde_json::Value) -> serde_json::Value {
    json!({ "content": [{ "type": "text", "text": text }], "details": details })
}

/// A `+`/`-` diff row for `needle` is on screen. `render_diff` prefixes each row with a sign and a
/// line number (`diff.ts:8-12`), so the sign and the text are not adjacent — assert per row.
fn has_diff_row(text: &str, sign: char, needle: &str) -> bool {
    text.lines().any(|l| {
        let t = l.trim_start();
        t.starts_with(sign) && t.contains(needle)
    })
}

fn count_diff_rows(text: &str, sign: char, needle: &str) -> usize {
    text.lines()
        .filter(|l| {
            let t = l.trim_start();
            t.starts_with(sign) && t.contains(needle)
        })
        .count()
}

/// The load-bearing one: the diff renders from the arguments ALONE, with the file still untouched.
#[test]
fn edit_diff_is_rendered_before_the_write_lands() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, BEFORE).unwrap();

    let mut app = App::new(TestBackend::new(72, 24), UiTheme::dark()).unwrap();
    app.set_title_cwd(dir.path().to_path_buf());

    // No `ToolExecutionEnd` — the call is still pending, exactly where a permission prompt sits.
    start_edit(
        &mut app,
        "call-1",
        json!({ "path": "hello.txt", "edits": [{ "oldText": "bravo", "newText": "BRAVO" }] }),
    );

    let text = viewport(&app);
    assert!(
        text.contains("edit hello.txt"),
        "call header missing:\n{text}"
    );
    assert!(
        has_diff_row(&text, '-', "bravo"),
        "removed line not previewed:\n{text}"
    );
    assert!(
        has_diff_row(&text, '+', "BRAVO"),
        "added line not previewed:\n{text}"
    );
    // The whole point: nothing has been written. The preview came from `computeEditsDiff`, which
    // reads the file and applies the edits in memory only.
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        BEFORE,
        "the preview must not have touched the file"
    );
}

/// The legacy top-level `{oldText, newText}` shape Pi's `getRenderablePreviewInput` also accepts
/// (edit.ts:188-190). `ToolExecutionStart` carries the RAW arguments, before the agent preflight
/// runs `prepare_arguments`, so this is the shape that actually arrives for a model that emits it.
#[test]
fn edit_preview_accepts_the_legacy_single_edit_shape() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), BEFORE).unwrap();

    let mut app = App::new(TestBackend::new(72, 24), UiTheme::dark()).unwrap();
    app.set_title_cwd(dir.path().to_path_buf());
    start_edit(
        &mut app,
        "call-legacy",
        json!({ "file_path": "hello.txt", "oldText": "charlie", "newText": "CHARLIE" }),
    );

    let text = viewport(&app);
    assert!(
        has_diff_row(&text, '+', "CHARLIE"),
        "legacy-shape preview missing:\n{text}"
    );
}

/// The preview is visible while pending, and the settled `details.diff` REPLACES it rather than
/// stacking a second copy underneath — Pi's `renderResult` overwrites `callComponent.preview` before
/// `formatEditResult` compares against it, so the diff is drawn exactly once (edit.ts:196-226).
#[test]
fn settled_result_replaces_the_preview_instead_of_duplicating_it() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, BEFORE).unwrap();

    let mut app = App::new(TestBackend::new(72, 24), UiTheme::dark()).unwrap();
    app.set_title_cwd(dir.path().to_path_buf());
    start_edit(
        &mut app,
        "call-2",
        json!({ "path": "hello.txt", "edits": [{ "oldText": "bravo", "newText": "BRAVO" }] }),
    );

    let pending = viewport(&app);
    assert!(
        has_diff_row(&pending, '+', "BRAVO"),
        "no preview while pending:\n{pending}"
    );

    // The tool runs and reports the same diff its own core produced — byte-identical, because the
    // preview went through `apply_edits_to_normalized_content` + `generate_diff_string` too.
    std::fs::write(&file, "alpha\nBRAVO\ncharlie\ndelta\n").unwrap();
    let diff = "-2 bravo\n+2 BRAVO";
    end_edit(
        &mut app,
        "call-2",
        false,
        text_result(
            "Successfully replaced 1 block(s) in hello.txt.",
            json!({ "diff": diff }),
        ),
    );

    let settled = history(&app);
    assert_eq!(
        count_diff_rows(&settled, '+', "BRAVO"),
        1,
        "the diff must be drawn once, not once per source:\n{settled}"
    );
}

/// A preview that CANNOT be produced reports the failure in place of a diff — Pi renders
/// `theme.fg("error", preview.error)` under the header (edit.ts:255-261) — and the identical error
/// coming back on the result is not repeated (`formatEditResult`, `:212-218`).
#[test]
fn unmatchable_edit_previews_its_error_and_the_result_does_not_repeat_it() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.txt"), BEFORE).unwrap();

    let mut app = App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap();
    app.set_title_cwd(dir.path().to_path_buf());
    start_edit(
        &mut app,
        "call-3",
        json!({ "path": "hello.txt", "edits": [{ "oldText": "nowhere", "newText": "x" }] }),
    );

    // `err_not_found` (edit_diff.rs) — the message the tool itself would have produced.
    let expected = "Could not find the exact text in hello.txt.";
    let pending = viewport(&app);
    assert!(
        pending.contains(expected),
        "preview error not shown while pending:\n{pending}"
    );

    end_edit(
        &mut app,
        "call-3",
        true,
        text_result(
            "Could not find the exact text in hello.txt. The old text must match exactly \
             including all whitespace and newlines.",
            json!(null),
        ),
    );
    let settled = history(&app);
    let occurrences = settled.matches("Could not find the exact text").count();
    assert_eq!(
        occurrences, 1,
        "preview error repeated by the result body:\n{settled}"
    );
}

/// MIRROR — stays green with or without the preview. A run that never got one (a replayed history
/// entry, an oversized file, a non-`edit` path) still renders the post-write `details.diff` exactly
/// as it did before. Its job is to show the assertions above are not vacuous: the same
/// render-and-search machinery finds a diff here in both worlds, so when they fail it is the
/// preview that is missing, not the harness.
#[test]
fn mirror_post_write_diff_still_renders_without_any_preview() {
    let mut view = TranscriptView::new();
    view.push_tool_start("edit", json!({ "path": "hello.txt" }));
    view.push_tool_end(
        "edit",
        false,
        Some(text_result(
            "Successfully replaced 1 block(s) in hello.txt.",
            json!({ "diff": "-2 bravo\n+2 BRAVO" }),
        )),
    );

    let (w, h) = (72u16, 12u16);
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    let theme = UiTheme::dark();
    term.draw(|f| view.render(f, Rect::new(0, 0, w, h), &theme))
        .unwrap();
    let buf = term.backend().buffer().clone();
    let mut text = String::new();
    for y in 0..h {
        for x in 0..w {
            if let Some(c) = buf.cell((x, y)) {
                text.push_str(c.symbol());
            }
        }
        text.push('\n');
    }

    assert!(text.contains("edit hello.txt"), "header missing:\n{text}");
    assert!(
        has_diff_row(&text, '-', "bravo"),
        "post-write removed line missing:\n{text}"
    );
    assert!(
        has_diff_row(&text, '+', "BRAVO"),
        "post-write added line missing:\n{text}"
    );
}

/// [CYRUP-DELTA] A partial batch previews as `Ok` — the survivors' diff — plus one line per edit
/// that will not land. Pi has nothing to show here, because the call would be discarded whole.
/// This is the branch in `edit_preview` that joins `unapplied` onto the diff, and it is what a
/// user reads while deciding whether to approve a write that will only partly succeed.
#[test]
fn edit_preview_names_the_edits_that_will_not_apply() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("hello.txt");
    std::fs::write(&file, BEFORE).unwrap();

    // Wider than the 72 the other cases use: the shortfall sentence must not wrap through the
    // fragment being asserted.
    let mut app = App::new(TestBackend::new(120, 24), UiTheme::dark()).unwrap();
    app.set_title_cwd(dir.path().to_path_buf());
    start_edit(
        &mut app,
        "call-partial",
        json!({ "path": "hello.txt", "edits": [
            { "oldText": "bravo", "newText": "BRAVO" },
            { "oldText": "zzz", "newText": "9" },
        ] }),
    );

    let text = viewport(&app);
    // What WILL land is still previewed.
    assert!(
        has_diff_row(&text, '-', "bravo"),
        "surviving edit not previewed:\n{text}"
    );
    assert!(
        has_diff_row(&text, '+', "BRAVO"),
        "surviving edit not previewed:\n{text}"
    );
    // And what will NOT is named — without the near-miss region, which belongs in the result.
    assert!(
        text.contains("Could not find edits[1] in hello.txt."),
        "shortfall not named:\n{text}"
    );
    assert!(
        !text.contains("Closest region"),
        "region leaked into the preview:\n{text}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        BEFORE,
        "the preview must not have touched the file"
    );
}
