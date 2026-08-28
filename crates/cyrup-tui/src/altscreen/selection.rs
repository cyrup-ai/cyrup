//! **Application-owned text selection**, and the three things a user does with one — copy, link
//! activation and paste. cyrup's port of the pointer half of pi's `TuiAltScreen`
//! (`packages/tui/src/tui-alt-screen.ts` @v0.84.3: `:700-716`, `:797-834`, `:836-898`, `:900-1047`,
//! `:1049-1112`, `:1192-1256`, and the `FOCUS_OUT` arm at `:543-559`). ADR-0005 §Decision B-8. The
//! ADR's `:514-524` / `:605-963` citations are the @v0.84.1 line numbering for the same spans.
//!
//! # Why a renderer owes the user this at all
//! Capturing the mouse takes the terminal's own selection away — the moment ADR-0005 §B-4 writes
//! `?1000h ?1002h` the emulator stops drawing a highlight and starts forwarding reports instead. A
//! renderer that captures therefore owes the user a replacement, and ratatui provides none of it:
//! [`ratatui::buffer::Buffer`] has no notion of a selected range, no hit test and no clipboard.
//! Everything below is that replacement, and nothing else. Without it, turning fullscreen on is a
//! net regression for anyone who copies text out of their terminal.
//!
//! # Selection is view state
//! Nothing here mutates a [`crate::Entry`]. The anchor and focus are *rendered document* rows and
//! visible columns over the same `&[Line]` the frame painted, which is what makes a selection
//! survive a re-render of the entries beneath it and what makes [`highlight`] a pure post-pass over
//! cells that are already on the screen. It is also why the copied string matches the visible text
//! across wrapped rows for free: a wrapped row IS a document row here, exactly as it is upstream,
//! where the selection runs over `box.scrollContentLines` (`:1094-1097`).
//!
//! # The parameters ADR-0005 §B-3 will eventually bundle
//! The renderer's `AltUi` bag does not exist yet, so the state a pointer gesture touches arrives as
//! arguments rather than as one `&mut AltUi` — the same shape [`super::scrollbar_drag`] uses, and
//! the same destructure §B-3 performs at its call site: this module's own [`SelectionState`], the
//! [`super::scroll::ScrollState`] the drag scrolls, the rendered document (§B-5's per-frame cache,
//! `&[Line]` here so this module never touches a [`crate::TranscriptView`] — `altscreen/mod.rs`,
//! rule 2) and the viewport [`Rect`] the document was painted into.
//!
//! # What this module does NOT do, and who does
//! [`route`] answers a [`PointerOutcome`], because two of the four things a gesture can end in are
//! not this module's to perform:
//!
//! * **Copy is async.** The crate's text-clipboard write is [`crate::clipboard::copy_to_clipboard`]
//!   — an `async fn` with a four-branch CLI/OSC-52 plan behind it (`clipboard.rs:212`) — and this
//!   module is called from the render path. So [`PointerOutcome::Copy`] carries the exact string
//!   upstream would have written (`:1087-1112`) and the `await`, together with the `Copied!` /
//!   `Copy failed` flash pi raises at `:1114-1115`, belongs to the dispatch tier (ADR-0005 §B-11).
//!   [`selected_text`] is the same string on demand, for `/copy` with a live selection.
//! * **Paste needs the editor.** [`PointerOutcome::Paste`] carries clipboard text read through the
//!   crate's existing synchronous [`crate::clipboard::read_clipboard_text`] (`clipboard.rs:334`);
//!   inserting it is [`crate::AppState`]'s, which this module may not hold (rule 2).
//!
//! The other two it performs itself, because both are synchronous and neither touches application
//! state: the offset moves through [`super::scroll`], and a link opens through
//! [`crate::open_browser::open_browser`] — which spawns detached and returns at once, so it sits
//! where pi's `this.openUrl(clickedUrl)` sits (`:1005`) rather than in an outcome.
//!
//! # No new clipboard dependency
//! `arboard` is already this crate's clipboard backend and is reached only through
//! [`crate::clipboard`], which is where the Wayland/`wl-paste` branch and the OSC-52 fallback live.
//! Nothing here talks to `arboard`, or to any clipboard, directly.
//!
//! # Deadlines, not timers
//! Upstream auto-scrolls a drag past the viewport edge from a 50 ms `setInterval`
//! (`:949-951`). cyrup has no per-component scheduler, so — exactly as [`super::scroll`] does for
//! the transient scrollbar and [`super::flash`] for a notice — the interval becomes a deadline:
//! [`next_auto_scroll`] is what the alternate-screen loop arms its next wake on and
//! [`tick_auto_scroll`] is the callback body (`:954-970`). Without it a drag held motionless
//! against the edge would stop extending, which is the acceptance criterion the timer exists for.

use std::time::{Duration, Instant};

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::Frame;
use unicode_segmentation::UnicodeSegmentation;

use super::scroll::{self, ScrollState};
use crate::text_width::str_width;
use crate::transcript::is_ws_grapheme;

/// How close together two presses must fall to advance the click count — pi's
/// `DOUBLE_CLICK_INTERVAL_MS = 500` (`tui-alt-screen.ts:68`), applied at `:906`.
pub(super) const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

/// How often a drag held beyond the viewport edge moves the document — the period of upstream's
/// `setInterval(() => this.autoScrollSelection(), 50)` (`tui-alt-screen.ts:950`), read here as a
/// deadline instead of a timer (see the module doc).
pub(super) const AUTO_SCROLL_INTERVAL: Duration = Duration::from_millis(50);

/// The two punctuation graphemes that keep a word selection whole across them — pi's
/// `TERMINAL_WORD_SELECTION_JOINERS = new Set(["/", "-"])` (`tui-alt-screen.ts:71`).
///
/// Upstream's comment is the rationale and it is worth keeping: regular mode delegates
/// double-click selection to the terminal emulator, so fullscreen — which owns the mouse — mirrors
/// what a terminal does and keeps paths and kebab-case tokens whole (`:69-70`).
const WORD_JOINERS: [&str; 2] = ["/", "-"];

/// The schemes a clicked run of text must open with to be treated as a link.
///
/// `[CYRUP-DELTA]`, and the one place this unit cannot be mechanical — see [`link_at`].
const LINK_SCHEMES: [&str; 4] = ["https://", "http://", "file://", "mailto:"];

/// One end of a selection — pi's `SelectionPoint` (`tui-alt-screen.ts:80-86`).
///
/// Upstream's `scrollView` field names *which* view the point belongs to, because its layout can
/// hold several; cyrup's alternate screen has exactly one (`interactive-mode.ts:918-923`), so the
/// field would name the only candidate and is dropped — the same reduction
/// [`super::scrollbar_drag::DragState`] makes for the same reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Point {
    /// Row of the **rendered document**, not of the screen — upstream's `row` (`:81`), which is
    /// likewise a `scrollContentLines` index (`:807`).
    row: usize,
    /// Visible column within the viewport's content — upstream's `col` (`:82`).
    col: usize,
    /// Whether the point lies *between* cells rather than on one — upstream's `boundary` (`:85`).
    /// Set on the end of a word or line selection, where the range stops before the column it
    /// names instead of including it; read by [`selection_columns`] and nothing else (`:1074`).
    boundary: bool,
}

/// An ordered pair of points — pi's `SelectionRange` (`tui-alt-screen.ts:88-91`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Range {
    start: Point,
    end: Point,
}

/// What a press selects — pi's
/// `type SelectionGranularity = "character" | "word" | "line"` (`tui-alt-screen.ts:93`), chosen by
/// click count at `:1035`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Granularity {
    /// One cell per drag step — a single click (upstream's `"character"`).
    #[default]
    Character,
    /// Whole words, extended word-wise while the drag continues — a double click.
    Word,
    /// Whole rows — a triple click.
    Line,
}

/// The previous press, for the click-count ladder — pi's `ClickTarget`
/// (`tui-alt-screen.ts:95-102`), minus its `scrollView` (see [`Point`]).
#[derive(Clone, Copy, Debug)]
struct LastClick {
    /// Upstream's `timestamp` (`:96`) as a monotonic [`Instant`]: `Date.now()` is a wall clock and
    /// a clock step must not turn a double click into a single one.
    at: Instant,
    /// How many presses deep the ladder is — upstream's `count` (`:97`), cycled `1 → 2 → 3 → 1`
    /// by `(previous.count % 3) + 1` (`:911`).
    count: u8,
    /// The row the press landed on (`:98`).
    row: usize,
    /// The word range that press resolved, which is what identifies "the same target" across two
    /// presses — upstream's `wordStart`/`wordEnd` (`:100-101`).
    word_start: usize,
    word_end: usize,
}

/// A drag holding the pointer at or past a viewport edge — pi's
/// `selectionAutoScrollDirection` / `selectionDragPointer` / `selectionAutoScrollTimer` triple
/// (`tui-alt-screen.ts:188-190`) as one optional record, since upstream's three fields are always
/// armed and disarmed together (`:944-951`, `:972-979`).
#[derive(Clone, Copy, Debug)]
struct AutoScroll {
    /// `-1` up, `1` down — upstream's `selectionAutoScrollDirection` (`:189`), which is also the
    /// row count each tick moves (`:960`).
    direction: i32,
    /// Where the pointer was left, so a tick that scrolls can re-resolve the focus point under it
    /// without a new report — upstream's `selectionDragPointer` (`:188`, read at `:967`).
    pointer: Position,
    /// When the next tick is due — the deadline standing in for upstream's interval (see the
    /// module doc).
    due: Instant,
}

/// The live selection — pi's nine `selection*` fields (`tui-alt-screen.ts:185-196`) in one bag.
///
/// Lives in the alternate-screen renderer's UI bag (ADR-0005 §B-3) and holds no application state:
/// no transcript, no theme, no keymap. The document it describes arrives as an argument on every
/// entry point, which is what lets a selection outlive any single frame without borrowing one.
#[derive(Default)]
pub(super) struct SelectionState {
    /// Where the gesture began — upstream's `selectionAnchor` (`:185`). Not necessarily the
    /// earlier end: a backwards drag leaves the anchor after the focus, which is what
    /// [`bounds`] orders.
    anchor: Option<Point>,
    /// Where the pointer is now — upstream's `selectionFocus` (`:186`).
    focus: Option<Point>,
    /// Upstream's `selectionGranularity` (`:187`).
    granularity: Granularity,
    /// The word or line range the press itself resolved — upstream's `selectionInitialRange`
    /// (`:188`). A word- or line-granularity drag pivots around this rather than around a bare
    /// cell, so dragging back past the origin re-selects whole words on the other side (`:885-897`).
    initial: Option<Range>,
    /// Upstream's `lastClick` (`:191`).
    last_click: Option<LastClick>,
    /// Upstream's auto-scroll triple (`:188-190`); see [`AutoScroll`].
    auto: Option<AutoScroll>,
    /// Whether a press is being held — upstream's `selectionPressActive` (`:191`). A drag or a
    /// release with this false belongs to no gesture of ours and is declined.
    press_active: bool,
    /// Whether this press has moved at all — upstream's `selectionDragged` (`:196`). A link
    /// activates on a click, never at the end of a drag (`:996`).
    dragged: bool,
    /// The link under the press, captured at press time so a release can tell a click on a link
    /// from a click that merely ended near one — upstream's `pressedUrl` (`:195`, set at `:1040`).
    pressed_url: Option<String>,
}

/// What [`route`] did with one mouse report, and what the caller still owes.
///
/// Upstream needs no equivalent: `handleSelectionMouseEvent` returns `void` and its dispatcher
/// consumes every parsed mouse report unconditionally (`tui-alt-screen.ts:571-576`). cyrup reports
/// consumption instead, for the reason [`super::keys::KeyOutcome`] does — an unconsumed report must
/// be free to reach [`crate::App::handle_input`], because there is exactly one editor and one
/// overlay stack in either mode — and carries the two effects this module cannot perform itself
/// (see the module doc).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerOutcome {
    /// Not a selection gesture: the caller offers the report onward. A wheel notch, a middle
    /// press, a motion report with no press behind it and a press outside the document viewport
    /// all land here — the last being upstream's screen-coordinate fallback (`:822-826`), which
    /// cyrup declines instead so the editor and status chrome keep their own behaviour.
    Ignored,
    /// The report was ours and is fully dealt with.
    Handled,
    /// The report was ours and ended a selection: the caller writes this string with
    /// [`crate::clipboard::copy_to_clipboard`] and flashes `Copied!` or `Copy failed` on the
    /// result — pi's `copySelectionToClipboard` tail (`:1113-1117`).
    Copy(String),
    /// An unmodified secondary-button press with clipboard text behind it: the caller inserts the
    /// string into the editor — pi's `onRightClickPaste()` (`:711`).
    Paste(String),
}

/// Offer one decoded mouse report to the selection — pi's `handleSelectionMouseEvent`
/// (`tui-alt-screen.ts:981-1047`) with the right-click paste its dispatcher checks first
/// (`:572`, `:700-716`) folded in.
///
/// `viewport` is the [`Rect`] the rendered document was painted into and `doc` is what was painted
/// into it, so the rows this hit-tests are the rows the user can see.
///
/// # Precedence this sits inside
/// ADR-0005 §B-3's dispatcher offers a report to [`super::wheel::route`] first and
/// [`super::scrollbar_drag::route`] second, which is upstream's order and not a preference
/// (`:565-575`); a `true` from the scrollbar means the pointer belongs to the thumb, so [`cancel`]
/// runs and the report never arrives here. Upstream's own right-click check runs ahead of the
/// scrollbar rather than here, which is immaterial: the scrollbar declines every non-left press
/// (`altscreen/scrollbar_drag.rs`), so the report reaches this function either way.
///
/// # The button gate
/// Upstream's is `const button = event.button & 3; if (button !== 0 && !(event.release && button
/// === 3)) return;` (`:982-983`) — the left button, plus a release carrying X10's "unknown button"
/// code. crossterm decodes that same code as `Up(MouseButton::Left)` (`parse_cb`, button 3
/// undragged), so the three arms below are exactly upstream's gate, and a right or middle press,
/// a bare `Moved` report and every wheel kind fall through to [`PointerOutcome::Ignored`].
///
/// # It reads the offset and never moves it
/// `scroll` is shared, not exclusive, and that is upstream's shape rather than an oversight: a drag
/// that reaches the viewport edge *arms* the auto-scroll (`:926-952`) and the interval is what
/// actually moves the document one row at a time (`:960`). [`tick_auto_scroll`] is that interval,
/// and it is the only function here that takes the offset by `&mut`.
pub(super) fn route(
    sel: &mut SelectionState,
    scroll: &ScrollState,
    doc: &[Line<'_>],
    viewport: Rect,
    ev: &MouseEvent,
) -> PointerOutcome {
    if right_click_paste_applies(ev) {
        // `try { this.onRightClickPaste(); } catch {}` then `return true` (`:709-715`): consumed
        // whether or not the clipboard had anything, because the gesture was claimed either way.
        return match crate::clipboard::read_clipboard_text() {
            Some(text) => PointerOutcome::Paste(text),
            None => PointerOutcome::Handled,
        };
    }
    match ev.kind {
        MouseEventKind::Down(MouseButton::Left) => press(sel, scroll, doc, viewport, ev),
        MouseEventKind::Drag(MouseButton::Left) => drag(sel, scroll, doc, viewport, ev),
        MouseEventKind::Up(MouseButton::Left) => release(sel, scroll, doc, viewport, ev),
        _ => PointerOutcome::Ignored,
    }
}

/// When the next auto-scroll tick is due, or `None` while no drag holds an edge.
///
/// The alternate-screen loop schedules its next wake on this — the deadline half of upstream's
/// interval (`tui-alt-screen.ts:949-951`); see the module doc. It is the same contract
/// [`super::flash::next_expiry`] and [`super::scroll::next_hide`] answer.
pub(super) fn next_auto_scroll(sel: &SelectionState) -> Option<Instant> {
    sel.auto.map(|auto| auto.due)
}

/// Run one auto-scroll step if one is due, returning whether the view moved — pi's
/// `autoScrollSelection` (`tui-alt-screen.ts:954-970`), whose `requestRender` (`:969`) is this
/// return value.
///
/// Calling it every tick is free: with no drag against an edge, or before the deadline, it is two
/// comparisons and a `false`.
///
/// The stop condition is upstream's and is the reason the remainder exists at all: when
/// [`super::scroll::scroll_by_remaining`] gives the whole request back the document is already at
/// that end, so the interval disarms rather than spinning (`:961-964`). Otherwise the focus is
/// re-resolved under the *stored* pointer, which is what keeps the selection extending while the
/// pointer itself sends no further reports.
pub(super) fn tick_auto_scroll(
    sel: &mut SelectionState,
    scroll: &mut ScrollState,
    doc: &[Line<'_>],
    viewport: Rect,
) -> bool {
    let Some(auto) = sel.auto else {
        return false;
    };
    if Instant::now() < auto.due {
        return false;
    }
    // `const remaining = scrollView.scrollBy(direction); if (remaining === direction) …` (`:960-964`).
    if scroll::scroll_by_remaining(scroll, auto.direction) == auto.direction {
        stop_auto_scroll(sel);
        return false;
    }
    if let Some(point) = point_at(scroll, doc, viewport, auto.pointer.x, auto.pointer.y) {
        update_focus(sel, doc, point);
    }
    let now = Instant::now();
    if let Some(next) = sel.auto.as_mut() {
        next.due = now.checked_add(AUTO_SCROLL_INTERVAL).unwrap_or(now);
    }
    true
}

/// Drop the selection and everything armed with it — pi's unconditional clear
/// (`tui-alt-screen.ts:776-784`).
///
/// Upstream runs it when a scrollbar grab starts, which is why [`super::scrollbar_drag`]'s module
/// doc names this function: a `true` from that unit means the pointer belongs to the thumb, so the
/// in-flight selection ends rather than being extended by a gesture that is no longer over the
/// document. Idempotent, so ADR-0005 §B-3 may also call it on both ends of the alternate-screen
/// excursion, where upstream clears the same state (`:260-261`, `:301-302`).
///
/// [`focus_lost`] is the *other* clear, and deliberately not this one.
#[allow(
    dead_code,
    reason = "pi's unconditional selection clear on a scrollbar grab (tui-alt-screen.ts:776-784). \
              `super::scrollbar_drag`'s module doc specifies the call site — one line in §B-3's \
              mouse dispatcher, on a `true` from `scrollbar_drag::route` — but wiring it would \
              change what a grab does to a live selection, so it is left to the change that \
              lands §B-8's dispatcher rather than folded into a lint pass."
)]
pub(super) fn cancel(sel: &mut SelectionState) {
    stop_auto_scroll(sel);
    sel.press_active = false;
    sel.anchor = None;
    sel.focus = None;
    sel.granularity = Granularity::Character;
    sel.initial = None;
    sel.last_click = None;
    sel.pressed_url = None;
    sel.dragged = false;
}

/// Cancel on focus loss, returning whether the screen needs repainting — pi's `FOCUS_OUT` arm
/// (`tui-alt-screen.ts:543-559`).
///
/// This is the failure the unit would otherwise ship: a pointer that leaves the window mid-drag
/// sends no release, so without an explicit cancel the press stays armed and the next unrelated
/// report extends a gesture the user finished minutes ago. `?1004h` — the focus reporting ADR-0005
/// §B-4 asks the terminal for — is what delivers the event this arm reads.
///
/// It is **not** [`cancel`]: upstream clears the anchor and focus only when a press was actually
/// active (`:551-557`), so a *completed* selection stays on screen and stays copyable when the user
/// clicks away to another window and back. The return value is upstream's
/// `hadNonEmptyActiveSelection` (`:545`, `:556`) — a repaint is owed only when something visible
/// was removed.
pub(super) fn focus_lost(sel: &mut SelectionState) -> bool {
    let had_press = sel.press_active;
    let had_visible = had_press && bounds(sel).is_some();
    sel.press_active = false;
    stop_auto_scroll(sel);
    sel.pressed_url = None;
    sel.dragged = false;
    if had_press {
        sel.anchor = None;
        sel.focus = None;
        sel.granularity = Granularity::Character;
        sel.initial = None;
    }
    sel.last_click = None;
    had_visible
}

/// Whether anything is selected — upstream's `this.getSelectionBounds() !== undefined` (`:545`).
///
/// ADR-0005 §B-11's `/copy` reads it to choose between the selection and the transcript, which is
/// the same question upstream asks before copying.
#[allow(
    dead_code,
    reason = "pi's `this.getSelectionBounds() !== undefined` (tui-alt-screen.ts:545). ADR-0005 \
              §B-11's `/copy` fork is the caller named in the doc above; that fork is not wired \
              yet, and `bounds` stays private so this is the only way to ask."
)]
pub(super) fn has_selection(sel: &SelectionState) -> bool {
    bounds(sel).is_some()
}

/// The selected text exactly as it appears on screen, or `None` when nothing is selected — the
/// string half of pi's `copySelectionToClipboard` (`tui-alt-screen.ts:1087-1112`), stopping short
/// of the write for the reason in the module doc.
///
/// Row by row between the two ends: the first row starts at the selection's start column, the last
/// ends at its end column, and every row in between is taken whole — upstream's
/// `getSelectionColumns` defaults (`:1066-1091`). Each row is right-trimmed and the rows are joined
/// with `\n` (`:1099-1105`), so a rectangular-looking selection over ragged rows copies as the text
/// a reader would have highlighted and not as padded cells.
///
/// Upstream additionally runs `stripTerminalSequences` over each row (`:1102`) because its document
/// rows are ANSI-bearing strings. cyrup's are [`Line`]s, whose styling travels out of band in each
/// [`ratatui::text::Span`], so the concatenated content is already the plain text and there is
/// nothing to strip — the reason [`crate::ansi::strip_ansi`] is not called here.
pub(super) fn selected_text(sel: &SelectionState, doc: &[Line<'_>]) -> Option<String> {
    let (start, end) = bounds(sel)?;
    let mut rows: Vec<String> = Vec::new();
    for row in start.row..=end.row {
        let text = line_text(doc, row);
        let width = str_width(&text);
        let (from, to) = selection_columns(&text, row, start, end, 0, width);
        rows.push(slice_by_column(&text, from, to).trim_end().to_string());
    }
    let joined = rows.join("\n");
    // `if (text.length === 0) return;` (`:1106`) — a selection that resolves to nothing (whitespace
    // only, or a zero-width span) copies nothing rather than clearing the clipboard.
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

/// Paint the selection over cells already drawn — pi's `applySelection` (`:1209-1256`) with the
/// reverse-video wrapper it applies per row (`applySelectionHighlight`, `:1192-1207`).
///
/// Runs **after** the document and before the flash stack, which is upstream's composite order
/// (`:1290`). Reverse video rather than a theme colour is upstream's choice and is worth keeping
/// for a reason beyond fidelity: it inverts whatever each cell already is, so a selection stays
/// visible over syntax-highlighted code, diff backgrounds and images alike, and this module needs
/// no [`crate::UiTheme`] and cannot go stale against one.
///
/// `Cell::set_style` *inserts* the modifier rather than replacing the cell's style
/// (ratatui-core `buffer/cell.rs:192-205`), which is the same preservation upstream reaches by
/// re-emitting `\x1b[7m` after every inner SGR code it copies through (`:1200-1202`).
///
/// Rows outside the viewport are skipped rather than clamped: a selection that runs off the top or
/// bottom of the scrolled view is simply not painted there, exactly as upstream's `row < minRow ||
/// row > maxRow` test does (`:1240-1241`).
pub(super) fn highlight(
    sel: &SelectionState,
    scroll: &ScrollState,
    doc: &[Line<'_>],
    frame: &mut Frame,
    viewport: Rect,
) {
    let Some((start, end)) = bounds(sel) else {
        return;
    };
    if viewport.width == 0 || viewport.height == 0 {
        return;
    }
    let top = scroll::scroll_top(scroll);
    let bottom = top
        .saturating_add(usize::from(viewport.height))
        .saturating_sub(1);
    let (first, last) = (start.row.max(top), end.row.min(bottom));
    if last < first {
        return;
    }
    let style = Style::default().add_modifier(Modifier::REVERSED);
    let max_col = usize::from(viewport.width);
    for row in first..=last {
        let text = line_text(doc, row);
        let (from, to) = selection_columns(&text, row, start, end, 0, max_col);
        // `if (columns.end <= columns.start) return line;` (`:1249`).
        if to <= from {
            continue;
        }
        if let Some(rect) = row_rect(viewport, top, row, from, to) {
            frame.buffer_mut().set_style(rect, style);
        }
    }
}

/// Begin a gesture — the press tail of pi's `handleSelectionMouseEvent` (`:1024-1046`).
///
/// The click count picks the granularity: one press selects characters, two the word under the
/// pointer, three the whole row (`:1032-1036`). A word or line press seeds both ends from the
/// resolved range, so the selection is already visible before any drag, and records that range as
/// the pivot every later extension turns around ([`SelectionState::initial`]).
///
/// A press outside the document viewport is declined. Upstream instead falls back to whole-screen
/// coordinates (`:822-826`) and lets a selection run over its chrome; cyrup's chrome is the shared
/// [`crate::AppState`] editor, status band and selector slot that both renderers paint, and each
/// already owns its pointer behaviour — so the report is passed on rather than claimed.
fn press(
    sel: &mut SelectionState,
    scroll: &ScrollState,
    doc: &[Line<'_>],
    viewport: Rect,
    ev: &MouseEvent,
) -> PointerOutcome {
    // `getScrollViewsAt(this.currentLayout, event.x, event.y)[0]` (`:1029`) reduced to cyrup's one
    // view — the containment test `layout.ts:384-386` performs, as [`super::wheel::route`] does it.
    if !viewport.contains(Position::new(ev.column, ev.row)) {
        return PointerOutcome::Ignored;
    }
    let Some(anchor) = point_at(scroll, doc, viewport, ev.column, ev.row) else {
        return PointerOutcome::Ignored;
    };
    stop_auto_scroll(sel);
    sel.press_active = true;
    let text = line_text(doc, anchor.row);
    let word = word_selection(&text, anchor);
    let count = click_count(sel, anchor, word, Instant::now());
    // `clickCount === 2 ? word : clickCount === 3 ? this.getLineSelection(anchor) : undefined`
    // (`:1034`). A double click on a column with no word under it resolves nothing and stays
    // character-granular, which is upstream's `range ? … : "character"` (`:1035`).
    let range = match count {
        2 => word,
        3 => Some(line_selection(&text, anchor)),
        _ => None,
    };
    sel.granularity = match (range, count) {
        (None, _) => Granularity::Character,
        (Some(_), 2) => Granularity::Word,
        (Some(_), _) => Granularity::Line,
    };
    sel.initial = range;
    sel.anchor = Some(range.map_or(anchor, |r| r.start));
    sel.focus = Some(range.map_or(anchor, |r| r.end));
    sel.dragged = false;
    // `this.pressedUrl = range ? undefined : getOsc8LinkAtColumn(…)` (`:1040-1046`): a word or line
    // press is a selection gesture and never a link click.
    sel.pressed_url = if range.is_some() {
        None
    } else {
        link_at(&text, anchor.col)
    };
    PointerOutcome::Handled
}

/// Extend a gesture — the motion branch of pi's `handleSelectionMouseEvent` (`:1016-1023`).
///
/// Clearing [`SelectionState::last_click`] is upstream's (`:1019`) and is what stops a drag from
/// counting toward the next double click: a press, a drag and a second press are three separate
/// gestures, not a double click on the row the drag ended over.
fn drag(
    sel: &mut SelectionState,
    scroll: &ScrollState,
    doc: &[Line<'_>],
    viewport: Rect,
    ev: &MouseEvent,
) -> PointerOutcome {
    // `if (!this.selectionPressActive || !this.selectionAnchor) return;` (`:1017`).
    if !sel.press_active || sel.anchor.is_none() {
        return PointerOutcome::Ignored;
    }
    sel.dragged = true;
    sel.last_click = None;
    sel.pressed_url = None;
    // The point is CLAMPED into the viewport rather than rejected (`:806-811`), which is what lets
    // a drag past the edge keep extending while [`update_auto_scroll`] brings more document to it.
    if let Some(point) = point_at(scroll, doc, viewport, ev.column, ev.row) {
        update_focus(sel, doc, point);
    }
    update_auto_scroll(sel, viewport, ev);
    PointerOutcome::Handled
}

/// End a gesture — the release branch of pi's `handleSelectionMouseEvent` (`:986-1014`).
///
/// A release that did not move, landed on the cell it started on and had a link under that cell
/// activates the link and clears the selection instead of copying (`:995-1009`); anything else
/// copies what is selected (`:1011`). That ordering is the whole of "clicking a link activates it,
/// clicking plain text does not": a click on plain text selects nothing, so [`selected_text`]
/// answers `None` and the release is a no-op.
fn release(
    sel: &mut SelectionState,
    scroll: &ScrollState,
    doc: &[Line<'_>],
    viewport: Rect,
    ev: &MouseEvent,
) -> PointerOutcome {
    // `if (!this.selectionPressActive) return;` (`:987`) — a release belonging to no press of ours.
    if !sel.press_active {
        return PointerOutcome::Ignored;
    }
    sel.press_active = false;
    stop_auto_scroll(sel);
    let (Some(anchor), Some(point)) = (
        sel.anchor,
        point_at(scroll, doc, viewport, ev.column, ev.row),
    ) else {
        return PointerOutcome::Handled;
    };
    update_focus(sel, doc, point);
    let clicked_url = if sel.dragged || !same_cell(anchor, point) {
        None
    } else {
        sel.pressed_url.clone()
    };
    sel.pressed_url = None;
    if let Some(url) = clicked_url {
        sel.anchor = None;
        sel.focus = None;
        // `try { this.openUrl(clickedUrl); } catch {}` (`:1004-1008`) — activation is best-effort,
        // and [`crate::open_browser::open_browser`] is already exactly that: it spawns the platform
        // launcher with its stdio nulled, reaps it off-thread and swallows every failure.
        crate::open_browser::open_browser(&url);
        return PointerOutcome::Handled;
    }
    match selected_text(sel, doc) {
        Some(text) => PointerOutcome::Copy(text),
        None => PointerOutcome::Handled,
    }
}

/// Whether this report is the unmodified secondary-button press upstream pastes on — the gate of
/// pi's `handleRightClickPaste` (`tui-alt-screen.ts:700-710`).
///
/// Every clause is upstream's, including the two that make it rare: the paste is Windows-only
/// (`process.platform !== "win32"`, `:703`) because elsewhere a right-click is the terminal's own
/// context menu, and it is off under VS Code's integrated terminal (`:704`), which pastes on
/// right-click itself. Upstream's fifth clause — `!this.onRightClickPaste` (`:702`) — is the host
/// declining to supply a paste at all; cyrup supplies one, through the same
/// [`crate::clipboard::read_clipboard_text`] the editor's own paste already uses, so that clause has
/// no counterpart.
///
/// `std::env::consts::OS` rather than `cfg!(windows)`, matching [`crate::clipboard`]'s own platform
/// tests (`clipboard.rs:97`, `:259`): the branch stays compiled on every host, so it cannot rot.
fn right_click_paste_applies(ev: &MouseEvent) -> bool {
    // `event.release || event.button !== 2` (`:705-706`) — the press, not the release.
    if !matches!(ev.kind, MouseEventKind::Down(MouseButton::Right)) {
        return false;
    }
    if std::env::consts::OS != "windows" {
        return false;
    }
    // `process.env.TERM_PROGRAM?.toLowerCase() === "vscode"` (`:704`).
    !std::env::var("TERM_PROGRAM").is_ok_and(|program| program.eq_ignore_ascii_case("vscode"))
}

/// Move the focus end to `point`, pivoting a word or line selection around its initial range — pi's
/// `updateSelectionFocus` (`tui-alt-screen.ts:880-898`).
///
/// Character granularity just follows the pointer (`:881-884`). The other two re-resolve the range
/// under the pointer and then choose which end of the *initial* range to anchor to, so dragging
/// backwards past the word the gesture started on keeps whole words selected on that side rather
/// than cutting the origin word in half (`:886-897`).
fn update_focus(sel: &mut SelectionState, doc: &[Line<'_>], point: Point) {
    // `if (this.selectionGranularity === "character" || !this.selectionInitialRange)` (`:881`).
    if sel.granularity == Granularity::Character {
        sel.focus = Some(point);
        return;
    }
    let Some(initial) = sel.initial else {
        sel.focus = Some(point);
        return;
    };
    let text = line_text(doc, point.row);
    let range = if sel.granularity == Granularity::Word {
        word_selection(&text, point)
    } else {
        Some(line_selection(&text, point))
    };
    // `if (!range) return;` (`:889`) — a pointer past the end of a row with no word under it leaves
    // the selection where it was rather than collapsing it.
    let Some(range) = range else {
        return;
    };
    if before(range.start, initial.start) {
        sel.anchor = Some(initial.end);
        sel.focus = Some(range.start);
    } else {
        sel.anchor = Some(initial.start);
        sel.focus = Some(range.end);
    }
}

/// Advance the click ladder and record this press — pi's `getClickCount`
/// (`tui-alt-screen.ts:900-923`).
///
/// The ladder advances only when the press is close enough in time *and* lands on the same row with
/// the same word under it (`:904-910`), so a fast pair of clicks on two different words is two
/// single clicks. `(previous.count % 3) + 1` (`:911`) is what makes a fourth click start over at
/// character granularity rather than staying on lines.
///
/// A press with no word under it records nothing (`:913-923`), which is upstream's way of saying an
/// empty column cannot be the first half of a double click.
fn click_count(sel: &mut SelectionState, point: Point, word: Option<Range>, now: Instant) -> u8 {
    let count = match (word, sel.last_click) {
        (Some(range), Some(previous))
            if now.saturating_duration_since(previous.at) <= DOUBLE_CLICK_INTERVAL
                && previous.row == point.row
                && previous.word_start == range.start.col
                && previous.word_end == range.end.col =>
        {
            (previous.count % 3).saturating_add(1)
        }
        _ => 1,
    };
    sel.last_click = word.map(|range| LastClick {
        at: now,
        count,
        row: point.row,
        word_start: range.start.col,
        word_end: range.end.col,
    });
    count
}

/// Arm, re-aim or disarm the edge auto-scroll from a drag report — pi's
/// `updateSelectionAutoScroll` (`tui-alt-screen.ts:926-952`).
///
/// The edge rows themselves trigger it, not merely the rows beyond them: upstream's test is
/// `y <= visibleTop ? -1 : y >= visibleBottom ? 1 : 0` (`:944`). A pointer dragged back inside
/// disarms (`:945-948`), and a pointer already holding an edge re-aims the existing deadline rather
/// than restarting it — upstream's `if (this.selectionAutoScrollTimer) return;` (`:949`), which is
/// what keeps the scroll rate constant instead of accelerating with report frequency.
fn update_auto_scroll(sel: &mut SelectionState, viewport: Rect, ev: &MouseEvent) {
    if viewport.height == 0 {
        stop_auto_scroll(sel);
        return;
    }
    let visible_top = viewport.y;
    let visible_bottom = viewport.y.saturating_add(viewport.height).saturating_sub(1);
    let direction = if ev.row <= visible_top {
        -1
    } else if ev.row >= visible_bottom {
        1
    } else {
        0
    };
    if direction == 0 {
        stop_auto_scroll(sel);
        return;
    }
    let pointer = Position::new(ev.column, ev.row);
    if let Some(auto) = sel.auto.as_mut() {
        auto.direction = direction;
        auto.pointer = pointer;
        return;
    }
    let now = Instant::now();
    sel.auto = Some(AutoScroll {
        direction,
        pointer,
        due: now.checked_add(AUTO_SCROLL_INTERVAL).unwrap_or(now),
    });
}

/// Disarm the edge auto-scroll — pi's `stopSelectionAutoScroll` (`tui-alt-screen.ts:972-979`),
/// which clears the interval, the direction and the stored pointer together.
fn stop_auto_scroll(sel: &mut SelectionState) {
    sel.auto = None;
}

/// The document point a pointer position names, clamped into the viewport — pi's
/// `getScrollSelectionPoint` (`tui-alt-screen.ts:797-815`).
///
/// The row clamp is what makes an edge drag work at all: a pointer below the last visible row
/// resolves to that row, so the focus sits at the bottom of the document and each auto-scroll tick
/// brings the next row under it (`:806`). The result is then clamped to the document's own last
/// row (`:808-810`), and the column to the viewport's width (`:811`).
///
/// `None` only for a viewport with no cells, which is upstream's `box.rect.height <= 0` guard
/// (`:800`).
fn point_at(
    scroll: &ScrollState,
    doc: &[Line<'_>],
    viewport: Rect,
    column: u16,
    row: u16,
) -> Option<Point> {
    if viewport.width == 0 || viewport.height == 0 {
        return None;
    }
    let visible_bottom = viewport.y.saturating_add(viewport.height).saturating_sub(1);
    let pointer_row = row.clamp(viewport.y, visible_bottom);
    // `Math.max(0, (box.scrollContentLines?.length ?? 1) - 1)` (`:808`).
    let max_content_row = doc.len().saturating_sub(1);
    let offset = usize::from(pointer_row.saturating_sub(viewport.y));
    Some(Point {
        row: scroll::scroll_top(scroll)
            .saturating_add(offset)
            .min(max_content_row),
        col: usize::from(
            column
                .saturating_sub(viewport.x)
                .min(viewport.width.saturating_sub(1)),
        ),
        boundary: false,
    })
}

/// The plain text of one document row — pi's `getSelectionSourceLine`
/// (`tui-alt-screen.ts:828-834`), whose `lines[point.row] ?? ""` is this `unwrap_or_default`.
///
/// Concatenating a [`Line`]'s spans is [`std::fmt::Display`] for it (ratatui-core
/// `text/line.rs:839-845`), and the result carries no styling — see [`selected_text`].
fn line_text(doc: &[Line<'_>], row: usize) -> String {
    doc.get(row).map(ToString::to_string).unwrap_or_default()
}

/// One segment of a row's word segmentation — the shape pi builds inside `getWordSelection`
/// (`tui-alt-screen.ts:838-844`).
struct WordSegment {
    /// First column of the segment.
    start: usize,
    /// First column past it.
    end: usize,
    /// `segment.isWordLike === true || joiner` (`:843`).
    selectable: bool,
    /// Whether the segment is one of [`WORD_JOINERS`].
    joiner: bool,
}

/// The word range under `point`, or `None` when the column has no segment — pi's
/// `getWordSelection` (`tui-alt-screen.ts:836-871`).
///
/// Upstream segments with `Intl.Segmenter`'s word iterator and asks each segment for `isWordLike`;
/// `unicode_segmentation`'s `split_word_bounds` is the same UAX #29 word-boundary algorithm, and
/// "word-like" is a segment carrying an alphanumeric — the crate already depends on it for
/// grapheme-cluster editor motion, so no new dependency appears here.
///
/// The joiner walk is what keeps `src/main.rs` and `kebab-case-token` whole: two selectable
/// segments merge when either is a joiner (`:851-869`), so a run of word/joiner/word extends in
/// both directions from the clicked segment while `word punctuation word` does not.
fn word_selection(text: &str, point: Point) -> Option<Range> {
    let mut segments: Vec<WordSegment> = Vec::new();
    let mut start = 0usize;
    for segment in text.split_word_bounds() {
        let end = start.saturating_add(str_width(segment));
        let joiner = WORD_JOINERS.contains(&segment);
        segments.push(WordSegment {
            start,
            end,
            selectable: joiner || segment.chars().any(char::is_alphanumeric),
            joiner,
        });
        start = end;
    }
    // `segments.findIndex((s) => point.col >= s.start && point.col < s.end)` (`:847-849`).
    let clicked = segments
        .iter()
        .position(|segment| point.col >= segment.start && point.col < segment.end)?;
    let mut selection_start = segments.get(clicked)?.start;
    let mut selection_end = segments.get(clicked)?.end;
    let mut index = clicked;
    while index > 0 {
        let previous = index.saturating_sub(1);
        let (Some(left), Some(right)) = (segments.get(previous), segments.get(index)) else {
            break;
        };
        if !can_join(left, right) {
            break;
        }
        selection_start = left.start;
        index = previous;
    }
    let mut index = clicked;
    while let (Some(left), Some(right)) =
        (segments.get(index), segments.get(index.saturating_add(1)))
    {
        if !can_join(left, right) {
            break;
        }
        selection_end = right.end;
        index = index.saturating_add(1);
    }
    Some(Range {
        start: Point {
            row: point.row,
            col: selection_start,
            boundary: false,
        },
        end: Point {
            row: point.row,
            col: selection_end,
            boundary: true,
        },
    })
}

/// Whether two adjacent segments merge into one word — pi's `canJoin`
/// (`tui-alt-screen.ts:855-858`): `left.selectable && right.selectable && (left.joiner ||
/// right.joiner)`.
fn can_join(left: &WordSegment, right: &WordSegment) -> bool {
    left.selectable && right.selectable && (left.joiner || right.joiner)
}

/// The whole row as a range — pi's `getLineSelection` (`tui-alt-screen.ts:873-878`).
fn line_selection(text: &str, point: Point) -> Range {
    Range {
        start: Point {
            row: point.row,
            col: 0,
            boundary: false,
        },
        end: Point {
            row: point.row,
            col: str_width(text),
            boundary: true,
        },
    }
}

/// The selection ordered start-to-end, or `None` when nothing is selected — pi's
/// `getSelectionBounds` (`tui-alt-screen.ts:1049-1064`).
///
/// A selection whose two ends are the same cell is `None` (`:1058-1063`), which is what makes a
/// plain click select nothing and a release on it copy nothing.
fn bounds(sel: &SelectionState) -> Option<(Point, Point)> {
    let (anchor, focus) = (sel.anchor?, sel.focus?);
    if same_cell(anchor, focus) {
        return None;
    }
    if before(anchor, focus) {
        Some((anchor, focus))
    } else {
        Some((focus, anchor))
    }
}

/// Whether `a` comes before `b` in reading order — upstream's `anchorBeforeFocus` comparison
/// (`tui-alt-screen.ts:1052-1054`), also used as `targetBeforeInitial` (`:886-888`).
fn before(a: Point, b: Point) -> bool {
    a.row < b.row || (a.row == b.row && a.col < b.col)
}

/// Whether two points name the same cell — upstream's collapsed-selection test (`:1058-1061`) and
/// the click-not-drag test (`:998-1000`).
fn same_cell(a: Point, b: Point) -> bool {
    a.row == b.row && a.col == b.col
}

/// The `[start, end)` column span of `row` inside the selection — pi's `getSelectionColumns`
/// (`tui-alt-screen.ts:1066-1091`).
///
/// A row strictly inside the selection takes the caller's bounds whole; the first and last rows are
/// cut to the selection's own columns. Both cuts snap to **grapheme** cell boundaries through
/// [`grapheme_cell_range`], so a selection that starts or ends inside a wide character or an emoji
/// covers the whole of it rather than half a cell pair (`:1073`, `:1077`). A `boundary` end column
/// is taken as-is, because a word or line selection already stops *before* the column it names
/// (`:1075`).
fn selection_columns(
    text: &str,
    row: usize,
    start: Point,
    end: Point,
    min_column: usize,
    max_column: usize,
) -> (usize, usize) {
    let width = str_width(text);
    let mut from = min_column;
    let mut to = width.min(max_column);
    if row == start.row {
        from = grapheme_cell_range(text, start.col)
            .map_or_else(|| start.col.min(width), |(cell_start, _)| cell_start);
    }
    if row == end.row {
        to = if end.boundary {
            end.col.min(width)
        } else {
            grapheme_cell_range(text, end.col).map_or_else(
                || end.col.saturating_add(1).min(width),
                |(_, cell_end)| cell_end,
            )
        };
    }
    (from.max(min_column), to.min(max_column))
}

/// The `[start, end)` cell span of the grapheme covering `column` — pi's `getGraphemeCellRange`
/// (`utils.ts:320-341`).
///
/// Zero-width graphemes — combining marks, variation selectors — are skipped rather than matched
/// (`utils.ts:333`), so a column always resolves to the visible cluster it is painted in.
fn grapheme_cell_range(text: &str, column: usize) -> Option<(usize, usize)> {
    let mut current = 0usize;
    for grapheme in text.graphemes(true) {
        let width = str_width(grapheme);
        let end = current.saturating_add(width);
        if width > 0 && column >= current && column < end {
            return Some((current, end));
        }
        current = end;
    }
    None
}

/// The text of `[start_col, end_col)` — pi's `sliceByColumn(line, start, length, true)`
/// (`utils.ts:1195-1197` over `sliceWithWidth`, `:1200-1250`).
///
/// `strict`, which is what every selection caller passes: a wide grapheme that would straddle the
/// end column is dropped rather than half-copied (`utils.ts:1229`). Upstream's ANSI bookkeeping has
/// no counterpart — the text handed in carries no escapes (see [`selected_text`]).
///
/// Grapheme-atomic throughout, never a byte range: this crate denies `clippy::string_slice` for
/// exactly the defect a byte cut would reintroduce.
fn slice_by_column(text: &str, start_col: usize, end_col: usize) -> String {
    let mut out = String::new();
    if end_col <= start_col {
        return out;
    }
    let mut current = 0usize;
    for grapheme in text.graphemes(true) {
        let end = current.saturating_add(str_width(grapheme));
        if current >= start_col && current < end_col && end <= end_col {
            out.push_str(grapheme);
        }
        current = end;
        if current >= end_col {
            break;
        }
    }
    out
}

/// The screen rectangle one row's selected columns occupy, clipped to the viewport.
///
/// `row` is a document row and `top` is [`super::scroll::scroll_top`], so the screen row is
/// `viewport.y + (row - top)` — upstream's `box.rect.y + selection.row - scrollTop` (`:1226`).
/// `None` for a row above the view, and the intersection guards the rest; the buffer clips again on
/// the way in (ratatui-core `buffer/buffer.rs:405-413`), so a wrong answer here cannot paint
/// outside the frame.
fn row_rect(viewport: Rect, top: usize, row: usize, from: usize, to: usize) -> Option<Rect> {
    let offset = u16::try_from(row.checked_sub(top)?).ok()?;
    let rect = Rect {
        x: viewport.x.checked_add(u16::try_from(from).ok()?)?,
        y: viewport.y.checked_add(offset)?,
        width: u16::try_from(to.saturating_sub(from)).ok()?,
        height: 1,
    };
    Some(viewport.intersection(rect))
}

/// The link under `column`, or `None` when that column is not on one — cyrup's stand-in for pi's
/// `getOsc8LinkAtColumn(this.previousScreen[y], x)` (`tui-alt-screen.ts:1042-1046`, over
/// `utils.ts:344`).
///
/// `[CYRUP-DELTA]`, and it is forced rather than chosen. Upstream hit-tests the OSC 8 escapes
/// embedded in its rendered screen strings; cyrup renders through ratatui's cell buffer, which has
/// **no channel for an OSC 8 escape at all** — a `\x1b]8;;…` inside a [`ratatui::text::Span`] would
/// be laid into cells as literal text — so the crate drops hyperlink wrapping everywhere and prints
/// pi's legacy `text (url)` instead (`markdown/mod.rs:142-152`, matching `markdown.ts:692-707`).
/// There is therefore no escape to hit-test, and the URL that upstream hides in one is on the row
/// as ordinary text.
///
/// So the hit test is over that text: the whitespace-delimited run containing the column, trailing
/// sentence punctuation and one unbalanced closing bracket removed, accepted only if it opens with
/// one of [`LINK_SCHEMES`]. That keeps the acceptance criterion exactly — a click on a link
/// activates its target, a click on plain text does not — because plain prose contains no run
/// beginning `https://`. It is deliberately conservative: a bare `www.` host or a naked domain is
/// not a link here, since guessing wrong opens a browser the user did not ask for.
fn link_at(text: &str, column: usize) -> Option<String> {
    let run = run_at_column(text, column)?;
    let trimmed = run.trim_end_matches(['.', ',', ';', ':', '!', '?']);
    // A URL pasted inside prose is commonly wrapped: `(https://x)` or `[https://x]`. Only an
    // UNBALANCED closer is dropped, so a path that legitimately ends in one survives.
    let trimmed = match (trimmed.ends_with(')'), trimmed.contains('(')) {
        (true, false) => trimmed.trim_end_matches(')'),
        _ => trimmed,
    };
    let trimmed = match (trimmed.ends_with(']'), trimmed.contains('[')) {
        (true, false) => trimmed.trim_end_matches(']'),
        _ => trimmed,
    };
    LINK_SCHEMES
        .iter()
        .any(|scheme| {
            trimmed
                .strip_prefix(*scheme)
                .is_some_and(|rest| !rest.is_empty())
        })
        .then(|| trimmed.to_string())
}

/// The whitespace-delimited run of text covering `column`, or `None` when the column is on
/// whitespace or past the end of the row. Grapheme-atomic and accumulated by `push_str`, never by
/// byte range (`clippy::string_slice` is denied crate-wide).
fn run_at_column(text: &str, column: usize) -> Option<String> {
    let mut run = String::new();
    let mut hit = false;
    let mut current = 0usize;
    for grapheme in text.graphemes(true) {
        let end = current.saturating_add(str_width(grapheme));
        if is_ws_grapheme(grapheme) {
            if hit {
                return Some(run);
            }
            run.clear();
        } else {
            run.push_str(grapheme);
            if column >= current && column < end {
                hit = true;
            }
        }
        current = end;
    }
    hit.then_some(run)
}
