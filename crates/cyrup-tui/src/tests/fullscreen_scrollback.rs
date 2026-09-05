//! The excursion's history must survive leaving the alternate screen — `App`-level, because the
//! failure it guards is an interaction between two units that are each correct alone.
//!
//! `draw_fullscreen` drains the transcript's pending queue and drops it (`app/draw.rs`): committing
//! through `insert_before` while the alternate screen is up would write into a buffer about to be
//! discarded, so the entries go to the retained document instead. `App::stop_fullscreen` then clears
//! that document one line after `AltScreen::stop`. Between those two facts the document is the ONLY
//! surviving copy of the excursion, and the repaint inside `stop` is the only thing that carries it
//! out — which is why both teardowns pass `preserve_screen: false`.
//!
//! Nothing below `App` can pin this. `src/tests/alt_screen.rs` drives an `AltScreen` directly, so the
//! drain and the clear never meet there, and every one of its 27 cases passed throughout the window
//! in which quitting fullscreen destroyed the session.
//!
//! ## What these assert against
//!
//! The repaint's rows leave through the renderer's escape sink, not through `insert_before` — so the
//! evidence is the captured byte stream, and `App::scrollback_lines` is deliberately NOT the witness
//! here: it stays empty across a fullscreen excursion precisely because the inline commit path is the
//! one `draw_fullscreen` skips. `enter_fullscreen_captured` hands back that sink.
//!
//! Upstream reaches the same end state by a route cyrup cannot take — `switchTuiMode` stops the
//! alternate screen with `preserveScreen: true` and lets the regular renderer re-render the shared
//! chat container (`interactive-mode.ts:779-786` @v0.84.1) — because pi's components move between
//! renderers while cyrup's committed entries have already left the app for the terminal.

//! ## What these actually catch, established by mutation
//!
//! * Suppress the repaint (`term.leave(true, ..)` regardless of the caller) — **both fail**. This is
//!   the historical bug: quitting fullscreen erased the session.
//! * Stop `draw_fullscreen` feeding the retained document (drop its `drain_committed`) — **both
//!   fail**. The repaint then has nothing to write.
//! * Move `stop_fullscreen`'s `clear_document` ahead of `AltScreen::stop` — **both still pass, and
//!   correctly so.** `AltScreen::doc` is the renderer's own rendered copy, taken by `sync_document`
//!   during the frame, not a borrow of the transcript's document; clearing the transcript afterwards
//!   cannot reach it. That ordering is worth keeping for clarity, but it is not load-bearing, and a
//!   guard that failed on it would be asserting something untrue.
//!
//! ## The one call site these do NOT reach
//!
//! `switching_back_to_inline_carries_the_excursion_out` drives a real call site: `install_renderer`'s
//! `Regular` arm passes the `false` itself. The exit path (`app/run.rs`) is inside `App::run`'s event
//! loop, which a unit test cannot enter without a live session and event stream — so its own
//! `stop_fullscreen` CALL is still unreached here. Since CFG-078 that call's argument is no longer a
//! literal but `App::preserve_screen_on_exit`, and the DECISION it makes is covered below by
//! `the_exit_output_setting_decides_whether_the_excursion_reaches_scrollback`; what remains
//! unreached is only that `run.rs` passes that method rather than something else.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use ratatui::backend::TestBackend;

use crate::altscreen::captured_text;
use crate::app::ModeSwitchOptions;
use crate::{App, TuiRenderMode, UiTheme};

fn new_app() -> App<TestBackend> {
    App::new(TestBackend::new(80, 16), UiTheme::dark()).unwrap()
}

/// Quitting from fullscreen puts the excursion on the main screen.
///
/// The `false` at the exit call site (`app/run.rs`) is what makes this true; with `true` the rows are
/// never written and the session ends having erased itself.
#[test]
fn quitting_fullscreen_carries_the_excursion_out() {
    let mut app = new_app();
    app.transcript_mut()
        .push_status("a turn from inside the excursion");

    let captured = app
        .enter_fullscreen_captured()
        .expect("the capture renderer builds");
    app.draw().unwrap();

    // The precondition that makes the repaint load-bearing: the fullscreen frame has taken the
    // entries out of the pending queue, so the inline commit path has nothing left to flush.
    assert!(
        app.transcript_mut().pending().is_empty(),
        "the fullscreen frame is expected to have drained the pending queue"
    );

    app.stop_fullscreen(false);

    let text = captured_text(&captured);
    assert!(
        text.contains("a turn from inside the excursion"),
        "the excursion's history did not survive the teardown:\n{text}"
    );
    let leave_at = text
        .find("\x1b[?1049l")
        .expect("the teardown leaves the alternate screen");
    let row_at = text
        .find("a turn from inside")
        .expect("the row was just asserted present");
    assert!(
        leave_at < row_at,
        "the rows must land on the MAIN screen, after leaving:\n{text}"
    );
}

/// The same contract on the live switch back to inline, which had no coverage at all.
///
/// This path runs through `switch_tui_mode`, not the exit, and needs the repaint just as much: the
/// inline renderer resumes below history it does not hold.
/// CFG-078 — `fullscreenExitOutput` decides whether quitting from fullscreen dumps the excursion
/// into scrollback (`transcript`, pi's default) or restores the screen the terminal had and leaves
/// only the resume hint (`resume-hint`).
///
/// pi does this by conditionally switching back to the regular renderer before stopping, so its
/// `preserveScreen` ends up `false` for `transcript` and `true` for `resume-hint`
/// (`interactive-mode.ts:836-842` @v0.84.4); cyrup's inline renderer cannot re-render history it
/// has already given to the terminal, so the same two outcomes come from feeding
/// `App::preserve_screen_on_exit` to the repaint instead.
///
/// Red before this change: `set_fullscreen_exit_output` / `preserve_screen_on_exit` did not exist,
/// and the exit teardown was a hardcoded `false` — the `resume-hint` half was unreachable.
#[test]
fn the_exit_output_setting_decides_whether_the_excursion_reaches_scrollback() {
    // The default is pi's `"transcript"`, which is the behaviour every existing case here asserts.
    let app = new_app();
    assert!(
        !app.preserve_screen_on_exit(),
        "an unconfigured App exits with pi's default, `transcript`"
    );

    for (output, expect_rows) in [
        (crate::FullscreenExitOutput::Transcript, true),
        (crate::FullscreenExitOutput::ResumeHint, false),
    ] {
        let mut app = new_app();
        app.set_fullscreen_exit_output(output);
        app.transcript_mut()
            .push_status("a turn from the excursion");

        let captured = app
            .enter_fullscreen_captured()
            .expect("the capture renderer builds");
        app.draw().unwrap();
        assert!(
            app.transcript_mut().pending().is_empty(),
            "the fullscreen frame is expected to have drained the pending queue"
        );

        // Exactly what `App::run`'s exit tail does.
        app.stop_fullscreen(app.preserve_screen_on_exit());

        let text = captured_text(&captured);
        assert!(
            text.contains("\x1b[?1049l"),
            "either setting still leaves the alternate screen:\n{text}"
        );
        assert_eq!(
            text.contains("a turn from the excursion"),
            expect_rows,
            "{output:?} put the wrong thing on the main screen:\n{text}"
        );
    }
}

#[test]
fn switching_back_to_inline_carries_the_excursion_out() {
    let mut app = new_app();
    app.transcript_mut()
        .push_status("a turn the switch must not eat");

    let captured = app
        .enter_fullscreen_captured()
        .expect("the capture renderer builds");
    app.draw().unwrap();
    assert_eq!(
        app.render_mode(),
        TuiRenderMode::Fullscreen,
        "the excursion is live"
    );

    let outcome = app.switch_tui_mode(TuiRenderMode::Regular, ModeSwitchOptions::default());
    assert!(
        outcome.accepted(),
        "the switch back to inline was refused: {outcome:?}"
    );
    assert_eq!(
        app.render_mode(),
        TuiRenderMode::Regular,
        "the inline renderer is live again"
    );

    let text = captured_text(&captured);
    assert!(
        text.contains("a turn the switch must not eat"),
        "the excursion's history did not survive the switch:\n{text}"
    );
}
