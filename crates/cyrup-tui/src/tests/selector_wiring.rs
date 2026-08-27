//! **Every state seam a dialog exposes must be reachable from a user action.**
//!
//! The batch-9 selector work added a row of setters — `set_parent_paths`,
//! `set_current_session_path`, `set_project_mode_available`, `set_write_scope`,
//! `set_override_state`, `SettingRow::with_description`, plus the three keymap adopters
//! `set_select_keymap` / `set_editor_keymap` / `TrustSelector::with_hints` — and wired **none** of
//! them: their only callers were tests, so each fix rendered correctly in a fixture and never once
//! in the product.
//!
//! These tests therefore drive the USER ACTION, not the setter: a slash command through
//! `App::execute_command` against a real faux-provider-backed `AgentSession`, or a
//! `keybindings.json` load followed by the command that reads it. A test that called the setter
//! directly would pass with the wiring deleted, which is the exact failure being closed.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::string_slice
)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::StopReason;
use cyrup_provider::faux::{faux_assistant_message, faux_text, FauxProvider, FauxResponseStep};
use cyrup_provider::Provider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, Settings};
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppCommand, InputEvent, SelectorKind, UiTheme};
use ratatui::backend::TestBackend;
use tempfile::TempDir;
use super::harness::*;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(110, 34), UiTheme::dark()).unwrap()
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
    Fixture { _tmp: tmp, cwd, agent_dir }
}

/// A session backed by the offline faux provider (no network, no paid tokens).
async fn session(fx: &Fixture) -> Arc<AgentSession> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![FauxResponseStep::from(faux_assistant_message(
        vec![faux_text("a1")],
        StopReason::Stop,
    ))]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    Arc::new(SessionBuilder::new(provider, cfg).cli_settings(Settings::new()).build().await.unwrap())
}

/// One completed turn, so the running session is persisted and `list_sessions()` can see it.
async fn one_turn(session: &Arc<AgentSession>) {
    let _ = session.prompt("first").await.unwrap();
    session.wait_for_idle().await;
}

/// The foreground of the first cell of `needle` on the row that carries it.
fn fg_of(app: &App<TestBackend>, needle: &str) -> ratatui::style::Color {
    let (y, row) = row_with(app, needle);
    let col = row.find(needle).map(|b| row[..b].chars().count()).unwrap();
    let buf = app.terminal().backend().buffer();
    buf.cell((col as u16, y)).unwrap().fg
}

/// Write a second, hand-built session file into the SAME session directory whose header names
/// `parent` as its `parentSession`. This is the on-disk shape `SessionInfo.parent_session_path`
/// is read from (`cyrup-session/src/listing.rs:280` ← `header.parent_session`, camelCase on the
/// wire), i.e. the shape a real `/fork` leaves behind.
fn write_child_session(dir: &std::path::Path, cwd: &std::path::Path, parent: &std::path::Path) {
    let id = "01890000-0000-7000-8000-000000000abc";
    let header = serde_json::json!({
        "type": "session",
        "version": 3,
        "id": id,
        "timestamp": "2026-08-09T10:00:00Z",
        "cwd": cwd.display().to_string(),
        "parentSession": parent.display().to_string(),
    });
    let message = serde_json::json!({
        "type": "message",
        "id": "m1",
        "parentId": null,
        "message": { "role": "user", "content": [{ "type": "text", "text": "forked child" }] },
    });
    // Written AFTER the parent, so its mtime is the newer one and `buildSessionTree`'s
    // `latestActivity` sort (`session-selector.ts:231-251`) keeps the pair together.
    std::fs::write(dir.join(format!("{id}.jsonl")), format!("{header}\n{message}\n")).unwrap();
}

// ---------------------------------------------------------------------------------------------
// `/resume` — `set_parent_paths` + `set_current_session_path` + the `app.session.*` keymap
// ---------------------------------------------------------------------------------------------

/// **`set_parent_paths`.** Upstream reads `SessionInfo.parentSessionPath` straight off the listing
/// (`session-selector.ts:222`) and `buildSessionTree`/`buildTreePrefix` (`:209-254`, `:522-530`)
/// turn the edges into `└─`/`├─` connectors. cyrup's edge map had no producer, so the threaded
/// view — the DEFAULT sort (`:293`) — was permanently a flat forest and the connectors it can draw
/// were unreachable from `/resume`.
///
/// The user action is opening `/resume` in a directory that holds a forked session.
#[tokio::test]
async fn resume_draws_tree_connectors_for_a_forked_session_on_disk() {
    let fx = fixture();
    let session = session(&fx).await;
    one_turn(&session).await;
    let parent = session
        .list_sessions()
        .first()
        .map(|s| s.path.clone())
        .expect("the running session is persisted");
    write_child_session(session.session_dir(), &fx.cwd, &parent);

    let mut app = app();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Session), &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Session));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("forked child"), "both sessions are listed:\n{text}");
    assert!(
        text.contains("└─ ") || text.contains("├─ "),
        "the child must hang off its parent (`buildTreePrefix`, :522-530):\n{text}"
    );
}

/// **`set_current_session_path`.** `currentSessionFilePath` is a constructor argument upstream
/// (`session-selector.ts:761`, stored at `:337`) and it drives the `accent` row colour at
/// `:486-490`. Nothing fed cyrup's, so the session you are sitting in rendered exactly like the
/// others.
///
/// The user action is opening `/resume` from inside a live session.
#[tokio::test]
async fn resume_accents_the_row_of_the_session_you_are_in() {
    let fx = fixture();
    let session = session(&fx).await;
    one_turn(&session).await;
    write_child_session(session.session_dir(), &fx.cwd, std::path::Path::new("/nowhere.jsonl"));

    let mut app = app();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Session), &session, None).await;
    app.draw().unwrap();
    let theme = UiTheme::dark();
    // The running session's row carries its first user message; the hand-written one does not.
    assert_eq!(
        fg_of(&app, "first"),
        theme.accent_style().fg.unwrap(),
        "the current session is accent (:489-490):\n{}",
        buf_text(&app)
    );
    assert_ne!(
        fg_of(&app, "forked child"),
        theme.accent_style().fg.unwrap(),
        "and no other row is:\n{}",
        buf_text(&app)
    );
}

/// **The `app.session.*` table.** `keyHint("app.session.delete", "delete")` resolves through
/// `getKeybindings()` (`keybinding-hints.ts:34-44`), which is the app's MERGED table — so a user
/// `keybindings.json` has to reach `/resume`, both in the hint row and in the handler.
///
/// The user action is loading a `keybindings.json` and then opening `/resume`.
#[tokio::test]
async fn a_rebound_session_key_reaches_both_the_resume_hint_row_and_its_handler() {
    let fx = fixture();
    let session = session(&fx).await;
    one_turn(&session).await;

    let mut app = app();
    app.load_keybindings_json(r#"{ "app.session.delete": "ctrl+k" }"#).unwrap();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Session), &session, None).await;
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("ctrl+k delete"), "the hint names the user's key:\n{text}");
    assert!(!text.contains("ctrl+d delete"), "and not the stock one:\n{text}");

    // The handler moved with it: the old key does nothing, the new one opens the confirmation.
    app.handle_input(&InputEvent::Key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL)));
    app.draw().unwrap();
    assert!(!buf_text(&app).contains("Delete session?"), "ctrl+d is unbound now");
    app.handle_input(&InputEvent::Key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)));
    app.draw().unwrap();
    assert!(buf_text(&app).contains("Delete session?"), "ctrl+k is bound: {}", buf_text(&app));
}

// ---------------------------------------------------------------------------------------------
// `/settings` — `SettingRow::with_description`
// ---------------------------------------------------------------------------------------------

/// **`SettingRow::with_description`.** Every one of upstream's `/settings` items carries a
/// `description` (`settings-selector.ts:494-654`) and `SettingsList` renders the highlighted row's
/// as a wrapped dim block under the list (`settings-list.ts:152-160`). cyrup's field was set by
/// nothing, so the block never appeared and `/settings` never explained what a row does.
///
/// The user action is opening `/settings` and moving the highlight.
#[tokio::test]
async fn settings_shows_the_highlighted_rows_description_block() {
    let fx = fixture();
    let session = session(&fx).await;

    let mut app = app();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Settings), &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Settings));
    app.draw().unwrap();
    let text = buf_text(&app);
    // Row 0 is "Theme" (`settings-selector.ts:646-653`).
    assert!(text.contains("Color theme for the interface"), "row 0's description:\n{text}");

    // Moving the highlight swaps the block for the new row's text (`:152-160` reads
    // `displayItems[selectedIndex]`).
    app.handle_input(&key(KeyCode::Down));
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(
        text.contains("Automatically compact context when it gets too large"),
        "row 1's description (`settings-selector.ts:498`):\n{text}"
    );
    assert!(!text.contains("Color theme for the interface"), "only ONE block at a time:\n{text}");
}

// ---------------------------------------------------------------------------------------------
// The three keymap adopters
// ---------------------------------------------------------------------------------------------

/// **`CheckboxSelector::set_select_keymap` + `set_models_keymap`.** `getFooterText` resolves every
/// key it prints through `keyText` (`scoped-models-selector.ts:198-204`) — the toggle key from
/// `tui.select.confirm`, the rest from `app.models.*`. Both tables have to be the app's merged
/// ones; the stock defaults are what the component starts with.
///
/// The user action is loading a `keybindings.json` and then opening `/scoped-models`.
#[test]
fn scoped_models_footer_names_the_users_own_keys() {
    let mut app = App::new(TestBackend::new(110, 30), UiTheme::dark()).unwrap();
    app.load_keybindings_json(
        r#"{ "tui.select.confirm": "space", "app.models.save": "ctrl+w" }"#,
    )
    .unwrap();
    app.open_checkbox_selector(
        vec![("m0".into(), "Model Zero".into(), "openai".into(), None)],
        Some(vec!["m0".into()]),
    );
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("space toggle"), "the confirm key comes from the merged table:\n{text}");
    assert!(text.contains("ctrl+w save"), "and so do the app.models.* keys:\n{text}");
    assert!(!text.contains("enter toggle"), "not the stock default:\n{text}");
    assert!(!text.contains("ctrl+s save"), "not the stock default:\n{text}");
}

/// **`ModelSelector::set_editor_keymap`.** `getScopeHintText` is
/// `keyHint("tui.input.tab", "scope") + …` (`model-selector.ts:228-230`), resolved live. cyrup's
/// editor tier owns that binding, and nothing handed it to `/model`.
///
/// The user action is loading a `keybindings.json` and then opening `/model`. (cyrup spells
/// upstream's `tui.input.tab` as `editor.tab` — `EditorAction::Tab`, `keymap.rs:171` — because the
/// binding lives in the editor tier here; it is the same one binding either way.)
#[test]
fn model_scope_hint_names_the_users_own_tab_key() {
    let mut app = App::new(TestBackend::new(110, 30), UiTheme::dark()).unwrap();
    app.load_keybindings_json(r#"{ "editor.tab": "ctrl+i" }"#).unwrap();
    app.open_model_selector(
        vec![
            crate::ModelEntry {
                id: "m0".to_string(),
                name: "Model Zero".to_string(),
                provider: "openai".to_string(),
                current: true,
                scoped: true,
            },
            crate::ModelEntry {
                id: "m1".to_string(),
                name: "Model One".to_string(),
                provider: "openai".to_string(),
                current: false,
                scoped: false,
            },
        ],
        None,
    );
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("ctrl+i scope (all/scoped)"), "the live tab binding:\n{text}");
    assert!(!text.contains("tab scope"), "not the stock default:\n{text}");
}

/// **`TrustSelector::with_hints`.** `keyHint("tui.select.confirm", "save")` /
/// `keyHint("tui.select.cancel", "cancel")` (`trust-selector.ts:78-82`) read `getKeybindings()` on
/// every render, i.e. from the FIRST paint. `TrustSelector::handle` adopts whatever table routed a
/// key, but that is one paint too late — a user who never presses a key sees the stock hints.
///
/// The user action is loading a `keybindings.json` and then opening `/trust`.
#[tokio::test]
async fn trust_hint_row_names_the_users_own_keys_on_the_first_paint() {
    let fx = fixture();
    let session = session(&fx).await;

    let mut app = app();
    app.load_keybindings_json(r#"{ "tui.select.confirm": "ctrl+y" }"#).unwrap();
    app.execute_command(AppCommand::OpenSelector(SelectorKind::Trust), &session, None).await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Trust));
    // NO key is pressed before this draw — that is the whole point.
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("ctrl+y save"), "the live confirm binding on paint 1:\n{text}");
    assert!(!text.contains("enter save"), "not the stock default:\n{text}");
}
