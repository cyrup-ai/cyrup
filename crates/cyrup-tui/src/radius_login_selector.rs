//! TUI-144 — the top-level `/login` menu with the animated "Sign in with Radius" option: a port of
//! `packages/coding-agent/src/modes/interactive/components/radius-login-selector.ts` (114 lines,
//! added by `ed8b3bcc1`, unchanged at the pin `f1b2e77f5`).
//!
//! Upstream's own header: "The `/login` menu with the animated 'Sign in with Radius' option.
//! Internal to the interactive mode: the shimmer is Radius-only and is not exposed to other
//! selectors." The same holds here — nothing outside `/login` builds a [`RadiusLoginMenu`].
//!
//! **Shape.** pi subclasses `ExtensionSelectorComponent`, lets it draw, then finds the line it
//! drew for the SELECTED Radius option and swaps in the animated one (`:76-88`): "When the
//! selector's row style changes or the label wraps, the line no longer matches and the option
//! renders normally." [`RadiusLoginMenu`] does the same over the [`ListSelector`] cyrup's `/login`
//! menu already is: it renders the list untouched, then finds the Radius row in the cells it just
//! drew and recolours that row in place. A truncated label no longer matches and renders normally,
//! exactly as upstream's line comparison fails.
//!
//! **Status colours.** pi's option label carries `formatAuthSelectorProviderStatus` already themed
//! (`interactive-mode.ts:5891`), so the ` ✓ configured` suffix is green on every row state. cyrup's
//! [`ListSelector`] rows are plain strings, so the same recolouring pass paints the suffix with the
//! tones [`crate::auth_select::status_indicator_runs`] reports — the runs `/login`'s provider picker
//! paints with.
//!
//! **The clock.** pi arms its own `setInterval(…, ANIMATION_FRAME_MS)` that requests a render only
//! while `animating` (`:68-71`) and clears it on select, cancel and dispose (`:57-64`, `:90-99`).
//! cyrup has ONE animation clock, the run loop's spinner tick; this selector reports
//! [`ANIMATION_FRAME`] through [`Selector::animation_frame`] while the Radius row is the one it
//! animated last frame, and the loop arms that tick from it
//! ([`crate::app::App::animation_period`]). Closing the menu by any path stops the shimmer, because
//! there is no timer to clear — the loop stops asking a selector that is no longer in the slot.

use std::time::{Duration, Instant};

use cyrup_resources::color::Rgb;
use cyrup_resources::system_theme::{ColorMixSpace, mix_colors};
use ratatui::Frame;
use ratatui::crossterm::event::KeyEvent;
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::auth_select::StatusTone;
use crate::keymap::SelectKeymap;
use crate::selector::{ListSelector, Selector, SelectorOutcome};
use crate::theme::UiTheme;

/// "The four colors of the Radius logo, in the order they stream across the text."
/// (`radius-login-selector.ts:10-11`: `#4d9abf`, `#83ccd2`, `#f1be57`, `#f09082`.)
pub(crate) const RADIUS_COLORS: [Rgb; 4] = [
    Rgb::new(0x4d, 0x9a, 0xbf),
    Rgb::new(0x83, 0xcc, 0xd2),
    Rgb::new(0xf1, 0xbe, 0x57),
    Rgb::new(0xf0, 0x90, 0x82),
];
/// "Width of each color band, in characters." (`:12-13`).
pub(crate) const CHARS_PER_COLOR: usize = 4;
/// `CHARS_PER_SECOND = 10` (`:14`).
pub(crate) const CHARS_PER_SECOND: f64 = 10.0;
/// `ANIMATION_FRAME_MS = 50` (`:15`) — the period the run loop's animation tick is armed at while
/// the Radius row shimmers.
pub(crate) const ANIMATION_FRAME: Duration = Duration::from_millis(50);

/// `radiusShimmer(text, elapsedMs)` (`:18-36`) minus the escape-string assembly: the colour of each
/// of `char_count` characters, "the Radius logo colors flowing left to right".
///
/// Per character: its position in the 16-character cycle, shifted back by
/// `elapsed / 1000 * CHARS_PER_SECOND`; the band it falls in and the next band's colour; and a
/// smoothstepped mix between the two, IN sRGB (`mixColors(from, to, amount, "srgb")`, `:31`) — see
/// [`cyrup_resources::system_theme::mix_colors`] for why that arm exists. The arithmetic is pi's
/// line for line, including the double `%` (JS `%` and Rust's `f64 %` are both the truncated
/// remainder, so `((x % c) + c) % c` means the same thing on both sides).
pub(crate) fn radius_shimmer(char_count: usize, elapsed_ms: f64) -> Vec<Rgb> {
    let bands = RADIUS_COLORS.len();
    let per_color = CHARS_PER_COLOR as f64;
    let cycle = (bands * CHARS_PER_COLOR) as f64;
    let offset = (elapsed_ms / 1000.0) * CHARS_PER_SECOND;
    (0..char_count)
        .map(|index| {
            let position = (((index as f64 - offset) % cycle) + cycle) % cycle;
            let band_f = (position / per_color).floor();
            let t = position / per_color - band_f;
            // "Smoothstep keeps each band recognizable while still blending into the next one."
            let amount = t * t * (3.0 - 2.0 * t);
            let band = (band_f as usize) % bands;
            let from = RADIUS_COLORS.get(band).copied().unwrap_or(RADIUS_COLORS[0]);
            let to = RADIUS_COLORS
                .get((band + 1) % bands)
                .copied()
                .unwrap_or(RADIUS_COLORS[0]);
            mix_colors(from, to, amount, ColorMixSpace::Srgb)
        })
        .collect()
}

/// "The 'Sign in with Radius' option: `label` is the full option, starting with the animated
/// `text`." (`RadiusOption`, `:38-39`). The status half of the label is kept as its styled runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RadiusOption {
    /// `` `Sign in with ${radiusOption.name}` `` (`interactive-mode.ts:5890`) — the animated part.
    pub(crate) text: String,
    /// `formatAuthSelectorProviderStatus(radiusOption)` (`:5891`) as tone runs.
    pub(crate) status: Vec<(StatusTone, String)>,
}

impl RadiusOption {
    /// `radiusLabel` — `` `${radiusText}${formatAuthSelectorProviderStatus(radiusOption)}` ``
    /// (`interactive-mode.ts:5891`), uncoloured.
    pub(crate) fn label(&self) -> String {
        let mut label = self.text.clone();
        for (_, run) in &self.status {
            label.push_str(run);
        }
        label
    }
}

/// `RadiusLoginMenuComponent` (`radius-login-selector.ts:46-100`).
pub(crate) struct RadiusLoginMenu {
    inner: ListSelector,
    option: RadiusOption,
    /// `animationStart = performance.now()` (`:48`), taken at construction like upstream's.
    started: Instant,
    /// A fixed animation time in place of the wall clock — set only by tests, so a frame can be
    /// asserted colour for colour.
    frozen_elapsed: Option<Duration>,
    /// `animating` (`:50`): whether the last render found and animated the SELECTED Radius row.
    animating: bool,
}

impl RadiusLoginMenu {
    /// `createLoginMenuSelector(tui, title, options, radiusOption, …)` (`:103-113`): `inner` is the
    /// menu exactly as it would be without Radius, with the Radius label already its LAST row.
    pub(crate) fn new(inner: ListSelector, option: RadiusOption) -> Self {
        Self {
            inner,
            option,
            started: Instant::now(),
            frozen_elapsed: None,
            animating: false,
        }
    }

    /// Pin the animation time (tests only).
    #[cfg(test)]
    pub(crate) fn freeze_clock_at(&mut self, elapsed: Duration) {
        self.frozen_elapsed = Some(elapsed);
    }

    /// The option this menu animates (tests/inspection).
    #[cfg(test)]
    pub(crate) fn option(&self) -> &RadiusOption {
        &self.option
    }

    /// `performance.now() - this.animationStart` (`:84`).
    fn elapsed(&self) -> Duration {
        self.frozen_elapsed
            .unwrap_or_else(|| self.started.elapsed())
    }

    /// Recolour the Radius row in the cells the list just drew. Returns whether that row was the
    /// SELECTED one, i.e. whether it now shimmers.
    fn decorate(&self, frame: &mut Frame, area: Rect, theme: &UiTheme) -> bool {
        let text: Vec<char> = self.option.text.chars().collect();
        let label: Vec<char> = self.option.label().chars().collect();
        let buf = frame.buffer_mut();
        for y in area.top()..area.bottom() {
            let row: Vec<String> = (area.left()..area.right())
                .map(|x| {
                    buf.cell((x, y))
                        .map_or_else(String::new, |c| c.symbol().to_string())
                })
                .collect();
            let Some((start, selected)) = locate_option(&row, &label) else {
                continue;
            };
            let column = |offset: usize| {
                u16::try_from(start.saturating_add(offset))
                    .ok()
                    .and_then(|o| area.left().checked_add(o))
            };
            if selected {
                // `radiusShimmer(text, …)` over the option's text (`:84-85`); the status suffix
                // keeps its own colours (`label.slice(text.length)`, `:85`).
                let elapsed_ms = self.elapsed().as_secs_f64() * 1000.0;
                for (offset, rgb) in radius_shimmer(text.len(), elapsed_ms)
                    .into_iter()
                    .enumerate()
                {
                    if let Some(cell) = column(offset).and_then(|x| buf.cell_mut((x, y))) {
                        // `foregroundAnsi(color, theme.getColorMode())` (`:32`) — the projection
                        // every themed colour goes through.
                        cell.set_fg(theme.color_mode.project(Color::Rgb(rgb.r, rgb.g, rgb.b)));
                    }
                }
            }
            let mut offset = text.len();
            for (tone, run) in &self.option.status {
                let style = match tone {
                    StatusTone::Muted => theme.muted_style(),
                    StatusTone::Warning => theme.warning_style(),
                    StatusTone::Success => theme.success_style(),
                };
                for _ in run.chars() {
                    if let Some(cell) = column(offset).and_then(|x| buf.cell_mut((x, y))) {
                        cell.set_style(style);
                    }
                    offset = offset.saturating_add(1);
                }
            }
            return selected;
        }
        false
    }
}

/// Where the Radius label starts in one drawn row, and whether the row is the selected one — pi's
/// `lines.indexOf(selectedLine)` (`:79-80`), cell-wise. A row matches only when, after its leading
/// blank inset, it is exactly `→ ` + label (selected) or exactly the label (not selected), followed
/// by nothing but blanks; a truncated or wrapped label therefore never matches, and that row keeps
/// the list's own rendering, as upstream's does.
fn locate_option(row: &[String], label: &[char]) -> Option<(usize, bool)> {
    let blank = |s: &String| s.is_empty() || s == " ";
    let first = row.iter().position(|s| !blank(s))?;
    let matches_at = |start: usize| {
        let end = start.checked_add(label.len())?;
        let cells = row.get(start..end)?;
        let same = cells
            .iter()
            .zip(label)
            .all(|(cell, ch)| cell.chars().eq(std::iter::once(*ch)));
        (same && row.get(end..).is_some_and(|rest| rest.iter().all(blank))).then_some(start)
    };
    let selected = row.get(first).is_some_and(|s| s == "→")
        && row.get(first.saturating_add(1)).is_some_and(blank);
    if selected {
        return matches_at(first.saturating_add(2)).map(|start| (start, true));
    }
    matches_at(first).map(|start| (start, false))
}

impl Selector for RadiusLoginMenu {
    fn desired_height(&self, width: u16) -> u16 {
        self.inner.desired_height(width)
    }

    /// `override render(width)` (`:76-88`).
    fn render(&mut self, frame: &mut Frame, area: Rect, theme: &UiTheme) {
        self.inner.render(frame, area, theme);
        self.animating = self.decorate(frame, area, theme);
    }

    fn handle(&mut self, key: &KeyEvent, keymap: &SelectKeymap) -> SelectorOutcome {
        let outcome = self.inner.handle(key, keymap);
        // `stopAnimation()` before `onSelect` / `onCancel` (`:57-64`). The slot closes on both, so
        // the loop would stop asking anyway; clearing here keeps this selector's own answer honest
        // for the frame in between.
        if matches!(
            outcome,
            SelectorOutcome::Confirm(_) | SelectorOutcome::Cancel
        ) {
            self.animating = false;
        }
        outcome
    }

    fn pointer(&mut self, area: Rect, event: crate::app::Pointer) -> SelectorOutcome {
        let outcome = self.inner.pointer(area, event);
        if matches!(outcome, SelectorOutcome::Confirm(_)) {
            self.animating = false;
        }
        outcome
    }

    fn set_title(&mut self, title: String) {
        self.inner.set_title(title);
    }

    /// `if (this.animating) tui.requestRender()` every `ANIMATION_FRAME_MS` (`:68-71`).
    fn animation_frame(&self) -> Option<Duration> {
        self.animating.then_some(ANIMATION_FRAME)
    }
}
