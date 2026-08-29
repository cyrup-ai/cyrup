//! The frame scheduler — pi's `renderRequested` / `renderTimer` / `lastRenderAt` triple
//! (`packages/tui/src/tui.ts:772-822`), ported (PERF-005 §3.1).
//!
//! A request is a FLAG, never a paint. N state changes inside one interval produce one frame, which
//! is the coalescing guarantee upheld ACROSS arms rather than inside each one — and it is what stops
//! a draw that outruns the spinner tick from starving the input arm under `biased;`.

use std::time::{Duration, Instant};

/// pi `TuiBase.MIN_RENDER_INTERVAL_MS` (`tui.ts:343`) — a 62.5 Hz cap.
pub(crate) const MIN_RENDER_INTERVAL: Duration = Duration::from_millis(16);

/// Owned by the run loop; every arm requests, one site paints.
pub(crate) struct FrameScheduler {
    requested: bool,
    /// pi's `requestImmediateRender` (`tui.ts:783-796`): a keystroke must not wait out the
    /// throttle. Set only by the input arm.
    force: bool,
    /// pi's `requestRender(true)` → `resetRenderState()` (`tui.ts:773-777`). Upstream drops its
    /// whole line-diff state so the next frame repaints from scratch; cyrup's equivalent is
    /// `terminal.clear()` before the draw, since ratatui's diff lives in the `Terminal`'s back
    /// buffer.
    ///
    /// **[CYRUP-DELTA] No requester yet, deliberately.** pi pairs this with a
    /// `requestRender(true)` caller, but every cyrup path that needs a from-scratch repaint —
    /// `suspend` after `fg`, and both external-editor returns — is one of the five SYNCHRONOUS
    /// `renderNow` survivors, which clears the terminal itself and paints before returning
    /// (`crossterm.rs:135`, `:225`). There is nothing to defer, so the setter is omitted rather
    /// than added dead; the field stays because [`Self::taken`] is what performs the clear, and the
    /// exit flush already routes through it.
    full: bool,
    last: Instant,
}

impl Default for FrameScheduler {
    fn default() -> Self {
        Self {
            requested: false,
            force: false,
            full: false,
            // `Instant::now()` minus one interval so the FIRST request is due immediately: a
            // session must not wait 16 ms for the frame that shows it started.
            last: Instant::now()
                .checked_sub(MIN_RENDER_INTERVAL)
                .unwrap_or_else(Instant::now),
        }
    }
}

impl FrameScheduler {
    /// pi's `requestRender()` — the ordinary path every arm takes.
    pub(crate) fn request(&mut self) {
        self.requested = true;
    }

    /// pi's `requestImmediateRender()` — the input arm only.
    pub(crate) fn request_immediate(&mut self) {
        self.requested = true;
        self.force = true;
    }

    /// Is a frame owed RIGHT NOW?
    pub(crate) fn due(&self) -> bool {
        self.requested && (self.force || self.last.elapsed() >= MIN_RENDER_INTERVAL)
    }

    /// How long until one is — `None` when nothing is pending, which the run loop expresses as the
    /// `pending()` arm every optional arm there already uses, so an idle session costs no wakeups.
    pub(crate) fn due_in(&self) -> Option<Duration> {
        if !self.requested {
            return None;
        }
        if self.force {
            return Some(Duration::ZERO);
        }
        Some(MIN_RENDER_INTERVAL.saturating_sub(self.last.elapsed()))
    }

    /// Consume the request. Returns whether this frame must repaint from scratch.
    pub(crate) fn taken(&mut self) -> bool {
        let full = self.full;
        self.requested = false;
        self.force = false;
        self.full = false;
        self.last = Instant::now();
        full
    }

    /// Whether a frame is owed at all, due or not — the exit path flushes on this, because a state
    /// change made just before a quit would otherwise never be drawn.
    pub(crate) fn pending(&self) -> bool {
        self.requested
    }
}
