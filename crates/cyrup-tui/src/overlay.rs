//! The floating overlay layer (spec/tui/05 §2; arch-10 §3.5 `OverlayManager`).
//!
//! Unlike the editor-swap selectors (which replace the input slot in place, spec/tui/05 §1.1), an
//! overlay is a true z-ordered floating modal drawn **on top of** the live region at an anchor/size,
//! capturing focus until it dismisses (`tui.ts:showOverlay`).
//!
//! **This layer currently has no implementor, and that is upstream-accurate.** Its only in-crate
//! consumer used to be a `HotkeysOverlay` that `/hotkeys` opened — a cyrup invention with no upstream
//! counterpart: `handleHotkeysCommand` appends a bordered block to the TRANSCRIPT
//! (interactive-mode.ts:6197-6203), and `git grep showOverlay v0.84.1 -- packages/` finds the call
//! ONLY in `tui/src/tui.ts` (the primitive), in `examples/extensions/overlay-qa-tests.ts`, and at
//! `interactive-mode.ts:2719` — the extension custom-UI path. So upstream's sole overlay consumer is
//! extension UI, which cyrup has not ported yet; the trait and the z-stack stay as its landing site
//! rather than being kept alive by a first-party popup pi does not have.
//!
//! Rendering: stack bottom→top, each overlay computes its `Rect` from the full
//! frame, erases the cells under it (`ratatui::widgets::Clear`), then draws into the box. Focus
//! routing (spec/tui/05 §2) delivers a key to the **topmost** overlay first; an unconsumed key
//! bubbles. Everything is pure ratatui layout over existing state — no new dependency.

use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::Frame;

use crate::theme::UiTheme;

/// The result of routing one key to an overlay (spec/tui/05 §2 step 2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayOutcome {
    /// The overlay handled the key and stays open (scroll, etc.) → redraw.
    Redraw,
    /// The overlay requested dismissal (`Esc`/`q`/`Enter`).
    Close,
    /// The key was not an overlay binding — the chrome may let it bubble.
    Ignored,
}

/// A floating, focus-capturing modal drawn over the live region. Object-safe so the chrome holds a
/// z-stack of `Box<dyn Overlay>`.
pub trait Overlay: Send {
    /// Render into the full-frame `area` (the overlay computes its own centered/anchored sub-`Rect`,
    /// clears it, and draws the box).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme);
    /// Route one key, returning the outcome.
    fn handle(&mut self, key: &KeyEvent) -> OverlayOutcome;
}
