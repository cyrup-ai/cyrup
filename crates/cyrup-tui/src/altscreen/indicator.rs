//! The **"Jump to latest message" indicator** — pi's scroll-to-end label (TUI-109), drawn while the
//! fullscreen transcript is scrolled away from its tail and clickable to return there.
//!
//! # What upstream does
//! `TuiAltScreen.compositeScrollToEndIndicator` (`tui-alt-screen.ts:1623-1645` @v0.87.1) runs after
//! the document is laid out and before the selection and the flash stack are composited. It
//! requires a `follow: "end"` primary scroll view that is NOT currently following its end, takes
//! the last row of that view's clip, centres the label on the clip's width, and stops it at the
//! scrollbar column (the column, when a bar is painted, else the clip's right edge). A row holding
//! an image line is left alone. The painted span is recorded as `scrollToEndIndicatorRect`, and
//! `handleScrollToEndIndicatorMouseEvent` (`:1018-1024`) turns an unmodified-button left press
//! inside it into `scrollToBottom()`; it runs ahead of the scrollbar handling (`:916`).
//!
//! The label text is the coding-agent's (`modes/interactive/tui-renderer.ts:29-33`):
//! `` ` ↓ Jump to latest message${shortcut ? ` · ${shortcut}` : ""} ` `` on `selectedBg` over the
//! `text` foreground, `shortcut` being `keyDisplayText("tui.altScreen.bottom")`. This renderer does
//! not own a keymap, so the composition root pushes that shortcut in with
//! [`super::AltScreen::set_scroll_to_end_key`] whenever the bindings change.
//!
//! # Where cyrup differs
//! - **Paint order.** `[CYRUP-DELTA]` Upstream composites the label before the selection tint; here
//!   it is painted after [`super::selection::highlight`] and the attachment strip, so a selection
//!   that covers the last row does not tint the label. A clickable control that a drag-selection
//!   recolours stops reading as a control, and nothing is lost: the selection's text is copied from
//!   the document, never from the painted cells.
//! - **Image rows.** Upstream's `isImageLine` tests the row's string for a kitty/iTerm2 sequence.
//!   cyrup's transcript document is text, so the only rows a graphics sequence can occupy are the
//!   attachment strip's, which [`super::images::place`] reports as a row count.

use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::text_width::{str_width, truncate_to_width};
use crate::theme::UiTheme;

use super::scroll;

/// The indicator's state: the shortcut text pushed by the composition root, and the rectangle it
/// was last painted at — pi's `scrollToEndIndicator` callback result and
/// `scrollToEndIndicatorRect` (`tui-alt-screen.ts:181`, `:224`).
#[derive(Debug, Default)]
pub(super) struct Indicator {
    /// Whether the owner supplied the indicator at all — upstream's `scrollToEndIndicator` option
    /// is optional (`tui-alt-screen.ts:181`, tested at `:1626`), and a bare `TuiAltScreen` (every
    /// case in `test/tui-alt-screen.test.ts`) has none. Set by the first [`set_key`].
    installed: bool,
    /// `keyDisplayText("tui.altScreen.bottom")`, `None` when the action is unbound
    /// (`shortcut ? … : ""`).
    key: Option<String>,
    /// Where the label sits as of the last frame; `None` when it was not painted. Re-derived by
    /// every [`paint`], so a frame that hides the label also disarms the click (`:1624`).
    rect: Option<Rect>,
}

/// Install the indicator and record the shortcut shown in its label. An empty string is unbound,
/// as upstream's falsy test.
pub(super) fn set_key(ind: &mut Indicator, key: Option<String>) {
    ind.installed = true;
    ind.key = key.filter(|k| !k.is_empty());
}

/// The label text, including its padding spaces — `tui-renderer.ts:30-32`.
fn label(ind: &Indicator) -> String {
    match ind.key.as_deref() {
        Some(key) => format!(" ↓ Jump to latest message · {key} "),
        None => " ↓ Jump to latest message ".to_owned(),
    }
}

/// Composite the label onto the last row of `area` — `compositeScrollToEndIndicator`.
///
/// `image_rows` is how many rows from the top of `area` the attachment strip fills with a graphics
/// protocol image; the label is skipped when the last row is one of them (`isImageLine`).
pub(super) fn paint(
    ind: &mut Indicator,
    bar: &scroll::ScrollbarView,
    scroll: &scroll::ScrollState,
    theme: &UiTheme,
    frame: &mut Frame,
    area: Rect,
    image_rows: u16,
) {
    ind.rect = None;
    // `!this.scrollToEndIndicator || !scrollView.followEnd || scrollView.isFollowingEnd` (`:1626`).
    // The transcript always follows its end, so the middle term cannot fail.
    if !ind.installed || scroll::is_following_end(scroll) || area.width == 0 || area.height == 0 {
        return;
    }
    let row = area.y.saturating_add(area.height - 1);
    // `isImageLine(screen[row])` (`:1631`).
    if image_rows >= area.height {
        return;
    }
    let text = label(ind);
    let clipped = truncate_to_width(&text, usize::from(area.width), "");
    let Ok(label_width) = u16::try_from(str_width(&clipped)) else {
        return;
    };
    let column = area
        .x
        .saturating_add(area.width.saturating_sub(label_width) / 2);
    // `scrollbarColumn ?? clip.x + clip.width` (`:1635`): a label never paints over the bar.
    let right_edge = scroll::geometry(bar, scroll, area)
        .map_or(area.x.saturating_add(area.width), |geometry| {
            geometry.column
        });
    let available = usize::from(right_edge.saturating_sub(column));
    let shown = truncate_to_width(&clipped, available, "");
    let Ok(width) = u16::try_from(str_width(&shown)) else {
        return;
    };
    if width == 0 {
        return;
    }
    let rect = Rect {
        x: column,
        y: row,
        width,
        height: 1,
    };
    // `theme.bg("selectedBg", theme.fg("text", label))` (`tui-renderer.ts:32`).
    let style: Style = theme.selected_bg_over(theme.base_style());
    frame.render_widget(Paragraph::new(Line::from(Span::styled(shown, style))), rect);
    ind.rect = Some(rect);
}

/// Whether `position` is on the painted label.
pub(super) fn contains(ind: &Indicator, position: Position) -> bool {
    ind.rect.is_some_and(|rect| rect.contains(position))
}

/// Forget the rectangle after a click has been acted on, so a second report before the next frame
/// cannot hit a label that is no longer there.
pub(super) fn consume(ind: &mut Indicator) {
    ind.rect = None;
}
