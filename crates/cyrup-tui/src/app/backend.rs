use super::*;

/// A backend that can rebuild a fresh handle over the **same** underlying terminal, so the inline
/// viewport can be re-sized to the live region's content height (ratatui's `Viewport::Inline` height
/// is fixed at construction; ADR-0001 commitment #1 / audit #1 require a content-sized region). The
/// rebuilt backend must preserve the cursor anchor used to place the inline viewport: `TestBackend`
/// copies its grid cursor; [`InlineBackend`] copies its tracked anchor across the re-wrap (TUI-093 —
/// the real terminal is never asked where the cursor is; see [`InlineBackend::get_cursor_position`]).
pub trait RebuildBackend: Backend + Sized {
    /// A fresh backend over the same terminal, preserving the inline-viewport cursor anchor.
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
    let anchor_y = term_h.saturating_sub(old_height.min(new_height)).min(last_row);
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
        let bottom = ratatui::layout::Position { x: 0, y: area.height.saturating_sub(1) };
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
        Self { inner: CrosstermBackend::new(writer), anchor }
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
        let last =
            Backend::size(&self.inner).map(|s| s.height.saturating_sub(1)).unwrap_or(u16::MAX);
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
        Self { inner: CrosstermBackend::new(io::stdout()), anchor: self.anchor }
    }

    fn reanchor_inline(&mut self, term_height: u16, old_height: u16, new_height: u16) {
        // `CrosstermBackend<W>: io::Write` — emit the erase + re-anchor straight to stdout. The
        // returned row is no longer discarded (TUI-093): it IS the answer `get_cursor_position`
        // will report for `compute_inline_size` two statements from now.
        let row = reanchor_inline_region(&mut self.inner, term_height, old_height, new_height);
        self.anchor = ratatui::layout::Position::new(0, row);
    }
}

