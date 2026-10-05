//! Crossing the edges of a fullscreen excursion: a session swap inside it, and leaving it.
//!
//! * **A swap replaces the conversation, not the renderer.** Pi's `rebindCurrentSession` clears
//!   `chatContainer` and `loadedResourcesContainer` and re-adds the new session's panel and
//!   messages; the header, the scroll view and the terminal's capabilities are untouched. So the
//!   document holds exactly one loaded-resources panel, and links stay links.
//! * **Leaving shows the header once.** Pi's `switchTuiMode` re-renders the shared containers on the
//!   incoming renderer, so whichever renderer is live shows the header a single time; the exit
//!   repaint carries the transcript — OSC-8 hyperlinks included — into the user's scrollback.
//! * **The startup header follows `quietStartup`** the way pi's `shouldShowStartupHeader` does.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::Arc;

use ratatui::backend::TestBackend;
use serde_json::json;

use crate::altscreen::{ViewportRenderer, captured_text};
use crate::{App, StartupHeader, StartupReport, UiTheme};

const COLS: u16 = 80;
const ROWS: u16 = 24;

fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}

const CLOSE: &str = "\u{1b}]8;;\u{7}";

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(COLS, ROWS), UiTheme::dark()).unwrap()
}

/// A fullscreen app with the built-in startup block off, so the document holds only what a test
/// puts in it.
fn bare_fullscreen() -> (App<TestBackend>, crate::altscreen::Captured) {
    let mut app = new_app();
    app.state_mut().show_startup_hints = false;
    app.state_mut().startup_header = StartupHeader::Hidden;
    let captured = app.enter_fullscreen_captured().expect("renderer builds");
    (app, captured)
}

fn screen(app: &mut App<TestBackend>) -> Vec<String> {
    app.draw().unwrap();
    let buf = app
        .altscreen_for_test()
        .expect("fullscreen is live")
        .backend_for_test()
        .buffer()
        .clone();
    (0..buf.area.height)
        .map(|y| {
            (0..COLS - 1)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn has(rows: &[String], needle: &str) -> bool {
    rows.iter().any(|r| r.contains(needle))
}

fn count(rows: &[String], needle: &str) -> usize {
    rows.iter().filter(|r| r.contains(needle)).count()
}

fn panel(skill: &str) -> StartupReport {
    StartupReport {
        skills: vec![skill.to_string()],
        ..StartupReport::default()
    }
}

fn read_result(text: &str) -> serde_json::Value {
    json!({ "content": [{ "type": "text", "text": text }], "details": null })
}

// ---------------------------------------------------------------------------------------------
// a session swap inside the excursion
// ---------------------------------------------------------------------------------------------

/// The conversation of the replacing session appears, and the replaced one's does not — a swap used
/// to leave a fullscreen session blank for good, because the fresh view it installed no longer
/// retained its document.
#[test]
fn a_swapped_in_session_is_shown_in_the_fullscreen_document() {
    let (mut app, _captured) = bare_fullscreen();
    app.transcript_mut().push_status("the first conversation");
    assert!(has(&screen(&mut app), "the first conversation"));

    app.rebind_session();
    app.transcript_mut().push_status("the second conversation");
    let rows = screen(&mut app);
    assert!(has(&rows, "the second conversation"), "{rows:#?}");
    assert!(!has(&rows, "the first conversation"), "{rows:#?}");

    // …and the one after that.
    app.rebind_session();
    app.transcript_mut().push_status("the third conversation");
    let rows = screen(&mut app);
    assert!(has(&rows, "the third conversation"), "{rows:#?}");
    assert!(!has(&rows, "the second conversation"), "{rows:#?}");
}

/// The replaced document is not mistaken for the new one when both have the same shape: one entry
/// each, same width, same settings.
#[test]
fn a_swap_whose_document_has_the_same_length_still_repaints() {
    let (mut app, _captured) = bare_fullscreen();
    app.transcript_mut().push_status("old text");
    assert!(has(&screen(&mut app), "old text"));

    app.rebind_session();
    let rows = screen(&mut app);
    assert!(has(&rows, "session replaced"), "{rows:#?}");
    assert!(
        !has(&rows, "old text"),
        "the cached rows were reused: {rows:#?}"
    );
}

/// A reader parked at the top of the old conversation is not left at a stale offset in the new one.
#[test]
fn a_swap_returns_the_view_to_the_tail() {
    let (mut app, _captured) = bare_fullscreen();
    for i in 0..60 {
        app.transcript_mut().push_status(format!("old {i:02}"));
    }
    let _ = screen(&mut app);
    app.altscreen_for_test().unwrap().scroll_to_top();
    let _ = screen(&mut app);

    app.rebind_session();
    for i in 0..60 {
        app.transcript_mut().push_status(format!("new {i:02}"));
    }
    let rows = screen(&mut app);
    assert!(has(&rows, "new 59"), "following the new tail: {rows:#?}");
    assert!(app.altscreen_for_test().unwrap().is_following_output());
}

/// pi's `loadedResourcesContainer.clear()` before every `showLoadedResources`: after two swaps the
/// document holds the second panel only, between the header and the conversation.
#[test]
fn a_second_swap_replaces_the_loaded_resources_panel() {
    let (mut app, _captured) = bare_fullscreen();
    app.push_loaded_resources(&panel("first-skill"));
    app.transcript_mut().push_status("chat one");
    let rows = screen(&mut app);
    assert!(
        has(&rows, "first-skill") && has(&rows, "chat one"),
        "{rows:#?}"
    );

    app.rebind_session();
    app.push_loaded_resources(&panel("second-skill"));
    app.transcript_mut().push_status("chat two");
    let rows = screen(&mut app);
    assert!(has(&rows, "second-skill"), "{rows:#?}");
    assert!(
        !has(&rows, "first-skill"),
        "the first panel is gone: {rows:#?}"
    );
    assert_eq!(count(&rows, "[Skills]"), 1, "{rows:#?}");
    assert!(
        rows.iter()
            .position(|r| r.contains("second-skill"))
            .unwrap()
            < rows.iter().position(|r| r.contains("chat two")).unwrap(),
        "the panel sits above the conversation: {rows:#?}"
    );
}

/// The terminal's OSC-8 capability describes the terminal, not the session: it survives a swap.
#[test]
fn links_stay_links_after_a_swap() {
    let (mut app, _captured) = bare_fullscreen();
    app.transcript_mut().set_hyperlinks(true);
    app.rebind_session();
    app.transcript_mut()
        .set_cwd(Some(PathBuf::from("/tmp/aug-osc")));
    let t = app.transcript_mut();
    t.push_tool_start("read", json!({ "path": "a.rs" }));
    t.push_tool_end("read", false, Some(read_result("body")));
    t.commit_tools();
    app.draw().unwrap();
    let buf = app
        .altscreen_for_test()
        .unwrap()
        .backend_for_test()
        .buffer()
        .clone();
    let raw: String = buf.content.iter().map(|c| c.symbol()).collect();
    assert!(raw.contains(&open("file:///tmp/aug-osc/a.rs")), "{raw:?}");
}

// ---------------------------------------------------------------------------------------------
// the startup header follows quietStartup
// ---------------------------------------------------------------------------------------------

/// `quiet` is the JSON text of the `quietStartup` value: `true`, `false` or `"header"`.
async fn session_with(dir: &std::path::Path, quiet: &str) -> Arc<cyrup_session_svc::AgentSession> {
    let cwd = dir.join("project");
    let agent_dir = dir.join("agent");
    let home = dir.join("home");
    for d in [&cwd, &agent_dir, &home] {
        std::fs::create_dir_all(d).unwrap();
    }
    let faux: Arc<dyn cyrup_provider::Provider> =
        Arc::new(cyrup_provider::faux::FauxProvider::new());
    let mut cfg = cyrup_session_svc::SessionConfig::new(cwd, agent_dir);
    cfg.home = home;
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    Arc::new(
        cyrup_session_svc::SessionBuilder::new(faux, cfg)
            .cli_settings(
                cyrup_config::settings::Settings::parse(&format!("{{\"quietStartup\": {quiet}}}"))
                    .unwrap(),
            )
            .build()
            .await
            .unwrap(),
    )
}

#[tokio::test]
async fn quiet_startup_hides_the_fullscreen_startup_header_unless_verbose() {
    let dir = tempfile::tempdir().unwrap();
    let quiet = session_with(dir.path(), "true").await;

    let mut app = new_app();
    app.push_session_loaded_resources(&quiet);
    assert_eq!(app.state().startup_header, StartupHeader::Hidden);
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    assert!(
        !has(&screen(&mut app), "interrupt"),
        "no header under quietStartup"
    );

    // `--verbose` wins (`shouldShowStartupHeader`: `options.verbose === true || …`).
    let mut app = new_app();
    app.set_verbose_startup(true);
    app.push_session_loaded_resources(&quiet);
    assert_eq!(app.state().startup_header, StartupHeader::Shown);
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    assert!(has(&screen(&mut app), "interrupt"));

    // Not quiet: shown. And decided once — a later swap onto a quiet session does not hide it.
    let loud = session_with(dir.path(), "false").await;
    let mut app = new_app();
    app.push_session_loaded_resources(&loud);
    assert_eq!(app.state().startup_header, StartupHeader::Shown);
    app.push_session_loaded_resources(&quiet);
    assert_eq!(
        app.state().startup_header,
        StartupHeader::Shown,
        "pi builds its header once, in init()"
    );
}

/// `quietStartup` is `true | false | "header"`: the header is hidden only by `true`, and the
/// onboarding line under the hint bar names "loaded resources" only while the details it would
/// reveal are shown (`interactive-mode.ts:1044-1048`, `:1409-1417` @v1.0.0).
#[tokio::test]
async fn the_fullscreen_header_follows_each_quiet_startup_value() {
    const WITH_RESOURCES: &str = "show full startup help and loaded resources.";
    const HELP_ONLY: &str = "show full startup help.";
    let dir = tempfile::tempdir().unwrap();
    // (stored value, decision, header on screen, onboarding promises resources)
    for (stored, decision, header, resources) in [
        ("false", StartupHeader::Shown, true, true),
        ("true", StartupHeader::Hidden, false, false),
        ("\"header\"", StartupHeader::HeaderOnly, true, false),
    ] {
        let session = session_with(dir.path(), stored).await;
        let mut app = new_app();
        app.push_session_loaded_resources(&session);
        assert_eq!(
            app.state().startup_header,
            decision,
            "quietStartup {stored}"
        );
        let _captured = app.enter_fullscreen_captured().expect("renderer builds");
        let rows = screen(&mut app);
        assert_eq!(
            has(&rows, "interrupt"),
            header,
            "quietStartup {stored}: {rows:#?}"
        );
        assert_eq!(
            has(&rows, WITH_RESOURCES),
            header && resources,
            "quietStartup {stored}: {rows:#?}"
        );
        assert_eq!(
            has(&rows, HELP_ONLY),
            header && !resources,
            "quietStartup {stored}: {rows:#?}"
        );
    }
}

/// `--verbose` overrides `"header"` too: the details come back, and so does the onboarding's
/// promise of them.
#[tokio::test]
async fn verbose_overrides_the_header_value() {
    let dir = tempfile::tempdir().unwrap();
    let session = session_with(dir.path(), "\"header\"").await;
    let mut app = new_app();
    app.set_verbose_startup(true);
    app.push_session_loaded_resources(&session);
    assert_eq!(app.state().startup_header, StartupHeader::Shown);
    let _captured = app.enter_fullscreen_captured().expect("renderer builds");
    assert!(has(
        &screen(&mut app),
        "show full startup help and loaded resources."
    ));
}

/// The inline renderer paints the same header in a band of its own, and pi builds it from the same
/// `shouldShowStartupHeader`: `true` leaves the band out, `"header"` and `false` keep it, with the
/// onboarding line worded for the details that are (or are not) there.
#[tokio::test]
async fn the_inline_header_band_follows_each_quiet_startup_value() {
    let dir = tempfile::tempdir().unwrap();
    for (stored, header, resources) in [
        ("false", true, true),
        ("true", false, false),
        ("\"header\"", true, false),
    ] {
        let session = session_with(dir.path(), stored).await;
        let mut app = new_app();
        app.push_session_loaded_resources(&session);
        app.draw().unwrap();
        let text = crate::tests::harness::buf_text(&app);
        assert_eq!(
            text.contains("interrupt"),
            header,
            "quietStartup {stored}: {text}"
        );
        assert_eq!(
            text.contains("show full startup help and loaded resources."),
            header && resources,
            "quietStartup {stored}: {text}"
        );
        assert_eq!(
            text.contains("show full startup help."),
            header && !resources,
            "quietStartup {stored}: {text}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// leaving
// ---------------------------------------------------------------------------------------------

/// Back to inline with the built-in startup block in the document: it goes into scrollback with the
/// rest of the conversation, and the inline renderer does not paint its own copy as well.
#[test]
fn switching_to_inline_shows_the_startup_block_once() {
    let mut app = new_app();
    let captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.transcript_mut().push_status("a turn in the excursion");
    app.draw().unwrap();
    assert!(
        app.state().show_startup_hints,
        "the inline band is still armed"
    );

    assert!(app.stop_fullscreen_to_inline());
    let text = captured_text(&captured);
    assert_eq!(
        text.matches("interrupt").count(),
        1,
        "the block is in the repaint:\n{text}"
    );

    app.draw().unwrap();
    let inline: String = app
        .terminal()
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        !inline.contains("interrupt"),
        "and the inline renderer does not paint it again: {inline:?}"
    );
}

/// An extension's header stays the inline renderer's band, so it is not also written into
/// scrollback.
#[test]
fn switching_to_inline_leaves_an_extension_header_to_the_inline_band() {
    let mut app = new_app();
    app.state_mut().extension_header = Some("EXTENSION HEADER".to_string());
    let captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.transcript_mut().push_status("a turn in the excursion");
    app.draw().unwrap();

    assert!(app.stop_fullscreen_to_inline());
    let text = captured_text(&captured);
    assert!(
        text.contains("a turn in the excursion"),
        "the conversation still crosses over:\n{text}"
    );
    assert!(
        !text.contains("EXTENSION HEADER"),
        "the header is not written twice:\n{text}"
    );

    app.draw().unwrap();
    let inline: String = app
        .terminal()
        .backend()
        .buffer()
        .content
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert_eq!(inline.matches("EXTENSION HEADER").count(), 1, "{inline:?}");
}

/// Quitting has no inline renderer to hand the header to: the transcript the user keeps starts
/// with it, as pi's does.
#[test]
fn quitting_writes_the_header_into_the_transcript() {
    let mut app = new_app();
    app.state_mut().extension_header = Some("EXTENSION HEADER".to_string());
    let captured = app.enter_fullscreen_captured().expect("renderer builds");
    app.transcript_mut().push_status("a turn in the excursion");
    app.draw().unwrap();

    app.stop_fullscreen(false);
    let text = captured_text(&captured);
    let header_at = text
        .find("EXTENSION HEADER")
        .expect("the header is written");
    let turn_at = text
        .find("a turn in the excursion")
        .expect("so is the turn");
    assert!(header_at < turn_at, "header first:\n{text}");
}

/// The repaint keeps links clickable: committed rows and the in-flight turn's rows both come back
/// out as OSC-8 pairs around the same text the screen showed.
#[test]
fn the_exit_repaint_carries_hyperlinks_into_scrollback() {
    let (mut app, captured) = bare_fullscreen();
    app.transcript_mut().set_hyperlinks(true);
    app.transcript_mut()
        .set_cwd(Some(PathBuf::from("/tmp/aug-osc")));
    let t = app.transcript_mut();
    t.push_tool_start("read", json!({ "path": "a.rs" }));
    t.push_tool_end("read", false, Some(read_result("body")));
    t.commit_tools();
    t.commit_assistant(Some("see [docs](https://x.io/committed) here".into()));
    t.push_assistant_delta("and https://x.io/live");
    app.draw().unwrap();

    app.stop_fullscreen(false);
    let text = captured_text(&captured);
    let (_, after_leave) = text.split_once("\u{1b}[?1049l").expect("leaves the screen");
    for (url, shown) in [
        ("file:///tmp/aug-osc/a.rs", "a.rs"),
        ("https://x.io/committed", "docs"),
        ("https://x.io/live", "https://x.io/live"),
    ] {
        assert!(
            after_leave.contains(&open(url)),
            "{url} lost its link in the repaint:\n{after_leave:?}"
        );
        assert!(after_leave.contains(shown), "{shown}");
    }
    assert!(after_leave.contains(CLOSE));
    // A link opened in the repaint is closed before the row ends.
    assert_eq!(
        after_leave.matches("\u{1b}]8;;\u{7}").count(),
        after_leave.matches("\u{1b}]8;;http").count()
            + after_leave.matches("\u{1b}]8;;file").count(),
        "every open has its close"
    );
}
