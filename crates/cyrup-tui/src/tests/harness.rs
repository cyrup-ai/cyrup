//! Shared scrape-and-key harness for the `App<TestBackend>` suites in this directory.
//!
//! Before this module every file hand-rolled its own buffer scrape — 21 copies of `buf_text`, 25 of
//! the `key`/`ctrl`/`alt`/`esc` event constructors — and they had already drifted. Twenty of the
//! twenty-one `buf_text` copies pushed `'\n'` after **every** row; one built the same string with
//! `.join("\n")` and so had none after the last. `buf_text(&app).contains("foo\n")` asserted on the
//! bottom row therefore meant two different things depending on which file it was written in. The
//! same defect had already been paid for once on the locking side: `image_capabilities` had to
//! promote a file-private mutex to a named accessor so `markdown` could serialize against it, since
//! "a lock the reader cannot name does not serialize anything".
//!
//! # Trailing-newline convention (pinned here, once)
//!
//! **Every row-range helper in this module — [`buf_text`], [`live_text`], [`rows_text`] —
//! terminates EVERY row with `'\n'`, the last row included.** For a non-empty range the returned
//! string therefore always ends in `'\n'`, and `text.contains("needle\n")` matches a needle that
//! ends the bottom row exactly as it matches one that ends any other row. This is the convention
//! the twenty-copy majority already used, so it is the one that stays.
//!
//! [`row_text`] is the deliberate exception and carries no newline at all: it returns a single row,
//! not a range, so there is no separator to be consistent about.
#![allow(clippy::panic)]

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{App, InputEvent};
use ratatui::backend::TestBackend;
use ratatui::style::Color;

// ------------------------------------------------------------------------- buffer scrapes ----

/// Rows `y0..y1` of the rendered buffer, each terminated by `'\n'` (see the module convention).
pub(crate) fn rows_text(app: &App<TestBackend>, y0: u16, y1: u16) -> String {
    let buf = app.terminal().backend().buffer();
    let area = buf.area;
    let mut out = String::new();
    for y in y0..y1 {
        for x in 0..area.width {
            if let Some(cell) = buf.cell((x, y)) {
                out.push_str(cell.symbol());
            }
        }
        out.push('\n');
    }
    out
}

/// The whole rendered buffer as text (every row, including the scrollback band above the viewport).
pub(crate) fn buf_text(app: &App<TestBackend>) -> String {
    rows_text(app, 0, app.terminal().backend().buffer().area.height)
}

/// Only the **live region** — the bottom `viewport_height` rows the app repaints each frame.
pub(crate) fn live_text(app: &App<TestBackend>) -> String {
    let h = app.terminal().backend().buffer().area.height;
    let vh = app.viewport_height().min(h);
    rows_text(app, h - vh, h)
}

/// Row `y` as text, with **no** trailing newline — one row, not a range.
pub(crate) fn row_text(app: &App<TestBackend>, y: u16) -> String {
    let buf = app.terminal().backend().buffer();
    (0..buf.area.width).filter_map(|x| buf.cell((x, y))).map(|c| c.symbol()).collect()
}

/// The index of the first row whose text contains `needle`.
pub(crate) fn find_row(app: &App<TestBackend>, needle: &str) -> u16 {
    let h = app.terminal().backend().buffer().area.height;
    (0..h)
        .find(|&y| row_text(app, y).contains(needle))
        .unwrap_or_else(|| panic!("no row contains {needle:?}:\n{}", buf_text(app)))
}

/// The first row containing `needle`, as `(y, row_text)`.
pub(crate) fn row_with(app: &App<TestBackend>, needle: &str) -> (u16, String) {
    let y = find_row(app, needle);
    (y, row_text(app, y))
}

/// The CELL column of `needle` in `row`. `str::find` returns a BYTE offset, and these rows carry
/// `↑`/`↓`/`·`/`…`, so the two are not the same number — indexing the buffer with the byte offset
/// samples the wrong cell.
pub(crate) fn col_of(row: &str, needle: &str) -> u16 {
    let byte = row.find(needle).unwrap_or_else(|| panic!("{needle:?} not in [{row}]"));
    row.get(..byte).map_or(0, |p| p.chars().count()) as u16
}

/// The foreground colour of the cell at `(x, y)`.
pub(crate) fn fg_at(app: &App<TestBackend>, x: u16, y: u16) -> Option<Color> {
    app.terminal().backend().buffer().cell((x, y)).map(|c| c.fg)
}

// --------------------------------------------------------------------------- input events ----

/// An unmodified key as the app-level [`InputEvent`] the run loop feeds `App::handle_input`.
pub(crate) fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(key_event(code))
}

/// The same unmodified key as a bare crossterm [`KeyEvent`], for the component-tier tests that call
/// `InputEditor`/selector `handle_key` directly instead of going through `App`.
pub(crate) fn key_event(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// `Ctrl` + `code` as an [`InputEvent`].
pub(crate) fn ctrl(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::CONTROL))
}

/// `Alt` + `code` as an [`InputEvent`].
pub(crate) fn alt(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::ALT))
}

/// A bare `Esc` press.
pub(crate) fn esc() -> InputEvent {
    key(KeyCode::Esc)
}

// ------------------------------------------------------------------- process-global locking ----

/// The process-wide cache makes every test that touches it order-dependent; one lock serializes
/// them.
static CAPS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Take [`CAPS_LOCK`], ignoring poisoning — same device as `crate::panic_hook`'s `HOOK_LOCK` and
/// `crate::drain`'s `DRAIN_LOCK`: a sibling that panicked has already reported its own failure, and
/// refusing the lock here would turn that into a second, misleading one.
///
/// `pub(crate)` in the shared harness rather than file-private to `image_capabilities`, and that is
/// the whole point of this accessor. The writer in `tests::image_capabilities` is not the only test
/// that depends on `image::CAPABILITIES`: `render_markdown` reaches `hyperlinks_supported()`
/// through `markdown::render_with_default_style` (`markdown.rs:126`), so
/// `tests::markdown::inline_code_bold_and_links` READS the same global — and it read it TWICE, once
/// directly and once inside `render_markdown`, asserting the two agree. A pin landing between those
/// two reads makes a correct renderer disagree with itself. A lock the reader cannot name does not
/// serialize anything, which is what a file-private `static` amounted to here.
pub(crate) fn caps_lock() -> std::sync::MutexGuard<'static, ()> {
    CAPS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}
