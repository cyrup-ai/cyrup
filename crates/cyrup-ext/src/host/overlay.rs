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
//! Both halves now exist and serve different TIERS. A native extension implements
//! [`InteractiveOverlay`] directly and hands it to `open_overlay`. A WASM guest cannot — a `dyn`
//! trait object does not cross a component boundary — so it sends a [`CustomSpec`] through
//! `HostServices::custom`, which the host turns into a [`SpecOverlay`] (an `InteractiveOverlay`
//! like any other) and drives on the guest's behalf. One renderer, two producers.
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

use std::sync::{Arc, Mutex};

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
        Self {
            text: text.into(),
            ..Self::default()
        }
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
        Self {
            code,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// A `Ctrl`-modified keystroke.
    #[must_use]
    pub fn ctrl(code: OverlayKeyCode) -> Self {
        Self {
            code,
            ctrl: true,
            alt: false,
            shift: false,
        }
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
/// pi's `overlayOptions` (`interactive-mode.ts:2719`), carried **with the factory** rather than
/// through `open_overlay`.
///
/// `open_overlay` is a `HostServices` trait method with a default body, a `LiveHostServices` impl
/// and a `fenced!` macro arm whose arity the macro fixes — widening it would edit five sites across
/// three crates to carry a value the component already knows. Upstream puts it on the factory for
/// the same reason: `ctx.ui.custom(factory, { overlay: true, overlayOptions })`.
///
/// The defaults are `cyrup-tui`'s four existing constants, so every overlay that does not override
/// [`InteractiveOverlay::options`] is painted exactly as before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayOptions {
    /// Fixed column count (pi `width: 82 | 92`), or `None` for the percentage default.
    pub width: Option<u16>,
    /// pi `width: "95%"`.
    pub width_pct: u16,
    /// pi `minWidth: 60`.
    pub min_width: u16,
    /// pi `maxHeight: "85%"`.
    pub max_height_pct: u16,
    /// pi `margin: 1`.
    pub margin: u16,
}

impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            width: None,
            width_pct: 95,
            min_width: 60,
            max_height_pct: 85,
            margin: 1,
        }
    }
}

impl OverlayOptions {
    /// The row budget the host will paint into, given the FRAME height.
    ///
    /// Called by the adapter's `box_rect` **and** by an overlay that windows its own body
    /// (MCP-377), so the two cannot disagree — which is the whole reason the height half and the
    /// width half are one change.
    #[must_use]
    pub fn max_rows(&self, frame_height: u16) -> u16 {
        let usable = frame_height.saturating_sub(self.margin.saturating_mul(2));
        let capped = u32::from(frame_height)
            .saturating_mul(u32::from(self.max_height_pct))
            .checked_div(100)
            .unwrap_or(0);
        u16::try_from(capped).unwrap_or(u16::MAX).min(usable).max(1)
    }
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

    /// Whether the overlay has decided to close itself **without a keystroke** — pi's
    /// `setTimeout(() => done(...), INACTIVITY_MS)`.
    ///
    /// [`Self::tick`] returns `bool` (did anything change?) and structurally cannot express "tear me
    /// down", which is why an inactivity deadline could previously only be honoured on the next key
    /// press — leaving an expired panel painted. The host consults this after every tick and drops
    /// the overlay when it answers `true`.
    ///
    /// Defaulted to `false`, so every existing overlay is unaffected.
    fn should_close(&self) -> bool {
        false
    }

    /// This overlay's geometry (pi's `overlayOptions`). Defaulted, so existing overlays are
    /// untouched.
    fn options(&self) -> OverlayOptions {
        OverlayOptions::default()
    }
}

// ---------------------------------------------------------------------------
// The WASM tier's `ui.custom`: a SERIALIZED component spec
// ---------------------------------------------------------------------------

/// One selectable row of a [`CustomSpec`].
///
/// Accepts either a bare string (`"restart"` — the id doubling as the label) or an object
/// (`{"id": "restart", "label": "Restart the server"}`). Both spellings exist because a guest
/// writing the spec has no cyrup type to build it from: `cyrup-ext-sdk`'s
/// `ctx.ui.custom(spec: impl Serialize)` takes arbitrary JSON and the SDK deliberately does not
/// depend on this crate (its `Cargo.toml` carries only `serde`/`serde_json`/`wit-bindgen`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct CustomOption {
    /// The value handed back to the guest when this row is chosen.
    pub id: String,
    /// What the human reads; falls back to [`Self::id`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl CustomOption {
    /// An id-only row.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: None,
        }
    }

    /// The display text — [`Self::label`] when set, else [`Self::id`].
    #[must_use]
    pub fn display(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

impl<'de> Deserialize<'de> for CustomOption {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Bare(String),
            Rich {
                id: String,
                #[serde(default)]
                label: Option<String>,
            },
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Bare(id) => CustomOption { id, label: None },
            Raw::Rich { id, label } => CustomOption { id, label },
        })
    }
}

/// The component a WASM guest describes to `ui.custom(spec-json)` (`world.wit`'s
/// `custom: func(spec-json: string) -> option<string>`).
///
/// **[CYRUP-DELTA]** against pi `custom<T>(factory, options?)`
/// (`core/extensions/types.ts:196` @v0.83.0, "Show a custom component with keyboard focus"): pi
/// hands the TUI a component FACTORY that the draw path re-invokes, and a factory cannot cross a
/// WASM component boundary — the SAME collapse `set-header`/`set-footer` already carry an explicit
/// `[CYRUP-DELTA]` for in `crates/cyrup-ext/wit/world.wit` (factory → rendered data). The guest
/// therefore describes what it wants painted, once, and the host owns the keyboard.
///
/// This is the WASM tier ONLY. A NATIVE extension never travels this path: it hands the host a real
/// `Box<dyn `[`InteractiveOverlay`]`>` through [`super::services::HostServices::open_overlay`] —
/// the same route pi's `{ overlay: true }` branch takes — and keeps full per-keystroke control.
/// `cyrup-ext-subagents`' fleet modal and `cyrup-permission-system`'s settings modal are both live
/// users of that route.
///
/// Every field is optional; an all-empty spec is a no-op the host declines to open
/// ([`Self::is_empty`]).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomSpec {
    /// A header row, painted bold. `None` draws no header — pi's own components paint their own
    /// title when they want one (`pi-subagents/src/tui/fleet.ts:809`), so this is a convenience,
    /// not a frame the host imposes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The body, one plain-text row per entry. Not wrapped: the host clips to its own anchored box,
    /// exactly as it does for a native overlay that returns more rows than fit
    /// ([`InteractiveOverlay::render`]'s "lossless-by-design" note).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lines: Vec<String>,
    /// Selectable rows. Non-empty makes the overlay a chooser: `Up`/`Down` move, `Enter` resolves
    /// `ui.custom` to the highlighted [`CustomOption::id`], `Esc` resolves it to `none`. Empty makes
    /// it a read-only panel that either key dismisses with `none`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<CustomOption>,
}

impl CustomSpec {
    /// Read a spec off the guest's JSON. A malformed or non-object payload yields the empty spec
    /// (which [`Self::is_empty`] rejects) rather than an error, because the WIT return type is a
    /// bare `option<string>` with no error arm to carry a diagnostic.
    #[must_use]
    pub fn from_json(spec: &serde_json::Value) -> Self {
        serde_json::from_value(spec.clone()).unwrap_or_default()
    }

    /// Whether there is nothing at all to paint — no title, no body, no options.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none() && self.lines.is_empty() && self.options.is_empty()
    }

    /// Build the driveable overlay plus the cell its result lands in. The host keeps the cell,
    /// hands the box to its renderer, and reads the cell once the modal closes.
    #[must_use]
    pub fn into_overlay(self) -> (SpecOverlay, Arc<Mutex<Option<String>>>) {
        let result = Arc::new(Mutex::new(None));
        (
            SpecOverlay {
                spec: self,
                selected: 0,
                result: Arc::clone(&result),
            },
            result,
        )
    }
}

/// The host-side [`InteractiveOverlay`] a [`CustomSpec`] becomes — the WASM tier's stand-in for the
/// component pi's `custom(factory)` would have built.
pub struct SpecOverlay {
    spec: CustomSpec,
    /// Index into [`CustomSpec::options`]; meaningless (and never moved) when there are none.
    selected: usize,
    /// Where the chosen [`CustomOption::id`] is published for the blocked `ui.custom` caller.
    result: Arc<Mutex<Option<String>>>,
}

impl InteractiveOverlay for SpecOverlay {
    fn render(&mut self, _width: usize, _height: usize) -> Vec<OverlayLine> {
        let mut out = Vec::with_capacity(
            usize::from(self.spec.title.is_some())
                + self.spec.lines.len()
                + self.spec.options.len(),
        );
        if let Some(title) = &self.spec.title {
            out.push(OverlayLine::new(vec![OverlaySpan {
                text: title.clone(),
                bold: true,
                ..OverlaySpan::default()
            }]));
        }
        for line in &self.spec.lines {
            out.push(OverlayLine::new(vec![OverlaySpan::raw(line.clone())]));
        }
        for (i, opt) in self.spec.options.iter().enumerate() {
            let picked = i == self.selected;
            // The two-column gutter is what distinguishes the highlighted row when the terminal
            // renders no reverse video; the reversed span is the primary cue.
            out.push(OverlayLine::new(vec![
                OverlaySpan::raw(if picked { "> " } else { "  " }),
                OverlaySpan {
                    text: opt.display().to_string(),
                    reversed: picked,
                    ..OverlaySpan::default()
                },
            ]));
        }
        out
    }

    fn handle_key(&mut self, key: OverlayKey) -> OverlayOutcome {
        let last = self.spec.options.len().saturating_sub(1);
        match key.code {
            OverlayKeyCode::Up if !self.spec.options.is_empty() => {
                self.selected = self.selected.saturating_sub(1);
                OverlayOutcome::Redraw
            }
            OverlayKeyCode::Down if !self.spec.options.is_empty() => {
                self.selected = self.selected.saturating_add(1).min(last);
                OverlayOutcome::Redraw
            }
            OverlayKeyCode::Home if !self.spec.options.is_empty() => {
                self.selected = 0;
                OverlayOutcome::Redraw
            }
            OverlayKeyCode::End if !self.spec.options.is_empty() => {
                self.selected = last;
                OverlayOutcome::Redraw
            }
            OverlayKeyCode::Enter => {
                // `done(result)` with a value — pi's resolve path. A read-only panel has no value to
                // carry, so Enter dismisses it exactly as Esc does.
                if let Some(opt) = self.spec.options.get(self.selected)
                    && let Ok(mut slot) = self.result.lock()
                {
                    *slot = Some(opt.id.clone());
                }
                OverlayOutcome::Close
            }
            // `done(undefined)` — the result cell is left untouched, so the caller sees `None`.
            OverlayKeyCode::Escape => OverlayOutcome::Close,
            _ => OverlayOutcome::Ignored,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
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
            OverlaySpan {
                text: "cd".into(),
                bold: true,
                ..OverlaySpan::default()
            },
        ]);
        assert_eq!(line.plain_text(), "abcd");
    }

    // These four cover the NEW `ui.custom` spec vocabulary. They are coverage, not the seam proof:
    // the types did not exist before the fix, so they cannot go red against the unfixed tree. The
    // test that genuinely fails without the fix is
    // `cyrup_session_svc::host_services::tests::custom_drives_a_guest_spec_through_the_overlay_renderer`,
    // which asserts that `LiveHostServices::custom` reaches a real overlay renderer at all.

    #[test]
    fn an_option_reads_from_both_a_bare_string_and_an_object() {
        let spec: CustomSpec = serde_json::from_str(
            r#"{"lines":["pick one"],"options":["staging",{"id":"prod","label":"production"}]}"#,
        )
        .unwrap();
        assert_eq!(spec.options[0], CustomOption::new("staging"));
        assert_eq!(
            spec.options[0].display(),
            "staging",
            "a bare string labels itself"
        );
        assert_eq!(spec.options[1].display(), "production");
        assert_eq!(
            spec.options[1].id, "prod",
            "the ID is what comes back, not the label"
        );
    }

    #[test]
    fn an_unusable_spec_is_empty_rather_than_an_error() {
        // No error arm exists on the WIT return, so every unusable shape lands on the same value.
        for raw in ["null", "{}", "[1,2,3]", r#""just a string""#, "17"] {
            let v: serde_json::Value = serde_json::from_str(raw).unwrap();
            assert!(
                CustomSpec::from_json(&v).is_empty(),
                "{raw} must be declined"
            );
        }
        assert!(!CustomSpec::from_json(&serde_json::json!({"lines": ["x"]})).is_empty());
    }

    #[test]
    fn enter_publishes_the_selected_id_and_escape_publishes_nothing() {
        let spec = CustomSpec::from_json(&serde_json::json!({
            "options": ["a", "b", "c"],
        }));
        let (mut ov, result) = spec.clone().into_overlay();
        // Down past the end clamps rather than wrapping or panicking.
        for _ in 0..9 {
            assert_eq!(
                ov.handle_key(OverlayKey::plain(OverlayKeyCode::Down)),
                OverlayOutcome::Redraw
            );
        }
        assert_eq!(
            ov.handle_key(OverlayKey::plain(OverlayKeyCode::Enter)),
            OverlayOutcome::Close
        );
        assert_eq!(result.lock().unwrap().as_deref(), Some("c"));

        let (mut ov, result) = spec.into_overlay();
        assert_eq!(
            ov.handle_key(OverlayKey::plain(OverlayKeyCode::Escape)),
            OverlayOutcome::Close
        );
        assert_eq!(
            result.lock().unwrap().as_deref(),
            None,
            "Esc is pi's `done(undefined)`"
        );
    }

    #[test]
    fn a_read_only_panel_ignores_navigation_and_resolves_to_nothing() {
        let (mut ov, result) =
            CustomSpec::from_json(&serde_json::json!({"lines": ["read me"]})).into_overlay();
        assert_eq!(
            ov.handle_key(OverlayKey::plain(OverlayKeyCode::Down)),
            OverlayOutcome::Ignored,
            "no options ⇒ nothing to move between"
        );
        assert_eq!(
            ov.handle_key(OverlayKey::plain(OverlayKeyCode::Enter)),
            OverlayOutcome::Close
        );
        assert_eq!(
            result.lock().unwrap().as_deref(),
            None,
            "there is no value to carry back"
        );
        assert_eq!(
            ov.render(40, 10)
                .iter()
                .map(OverlayLine::plain_text)
                .collect::<Vec<_>>(),
            vec!["read me".to_string()]
        );
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

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod overlay_options_tests {
    use super::OverlayOptions;

    /// The default is pi's `{ width: "95%", minWidth: 60, maxHeight: "85%", margin: 1 }`, so every
    /// overlay that does not override `options()` is painted exactly as it was before MCP-368.
    #[test]
    fn the_default_is_pis_four_constants() {
        let options = OverlayOptions::default();
        assert_eq!(options.width, None);
        assert_eq!(options.width_pct, 95);
        assert_eq!(options.min_width, 60);
        assert_eq!(options.max_height_pct, 85);
        assert_eq!(options.margin, 1);
    }

    /// `max_rows` is the ONE row budget MCP-368's box height and MCP-377's body windowing share.
    #[test]
    fn max_rows_applies_the_percentage_then_the_margin() {
        let options = OverlayOptions::default();
        // 85% of 40 is 34; the margin leaves 38 usable, so the percentage binds.
        assert_eq!(options.max_rows(40), 34);
        // A frame so short the margin binds instead: 85% of 4 is 3, usable is 2.
        assert_eq!(options.max_rows(4), 2);
        // Never zero — an unusable overlay is worse than a cramped one.
        assert_eq!(options.max_rows(1), 1);
        assert_eq!(options.max_rows(0), 1);
    }
}
