//! `ConfirmSelectionAsDefault` must reach a handler for BOTH kinds.
//!
//! The gap this closes was invisible to every other test in the suite. `Ctrl+S` in the model
//! picker correctly produced `SelectorOutcome::ConfirmDefault`, `apply_selector_outcome`
//! correctly turned it into `AppCommand::ConfirmSelectionAsDefault { kind: Model, .. }`, and the
//! arm that handles that command was correct too — but the arm lived in
//! `execute_selector_command`, and `execute_command`'s selector bucket lists only
//! `OpenSelector | ConfirmSelection | SetEntryLabel`. The variant therefore fell to the `_` arm,
//! landed in `execute_misc_command`, found no `Model` arm there, and was swallowed by its
//! `_ => {}`. The picker still closed, so the failure was SILENT: no model change, no settings
//! write, and no status line to say why.
//!
//! Nothing unit-tested the seam. The selector tests stop at the outcome; the arm's body was only
//! ever read, never dispatched to. These tests drive the real `execute_command`, which is the only
//! level at which a misrouted arm is observable.
//!
//! The assertion is deliberately weak — *some* status must appear — because the point is
//! reachability, not the message. A wrong message is a different bug; no message at all is this
//! one.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;

use crate::selector::SelectorKind;
use crate::{App, AppCommand, UiTheme};

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap()
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

async fn session(dir: &std::path::Path) -> Arc<AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    Arc::new(SessionBuilder::new(faux, cfg).build().await.unwrap())
}

/// Drive the command and return everything the transcript gained.
async fn status_after(kind: SelectorKind, value: &str) -> String {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path()).await;
    let mut app = new_app();
    let before = {
        app.draw().unwrap();
        screen(&app)
    };
    app.execute_command(
        AppCommand::ConfirmSelectionAsDefault {
            kind,
            value: value.to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    let after = screen(&app);
    assert_ne!(before, after, "the screen did not change at all");
    after
}

/// RED before the arm moved: the model kind produced NOTHING — the dispatcher dropped it.
///
/// An unresolvable id is used on purpose. Whether the session accepts the model is beside the
/// point; either outcome writes a status, and only an unreached arm writes none.
#[tokio::test]
async fn model_kind_reaches_its_arm_and_reports() {
    let out = status_after(SelectorKind::Model, "faux/no-such-model-here").await;
    assert!(
        out.contains("model") || out.contains("Model") || out.contains("settings"),
        "`ConfirmSelectionAsDefault{{Model}}` produced no status — it never reached a handler, \
         which is the silent-drop this test exists for:\n{out}"
    );
}

/// The sibling that always worked, pinned so a future re-shuffle cannot quietly break IT while
/// fixing the other — the two arms must stay in the same dispatcher.
#[tokio::test]
async fn thinking_kind_reaches_its_arm_and_reports() {
    let out = status_after(SelectorKind::Thinking, "high").await;
    assert!(
        out.contains("thinking") || out.contains("Thinking") || out.contains("settings"),
        "`ConfirmSelectionAsDefault{{Thinking}}` produced no status:\n{out}"
    );
}

/// The `/settings` list is built ONCE (`app/execute.rs`'s `C::OpenSelector` arm) and a row cycle
/// keeps the slot open, so a sibling row whose display depends on another row goes stale. Flipping
/// `hideThinkingBlock` must refresh the `Thinking level` row IN PLACE, in both directions.
///
/// This drives `execute_command` for the same reason the tests above do: the marker is applied in
/// the `C::ApplySetting` arm, and a formatter-level assertion passes whether or not that arm ever
/// writes the row through — which is exactly how the staleness shipped.
#[tokio::test]
async fn hide_thinking_refreshes_the_thinking_level_row_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path()).await;
    let mut app = App::new(TestBackend::new(100, 40), UiTheme::dark()).unwrap();

    app.execute_command(
        AppCommand::OpenSelector(SelectorKind::Settings),
        &session,
        None,
    )
    .await;
    // The list windows 10 of 29 rows and `Thinking level` is not in the first page, so filter to
    // it through the real search box. `filtered` holds INDICES into `rows` (`apply_filter`), so a
    // row written through while a filter is active still shows — which is the property under test.
    for c in "thinking level".chars() {
        app.handle_input(&crate::InputEvent::Key(
            crate::crossterm::event::KeyEvent::new(
                crate::crossterm::event::KeyCode::Char(c),
                crate::crossterm::event::KeyModifiers::NONE,
            ),
        ));
    }
    app.draw().unwrap();
    let before = screen(&app);
    assert!(
        before.contains("Thinking level"),
        "the row is not on screen at all:\n{before}"
    );
    assert!(
        !before.contains("(hidden)"),
        "clean before the flip:\n{before}"
    );

    // The command BOTH writers dispatch: the row cycle via the `Apply` payload, and `Ctrl+T`.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "hideThinkingBlock".to_string(),
            value: "true".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    let hidden = screen(&app);
    assert!(
        hidden.contains("(hidden)"),
        "the sibling row did not refresh while the slot stayed open:\n{hidden}"
    );

    // ...and back off, leaving no stale marker for the rest of the dialog.
    app.execute_command(
        AppCommand::ApplySetting {
            id: "hideThinkingBlock".to_string(),
            value: "false".to_string(),
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    let visible = screen(&app);
    assert!(
        !visible.contains("(hidden)"),
        "a stale marker survived the flip back:\n{visible}"
    );
}
