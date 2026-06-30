//! `!`/`!!` bash-execution block + floating overlay (hotkeys popup) wiring, headless against a
//! `TestBackend` (spec/tui/03 §7 bash-execution; spec/tui/05 §2 overlays; gaps 2 + 4).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use cyrup_tui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use cyrup_tui::{App, AppAction, BashStatus, InputEvent, UiTheme};
use ratatui::backend::TestBackend;

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn ctrl(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

fn submit(app: &mut App<TestBackend>, line: &str) -> AppAction {
    app.editor_mut().set_text(line);
    app.handle_input(&key(KeyCode::Enter))
}

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

fn buf_text(app: &App<TestBackend>) -> String {
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

#[test]
fn bang_command_opens_a_live_bash_block_and_requests_a_run() {
    let mut app = new_app();
    let action = submit(&mut app, "!echo hi");
    assert_eq!(
        action,
        AppAction::RunBash { command: "echo hi".to_string(), excluded: false }
    );
    assert!(app.state().transcript.has_bash(), "a live bash block is open");
    assert!(app.state().transcript.bash_running(), "it starts running");
    let b = app.state().transcript.bash().unwrap();
    assert_eq!(b.command(), "echo hi");
    assert!(!b.excluded());
}

#[test]
fn double_bang_marks_excluded_from_context() {
    let mut app = new_app();
    let action = submit(&mut app, "!!secret-cmd");
    assert_eq!(
        action,
        AppAction::RunBash { command: "secret-cmd".to_string(), excluded: true }
    );
    assert!(app.state().transcript.bash().unwrap().excluded());
}

#[test]
fn bash_block_streams_output_and_renders_in_the_viewport() {
    let mut app = new_app();
    submit(&mut app, "!echo hi");
    // The run loop normally pumps these; drive the transcript directly here.
    app.transcript_mut().bash_append("hello\nworld\n");
    app.transcript_mut().bash_complete(Some(0), false);
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(screen.contains("$ echo hi"), "command header rendered:\n{screen}");
    assert!(screen.contains("hello"), "stdout rendered:\n{screen}");
    assert!(screen.contains("world"), "stdout rendered:\n{screen}");
    assert_eq!(app.state().transcript.bash().unwrap().status(), BashStatus::Complete);
}

#[test]
fn ctrl_o_toggles_bash_expansion() {
    let mut app = new_app();
    submit(&mut app, "!seq 100");
    for i in 1..=40 {
        app.transcript_mut().bash_append(&format!("row{i}\n"));
    }
    app.transcript_mut().bash_complete(Some(0), false);
    assert!(!app.state().transcript.bash().unwrap().expanded());
    app.handle_input(&ctrl(KeyCode::Char('o')));
    assert!(app.state().transcript.bash().unwrap().expanded(), "Ctrl+O expands the bash block");
    app.handle_input(&ctrl(KeyCode::Char('o')));
    assert!(!app.state().transcript.bash().unwrap().expanded(), "Ctrl+O collapses again");
}

#[test]
fn interrupt_cancels_a_running_bash_block() {
    let mut app = new_app();
    submit(&mut app, "!sleep 9");
    app.transcript_mut().bash_append("partial");
    // Esc → Interrupt cancels + commits the block to scrollback.
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Interrupt);
    assert!(!app.state().transcript.has_bash(), "the live block was committed away");
    // The committed block shows the cancelled status once flushed.
    app.draw().unwrap();
    assert!(app.scrollback_text().contains("(cancelled)"), "{}", app.scrollback_text());
}

#[test]
fn hotkeys_overlay_renders_floating_and_dismisses() {
    let mut app = new_app();
    submit(&mut app, "/hotkeys");
    assert!(app.overlay_open());
    app.draw().unwrap();
    let screen = buf_text(&app);
    assert!(screen.contains("Keyboard Shortcuts"), "overlay title:\n{screen}");
    assert!(screen.contains("Send message"), "overlay lists the submit binding:\n{screen}");
    // The box is drawn with a border (ratatui Block borders → corner glyphs).
    assert!(screen.contains('┌') || screen.contains('╭'), "overlay has a box border:\n{screen}");

    // Esc dismisses it; the key never leaks to the editor beneath.
    let action = app.handle_input(&key(KeyCode::Esc));
    assert_eq!(action, AppAction::Redraw);
    assert!(!app.overlay_open(), "Esc closes the overlay");
}

#[test]
fn page_up_scrolls_the_active_region_and_page_down_returns_to_tail() {
    // A tall streaming partial exceeds the small viewport; PageUp reveals earlier lines, PageDown
    // pins back to the live tail (spec/tui/07 page-scroll over the active region).
    let mut app = App::new(TestBackend::new(40, 8), UiTheme::dark()).unwrap();
    // Paragraph breaks (blank line between) keep each `rowN` on its own rendered line (a single
    // newline is a markdown soft break → collapsed), so the active region is genuinely tall.
    let body: String = (1..=40).map(|i| format!("row{i}")).collect::<Vec<_>>().join("\n\n");
    app.transcript_mut().push_assistant_delta(&body);
    app.draw().unwrap();
    let tail = buf_text(&app);
    assert!(tail.contains("row40"), "tail anchored to newest:\n{tail}");

    app.handle_input(&key(KeyCode::PageUp));
    app.handle_input(&key(KeyCode::PageUp));
    app.draw().unwrap();
    assert!(app.state().transcript.scroll_offset() > 0, "paged up off the tail");

    app.handle_input(&key(KeyCode::PageDown));
    app.handle_input(&key(KeyCode::PageDown));
    app.draw().unwrap();
    assert_eq!(app.state().transcript.scroll_offset(), 0, "PageDown returns to the tail");
    assert!(buf_text(&app).contains("row40"), "tail visible again");
}

#[test]
fn ctrl_g_requests_the_external_editor() {
    // `app.editor.external` (Ctrl+G) surfaces to the run loop, which launches $VISUAL/$EDITOR.
    let mut app = new_app();
    app.editor_mut().set_text("draft text");
    assert_eq!(app.handle_input(&ctrl(KeyCode::Char('g'))), AppAction::OpenExternalEditor);
}

#[test]
fn overlay_captures_navigation_keys() {
    let mut app = new_app();
    submit(&mut app, "/hotkeys");
    // Arrow scroll is consumed by the overlay (Redraw), not the editor.
    assert_eq!(app.handle_input(&key(KeyCode::Down)), AppAction::Redraw);
    assert!(app.overlay_open());
}
