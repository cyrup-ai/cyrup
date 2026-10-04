//! The screen's regions for one frame, as ratatui rectangles — the single answer both renderers and
//! the pointer layer read.
//!
//! Pi lays its chat out as a flex column (`chat-viewport.ts`: a growing transcript over a shrinking
//! dock). ratatui's own idiom for the same thing is a [`Layout`] over a constraint list, so that is
//! what this is: [`region_constraints`] decides how many rows each dock region gets (it already
//! encodes pi's shrink priorities and floors), a vertical [`Layout`] turns the counts into
//! [`Rect`]s, and the message region takes whatever is left.
//!
//! Computing the rectangles once, in one place, matters beyond painting. A click is resolved against
//! the rectangles the LAST frame was painted with ([`AppState::regions`]), so the painter and the
//! hit test cannot disagree about where the editor, the popup or a selector row is.

use ratatui::layout::{Constraint, Layout, Rect};

use super::{AppState, region_constraints};

/// Where each part of the screen sits this frame. A region with no rows has zero height.
///
/// Field order is top to bottom, which is also the order [`region_constraints`] returns its counts
/// in and the order the inline renderer has always painted them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Regions {
    /// The extension header (`setHeader`), docked above the message region.
    pub(crate) header: Rect,
    /// The message region. Inline, the active turn's rows (committed rows live in native
    /// scrollback); fullscreen, the scrolled document's viewport.
    pub(crate) msg: Rect,
    /// Queued steering / follow-up messages.
    pub(crate) pending: Rect,
    /// The working indicator band.
    pub(crate) band: Rect,
    /// The pending-attachment image strip.
    pub(crate) images: Rect,
    /// Extension widgets above the editor.
    pub(crate) widgets_above: Rect,
    /// The editor slot — the editor, or the selector or loader that replaced it.
    pub(crate) slot: Rect,
    /// The editor's autocomplete popup, directly under the slot.
    pub(crate) popup: Rect,
    /// Extension widgets below the editor.
    pub(crate) widgets_below: Rect,
    /// The footer (or the extension footer that replaced it).
    pub(crate) footer: Rect,
}

impl Regions {
    /// Lay `area` out for the state as it stands. Takes `&mut AppState` because sizing the slot asks
    /// the editor to measure its wrapped line count ([`region_constraints`]).
    pub(crate) fn compute(state: &mut AppState, area: Rect) -> Self {
        let [
            header_h,
            _msg_h,
            pending_h,
            band_h,
            images_h,
            wabove_h,
            slot_h,
            popup_h,
            wbelow_h,
            footer_h,
        ] = region_constraints(state, area.width, area.height);
        let [
            header,
            msg,
            pending,
            band,
            images,
            widgets_above,
            slot,
            popup,
            widgets_below,
            footer,
        ] = Layout::vertical([
            // TUI-033 — `headerContainer` is docked above `chatContainer` (`interactive-mode.ts:709`).
            Constraint::Length(header_h),
            // `Min(0)` (not `Min(1)`): the empty turn must not balloon the inline viewport, and in
            // fullscreen this region is exactly what the dock leaves over.
            Constraint::Min(0),
            Constraint::Length(pending_h),
            Constraint::Length(band_h),
            Constraint::Length(images_h),
            // TUI-014 — `widgetContainerAbove`, immediately before `editorContainer` (`:715-716`).
            Constraint::Length(wabove_h),
            Constraint::Length(slot_h),
            Constraint::Length(popup_h),
            // TUI-014 — `widgetContainerBelow`, immediately after `editorContainer` (`:717`).
            Constraint::Length(wbelow_h),
            Constraint::Length(footer_h),
        ])
        .areas(area);
        Self {
            header,
            msg,
            pending,
            band,
            images,
            widgets_above,
            slot,
            popup,
            widgets_below,
            footer,
        }
    }
}
