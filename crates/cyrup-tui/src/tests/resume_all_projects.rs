//! TUI-122 — in-session `/resume` reaches other projects' sessions.
//!
//! ```ts
//! // pi v0.87.1 coding-agent/src/modes/interactive/interactive-mode.ts:5549-5562
//! private showSessionSelector(): void {
//!     this.showSelector((done) => {
//!         const selector = new SessionSelectorComponent(
//!             (onProgress, signal) => SessionManager.list(this.sessionManager.getCwd(),
//!                 this.sessionManager.getSessionDir(), onProgress, signal),
//!             (onProgress, signal) => this.sessionManager.usesDefaultSessionDir()
//!                 ? SessionManager.listAll(onProgress, signal)
//!                 : SessionManager.listAll(this.sessionManager.getSessionDir(), onProgress, signal),
//!             …
//! ```
//!
//! cyrup's `/resume` handed the picker the current-folder list only, so `Tab` — the scope toggle —
//! had nothing to switch to, and a project with no sessions of its own got a "no saved sessions"
//! status instead of a picker at all. Both cases are driven through the real `/resume` command and
//! a real `Tab` keystroke, reading the drawn frame.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::harness::buf_text;
use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, AppCommand, InputEvent, SelectorKind, UiTheme};
use cyrup_core::StopReason;
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, SessionLayout};
use tempfile::TempDir;

struct Fixture {
    tmp: TempDir,
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
        tmp,
        cwd,
        agent_dir,
    }
}

async fn session(fx: &Fixture) -> Arc<AgentSession> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("a1")],
        StopReason::Stop,
    )]);
    let provider: Arc<dyn Provider> = faux;
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap())
}

/// A session recorded by ANOTHER project, in that project's cwd-encoded directory under the same
/// sessions root — where pi's `listAll()` finds it (`session-manager.ts:1946-1953` @v0.87.1).
fn write_other_project_session(agent_dir: &Path, other_cwd: &Path) {
    let dir = SessionLayout::new(agent_dir.join("sessions"), other_cwd.to_path_buf()).dir();
    std::fs::create_dir_all(&dir).unwrap();
    let id = "01890000-0000-7000-8000-00000000cafe";
    let header = serde_json::json!({
        "type": "session", "version": 3, "id": id,
        "timestamp": "2026-08-09T10:00:00Z", "cwd": other_cwd.display().to_string(),
    });
    let message = serde_json::json!({
        "type": "message", "id": "m1", "parentId": null, "timestamp": "2026-08-09T10:00:01Z",
        "message": {"role": "user", "content": "question from the other project", "timestamp": 1},
    });
    std::fs::write(
        dir.join(format!("{id}.jsonl")),
        format!("{header}\n{message}\n"),
    )
    .unwrap();
}

fn tab() -> InputEvent {
    InputEvent::Key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE))
}

async fn open_resume(session: &Arc<AgentSession>) -> App<ratatui::backend::TestBackend> {
    let mut app = App::new(ratatui::backend::TestBackend::new(140, 34), UiTheme::dark()).unwrap();
    app.execute_command(
        AppCommand::OpenSelector(SelectorKind::Session),
        session,
        None,
    )
    .await;
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Session));
    app.draw().unwrap();
    app
}

/// The row's Verify: `/resume` with a session in another project's directory, then `Tab` → it is
/// listed, with its cwd.
#[tokio::test]
async fn tab_lists_another_projects_session_with_its_cwd() {
    let fx = fixture();
    let session = session(&fx).await;
    let _ = session.prompt("question from this project").await.unwrap();
    session.wait_for_idle().await;
    let other_cwd = fx.tmp.path().join("elsewhere");
    write_other_project_session(&fx.agent_dir, &other_cwd);

    let mut app = open_resume(&session).await;
    let current_view = buf_text(&app);
    assert!(
        current_view.contains("question from this project"),
        "the current folder's session is listed:\n{current_view}"
    );
    assert!(
        !current_view.contains("question from the other project"),
        "the current-folder scope is this project's only:\n{current_view}"
    );

    app.handle_input(&tab());
    app.draw().unwrap();
    let all_view = buf_text(&app);
    assert!(
        all_view.contains("question from the other project")
            && all_view.contains("question from this project"),
        "`Tab` switches to every project's sessions:\n{all_view}"
    );
    assert!(
        all_view.contains("elsewhere"),
        "the all scope names each row's project (`session-selector.ts:468-470`):\n{all_view}"
    );
}

/// pi opens the picker whatever the current folder holds; its empty state points at `Tab`
/// (`session-selector.ts:443`), and `Tab` then lists the rest of the machine's sessions.
#[tokio::test]
async fn a_project_with_no_sessions_still_opens_resume_and_tab_reaches_the_others() {
    let fx = fixture();
    let session = session(&fx).await;
    let other_cwd = fx.tmp.path().join("elsewhere");
    write_other_project_session(&fx.agent_dir, &other_cwd);

    let mut app = open_resume(&session).await;
    assert!(
        buf_text(&app).contains("No sessions in current folder. Press Tab to view all."),
        "{}",
        buf_text(&app)
    );
    app.handle_input(&tab());
    app.draw().unwrap();
    assert!(
        buf_text(&app).contains("question from the other project"),
        "{}",
        buf_text(&app)
    );
}
