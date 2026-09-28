//! **TUI-097** — `/copy` and `app.message.copy` are not the same call.
//!
//! Upstream `handleCopyCommand` takes an options bag (`interactive-mode.ts:6134-6136` @v0.85.1) and
//! its selection leg is a FOUR-part conjunction (`:6137-6145`):
//!
//! ```ts
//! if (options.preferSelection && this.ui instanceof TuiAltScreen &&
//!     !this.ui.getCopyOnSelect() && this.ui.hasActiveSelection()) {
//!   await this.ui.copyActiveSelectionToClipboard();
//!   return;
//! }
//! ```
//!
//! Only the keybinding passes `preferSelection` (`:2894-2897`); `/copy` passes no options at all
//! (`:3007-3010`). cyrup carried a unit `AppCommand::Copy` whose guard was the last clause alone, so
//! BOTH entry points preferred a live selection — a behaviour no upstream tag has (at the ported
//! baseline v0.83.0 `handleCopyCommand()` took no options and both always copied the last assistant
//! message).
//!
//! The session here deliberately has no assistant message, so the last-message leg answers pi's
//! `getLastAssistantText()`-empty path and the two legs are distinguishable without a clipboard: a
//! headless CI host cannot write one, and both legs would otherwise report the same failure.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::sync::Arc;

use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind as Kind};

use crate::component::InputEvent;
use crate::{App, AppCommand, UiTheme};
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session_svc::{AgentSession, SessionBuilder, SessionConfig};
use tempfile::TempDir;

/// An offline session that has never produced an assistant message, so `last_assistant_text()` is
/// `None` — pi's `if (!text) { this.showError("No agent messages to copy yet."); return; }`
/// (`interactive-mode.ts:6147-6151`).
async fn empty_session(tmp: &TempDir) -> Arc<AgentSession> {
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    let provider: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    Arc::new(
        SessionBuilder::new(provider, cfg)
            .cli_settings(cyrup_session_svc::Settings::new())
            .build()
            .await
            .unwrap(),
    )
}

fn mouse(kind: Kind, column: u16, row: u16) -> InputEvent {
    InputEvent::Mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    })
}

/// An app in the alternate screen with a LIVE (un-released) selection over its document — pi's
/// `hasActiveSelection()`. The press/drag pair is left open on purpose: a release would hand the
/// text out through `PointerOutcome::Copy` and end the selection.
fn app_with_a_live_selection(copy_on_select: bool) -> App<TestBackend> {
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    app.set_fullscreen_copy_on_select(copy_on_select);
    app.transcript_mut().push_status("selectable text");
    let _captured = app
        .enter_fullscreen_captured()
        .expect("the capture renderer builds");
    app.draw().unwrap();
    for row in 0..10 {
        app.handle_input(&mouse(Kind::Down(MouseButton::Left), 0, row));
        app.handle_input(&mouse(Kind::Drag(MouseButton::Left), 30, row));
        if app.has_active_selection_for_test() {
            return app;
        }
    }
    panic!("no document row yielded a selection");
}

/// Every committed transcript entry the app has flushed, as text.
///
/// Neither renderer keeps committed entries in the live viewport: the inline one hands them to the
/// terminal through `insert_before` (they land in the `scrollback_lines` accumulator) and the
/// alternate screen retains them in `TranscriptView::document()` (ADR-0005 §B-1). Both are read
/// here so one helper answers for either mode.
fn flushed_text(app: &mut App<TestBackend>) -> String {
    app.draw().unwrap();
    let inline: String = app
        .scrollback_lines()
        .iter()
        .map(|l| -> String { l.spans.iter().map(|s| s.content.as_ref()).collect() })
        .collect::<Vec<_>>()
        .join("\n");
    // The retained document is a `Vec<Entry>`; its `Debug` rendering carries each entry's text,
    // which is all this needs to tell pi's two legs apart.
    let retained = format!("{:?}", app.transcript_mut().document());
    format!("{inline}\n{retained}")
}

/// Whether the last-assistant-message leg ran, read off the line it pushes when there is none —
/// pi's `if (!text) { showError("No agent messages to copy yet."); return; }` (`:6147-6151`).
async fn ran_the_last_message_leg(
    app: &mut App<TestBackend>,
    command: AppCommand,
    session: &Arc<AgentSession>,
) -> bool {
    app.execute_command(command, session, None).await;
    flushed_text(app).contains("no assistant message to copy")
}

/// (1) Copy-on-select OFF + `app.message.copy`: the one combination upstream gives the selection to.
#[tokio::test]
async fn copy_on_select_off_ctrl_x_copies_the_selection() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = app_with_a_live_selection(false);
    assert!(
        app.prefers_active_selection(true),
        "all four of pi's clauses hold (`:6137-6145`)"
    );
    assert!(
        !ran_the_last_message_leg(
            &mut app,
            AppCommand::Copy {
                prefer_selection: true
            },
            &session
        )
        .await,
        "the selection leg must win, so the empty-session line is never pushed"
    );
}

/// (2) THE filed defect: `/copy` passes no options (`:3008`), so `preferSelection` is falsy and the
/// last assistant message wins even with a selection standing.
///
/// FAILS before TUI-097: `AppCommand::Copy` carried no discriminator, so `/copy` took the selection.
#[tokio::test]
async fn copy_on_select_off_slash_copy_copies_the_last_message() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = app_with_a_live_selection(false);
    assert!(
        !app.prefers_active_selection(false),
        "`options.preferSelection` is pi's FIRST clause (`:6138`)"
    );
    assert!(
        ran_the_last_message_leg(
            &mut app,
            AppCommand::Copy {
                prefer_selection: false
            },
            &session
        )
        .await,
        "`/copy` must fall through to `getLastAssistantText()`"
    );
}

/// (3) The second missing clause: with copy-on-select ON the selection is already on the clipboard,
/// so `!this.ui.getCopyOnSelect()` (`:6141`) releases Ctrl+X back to the last assistant message.
///
/// FAILS before TUI-097: the guard never consulted `fullscreen_copy_on_select` at all.
#[tokio::test]
async fn copy_on_select_on_ctrl_x_copies_the_last_message() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = app_with_a_live_selection(true);
    assert!(
        !app.prefers_active_selection(true),
        "`!getCopyOnSelect()` fails, so the conjunction fails"
    );
    assert!(
        ran_the_last_message_leg(
            &mut app,
            AppCommand::Copy {
                prefer_selection: true
            },
            &session
        )
        .await,
        "Ctrl+X falls through when copy-on-select already took the selection"
    );
}

/// (4) `this.ui instanceof TuiAltScreen` (`:6139`): with no alternate screen there is no selection
/// to prefer, whatever the flag says.
#[tokio::test]
async fn no_altscreen_ctrl_x_copies_the_last_message() {
    let tmp = TempDir::new().unwrap();
    let session = empty_session(&tmp).await;
    let mut app = App::new(TestBackend::new(40, 10), UiTheme::dark()).unwrap();
    for on in [false, true] {
        app.set_fullscreen_copy_on_select(on);
        assert!(
            !app.prefers_active_selection(true),
            "no alternate screen, so no `hasActiveSelection()` (copy_on_select={on})"
        );
    }
    app.execute_command(
        AppCommand::Copy {
            prefer_selection: true,
        },
        &session,
        None,
    )
    .await;
    app.draw().unwrap();
    let t = flushed_text(&mut app);
    assert!(
        t.contains("no assistant message to copy"),
        "the inline path takes the last-message leg unconditionally: [{t}]"
    );
}

/// **TUI-102**, the fullscreen half. Upstream's `copySelection` returns the THROWN message
/// (`tui-renderer.ts:37-44`) and `copyTextToClipboard` flashes it for `COPY_ERROR_FLASH_DURATION_MS`
/// (`tui-alt-screen.ts:80`, `:1459-1462`):
///
/// ```ts
/// this.flash(
///   ok ? "Copied!" : typeof result === "string" ? result : "Copy failed",
///   ok ? undefined : COPY_ERROR_FLASH_DURATION_MS,
/// );
/// ```
///
/// cyrup's `AppAction::CopySelection` arm mapped a `bool` to the constant `"Copy failed"` and passed
/// `None`, so a fullscreen copy failure flashed a message that named nothing for the DEFAULT one
/// second (`altscreen/flash.rs:41`) — long enough to notice and not to read.
///
/// The decision is asserted through `copy_flash_for` rather than by driving the arm, because a CI
/// host cannot make a real `copy_to_clipboard` produce either outcome on demand — the same reason
/// `clipboard_write_plan` is a pure function over a parameterised platform.
///
/// **Red without the change:** `copy_flash_for` and `ClipboardError` do not exist.
#[test]
fn tui102_copy_failure_flashes_pis_message_for_five_seconds() {
    use crate::app::run_action::copy_flash_for;
    use crate::clipboard::ClipboardError;
    use std::time::Duration;

    assert_eq!(
        copy_flash_for(&Err(ClipboardError::X11)),
        (
            "Clipboard unavailable: install `xclip` or `xsel`, or check X11 access",
            Some(Duration::from_millis(5000)),
        ),
        "`tui-alt-screen.ts:1459-1462` flashes the thrown message for COPY_ERROR_FLASH_DURATION_MS",
    );
    assert_eq!(
        copy_flash_for(&Ok(())),
        ("Copied!", None),
        "`ok ? \"Copied!\" : …`, `ok ? undefined : …` — success takes the DEFAULT duration",
    );
    // Every rung of the ladder reaches the flash as its own message, not one generic string.
    for error in [
        ClipboardError::Oversized,
        ClipboardError::Termux,
        ClipboardError::Wayland,
        ClipboardError::X11,
        ClipboardError::Unavailable,
    ] {
        let (message, dwell) = copy_flash_for(&Err(error));
        assert_eq!(message, error.message());
        assert_ne!(message, "Copy failed", "pi's fallback is unreachable here");
        assert_eq!(dwell, Some(Duration::from_millis(5000)));
    }
}

/// **TUI-102, the documentation half.** The row replaced the constant `"Copy failed"` flash with
/// pi's thrown [`crate::clipboard::ClipboardError`] message held for
/// [`crate::altscreen::COPY_ERROR_FLASH_DURATION`] (`tui-alt-screen.ts:1456-1463` @v0.87.1), but
/// three doc comments went on describing the REMOVED behaviour as current, and the doc comment that
/// names the new decision function linked a path that does not exist.
///
/// Both halves are read out of the sources at compile time because neither is reachable from a
/// running test: rustdoc, not the binary, is what consumes them.
///
/// * The link. `clipboard.rs` wrote ``[`crate::app::copy_flash_for`]``; the function is
///   `crate::app::run_action::copy_flash_for` (`app/run_action.rs:416`) and `app/mod.rs` has no
///   re-export, so the workspace's `rustdoc::broken_intra_doc_links` deny turned
///   `cargo doc -p cyrup-tui --no-deps` into `error: unresolved link to
///   `crate::app::copy_flash_for`` / `error: could not document `cyrup-tui``. The crate's docs did
///   not build at all.
/// * The three claims. `app/action.rs`, `altscreen/mod.rs` and `altscreen/selection.rs` each still
///   promised a flash of ``Copied!`` or ``Copy failed`` — the exact string pi reaches only when NO
///   message came back (`ok ? "Copied!" : typeof result === "string" ? result : "Copy failed"`),
///   which cannot happen in cyrup because every `ClipboardError` has one.
///
/// **Red without the change:** the link assertion fails on `clipboard.rs`'s old spelling, and each
/// of the three files fails its own pair of assertions.
#[test]
fn tui102_the_flash_docs_name_pis_message_and_resolve_their_link() {
    // (`path`, source) — the doc sites the row's behaviour change invalidated.
    const SITES: [(&str, &str); 3] = [
        ("app/action.rs", include_str!("../app/action.rs")),
        ("altscreen/mod.rs", include_str!("../altscreen/mod.rs")),
        (
            "altscreen/selection.rs",
            include_str!("../altscreen/selection.rs"),
        ),
    ];

    for (path, src) in SITES {
        for stale in ["`Copied!` or `Copy failed`", "`Copied!` / `Copy failed`"] {
            assert!(
                !src.contains(stale),
                "{path} still documents the flash as {stale}, which TUI-102 removed: a fullscreen \
                 copy failure now flashes the thrown `ClipboardError` message for \
                 `COPY_ERROR_FLASH_DURATION` (`tui-alt-screen.ts:1456-1463`), and pi's bare \
                 \"Copy failed\" fallback is unreachable in cyrup because every error carries a \
                 message"
            );
        }
        assert!(
            src.contains("ClipboardError"),
            "{path} describes the copy flash, so it must name `ClipboardError` — the enum whose \
             message the flash now carries"
        );
        assert!(
            src.contains("COPY_ERROR_FLASH_DURATION"),
            "{path} describes the copy flash, so it must name the 5 s dwell a FAILURE now takes \
             (`COPY_ERROR_FLASH_DURATION_MS = 5000`, `tui-alt-screen.ts:80`)"
        );
    }

    const CLIPBOARD_SRC: &str = include_str!("../clipboard.rs");
    assert!(
        !CLIPBOARD_SRC.contains("[`crate::app::copy_flash_for`]"),
        "clipboard.rs links `crate::app::copy_flash_for`, which does not resolve: the function is \
         `crate::app::run_action::copy_flash_for` and `app/mod.rs` re-exports nothing. The \
         workspace denies `rustdoc::broken_intra_doc_links`, so this one link makes \
         `cargo doc -p cyrup-tui --no-deps` fail with `could not document `cyrup-tui``"
    );
    assert!(
        CLIPBOARD_SRC.contains("[`crate::app::run_action::copy_flash_for`]"),
        "clipboard.rs must still point at the flash-duration decision by its resolvable path — the \
         reason `ClipboardError` is an enum and not a `String` is that `copy_flash_for` never has \
         to re-parse a message"
    );
}
