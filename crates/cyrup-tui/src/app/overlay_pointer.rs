//! The pointer over floating overlays: an extension's modal list or text field answering a click or
//! a wheel notch.
//!
//! Pi's `dispatchMouseToOverlay` (`tui.ts`) hands a mouse report to the overlay under the pointer
//! before anything else sees it. ratatui needs no component tree for that, because an overlay
//! already records the one fact the test needs: [`Overlay::painted_rect`], the rectangle its own
//! `render` computed and painted. The hit test therefore reads what was painted and the painter
//! cannot disagree with it.
//!
//! The rules, all of them a consequence of the overlay being a modal:
//!
//! * the topmost overlay whose painted rectangle contains the pointer takes the event, and
//!   **nothing beneath it is also offered the event** — not the dock, not the editor, not the
//!   scrolled document. Whatever the overlay answers, a hit is consumed;
//! * a pointer outside every overlay is a miss, and the caller falls through to the dock and the
//!   document exactly as it did before overlays could take the pointer;
//! * positions are made local to the overlay's rectangle ([`Pointer::localized`]), so the overlay
//!   indexes straight into the lines it painted;
//! * a press and its release on the same cell are folded into a [`Pointer::Click`] carrying the
//!   consecutive-click count, as for the dock ([`super::pointer`]).
//!
//! Everything here is inert in regular mode, where the terminal is never asked for mouse reports.

use std::time::Instant;

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::pointer::DOUBLE_CLICK_INTERVAL;
use super::{App, AppAction, Pointer};
use crate::overlay::OverlayOutcome;

/// A press that landed on an overlay and has not been released yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Gesture {
    /// Index into the overlay stack of the overlay that took the press.
    overlay: usize,
    press: Position,
    /// Whether the pointer left the cell it went down on. A press dragged away is not a click.
    moved: bool,
}

/// The last click on an overlay, for counting consecutive ones.
#[derive(Clone, Copy, Debug)]
struct LastClick {
    at: Instant,
    cell: Position,
    overlay: usize,
    count: u8,
}

/// Overlay pointer bookkeeping that outlives one event.
#[derive(Debug, Default)]
pub(crate) struct OverlayPointerState {
    gesture: Option<Gesture>,
    last_click: Option<LastClick>,
}

impl OverlayPointerState {
    /// Record the click a release completes and answer its consecutive-click count: one more than
    /// the previous click if it was on the same cell of the same overlay within the dock's
    /// double-click window, wrapping 3 to 1 as pi's `(previous.count % 3) + 1` does.
    fn register_click(&mut self, now: Instant, cell: Position, overlay: usize) -> u8 {
        let count = match self.last_click {
            Some(previous)
                if previous.overlay == overlay
                    && previous.cell == cell
                    && now.saturating_duration_since(previous.at) <= DOUBLE_CLICK_INTERVAL =>
            {
                (previous.count % 3) + 1
            }
            _ => 1,
        };
        self.last_click = Some(LastClick {
            at: now,
            cell,
            overlay,
            count,
        });
        count
    }
}

impl<B: ratatui::backend::Backend> App<B> {
    /// Offer a mouse report to the floating overlays. `Some` means an overlay took the event and
    /// the caller must offer it to nothing else; `None` is a miss and the report is the dock's.
    pub(crate) fn handle_overlay_pointer(&mut self, ev: &MouseEvent) -> Option<AppAction> {
        let at = Position::new(ev.column, ev.row);
        match ev.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let index = self.overlay_at(at)?;
                let lines = if matches!(ev.kind, MouseEventKind::ScrollUp) {
                    -1
                } else {
                    1
                };
                Some(self.dispatch_overlay_pointer(index, Pointer::Wheel { at, lines }))
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let Some(index) = self.overlay_at(at) else {
                    // A press elsewhere ends whatever an earlier press began.
                    self.state.overlay_pointer.gesture = None;
                    return None;
                };
                self.state.overlay_pointer.gesture = Some(Gesture {
                    overlay: index,
                    press: at,
                    moved: false,
                });
                // The document's selection must not start under the modal, and one already made
                // is dropped, as for a press the dock takes.
                if let Some(alt) = self.altscreen.as_mut() {
                    alt.clear_selection();
                }
                Some(self.dispatch_overlay_pointer(index, Pointer::Press { at }))
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let gesture = self.state.overlay_pointer.gesture.as_mut()?;
                if at != gesture.press {
                    gesture.moved = true;
                }
                Some(AppAction::None)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let gesture = self.state.overlay_pointer.gesture.take()?;
                if gesture.moved || at != gesture.press {
                    return Some(AppAction::None);
                }
                let count =
                    self.state
                        .overlay_pointer
                        .register_click(Instant::now(), at, gesture.overlay);
                Some(self.dispatch_overlay_pointer(gesture.overlay, Pointer::Click { at, count }))
            }
            // Any other report over an overlay (a right-click paste, a middle click, hover) is
            // still a report over the modal: it must not reach the editor or the document under it.
            _ => self.overlay_at(at).map(|_| AppAction::None),
        }
    }

    /// The topmost overlay whose painted rectangle contains `at`.
    fn overlay_at(&self, at: Position) -> Option<usize> {
        self.state
            .overlays
            .iter()
            .enumerate()
            .rev()
            .find(|(_, overlay)| overlay.painted_rect().is_some_and(|rect| rect.contains(at)))
            .map(|(index, _)| index)
    }

    /// Hand `event` to the overlay at `index`, its position made local to the rectangle it was
    /// painted in. A `Close` tears the overlay down like a key that closes it does.
    fn dispatch_overlay_pointer(&mut self, index: usize, event: Pointer) -> AppAction {
        let Some(overlay) = self.state.overlays.get_mut(index) else {
            return AppAction::None;
        };
        let Some(rect) = overlay.painted_rect() else {
            return AppAction::None;
        };
        match overlay.pointer(event.localized(rect)) {
            OverlayOutcome::Close => {
                self.state.overlays.remove(index);
                AppAction::Redraw
            }
            OverlayOutcome::Redraw => AppAction::Redraw,
            OverlayOutcome::Ignored => AppAction::None,
        }
    }
}
