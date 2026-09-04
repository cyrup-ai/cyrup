//! The **automatic** session/cwd window title — Pi `updateTerminalTitle`
//! (`pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:818-826`).
//!
//! Cyrup already had the OSC 0 primitive (`App`'s `write_terminal_title`, Pi `terminal.ts:504-507`)
//! but only an extension's `ui.setTitle` ever reached it, so nothing ever titled the window by
//! itself and several cyrup sessions in adjacent tabs/panes were indistinguishable. Pi titles the
//! window `${APP_TITLE} - ${sessionName} - ${cwdBasename}` at startup (`:860`), on a session
//! (re-)bind (`:1761`), when the extension set is unbound (`:1995`) and on every
//! `session_info_changed` (`:2900-2903`).
//!
//! These tests drive the production seams: the pure composer
//! ([`crate::session_terminal_title`]) and the two `App` entry points the crossterm run loop
//! calls — `App::update_terminal_title` (which the loop turns into the OSC 0 write) and
//! `App::ingest_event`'s `SessionInfoChanged` arm.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;

use crate::{APP_TITLE, App, UiTheme, session_terminal_title};
use cyrup_session_svc::AgentSessionEvent;
use ratatui::backend::TestBackend;

fn app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap()
}

/// Pi's two branches, verbatim (`interactive-mode.ts:820-825`).
#[test]
fn composer_matches_pis_two_branches() {
    let cwd = PathBuf::from("/home/u/src/cyrup");
    assert_eq!(
        APP_TITLE, "cyrup",
        "the rebranded APP_TITLE (config.ts:490)"
    );
    assert_eq!(
        session_terminal_title(Some("nightly audit"), &cwd),
        "cyrup - nightly audit - cyrup"
    );
    assert_eq!(session_terminal_title(None, &cwd), "cyrup - cyrup");
    // `if (sessionName)` is a JS truthiness test, so an empty name takes the un-named branch.
    assert_eq!(session_terminal_title(Some(""), &cwd), "cyrup - cyrup");
}

/// The title is composed from the session's cwd + name and is recomputed only when it changed —
/// the exact value the crossterm run loop hands to the OSC 0 writer at startup (Pi `:860`).
#[test]
fn update_terminal_title_composes_and_deduplicates() {
    let mut app = app();
    app.set_title_cwd(PathBuf::from("/home/u/work/my-repo"));

    // Startup, un-named session: `cyrup - <cwd basename>`.
    assert_eq!(
        app.update_terminal_title().as_deref(),
        Some("cyrup - my-repo")
    );
    assert_eq!(
        app.state().terminal_title.as_deref(),
        Some("cyrup - my-repo")
    );
    // Nothing moved ⇒ nothing to write (Pi calls `setTitle` only from the four sites above).
    assert_eq!(app.update_terminal_title(), None);
}

/// `session_info_changed` must reach BOTH the footer's location line and the window title —
/// Pi's arm is `updateTerminalTitle()` + `footer.invalidate()` (`:2900-2903`).
#[test]
fn a_rename_retitles_the_window_and_the_footer() {
    let mut app = app();
    app.set_title_cwd(PathBuf::from("/home/u/work/my-repo"));
    app.update_terminal_title();

    app.ingest_event(&AgentSessionEvent::SessionInfoChanged {
        name: Some("nightly audit".to_string()),
    });

    assert_eq!(
        app.state().terminal_title.as_deref(),
        Some("cyrup - nightly audit - my-repo"),
        "the rename must recompute the window title"
    );
    assert_eq!(
        app.state().status.session_name.as_deref(),
        Some("nightly audit"),
        "and must reach the footer's location line (footer.ts:116-130)"
    );

    // Clearing the name falls back to Pi's un-named branch.
    app.ingest_event(&AgentSessionEvent::SessionInfoChanged { name: None });
    assert_eq!(
        app.state().terminal_title.as_deref(),
        Some("cyrup - my-repo")
    );
    assert_eq!(app.state().status.session_name, None);
}
