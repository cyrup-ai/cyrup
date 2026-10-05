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
    // The startup block is the document's first rows, and its text is selectable too: leave it out
    // so the drag below has exactly one piece of text to find.
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = crate::StartupHeader::Hidden;
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

// ---------------------------------------------------------------------------------------------
// TUI-136 / CFG-100 — `fullscreenWheelScrollLines`
// ---------------------------------------------------------------------------------------------

/// An app whose transcript is tall enough that a wheel notch of any size has room to move.
fn app_with_a_tall_document() -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    for i in 0..80 {
        app.transcript_mut().push_status(format!("row {i}"));
    }
    app
}

fn wheel_up(app: &mut App<TestBackend>, modifiers: KeyModifiers) {
    app.handle_input(&InputEvent::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 2,
        row: 2,
        modifiers,
    }));
}

fn top(app: &mut App<TestBackend>) -> usize {
    app.altscreen_for_test().unwrap().viewport_top()
}

async fn apply(app: &mut App<TestBackend>, session: &Arc<AgentSession>, value: &str) {
    app.execute_command(
        AppCommand::ApplySetting {
            id: "fullscreenWheelScrollLines".to_string(),
            value: value.to_string(),
        },
        session,
        None,
    )
    .await;
}

/// Cycling the `/settings` row reaches the LIVE alternate screen — pi's
/// `onFullscreenWheelScrollLinesChange`, which persists and then `if (this.renderer instanceof
/// TuiAltScreen) this.renderer.setWheelScrollLines(lines)` (`interactive-mode.ts:5050-5053`
/// @v1.0.0). With the setting at 3 a notch moves 3 rows and Alt moves 3 x 5 = 15 — the Alt
/// multiplier lands on the setting's count, after the accelerator.
///
/// Red before this change: the id had no `ApplySetting` arm and the renderer had no count to push
/// into, so every notch stayed one row.
#[tokio::test]
async fn cycling_the_wheel_row_reaches_the_live_alternate_screen() {
    let tmp = TempDir::new().unwrap();
    let session = session(&tmp).await;
    let mut app = app_with_a_tall_document();
    let _captured = app.enter_fullscreen_captured().unwrap();
    app.draw().unwrap();

    let start = top(&mut app);
    wheel_up(&mut app, KeyModifiers::NONE);
    assert_eq!(
        top(&mut app),
        start - 1,
        "the renderer's own default is one row"
    );

    apply(&mut app, &session, "3").await;
    app.draw().unwrap();
    let before = top(&mut app);
    wheel_up(&mut app, KeyModifiers::NONE);
    assert_eq!(top(&mut app), before - 3, "the pushed setting is the count");
    wheel_up(&mut app, KeyModifiers::ALT);
    assert_eq!(
        top(&mut app),
        before - 3 - 15,
        "Alt multiplies the setting: 3 x 5"
    );

    // `auto` resets the gesture, so the first notch after it is one row on every platform.
    apply(&mut app, &session, "auto").await;
    let before = top(&mut app);
    wheel_up(&mut app, KeyModifiers::NONE);
    assert_eq!(
        top(&mut app),
        before - 1,
        "an isolated `auto` notch is one row"
    );
}

/// A screen entered AFTER the row was cycled is BUILT with the setting — pi's
/// `wheelScrollLines: options.fullscreenWheelScrollLines ?? "auto"` constructor option
/// (`tui-renderer.ts:38`), re-read for the incoming renderer on every mode switch. The half a
/// live-only push misses: in a regular-mode session there is no renderer to push into.
///
/// Red before this change: `adopt_fullscreen_renderer` seeded nothing.
#[tokio::test]
async fn a_screen_entered_after_the_wheel_row_is_cycled_is_built_with_the_setting() {
    let tmp = TempDir::new().unwrap();
    let session = session(&tmp).await;
    let mut app = app_with_a_tall_document();
    apply(&mut app, &session, "5").await;

    let _captured = app.enter_fullscreen_captured().unwrap();
    app.draw().unwrap();
    let before = top(&mut app);
    wheel_up(&mut app, KeyModifiers::NONE);
    assert_eq!(top(&mut app), before - 5);
}

/// The row stores a clamped NUMBER (or the string `auto`), the way `setFullscreenWheelScrollLines`
/// does (`settings-manager.ts:1396-1400`) — a numeric string would read back as `"auto"`, and an
/// out-of-range value reaching the command is clamped on WRITE, independently of the read clamp.
///
/// Red before this change: `parse_setting_value("500")` persisted 500 verbatim and the live count
/// was never set.
#[tokio::test]
async fn the_wheel_row_persists_a_clamped_number() {
    let tmp = TempDir::new().unwrap();
    // `session()` leaves the settings store at its default; this one needs the file it writes.
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd.clone(), agent_dir.clone());
    cfg.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = Arc::new(
        SessionBuilder::new(provider, cfg)
            .settings_store(Arc::new(cyrup_config::FileSettingsStore::new(
                agent_dir.join("settings.json"),
                cwd.join(".cyrup").join("settings.json"),
            )))
            .build()
            .await
            .unwrap(),
    );
    let mut app = app_with_a_tall_document();
    let stored = || -> serde_json::Value {
        let text = std::fs::read_to_string(tmp.path().join("agent").join("settings.json")).unwrap();
        serde_json::from_str::<serde_json::Value>(&text).unwrap()["fullscreenWheelScrollLines"]
            .clone()
    };

    apply(&mut app, &session, "500").await;
    assert_eq!(stored(), serde_json::json!(100), "clamped on write");
    apply(&mut app, &session, "10").await;
    assert_eq!(stored(), serde_json::json!(10), "a JSON number, not \"10\"");
    apply(&mut app, &session, "auto").await;
    assert_eq!(stored(), serde_json::json!("auto"));
}

/// The `/settings` row (`settings-selector.ts:733-745` @v1.0.0): label "Fullscreen wheel
/// scrolling", the current value as text, and `auto` followed by `1, 2, 3, 5, 10` with the current
/// value merged in ascending, once — so a hand-edited 7 (or a clamped 100) is still on the cycle.
///
/// Red before this change: `settings_rows` emitted no such row.
#[test]
fn the_settings_grid_offers_the_wheel_scrolling_row() {
    let rows_for = |json: &str| {
        let eff = cyrup_session_svc::EffectiveSettings::from_settings(
            cyrup_session_svc::Settings::parse(json).unwrap(),
        );
        crate::app::settings_rows(
            &eff,
            "dark",
            &crate::keymap::Keymap::default(),
            "medium",
            false,
            &cyrup_session_svc::EnvVars::default(),
        )
    };
    let strings = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    let rows = rows_for("{}");
    let row = rows
        .iter()
        .find(|r| r.id == "fullscreenWheelScrollLines")
        .expect("pi's `fullscreen-wheel-scroll-lines` row is missing");
    assert_eq!(row.label, "Fullscreen wheel scrolling");
    assert_eq!(row.value, "auto", "pi's documented default");
    assert_eq!(row.cycle, strings(&["auto", "1", "2", "3", "5", "10"]));
    let idx = |id: &str| rows.iter().position(|r| r.id == id).unwrap();
    assert!(idx("fullscreenCopyOnSelect") < idx("fullscreenWheelScrollLines"));

    let seven = rows_for(r#"{"fullscreenWheelScrollLines":7}"#);
    let row = seven
        .iter()
        .find(|r| r.id == "fullscreenWheelScrollLines")
        .unwrap();
    assert_eq!(row.value, "7");
    assert_eq!(row.cycle, strings(&["auto", "1", "2", "3", "5", "7", "10"]));

    // A hand-edited 500 shows as the clamped 100, last on the cycle.
    let clamped = rows_for(r#"{"fullscreenWheelScrollLines":500}"#);
    let row = clamped
        .iter()
        .find(|r| r.id == "fullscreenWheelScrollLines")
        .unwrap();
    assert_eq!(row.value, "100");
    assert_eq!(
        row.cycle,
        strings(&["auto", "1", "2", "3", "5", "10", "100"])
    );

    // A value already on the list is not duplicated.
    let five = rows_for(r#"{"fullscreenWheelScrollLines":5}"#);
    let row = five
        .iter()
        .find(|r| r.id == "fullscreenWheelScrollLines")
        .unwrap();
    assert_eq!(row.cycle, strings(&["auto", "1", "2", "3", "5", "10"]));
}
