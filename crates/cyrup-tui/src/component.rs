//! The retained component layer over ratatui's immediate mode (R-10-007 / R-ARCH-TUI-011).
//!
//! ratatui is immediate-mode (widgets re-issued each frame); our [`Component`]s own their state
//! across frames and are *rendered into* a `Frame`/`Rect` on each pass. This keeps the
//! func-10 component contract (state + render + invalidate) as the stable surface while ratatui
//! does the cell-level diffing underneath.

use ratatui::layout::Rect;
use ratatui::Frame;

use crate::theme::UiTheme;

/// A retained UI component: owns state, renders into an `area` of the current `frame`.
///
/// Object-safe so the chrome can hold `Box<dyn Component>` slots (header/footer/widgets later).
pub trait Component {
    /// Render this component into `area`. ratatui clips to `area`, so a component cannot corrupt
    /// cells outside its rect.
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme);

    /// Drop any cached render state (R-10-006). Default: no-op.
    fn invalidate(&mut self) {}
}

/// Terminal input delivered to the app/run loop. A thin projection of `crossterm::event::Event`
/// plus the bracketed-paste payload (arch-10 §3.7). The async crossterm `EventStream` feature is
/// not enabled in this build, so the production reader (see [`crate::app`]) feeds these from a
/// blocking `event::read()` task — the run loop is agnostic to the source.
#[derive(Clone, Debug)]
pub enum InputEvent {
    /// A key press (already filtered to `Press`/`Repeat` kinds by the reader).
    Key(ratatui::crossterm::event::KeyEvent),
    /// A bracketed paste (R-10-015 — large-paste handling is deferred, see arch-10 §12).
    Paste(String),
    /// Terminal resize → full re-render (R-10-001 strategy b).
    Resize(u16, u16),
    /// Terminal focus gained / lost.
    FocusGained,
    FocusLost,
    /// A mouse report, delivered only while the alternate screen has asked the terminal for them
    /// (ADR-0005 §B-4; the gate is `altscreen::mouse::reporting_enabled`). In regular mode cyrup
    /// enables no mouse mode at all, so this variant is never constructed there.
    Mouse(ratatui::crossterm::event::MouseEvent),
}
