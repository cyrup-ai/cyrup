//! TUI-121 — in-session `/resume` opens at once and streams its listing in.
//!
//! ```ts
//! // pi v0.87.1 coding-agent/src/modes/interactive/components/session-selector.ts
//! // constructor, :869 — the picker is built empty, then:
//! void this.loadScope("current");
//! // loadScope, :941-988 — header.setLoading(true); onProgress(loaded, total, partialSessions)
//! // drops each partial set into the list and shows `Loading loaded/total`; the resolved set ends
//! // the loading state. cancelLoads(), :872-883 — select / cancel / exit abort the loads.
//! ```
//!
//! cyrup read every session file of every project in full on the run loop's own task before the
//! picker was built, so the UI froze between `/resume` and the picker. These tests make the
//! listing BLOCK deterministically — one of the session "files" is a FIFO, which a reader blocks on
//! until a writer opens it — and assert on what the open picker does while the listing is stuck,
//! then after it is released.
#![cfg(unix)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::harness::{buf_text, esc, key};
use crate::crossterm::event::KeyCode;
use crate::{App, AppCommand, SelectorKind, SessionListMsg, SessionListUpdate, UiTheme};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig, SessionLayout};
use ratatui::backend::TestBackend;
use tempfile::TempDir;

pub(super) struct Fixture {
    _tmp: TempDir,
    pub(super) cwd: PathBuf,
    pub(super) agent_dir: PathBuf,
}

pub(super) fn fixture() -> Fixture {
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

async fn session(fx: &Fixture) -> Arc<AgentSession> {
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    Arc::new(SessionBuilder::new(provider, cfg).build().await.unwrap())
}

/// The current folder's session directory — the one `/resume`'s first loader lists.
pub(super) fn session_dir(fx: &Fixture) -> PathBuf {
    let dir = SessionLayout::new(fx.agent_dir.join("sessions"), fx.cwd.clone()).dir();
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

pub(super) fn session_lines(id: &str, cwd: &Path, text: &str) -> String {
    let header = serde_json::json!({
        "type": "session", "version": 3, "id": id,
        "timestamp": "2026-08-09T10:00:00Z", "cwd": cwd.display().to_string(),
    });
    let message = serde_json::json!({
        "type": "message", "id": "m1", "parentId": null, "timestamp": "2026-08-09T10:00:01Z",
        "message": {"role": "user", "content": text, "timestamp": 1},
    });
    format!("{header}\n{message}\n")
}

/// A listing that blocks until released: `dir` holds one ordinary session file, read FIRST (pi
/// reads file names descending, `session-manager.ts:951`), and a FIFO read SECOND, on which the
/// listing blocks until [`Blocker::release`] writes a session into it.
///
/// A guard thread is the FIFO's only writer. It writes when released — or, after a long backstop,
/// on its own, so a listing wrongly run on the caller's thread still finishes (and the test fails
/// on `released_early`) instead of hanging the suite.
pub(super) struct Blocker {
    release_tx: std::sync::mpsc::Sender<()>,
    /// Set by the guard just before it opens the FIFO: the listing can only have got past the FIFO
    /// once this is `true`.
    released: Arc<AtomicBool>,
    guard: Option<std::thread::JoinHandle<()>>,
}

impl Blocker {
    pub(super) fn new(dir: &Path, cwd: &Path) -> Self {
        std::fs::write(
            dir.join("2026-08-09T10-00-00-000Z_aaaa.jsonl"),
            session_lines(
                "01890000-0000-7000-8000-00000000aaaa",
                cwd,
                "the newest session",
            ),
        )
        .unwrap();
        let fifo = dir.join("2025-01-01T00-00-00-000Z_bbbb.jsonl");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo failed");
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let released = Arc::new(AtomicBool::new(false));
        let flag = released.clone();
        let body = session_lines(
            "01890000-0000-7000-8000-00000000bbbb",
            cwd,
            "the blocked session",
        );
        let guard = std::thread::spawn(move || {
            let _ = release_rx.recv_timeout(Duration::from_secs(5));
            flag.store(true, Ordering::SeqCst);
            // Blocks until the listing opens the FIFO for reading, then hands it the session.
            let mut w = std::fs::OpenOptions::new().write(true).open(&fifo).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        });
        Blocker {
            release_tx,
            released,
            guard: Some(guard),
        }
    }

    pub(super) fn released_early(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    /// Let the listing past the FIFO, and wait until the writer has handed it the session.
    pub(super) async fn release(&mut self) {
        let _ = self.release_tx.send(());
        let guard = self.guard.take().unwrap();
        tokio::task::spawn_blocking(move || guard.join().unwrap())
            .await
            .unwrap();
    }
}

async fn next_msg(rx: &mut tokio::sync::mpsc::UnboundedReceiver<SessionListMsg>) -> SessionListMsg {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("the listing reported nothing")
        .expect("channel closed")
}

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(140, 34), UiTheme::dark()).unwrap()
}

async fn open_resume(app: &mut App<TestBackend>, session: &Arc<AgentSession>) {
    app.execute_command(
        AppCommand::OpenSelector(SelectorKind::Session),
        session,
        None,
    )
    .await;
}

/// The row's Verify, end to end: the picker is open, drawn in its loading state and taking keys
/// while the listing is still blocked; the first batch lands before the second; releasing the
/// listing completes it.
///
/// **Red without the fix:** the listing ran on the caller's task, so opening `/resume` sat on the
/// FIFO until the guard's backstop released it — `released_early` is `true` by the time the picker
/// exists.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_picker_opens_before_the_listing_finishes_and_stays_interactive() {
    let fx = fixture();
    let session = session(&fx).await;
    let mut blocker = Blocker::new(&session_dir(&fx), &fx.cwd);

    let mut app = new_app();
    let mut rx = app.install_session_list_channel();
    open_resume(&mut app, &session).await;
    assert!(
        !blocker.released_early(),
        "`/resume` waited for the listing before the picker opened"
    );
    assert_eq!(app.active_selector_kind(), Some(SelectorKind::Session));

    // Open, empty, `Loading ...` (`session-selector.ts:141-143`).
    app.draw().unwrap();
    let view = buf_text(&app);
    assert!(view.contains("Loading ..."), "{view}");
    assert_eq!(app.session_selector_mut().unwrap().visible_len(), 0);

    // The first batch: pi publishes after the first file (`loaded === 1`, `session-manager.ts:963`).
    let first = next_msg(&mut rx).await;
    match &first.update {
        SessionListUpdate::Progress {
            loaded: 1,
            total: 2,
            partial: Some(set),
        } => assert_eq!(set.rows.len(), 1),
        other => panic!("expected the first partial batch, got {other:?}"),
    }
    app.apply_session_list_msg(first);
    app.draw().unwrap();
    let view = buf_text(&app);
    assert!(view.contains("Loading 1/2"), "{view}");
    assert!(view.contains("the newest session"), "{view}");
    assert!(!view.contains("the blocked session"), "{view}");

    // Still blocked: nothing more arrives…
    assert!(
        tokio::time::timeout(Duration::from_millis(150), rx.recv())
            .await
            .is_err(),
        "the listing should be parked on the FIFO"
    );
    // …yet the picker takes keys: the search filters the batch it has, and clears again. The
    // query is a character no row's search text can hold — that text includes the session's cwd,
    // whose tempdir name is random alphanumerics, so a letter would sometimes match.
    app.handle_input(&key(KeyCode::Char('§')));
    assert_eq!(app.session_selector_mut().unwrap().visible_len(), 0);
    app.handle_input(&key(KeyCode::Backspace));
    assert_eq!(app.session_selector_mut().unwrap().visible_len(), 1);

    blocker.release().await;
    loop {
        let msg = next_msg(&mut rx).await;
        let done = matches!(msg.update, SessionListUpdate::Done(_));
        app.apply_session_list_msg(msg);
        if done {
            break;
        }
    }
    let picker = app.session_selector_mut().unwrap();
    assert_eq!(picker.load_status(), (false, None));
    assert_eq!(picker.visible_len(), 2);
    app.draw().unwrap();
    let view = buf_text(&app);
    assert!(view.contains("the blocked session"), "{view}");
    assert!(view.contains("◉ Current Folder"), "{view}");
}

/// pi `cancelLoads()` on cancel (`session-selector.ts:806-810`, `:872-883`): closing the picker
/// while the listing is blocked aborts it, and the load reports nothing once released.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_picker_cancels_the_listing() {
    let fx = fixture();
    let session = session(&fx).await;
    let mut blocker = Blocker::new(&session_dir(&fx), &fx.cwd);

    let mut app = new_app();
    let mut rx = app.install_session_list_channel();
    open_resume(&mut app, &session).await;
    let first = next_msg(&mut rx).await;
    app.apply_session_list_msg(first);

    app.handle_input(&esc());
    assert_eq!(app.active_selector_kind(), None);

    blocker.release().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .is_err(),
        "a cancelled listing must stop, not report its remaining files"
    );
}

/// pi's `isActive()` (`session-selector.ts:955`): a report from a picker that has since closed is
/// ignored by the one open now, even though both are `/resume` pickers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_report_from_a_closed_picker_is_ignored() {
    let fx = fixture();
    let session = session(&fx).await;
    let dir = session_dir(&fx);
    std::fs::write(
        dir.join("2026-08-09T10-00-00-000Z_cccc.jsonl"),
        session_lines(
            "01890000-0000-7000-8000-00000000cccc",
            &fx.cwd,
            "listed by the first picker",
        ),
    )
    .unwrap();

    let mut app = new_app();
    let mut rx = app.install_session_list_channel();
    open_resume(&mut app, &session).await;
    // The first picker's load finishes, but its reports are held back until it has closed.
    let mut stale = Vec::new();
    loop {
        let msg = next_msg(&mut rx).await;
        let done = matches!(msg.update, SessionListUpdate::Done(_));
        stale.push(msg);
        if done {
            break;
        }
    }
    app.handle_input(&esc());
    open_resume(&mut app, &session).await;

    for msg in stale {
        app.apply_session_list_msg(msg);
    }
    let picker = app.session_selector_mut().unwrap();
    assert_eq!(
        picker.visible_len(),
        0,
        "the closed picker's rows must not land in the new one"
    );
    assert_eq!(
        picker.load_status(),
        (true, None),
        "still its own loading state"
    );
}

/// `Tab` onto the all-projects scope starts that scope's load only then (`toggleScope`,
/// `session-selector.ts:1031-1040`), and its result streams in like the first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tab_loads_the_all_projects_scope_on_demand() {
    let fx = fixture();
    let session = session(&fx).await;
    let other_cwd = fx.cwd.parent().unwrap().join("elsewhere");
    let other = SessionLayout::new(fx.agent_dir.join("sessions"), other_cwd.clone()).dir();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(
        other.join("2026-08-09T10-00-00-000Z_dddd.jsonl"),
        session_lines(
            "01890000-0000-7000-8000-00000000dddd",
            &other_cwd,
            "question from the other project",
        ),
    )
    .unwrap();

    let mut app = new_app();
    let mut rx = app.install_session_list_channel();
    open_resume(&mut app, &session).await;
    // The current folder has no sessions: its load settles empty.
    loop {
        let msg = next_msg(&mut rx).await;
        assert_eq!(msg.scope, crate::SessionScope::Current);
        let done = matches!(msg.update, SessionListUpdate::Done(_));
        app.apply_session_list_msg(msg);
        if done {
            break;
        }
    }
    assert!(
        tokio::time::timeout(Duration::from_millis(150), rx.recv())
            .await
            .is_err(),
        "the all-projects scan must not start before `Tab` asks for it"
    );

    app.handle_input(&key(KeyCode::Tab));
    app.draw().unwrap();
    assert!(buf_text(&app).contains("Loading ..."), "{}", buf_text(&app));
    loop {
        let msg = next_msg(&mut rx).await;
        assert_eq!(msg.scope, crate::SessionScope::All);
        let done = matches!(msg.update, SessionListUpdate::Done(_));
        app.apply_session_list_msg(msg);
        if done {
            break;
        }
    }
    app.draw().unwrap();
    let view = buf_text(&app);
    assert!(view.contains("question from the other project"), "{view}");
    assert!(view.contains("◉ All"), "{view}");
}

/// pi `refreshSessionsAfterMutation` (`session-selector.ts:1024-1029` @v0.87.1), run after a
/// successful rename (`confirmRename`, `:934-935`) or delete (`:859`): the open picker forgets its
/// cached sets and reloads the scope on screen, so the list is re-read from disk rather than left
/// as the picker's own patch of it.
///
/// **Red without the reload:** the rename lands, but the picker stays settled — `load_status()` is
/// `(false, None)` and no load reports again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rename_reloads_the_scope_on_screen() {
    let fx = fixture();
    let session = session(&fx).await;
    let dir = session_dir(&fx);
    let target = dir.join("2026-08-09T10-00-00-000Z_eeee.jsonl");
    std::fs::write(
        &target,
        session_lines(
            "01890000-0000-7000-8000-00000000eeee",
            &fx.cwd,
            "a session to rename",
        ),
    )
    .unwrap();

    let mut app = new_app();
    let mut rx = app.install_session_list_channel();
    open_resume(&mut app, &session).await;
    loop {
        let msg = next_msg(&mut rx).await;
        let done = matches!(msg.update, SessionListUpdate::Done(_));
        app.apply_session_list_msg(msg);
        if done {
            break;
        }
    }
    assert_eq!(
        app.session_selector_mut().unwrap().load_status(),
        (false, None)
    );

    app.execute_command(
        AppCommand::RenameSession {
            path: target.display().to_string(),
            name: "renamed on disk".to_string(),
        },
        &session,
        None,
    )
    .await;
    // `loadScope(this.scope)`: the header is back in its loading state, and the rows on screen stay
    // until the reload's first batch replaces them.
    let picker = app.session_selector_mut().unwrap();
    assert_eq!(
        picker.load_status(),
        (true, None),
        "the scope was not reloaded"
    );
    assert_eq!(picker.visible_len(), 1);

    let mut reloaded = None;
    loop {
        let msg = next_msg(&mut rx).await;
        assert_eq!(msg.scope, crate::SessionScope::Current);
        if let SessionListUpdate::Done(set) = &msg.update {
            reloaded = Some(set.rows.clone());
        }
        app.apply_session_list_msg(msg);
        if reloaded.is_some() {
            break;
        }
    }
    let rows = reloaded.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name.as_deref(), Some("renamed on disk"));
    assert_eq!(
        app.session_selector_mut().unwrap().load_status(),
        (false, None)
    );
}
