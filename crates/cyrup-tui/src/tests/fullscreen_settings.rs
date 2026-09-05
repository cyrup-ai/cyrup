//! CFG-078 — the two v0.84.4 alternate-screen settings reaching BEHAVIOUR, not just a settings row.
//!
//! `fullscreenCopyOnSelect` (`core/settings-manager.ts:145` @v0.84.4, default `true`) and
//! `fullscreenExitOutput` (`:143`, default `"transcript"`) are both consumed by the renderer, and
//! both are LIVE upstream: pi seeds `copyOnSelect` into every `createInteractiveTui`
//! (`modes/interactive/interactive-mode.ts:378`), pushes a change into the running one from the
//! `/settings` handler (`:4757-4760`) and from `applyRuntimeSettings` (`:1995`), and re-reads
//! `getFullscreenExitOutput()` at the moment it stops (`:6556`).
//!
//! The three things asserted here are exactly the three places a cached copy could go stale:
//!
//! 1. the `/settings` row's `AppCommand::ApplySetting` arm must push into the LIVE alternate screen
//!    (upstream `:4757-4760`);
//! 2. a screen entered AFTER the row is cycled must be BUILT with the new value (upstream `:378`);
//! 3. the exit teardown must read the current value, not one latched at boot (upstream `:6556`).
//!
//! These drive the real `App::execute_command` / `App::handle_input` paths against a real
//! faux-provider-backed session, the same shape `settings_inert_keys.rs` uses, and observe the
//! renderer's own answer — `AppAction::CopySelection` carries the string the clipboard write would
//! have taken (`app/input.rs`), so its presence or absence IS the copy-on-select decision.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{
    KeyModifiers, MouseButton, MouseEvent, MouseEventKind as Kind, MouseEventKind,
};

use crate::component::InputEvent;
use crate::{App, AppAction, AppCommand, FullscreenExitOutput, UiTheme};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// A session backed by the offline faux provider — no network, no settings on disk, so every
/// getter answers its documented default until a row is cycled.
async fn session(tmp: &TempDir) -> Arc<AgentSession> {
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::from(faux_assistant_message(
        vec![faux_text("a1")],
        StopReason::Stop,
    ))]);
    let provider: Arc<dyn Provider> = faux;
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

fn app_with_a_document() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    app.transcript_mut()
        .push_status("selectable text for the drag");
    app
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// Drag across one document row and release — pi's "selects visible text with the mouse and copies
/// it after a generic release" gesture (`packages/tui/test/tui-alt-screen.test.ts:945`), driven
/// through `App::handle_input` so the answer is the app's, not the renderer's in isolation.
fn drag_and_release(app: &mut App<TestBackend>, row: u16) -> AppAction {
    app.handle_input(&mouse(Kind::Down(MouseButton::Left), 0, row));
    app.handle_input(&mouse(Kind::Drag(MouseButton::Left), 30, row));
    app.handle_input(&mouse(Kind::Up(MouseButton::Left), 30, row))
}

/// The text a full-width drag copies, or `None` if NO row of the rendered document yields one.
///
/// Every row is tried because the transcript's vertical rhythm puts spacers around a status entry
/// and a whitespace-only span deliberately copies nothing (`altscreen/selection.rs`); which row
/// carries the glyphs is a rendering detail this test has no business pinning.
fn copied_text(app: &mut App<TestBackend>) -> Option<String> {
    for row in 0..10 {
        if let AppAction::CopySelection(text) = drag_and_release(app, row)
            && !text.trim().is_empty()
        {
            // The status entry renders with the transcript's one-column inset; the leading space is
            // part of the visible row and is deliberately copied, exactly as it is upstream.
            return Some(text.trim().to_string());
        }
    }
    None
}

/// (1) and the baseline: the default copies, and cycling the `/settings` row to `false` stops the
/// LIVE alternate screen copying — pi's `onFullscreenCopyOnSelectChange`, which persists and then
/// `if (this.renderer instanceof TuiAltScreen) this.renderer.setCopyOnSelect(enabled)`
/// (`interactive-mode.ts:4757-4760`).
///
/// RED before this change: `AppCommand::ApplySetting` had no arm for the id, so the release kept
/// answering `CopySelection` and the key was a row that changed a JSON file and nothing else.
#[tokio::test]
async fn cycling_the_copy_on_select_row_reaches_the_live_alternate_screen() {
    let tmp = TempDir::new().unwrap();
    let session = session(&tmp).await;
    let mut app = app_with_a_document();
    let _captured = app
        .enter_fullscreen_captured()
        .expect("the capture renderer builds");
    app.draw().unwrap();

    // Baseline — pi's `?? true`: a release copies.
    assert_eq!(
        copied_text(&mut app).as_deref(),
        Some("selectable text for the drag"),
        "the default must copy on select"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenCopyOnSelect".to_string(),
            value: "false".to_string(),
        },
        &session,
        None,
    )
    .await;

    assert_eq!(
        copied_text(&mut app),
        None,
        "with the row off no release may reach the clipboard"
    );

    // And back on, without re-entering the renderer.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenCopyOnSelect".to_string(),
            value: "true".to_string(),
        },
        &session,
        None,
    )
    .await;
    assert_eq!(
        copied_text(&mut app).as_deref(),
        Some("selectable text for the drag"),
        "re-enabling must restore the copy on the very next release"
    );
}

/// (2) A screen entered AFTER the row was cycled is BUILT with the setting — pi's `copyOnSelect:
/// options.fullscreenCopyOnSelect` constructor argument, which `switchTuiMode` re-reads from the
/// settings manager for the incoming renderer (`interactive-mode.ts:871`, over `:378`).
///
/// This is the half a live-only push would miss, and the reason `App` holds the value at all: in a
/// regular-mode session there is no renderer to push into when the row is cycled.
///
/// RED before this change: `adopt_fullscreen_renderer` seeded nothing, so a screen entered after
/// the row was cycled came up with the renderer's own `true`.
#[tokio::test]
async fn a_screen_entered_after_the_row_is_cycled_is_built_with_the_setting() {
    let tmp = TempDir::new().unwrap();
    let session = session(&tmp).await;
    let mut app = app_with_a_document();

    // Cycled in REGULAR mode: there is no alternate screen to push into.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenCopyOnSelect".to_string(),
            value: "false".to_string(),
        },
        &session,
        None,
    )
    .await;

    let _captured = app
        .enter_fullscreen_captured()
        .expect("the capture renderer builds");
    app.draw().unwrap();
    assert_eq!(
        copied_text(&mut app),
        None,
        "the screen must be built with the setting the app already held"
    );
}

/// (3) The exit teardown reads the CURRENT `fullscreenExitOutput`, not one latched at boot — pi's
/// `stop(fullscreenExitOutput = this.settingsManager.getFullscreenExitOutput())`
/// (`interactive-mode.ts:6556` @v0.84.4), whose default argument is evaluated per call.
///
/// The repaint itself is asserted in `fullscreen_scrollback.rs`; what this pins is that cycling the
/// row moves the decision the exit path consults.
///
/// RED before this change: `AppCommand::ApplySetting` had no arm for the id and
/// `preserve_screen_on_exit` did not exist.
#[tokio::test]
async fn cycling_the_exit_output_row_moves_the_exit_decision() {
    let tmp = TempDir::new().unwrap();
    let session = session(&tmp).await;
    let mut app = app_with_a_document();
    assert!(
        !app.preserve_screen_on_exit(),
        "pi's documented default is `transcript`"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenExitOutput".to_string(),
            value: "resume-hint".to_string(),
        },
        &session,
        None,
    )
    .await;
    assert!(
        app.preserve_screen_on_exit(),
        "`resume-hint` must skip the exit repaint"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenExitOutput".to_string(),
            value: "transcript".to_string(),
        },
        &session,
        None,
    )
    .await;
    assert!(!app.preserve_screen_on_exit(), "and back");

    // The getter's degrade rule reaches the cached copy too: anything but `resume-hint` is
    // `transcript` (`settings-manager.ts:1213`), so a value from a newer pi cannot land here as a
    // third state.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenExitOutput".to_string(),
            value: "something-newer".to_string(),
        },
        &session,
        None,
    )
    .await;
    assert!(
        !app.preserve_screen_on_exit(),
        "an unrecognized spelling degrades to `transcript`, it does not latch"
    );
    app.set_fullscreen_exit_output(FullscreenExitOutput::ResumeHint);
    assert!(app.preserve_screen_on_exit());
}
