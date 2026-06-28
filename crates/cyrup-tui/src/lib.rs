//! cyrup-tui — the terminal UI front-end (arch-10; conformance: func-10; binds ADR-0001).
//!
//! ratatui + crossterm: inline viewport + `insert_before` scrollback, synchronized output, a
//! retained component layer over immediate mode, overlays, the editor, inline images, theming.
//!
//! Scaffold stub (ratatui/crossterm added during arch-10 implementation).

/// TUI error (arch-10 §8). Scaffold placeholder.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    #[error("not yet implemented: {0}")]
    Unimplemented(&'static str),
}
