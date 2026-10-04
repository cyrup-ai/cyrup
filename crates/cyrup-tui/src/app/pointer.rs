//! The pointer over the dock: clicks and wheel notches on the editor, its completion popup and the
//! selector that replaces it.
//!
//! Pi routes a mouse report through its component tree: every component may implement
//! `handleMouse`, a container forwards the event to the child under the pointer after translating
//! the coordinates, and a result says whether the event was handled, whether the child wants the
//! rest of the gesture, and whether it wants focus. ratatui has no component tree to walk, and
//! does not need one. The screen is already a set of rectangles — [`Regions`], recorded when the
//! frame was painted — so the same thing is three steps:
//!
//! 1. **Resolve.** The pointer position picks a [`DockTarget`] from the rectangles the last frame
//!    was painted with. The painter and the hit test read one source, so they cannot disagree about
//!    where the editor, the popup or a selector row is.
//! 2. **Translate.** The raw crossterm report becomes a [`Pointer`] — a closed enum of the three
//!    things a component can be told (a press, a click, a wheel notch) — with its position made
//!    local to the target's rectangle.
//! 3. **Dispatch.** A `match` on the target calls the one method that owns that component's
//!    geometry: [`crate::editor::InputEditor::pointer`] for the editor and its popup,
//!    [`crate::selector::Selector::pointer`] for a selector. Each answers with what it would have
//!    answered a key: a redraw, a selection, a confirmation.
//!
//! A click is not a raw event. The terminal reports a press and, later, a release; pi's renderer
//! turns a press and a release on the same cell, with nothing in between, into a `click` carrying a
//! consecutive-click count, and components act on that (a list row is *activated* on click but only
//! *selected* on press). [`Gesture`] is that bookkeeping: it remembers where a press began and
//! whether the pointer has moved since.
//!
//! Everything here is inert in regular mode, where the terminal is never asked for mouse reports.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::{App, AppAction, AppState, Regions};

/// Consecutive clicks on the same cell count as a double or triple click only if each follows the
/// last within this window — pi's `DOUBLE_CLICK_INTERVAL_MS = 500` (`tui-alt-screen.ts:68`).
pub(super) const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// What a component is told about the pointer. Positions are **local**: `(0, 0)` is the top-left
/// cell of the rectangle the component was painted in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pointer {
    /// The left button went down. A list row is *selected* on press.
    Press { at: Position },
    /// A press and its release landed on the same cell with nothing in between. `count` is the
    /// number of consecutive clicks on that cell: 1, 2 or 3. A list row is *activated* on click.
    Click { at: Position, count: u8 },
    /// One wheel notch. `lines` is signed: negative scrolls up.
    Wheel { at: Position, lines: i32 },
}

impl Pointer {
    /// The same event with its position translated into `origin`'s coordinates.
    #[must_use]
    pub fn localized(self, origin: Rect) -> Self {
        let local = |at: Position| {
            Position::new(at.x.saturating_sub(origin.x), at.y.saturating_sub(origin.y))
        };
        match self {
            Self::Press { at } => Self::Press { at: local(at) },
            Self::Click { at, count } => Self::Click {
                at: local(at),
                count,
            },
            Self::Wheel { at, lines } => Self::Wheel {
                at: local(at),
                lines,
            },
        }
    }
}

/// Which docked component a position falls in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DockTarget {
    /// The editor's completion popup, directly under the editor.
    Popup,
    /// The editor slot: the editor, or the selector that replaced it.
    Slot,
}

impl DockTarget {
    /// The target `at` falls in, against the rectangles the last frame was painted with. The popup
    /// is tried first; the two never overlap, but a zero-height slot would otherwise swallow a
    /// zero-height popup's row.
    pub(crate) fn at(regions: &Regions, at: Position) -> Option<Self> {
        if regions.popup.contains(at) {
            Some(Self::Popup)
        } else if regions.slot.contains(at) {
            Some(Self::Slot)
        } else {
            None
        }
    }

    fn rect(self, regions: &Regions) -> Rect {
        match self {
            Self::Popup => regions.popup,
            Self::Slot => regions.slot,
        }
    }
}

/// A press that landed on a docked component and has not been released yet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Gesture {
    target: DockTarget,
    press: Position,
    /// Whether the pointer left the cell it went down on. A press dragged away is not a click.
    moved: bool,
}

/// The last click, for counting consecutive ones.
#[derive(Clone, Copy, Debug)]
struct LastClick {
    at: Instant,
    cell: Position,
    target: DockTarget,
    count: u8,
}

/// Pointer bookkeeping that outlives one event.
#[derive(Debug, Default)]
pub(crate) struct PointerState {
    gesture: Option<Gesture>,
    last_click: Option<LastClick>,
}

impl PointerState {
    /// Record the click a release completes and answer its consecutive-click count: one more than
    /// the previous click if it was on the same cell of the same component within
    /// [`DOUBLE_CLICK_INTERVAL`], wrapping 3 → 1 as pi's `(previous.count % 3) + 1` does
    /// (`tui-alt-screen.ts:867-881`).
    fn register_click(&mut self, now: Instant, cell: Position, target: DockTarget) -> u8 {
        let count = match self.last_click {
            Some(previous)
                if previous.target == target
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
            target,
            count,
        });
        count
    }

    /// Whether a press on a docked component is waiting for its release — how a test sees that the
    /// dock was offered a report.
    #[cfg(test)]
    pub(crate) fn press_in_flight(&self) -> bool {
        self.gesture.is_some()
    }

    /// Forget a gesture in flight, for a focus change or a mode switch that would orphan it.
    pub(crate) fn cancel(&mut self) {
        self.gesture = None;
    }
}

impl<B: ratatui::backend::Backend> App<B> {
    /// Offer a mouse report to the docked components. `Some` means the dock claimed the event and
    /// the caller must not also give it to the scrolled document; `None` means it is the
    /// document's.
    ///
    /// A wheel notch over a component the component does not act on (an editor with nothing to
    /// scroll, a selector without a list) answers `None`, so it falls through to the document —
    /// pi's `routeWheel` does the same, handing whatever no component took to the primary scroll
    /// view.
    pub(crate) fn handle_dock_pointer(&mut self, ev: &MouseEvent) -> Option<AppAction> {
        let at = Position::new(ev.column, ev.row);
        let regions = self.state.regions;
        match ev.kind {
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let target = DockTarget::at(&regions, at)?;
                let lines = if matches!(ev.kind, MouseEventKind::ScrollUp) {
                    -1
                } else {
                    1
                };
                self.dispatch_pointer(target, Pointer::Wheel { at, lines })
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let target = DockTarget::at(&regions, at)?;
                self.state.pointer.gesture = Some(Gesture {
                    target,
                    press: at,
                    moved: false,
                });
                // The press is claimed whether or not the component acts on it: the document's
                // selection must not start under the editor, and one already made is dropped, as
                // pi's `clearTextSelection()` does on a handled press.
                if let Some(alt) = self.altscreen.as_mut() {
                    alt.clear_selection();
                }
                Some(
                    self.dispatch_pointer(target, Pointer::Press { at })
                        .unwrap_or(AppAction::None),
                )
            }
            MouseEventKind::Drag(MouseButton::Left) => {
                let gesture = self.state.pointer.gesture.as_mut()?;
                if at != gesture.press {
                    gesture.moved = true;
                }
                Some(AppAction::None)
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let gesture = self.state.pointer.gesture.take()?;
                if gesture.moved || at != gesture.press {
                    return Some(AppAction::None);
                }
                let count = self
                    .state
                    .pointer
                    .register_click(Instant::now(), at, gesture.target);
                Some(
                    self.dispatch_pointer(gesture.target, Pointer::Click { at, count })
                        .unwrap_or(AppAction::None),
                )
            }
            _ => None,
        }
    }

    /// Hand `event` to the component `target` names, positions made local to its rectangle.
    /// `None` when the component did not act on it.
    fn dispatch_pointer(&mut self, target: DockTarget, event: Pointer) -> Option<AppAction> {
        let area = target.rect(&self.state.regions);
        let event = event.localized(area);
        match target {
            DockTarget::Popup => {
                let AppState { editor, .. } = &mut self.state;
                // Everything over the popup is the popup's, repaint or not: a wheel notch against
                // the end of the list is handled (pi `select-list.ts:116`), and must not scroll
                // the document under it.
                Some(if editor.pointer_popup(area, event) {
                    AppAction::Redraw
                } else {
                    AppAction::None
                })
            }
            DockTarget::Slot => {
                if let Some(active) = self.state.selector.as_mut() {
                    let kind = active.kind;
                    let outcome = active.inner.pointer(area, event);
                    return match outcome {
                        crate::selector::SelectorOutcome::Ignored => None,
                        outcome => Some(self.apply_selector_outcome(kind, outcome)),
                    };
                }
                if self.state.loader.is_some() {
                    return None;
                }
                self.state
                    .editor
                    .pointer(area, event)
                    .then_some(AppAction::Redraw)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cell(x: u16, y: u16) -> Position {
        Position::new(x, y)
    }

    /// Three clicks on one cell inside the window count 1, 2, 3 and then wrap to 1 — pi's
    /// `(previous.count % 3) + 1` (`tui-alt-screen.ts:867-881`).
    #[test]
    fn consecutive_clicks_on_one_cell_count_to_three_and_wrap() {
        let mut state = PointerState::default();
        let t0 = Instant::now();
        let counts: Vec<u8> = (0..4)
            .map(|i| {
                state.register_click(
                    t0 + Duration::from_millis(100 * i),
                    cell(4, 2),
                    DockTarget::Slot,
                )
            })
            .collect();
        assert_eq!(counts, [1, 2, 3, 1]);
    }

    /// A click after the window, on another cell, or on another component starts over.
    #[test]
    fn a_pause_a_move_or_another_component_resets_the_count() {
        let t0 = Instant::now();
        let mut state = PointerState::default();
        assert_eq!(state.register_click(t0, cell(4, 2), DockTarget::Slot), 1);
        let late = t0 + DOUBLE_CLICK_INTERVAL + Duration::from_millis(1);
        assert_eq!(state.register_click(late, cell(4, 2), DockTarget::Slot), 1);
        assert_eq!(
            state.register_click(late, cell(5, 2), DockTarget::Slot),
            1,
            "another cell"
        );
        assert_eq!(
            state.register_click(late, cell(5, 2), DockTarget::Popup),
            1,
            "another component"
        );
        assert_eq!(
            state.register_click(late, cell(5, 2), DockTarget::Popup),
            2,
            "and the next click on it counts again"
        );
    }

    /// A point in neither rectangle is not the dock's, and where they would overlap the popup wins:
    /// it is drawn over whatever it sits on.
    #[test]
    fn a_position_resolves_to_the_rectangle_it_falls_in() {
        let regions = Regions {
            slot: Rect::new(0, 10, 20, 5),
            popup: Rect::new(0, 13, 20, 2),
            ..Regions::default()
        };
        assert_eq!(
            DockTarget::at(&regions, cell(3, 10)),
            Some(DockTarget::Slot)
        );
        assert_eq!(
            DockTarget::at(&regions, cell(3, 13)),
            Some(DockTarget::Popup),
            "the popup is tried first"
        );
        assert_eq!(DockTarget::at(&regions, cell(3, 9)), None);
        assert_eq!(DockTarget::at(&regions, cell(3, 15)), None);
    }

    /// A position is made local to the rectangle it is dispatched to.
    #[test]
    fn a_pointer_is_translated_into_the_target_rectangle() {
        let origin = Rect::new(5, 10, 20, 3);
        let at = cell(7, 11);
        assert_eq!(
            Pointer::Press { at }.localized(origin),
            Pointer::Press { at: cell(2, 1) }
        );
        assert_eq!(
            Pointer::Click { at, count: 2 }.localized(origin),
            Pointer::Click {
                at: cell(2, 1),
                count: 2
            }
        );
        assert_eq!(
            Pointer::Wheel { at, lines: -1 }.localized(origin),
            Pointer::Wheel {
                at: cell(2, 1),
                lines: -1
            }
        );
    }
}
