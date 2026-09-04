//! TUI-005 / TUI-009 / TUI-010 / TUI-038 / TUI-S10 — the global-key arms Pi orders differently from
//! the way cyrup did.
//!
//! Pi's `defaultEditor.onEscape` is a chain of **four mutually exclusive** `else if` branches
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2569-2595` @v0.83.0):
//!
//! ```ts
//! if      (this.session.isStreaming)   this.restoreQueuedMessagesToEditor({ abort: true });
//! else if (this.session.isBashRunning) this.session.abortBash();
//! else if (this.isBashMode)          { this.editor.setText(""); this.isBashMode = false; … }
//! else if (!this.editor.getText().trim()) { …the 500 ms double-Escape window… }
//! ```
//!
//! cyrup ran the bash-child cancel as a plain `if` **ahead of** the streaming read, so an Escape
//! during a turn that also had a `!`-child killed the child as collateral; and the third and fourth
//! branches did not exist at all, which left the live, persisted, documented `doubleEscapeAction`
//! `/settings` row with no consumer.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use super::harness::*;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppAction, AppCommand, Entry, InputEvent, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap()
}

fn type_str(app: &mut App<TestBackend>, s: &str) {
    for c in s.chars() {
        app.handle_input(&InputEvent::Key(KeyEvent::new(
            KeyCode::Char(c),
            KeyModifiers::NONE,
        )));
    }
}

/// TUI-005, the destructive half. RED at HEAD: `Action::Interrupt` cancelled a running bash block in
/// a plain `if` *before* reading `status.streaming`, so an Escape mid-turn killed the `!`-child as
/// collateral. Upstream's arms are exclusive (`:2570-2573`), so pi never touches a bash child while
/// streaming — the streaming arm returns first.
#[test]
fn escape_while_streaming_does_not_kill_a_running_bash_child() {
    let mut app = new_app();
    app.transcript_mut()
        .start_bash("sleep 100".to_string(), false, None, None);
    app.state_mut().status.set_streaming(true);
    assert!(
        app.state().transcript.bash_running(),
        "precondition: a child is running"
    );

    let out = app.handle_input(&esc());
    assert_eq!(
        out,
        AppAction::InterruptRestoreQueued,
        "the streaming arm must win"
    );
    assert!(
        app.state().transcript.bash_running(),
        "the bash child must survive an Escape that belongs to the turn"
    );
}

/// With nothing streaming, the SECOND arm fires and the child is cancelled (`:2571-2572`).
#[test]
fn escape_with_no_turn_cancels_the_bash_child() {
    let mut app = new_app();
    app.transcript_mut()
        .start_bash("sleep 100".to_string(), false, None, None);
    let out = app.handle_input(&esc());
    assert_eq!(out, AppAction::Interrupt, "the run loop kills the child");
    assert!(
        !app.state().transcript.bash_running(),
        "the block is marked cancelled"
    );
}

/// TUI-005's other half — the bash-MODE arm (`:2574-2578`): a typed-but-unsent `!cmd` is cleared.
/// RED at HEAD: `rg 'bash_mode|starts_with("!")' src/app.rs` found nothing, so Escape did nothing.
#[test]
fn escape_in_bash_mode_clears_the_editor() {
    let mut app = new_app();
    type_str(&mut app, "!echo hi");
    assert_eq!(app.state().editor.text(), "!echo hi");
    let out = app.handle_input(&esc());
    assert_eq!(out, AppAction::Redraw);
    assert!(
        app.state().editor.text().is_empty(),
        "pi's `this.editor.setText(\"\")`"
    );
}

/// A non-empty, non-`!` buffer falls off the end of pi's chain and does nothing (`:2569-2595`) — in
/// particular it must NOT clear the buffer, which is `app.clear`'s job, not Escape's.
#[test]
fn escape_with_ordinary_text_does_nothing() {
    let mut app = new_app();
    type_str(&mut app, "hello");
    let out = app.handle_input(&esc());
    assert_eq!(out, AppAction::Redraw);
    assert_eq!(
        app.state().editor.text(),
        "hello",
        "Escape must not clear an ordinary buffer"
    );
}

/// TUI-009 — the double-Escape window (`interactive-mode.ts:2579-2594`, `lastEscapeTime` at `:355`).
/// RED at HEAD: `AppState` had no escape timestamp at all (`rg 'last_escape'` → zero) and
/// `Action::Interrupt` had no empty-editor branch, so `doubleEscapeAction` — live and persisted in
/// `/settings` — changed nothing whichever value it held.
#[test]
fn two_escapes_on_an_empty_editor_open_the_configured_target() {
    let mut app = new_app();
    app.state_mut().double_escape_action = "tree".to_string();
    // The first press only arms the window.
    assert_eq!(app.handle_input(&esc()), AppAction::Redraw);
    // The second, inside 500 ms, fires.
    assert_eq!(
        app.handle_input(&esc()),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::Tree))
    );
    // A third press starts a NEW pair (`this.lastEscapeTime = 0` at `:2590`), so it only arms.
    assert_eq!(app.handle_input(&esc()), AppAction::Redraw);
}

/// `fork` routes to pi's `showUserMessageSelector` (`:2588`).
#[test]
fn the_fork_setting_opens_the_user_message_selector() {
    let mut app = new_app();
    app.state_mut().double_escape_action = "fork".to_string();
    app.handle_input(&esc());
    assert_eq!(
        app.handle_input(&esc()),
        AppAction::Command(AppCommand::OpenSelector(SelectorKind::UserMessage))
    );
}

/// `none` is checked BEFORE the window is even armed (`if (action !== "none")`, `:2581`).
#[test]
fn the_none_setting_never_fires() {
    let mut app = new_app();
    app.state_mut().double_escape_action = "none".to_string();
    app.handle_input(&esc());
    assert_eq!(
        app.handle_input(&esc()),
        AppAction::Redraw,
        "`none` must do nothing"
    );
}

/// TUI-010 / TUI-038 — Ctrl+O is a FAN-OUT plus a status echo, not an if/else.
///
/// RED at HEAD: `Action::ToolsExpand` was
/// `if transcript.has_bash() { toggle_bash_expanded() } else { toggle_tool_expanded() }`, so while
/// any `!cmd` block was present the tool-expansion flag could not be moved at all — and no status
/// was pushed, while the extension path pushed one for the identical user-visible action.
/// Upstream's `setToolsExpanded` sets one flag, broadcasts it to every `isExpandable` child of
/// `loadedResourcesContainer` and `chatContainer`, and ends in
/// `showStatus("Tool output: …")` (`interactive-mode.ts:4032-4048` @v0.84.1).
#[test]
fn ctrl_o_expands_the_bash_block_and_the_tool_blocks_together_and_echoes() {
    let mut app = new_app();
    app.transcript_mut()
        .start_bash("ls".to_string(), false, None, None);
    assert!(
        !app.state().transcript.tool_expanded(),
        "precondition: collapsed"
    );

    let ctrl_o = InputEvent::Key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    app.handle_input(&ctrl_o);
    assert!(
        app.state().transcript.tool_expanded(),
        "the tool flag must move even with a live bash block present"
    );
    app.draw().unwrap();
    assert!(
        screen(&app).contains("Tool output: expanded"),
        "pi's status echo is missing:\n{}",
        screen(&app)
    );

    app.handle_input(&ctrl_o);
    assert!(
        !app.state().transcript.tool_expanded(),
        "a second press collapses"
    );
    app.draw().unwrap();
    assert!(
        screen(&app).contains("Tool output: collapsed"),
        "both directions echo"
    );
}

/// TUI-S10 — Shift+Ctrl+D reaches `/debug` regardless of focus. Pi tests it inside
/// `handleTerminalInput` and BEFORE dispatching to the focused component
/// (`packages/tui/src/tui.ts:850` @v0.83.0), wired at `interactive-mode.ts:2803`.
///
/// RED at HEAD: `rg 'debug' src/keymap.rs` → zero and `rg 'onDebug|on_debug'` → zero; `/debug` was
/// reachable only by typing it into the editor, i.e. never while a selector had focus.
#[test]
fn shift_ctrl_d_dumps_debug_even_with_a_selector_focused() {
    let mut app = new_app();
    app.open_selector(SelectorKind::Theme);
    assert!(
        app.state().selector.is_some(),
        "precondition: a selector owns the slot"
    );

    let chord = InputEvent::Key(KeyEvent::new(
        KeyCode::Char('d'),
        KeyModifiers::CONTROL | KeyModifiers::SHIFT,
    ));
    app.handle_input(&chord);
    // The chord's effect is `handleDebugCommand()` — a `Debug` block appended to the transcript.
    // Assert that directly: this harness is 80x16 and a selector owns most of it, so the block's
    // own title row is scrolled above the viewport by the body (block *titles* render, and are
    // asserted on a taller harness in `assembled_render`/`bash_overlay`).
    assert!(
        app.state()
            .transcript
            .pending()
            .iter()
            .any(|e| matches!(e, Entry::Block { title, .. } if title == "Debug")),
        "no `Debug` block was appended to the transcript"
    );
    app.draw().unwrap();
    assert!(
        screen(&app).contains("thinking"),
        "the debug block's body must actually paint:\n{}",
        screen(&app)
    );
    assert!(
        app.state().selector.is_some(),
        "the selector keeps its slot"
    );
}

fn screen(app: &App<TestBackend>) -> String {
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

/// Escape during a COMPACTION cancels the compaction and nothing else — Pi rebinds
/// `defaultEditor.onEscape = () => { this.session.abortCompaction(); }` on `compaction_start`
/// (`interactive-mode.ts:3080-3086` @v0.83.0) and restores the previous handler on `compaction_end`
/// (`:3094-3097`), so the rebind SHADOWS the four-branch chain for the whole window.
///
/// This matters specifically because of TUI-005's restructuring: `isStreaming` is false during a
/// compaction (compaction ABORTS the active run and does not set the agent snapshot), so without
/// the rebind an Escape mid-compaction falls through to the empty-editor branch and does nothing.
#[tokio::test]
async fn escape_during_a_compaction_aborts_the_compaction() {
    use cyrup_session_svc::{AgentSessionEvent, CompactionReason};
    let mut app = new_app();
    app.ingest_event(&AgentSessionEvent::CompactionStart {
        reason: CompactionReason::Manual,
    });
    assert_eq!(app.handle_input(&esc()), AppAction::AbortCompaction);

    app.ingest_event(&AgentSessionEvent::CompactionEnd {
        reason: CompactionReason::Manual,
        result: None,
        aborted: true,
        will_retry: false,
        error_message: None,
    });
    assert_ne!(
        app.handle_input(&esc()),
        AppAction::AbortCompaction,
        "the rebind must be undone at compaction_end"
    );
}
