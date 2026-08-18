//! TUI-093 DoD-4 — a failed viewport reconstruction must not end the session.
//!
//! Before the fix, [`App::draw`] propagated `resize_viewport`'s error with a bare `?`, which
//! unwound roughly forty `draw_synchronized()?` call sites out of `App::run` and ended the whole
//! process (`main.rs`'s `anyhow!("tui: {e}")` → `eprintln!("cyrup: {err:#}")`) — this is the exact
//! shape of the reported crash, `cyrup: tui: terminal backend error: The cursor position could not
//! be read within a normal duration`. The real failure was a DSR racing the input-reader thread
//! (fixed by [`crate::InlineBackend`] never issuing one); this suite exercises the OTHER half of
//! the fix — that whatever still can fail inside `resize_viewport` (a transient write error) is now
//! recoverable — without needing a real terminal or a real race, by injecting a one-shot failure at
//! the exact seam the DSR used to fail at: `Backend::get_cursor_position`, called from
//! `compute_inline_size` inside `Terminal::with_options` (`ratatui-core/src/terminal/inline.rs:396`,
//! `init.rs:122-130`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::cell::Cell;
use std::convert::Infallible;
use std::io;

use ratatui::backend::{Backend, ClearType, TestBackend, WindowSize};
use ratatui::buffer::Cell as BufCell;
use ratatui::layout::{Position, Size};

use crate::{App, RebuildBackend, UiTheme};

/// Wraps a `TestBackend`, delegating every [`Backend`] method to it except
/// [`get_cursor_position`](Backend::get_cursor_position), which can be armed to fail exactly once —
/// standing in for the real DSR timeout TUI-093 fixes, at the one seam `resize_viewport` still
/// funnels every reconstruction failure through.
///
/// The arm is a `Cell<bool>` and [`rebuild`](RebuildBackend::rebuild) **consumes** it (via the
/// `Cell`'s interior mutability through `&self`) rather than merely copying it onto the rebuilt
/// instance: `resize_viewport` calls `rebuild()` on the CURRENT backend to obtain the one whose
/// `get_cursor_position` will actually be queried, and — critically, on a FAILED reconstruction —
/// `self.terminal` is never replaced, so the current backend (the one `rebuild()` was called on)
/// is what the NEXT frame calls `rebuild()` on again. Consuming the arm at that call is what makes
/// the injected failure genuinely one-shot; a naive `Cell::new(self.fail_next_cursor_query.get())`
/// copy would re-arm forever, since the original's flag would never be cleared on a failure path.
struct FlakyBackend {
    inner: TestBackend,
    fail_next_cursor_query: Cell<bool>,
}

impl FlakyBackend {
    fn new(inner: TestBackend) -> Self {
        Self { inner, fail_next_cursor_query: Cell::new(false) }
    }
}

/// `TestBackend`'s `Backend::Error` is `Infallible` (it can never fail), so every delegated call
/// converts via an exhaustive, unreachable match on the uninhabited error type — total, and not a
/// panic (there is no variant to construct, so this arm can never execute).
fn infallible(e: Infallible) -> io::Error {
    match e {}
}

impl Backend for FlakyBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a BufCell)>,
    {
        self.inner.draw(content).map_err(infallible)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n).map_err(infallible)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor().map_err(infallible)
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor().map_err(infallible)
    }

    /// The injected failure. Consumes the arm (`replace(false)`) so only the query that was
    /// actually armed for fails — matching a real transient DSR timeout, not a permanently dead
    /// backend.
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        if self.fail_next_cursor_query.replace(false) {
            return Err(io::Error::other("simulated resize-time cursor query failure"));
        }
        self.inner.get_cursor_position().map_err(infallible)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position).map_err(infallible)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear().map_err(infallible)
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type).map_err(infallible)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size().map_err(infallible)
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size().map_err(infallible)
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner).map_err(infallible)
    }

    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, amount).map_err(infallible)
    }

    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, amount).map_err(infallible)
    }
}

impl RebuildBackend for FlakyBackend {
    fn rebuild(&self) -> Self {
        // See the struct doc: consuming here (not copying) is what makes the injected failure
        // one-shot across a failed-then-retried `resize_viewport`.
        let armed = self.fail_next_cursor_query.replace(false);
        Self { inner: self.inner.rebuild(), fail_next_cursor_query: Cell::new(armed) }
    }
}

/// Eight short lines — comfortably above the empty-editor baseline and the terminal's own
/// `max(5, floor(term_rows * 0.3))` row cap at `term_rows = 24` (`editor.ts:499-501`), so it
/// reliably changes the live region's desired height regardless of exact wrapping. The editor caps
/// at that same budget, so a SECOND `insert_str` past this point would not force a second,
/// independent resize — both tests below need only ONE growth step.
const GROW_A_LOT: &str = "one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\n";

/// A booted app with one settled frame drawn (the constructor already seeds `viewport_height: 0`
/// specifically so the FIRST `draw()` always reconstructs the viewport; drawing once here gives
/// every test a real, non-zero baseline to compare against instead of that seed value).
fn booted_app(width: u16, height: u16) -> App<FlakyBackend> {
    let mut app = App::new(FlakyBackend::new(TestBackend::new(width, height)), UiTheme::dark())
        .expect("constructing over a plain TestBackend-backed FlakyBackend cannot fail");
    app.draw().expect("the first draw, unarmed, must succeed");
    app
}

/// Test-setup sanity check: proves `GROW_A_LOT` is a REAL resize trigger on a healthy backend, so
/// the "unchanged" assertion in [`a_viewport_resize_failure_does_not_end_the_session`] is not
/// vacuously true because the content growth never mattered in the first place.
#[test]
fn growing_the_editor_changes_the_desired_viewport_height() {
    let mut app = booted_app(60, 24);
    let before = app.viewport_height();
    app.editor_mut().insert_str(GROW_A_LOT);
    app.draw().unwrap();
    assert_ne!(
        app.viewport_height(),
        before,
        "test setup: this much editor growth must change the desired viewport height"
    );
}

/// TUI-093 DoD-4, core claim: `App::draw` returns `Ok(())` even when the viewport reconstruction it
/// attempts fails, `viewport_height` is left at its previous (successfully-established) value
/// rather than being advanced to the height that failed to materialise, and the failure is reported
/// to the user as a transcript status line rather than swallowed silently.
#[test]
fn a_viewport_resize_failure_does_not_end_the_session() {
    let mut app = booted_app(60, 24);
    let start = app.viewport_height();

    app.editor_mut().insert_str(GROW_A_LOT);
    app.terminal().backend().fail_next_cursor_query.set(true);

    // The call that, before TUI-093's non-fatal handling, would have propagated
    // `Err(TuiError::Backend("... could not be read within a normal duration"))` all the way out
    // of `App::run`.
    app.draw().expect("a resize failure must not propagate out of draw()");

    assert_eq!(
        app.viewport_height(),
        start,
        "a failed resize must leave viewport_height at its previous value, so a later frame can \
         retry the SAME reconstruction instead of believing a taller viewport it never built"
    );
    assert!(
        app.scrollback_text().contains("viewport resize failed"),
        "a resize failure should be visible to the user as a transcript status line, not silently \
         swallowed; scrollback was: {:?}",
        app.scrollback_text()
    );
}

/// The failure is one-shot: with nothing else changed, the very next frame retries the identical
/// reconstruction and this time succeeds, catching `viewport_height` up to the height the failed
/// frame wanted.
#[test]
fn the_frame_after_a_failed_resize_retries_and_succeeds() {
    let mut app = booted_app(60, 24);
    let start = app.viewport_height();

    app.editor_mut().insert_str(GROW_A_LOT);
    app.terminal().backend().fail_next_cursor_query.set(true);
    app.draw().unwrap();
    assert_eq!(app.viewport_height(), start, "the failed attempt (re-asserted from the test above)");

    app.draw().unwrap();
    assert_ne!(
        app.viewport_height(),
        start,
        "the frame after a failed resize should retry and succeed once the transient failure clears"
    );
}
