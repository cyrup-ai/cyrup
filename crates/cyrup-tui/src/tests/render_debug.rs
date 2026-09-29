//! CFG-063 — the `CYRUP_TUI_DEBUG` frame dump and the `CYRUP_TUI_DEBUG_REDRAW` log, driven
//! through [`App::draw`], the one inline frame path (pi `TuiMainScreen.doRender`,
//! `tui-main-screen.ts:321-327`, `:569-595` @v0.87.1).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::backend::TestBackend;

use crate::{App, RenderDebug, UiTheme};

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

/// `CYRUP_TUI_DEBUG_REDRAW=1` with the agent directory as the log directory appends exactly one
/// `fullRender:` line per full repaint, in pi's format, and none for a frame that only diffs.
///
/// Red before the fix: no such variable was read and no log existed.
#[test]
fn cfg063_the_redraw_log_names_each_full_repaint() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_render_debug(RenderDebug::from_env(
        |k| (k == "CYRUP_TUI_DEBUG_REDRAW").then(|| "1".to_string()),
        Some(dir.path()),
    ));
    app.draw().unwrap(); // first render
    app.draw().unwrap(); // nothing moved: diffed, not logged
    app.editor_mut().set_text("one\ntwo\nthree\nfour");
    app.draw().unwrap(); // the editor grew the live region: erase-and-rebuild

    let log = std::fs::read_to_string(dir.path().join("cyrup-tui-debug.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    for line in &lines {
        // `[${new Date().toISOString()}] fullRender: …`
        assert!(
            line.starts_with('[') && line.contains("Z] fullRender: "),
            "{line}"
        );
    }
    assert!(
        lines[0].contains("fullRender: first render (prev=0, new="),
        "{log}"
    );
    assert!(lines[0].ends_with(", height=24)"), "{log}");
    assert!(
        lines[1].contains("fullRender: live region height changed ("),
        "{log}"
    );
}

/// The redraw log is written only where the host points it: with the variable set but no log
/// directory (pi #8699) nothing is written — including into the working directory.
#[test]
fn cfg063_no_log_directory_means_no_redraw_log() {
    let debug = RenderDebug::from_env(
        |k| (k == "CYRUP_TUI_DEBUG_REDRAW").then(|| "1".to_string()),
        None,
    );
    assert_eq!(debug, RenderDebug::default());
}

/// `CYRUP_TUI_DEBUG=1` writes one `render-<ms>-<rand>.log` per DIFFED frame — a full repaint
/// returns before pi's dump — carrying the decision state and both line arrays.
///
/// Red before the fix: no frame state was recorded anywhere.
#[test]
fn cfg063_the_frame_dump_records_each_diffed_frame() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_render_debug(RenderDebug {
        frame_dump_dir: Some(dir.path().to_path_buf()),
        redraw_log_dir: None,
    });
    app.draw().unwrap(); // first render: a full repaint, no dump
    let dumps = || {
        let mut v: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        v.sort();
        v
    };
    assert!(dumps().is_empty(), "a full repaint is not dumped");
    app.editor_mut().set_text("zebra");
    app.draw().unwrap();
    let files = dumps();
    assert_eq!(files.len(), 1);
    let name = files[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(
        name.starts_with("render-") && name.ends_with(".log"),
        "{name}"
    );

    let dump = std::fs::read_to_string(&files[0]).unwrap();
    for field in [
        "firstChanged: ",
        "lastChanged: ",
        "viewportTop: ",
        "height: 24",
        "width: 80",
        "liveRegionHeight: ",
        "newLines.length: ",
        "previousLines.length: ",
        "=== newLines ===",
        "=== previousLines ===",
    ] {
        assert!(dump.contains(field), "missing {field:?}:\n{dump}");
    }
    assert!(
        !dump.contains("firstChanged: -1"),
        "the typed text changed a row:\n{dump}"
    );
    let (new, previous) = dump.split_once("=== previousLines ===").unwrap();
    assert!(
        new.contains("zebra"),
        "the new frame shows the text:\n{dump}"
    );
    assert!(
        !previous.contains("zebra"),
        "the previous frame did not:\n{dump}"
    );
}

/// A frame that rewrites no row writes no dump: pi returns before its `PI_TUI_DEBUG` block when
/// `firstChanged === -1` (`tui-main-screen.ts:392-397` @v0.87.1), so an idle redraw (a spinner-less
/// tick, a no-op key) leaves `/tmp/tui` alone.
///
/// Red before the fix: every diffed frame was dumped, changed or not, one file per tick.
#[test]
fn cfg063_an_unchanged_frame_is_not_dumped() {
    let dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_render_debug(RenderDebug {
        frame_dump_dir: Some(dir.path().to_path_buf()),
        redraw_log_dir: None,
    });
    app.draw().unwrap(); // first render: full repaint
    app.editor_mut().set_text("zebra");
    app.draw().unwrap(); // one row changed: dumped
    app.draw().unwrap(); // nothing changed: not dumped
    app.draw().unwrap();
    let count = std::fs::read_dir(dir.path()).unwrap().count();
    assert_eq!(count, 1, "only the frame that changed a row is dumped");
}

/// pi's `requestRender(true)` runs `resetRenderState()` (`previousWidth = -1`,
/// `tui-main-screen.ts:158-166`), so the repaint after a suspend or an external editor is a FULL
/// render logged as `terminal width changed (-1 -> <width>)` and never dumped as a diff.
/// [`App::reset_render_state`] is that reset — what `suspend` and the external-editor return run.
///
/// Red before the fix: the sites only cleared the terminal, so the redraw log missed the repaint
/// and the frame dump recorded it as a diff against the pre-suspend rows.
#[test]
fn cfg063_a_from_scratch_repaint_is_logged_as_pis_reset_width_change() {
    let log_dir = tempfile::tempdir().unwrap();
    let dump_dir = tempfile::tempdir().unwrap();
    let mut app = app();
    app.set_render_debug(RenderDebug {
        frame_dump_dir: Some(dump_dir.path().to_path_buf()),
        redraw_log_dir: Some(log_dir.path().to_path_buf()),
    });
    app.draw().unwrap(); // first render
    app.reset_render_state();
    app.editor_mut().set_text("after fg");
    app.draw().unwrap();

    let log = std::fs::read_to_string(log_dir.path().join("cyrup-tui-debug.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 2, "{log}");
    assert!(
        lines[1].contains("fullRender: terminal width changed (-1 -> 80) (prev="),
        "{log}"
    );
    assert_eq!(
        std::fs::read_dir(dump_dir.path()).unwrap().count(),
        0,
        "a full repaint is not a diffed frame"
    );
}
