//! Batch-11 group X — the **wiring** half of X7/X9/X13: the seams the transcript renderers now
//! read must actually be fed by the app, not only by unit tests.
//!
//! * X9 — `keyHint`/`keyText` resolve `app.tools.expand` against the LIVE keymap on every render
//!   (`components/keybinding-hints.ts:34-36`). cyrup's transcript holds no keymap, so
//!   `App::load_keybindings_json` must push the resolved label into it; without that the hints stay
//!   on the compile-time `ctrl+o`.
//! * X7 — `getCompactReadClassification(args, context.cwd)` (`core/tools/read.ts:336`) resolves the
//!   read path against the SESSION cwd (`components/tool-execution.ts:126`), which
//!   `App::set_title_cwd` is the one funnel for.
//! * X13 — `component.setComplete(message.exitCode, message.cancelled, message.truncated ? … ,
//!   message.fullOutputPath)` (`modes/interactive/interactive-mode.ts:3460-3465`) on the
//!   `bashExecution` replay arm.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::{App, UiTheme};
use cyrup_session_svc::agent_message::{AgentMessage, BashExecutionMessage};
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 24), UiTheme::dark()).unwrap()
}

/// **X9 — a rebound `app.tools.expand` reaches a transcript hint through the real app path.**
#[test]
fn rebinding_the_expand_key_changes_the_transcript_hints() {
    let mut app = new_app();
    app.transcript_mut()
        .push_branch_summary("we merged the spike");
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("(ctrl+o to expand)"),
        "the default binding label:\n{}",
        app.scrollback_text()
    );

    let mut rebound = new_app();
    rebound
        .load_keybindings_json(r#"{ "app.tools.expand": "ctrl+e" }"#)
        .expect("a valid keybindings document loads");
    rebound
        .transcript_mut()
        .push_branch_summary("we merged the spike");
    rebound.draw().unwrap();
    let out = rebound.scrollback_text();
    assert!(
        out.contains("(ctrl+e to expand)"),
        "the REBOUND label reaches the hint:\n{out}"
    );
    assert!(!out.contains("ctrl+o"), "and the literal is gone:\n{out}");
}

/// **X7 — the session cwd reaches `read`'s compact classification.**
///
/// The same relative path classifies as a `resource` or not purely by what it resolves to under
/// `context.cwd`, which is what makes the plumbing observable end-to-end.
#[test]
fn the_session_cwd_reaches_the_compact_read_classification() {
    let mut app = new_app();
    app.set_title_cwd(std::path::PathBuf::from("/w/project"));
    assert_eq!(
        app.state().transcript.cwd(),
        Some(std::path::Path::new("/w/project")),
        "`set_title_cwd` is the funnel Pi's `ToolRenderContext.cwd` rides"
    );
    app.transcript_mut()
        .push_tool_start("read", serde_json::json!({ "path": "AGENTS.md" }));
    app.draw().unwrap();
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            if let Some(c) = buf.cell((x, y)) {
                out.push_str(c.symbol());
            }
        }
        out.push('\n');
    }
    assert!(
        out.contains("read resource AGENTS.md"),
        "compact resource header:\n{out}"
    );
    assert!(out.contains("to expand"), "with its expand hint:\n{out}");
}

/// **X13 — a replayed `bashExecution` carries its truncation report to the block.**
#[test]
fn a_replayed_bash_execution_replays_its_truncation_warning() {
    let mut app = new_app();
    app.replay_session(&[AgentMessage::BashExecution(BashExecutionMessage {
        command: "gen-report".to_string(),
        output: "row 1\nrow 2".to_string(),
        exit_code: Some(0),
        cancelled: false,
        truncated: true,
        full_output_path: Some("/tmp/cyrup-bash-9.log".to_string()),
        timestamp: 0,
        exclude_from_context: Some(false),
    })]);
    app.draw().unwrap();
    let out = app.scrollback_text();
    assert!(
        out.contains("Output truncated. Full output: /tmp/cyrup-bash-9.log"),
        "the spool path is replayed (interactive-mode.ts:3460-3465):\n{out}"
    );

    // MIRROR: an untruncated replay says nothing — the row is not unconditional.
    let mut clean = new_app();
    clean.replay_session(&[AgentMessage::BashExecution(BashExecutionMessage {
        command: "gen-report".to_string(),
        output: "row 1\nrow 2".to_string(),
        exit_code: Some(0),
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: 0,
        exclude_from_context: Some(false),
    })]);
    clean.draw().unwrap();
    assert!(
        !clean.scrollback_text().contains("truncated"),
        "{}",
        clean.scrollback_text()
    );
}

/// **X7(b) — the PRODUCTION runtime-cwd adoption goes through the funnel.**
///
/// `App::set_title_cwd` does two things: it stores the window-title cwd AND it hands the same value
/// to the transcript as Pi's `ToolRenderContext.cwd` (`components/tool-execution.ts:126`), which
/// `getCompactReadClassification(args, context.cwd)` resolves a read path against
/// (`core/tools/read.ts:129`, `resolveToCwd(rawPath, cwd)`). The test above proves the funnel
/// works; it did NOT prove anything called it. Production assigned `state.title_cwd` directly in
/// the run loop, so `transcript.set_cwd` never ran and the classification fell back to
/// `std::env::current_dir()` — the PROCESS cwd, which the run loop's own comment says can differ
/// from the session runtime's after a `/resume` of a session recorded elsewhere.
///
/// The run loop needs a live `AgentSessionRuntime` to reach behaviourally, so this is a source
/// guard: no `title_cwd` assignment may exist outside the funnel, and the loop's adoption must be
/// the funnel call. It fails on the bare-assignment form.
#[test]
fn no_production_site_assigns_title_cwd_outside_the_funnel() {
    let app_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/app");
    let mut entries: Vec<_> = std::fs::read_dir(&app_dir)
        .expect("src/app is readable")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    let src = entries
        .iter()
        .map(|p| std::fs::read_to_string(p).expect("app module file is readable"))
        .collect::<Vec<_>>()
        .join("\n");

    let assignments: Vec<&str> = src
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with("//"))
        .filter(|l| l.contains("title_cwd = "))
        .collect();
    assert_eq!(
        assignments,
        vec!["self.state.title_cwd = cwd;"],
        "the ONLY `title_cwd` assignment may be `set_title_cwd`'s own — every other site must call \
         that funnel so the transcript's `ToolRenderContext.cwd` is set with it; found: {assignments:#?}"
    );

    assert!(
        src.contains("self.set_title_cwd(rt.cwd().to_path_buf());"),
        "the run loop must adopt the runtime cwd THROUGH the funnel (Pi `sessionManager.getCwd()`, \
         interactive-mode.ts:819)"
    );
}
