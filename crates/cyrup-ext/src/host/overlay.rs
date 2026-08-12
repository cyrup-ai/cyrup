//! The **interactive-overlay seam** (arch-08 §3.6, the `ui.custom` half): a host-driven,
//! focus-capturing modal an extension owns the state of and the host owns the terminal for.
//!
//! # Why this exists
//!
//! Pi's extension UI has `ctx.ui.custom(factory, { overlay: true, … })`
//! (`coding-agent/src/core/extensions/types.ts`; the only `showOverlay` consumer at v0.84.1 is
//! `interactive-mode.ts:2719`, this exact path). The factory returns a `pi-tui` `Component` —
//! `render(width): string[]` plus `handleInput(data: string)` — and the TUI drives it until the
//! component calls `done(...)`. `pi-subagents`' fleet inspector (`src/tui/fleet.ts:869-875`) is a
//! real user of it.
//!
//! cyrup's counterpart used to be [`super::services::HostServices::custom`], which takes a
//! serialized SPEC and returns an optional serialized RESULT — a one-shot, non-interactive
//! request/reply with no input subscription and no re-render channel. Nothing could be driven
//! through it, so every ported interactive component in every extension crate was dead above its
//! first `render` call.
//!
//! # The contract
//!
//! [`InteractiveOverlay`] is that missing half, object-safe so the host holds a
//! `Box<dyn InteractiveOverlay>` with no knowledge of what is inside it:
//!
//! * [`InteractiveOverlay::render`] — the extension paints, in columns/rows the host reports,
//!   returning [`OverlayLine`]s;
//! * [`InteractiveOverlay::handle_key`] — the host routes one keystroke and is told whether to
//!   repaint, ignore, or close ([`OverlayOutcome`]);
//! * [`InteractiveOverlay::tick`] — the extension's own refresh cadence
//!   ([`InteractiveOverlay::refresh_ms`]), because pi's components arm a `setInterval` in their
//!   constructor (`fleet.ts:516-521`) and cyrup has no timer inside an extension.
//!
//! # Why lines and not `ratatui::text::Line`
//!
//! `cyrup-ext` deliberately does not depend on `cyrup-tui` (the module header of `cyrup-ext`'s own
//! `Cargo.toml` states it), and "extension UI crosses as serializable commands" is the
//! architecture's rule for this boundary. [`OverlayLine`]/[`OverlaySpan`]/[`OverlayColor`] are
//! therefore a plain, `serde`-able mirror of a styled terminal line: enough fidelity to carry
//! every colour and modifier a ratatui `Span` can (16 ANSI colours, 256-colour indices, truecolor,
//! and the five modifiers cyrup's renderers actually set), and nothing that requires a rendering
//! library on either side. Both ends convert; neither end depends on the other.
//!
//! # No ambient authority
//!
//! [`super::services::HostServices::open_overlay`] defaults to `false` — the default host runs no
//! terminal and accepts no overlay. Only a backend that owns a live interactive surface
//! (`cyrup-session-svc`'s `LiveHostServices`, wired by `cyrup-tui`'s run loop) accepts one.

use serde::{Deserialize, Serialize};

/// A terminal colour, as a serializable mirror of `ratatui::style::Color`'s inhabited variants.
///
/// `Reset` is deliberately absent: an [`OverlaySpan`] expresses "no colour" as
/// [`OverlaySpan::fg`] `== None`, so there is exactly one representation of the default
/// foreground rather than two that a renderer would have to treat alike.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayColor {
    /// ANSI 0.
    Black,
    /// ANSI 1.
    Red,
    /// ANSI 2.
    Green,
    /// ANSI 3.
    Yellow,
    /// ANSI 4.
    Blue,
    /// ANSI 5.
    Magenta,
    /// ANSI 6.
    Cyan,
    /// ANSI 7.
    Gray,
    /// ANSI 8.
    DarkGray,
    /// ANSI 9.
    LightRed,
    /// ANSI 10.
    LightGreen,
    /// ANSI 11.
    LightYellow,
    /// ANSI 12.
    LightBlue,
    /// ANSI 13.
    LightMagenta,
    /// ANSI 14.
    LightCyan,
    /// ANSI 15.
    White,
    /// A 256-colour palette index.
    Indexed(u8),
    /// A 24-bit truecolor triple.
    Rgb(u8, u8, u8),
}

/// One styled run of text inside an [`OverlayLine`].
///
/// The five modifiers are exactly the ones cyrup's own renderers set (`bold`, `dim`, `italic`,
/// `underlined`, `reversed`); anything else a future renderer wants is an additive field here, not
/// a reinterpretation of an existing one.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySpan {
    /// The run's text. Never contains a newline — a line break is a new [`OverlayLine`].
    pub text: String,
    /// The foreground colour, or `None` for the terminal default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fg: Option<OverlayColor>,
    /// The background colour, or `None` for the terminal default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bg: Option<OverlayColor>,
    /// Bold.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bold: bool,
    /// Dim / faint.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dim: bool,
    /// Italic.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub italic: bool,
    /// Underlined.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub underlined: bool,
    /// Reverse video.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub reversed: bool,
}

impl OverlaySpan {
    /// An unstyled run.
    #[must_use]
    pub fn raw(text: impl Into<String>) -> Self {
        Self { text: text.into(), ..Self::default() }
    }
}

/// One painted row of an overlay — a list of styled runs, in left-to-right order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverlayLine {
    /// The row's runs.
    pub spans: Vec<OverlaySpan>,
}

impl OverlayLine {
    /// A row built from already-styled runs.
    #[must_use]
    pub fn new(spans: Vec<OverlaySpan>) -> Self {
        Self { spans }
    }

    /// The row's plain text, styles discarded — for logging, tests and non-terminal surfaces.
    #[must_use]
    pub fn plain_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// A key the host routed to an overlay, as the small closed set an overlay can act on.
///
/// This is deliberately NOT crossterm's `KeyCode`: `cyrup-ext` must stay free of the terminal
/// backend, and an overlay only ever needs the navigation/editing set plus printable characters.
/// An unmapped key never reaches the overlay at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayKeyCode {
    /// A printable character (already shift-resolved by the host: `Shift+k` arrives as `'K'`).
    Char(char),
    /// Return / Enter.
    Enter,
    /// Escape.
    Escape,
    /// Backspace.
    Backspace,
    /// Delete.
    Delete,
    /// Tab.
    Tab,
    /// Shift+Tab.
    BackTab,
    /// Cursor up.
    Up,
    /// Cursor down.
    Down,
    /// Cursor left.
    Left,
    /// Cursor right.
    Right,
    /// Home.
    Home,
    /// End.
    End,
    /// Page up.
    PageUp,
    /// Page down.
    PageDown,
    /// Insert.
    Insert,
    /// A function key (`F(1)` … `F(12)`).
    F(u8),
}

/// One keystroke, with its modifiers.
///
/// `shift` is reported for completeness, but a printable character arrives already shift-resolved
/// in [`OverlayKeyCode::Char`] (the host uppercases where the terminal did), because that is how
/// pi's own `matchesKey(data, Key.shift("k"))` distinguishes `K` from `k` — off the raw byte, not
/// off a modifier bit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlayKey {
    /// Which key.
    pub code: OverlayKeyCode,
    /// Control held.
    pub ctrl: bool,
    /// Alt / Meta held.
    pub alt: bool,
    /// Shift held.
    pub shift: bool,
}

impl OverlayKey {
    /// An unmodified keystroke.
    #[must_use]
    pub fn plain(code: OverlayKeyCode) -> Self {
        Self { code, ctrl: false, alt: false, shift: false }
    }

    /// A `Ctrl`-modified keystroke.
    #[must_use]
    pub fn ctrl(code: OverlayKeyCode) -> Self {
        Self { code, ctrl: true, alt: false, shift: false }
    }
}

/// What the host must do after routing one key to an overlay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverlayOutcome {
    /// The key changed nothing; no repaint required.
    Ignored,
    /// State changed; repaint (pi `this.tui.requestRender()`).
    Redraw,
    /// The overlay asked to close (pi `this.done(...)`); the host tears it down.
    Close,
}

/// An extension-owned, focus-capturing modal the host paints and feeds keystrokes to — cyrup's
/// counterpart of pi's `ctx.ui.custom(factory, { overlay: true, … })` `Component`.
///
/// The host owns the terminal; the implementor owns all state. The host guarantees:
/// * [`Self::render`] is called with the exact area it will paint into, before each frame;
/// * [`Self::handle_key`] is called for every keystroke while the overlay is topmost, and for no
///   keystroke after it returns [`OverlayOutcome::Close`];
/// * [`Self::tick`] is called no more often than [`Self::refresh_ms`] asks for (and never when
///   that is `0`).
pub trait InteractiveOverlay: Send {
    /// Paint the overlay.
    ///
    /// `width` is the exact column count the host has reserved and will paint at. `height` is the
    /// host FRAME's row count — the terminal's, not the modal's — because that is what pi's own
    /// components read (`this.tui.terminal?.rows`, `pi-subagents/src/tui/fleet.ts:791`) to decide
    /// how tall to draw themselves; the host then clips whatever comes back to its own anchored
    /// box. Returning more rows than the host can show is therefore normal and lossless-by-design,
    /// not an error.
    fn render(&mut self, width: usize, height: usize) -> Vec<OverlayLine>;

    /// Route one keystroke.
    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome;

    /// The self-refresh cadence in milliseconds, or `0` for "never tick me" (the default).
    fn refresh_ms(&self) -> u64 {
        0
    }

    /// Advance whatever the overlay polls on its own cadence. Returns `true` when the next frame
    /// would differ, i.e. when the host should repaint. Default: nothing to advance.
    ///
    /// There is deliberately no `title()` alongside these: pi's `Component` has none either
    /// (`pi/packages/tui/src/component.ts` is `render` + `handleInput`), and the frame's own header
    /// is something the component paints for itself (`fleet.ts:809`).
    fn tick(&mut self) -> bool {
        false
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn a_span_round_trips_through_json_with_absent_defaults() {
        let span = OverlaySpan {
            text: "hi".into(),
            fg: Some(OverlayColor::Cyan),
            bold: true,
            ..OverlaySpan::default()
        };
        let json = serde_json::to_string(&span).unwrap();
        // The four unset modifiers and the unset background must not appear at all.
        assert_eq!(json, r#"{"text":"hi","fg":"cyan","bold":true}"#);
        assert_eq!(serde_json::from_str::<OverlaySpan>(&json).unwrap(), span);
    }

    #[test]
    fn truecolor_and_indexed_survive_the_wire() {
        for color in [OverlayColor::Rgb(1, 2, 3), OverlayColor::Indexed(200)] {
            let json = serde_json::to_string(&color).unwrap();
            assert_eq!(serde_json::from_str::<OverlayColor>(&json).unwrap(), color);
        }
    }

    #[test]
    fn plain_text_concatenates_spans_in_order() {
        let line = OverlayLine::new(vec![
            OverlaySpan::raw("ab"),
            OverlaySpan { text: "cd".into(), bold: true, ..OverlaySpan::default() },
        ]);
        assert_eq!(line.plain_text(), "abcd");
    }

    #[test]
    fn the_default_trait_body_never_asks_for_a_tick() {
        struct Inert;
        impl InteractiveOverlay for Inert {
            fn render(&mut self, _w: usize, _h: usize) -> Vec<OverlayLine> {
                Vec::new()
            }
            fn handle_key(&mut self, _key: OverlayKey) -> OverlayOutcome {
                OverlayOutcome::Ignored
            }
        }
        let mut inert = Inert;
        assert_eq!(inert.refresh_ms(), 0);
        assert!(!inert.tick());
        assert_eq!(
            inert.handle_key(OverlayKey::plain(OverlayKeyCode::Escape)),
            OverlayOutcome::Ignored
        );
    }
}
