//! The **live-widget seam** (SUBA-210, SUBA-211): what the host tells an extension about a widget it
//! published through [`crate::host::HostServices::set_widget`], and what the extension answers.
//!
//! # Why this exists
//!
//! pi's extension widgets are mounted **components**: `ctx.ui.setWidget(key, factory)` hands the
//! TUI a factory whose component is re-rendered on every frame and may implement `handleMouse`.
//! `pi-subagents`' async-jobs card is one (`buildWidgetComponent`, `src/tui/render.ts:2911`
//! @ad11b7ab): its `handleMouse` (`:2938-2944`) folds the card on a left click on row 0 with no
//! shift, alt or ctrl, and its `render` reads `process.stdout.rows`/`columns` (`:2536-2547`) so the
//! card picks its tier from the real terminal and relocks on a resize.
//!
//! cyrup's `set_widget` carries **lines**, not a component (the EXT-047 history note above
//! `ui.set-widget` in `wit/world.wit` says why it is key-shaped), so two halves of that were
//! missing: the widget could not be told about a pointer event, and it could not learn the terminal
//! size or that it changed. Both now cross as one callback, [`WidgetEvent`], delivered to the
//! extension that last published the widget's key:
//!
//! * a native gets [`crate::NativeExtension::on_widget_event`];
//! * a WASM guest gets the `events.handle-widget-event` export, and reads the size through the
//!   `ui.get-terminal-size` import ([`crate::host::HostServices::terminal_size`]).
//!
//! The extension answers by republishing through `set_widget`, exactly as it publishes anything
//! else; the [`WidgetEventOutcome`] only says whether the event was its own, which is what the TUI
//! needs to decide whether a click belongs to the widget or to the text selection under it.
//!
//! # The precedent, and where this diverges from it
//!
//! [`crate::host::InteractiveOverlay::handle_mouse`] is the precedent for the callback shape:
//! positions local to what the component painted, a press/click/wheel set with the press/release
//! pair already folded into a click by the host, and an answer that separates "not mine" (the host
//! may select text) from "handled". [`WidgetPointer`] is that set, field for field, and
//! [`WidgetEventOutcome`] is [`crate::host::OverlayMouseOutcome`] without the two arms a widget
//! cannot use. Three differences, each forced:
//!
//! * **These types live outside `host`.** `host::overlay` is behind the `wasm-host` feature, and the
//!   native callback ([`crate::NativeExtension::on_widget_event`]) is not: a native extension runs
//!   in a build with no wasm host at all. The overlay types could not be reused without gating the
//!   native trait.
//! * **[`WidgetMouse`] carries the modifier keys.** pi's widget `handleMouse` reads
//!   `event.shift || event.alt || event.ctrl` and ignores a modified click (`:2939`), so the
//!   modifiers are part of what decides the outcome; no overlay consumer upstream reads them.
//! * **No `Redraw`/`Close`.** A widget changes what is drawn by republishing its lines, and that
//!   publish is what repaints; there is no modal to close.

use serde::{Deserialize, Serialize};

/// The terminal's size in cells — pi `process.stdout.columns` / `process.stdout.rows`.
///
/// This is the whole TERMINAL, not the region a widget was painted in: pi's widget reads
/// `process.stdout.rows` and subtracts its own reserve (`RESERVED_NON_WIDGET_ROWS = 19`,
/// `render.ts:2528` @ad11b7ab), so the extension is handed what pi's is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalSize {
    /// pi `process.stdout.columns`.
    pub columns: u16,
    /// pi `process.stdout.rows`.
    pub rows: u16,
}

/// The modifier keys held during a pointer event — pi's `MouseEvent.shift` / `.alt` / `.ctrl`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PointerModifiers {
    /// Shift was held.
    pub shift: bool,
    /// Alt (Option, Meta) was held.
    pub alt: bool,
    /// Control was held.
    pub ctrl: bool,
}

impl PointerModifiers {
    /// Whether any modifier was held.
    #[must_use]
    pub fn any(self) -> bool {
        self.shift || self.alt || self.ctrl
    }
}

/// One pointer event on a widget, as the closed set a component can be told about — the same set
/// as [`crate::host::OverlayMouse`] (see the module docs for why it is a separate type).
///
/// Positions are **local to the widget**: `row` 0 is the first line of the widget's own content
/// (pi's `event.y === 0`), and `column` 0 its first cell. The button is always the primary one:
/// the host routes no other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WidgetPointer {
    /// The primary button went down.
    Press {
        /// Column inside the widget.
        column: u16,
        /// Row inside the widget.
        row: u16,
    },
    /// A press and its release landed on the same cell with nothing in between (pi's `click`).
    /// `count` is the number of consecutive clicks on that cell: 1, 2 or 3.
    Click {
        /// Column inside the widget.
        column: u16,
        /// Row inside the widget.
        row: u16,
        /// Consecutive clicks on this cell (1 to 3).
        count: u8,
    },
    /// One wheel notch. `lines` is signed: negative scrolls up.
    Wheel {
        /// Column inside the widget.
        column: u16,
        /// Row inside the widget.
        row: u16,
        /// Notch direction and size; negative is up.
        lines: i32,
    },
}

impl WidgetPointer {
    /// The cell the event landed on, as `(column, row)` inside the widget.
    #[must_use]
    pub fn cell(self) -> (u16, u16) {
        match self {
            Self::Press { column, row }
            | Self::Click { column, row, .. }
            | Self::Wheel { column, row, .. } => (column, row),
        }
    }
}

/// A pointer event and the modifiers held during it — pi's widget `handleMouse` argument
/// (`{ type, button, y, shift, alt, ctrl }`, `render.ts:2938` @ad11b7ab).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WidgetMouse {
    /// What happened, where.
    pub pointer: WidgetPointer,
    /// The modifier keys held.
    pub modifiers: PointerModifiers,
}

impl WidgetMouse {
    /// pi's fold test, whole: a `click` (`event.type === "click"`, primary button) on `row`, with
    /// no shift, alt or ctrl (`render.ts:2939-2940` @ad11b7ab).
    #[must_use]
    pub fn is_plain_click_on_row(&self, row: u16) -> bool {
        matches!(self.pointer, WidgetPointer::Click { row: r, .. } if r == row)
            && !self.modifiers.any()
    }
}

/// What the host tells a widget's owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WidgetEvent {
    /// A pointer event landed on the widget. pi `component.handleMouse(event)`.
    Mouse(WidgetMouse),
    /// The terminal was resized to this size. pi re-renders every mounted component on a resize,
    /// and its widget reads the new `process.stdout.rows`/`columns` there (`render.ts:2549-2553`
    /// @ad11b7ab, `widgetSessionMatches`); a cyrup widget hands over lines once, so it is told, and
    /// republishes. Delivered once per widget key the extension owns.
    Resize(TerminalSize),
}

/// How a widget's owner answered a [`WidgetEvent`] — pi's `handleMouse` result, where `undefined`
/// means "not mine" and `{ handled: true }` means it was.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WidgetEventOutcome {
    /// Not the widget's: a click on a row it does not act on, a modified click, a resize it does
    /// not care about, or no owner at all. A click the widget leaves alone stays the text
    /// selection's, so the widget's text can still be selected and copied.
    #[default]
    Unhandled,
    /// The widget took the event. A widget that changed state has already republished its lines
    /// through `set_widget`.
    Handled,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn click(row: u16, modifiers: PointerModifiers) -> WidgetMouse {
        WidgetMouse {
            pointer: WidgetPointer::Click {
                column: 3,
                row,
                count: 1,
            },
            modifiers,
        }
    }

    /// pi's fold test: a plain click on row 0 folds; a modified one, a click on another row, a
    /// press and a wheel notch on row 0 do not.
    #[test]
    fn only_a_plain_click_on_the_row_matches() {
        assert!(click(0, PointerModifiers::default()).is_plain_click_on_row(0));
        assert!(!click(1, PointerModifiers::default()).is_plain_click_on_row(0));
        for modifiers in [
            PointerModifiers {
                shift: true,
                ..PointerModifiers::default()
            },
            PointerModifiers {
                alt: true,
                ..PointerModifiers::default()
            },
            PointerModifiers {
                ctrl: true,
                ..PointerModifiers::default()
            },
        ] {
            assert!(
                !click(0, modifiers).is_plain_click_on_row(0),
                "{modifiers:?}"
            );
        }
        let press = WidgetMouse {
            pointer: WidgetPointer::Press { column: 0, row: 0 },
            modifiers: PointerModifiers::default(),
        };
        assert!(!press.is_plain_click_on_row(0));
        let wheel = WidgetMouse {
            pointer: WidgetPointer::Wheel {
                column: 0,
                row: 0,
                lines: 1,
            },
            modifiers: PointerModifiers::default(),
        };
        assert!(!wheel.is_plain_click_on_row(0));
        assert_eq!(wheel.pointer.cell(), (0, 0));
    }
}
