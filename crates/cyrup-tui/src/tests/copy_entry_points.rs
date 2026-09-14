//! **TUI-097** — `/copy` and `app.message.copy` are not the same call.
//!
//! Upstream `handleCopyCommand` takes an options bag (`interactive-mode.ts:6134-6136` @v0.85.1) and
//! its selection leg is a FOUR-part conjunction (`:6137-6145`):
//!
//! ```ts
//! if (options.preferSelection && this.ui instanceof TuiAltScreen &&
//!     !this.ui.getCopyOnSelect() && this.ui.hasActiveSelection()) {
//!   await this.ui.copyActiveSelectionToClipboard();
//!   return;
//! }
//! ```
//!
//! Only the keybinding passes `preferSelection` (`:2894-2897`); `/copy` passes no options at all
//! (`:3007-3010`). cyrup carried a unit `AppCommand::Copy` whose guard was the last clause alone, so
//! BOTH entry points preferred a live selection — a behaviour no upstream tag has (at the ported
//! baseline v0.83.0 `handleCopyCommand()` took no options and both always copied the last assistant
//! message).
//!
//! The session here deliberately has no assistant message, so the last-message leg answers pi's
//! `getLastAssistantText()`-empty path and the two legs are distinguishable without a clipboard: a
//! headless CI host cannot write one, and both legs would otherwise report the same failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind as Kind};

use crate::component::InputEvent;
use crate::{App, AppCommand, UiTheme};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// An offline session that has never produced an assistant message, so `last_assistant_text()` is
/// `None` — pi's `if (!text) { this.showError("No agent messages to copy yet."); return; }`
/// (`interactive-mode.ts:6147-6151`).
async fn empty_session(tmp: &TempDir) -> Arc<AgentSession> {
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    Arc::new(
        SessionBuilder::new(provider, cfg)
            .cli_settings(cyrup_session_svc::Settings::new())
            .build()
            .await
            .unwrap(),
    )
}

fn mouse(kind: Kind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// An app in the alternate screen with a LIVE (un-released) selection over its document — pi's
/// `hasActiveSelection()`. The press/drag pair is left open on purpose: a release would hand the
/// text out through `PointerOutcome::Copy` and end the selection.
fn app_with_a_live_selection(copy_on_select: bool) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    app.set_fullscreen_copy_on_select(copy_on_select);
    app.transcript_mut().push_status("selectable text");
    let _captured = app
        .enter_fullscreen_captured()
        .expect("the capture renderer builds");
    app.draw().unwrap();
    for row in 0..10 {
        app.handle_input(&mouse(Kind::Down(MouseButton::Left), 0, row));
        app.handle_input(&mouse(Kind::Drag(MouseButton::Left), 30, row));
        if app.has_active_selection_for_test() {
            return app;
        }
    }
    panic!("no document row yielded a selection");
}

/// Every committed transcript entry the app has flushed, as text.
///
/// Neither renderer keeps committed entries in the live viewport: the inline one hands them to the
/// terminal through `insert_before` (they land in the `scrollback_lines` accumulator) and the
/// alternate screen retains them in `TranscriptView::document()` (ADR-0005 §B-1). Both are read
/// here so one helper answers for either mode.
fn flushed_text(app: &mut App<TestBackend>) -> String {
    app.draw().unwrap();
    let inline: String = app
        .scrollback_lines()
        .iter()
        .map(|l| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() })
        .collect::<Vec<_>>()
        .join("\n");
    // The retained document is a `Vec<Entry>`; its `Debug` rendering carries each entry's text,
    // which is all this needs to tell pi's two legs apart.
    let retained = format!("{:?}", app.transcript_mut().document());
    format!("{inline}\n{retained}")
}

/// Whether the last-assistant-message leg ran, read off the line it pushes when there is none —
/// pi's `if (!text) { showError("No agent messages to copy yet."); return; }` (`:6147-6151`).
async fn ran_the_last_message_leg(
    app: &mut App<TestBackend>,
    command: AppCommand,
    session: &Arc<AgentSession>,
) -> bool {
    app.execute_command(command, session, None).await;
    flushed_text(app).contains("no assistant message to copy")
}

/// (1) Copy-on-select OFF + `app.message.copy`: the one combination upstream gives the selection to.
#[tokio::test]
async fn copy_on_select_off_ctrl_x_copies_the_selection() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = app_with_a_live_selection(false);
    assert!(
        app.prefers_active_selection(true),
        "all four of pi's clauses hold (`:6137-6145`)"
    );
    assert!(
        !ran_the_last_message_leg(
            &mut app,
            AppCommand::Copy {
                prefer_selection: true
            },
            &session
        )
        .await,
        "the selection leg must win, so the empty-session line is never pushed"
    );
}

/// (2) THE filed defect: `/copy` passes no options (`:3008`), so `preferSelection` is falsy and the
/// last assistant message wins even with a selection standing.
///
/// FAILS before TUI-097: `AppCommand::Copy` carried no discriminator, so `/copy` took the selection.
#[tokio::test]
async fn copy_on_select_off_slash_copy_copies_the_last_message() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = app_with_a_live_selection(false);
    assert!(
        !app.prefers_active_selection(false),
        "`options.preferSelection` is pi's FIRST clause (`:6138`)"
    );
    assert!(
        ran_the_last_message_leg(
            &mut app,
            AppCommand::Copy {
                prefer_selection: false
            },
            &session
        )
        .await,
        "`/copy` must fall through to `getLastAssistantText()`"
    );
}

/// (3) The second missing clause: with copy-on-select ON the selection is already on the clipboard,
/// so `!this.ui.getCopyOnSelect()` (`:6141`) releases Ctrl+X back to the last assistant message.
///
/// FAILS before TUI-097: the guard never consulted `fullscreen_copy_on_select` at all.
#[tokio::test]
async fn copy_on_select_on_ctrl_x_copies_the_last_message() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = app_with_a_live_selection(true);
    assert!(
        !app.prefers_active_selection(true),
        "`!getCopyOnSelect()` fails, so the conjunction fails"
    );
    assert!(
        ran_the_last_message_leg(
            &mut app,
            AppCommand::Copy {
                prefer_selection: true
            },
            &session
        )
        .await,
        "Ctrl+X falls through when copy-on-select already took the selection"
    );
}

/// (4) `this.ui instanceof TuiAltScreen` (`:6139`): with no alternate screen there is no selection
/// to prefer, whatever the flag says.
#[tokio::test]
async fn no_altscreen_ctrl_x_copies_the_last_message() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    for on in [false, true] {
        app.set_fullscreen_copy_on_select(on);
        assert!(
            !app.prefers_active_selection(true),
            "no alternate screen, so no `hasActiveSelection()` (copy_on_select={on})"
        );
    }
    app.execute_command(
        AppCommand::Copy {
            prefer_selection: true,
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    let t = flushed_text(&mut app);
    assert!(
        t.contains("no assistant message to copy"),
        "the inline path takes the last-message leg unconditionally: [{t}]"
    );
}
