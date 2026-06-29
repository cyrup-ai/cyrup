//! ratatui `TestBackend` helpers + synthetic key driver (Pi `tui/test/{virtual-terminal.ts,
//! key-tester.ts}`; the crate's self-disclosed-deferred promise, lib.rs:5). Supports
//! differential-render tests (func-00 R-00-006): render a widget into an in-memory backend and
//! snapshot the resulting text grid; drive synthetic key events.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Position;

/// An in-memory terminal over ratatui's [`TestBackend`] (Pi virtual-terminal). Render into it, then
/// snapshot the text grid.
pub struct TestTerminal {
    terminal: Terminal<TestBackend>,
}

impl TestTerminal {
    /// A `width × height` in-memory terminal. `Terminal::new` over a [`TestBackend`] is infallible
    /// (returns `Result<_, Infallible>`), so this never panics and needs no error channel.
    pub fn new(width: u16, height: u16) -> Self {
        let backend = TestBackend::new(width.max(1), height.max(1));
        let terminal = match Terminal::new(backend) {
            Ok(t) => t,
            Err(never) => match never {},
        };
        Self { terminal }
    }

    /// Draw a frame (Pi virtual-terminal render). Errors from the backend are swallowed (the test
    /// backend does not fail).
    pub fn draw<F>(&mut self, render: F)
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        let _ = self.terminal.draw(render);
    }

    /// The current backend buffer.
    pub fn buffer(&self) -> &Buffer {
        self.terminal.backend().buffer()
    }

    /// The rendered text grid, one [`String`] per row, trailing spaces preserved.
    pub fn lines(&self) -> Vec<String> {
        buffer_lines(self.buffer())
    }

    /// The rendered text grid joined by newlines (snapshot-friendly).
    pub fn snapshot(&self) -> String {
        self.lines().join("\n")
    }
}

/// Extract the text grid from a [`Buffer`]: one row per [`String`] (Pi virtual-terminal text dump).
pub fn buffer_lines(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    let mut lines = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut row = String::new();
        for x in area.left()..area.right() {
            if let Some(cell) = buffer.cell(Position::new(x, y)) {
                row.push_str(cell.symbol());
            }
        }
        lines.push(row);
    }
    lines
}

// ---- synthetic key driver (Pi key-tester.ts) ----

/// A key event for a character (no modifiers).
pub fn char_key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

/// A key event for a non-character key code (no modifiers), e.g. [`KeyCode::Enter`].
pub fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A key event with explicit modifiers (Pi key-tester chords, e.g. ctrl+c).
pub fn key_with(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, modifiers)
}

/// Convert a string into a sequence of character key events (Pi `typeString`).
pub fn type_string(text: &str) -> Vec<KeyEvent> {
    text.chars().map(char_key).collect()
}
