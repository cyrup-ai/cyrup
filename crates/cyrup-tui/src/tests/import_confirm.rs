//! TUI-081 — `/import <path>` asks before it replaces the live session.
//!
//! Pi's `handleImportCommand` (`modes/interactive/interactive-mode.ts:6062-6095` @v0.84.4) runs
//! `await this.showExtensionConfirm("Import session", `Replace current session with
//! ${inputPath}?`)` (`:6069`) BEFORE `runtimeHost.importFromJsonl` (`:6076`), and shows
//! `Import cancelled` (`:6071`) when the answer is not `Yes`. `showExtensionConfirm` is a Yes/No
//! `showExtensionSelector` whose Escape resolves `undefined` → `false` (`:2557-2565`).
//!
//! cyrup used to call `import_from_jsonl` straight from the `/import` arm, so a mistyped path
//! destroyed the in-flight conversation with no prompt. These tests drive the real
//! `/import` → confirm → answer path against a faux-provider `AgentSessionRuntime` (the fixture
//! `runtime_swap.rs` uses) and pin: the prompt opens with pi's title and body and nothing has been
//! imported yet; `No` and Escape both leave the session untouched and push pi's status; `Yes` runs
//! the import and swaps the runtime.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::Arc;

use super::harness::*;
use crate::crossterm::event::KeyCode;
use crate::{App, AppAction, AppCommand, SelectorKind, UiTheme};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSessionRuntime, SessionConfig, SessionFactory, SessionTarget};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
    config: SessionConfig,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut config = SessionConfig::new(cwd, agent_dir);
    config.trust_override = Some(true);
    Fixture { tmp, config }
}

async fn runtime(fx: &Fixture) -> Arc<AgentSessionRuntime> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let factory = Arc::new(SessionFactory::new(provider, fx.config.clone()));
    AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap()
}

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

/// Submit `/import <path>` through the real editor → dispatch path and execute the routed command
/// against the runtime, exactly as the run loop's `AppAction::Command` arm does.
async fn submit_import(app: &mut App<TestBackend>, rt: &Arc<AgentSessionRuntime>, path: &str) {
    app.editor_mut().set_text(&format!("/import {path}"));
    let action = app.handle_input(&key(KeyCode::Enter));
    let AppAction::Command(cmd) = action else {
        panic!("`/import` routes a command, got {action:?}");
    };
    assert_eq!(cmd, AppCommand::Import(Some(path.to_string())));
    let session = rt.session().await;
    app.execute_command(cmd, &session, Some(rt)).await;
}

/// Answer the open confirm prompt with the key sequence `keys` and execute whatever command the
/// selector produced (an Escape produces none).
async fn answer(app: &mut App<TestBackend>, rt: &Arc<AgentSessionRuntime>, keys: &[KeyCode]) {
    let mut last = AppAction::None;
    for code in keys {
        last = app.handle_input(&key(*code));
    }
    if let AppAction::Command(cmd) = last {
        let session = rt.session().await;
        app.execute_command(cmd, &session, Some(rt)).await;
    }
}

/// A real session JSONL to import: one turn on the live session, copied out of the sessions dir
/// under a name the import can copy back in.
async fn exported_session(fx: &Fixture, rt: &Arc<AgentSessionRuntime>) -> PathBuf {
    let session = rt.session().await;
    let _ = session.prompt("remember this").await.unwrap();
    session.wait_for_idle().await;
    let live = session
        .session_file()
        .await
        .expect("the running session is persisted");
    let exported = fx.tmp.path().join("exported.jsonl");
    std::fs::copy(&live, &exported).unwrap();
    exported
}

/// `/import <path>` opens the guard with pi's title and body (`:6069`) and imports NOTHING yet:
/// the runtime's generation is still 0 and the path is only parked.
#[tokio::test]
async fn import_opens_pis_confirm_prompt_before_touching_the_session() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();
    assert_eq!(rt.generation().await, 0);

    submit_import(&mut app, &rt, "typo.jsonl").await;

    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ImportConfirm),
        "the confirm prompt occupies the input slot"
    );
    app.draw().unwrap();
    let text = buf_text(&app);
    assert!(text.contains("Import session"), "pi's title:\n{text}");
    assert!(
        text.contains("Replace current session with typo.jsonl?"),
        "pi's body names the path:\n{text}"
    );
    assert!(
        text.contains("Yes") && text.contains("No"),
        "Yes/No rows:\n{text}"
    );
    assert_eq!(
        rt.generation().await,
        0,
        "nothing is imported until the prompt is answered"
    );
    assert!(
        !app.scrollback_text().contains("import error"),
        "the mistyped path must not have been tried:\n{}",
        app.scrollback_text()
    );
}

/// Choosing `No` is pi's `!confirmed` branch (`:6070-6072`): `Import cancelled`, session untouched,
/// prompt closed.
#[tokio::test]
async fn declining_the_prompt_cancels_the_import() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();
    let id0 = rt.session().await.session_id().to_string();

    submit_import(&mut app, &rt, "typo.jsonl").await;
    // `Yes` is highlighted first (pi's `SelectList` starts at index 0); Down reaches `No`.
    answer(&mut app, &rt, &[KeyCode::Down, KeyCode::Enter]).await;

    assert_eq!(app.active_selector_kind(), None, "the prompt closed");
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("Import cancelled"),
        "pi's status (`:6071`):\n{}",
        app.scrollback_text()
    );
    assert_eq!(rt.generation().await, 0, "no swap");
    assert_eq!(rt.session().await.session_id().to_string(), id0);
    assert!(
        !app.scrollback_text().contains("import error"),
        "a declined import never reaches `import_from_jsonl`:\n{}",
        app.scrollback_text()
    );
}

/// Escape is the same decline: `showExtensionSelector` resolves `undefined`, `showExtensionConfirm`
/// reads it as `false` (`:2564`), and `handleImportCommand` shows `Import cancelled`.
#[tokio::test]
async fn escaping_the_prompt_cancels_the_import() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();

    submit_import(&mut app, &rt, "typo.jsonl").await;
    let action = app.handle_input(&esc());
    assert!(
        !matches!(action, AppAction::Command(_)),
        "Escape produces no command, got {action:?}"
    );

    assert_eq!(app.active_selector_kind(), None, "the prompt closed");
    app.draw().unwrap();
    assert!(
        app.scrollback_text().contains("Import cancelled"),
        "pi's status:\n{}",
        app.scrollback_text()
    );
    assert_eq!(rt.generation().await, 0, "no swap");

    // The parked path died with the decline: a later stray `Yes` cannot import it.
    let session = rt.session().await;
    app.execute_command(
        AppCommand::ConfirmSelection {
            kind: SelectorKind::ImportConfirm,
            value: "yes".to_string(),
        },
        &session,
        Some(&rt),
    )
    .await;
    assert_eq!(
        rt.generation().await,
        0,
        "no path is pending after a decline"
    );
}

/// `Yes` is the only way to `importFromJsonl` (`:6076`): the runtime swaps to the imported session
/// and the re-bind surfaces pi's `Session imported from: {path}` (`:6082`).
#[tokio::test]
async fn confirming_the_prompt_imports_and_swaps() {
    let fx = fixture();
    let rt = runtime(&fx).await;
    let mut app = app();
    let exported = exported_session(&fx, &rt).await;
    let live0 = rt.session().await.session_file().await;
    assert_eq!(rt.generation().await, 0);

    let path = exported.display().to_string();
    submit_import(&mut app, &rt, &path).await;
    assert_eq!(
        app.active_selector_kind(),
        Some(SelectorKind::ImportConfirm)
    );
    answer(&mut app, &rt, &[KeyCode::Enter]).await;

    assert_eq!(app.active_selector_kind(), None, "the prompt closed");
    assert_eq!(
        rt.generation().await,
        1,
        "`Yes` runs the import, which swaps the runtime"
    );
    // The install is the COPY `import_from_jsonl` placed in the sessions dir (Pi copies into
    // `sessionManager.getSessionDir()`, `agent-session-runtime.ts:367`), not the live file it
    // replaced and not the source path the user typed.
    let live1 = rt.session().await.session_file().await;
    assert_ne!(
        live1, live0,
        "the runtime now holds a different session file"
    );
    assert_ne!(live1.as_deref(), Some(exported.as_path()));
    assert_eq!(
        live1.as_deref().and_then(|p| p.file_name()),
        exported.file_name(),
        "the imported copy keeps the source file name"
    );
    app.rebind_session();
    app.draw().unwrap();
    assert!(
        app.scrollback_text()
            .contains(&format!("Session imported from: {path}")),
        "pi's success status (`:6082`):\n{}",
        app.scrollback_text()
    );
}
