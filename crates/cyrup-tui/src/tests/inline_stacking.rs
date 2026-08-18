//! Raw-escape-capture regression harness for the inline-viewport **STACKING** bug (the one a user hit
//! in a real terminal: several duplicated keybinding-hint bars stacked up the screen).
//!
//! `TestBackend` (used by `assembled_render.rs`) is a fixed grid that never scrolls, so the stacking —
//! which is produced by the terminal SCROLLING when ratatui reserves fresh inline rows on each
//! content-driven viewport reconstruction — is invisible to it. This class of bug has slipped past
//! `TestBackend` repeatedly. So here we drive the **real** `CrosstermBackend` over a SHARED capture
//! buffer (`Arc<Mutex<Vec<u8>>>`), whose `RebuildBackend::rebuild()` re-wraps the SAME buffer, so the
//! raw byte stream ACCUMULATES across reconstructions exactly like a physical terminal. We then REPLAY
//! the captured escape stream through a tiny VT screen model (visible grid + scrollback) — the
//! pty-equivalent — and assert what the user would actually SEE:
//!
//! - the keybinding-hints bar (`… interrupt …`) appears EXACTLY ONCE after the live region grows over
//!   several frames, and
//! - the editor's top+bottom rules appear EXACTLY ONCE (a single editor, not a stack of them),
//!
//! for BOTH growth (a growing multi-line buffer / a streaming answer) and shrink (commit). Before the
//! fix (erase-old-region-before-reconstruct, `app.rs::reanchor_inline_region`) the same replay shows
//! the hint bar and rules MANY times — see the module test `replay_detects_stacking_in_a_raw_stream`,
//! which feeds a hand-built "stacked" stream to prove the replay actually catches duplication.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use crate::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use crate::{reanchor_inline_region, App, InputEvent, RebuildBackend, UiTheme};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

const W: u16 = 80;
const H: u16 = 24;

// ----------------------------------------------------------------- shared capture writer ----

/// A `Write` over a shared byte buffer. Cloning shares the SAME buffer (via the `Arc`), so a rebuilt
/// backend keeps appending to the one stream — exactly what a real terminal's output pipe does.
#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A real `CrosstermBackend` writing to the shared buffer, with a FIXED size + a tracked cursor row so
/// construction never touches a controlling tty (there is none under `cargo test`). All the
/// byte-emitting operations (`draw`, `append_lines`, `set_cursor_position`, `clear*`) delegate to the
/// inner `CrosstermBackend`, so the captured stream is byte-for-byte what a terminal would receive.
struct CaptureBackend {
    inner: CrosstermBackend<SharedWriter>,
    buf: Arc<Mutex<Vec<u8>>>,
    /// Tracked cursor row: `get_cursor_position` returns this (mirrors a real terminal reporting its
    /// cursor). `set_cursor_position`/`append_lines`/`reanchor_inline` keep it current, and `rebuild`
    /// copies it across the stdout re-wrap so `compute_inline_size` reads the anchor we set.
    cursor_y: u16,
}

impl CaptureBackend {
    fn new() -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        CaptureBackend {
            inner: CrosstermBackend::new(SharedWriter(buf.clone())),
            buf,
            cursor_y: H.saturating_sub(1), // shell prompt sits at the bottom row at launch
        }
    }
    fn bytes(&self) -> Vec<u8> {
        self.buf.lock().unwrap().clone()
    }
}

impl Backend for CaptureBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        // At the bottom of the screen each newline scrolls, so the cursor stays on the last row.
        self.cursor_y = self.cursor_y.saturating_add(n).min(H.saturating_sub(1));
        self.inner.append_lines(n)
    }
    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(Position { x: 0, y: self.cursor_y })
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let p = position.into();
        self.cursor_y = p.y;
        self.inner.set_cursor_position(p)
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn size(&self) -> io::Result<Size> {
        Ok(Size { width: W, height: H })
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        Ok(WindowSize { columns_rows: Size { width: W, height: H }, pixels: Size { width: 0, height: 0 } })
    }
    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, amount)
    }
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, amount)
    }
}

impl RebuildBackend for CaptureBackend {
    fn rebuild(&self) -> Self {
        // Re-wrap the SAME shared buffer (bytes keep accumulating) and preserve the tracked cursor,
        // exactly as a fresh `CrosstermBackend::new(stdout())` inherits the real terminal's cursor.
        CaptureBackend {
            inner: CrosstermBackend::new(SharedWriter(self.buf.clone())),
            buf: self.buf.clone(),
            cursor_y: self.cursor_y,
        }
    }
    fn reanchor_inline(&mut self, term_height: u16, old_height: u16, new_height: u16) {
        // Emit the SAME erase + re-anchor the production `CrosstermBackend<Stdout>` emits, into the
        // shared buffer, and track where the cursor is left so `rebuild` carries the anchor forward.
        self.cursor_y = reanchor_inline_region(&mut self.inner, term_height, old_height, new_height);
    }
}

// --------------------------------------------------------------------- tiny VT screen model ----

/// A minimal terminal screen: a visible grid plus a scrollback history, driven by the exact escape
/// subset ratatui + crossterm emit here (CUP `H`, ED `J`, EL `K`, LF scroll, SGR/private-modes
/// ignored). Reconstructs "what the user sees" so we can count duplicated chrome the way a pty would.
struct Screen {
    w: usize,
    h: usize,
    grid: Vec<Vec<char>>,
    scrollback: Vec<Vec<char>>,
    cx: usize,
    cy: usize,
}

impl Screen {
    fn new(w: usize, h: usize) -> Self {
        Screen { w, h, grid: vec![vec![' '; w]; h], scrollback: Vec::new(), cx: 0, cy: 0 }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n {
            if self.grid.is_empty() {
                break;
            }
            let top = self.grid.remove(0);
            self.scrollback.push(top);
            self.grid.push(vec![' '; self.w]);
        }
    }
    fn newline(&mut self) {
        self.cy += 1;
        if self.cy >= self.h {
            self.scroll_up(1 + self.cy - self.h);
            self.cy = self.h.saturating_sub(1);
        }
    }
    fn put(&mut self, ch: char) {
        if self.cy < self.h && self.cx < self.w {
            self.grid[self.cy][self.cx] = ch;
        }
        if self.cx < self.w {
            self.cx += 1;
        }
    }
    fn blank_row_from(&mut self, y: usize, x0: usize) {
        if y < self.h {
            for x in x0..self.w {
                self.grid[y][x] = ' ';
            }
        }
    }
    fn clear_from_cursor_down(&mut self) {
        self.blank_row_from(self.cy, self.cx);
        for y in (self.cy + 1)..self.h {
            for x in 0..self.w {
                self.grid[y][x] = ' ';
            }
        }
    }
    fn clear_all(&mut self) {
        for row in &mut self.grid {
            for c in row.iter_mut() {
                *c = ' ';
            }
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        // The stream is valid UTF-8 (escapes are ASCII, text is UTF-8); decode once and walk chars.
        let text = String::from_utf8_lossy(bytes);
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            match c {
                '\u{1b}' => i = self.escape(&chars, i),
                '\r' => {
                    self.cx = 0;
                    i += 1;
                }
                '\n' => {
                    self.newline();
                    i += 1;
                }
                '\t' => {
                    self.cx = (((self.cx / 8) + 1) * 8).min(self.w);
                    i += 1;
                }
                '\u{08}' => {
                    self.cx = self.cx.saturating_sub(1);
                    i += 1;
                }
                c if (c as u32) >= 0x20 => {
                    self.put(c);
                    i += 1;
                }
                _ => i += 1,
            }
        }
    }

    fn escape(&mut self, chars: &[char], i: usize) -> usize {
        let mut j = i + 1;
        if j >= chars.len() {
            return j;
        }
        match chars[j] {
            '[' => {
                j += 1;
                let start = j;
                while j < chars.len() && !('@'..='~').contains(&chars[j]) {
                    j += 1;
                }
                if j >= chars.len() {
                    return j;
                }
                let final_c = chars[j];
                let params: String = chars[start..j].iter().collect();
                self.csi(final_c, &params);
                j + 1
            }
            ']' => {
                // OSC: consume through BEL or ST (ESC \).
                j += 1;
                while j < chars.len() {
                    if chars[j] == '\u{07}' {
                        return j + 1;
                    }
                    if chars[j] == '\u{1b}' && j + 1 < chars.len() && chars[j + 1] == '\\' {
                        return j + 2;
                    }
                    j += 1;
                }
                j
            }
            '(' | ')' => (j + 2).min(chars.len()),
            _ => j + 1,
        }
    }

    fn csi(&mut self, final_c: char, params: &str) {
        if params.starts_with('?') {
            return; // private modes (?25h/l cursor, ?2026h/l sync, ?7h/l wrap) — no screen effect here
        }
        let nums: Vec<usize> =
            params.split(';').map(|p| p.parse::<usize>().unwrap_or(0)).collect();
        let p0 = nums.first().copied().unwrap_or(0);
        match final_c {
            'H' | 'f' => {
                let row = nums.first().copied().filter(|&n| n > 0).unwrap_or(1);
                let col = nums.get(1).copied().filter(|&n| n > 0).unwrap_or(1);
                self.cy = (row - 1).min(self.h.saturating_sub(1));
                self.cx = (col - 1).min(self.w);
            }
            'J' => match p0 {
                0 => self.clear_from_cursor_down(),
                2 => self.clear_all(),
                3 => {
                    self.scrollback.clear();
                    self.clear_all();
                }
                _ => {}
            },
            'K' => {
                if p0 == 0 {
                    self.blank_row_from(self.cy, self.cx);
                } else if p0 == 2 {
                    self.blank_row_from(self.cy, 0);
                }
            }
            'A' => self.cy = self.cy.saturating_sub(p0.max(1)),
            'B' => self.cy = (self.cy + p0.max(1)).min(self.h.saturating_sub(1)),
            'C' => self.cx = (self.cx + p0.max(1)).min(self.w),
            'D' => self.cx = self.cx.saturating_sub(p0.max(1)),
            _ => {} // SGR 'm', scroll 'S'/'T', region 'r', etc — no effect on the text we count
        }
    }

    /// Every row the user could see (scrollback history above + the current visible grid), as strings.
    fn all_rows(&self) -> Vec<String> {
        self.scrollback
            .iter()
            .chain(self.grid.iter())
            .map(|row| row.iter().collect::<String>())
            .collect()
    }
}

/// Replay a captured escape stream and return the reconstructed rows (scrollback + visible).
fn replay(bytes: &[u8]) -> Vec<String> {
    let mut screen = Screen::new(W as usize, H as usize);
    screen.feed(bytes);
    screen.all_rows()
}

/// Count rows in which `needle` appears (the hint bar / a committed line lands on a single row, so a
/// per-row substring count is the "how many copies does the user see" measure).
fn rows_containing(rows: &[String], needle: &str) -> usize {
    rows.iter().filter(|r| r.contains(needle)).count()
}

/// Count the editor's horizontal rule rows (a full-width `─` run; the editor draws exactly one top +
/// one bottom rule, so a single un-stacked editor yields 2).
fn rule_rows(rows: &[String]) -> usize {
    rows.iter().filter(|r| r.chars().filter(|&c| c == '─').count() >= 10).count()
}

/// Count non-overlapping-ish occurrences of a byte subsequence (used to sanity-check that an erase
/// escape was emitted per reconstruction).
fn count_subseq(hay: &[u8], needle: &[u8]) -> usize {
    if needle.is_empty() || hay.len() < needle.len() {
        return 0;
    }
    hay.windows(needle.len()).filter(|w| *w == needle).count()
}

// --------------------------------------------------------------------------- driving helpers ----

fn key(code: KeyCode) -> InputEvent {
    InputEvent::Key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn app() -> App<CaptureBackend> {
    let mut app = App::new(CaptureBackend::new(), UiTheme::dark()).unwrap();
    // A seeded model keeps the footer stable across frames (mirrors the binary's `seed_footer`).
    app.status_mut().set_model("anthropic/claude-opus-4-8");
    app
}

// ============================================================================= the harness ====

/// GROWTH with the keybinding-hints bar visible (the exact reported bug: hint bars stacking as the
/// live region grows). Drive an empty app, then grow a multi-line buffer over several frames so the
/// content-sized inline viewport is reconstructed each frame. Replay the captured pty stream and
/// assert the user sees ONE hint bar and ONE editor (2 rule rows) — not a stack.
#[test]
fn hint_bar_and_editor_rules_do_not_stack_as_the_live_region_grows() {
    let mut app = app();
    app.draw().unwrap(); // frame 0: empty, viewport = 6, startup hints shown

    // Grow the editor 2..=8 logical lines → viewport 7..=13 → 7 reconstructions, hints stay visible.
    for n in 2..=8u16 {
        let text: String =
            (1..=n).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        app.editor_mut().set_text(&text);
        app.draw().unwrap();
    }

    let rows = replay(&app.terminal().backend().bytes());
    let interrupts = rows_containing(&rows, "interrupt");
    let rules = rule_rows(&rows);

    assert_eq!(
        interrupts, 1,
        "keybinding-hints bar STACKED: the user sees {interrupts} `interrupt` hint bars, expected 1.\n{}",
        rows.join("\n")
    );
    assert_eq!(
        rules, 2,
        "editor rules STACKED: {rules} rule rows on screen, expected 2 (one editor's top+bottom).\n{}",
        rows.join("\n")
    );
    // Sanity: the growth really did drive several reconstructions (each one erases-then-reserves).
    let erases = count_subseq(&app.terminal().backend().bytes(), b"\x1b[J");
    assert!(erases >= 6, "expected an erase per reconstruction, saw {erases}");
}

/// GROWTH then SHRINK with a streaming assistant turn (the task's push_user → stream → commit flow).
/// The active turn grows over several frames (viewport reconstructs), then commits (viewport shrinks +
/// the answer flushes to native scrollback via `insert_before`). Assert: the editor never stacks (2
/// rule rows) across grow AND shrink, the committed answer reaches scrollback exactly once, and — at
/// the streaming peak — the newest text + stream caret are visible.
#[test]
fn streaming_turn_grows_and_commits_without_stacking_chrome() {
    let mut app = app();
    app.draw().unwrap();

    // A real submission: dismisses the startup hints and pushes the user turn (flushed to scrollback).
    app.editor_mut().set_text("explain the fix");
    app.handle_input(&key(KeyCode::Enter));

    // Stream a growing multi-line answer; each newline grows the wrapped height → reconstruction.
    let mut peak_bytes = Vec::new();
    for i in 1..=8u16 {
        app.transcript_mut().push_assistant_delta(&format!("answer line {i}\n"));
        app.draw().unwrap();
        peak_bytes = app.terminal().backend().bytes();
    }
    // The last streamed delta carries a sentinel that word-wrap cannot split.
    app.transcript_mut().push_assistant_delta("final tail SENTINELZZ");
    app.draw().unwrap();
    let at_peak = replay(&app.terminal().backend().bytes());
    let _ = peak_bytes;

    // (peak) The newest text is visible and the editor has NOT stacked.
    assert!(
        rows_containing(&at_peak, "SENTINELZZ") >= 1,
        "newest streaming text missing (should be tail-anchored + visible):\n{}",
        at_peak.join("\n")
    );
    // X1: pi draws no stream caret (`assistant-message.ts:104-114`); the only caret in the TUI is
    // the editor's reverse-video cell (`editor.ts:545-564`).
    assert!(
        !at_peak.iter().any(|r| r.contains('▌')),
        "invented stream caret `▌` at the streaming peak:\n{}",
        at_peak.join("\n")
    );
    assert_eq!(
        rule_rows(&at_peak),
        2,
        "editor rules STACKED during streaming growth: {} rule rows, expected 2:\n{}",
        rule_rows(&at_peak),
        at_peak.join("\n")
    );

    // Commit → the answer drains to native scrollback (`insert_before`) and the viewport SHRINKS.
    app.transcript_mut().commit_assistant(None);
    app.draw().unwrap();
    // A couple of idle frames after the shrink (steady state).
    app.draw().unwrap();

    let after = replay(&app.terminal().backend().bytes());
    assert_eq!(
        rule_rows(&after),
        2,
        "editor rules STACKED across grow+shrink: {} rule rows, expected 2:\n{}",
        rule_rows(&after),
        after.join("\n")
    );
    // The committed answer reached the terminal's scrollback exactly once (not clipped, not duplicated).
    assert_eq!(
        rows_containing(&after, "SENTINELZZ"),
        1,
        "committed answer should appear once in scrollback, saw {}:\n{}",
        rows_containing(&after, "SENTINELZZ"),
        after.join("\n")
    );
    // The in-memory flush accumulator agrees (test-visible mirror of the `insert_before` payload).
    assert!(app.scrollback_text().contains("SENTINELZZ"), "flush accumulator missing the answer");
}

// -------------------------------------------------------- the replay itself catches stacking ----

/// Guard the guard: feed the VT model a hand-built stream that draws the hint bar at the bottom, then
/// scrolls it up WITHOUT erasing (the pre-fix behavior) and redraws it — twice. The replay must report
/// the duplicates. This proves the harness above would FAIL if the erase-before-reconstruct fix
/// regressed (it is exactly the raw shape the buggy reconstruction produced).
#[test]
fn replay_detects_stacking_in_a_raw_stream() {
    let mut raw: Vec<u8> = Vec::new();
    // Draw "esc interrupt" on the bottom row, then push it up with 2 newlines (scroll) and redraw —
    // three times over, mimicking three un-erased reconstructions.
    for _ in 0..3 {
        raw.extend_from_slice(format!("\x1b[{};1H", H).as_bytes()); // CUP bottom row
        raw.extend_from_slice(b"esc interrupt");
        raw.extend_from_slice(b"\r\n\n"); // scroll up 2
    }
    let rows = replay(&raw);
    assert!(
        rows_containing(&rows, "interrupt") >= 3,
        "replay failed to see stacked hint bars: {}",
        rows_containing(&rows, "interrupt")
    );
}
