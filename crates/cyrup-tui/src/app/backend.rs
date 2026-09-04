use super::*;

/// A backend that can rebuild a fresh handle over the **same** underlying terminal, so the inline
/// viewport can be re-sized to the live region's content height (ratatui's `Viewport::Inline` height
/// is fixed at construction; ADR-0001 commitment #1 / audit #1 require a content-sized region). The
/// rebuilt backend must preserve the cursor anchor used to place the inline viewport: `TestBackend`
/// copies its grid cursor; [`InlineBackend`] copies its tracked anchor across the re-wrap (TUI-093 —
/// the real terminal is never asked where the cursor is; see [`InlineBackend::get_cursor_position`]).
pub trait RebuildBackend: Backend + Sized {
    /// A fresh backend over the same terminal, preserving the inline-viewport cursor anchor.
    #[must_use]
    fn rebuild(&self) -> Self;

    /// Prepare the **current** backend for a reconstruction at `new_height` (called by
    /// [`App::draw`] → `resize_viewport` immediately before [`rebuild`](Self::rebuild)): erase the
    /// current inline region and re-anchor the cursor so the rebuilt viewport leaves **no residual
    /// chrome** on a real terminal — the fix for the inline-viewport STACKING bug. See
    /// [`reanchor_inline_region`] for the geometry.
    ///
    /// The default is a **no-op**, which is correct for fresh-grid backends whose `rebuild` starts
    /// from a blank buffer and re-anchors itself (e.g. `TestBackend`): they can never stack, so there
    /// is nothing to erase. Only a persistent real-screen backend ([`InlineBackend`]) needs it.
    fn reanchor_inline(&mut self, _term_height: u16, _old_height: u16, _new_height: u16) {}
}

/// Erase the current inline region and re-anchor the cursor so that reconstructing the inline
/// viewport at `new_height` leaves **no residual chrome** (the inline-viewport STACKING fix).
///
/// ratatui's `Viewport::Inline(height)` is immutable after construction and `Terminal::resize` cannot
/// change it (it treats its argument as the *terminal* size), so a content-sized live region must be
/// reconstructed whenever its height changes. Reconstruction reserves the new rows by calling
/// `Backend::append_lines`, which — at the bottom of a real terminal — SCROLLS: without this step the
/// prior frame's hint bar / editor rules / footer scroll up into native scrollback and stay visible,
/// so a streaming turn that grows over several frames leaves a stack of duplicated chrome.
///
/// Emitting `MoveTo(top-of-current-region)` + `Clear(FromCursorDown)` first means that scroll carries
/// **blanks**, not chrome. The returned row is where the cursor is left; the reconstruction's
/// `compute_inline_size` reads it via `get_cursor_position`. The anchor is bottom-aligned with the
/// **minimal** scroll — `term_height - min(old, new)`: on growth this is the old region's top (append
/// scrolls exactly the delta); on shrink it sits lower so the shorter region stays pinned to the
/// bottom with no scroll. Both cases leave the region bottom-anchored, for growth **and** shrink.
pub fn reanchor_inline_region<W: io::Write>(
    w: &mut W,
    term_height: u16,
    old_height: u16,
    new_height: u16,
) -> u16 {
    let term_h = term_height.max(1);
    let last_row = term_h.saturating_sub(1);
    let anchor_y = term_h
        .saturating_sub(old_height.min(new_height))
        .min(last_row);
    if old_height > 0 {
        // Erase the whole current inline region (its top row down to the bottom of the screen).
        let erase_top = term_h.saturating_sub(old_height).min(last_row);
        let _ = queue!(w, MoveTo(0, erase_top), Clear(ClearType::FromCursorDown));
    }
    let _ = queue!(w, MoveTo(0, anchor_y));
    let _ = w.flush();
    anchor_y
}

impl RebuildBackend for ratatui::backend::TestBackend {
    fn rebuild(&self) -> Self {
        let area = self.buffer().area;
        let mut next = ratatui::backend::TestBackend::new(area.width, area.height);
        // Anchor the inline viewport at the **bottom** of the screen, exactly as a real terminal does
        // at launch (the cursor sits after the shell prompt). This makes `insert_before` scroll
        // committed history up off the top into native scrollback (out of the visible buffer) instead
        // of leaving it on-screen above a top-anchored viewport (ADR-0001; audit #1).
        let bottom = ratatui::layout::Position {
            x: 0,
            y: area.height.saturating_sub(1),
        };
        let _ = Backend::set_cursor_position(&mut next, bottom);
        next
    }
}

/// The production inline-viewport backend: a `CrosstermBackend<W>` whose cursor position cyrup
/// ANSWERS instead of ASKING (TUI-093).
///
/// `CrosstermBackend::get_cursor_position` is a raw DSR round-trip
/// (`ratatui-crossterm-0.1.2/src/lib.rs:302-306` → `crossterm::cursor::position()`), and crossterm
/// documents that it "will block and possibly time out while `event::read`/`event::poll` are being
/// called" (`crossterm-0.29.0/src/cursor/sys/unix.rs:13-15`) — which `crossterm_input_stream`'s
/// reader thread does permanently. ratatui reaches that query from `insert_before` (via
/// `Terminal::clear`, `inline.rs:212`), from `autoresize` inside every `Terminal::draw`
/// (`resize.rs:64-73`) and from `Terminal::with_options(Viewport::Inline)` (`init.rs:122-130`) — so
/// a commit flush, a window resize and a live-region height change each risked a 2s stall and an
/// `Err` that `App::draw` propagated as a FATAL `TuiError::Backend`.
///
/// The query is unnecessary: `reanchor_inline_region` MOVES the cursor to the anchor and returns the
/// row, one statement before ratatui asks. This backend keeps that row and reports it. Modelled on
/// `tests/inline_stacking.rs`'s `CaptureBackend`, which has driven the whole app this way since the
/// stacking fix.
pub struct InlineBackend<W: io::Write> {
    inner: CrosstermBackend<W>,
    /// Where cyrup last PUT the cursor. Authoritative for `get_cursor_position`.
    anchor: ratatui::layout::Position,
}

impl<W: io::Write> InlineBackend<W> {
    /// Wrap `writer`, seeding the anchor with the row the caller has established (see
    /// `App::into_stdout`'s one bounded probe).
    pub fn with_anchor(writer: W, anchor: ratatui::layout::Position) -> Self {
        Self {
            inner: CrosstermBackend::new(writer),
            anchor,
        }
    }

    /// The tracked anchor (inspection / re-seed after a probe).
    pub fn anchor(&self) -> ratatui::layout::Position {
        self.anchor
    }

    /// Re-seed the anchor (`App::into_stdout` after clamping the probed row to the live height).
    pub(crate) fn set_anchor(&mut self, anchor: ratatui::layout::Position) {
        self.anchor = anchor;
    }
}

impl<W: io::Write> Backend for InlineBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        // Deliberately does NOT touch `anchor`: a painted frame leaves the hardware cursor at the
        // last cell, and the only reader of the anchor after a draw is `Terminal::clear`'s
        // save/restore (`buffers.rs:147-152`) — whose two call sites (`App::suspend`,
        // `edit_in_external_editor`) redraw the full viewport immediately afterwards. Modelling
        // cell-wrap here would be a worse guess than the anchor cyrup actually set.
        self.inner.draw(content)
    }

    /// `\n` × n. In raw mode `cfmakeraw` clears `OPOST`, so `ONLCR` is off and LF moves DOWN
    /// without a carriage return — the column is unchanged. At the bottom row each LF scrolls, so
    /// the row saturates at the last line (`compute_inline_size`'s own `missing_lines` math,
    /// `inline.rs:405-410`, and `CaptureBackend::append_lines`).
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        let last = Backend::size(&self.inner)
            .map(|s| s.height.saturating_sub(1))
            .unwrap_or(u16::MAX);
        self.anchor.y = self.anchor.y.saturating_add(n).min(last);
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    /// The anchor cyrup set — and, so the answer is TRUE rather than merely remembered, the move
    /// that makes it so. Idempotent: `reanchor_inline_region` has just emitted the same `MoveTo`,
    /// and re-emitting it is what keeps `compute_inline_size`'s following `append_lines` reserving
    /// rows from the anchor even if a `draw` left the hardware cursor mid-screen. NEVER a DSR: this
    /// is the whole of TUI-093.
    fn get_cursor_position(&mut self) -> io::Result<ratatui::layout::Position> {
        let anchor = self.anchor;
        Backend::set_cursor_position(&mut self.inner, anchor)?;
        Ok(anchor)
    }

    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        position: P,
    ) -> io::Result<()> {
        let p = position.into();
        self.anchor = p;
        self.inner.set_cursor_position(p)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<ratatui::layout::Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<ratatui::backend::WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }

    // TUI-092-F5 readiness: `Terminal::insert_before` switches to these the moment
    // `ratatui/scrolling-regions` is on, and ratatui documents the cursor as UNDEFINED afterwards —
    // so the anchor is re-seeded by the next `reanchor_inline`, never inferred from a scroll.
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, amount)
    }

    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, amount)
    }
}

impl RebuildBackend for InlineBackend<Stdout> {
    /// A fresh wrapper over the same stdout, CARRYING the anchor across the re-wrap — the step that
    /// makes the rebuilt `Terminal::with_options` reserve rows from the row `reanchor_inline` just
    /// moved to (`CaptureBackend::rebuild` does the same, `inline_stacking.rs:131-138`).
    fn rebuild(&self) -> Self {
        Self {
            inner: CrosstermBackend::new(io::stdout()),
            anchor: self.anchor,
        }
    }

    fn reanchor_inline(&mut self, term_height: u16, old_height: u16, new_height: u16) {
        // `CrosstermBackend<W>: io::Write` — emit the erase + re-anchor straight to stdout. The
        // returned row is no longer discarded (TUI-093): it IS the answer `get_cursor_position`
        // will report for `compute_inline_size` two statements from now.
        let row = reanchor_inline_region(&mut self.inner, term_height, old_height, new_height);
        self.anchor = ratatui::layout::Position::new(0, row);
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic
    )]
    use std::sync::{Arc, Mutex};

    use ratatui::layout::Position;

    use super::*;

    /// A `Write` over a shared byte buffer, so the bytes [`InlineBackend`]/[`reanchor_inline_region`]
    /// write can be inspected after the call — `CrosstermBackend`'s own writer field is private and
    /// its `writer()` accessor is gated behind the `backend-writer` feature, which this crate does
    /// not enable. Mirrors `tests/inline_stacking.rs`'s `SharedWriter`; kept local here rather than
    /// shared across the two test trees (this file's tests are unit tests colocated with the code,
    /// that file's are an App-level integration suite under `src/tests/`).
    #[derive(Clone)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl SharedBuf {
        fn new() -> Self {
            Self(Arc::new(Mutex::new(Vec::new())))
        }

        fn bytes(&self) -> Vec<u8> {
            self.0.lock().unwrap().clone()
        }
    }

    impl io::Write for SharedBuf {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    /// `crossterm::cursor::MoveTo(x, y)`'s ANSI encoding — `CSI {y+1};{x+1}H` (1-based, row before
    /// column; `crossterm-0.29.0/src/cursor.rs:60-65`).
    fn move_to(x: u16, y: u16) -> String {
        format!("\x1b[{};{}H", y + 1, x + 1)
    }

    // ---------------------------------------------------------- reanchor_inline_region ----

    #[test]
    fn reanchor_first_call_just_anchors_at_the_bottom_with_no_erase() {
        let mut buf = Vec::new();
        let row = reanchor_inline_region(&mut buf, 24, 0, 6);
        assert_eq!(row, 23, "the bottom row of a 24-row terminal is index 23");
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            move_to(0, 23),
            "old_height 0 (first construction) has nothing to erase"
        );
    }

    #[test]
    fn reanchor_reconstruction_erases_the_old_region_before_reanchoring() {
        let mut buf = Vec::new();
        let row = reanchor_inline_region(&mut buf, 24, 6, 3);
        let expected = format!("{}\x1b[J{}", move_to(0, 18), move_to(0, row));
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            expected,
            "erase (MoveTo the old region's top + Clear-from-cursor-down) must precede the re-anchor"
        );
    }

    #[test]
    fn reanchor_growth_anchors_at_the_old_regions_top() {
        // Growing 3 -> 6 rows in a 24-row terminal: `append_lines` scrolls exactly the delta, so the
        // anchor sits at the OLD region's top (`term_height - old_height`).
        let mut buf = Vec::new();
        let row = reanchor_inline_region(&mut buf, 24, 3, 6);
        assert_eq!(row, 21, "24 - min(3, 6) = 21, the old region's top row");
    }

    #[test]
    fn reanchor_shrink_sits_lower_and_stays_bottom_anchored() {
        // Shrinking 6 -> 3 rows: the anchor sits at `term_height - new_height`, one row LOWER than
        // the erase's own top, so the shorter region stays pinned to the bottom with no scroll.
        let mut buf = Vec::new();
        let row = reanchor_inline_region(&mut buf, 24, 6, 3);
        assert_eq!(row, 21, "24 - min(6, 3) = 21");
        assert!(
            String::from_utf8(buf).unwrap().starts_with(&move_to(0, 18)),
            "the erase starts at the OLD (taller) region's top, row 18"
        );
    }

    #[test]
    fn reanchor_a_zero_height_terminal_is_clamped_to_one_row_without_underflow() {
        let mut buf = Vec::new();
        let row = reanchor_inline_region(&mut buf, 0, 0, 5);
        assert_eq!(
            row, 0,
            "term_height.max(1) makes row index 0 the only row, not a saturating_sub underflow"
        );
    }

    // -------------------------------------------------------------------- InlineBackend ----

    #[test]
    fn with_anchor_seeds_the_given_position() {
        let backend = InlineBackend::with_anchor(SharedBuf::new(), Position::new(4, 9));
        assert_eq!(backend.anchor(), Position::new(4, 9));
    }

    /// THE regression test (TUI-093): `get_cursor_position` must ANSWER, never ASK. The old
    /// `CrosstermBackend::get_cursor_position` issued a DSR, and crossterm's own hard-coded timeout
    /// for a reply that never arrives is 2000ms (`crossterm-0.29.0/src/cursor/sys/unix.rs`); a
    /// backend that instead answers from its tracked anchor returns in microseconds. 250ms is a
    /// generous ceiling that still stays two orders of magnitude under that timeout, so this
    /// genuinely distinguishes "answers" from "asks" without being a flaky wall-clock assertion —
    /// nothing on this path performs I/O whose latency could legitimately approach it.
    #[test]
    fn get_cursor_position_returns_the_tracked_anchor_without_querying_the_terminal() {
        let anchor = Position::new(4, 9);
        let mut backend = InlineBackend::with_anchor(SharedBuf::new(), anchor);
        let started = std::time::Instant::now();
        let got = backend.get_cursor_position().unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(250),
            "get_cursor_position took {:?} — a backend that ANSWERS never approaches crossterm's \
             2000ms DSR timeout",
            started.elapsed()
        );
        assert_eq!(got, anchor);
    }

    #[test]
    fn get_cursor_position_asserts_the_anchor_with_a_moveto() {
        let buf = SharedBuf::new();
        let mut backend = InlineBackend::with_anchor(buf.clone(), Position::new(4, 9));
        backend.get_cursor_position().unwrap();
        assert_eq!(
            String::from_utf8(buf.bytes()).unwrap(),
            move_to(4, 9),
            "the answer is made TRUE, not merely remembered — idempotent with whatever \
             `reanchor_inline_region` just emitted"
        );
    }

    #[test]
    fn set_cursor_position_updates_the_tracked_anchor() {
        let mut backend = InlineBackend::with_anchor(SharedBuf::new(), Position::ORIGIN);
        backend.set_cursor_position(Position::new(7, 2)).unwrap();
        assert_eq!(backend.anchor(), Position::new(7, 2));
        assert_eq!(backend.get_cursor_position().unwrap(), Position::new(7, 2));
    }

    #[test]
    fn set_anchor_reseeds_without_writing_anything() {
        let buf = SharedBuf::new();
        let mut backend = InlineBackend::with_anchor(buf.clone(), Position::ORIGIN);
        backend.set_anchor(Position::new(1, 5));
        assert_eq!(backend.anchor(), Position::new(1, 5));
        assert!(
            buf.bytes().is_empty(),
            "re-seeding the anchor (the boot probe) writes nothing"
        );
    }

    #[test]
    fn append_lines_advances_the_anchor_row_by_n() {
        // No real controlling terminal under `cargo test`, so `Backend::size` on the wrapped
        // `CrosstermBackend` (which queries the REAL terminal regardless of `W`) errors and
        // `append_lines` falls back to `unwrap_or(u16::MAX)` — i.e. no clamp is exercised here. The
        // CLAMPED case (advancing past a real terminal's height) is covered by
        // `tests/inline_stacking.rs`'s `CaptureBackend`, whose fixed, test-controlled `size()` is
        // exactly what `InlineBackend` cannot fake without wrapping `CrosstermBackend::size` too.
        let mut backend = InlineBackend::with_anchor(SharedBuf::new(), Position::ORIGIN);
        backend.append_lines(3).unwrap();
        assert_eq!(backend.anchor().y, 3);
        backend.append_lines(4).unwrap();
        assert_eq!(
            backend.anchor().y,
            7,
            "append_lines accumulates across calls"
        );
    }

    #[test]
    fn append_lines_never_panics_near_the_top_of_u16() {
        // Whether `Backend::size` on the wrapped `CrosstermBackend` succeeds (clamping to a real
        // terminal height, as it does in this environment — confirmed by
        // `append_lines_advances_the_anchor_row_by_n` never approaching a clamp at small values) or
        // fails elsewhere (falling back to `unwrap_or(u16::MAX)`) is environment-dependent, so no
        // exact resulting value is asserted here (a `y <= u16::MAX` check would be tautological for
        // a `u16` — clippy's `absurd_extreme_comparisons` correctly rejects that). What this proves
        // is the thing that actually matters: starting near `u16::MAX` and adding more than fits
        // does not PANIC — `saturating_add`/`.min` are the guard; a bare `+` would overflow-panic in
        // a debug build on exactly this input, and reaching the line below at all is the pass.
        let mut backend =
            InlineBackend::with_anchor(SharedBuf::new(), Position::new(0, u16::MAX - 1));
        backend.append_lines(10).unwrap();
    }

    #[test]
    fn rebuild_carries_the_anchor_across_the_rewrap() {
        // `InlineBackend<Stdout>` specifically — `RebuildBackend` is only implemented for that
        // concrete type. This writes nothing (`CrosstermBackend::new` just wraps the handle), so
        // it is safe to run under `cargo test`'s captured stdout.
        let backend = InlineBackend::with_anchor(io::stdout(), Position::new(2, 9));
        let rebuilt = backend.rebuild();
        assert_eq!(
            rebuilt.anchor(),
            Position::new(2, 9),
            "TUI-093: the old `impl RebuildBackend for CrosstermBackend<Stdout>` had no memory here \
             — `rebuild()` just re-wrapped stdout, trusting the real terminal cursor to still be \
             authoritative. InlineBackend must carry the tracked anchor across the re-wrap instead."
        );
    }
}
