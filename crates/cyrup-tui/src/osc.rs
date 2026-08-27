//! OSC-8 hyperlink emission — pi's `hyperlink(text, url)`, gated on `getCapabilities().hyperlinks`
//! (`terminal-image.ts:130-143`). The module [`crate::app::AppState::capabilities`]'s doc comment
//! already names (`app/state.rs`, *"The `hyperlinks` flag gates OSC-8 emission in rendered links
//! (`osc::hyperlink`)"*). TUI-020's first landing.
//!
//! ## Why the escape is not in the `Span`
//!
//! `Span::styled_graphemes` filters `char::is_control` (ratatui-core `text/span.rs:311-317`), so an
//! `ESC` in span text is deleted and `]8;;…` lands in the transcript as visible garbage; and
//! `Span::width` would count those bytes as columns, corrupting `wrap_line`, `wrapped_height` and
//! the content-sized inline viewport. The escape therefore goes into the [`Buffer`] **cell**, which
//! `CrosstermBackend::draw` prints verbatim (`Print(cell.symbol())`), with
//! [`CellDiffOption::ForcedWidth`] restoring the true column count for the diff — the mechanism
//! `ratatui-image` uses for Kitty/iTerm2/sixel (`ratatui-image-11.0.6/src/protocol.rs:31`).
//!
//! `ForcedWidth` is mandatory rather than cosmetic here: this crate builds with the
//! `scrolling-regions` default feature, so `Terminal::insert_before` routes through
//! `draw_lines_over_cleared` → `Buffer::diff_iter`, and `diff_iter` advances its cursor by
//! `cell_width()` (`ratatui-core-0.1.2/src/buffer/diff.rs:132-142`). Without the override a cell
//! holding `\x1b]8;;file:///home/u/x.rs\x07~` measures ~25 columns and the iterator would skip 24
//! real cells. `CellDiffOption::Skip` must NOT be used — `diff_iter` drops skipped cells entirely
//! (`diff.rs:128-129`), which would delete the escape.
//!
//! ## How the renderer says "these cells are a link"
//!
//! `Modifier` is `bitflags!` over a `u16` with nine defined bits (`ratatui-core/src/style.rs`), so
//! bits 9..=15 are unallocated. A link is stamped into them as a 1..=[`MAX_ID`] id; the crossterm
//! backend's `ModifierDiff::queue` tests only the known flags and emits nothing for an unknown bit,
//! `Cell::set_style` carries them in through `self.modifier.insert(style.add_modifier)`, and
//! [`inject`] clears them before the frame is diffed. That is a clean, invisible, style-preserving
//! side channel from `Span` to `Cell` through `Paragraph`'s wrapper — the one thing the escape
//! itself cannot do.
//!
//! ## Why an id is a *pass-unique* index and not a cyclic one
//!
//! A marked run is NOT guaranteed to be contiguous in the buffer. `transcript::layout::box_lines`
//! wraps a tool header at `width - 2`, left-pads every produced row and then right-pads it to
//! `width`, so a path long enough to wrap arrives as two marked pieces with unmarked padding
//! between them. [`inject`] therefore has to resolve *every* run of an id to the same href, which
//! means an id must identify a link outright rather than the n-th link in a cycle. Ids are handed
//! out `1..=MAX_ID` in registration order and the table is capped: the 128th and later links in a
//! single pass simply go unmarked and render exactly as they do today. One paint of one turn (or
//! one `insert_before` flush) never approaches 127 linked tool headers, and the overflow behaviour
//! is the pre-existing plain-text one, not a wrong href.
//!
//! A wrapped link consequently emits one open/close pair per row rather than a single pair spanning
//! the wrap. Both forms are ordinary OSC-8 and both are clickable; the per-row form is the more
//! conservative of the two, because it does not depend on the terminal keeping hyperlink state
//! across the intervening `MoveTo`.

use ratatui::buffer::{Buffer, CellDiffOption};
use ratatui::style::{Modifier, Style};
use std::cell::RefCell;
use std::num::NonZeroU16;

/// Bits 9..=15 of `Modifier`, the seven the enum leaves unallocated.
const LINK_MASK: u16 = 0b1111_1110_0000_0000;
const LINK_SHIFT: u32 = 9;
/// The largest id the seven-bit channel carries, and therefore the number of links one render pass
/// can mark.
const MAX_ID: u16 = 127;

/// A future ratatui that allocates bit 9 would silently turn a link marker into a real attribute.
/// This is the cheapest defence: a build break rather than a visual one.
const _: () = assert!(Modifier::all().bits() & LINK_MASK == 0);

/// One cell wide regardless of how many bytes of escape the symbol carries.
const UNIT_WIDTH: CellDiffOption = match NonZeroU16::new(1) {
    Some(w) => CellDiffOption::ForcedWidth(w),
    None => CellDiffOption::None,
};

/// The hrefs registered during one render pass, in assignment order. Held behind a `RefCell` so it
/// can ride on the `Copy` per-paint bag ([`crate::transcript::ImageOpts`]) instead of threading an
/// `&mut` through every tool renderer.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct LinkSink {
    urls: RefCell<Vec<String>>,
}

impl LinkSink {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Register `url` and return the [`Style`] marker that tags its cells, or a neutral `Style` once
    /// this pass has spent all [`MAX_ID`] ids (the span then renders unlinked — today's behaviour).
    pub(crate) fn mark(&self, url: String) -> Style {
        let mut urls = self.urls.borrow_mut();
        if urls.len() >= MAX_ID as usize {
            return Style::default();
        }
        urls.push(url);
        // 1..=MAX_ID: `0` is "no link", so the table is one-based.
        let id = urls.len().min(MAX_ID as usize) as u16;
        Style::default().add_modifier(Modifier::from_bits_retain(id << LINK_SHIFT))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.urls.borrow().is_empty()
    }

    /// The href an id stands for — the inverse of [`Self::mark`]'s one-based assignment.
    fn url_for(&self, id: u16) -> Option<String> {
        let idx = usize::from(id).checked_sub(1)?;
        self.urls.borrow().get(idx).cloned()
    }
}

/// `\x1b]8;;<url>\x07` — OSC-8 open, BEL-terminated (the form pi emits and the form
/// [`crate::ansi::strip_ansi`] recognises).
fn open(url: &str) -> String {
    format!("\u{1b}]8;;{url}\u{7}")
}

/// `\x1b]8;;\x07` — OSC-8 close.
const CLOSE: &str = "\u{1b}]8;;\u{7}";

/// Read the link id a cell carries, or `None`.
fn id_of(modifier: Modifier) -> Option<u16> {
    let id = (modifier.bits() & LINK_MASK) >> LINK_SHIFT;
    (id != 0).then_some(id)
}

/// Wrap every marked run of cells in `buf` in its OSC-8 escape, and strip the marker bits.
///
/// Must run **after** the widget that produced the cells: the escape must not be present while
/// `Paragraph` measures columns, and the marked cells do not exist until it has written them.
///
/// A no-op when `sink` is empty, so the whole path costs one `is_empty()` on a hyperlink-incapable
/// terminal.
pub(crate) fn inject(buf: &mut Buffer, sink: &LinkSink) {
    if sink.is_empty() {
        return;
    }
    let mut i = 0usize;
    while i < buf.content.len() {
        let Some(id) = buf.content.get(i).and_then(|c| id_of(c.modifier)) else {
            i += 1;
            continue;
        };
        let start = i;
        while buf
            .content
            .get(i)
            .and_then(|c| id_of(c.modifier))
            .is_some_and(|next| next == id)
        {
            i += 1;
        }
        let end = i - 1;
        let url = sink.url_for(id);
        // Clear the marker before anything can observe it, run resolved or not.
        for cell in buf.content.get_mut(start..=end).into_iter().flatten() {
            cell.modifier = Modifier::from_bits_retain(cell.modifier.bits() & !LINK_MASK);
        }
        let Some(url) = url else { continue };
        // Head first, then tail — a one-cell run is `start == end`, and doing it in this order
        // leaves that single cell holding `open + symbol + CLOSE`.
        if let Some(cell) = buf.content.get_mut(start) {
            let symbol = format!("{}{}", open(&url), cell.symbol());
            cell.set_symbol(&symbol).set_diff_option(UNIT_WIDTH);
        }
        if let Some(cell) = buf.content.get_mut(end) {
            let symbol = format!("{}{CLOSE}", cell.symbol());
            cell.set_symbol(&symbol).set_diff_option(UNIT_WIDTH);
        }
    }
}
