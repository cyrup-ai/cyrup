//! CFG-015 follow-up — four settings keys that were parsed, displayed in `/settings`, and then
//! consumed by **nothing**: `enableSkillCommands`, `treeFilterMode`, `editorPaddingX` and
//! `showHardwareCursor`.
//!
//! Every one has a real consumer upstream at pi v0.83.0:
//!
//! * `enableSkillCommands` gates the `skill:<name>` half of the interactive autocomplete list
//!   (`coding-agent/src/modes/interactive/interactive-mode.ts:610-622`). It is deliberately
//!   interactive-ONLY — the `get_commands` RPC (`rpc-mode.ts:676-690`) emits skills unconditionally.
//! * `treeFilterMode` is read into `initialFilterMode` and handed to `TreeSelectorComponent`
//!   (`interactive-mode.ts:4644` → `tree-selector.ts:133-137`), so it is the filter `/tree` OPENS
//!   with.
//! * `editorPaddingX` becomes `CustomEditor({ paddingX })` and `setPaddingX(...)`
//!   (`interactive-mode.ts:470-474,1727-1733,5393-5399` → `tui/src/components/editor.ts:484-522`).
//! * `showHardwareCursor` becomes `new TUI(terminal, getShowHardwareCursor(), …)` /
//!   `ui.setShowHardwareCursor(...)` (`interactive-mode.ts:459,1721,5401` → `tui/src/tui.ts:346-352`
//!   and `:1659-1663`, where a false value makes every frame call `terminal.hideCursor()`).
//!
//! In cyrup each getter had exactly ONE caller — the `/settings` display row — so cycling any of
//! them changed a JSON file and nothing else. These tests drive the REAL `App::execute_command`
//! paths a user reaches (`/settings` cycle → `AppCommand::ApplySetting`; `/tree` open →
//! `AppCommand::OpenSelector(SelectorKind::Tree)`) against a REAL faux-provider-backed
//! `AgentSession`, and assert on the RENDERED frame / the live command registry.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::Arc;

use super::harness::{buf_text, key_event as key};
use crate::crossterm::event::KeyCode;
use crate::{App, AppCommand, SelectorKind, UiTheme};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, FauxResponseStep, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, Settings};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap()
}

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

/// Drop a real Agent-Skills `SKILL.md` into `<cwd>/.cyrup/skills/<name>/` so the session's resource
/// loader discovers it and `slash_command_catalog()` emits a genuine `skill:<name>` row. Nothing is
/// stubbed: this is the same on-disk layout a user's project uses.
fn write_skill(cwd: &std::path::Path, name: &str) {
    let dir = cwd.join(".cyrup").join("skills").join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: does the {name} thing\n---\n\nbody\n"),
    )
    .unwrap();
}

/// A session backed by the offline faux provider (no network, no paid tokens), carrying `cli`
/// as its highest-precedence settings layer.
async fn session_with(fx: &Fixture, cli: Settings) -> Arc<AgentSession> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::from(faux_assistant_message(
        vec![faux_text("a1")],
        StopReason::Stop,
    ))]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    Arc::new(
        SessionBuilder::new(provider, cfg)
            .cli_settings(cli)
            .build()
            .await
            .unwrap(),
    )
}

/// One completed turn, so `session_dag()` has entries for `/tree` to open over.
async fn one_turn(session: &Arc<AgentSession>) {
    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
}

fn settings(json: &str) -> Settings {
    Settings::parse(json).unwrap()
}

/// Type `/skill` into the live editor and render — the ACTUAL `/` autocomplete menu a user sees,
/// not an inspected data structure. Returns the whole frame.
fn open_slash_menu(app: &mut App<TestBackend>) -> String {
    app.editor_mut().clear();
    for c in "/skill".chars() {
        app.editor_mut().handle_key(&key(KeyCode::Char(c)));
    }
    app.draw().unwrap();
    buf_text(app)
}

/// The editor's text row, located by the buffer content it is showing.
///
/// It used to be located by the editor's prompt glyph `›`. E1 removed that glyph — pi's
/// `Editor.render` (`editor.ts:482-601`) emits only
/// `${leftPadding}${displayText}${padding}${lineRightPadding}` (`:578`), with no leading glyph
/// anywhere in the chat editor's construction — so the row is now found by `needle`, which is the
/// text the caller typed into the buffer. The column arithmetic below is unchanged: `leftPadding`
/// is `" ".repeat(paddingX)` (`:522`), so the first text column IS `editorPaddingX`.
fn prompt_line(app: &App<TestBackend>, needle: &str) -> String {
    buf_text(app)
        .lines()
        .find(|l| l.contains(needle))
        .map(str::to_string)
        .unwrap_or_else(|| panic!("no editor text row in the frame:\n{}", buf_text(app)))
}

// ------------------------------------------------------------------ treeFilterMode --------------

/// `/tree` must OPEN with the configured `treeFilterMode`.
///
/// Before the fix `TreeSelector::new(nodes)` hard-seeded `FilterMode::Default` and
/// `tree_filter_mode()`'s only caller was the `/settings` display row, so a user who set
/// `"treeFilterMode": "labeled-only"` got the default view every single time.
#[tokio::test]
async fn tree_opens_with_the_configured_filter_mode() {
    let fx = fixture();
    let session = session_with(&fx, settings(r#"{"treeFilterMode":"labeled-only"}"#)).await;
    one_turn(&session).await;

    let mut app = app();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Tree), &session, None)
        .await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::Tree),
        "the /tree selector did not open — the session DAG was empty"
    );
    app.draw().unwrap();

    let screen = buf_text(&app);
    assert!(
        screen.contains("Filter: labeled"),
        "the tree header must show the configured filter, not the hardcoded default:\n{screen}"
    );
    assert!(
        !screen.contains("Filter: default"),
        "the tree still opened on the hardcoded default filter:\n{screen}"
    );
}

/// The other end of the same wire: an ABSENT setting still opens on `default`, so the fix cannot be
/// a blanket behavior change.
#[tokio::test]
async fn tree_opens_on_default_when_the_setting_is_unset() {
    let fx = fixture();
    let session = session_with(&fx, Settings::new()).await;
    one_turn(&session).await;

    let mut app = app();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Tree), &session, None)
        .await;
    app.draw().unwrap();
    assert!(
        buf_text(&app).contains("Filter: default"),
        "unset must still mean `default`"
    );
}

// --------------------------------------------------------------- enableSkillCommands ------------

/// Cycling the `/settings` "Skill commands" row OFF must remove `skill:<name>` from the live `/`
/// menu — Pi's `if (this.settingsManager.getEnableSkillCommands())` around the skill half of
/// `createBaseAutocompleteProvider` (`interactive-mode.ts:613`).
///
/// Before the fix `enable_skill_commands()`'s only caller was the row that DISPLAYS it, so the
/// toggle wrote JSON and the skill stayed in the menu forever.
#[tokio::test]
async fn disabling_skill_commands_removes_skill_rows_from_the_slash_menu() {
    let fx = fixture();
    write_skill(&fx.cwd, "deploy");
    let session = session_with(&fx, Settings::new()).await;

    // The catalog the `/` menu is built from really does carry the skill.
    let catalog = session.slash_command_catalog();
    assert!(
        catalog
            .iter()
            .any(|r| r.get("name").and_then(|v| v.as_str()) == Some("skill:deploy")),
        "fixture skill never reached slash_command_catalog(): {catalog:?}"
    );

    let mut app = app();
    // Seed the registry the way the run loop's boot block does (default: skill commands ON).
    app.editor_mut()
        .set_registry(crate::CommandRegistry::with_dynamic(
            crate::dynamic_commands_from_catalog_gated(&catalog, true),
        ));
    assert!(
        open_slash_menu(&mut app).contains("skill:deploy"),
        "baseline: the `/` menu must offer the skill before anything is toggled"
    );

    // The real user path: cycle the `/settings` "Skill commands" row.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "enableSkillCommands".to_string(),
            value: "false".to_string(),
        },
        &session,
        None,
    )
    .await;

    let off = open_slash_menu(&mut app);
    assert!(
        !off.contains("skill:deploy"),
        "`enableSkillCommands: false` must drop skill rows from the `/` menu:\n{off}"
    );
    // …and ONLY skill rows: builtins survive (Pi gates the skill half alone).
    let builtins = {
        app.editor_mut().clear();
        for c in "/mod".chars() {
            app.editor_mut().handle_key(&key(KeyCode::Char(c)));
        }
        app.draw().unwrap();
        buf_text(&app)
    };
    assert!(
        builtins.contains("model"),
        "builtin commands must be untouched:\n{builtins}"
    );

    // Turning it back on restores them, rebuilt from the same catalog.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "enableSkillCommands".to_string(),
            value: "true".to_string(),
        },
        &session,
        None,
    )
    .await;
    assert!(
        open_slash_menu(&mut app).contains("skill:deploy"),
        "re-enabling must restore the skill rows"
    );
}

// ------------------------------------------------------------------- editorPaddingX -------------

/// Cycling `/settings` "Editor padding" must inset the editor text on the very next frame — Pi's
/// `onEditorPaddingChange` → `defaultEditor.setPaddingX(...)` (`interactive-mode.ts:5393-5399` →
/// `tui/src/components/editor.ts:484-522`).
///
/// Before the fix `editor_padding_x()` reached only the `/settings` display row and the editor had
/// no padding concept at all, so the prompt glyph sat flush against column 0 for every value.
#[tokio::test]
async fn editor_padding_insets_the_prompt_row_on_the_next_frame() {
    let fx = fixture();
    let session = session_with(&fx, Settings::new()).await;
    let mut app = app();
    app.editor_mut().set_text("hello");
    app.draw().unwrap();

    let flush = prompt_line(&app, "hello");
    let flush_col = flush.find("hello").unwrap();
    assert_eq!(
        flush_col, 0,
        "baseline (padding 0) must render the text flush at column 0"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "editorPaddingX".to_string(),
            value: "3".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();

    let padded = prompt_line(&app, "hello");
    let padded_col = padded.find("hello").unwrap();
    assert_eq!(
        padded_col, 3,
        "`editorPaddingX: 3` must inset the editor text by 3 columns, got column {padded_col} in:\n{padded}"
    );
    // Pi pads the TEXT rows only — the rule spans the full width (`editor.ts:530`
    // `horizontal.repeat(width)`), so the frame must still carry a full-width rule.
    let full_rule = "─".repeat(100);
    assert!(
        buf_text(&app).lines().any(|l| l == full_rule),
        "the editor rules must still span the full width when padded:\n{}",
        buf_text(&app)
    );
}

/// A persisted `editorPaddingX` must be honored without any `/settings` interaction — the value the
/// run loop seeds the editor with at boot. Asserted through the same rendered frame.
#[tokio::test]
async fn persisted_editor_padding_is_honored_by_the_render() {
    let fx = fixture();
    let session = session_with(&fx, settings(r#"{"editorPaddingX":2}"#)).await;
    let mut app = app();
    // Exactly what `App::run`'s boot block does with the effective settings.
    app.editor_mut()
        .set_padding_x(session.services().settings.effective().editor_padding_x());
    app.editor_mut().set_text("hi");
    app.draw().unwrap();
    assert_eq!(
        prompt_line(&app, "hi").find("hi").unwrap(),
        2,
        "persisted padding must reach the frame"
    );
}

// --------------------------------------------------------------- showHardwareCursor -------------

/// `showHardwareCursor` must decide whether the terminal's real cursor is shown at all — Pi's
/// `tui.ts:1659-1663` (`if (this.showHardwareCursor) showCursor() else hideCursor()`), whose default
/// is OFF (`tui.ts:312`, `settings-manager.ts:1182`).
///
/// Before the fix `show_hardware_cursor()` reached only the `/settings` display row and the editor
/// unconditionally called `Frame::set_cursor_position`, so the hardware cursor was always visible
/// regardless of the setting — the opposite of Pi's default.
#[tokio::test]
async fn show_hardware_cursor_gates_the_terminal_cursor() {
    let fx = fixture();
    let session = session_with(&fx, Settings::new()).await;
    let mut app = app();
    app.editor_mut().set_text("hello");
    app.draw().unwrap();
    assert!(
        !app.terminal().backend().cursor_visible(),
        "Pi's default is OFF — the hardware cursor must be hidden until the setting turns it on"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "showHardwareCursor".to_string(),
            value: "true".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    assert!(
        app.terminal().backend().cursor_visible(),
        "`showHardwareCursor: true` must place (and therefore show) the hardware cursor"
    );

    app.execute_command(
        AppCommand::ApplySetting {
            id: "showHardwareCursor".to_string(),
            value: "false".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    assert!(
        !app.terminal().backend().cursor_visible(),
        "turning it back off must hide the hardware cursor again"
    );
}

/// The hardware cursor, when enabled, lands INSIDE the padded content area — the two keys interact,
/// and Pi prefixes the same `leftPadding` to the cursor row (`editor.ts:522`).
#[tokio::test]
async fn hardware_cursor_respects_editor_padding() {
    let fx = fixture();
    let session = session_with(&fx, Settings::new()).await;
    let mut app = app();
    // Typed, not empty: an empty buffer puts the caret at `leftPadding + 0`, which cannot
    // distinguish "the padding is honored" from "the padding is honored AND nothing else is
    // silently prefixed" — the exact confusion that let the `› ` glyph ride along here.
    app.editor_mut().set_text("hello");
    app.execute_command(
        AppCommand::ApplySetting {
            id: "showHardwareCursor".to_string(),
            value: "true".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.execute_command(
        AppCommand::ApplySetting {
            id: "editorPaddingX".to_string(),
            value: "2".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();

    let pos = app.terminal().backend().cursor_position();
    // padding (2) + the 5 columns of `hello` before the end-of-line caret. E1 removed the two
    // columns the `› ` glyph used to add here; pi's caret sits at `leftPadding + visibleWidth(text
    // before it)` and nothing else (`editor.ts:522,546,559`).
    assert_eq!(
        pos.x, 7,
        "the hardware cursor must sit past the padding and the typed text, got {pos:?}"
    );
}
