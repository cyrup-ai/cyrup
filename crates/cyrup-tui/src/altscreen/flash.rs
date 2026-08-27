//! The alternate-screen **flash stack** — transient overlay notices, cyrup's port of pi's
//! `AltScreenFlashContainer` (`packages/tui/src/components/alt-screen-flash.ts` @v0.84.3) together
//! with the compositing pass that paints it (`tui-alt-screen.ts:1262-1273`). ADR-0005 §Decision
//! B-11.
//!
//! # Why the alternate screen needs one at all
//! The inline renderer answers a transient notice with the status line, so
//! [`crate::ViewportRenderer::flash`] is a no-op there (`app/shell.rs:513-516`). Fullscreen has no
//! status line of its own to write into, so upstream composites the notice over the top-right of
//! the already-rendered screen and drops it again on a timer — which is why `/copy` deliberately
//! forks between the two modes rather than calling `flash` unconditionally
//! (`interactive-mode.ts:6106-6112`, ADR-0005 §B-11).
//!
//! # Deadlines, not timers
//! Upstream gives every entry a `setTimeout` that splices it out of the list and calls
//! `requestRender` (`components/alt-screen-flash.ts:24-33`). cyrup has no per-component scheduler:
//! an entry records the [`Instant`] it dies at, [`overlay`] prunes the dead ones on the way to
//! painting, and [`next_expiry`] is what the alternate-screen loop schedules its next wake on so a
//! flash still clears itself with no further input. That wake is the loop's half of the contract —
//! this module never sleeps, blocks or performs I/O.
//!
//! # Stacking
//! Entries append and are painted oldest-first, one per row from the top of `area`
//! (`tui-alt-screen.ts:1267-1271`): a second flash arriving mid-display sits *under* the first
//! rather than replacing it. When more are live than there are rows, upstream's `.slice(-height)`
//! (`:1263`) keeps the newest.

use std::time::{Duration, Instant};

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::text_width::{str_width, truncate_to_width};

/// pi's `DEFAULT_DURATION_MS` (`components/alt-screen-flash.ts:4`) — the duration a `None` selects
/// on [`crate::ViewportRenderer::flash`], whose upstream signature carries the same default
/// (`tui-alt-screen.ts:534-536`).
pub(super) const DEFAULT_FLASH_DURATION: Duration = Duration::from_millis(1000);

/// One queued notice — pi's `FlashEntry` (`components/alt-screen-flash.ts:6-10`), with the
/// `NodeJS.Timeout` replaced by the deadline it would have fired at (see the module doc). Upstream's
/// `id` field exists only so the timer callback can find its own entry to splice out; pruning by
/// deadline needs no identity, so there is none here.
struct FlashEntry {
    /// The text as `flash` was given it — the padding and the truncation are applied at paint
    /// time, since both depend on the width of the frame the notice lands in
    /// (`components/alt-screen-flash.ts:46-49`).
    message: String,
    /// The first instant at which this entry is no longer painted.
    expires_at: Instant,
}

/// The live notices, oldest first — pi's `AltScreenFlashContainer.entries`
/// (`components/alt-screen-flash.ts:14`).
///
/// Owned by the alternate-screen renderer's UI bag (ADR-0005 §B-3), which is what
/// [`crate::ViewportRenderer::flash`] reaches to delegate into [`push`]. It holds no application
/// state, no theme and no transcript reference — the notice is a string and a deadline, and the
/// frame it paints into arrives as an argument.
#[derive(Default)]
pub(super) struct FlashStack {
    entries: Vec<FlashEntry>,
}

/// Queue `message` for `duration` — pi's `flash(message, durationMs = DEFAULT_DURATION_MS)`
/// (`components/alt-screen-flash.ts:22-36`). `None` is [`DEFAULT_FLASH_DURATION`].
///
/// Appends, so a flash arriving while another is displayed **stacks** rather than replacing it
/// (`:34`). Upstream additionally calls `requestRender` here (`:35`); cyrup's equivalent is the
/// loop redrawing on the next tick and re-arming its wake from [`next_expiry`].
pub(super) fn push(stack: &mut FlashStack, message: &str, duration: Option<Duration>) {
    let now = Instant::now();
    let ttl = duration.unwrap_or(DEFAULT_FLASH_DURATION);
    stack.entries.push(FlashEntry {
        message: message.to_owned(),
        // `Math.max(0, durationMs)` (`components/alt-screen-flash.ts:31`) clamps a nonsensical
        // duration to "gone at once"; `Instant::checked_add` is the only fallible step on this path
        // and a duration that overflows the monotonic clock degrades the same way rather than
        // panicking.
        expires_at: now.checked_add(ttl).unwrap_or(now),
    });
}

/// The earliest deadline still queued, or `None` when nothing is displayed.
///
/// The alternate-screen loop schedules its next redraw on this, standing in for the per-entry
/// `setTimeout` upstream arms (`components/alt-screen-flash.ts:24-33`): without it a flash would
/// linger until the next unrelated repaint, and the acceptance criterion is that it clears itself
/// with no further input.
pub(super) fn next_expiry(stack: &FlashStack) -> Option<Instant> {
    stack.entries.iter().map(|entry| entry.expires_at).min()
}

/// Drop every queued notice — pi's `dispose()` (`components/alt-screen-flash.ts:38-41`), which the
/// renderer calls when it leaves the alternate screen and when it stops
/// (`tui-alt-screen.ts:262`, `:303`). Upstream also clears the timers; cyrup has none to clear.
pub(super) fn clear(stack: &mut FlashStack) {
    stack.entries.clear();
}

/// Drop the notices whose deadline has passed — the pruning half of upstream's timer callback
/// (`components/alt-screen-flash.ts:26-29`).
fn expire(stack: &mut FlashStack, now: Instant) {
    stack.entries.retain(|entry| entry.expires_at > now);
}

/// Expire, then composite the survivors over `area` — pi's `compositeFlashes(screen, width, height)`
/// (`tui-alt-screen.ts:1262-1273`), called last in `doRender` so the notice sits above the document,
/// the selection highlight and the overlays (`:1290`).
///
/// Each notice is its own row, top-down from `area.y` and **right-aligned** at
/// `width - flashWidth` (`:1271`), drawn in reverse video — upstream wraps the row in
/// `\x1b[7m … \x1b[27m` (`components/alt-screen-flash.ts:48`) rather than in a theme colour, so
/// this needs no [`crate::UiTheme`] and cannot go stale against one.
pub(super) fn overlay(stack: &mut FlashStack, frame: &mut Frame, area: Rect) {
    expire(stack, Instant::now());
    if area.width == 0 || area.height == 0 || stack.entries.is_empty() {
        return;
    }
    let width = usize::from(area.width);
    let style = Style::default().add_modifier(Modifier::REVERSED);
    // `.slice(-height)` (`tui-alt-screen.ts:1263`): more notices than rows paints the newest ones.
    let first = stack.entries.len().saturating_sub(usize::from(area.height));
    let rows = area.y..area.y.saturating_add(area.height);
    for (entry, y) in stack.entries.iter().skip(first).zip(rows) {
        // The message behind one space of padding either side, cut to the frame with an EMPTY
        // ellipsis — upstream's `truncateToWidth(" ${entry.message} ", width, "")`
        // (`components/alt-screen-flash.ts:47`).
        let message = &entry.message;
        let text = truncate_to_width(&format!(" {message} "), width, "");
        let Ok(cols) = u16::try_from(str_width(&text)) else {
            continue;
        };
        let cols = cols.min(area.width);
        // `if (flashWidth === 0) continue;` (`tui-alt-screen.ts:1270`) — an empty notice leaves the
        // row it was allotted showing whatever was painted underneath it.
        if cols == 0 {
            continue;
        }
        let rect = Rect {
            x: area.x.saturating_add(area.width.saturating_sub(cols)),
            y,
            width: cols,
            height: 1,
        };
        frame.render_widget(Paragraph::new(Line::from(Span::styled(text, style))), rect);
    }
}
