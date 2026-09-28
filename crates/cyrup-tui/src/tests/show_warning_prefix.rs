//! TUI-062 — `showWarning`'s `Warning: ` prefix is built in one place, as pi builds it.
//!
//! ```ts
//! // pi v0.87.1 coding-agent/src/modes/interactive/interactive-mode.ts:4469-4473
//! showWarning(warningMessage: string): void {
//!     this.chatContainer.addChild(new Spacer(1));
//!     this.chatContainer.addChild(new Text(theme.fg("warning", `Warning: ${warningMessage}`), 1, 0));
//!     this.ui.requestRender();
//! }
//! ```
//!
//! cyrup's `Entry::Warning` draws its text verbatim, so the prefix used to be each caller's to
//! remember — and ports of `showWarning` kept forgetting it. The two `/name` warnings are driven
//! here through the real slash-command dispatch; every other migrated site shares the one
//! `TranscriptView::show_warning`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use super::harness::key;
use crate::crossterm::event::KeyCode;
use crate::transcript::Entry;
use crate::{App, AppAction, AppCommand, UiTheme};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;

async fn session(dir: &std::path::Path) -> Arc<AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap())
}

fn warnings(app: &App<TestBackend>) -> Vec<String> {
    app.state()
        .transcript
        .pending()
        .iter()
        .filter_map(|e| match e {
            Entry::Warning(s) => Some(s.clone()),
            _ => None,
        })
        .collect()
}

/// Submit `line` through the real editor → dispatch path and run the command it resolves to.
async fn run_slash(app: &mut App<TestBackend>, session: &Arc<AgentSession>, line: &str) {
    app.editor_mut().set_text(line);
    let AppAction::Command(cmd) = app.handle_input(&key(KeyCode::Enter)) else {
        panic!("`{line}` did not resolve to a command");
    };
    app.execute_command(cmd, session, None).await;
}

/// `this.showWarning("Usage: /name <name>")` (`interactive-mode.ts:6416` @v0.87.1): a bare `/name`
/// on an unnamed session.
#[tokio::test]
async fn bare_name_on_an_unnamed_session_warns_with_the_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path()).await;
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    run_slash(&mut app, &session, "/name").await;
    assert_eq!(
        warnings(&app),
        vec!["Warning: Usage: /name <name>".to_string()]
    );
}

/// `this.showWarning(`Session name was normalized from ${JSON.stringify(name)} to
/// ${JSON.stringify(sessionName)}`)` (`interactive-mode.ts:6425` @v0.87.1): a name the session
/// manager rewrites (`name.replace(/[\r\n]+/g, " ").trim()`).
#[tokio::test]
async fn a_normalized_name_warns_with_the_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path()).await;
    let mut app = App::new(TestBackend::new(100, 30), UiTheme::dark()).unwrap();
    app.execute_command(
        AppCommand::SetName("first\nsecond".to_string()),
        &session,
        None,
    )
    .await;
    assert_eq!(
        warnings(&app),
        vec![
            "Warning: Session name was normalized from \"first\\nsecond\" to \"first second\""
                .to_string()
        ]
    );
}
