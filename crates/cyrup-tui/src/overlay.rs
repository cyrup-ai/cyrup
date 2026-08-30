//! The floating overlay layer (spec/tui/05 §2; arch-10 §3.5 `OverlayManager`).
//!
//! Unlike the editor-swap selectors (which replace the input slot in place, spec/tui/05 §1.1), an
//! overlay is a true z-ordered floating modal drawn **on top of** the live region at an anchor/size,
//! capturing focus until it dismisses (`tui.ts:showOverlay`).
//!
//! **Its one upstream consumer is extension UI, and that is now wired.** `git grep showOverlay
//! v0.84.1 -- packages/` finds the call only in `tui/src/tui.ts` (the primitive), in
//! `examples/extensions/overlay-qa-tests.ts`, and at `interactive-mode.ts:2719` — the
//! `ctx.ui.custom(factory, { overlay: true, … })` path. `/hotkeys`, which cyrup once opened an
//! overlay for, is upstream a bordered block appended to the TRANSCRIPT
//! (interactive-mode.ts:6197-6203) and stays that way.
//!
//! [`ExtensionOverlay`] is that consumer: it adapts a `Box<dyn cyrup_ext::InteractiveOverlay>` — a
//! modal an extension owns the state of — onto this layer's [`Overlay`] trait, converting the
//! backend-free [`cyrup_ext::OverlayLine`] wire form into painted ratatui cells and crossterm key
//! events into the extension's [`cyrup_ext::OverlayKey`]. The extension never sees ratatui or
//! crossterm, and this crate never sees the extension's state.
//!
//! Rendering: stack bottom→top, each overlay computes its `Rect` from the full
//! frame, erases the cells under it (`ratatui::widgets::Clear`), then draws into the box. Focus
//! routing (spec/tui/05 §2) delivers a key to the **topmost** overlay first; an unconsumed key
//! bubbles. Everything is pure ratatui layout over existing state — no new dependency.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};
use ratatui::Frame;

use cyrup_ext::{
    InteractiveOverlay, OverlayOptions, OverlayColor, OverlayKey, OverlayKeyCode, OverlayLine,
    OverlayOutcome as ExtOverlayOutcome, OverlaySpan,
};

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
    /// The overlay's own refresh cadence in milliseconds, `0` for "never" (the default). The run
    /// loop arms one shared timer at the smallest non-zero value across the open stack.
    fn refresh_ms(&self) -> u64 {
        0
    }
    /// Advance whatever this overlay polls on its own cadence, returning `true` when the next frame
    /// would differ. Default: nothing to advance.
    fn tick(&mut self) -> bool {
        false
    }
    /// Whether the overlay wants to close itself **without a keystroke** — the host-side mirror of
    /// [`cyrup_ext::host::InteractiveOverlay::should_close`].
    ///
    /// [`Self::tick`] returns "did the frame change?", which structurally cannot say "tear me down",
    /// so an inactivity deadline could previously only be honoured on the next key press. The run
    /// loop consults this after every tick.
    fn should_close(&self) -> bool {
        false
    }
}

// =================================================================================================
// The extension adapter (pi `ctx.ui.custom(factory, { overlay: true, … })`,
// `interactive-mode.ts:2719`)
// =================================================================================================

// pi's `overlayOptions` for the extension custom-UI path — `{ anchor: "center", width: "95%",
// minWidth: 60, maxHeight: "85%", margin: 1 }` (`src/tui/fleet.ts:872-874`) — used to live here as
// four `const`s. MCP-368 moved them into `OverlayOptions::default()` in `cyrup-ext`, which is where
// an overlay can now override them per component.
//
// They are NOT kept here as that impl's source: `cyrup-ext` cannot depend on `cyrup-tui`, so a copy
// on this side could only ever drift out of agreement with the one that is actually read.

/// Adapts an extension-owned [`InteractiveOverlay`] onto this crate's [`Overlay`] z-stack.
///
/// Owns the boxed component and a one-shot that releases the blocked extension task. The one-shot
/// is fired **on drop**, so every teardown path releases it — a `Close` outcome, a session swap
/// clearing the stack (`App::rebind_session`), or the whole app quitting — and none of them can
/// leave the extension's task blocked forever.
pub struct ExtensionOverlay {
    inner: Box<dyn InteractiveOverlay>,
    done: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ExtensionOverlay {
    /// Wrap an extension's component and the one-shot that unblocks it.
    #[must_use]
    pub fn new(
        inner: Box<dyn InteractiveOverlay>,
        done: tokio::sync::oneshot::Sender<()>,
    ) -> Self {
        Self { inner, done: Some(done) }
    }

    /// pi's `{ anchor: "center", width: "95%", minWidth: 60, maxHeight: "85%", margin: 1 }` resolved
    /// against a concrete frame. Exposed (crate-visible) so a test can assert the geometry without
    /// a real terminal.
    #[must_use]
    pub(crate) fn box_rect(area: Rect, content_rows: u16, options: OverlayOptions) -> Rect {
        let usable_w = area.width.saturating_sub(options.margin.saturating_mul(2));
        // `95%` of the frame, floored at `minWidth: 60`, but never wider than what is actually
        // there — pi's own `minWidth` cannot manufacture columns a terminal does not have. A
        // `width: Some(n)` (MCP-368: the MCP panels' 82 and 92) replaces the percentage and is
        // clamped the same way, so a narrow terminal still wins.
        let width = match options.width {
            Some(fixed) => u32::from(fixed),
            None => u32::from(area.width)
                .saturating_mul(u32::from(options.width_pct))
                .checked_div(100)
                .unwrap_or(0),
        };
        let width = u16::try_from(width).unwrap_or(u16::MAX).max(options.min_width).min(usable_w);
        // The height budget is `OverlayOptions::max_rows`, shared with MCP-377's windowing so the
        // adapter and the component cannot disagree about how many rows there are.
        let max_h = options.max_rows(area.height);
        let height = content_rows.clamp(1, max_h);
        // `anchor: "center"`.
        let x = area.x.saturating_add(area.width.saturating_sub(width) / 2);
        let y = area.y.saturating_add(area.height.saturating_sub(height) / 2);
        Rect { x, y, width, height }
    }
}

impl Drop for ExtensionOverlay {
    fn drop(&mut self) {
        if let Some(done) = self.done.take() {
            // `Err` means the extension task is already gone (cancelled/aborted) — nothing to
            // release, and never a panic.
            let _ = done.send(());
        }
    }
}

impl Overlay for ExtensionOverlay {
    fn render(&mut self, frame: &mut Frame, area: Rect, _theme: &UiTheme) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        // Size the box first so the component is told the EXACT width it will be painted at (pi
        // hands its `Component.render(width)` the resolved overlay width, `tui.ts:showOverlay`),
        // then re-fit the height to what actually came back.
        //
        // The height it is told is the FRAME's, not the box's: pi's components read
        // `this.tui.terminal?.rows` directly (`pi-subagents/src/tui/fleet.ts:791`) and derive their
        // own body height from it — the fleet inspector's is `max(2, floor(rows * 0.85) - 6)`,
        // which lands its whole frame at exactly the `maxHeight: "85%"` this box then enforces.
        // Handing it the already-85%-clipped box height instead would apply that factor twice and
        // shrink the roster on every frame.
        // Read once per frame: the component owns its geometry (MCP-368), and asking twice could
        // see two different answers mid-frame.
        let options = self.inner.options();
        let probe = Self::box_rect(area, area.height, options);
        let lines = self.inner.render(probe.width as usize, area.height as usize);
        let rows = u16::try_from(lines.len()).unwrap_or(u16::MAX);
        let rect = Self::box_rect(area, rows, options);
        let painted: Vec<Line<'static>> = lines
            .into_iter()
            .take(rect.height as usize)
            .map(to_ratatui_line)
            .collect();
        frame.render_widget(Clear, rect);
        frame.render_widget(Paragraph::new(painted), rect);
    }

    fn handle(&mut self, key: &KeyEvent) -> OverlayOutcome {
        // An unmapped key (a media key, a bare modifier press) never reaches the extension: pi's
        // `handleInput(data)` only ever sees bytes a terminal actually produced for a key it knows.
        let Some(mapped) = to_overlay_key(key) else { return OverlayOutcome::Ignored };
        match self.inner.handle_key(mapped) {
            ExtOverlayOutcome::Ignored => OverlayOutcome::Ignored,
            ExtOverlayOutcome::Redraw => OverlayOutcome::Redraw,
            ExtOverlayOutcome::Close => OverlayOutcome::Close,
        }
    }

    fn refresh_ms(&self) -> u64 {
        self.inner.refresh_ms()
    }

    fn tick(&mut self) -> bool {
        self.inner.tick()
    }

    /// Delegated, so an extension overlay's own inactivity deadline reaches the run loop.
    fn should_close(&self) -> bool {
        self.inner.should_close()
    }
}

/// One backend-free [`OverlayLine`] as painted ratatui spans.
#[must_use]
pub fn to_ratatui_line(line: OverlayLine) -> Line<'static> {
    Line::from(line.spans.into_iter().map(to_ratatui_span).collect::<Vec<_>>())
}

/// One backend-free [`OverlaySpan`] as a painted ratatui span, colour and modifiers intact.
#[must_use]
pub fn to_ratatui_span(span: OverlaySpan) -> Span<'static> {
    let mut style = Style::default();
    if let Some(fg) = span.fg {
        style = style.fg(to_ratatui_color(fg));
    }
    if let Some(bg) = span.bg {
        style = style.bg(to_ratatui_color(bg));
    }
    if span.bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    if span.dim {
        style = style.add_modifier(Modifier::DIM);
    }
    if span.italic {
        style = style.add_modifier(Modifier::ITALIC);
    }
    if span.underlined {
        style = style.add_modifier(Modifier::UNDERLINED);
    }
    if span.reversed {
        style = style.add_modifier(Modifier::REVERSED);
    }
    Span::styled(span.text, style)
}

/// [`OverlayColor`] → `ratatui::style::Color`, variant for variant.
#[must_use]
pub fn to_ratatui_color(color: OverlayColor) -> Color {
    match color {
        OverlayColor::Black => Color::Black,
        OverlayColor::Red => Color::Red,
        OverlayColor::Green => Color::Green,
        OverlayColor::Yellow => Color::Yellow,
        OverlayColor::Blue => Color::Blue,
        OverlayColor::Magenta => Color::Magenta,
        OverlayColor::Cyan => Color::Cyan,
        OverlayColor::Gray => Color::Gray,
        OverlayColor::DarkGray => Color::DarkGray,
        OverlayColor::LightRed => Color::LightRed,
        OverlayColor::LightGreen => Color::LightGreen,
        OverlayColor::LightYellow => Color::LightYellow,
        OverlayColor::LightBlue => Color::LightBlue,
        OverlayColor::LightMagenta => Color::LightMagenta,
        OverlayColor::LightCyan => Color::LightCyan,
        OverlayColor::White => Color::White,
        OverlayColor::Indexed(i) => Color::Indexed(i),
        OverlayColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// A crossterm key event as the extension's backend-free [`OverlayKey`], or `None` when the key has
/// no counterpart the extension could act on.
///
/// A printable character arrives already shift-resolved (the terminal reports `Char('K')` for
/// `Shift+k`), which is exactly the distinction pi's `matchesKey(data, Key.shift("k"))` makes off
/// the raw byte — so the `shift` flag is reported but never load-bearing for characters.
#[must_use]
pub fn to_overlay_key(key: &KeyEvent) -> Option<OverlayKey> {
    let code = match key.code {
        KeyCode::Char(c) => OverlayKeyCode::Char(c),
        KeyCode::Enter => OverlayKeyCode::Enter,
        KeyCode::Esc => OverlayKeyCode::Escape,
        KeyCode::Backspace => OverlayKeyCode::Backspace,
        KeyCode::Delete => OverlayKeyCode::Delete,
        KeyCode::Tab => OverlayKeyCode::Tab,
        KeyCode::BackTab => OverlayKeyCode::BackTab,
        KeyCode::Up => OverlayKeyCode::Up,
        KeyCode::Down => OverlayKeyCode::Down,
        KeyCode::Left => OverlayKeyCode::Left,
        KeyCode::Right => OverlayKeyCode::Right,
        KeyCode::Home => OverlayKeyCode::Home,
        KeyCode::End => OverlayKeyCode::End,
        KeyCode::PageUp => OverlayKeyCode::PageUp,
        KeyCode::PageDown => OverlayKeyCode::PageDown,
        KeyCode::Insert => OverlayKeyCode::Insert,
        KeyCode::F(n) => OverlayKeyCode::F(n),
        _ => return None,
    };
    Some(OverlayKey {
        code,
        ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        alt: key.modifiers.contains(KeyModifiers::ALT),
        shift: key.modifiers.contains(KeyModifiers::SHIFT),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    /// A component that records what it was asked to paint and answers a fixed script.
    struct Probe {
        widths: std::sync::Arc<std::sync::Mutex<Vec<(usize, usize)>>>,
        keys: std::sync::Arc<std::sync::Mutex<Vec<OverlayKey>>>,
        rows: usize,
        outcome: ExtOverlayOutcome,
        ticks: usize,
    }

    impl InteractiveOverlay for Probe {
        fn render(&mut self, width: usize, height: usize) -> Vec<OverlayLine> {
            self.widths.lock().unwrap().push((width, height));
            (0..self.rows)
                .map(|i| {
                    OverlayLine::new(vec![OverlaySpan {
                        text: format!("row{i}"),
                        fg: Some(OverlayColor::Cyan),
                        bold: true,
                        ..OverlaySpan::default()
                    }])
                })
                .collect()
        }
        fn handle_key(&mut self, key: OverlayKey) -> ExtOverlayOutcome {
            self.keys.lock().unwrap().push(key);
            self.outcome
        }
        fn refresh_ms(&self) -> u64 {
            750
        }
        fn tick(&mut self) -> bool {
            self.ticks += 1;
            self.ticks.is_multiple_of(2)
        }
    }

    /// What each `Probe` reports back: the sizes it was asked to render at, the keys it received.
    type ProbeLog<T> = std::sync::Arc<std::sync::Mutex<Vec<T>>>;
    /// One built probe: the adapter, its two logs, and the one-shot its teardown fires.
    type ProbeRig = (
        ExtensionOverlay,
        ProbeLog<(usize, usize)>,
        ProbeLog<OverlayKey>,
        tokio::sync::oneshot::Receiver<()>,
    );

    fn probe(rows: usize, outcome: ExtOverlayOutcome) -> ProbeRig {
        let widths = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        let overlay = ExtensionOverlay::new(
            Box::new(Probe {
                widths: widths.clone(),
                keys: keys.clone(),
                rows,
                outcome,
                ticks: 0,
            }),
            tx,
        );
        (overlay, widths, keys, rx)
    }

    #[test]
    fn geometry_is_upstreams_center_95pct_min60_max85pct() {
        let area = Rect { x: 0, y: 0, width: 100, height: 40 };
        // 95% of 100 = 95, inside the margin-2 usable 98.
        let rect = ExtensionOverlay::box_rect(area, 100, OverlayOptions::default());
        assert_eq!(rect.width, 95);
        // Content wanted 100 rows; `maxHeight: 85%` of 40 = 34, and usable is 38, so 34 wins.
        assert_eq!(rect.height, 34);
        // Centered.
        assert_eq!(rect.x, (100 - 95) / 2);
        assert_eq!(rect.y, (40 - 34) / 2);
    }

    #[test]
    fn min_width_60_never_exceeds_the_real_terminal() {
        // 95% of 40 = 38, below `minWidth: 60`; the usable width (40 - 2 margin) caps it.
        let rect = ExtensionOverlay::box_rect(Rect { x: 0, y: 0, width: 40, height: 20 }, 5, OverlayOptions::default());
        assert_eq!(rect.width, 38);
        assert_eq!(rect.height, 5);
    }

    #[test]
    fn a_short_component_shrinks_the_box_to_its_own_row_count() {
        let rect = ExtensionOverlay::box_rect(Rect { x: 0, y: 0, width: 100, height: 40 }, 6, OverlayOptions::default());
        assert_eq!(rect.height, 6);
    }

    #[test]
    fn the_component_is_told_the_width_it_will_actually_be_painted_at() {
        let (mut overlay, widths, _keys, _rx) = probe(4, ExtOverlayOutcome::Redraw);
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = UiTheme::default();
        terminal
            .draw(|f| {
                let area = f.area();
                overlay.render(f, area, &theme);
            })
            .unwrap();
        let seen = widths.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![(95, 40)],
            "the component gets the resolved box WIDTH and the FRAME's row count"
        );
    }

    #[test]
    fn painted_cells_carry_the_components_colour_and_bold_not_just_its_characters() {
        let (mut overlay, _w, _k, _rx) = probe(2, ExtOverlayOutcome::Redraw);
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = UiTheme::default();
        terminal
            .draw(|f| {
                let area = f.area();
                overlay.render(f, area, &theme);
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        // The box is 95 wide, 2 tall, centered in 100x40 → x=2, y=19.
        let cell = buffer.cell((2, 19)).expect("first painted cell");
        assert_eq!(cell.symbol(), "r");
        assert_eq!(cell.fg, Color::Cyan, "the component's fg must survive the seam");
        assert!(
            cell.modifier.contains(Modifier::BOLD),
            "the component's bold must survive the seam"
        );
    }

    #[test]
    fn ctrl_o_and_shift_k_cross_the_seam_distinguishably() {
        let (mut overlay, _w, keys, _rx) = probe(1, ExtOverlayOutcome::Redraw);
        overlay.handle(&KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        overlay.handle(&KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT));
        overlay.handle(&KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE));
        let seen = keys.lock().unwrap().clone();
        assert_eq!(
            seen,
            vec![
                OverlayKey { code: OverlayKeyCode::Char('o'), ctrl: true, alt: false, shift: false },
                OverlayKey { code: OverlayKeyCode::Char('K'), ctrl: false, alt: false, shift: true },
                OverlayKey { code: OverlayKeyCode::Char('k'), ctrl: false, alt: false, shift: false },
            ]
        );
    }

    #[test]
    fn an_unmapped_key_never_reaches_the_component() {
        let (mut overlay, _w, keys, _rx) = probe(1, ExtOverlayOutcome::Redraw);
        let outcome = overlay.handle(&KeyEvent::new(KeyCode::Menu, KeyModifiers::NONE));
        assert_eq!(outcome, OverlayOutcome::Ignored);
        assert!(keys.lock().unwrap().is_empty());
    }

    #[test]
    fn close_propagates_and_dropping_releases_the_blocked_extension_task() {
        let (mut overlay, _w, _k, mut rx) = probe(1, ExtOverlayOutcome::Close);
        assert_eq!(
            overlay.handle(&KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            OverlayOutcome::Close
        );
        assert!(rx.try_recv().is_err(), "still open ⇒ the task stays blocked");
        drop(overlay);
        assert_eq!(rx.try_recv(), Ok(()), "teardown must release the blocked task");
    }

    #[test]
    fn refresh_cadence_and_tick_forward_to_the_component() {
        let (mut overlay, _w, _k, _rx) = probe(1, ExtOverlayOutcome::Redraw);
        assert_eq!(overlay.refresh_ms(), 750);
        assert!(!overlay.tick());
        assert!(overlay.tick());
    }

    #[test]
    fn every_colour_variant_maps_to_its_ratatui_twin() {
        assert_eq!(to_ratatui_color(OverlayColor::Rgb(9, 8, 7)), Color::Rgb(9, 8, 7));
        assert_eq!(to_ratatui_color(OverlayColor::Indexed(42)), Color::Indexed(42));
        assert_eq!(to_ratatui_color(OverlayColor::LightMagenta), Color::LightMagenta);
        assert_eq!(to_ratatui_color(OverlayColor::DarkGray), Color::DarkGray);
    }

    #[test]
    fn every_modifier_survives_span_conversion() {
        let span = to_ratatui_span(OverlaySpan {
            text: "x".into(),
            fg: Some(OverlayColor::Red),
            bg: Some(OverlayColor::Blue),
            bold: true,
            dim: true,
            italic: true,
            underlined: true,
            reversed: true,
        });
        assert_eq!(span.style.fg, Some(Color::Red));
        assert_eq!(span.style.bg, Some(Color::Blue));
        for m in [
            Modifier::BOLD,
            Modifier::DIM,
            Modifier::ITALIC,
            Modifier::UNDERLINED,
            Modifier::REVERSED,
        ] {
            assert!(span.style.add_modifier.contains(m), "{m:?} lost");
        }
    }
}
