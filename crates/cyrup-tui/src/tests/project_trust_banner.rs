//! TUI-N04 — the untrusted-project warning banner.
//!
//! ```ts
//! // pi v0.83.0 coding-agent/src/modes/interactive/interactive-mode.ts:3479-3514
//! renderInitialMessages(): void {
//!     const entries = this.sessionManager.buildContextEntries();
//!     this.renderSessionEntries(entries, { updateFooter: true, populateHistory: true });
//!     this.renderProjectTrustWarningIfNeeded();
//!     …
//! }
//!
//! private renderProjectTrustWarningIfNeeded(): void {
//!     if (this.settingsManager.isProjectTrusted() || !hasTrustRequiringProjectResources(this.sessionManager.getCwd())) {
//!         return;
//!     }
//!     if (this.chatContainer.children.length > 0) this.chatContainer.addChild(new Spacer(1));
//!     this.chatContainer.addChild(new Text(theme.fg("warning",
//!         `This project is not trusted. Project ${CONFIG_DIR_NAME} resources and packages are ignored. Use /trust to save a trust decision, then restart pi.`), 1, 0));
//! }
//! ```
//!
//! cyrup rendered it **nowhere**. Both halves of the predicate already existed and neither had a
//! reader on this path — `AgentSessionServices::project_trusted` (`services.rs:104`, read only by
//! the `/trust` dialog) and `cyrup_config::trust::has_trust_requiring_resources` (`trust.rs:201`,
//! read only by `SessionBuilder` at `builder.rs:597`) — so opening cyrup in a repo that ships
//! `.cyrup/` skills, prompts, themes or settings and has not been trusted silently ignored all of
//! them with no indication on screen and no pointer to `/trust`. It is the surface that tells the
//! user a security decision is in force.
//!
//! These tests drive the real `App::render_project_trust_warning_if_needed` seam — the one both the
//! boot path (`App::run`, before the first frame) and the `session_swapped` arm call — and read the
//! COMMITTED SCROLLBACK, i.e. what the user actually sees, including the warning colour.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use crate::{App, UiTheme};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use ratatui::backend::TestBackend;

/// The rebranded banner, spelled out here rather than imported so a silent edit to the constant is
/// a RED test and not a green one (`interactive-mode.ts:3506-3509` with `${CONFIG_DIR_NAME}` =
/// `.cyrup` and the closing `pi` = `cyrup`).
const BANNER: &str = "This project is not trusted. Project .cyrup resources and packages are ignored. Use /trust to save a trust decision, then restart cyrup.";

fn new_app() -> App<TestBackend> {
    // Wide enough that the banner is not wrapped mid-phrase, so `contains` reads the real string.
    App::new(TestBackend::new(200, 24), UiTheme::dark()).unwrap()
}

/// A session in `dir/project`, with `dir/home` as HOME so the `.agents/skills` ancestor walk in
/// `has_trust_requiring_resources` cannot escape into the developer's real home directory.
async fn session(dir: &std::path::Path, trusted: bool, with_resources: bool) -> Arc<AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    let home = dir.join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    if with_resources {
        // One of `has_trust_requiring_resources`'s `.cyrup` markers (`trust.rs:202-213`).
        std::fs::create_dir_all(cwd.join(".cyrup")).unwrap();
        std::fs::write(cwd.join(".cyrup").join("settings.json"), "{}").unwrap();
    }
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.home = home;
    cfg.trust_override = Some(trusted);
    Arc::new(SessionBuilder::new(faux, cfg).build().await.unwrap())
}

/// Push the banner through the real seam and return the committed scrollback.
fn commit(app: &mut App<TestBackend>, session: &Arc<AgentSession>) -> String {
    app.render_project_trust_warning_if_needed(session);
    app.draw().unwrap();
    app.scrollback_text()
}

/// The item's own scenario, and RED at HEAD before this landed: an untrusted project that ships
/// `.cyrup/settings.json` produced no banner at all.
#[tokio::test]
async fn an_untrusted_project_with_cyrup_resources_shows_the_banner() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path(), false, true).await;
    assert!(
        !session.services().project_trusted,
        "fixture must actually be untrusted"
    );
    let mut app = new_app();
    let out = commit(&mut app, &session);
    assert!(
        out.contains(BANNER),
        "the trust banner is missing from the transcript:\n{out}"
    );
}

/// `theme.fg("warning", …)` (`:3505`) — the colour is the signal that this is a security notice and
/// not chatter, so assert it and not just the text.
#[tokio::test]
async fn the_banner_is_painted_in_the_warning_colour() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path(), false, true).await;
    let mut app = new_app();
    commit(&mut app, &session);
    let theme = UiTheme::dark();
    let painted = app.scrollback_lines().iter().any(|line| {
        line.spans.iter().any(|s| {
            s.content.contains("This project is not trusted")
                && line.style.patch(s.style).fg == theme.warning_style().fg
        })
    });
    assert!(
        painted,
        "banner is not warning-coloured:\n{}",
        app.scrollback_text()
    );
}

/// `if (this.settingsManager.isProjectTrusted() … ) return;` (`:3497`) — the first half of pi's
/// guard. Assert presence first (above), then absence.
#[tokio::test]
async fn a_trusted_project_shows_no_banner() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path(), true, true).await;
    assert!(
        session.services().project_trusted,
        "fixture must actually be trusted"
    );
    let mut app = new_app();
    let out = commit(&mut app, &session);
    assert!(
        !out.contains("This project is not trusted"),
        "a trusted project must be silent — pi returns early at `:3497`:\n{out}"
    );
}

/// `|| !hasTrustRequiringProjectResources(cwd)` (`:3497`) — the second half. An untrusted project
/// with nothing to gate has nothing to warn about, so the banner must not fire on every bare
/// directory.
#[tokio::test]
async fn an_untrusted_project_with_nothing_to_gate_shows_no_banner() {
    let dir = tempfile::tempdir().unwrap();
    let session = session(dir.path(), false, false).await;
    assert!(!session.services().project_trusted);
    let mut app = new_app();
    let out = commit(&mut app, &session);
    assert!(
        !out.contains("This project is not trusted"),
        "no `.cyrup` resources means nothing is being ignored:\n{out}"
    );
}
