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
//! * the topmost overlay whose painted rectangle contains the pointer is offered the event, and
//!   **nothing beneath it is also offered it** — not the dock, not the editor, not the scrolled
//!   document, not the scrollbar. That is pi's `overlay.hit` (`tui-alt-screen.ts:929-960`);
//! * what the overlay *answers* decides the rest ([`OverlayPointerOutcome`]). A component that
//!   handles a press (a row, an input) keeps the gesture: a text selection already made is
//!   cleared, no new one starts, and the release comes back as a click. A press it does not handle
//!   (pi's `overlay.result` undefined) is [`OverlayRoute::Over`]: it falls through to the screen
//!   text selection, so the text of a modal can be selected and copied, as pi's
//!   `result ?? (hit ? undefined : layout)` leaves it to `handleSelectionMouseEvent`;
//! * a wheel notch the overlay does not handle is deferred to the overlay, as pi's
//!   `shouldDeferViewportInputToOverlay` does (`:709`): it is consumed here and scrolls nothing;
//! * a pointer outside every overlay is a miss ([`OverlayRoute::Miss`]) and the caller falls
//!   through to the dock and the document, which no longer takes the wheel while a modal is open
//!   (see [`crate::altscreen::Modal`]);
//! * positions are made local to the overlay's rectangle ([`Pointer::localized`]), so the overlay
//!   indexes straight into the lines it painted;
//! * a press the overlay handled and its release on the same cell are folded into a
//!   [`Pointer::Click`] carrying the consecutive-click count, as for the dock ([`super::pointer`]).
//!   A press it left to the selection becomes a click through the selection's release
//!   ([`App::offer_overlay_click`]), so a drag across text copies it and a click on text selects
//!   nothing.
//!
//! Everything here is inert in regular mode, where the terminal is never asked for mouse reports.

use std::time::Instant;

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Position;

use super::pointer::DOUBLE_CLICK_INTERVAL;
use super::{App, AppAction, Pointer};
use crate::overlay::OverlayPointerOutcome;

/// Where a mouse report goes after the floating overlays have seen it.
#[derive(Debug, PartialEq)]
pub(crate) enum OverlayRoute {
    /// The pointer is outside every overlay: the dock and the document are offered the report.
    Miss,
    /// The pointer is over an overlay that did not handle the report. Nothing beneath the overlay
    /// is offered it, but the text selection is: the overlay's painted rows are in the frame.
    Over,
    /// An overlay took the report; this is the answer, and nothing else sees it.
    Taken(AppAction),
}

/// What an overlay made of a click offered after a selection's release.
#[derive(Debug, PartialEq)]
pub(crate) enum OverlayClick {
    /// No overlay is under the cell: the dock may take the click.
    Miss,
    /// An overlay is under the cell and left the click alone; nothing beneath it is offered it.
    Unhandled,
    /// An overlay took the click.
    Taken(AppAction),
}

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
    /// Offer a mouse report to the floating overlays. See [`OverlayRoute`] for what each answer
    /// obliges the caller to do next.
    pub(crate) fn handle_overlay_pointer(&mut self, ev: &MouseEvent) -> OverlayRoute {
        let at = Position::new(ev.column, ev.row);
        match ev.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let Some(index) = self.overlay_at(at) else {
                    return OverlayRoute::Miss;
                };
                let lines = if matches!(ev.kind, MouseEventKind::ScrollUp) {
                    -1
                } else {
                    1
                };
                // A notch the overlay does not handle is deferred to it, not given to the
                // document: consumed, and it scrolls nothing.
                OverlayRoute::Taken(
                    self.dispatch_overlay_pointer(index, Pointer::Wheel { at, lines })
                        .unwrap_or(AppAction::None),
                )
            }
            MouseEventKind::Down(MouseButton::Left) => {
                // A press ends whatever an earlier press began.
                self.state.overlay_pointer.gesture = None;
                let Some(index) = self.overlay_at(at) else {
                    return OverlayRoute::Miss;
                };
                let Some(action) = self.dispatch_overlay_pointer(index, Pointer::Press { at })
                else {
                    // Not the overlay's press: it starts a text selection on the painted rows.
                    return OverlayRoute::Over;
                };
                self.state.overlay_pointer.gesture = Some(Gesture {
                    overlay: index,
                    press: at,
                    moved: false,
                });
                // The press is the overlay's, so a selection already made is dropped and none
                // starts: pi's `clearTextSelection()` on a handled press.
                if let Some(alt) = self.altscreen.as_mut() {
                    alt.clear_selection();
                }
                OverlayRoute::Taken(action)
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                // Without a press of the overlay's, a drag is the selection's (or the dock's).
                let Some(gesture) = self.state.overlay_pointer.gesture.as_mut() else {
                    return OverlayRoute::Miss;
                };
                if at != gesture.press {
                    gesture.moved = true;
                }
                OverlayRoute::Taken(AppAction::None)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let Some(gesture) = self.state.overlay_pointer.gesture.take() else {
                    return OverlayRoute::Miss;
                };
                if gesture.moved || at != gesture.press {
                    return OverlayRoute::Taken(AppAction::None);
                }
                let count =
                    self.state
                        .overlay_pointer
                        .register_click(Instant::now(), at, gesture.overlay);
                OverlayRoute::Taken(
                    self.dispatch_overlay_pointer(gesture.overlay, Pointer::Click { at, count })
                        .unwrap_or(AppAction::None),
                )
            }
            // Any other report over an overlay (a right press, a middle click, hover) is not
            // handled by a component: pi tries the right-click paste and the selection, which
            // ignore the rest. It must not reach the editor or the document under the modal.
            _ => {
                if self.overlay_at(at).is_some() {
                    OverlayRoute::Over
                } else {
                    OverlayRoute::Miss
                }
            }
        }
    }

    /// A clean click that began a text selection on the screen, released on `at`: offer it to the
    /// overlay under the cell, as pi's release does (`dispatchMouseToOverlay(clickEvent)`,
    /// `tui-alt-screen.ts:1346-1347`). `count` is the selection ladder's.
    pub(crate) fn offer_overlay_click(&mut self, at: Position, count: u8) -> OverlayClick {
        let Some(index) = self.overlay_at(at) else {
            return OverlayClick::Miss;
        };
        match self.dispatch_overlay_pointer(index, Pointer::Click { at, count }) {
            Some(action) => OverlayClick::Taken(action),
            None => OverlayClick::Unhandled,
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
    /// painted in. `None` when the overlay did not handle it. A `Close` tears the overlay down like
    /// a key that closes it does.
    fn dispatch_overlay_pointer(&mut self, index: usize, event: Pointer) -> Option<AppAction> {
        let overlay = self.state.overlays.get_mut(index)?;
        let rect = overlay.painted_rect()?;
        match overlay.pointer(event.localized(rect)) {
            OverlayPointerOutcome::Unhandled => None,
            OverlayPointerOutcome::Close => {
                self.state.overlays.remove(index);
                Some(AppAction::Redraw)
            }
            OverlayPointerOutcome::Redraw => Some(AppAction::Redraw),
            OverlayPointerOutcome::Handled => Some(AppAction::None),
        }
    }
}
