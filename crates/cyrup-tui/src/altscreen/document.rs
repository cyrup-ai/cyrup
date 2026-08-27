//! **The rendered document** — the bridge from ADR-0005 §B-1's retained [`crate::Entry`] list to the
//! two values [`super::AltScreen::set_document`] takes: every row the alternate screen paints, in
//! order, and the entry-index-to-first-row map §B-10's prompt walk indexes. ADR-0005 §Decision B-5's
//! per-frame cache half.
//!
//! # Upstream
//! Pi's alternate screen scrolls a component tree, so its document is whatever
//! `TuiBase.render(width)` produces: a walk of `this.children` that concatenates each child's rows
//! into one flat array, in child order (`packages/tui/src/tui.ts:235-245` @v0.84.3). `ScrollView`
//! hands that walk the *content* width — `this.child.render(this.getContentWidth(width))`
//! (`components/scroll-view.ts:207-211`) — i.e. the frame width less the scrollbar column, which is
//! exactly what [`super::scroll::content_width`] computes here.
//!
//! cyrup has no live component per committed message: a drained turn is an [`crate::Entry`] value in
//! [`crate::TranscriptView::document`] (§B-1), and the one function that turns an entry into rows is
//! [`crate::transcript::entry_lines`] — the same call the inline renderer flushes committed entries
//! to native scrollback with (`app/draw.rs:163-166`). So the walk below *is* upstream's walk with
//! `child.render(width)` spelled `entry_lines(entry, …, width, …)`, and there is deliberately no
//! second rendering path: a row the alternate screen paints and the row the inline flush would have
//! written for the same entry are produced by the same code at the same width.
//!
//! # Why the row map has no upstream counterpart
//! Upstream only ever has rendered *strings*, so it finds a prompt by scanning them for the OSC 133
//! prompt-start mark its message components emitted (`tui-alt-screen.ts:419`). cyrup retains
//! structured entries, so a prompt is an [`crate::Entry::User`] — identified by its variant, with no
//! text scan and no escape round trip (ADR-0005 §B-10 authorises exactly that substitution, and
//! [`super::prompt_nav`] records the whole of it). What the walk still needs is the row that entry
//! *starts* at, which only the pass that laid the rows out can know. `row_starts[i]` is that row for
//! `entries[i]`, and it is what [`super::scroll::row_of_entry`] looks into.
//!
//! Two properties of the map follow from its being parallel to `entries` rather than to the rows:
//! it always has exactly `entries.len()` elements, and an entry that renders to **no** rows — a
//! whitespace-only [`crate::Entry::Assistant`] returns `[]` from `entry_lines`, by the same trimmed
//! `hasVisibleContent` test upstream gates its component on (`assistant-message.ts:96-98`) — shares
//! the row of whatever follows it, or one past the last row when it is last. Nothing indexes with
//! it: [`super::scroll::row_of_entry`] is a `get`, and [`super::scroll::scroll_to_row`] clamps.
//!
//! # One [`Line`] is one display row
//! This is the invariant the whole §B-5 scroll model rests on, and it is upstream's too — its
//! document is a `string[]` of rows, and `ScrollView` slices it by index. Four consumers here read
//! the document that way and **none of them wrap**: [`super::AltScreen::draw`] paints
//! `doc.iter().skip(top).take(height)` into a `Paragraph` with no `.wrap()`, [`super::exit::repaint`]
//! writes `document[row]` per terminal line, [`super::selection`] addresses a caret as (row, column)
//! into it, and [`super::scroll`] takes `doc.len()` as the content height. A logical line wider than
//! the viewport would therefore be *truncated* by the painter, not reflowed — one row of content
//! lost per over-wide line, with the row map and the scrollbar quietly describing a document the
//! user cannot see the whole of.
//!
//! So every row leaves here already fitted to `width`, through
//! [`crate::transcript::wrap_line`] — the crate's single port of pi's `wrapSingleLine`
//! (`tui/src/utils.ts:857-936`), which is the *same* wrapper upstream's `Text`, `Box` and `Markdown`
//! run before a container ever sees their rows. Almost nothing reaches it: `entry_lines` already
//! wraps every arm it renders (markdown bodies at `markdown.ts:322`, `Text` bodies at `text.ts:67`),
//! so the gate below normally passes and the rows go through untouched. The one arm that can
//! genuinely arrive over-wide is [`crate::transcript::Rendered::Text`], the extension-supplied custom render
//! emitted verbatim because the renderer owns its own presentation (X11).
//!
//! The gate is [`crate::transcript::wrapped_height`], the measure the inline path sizes its
//! `insert_before` slot with (`app/draw.rs:177`): when an entry's display height already equals its
//! logical line count, the entry is one-row-per-line and needs no reflow. Using the inline path's
//! own measure is the point — it is what keeps the two renderers agreeing on how tall a given entry
//! is, rather than each deciding for itself.
//!
//! # Rule 2
//! Per `altscreen/mod.rs` rule 2 nothing here is owned: the entries, the theme, the padding and the
//! image options all arrive as arguments from the [`crate::AppState`] that already holds them for
//! the inline path, and the two vectors go straight back out to the caller.

use std::hash::{DefaultHasher, Hash, Hasher};

use ratatui::text::Line;

use crate::theme::UiTheme;
use crate::transcript::{entry_lines, wrap_line, wrapped_height, Entry, ImageOpts, TranscriptView};

/// Render the whole retained document at `width` — pi's `TuiBase.render(width)` (`tui.ts:235-245`),
/// with `child.render(width)` spelled [`crate::transcript::entry_lines`].
///
/// Returns `(rows, row_starts)`, the pair [`super::AltScreen::set_document`] takes: every display
/// row of the document in commit order, and `row_starts[i]` = the row `entries[i]` begins at. See
/// the module doc for why one [`Line`] is exactly one display row, and for the two properties of the
/// map that follow from its being parallel to `entries`.
///
/// `width` is the **content** width — the frame width less the scrollbar column
/// ([`super::scroll::content_width`], upstream's `ScrollView.getContentWidth`,
/// `components/scroll-view.ts:207-211`) — and `output_pad` and `images` are the same
/// [`crate::TranscriptView::output_pad`] and transcript-derived [`ImageOpts`] the inline flush
/// builds (`app/draw.rs:133-163`). Building them from anywhere else is what would let a row on the
/// alternate screen disagree with the row the same entry would have flushed inline.
///
/// # Cost, and when to cache it
/// This re-renders **every retained entry** — up to [`crate::transcript::MAX_RETAINED_ENTRIES`] of
/// them, each through markdown, syntax highlighting and image rasterisation. It is not a per-frame
/// call. The document changes only when an entry commits (a
/// [`crate::TranscriptView::drain_committed`] with retention on), when the front is trimmed
/// (`retained_dropped` moves), or when one of the *paint-time* inputs above changes — the width, the
/// theme, the output padding, `Ctrl+O`'s expansion flag, the image settings, the `app.tools.expand`
/// label, the hidden-thinking label or the session cwd. [`document_key`] is exactly that set as one
/// comparable value: hold the last key beside the last `(rows, row_starts)`, rebuild only when the
/// key changed, and hand the cached pair to `set_document` otherwise.
///
/// A streaming turn is **not** in this set. The live region is the transcript's own
/// [`crate::TranscriptView::content_height`] cache, and the spinner and elapsed-footer ticks that
/// invalidate it ([`crate::TranscriptView::bump_render_tick`]) touch nothing committed — which is
/// why [`document_key`] is built from the public accessors below rather than from the transcript's
/// internal render generation, whose tick bumps would rebuild the whole document once a frame for
/// the duration of every turn.
pub(super) fn render_document(
    entries: &[Entry],
    theme: &UiTheme,
    width: usize,
    output_pad: usize,
    images: ImageOpts<'_>,
) -> (Vec<Line<'static>>, Vec<usize>) {
    let mut rows: Vec<Line<'static>> = Vec::new();
    let mut row_starts: Vec<usize> = Vec::with_capacity(entries.len());
    for entry in entries {
        // Pushed BEFORE the entry renders, so the map records where the entry begins even when it
        // contributes nothing — see the module doc on zero-row entries.
        row_starts.push(rows.len());
        let lines = entry_lines(entry, theme, width, output_pad, images);
        // The common case by a wide margin: `entry_lines` wrapped every row it produced, so its
        // display height already equals its line count and the rows move across untouched.
        if wrapped_height(&lines, width) == lines.len() {
            rows.extend(lines);
            continue;
        }
        for line in &lines {
            rows.extend(wrap_line(line, width));
        }
    }
    (rows, row_starts)
}

/// Everything [`render_document`]'s output depends on, as one comparable value — the caller's test
/// for "is the cached document still the document this frame would produce?".
///
/// Deliberately **not** `Default`: a caller holds `Option<DocumentKey>` and treats `None` as "never
/// built", so an empty document at width 0 cannot be mistaken for a build that has already happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DocumentKey {
    /// [`crate::TranscriptView::document`]'s length. Paired with `dropped` below this pins the
    /// entry *window* exactly: the retained document is append-only at the back and trimmed only at
    /// the front (§B-1), so two builds agreeing on both agree on every entry in between.
    entries: usize,
    /// [`crate::TranscriptView::retained_dropped`]. Without it a trim that evicted as many entries
    /// as the same frame committed would leave `entries` unmoved over an entirely different window.
    dropped: u64,
    /// The content width [`render_document`] wrapped to. A resize rebuilds, because the rows are
    /// fitted to it and `doc.len()` — the scroll model's content height — is a function of it.
    width: usize,
    /// [`crate::TranscriptView::output_pad`].
    output_pad: usize,
    /// [`UiTheme::generation`], the counter the transcript's own render cache keys on for the same
    /// reason (`transcript/cache.rs:30`).
    theme: u64,
    /// [`crate::TranscriptView::image_width_cells`].
    image_width_cells: u16,
    /// [`crate::TranscriptView::show_images`].
    show_images: bool,
    /// [`crate::TranscriptView::graphical_images`].
    graphical_images: bool,
    /// [`crate::TranscriptView::tool_expanded`] — X14's live `toolOutputExpanded`, which every
    /// committed tool, branch summary and compaction summary renders at.
    tools_expanded: bool,
    /// The three string-shaped paint-time inputs hashed together: the `app.tools.expand` label, the
    /// hidden-thinking label and the session cwd. Hashed rather than cloned so the key stays `Copy`
    /// and free to build every frame; all three change only on a rebind, an extension call or a
    /// session-directory change, and a hash collision costs one stale label, never a wrong row.
    labels: u64,
}

/// Build a [`DocumentKey`] from the transcript the document is rendered from.
///
/// Reads the *same* accessors the caller builds its [`ImageOpts`] from (`app/draw.rs:143-163`), so
/// the key cannot drift from the inputs it is meant to cover — which is why it takes the transcript
/// rather than the already-built options bag.
pub(super) fn document_key(
    transcript: &TranscriptView,
    theme: &UiTheme,
    width: usize,
) -> DocumentKey {
    let mut hasher = DefaultHasher::new();
    transcript.expand_key().hash(&mut hasher);
    transcript.hidden_thinking_label().hash(&mut hasher);
    transcript.cwd().hash(&mut hasher);
    DocumentKey {
        entries: transcript.document().len(),
        dropped: transcript.retained_dropped(),
        width,
        output_pad: transcript.output_pad(),
        theme: theme.generation,
        image_width_cells: transcript.image_width_cells(),
        show_images: transcript.show_images(),
        graphical_images: transcript.graphical_images(),
        tools_expanded: transcript.tool_expanded(),
        labels: hasher.finish(),
    }
}

/// How many *rows* a front trim removed, for the offset compensation
/// [`super::scroll::rebuild_rows`] applies — the one number a rebuilt document cannot derive from
/// itself.
///
/// `previous` is the `row_starts` of the build being replaced, `previous_rows` is how many rows that
/// build produced, and `entries_dropped` is how far [`crate::TranscriptView::retained_dropped`]
/// moved since it. The evicted entries were the first `entries_dropped` of that map, so the rows
/// they occupied are the rows below its `entries_dropped`-th start — `0` when nothing was dropped,
/// and `previous_rows` (the whole previous document) when the drop consumed all of it, which is what
/// a [`crate::TranscriptView::clear_document`] does.
///
/// A caller that keeps the previous map beside the cached rows (which [`document_key`] already
/// obliges it to) therefore has this for free. Deriving it from the *new* map instead cannot work:
/// `row_starts[0]` of a freshly built document is `0` by construction, whatever was trimmed.
pub(super) fn rows_dropped(previous: &[usize], entries_dropped: u64, previous_rows: usize) -> usize {
    let dropped = usize::try_from(entries_dropped).unwrap_or(usize::MAX);
    match previous.get(dropped) {
        Some(row) => *row,
        // Every entry the previous build knew about is gone, so every row it produced is.
        None => previous_rows,
    }
}
