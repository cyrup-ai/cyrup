//! `TuiError` — the crate error type (arch-10 §8).
//!
//! `thiserror` per lib policy (arch-00 §8). No `unwrap`/`expect`/`panic`/indexing on any path
//! reachable from terminal input, model output, or theme files (R-00-009).

/// Errors surfaced by the terminal UI layer.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// The terminal backend (`ratatui::backend::Backend`) failed to draw/flush/size. Rendered as a
    /// string because the concrete `Backend::Error` differs per backend (crossterm vs. test).
    #[error("terminal backend error: {0}")]
    Backend(String),

    /// Terminal / process I/O failure (raw-mode toggle, stdout write).
    #[error("terminal io: {0}")]
    Io(#[from] std::io::Error),

    /// A component returned a line wider than the viewport — surfaced, never silently clipped
    /// (R-10-004 / R-ARCH-TUI-005).
    #[error("over-width line: {got} > {max}")]
    OverWidthLine { got: usize, max: u16 },

    /// A key spec string (`"ctrl+c"`) could not be parsed (R-10-023).
    #[error("invalid key spec: {0}")]
    KeySpec(String),

    /// A JSON keybindings document was malformed (spec/tui/07 §3.9; `core/keybindings.ts:14-262`).
    #[error("invalid keybindings json: {0}")]
    Keybindings(String),

    /// The run loop was cancelled (maps to `CoreError::Cancelled`).
    #[error("cancelled")]
    Cancelled,

    /// A shared-substrate error bubbled up.
    #[error(transparent)]
    Core(#[from] cyrup_core::CoreError),
}
