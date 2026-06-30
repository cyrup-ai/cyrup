//! Chrome-tail components (spec/tui/01 §startup-help; Pi `components/{keybinding-hints,
//! visual-truncate,bordered-loader}.ts`).
//!
//! Three small, dependency-free pieces of Pi's interactive chrome that sit *around* the transcript +
//! editor + footer the crate already renders:
//!
//! - [`format_key_text`] / [`key_hint_line`] / [`compact_hints`] — the keybinding-hint formatter and
//!   the startup "interrupt · clear/exit · / commands · ! bash · …" bar (`keybinding-hints.ts`,
//!   `interactive-mode.ts:697-703`), sourced from the **live** [`Keymap`] so rebinds flow through.
//! - [`truncate_to_visual_lines`] — the shared tail-truncate used by tool/bash blocks
//!   (`visual-truncate.ts` `truncateToVisualLines`): keep the last *N* wrapped lines, report how many
//!   were hidden.
//! - [`BorderedLoader`] — a `DynamicBorder`-delimited spinner+message with an optional cancel hint
//!   (`bordered-loader.ts` `BorderedLoader`), the loader chrome extension UI and long ops draw inline.

use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::keymap::{Action, Keymap};
use crate::status_indicator::SPINNER_FRAMES;
use crate::theme::UiTheme;

/// Format a key string for display (`formatKeyText`, `keybinding-hints.ts:18-27`): split on `/`
/// (alternatives) and `+` (chords); on macOS rewrite the `alt` modifier to `option`. With
/// `capitalize`, each part is title-cased (`keyDisplayText`).
pub fn format_key_text(key: &str, capitalize: bool) -> String {
    key.split('/')
        .map(|alt| {
            alt.split('+')
                .map(|part| format_key_part(part, capitalize))
                .collect::<Vec<_>>()
                .join("+")
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// Format one chord part: macOS shows `option` for `alt`; `capitalize` title-cases.
fn format_key_part(part: &str, capitalize: bool) -> String {
    let display = if cfg!(target_os = "macos") && part.eq_ignore_ascii_case("alt") {
        "option"
    } else {
        part
    };
    if capitalize {
        let mut chars = display.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => String::new(),
        }
    } else {
        display.to_string()
    }
}

/// One hint as a `[dim key] [muted description]` [`Line`] (`keyHint`, `keybinding-hints.ts:39-41`).
/// The key text is formatted via [`format_key_text`].
pub fn key_hint_line(key: &str, description: &str, theme: &UiTheme) -> Line<'static> {
    Line::from(key_hint_spans(key, description, theme))
}

/// The styled spans of a single key hint (dim key + muted ` description`), for composing a hint bar.
pub fn key_hint_spans(key: &str, description: &str, theme: &UiTheme) -> Vec<Span<'static>> {
    vec![
        Span::styled(format_key_text(key, false), theme.dim_style()),
        Span::styled(format!(" {description}"), theme.muted_style()),
    ]
}

/// The compact startup-help hint pairs (`(key, description)`), in Pi's order
/// (`interactive-mode.ts:697-703` `compactInstructions`), with the interrupt/clear/exit keys resolved
/// from the **live** keymap (so a rebind is reflected). `/` and `!` are literal affordances.
pub fn compact_hints(keymap: &Keymap) -> Vec<(String, String)> {
    let interrupt = keymap.key_label(Action::Interrupt).unwrap_or_else(|| "esc".into());
    let clear = keymap.key_label(Action::Clear).unwrap_or_else(|| "ctrl+c".into());
    let exit = keymap.key_label(Action::Quit).unwrap_or_else(|| "ctrl+d".into());
    let expand = keymap.key_label(Action::ToolsExpand).unwrap_or_else(|| "ctrl+o".into());
    vec![
        (interrupt, "interrupt".to_string()),
        (format!("{clear}/{exit}"), "clear/exit".to_string()),
        ("/".to_string(), "commands".to_string()),
        ("!".to_string(), "bash".to_string()),
        (expand, "more".to_string()),
    ]
}

/// Render the compact hint bar into `area`, joining hints with a muted ` · ` separator and clamping to
/// one line (`compactInstructions.join(theme.fg("muted"," · "))`). Sources keys from the live keymap.
pub fn render_compact_hints(frame: &mut Frame, area: Rect, theme: &UiTheme, keymap: &Keymap) {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, (key, desc)) in compact_hints(keymap).into_iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", theme.muted_style()));
        }
        spans.extend(key_hint_spans(&key, &desc, theme));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme.base_style()), area);
}

/// The result of [`truncate_to_visual_lines`]: the visible (last-N) wrapped lines + how many were
/// hidden above them (`VisualTruncateResult`, `visual-truncate.ts`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisualTruncate {
    /// The visual lines to display (≤ `max`).
    pub lines: Vec<String>,
    /// How many wrapped lines were skipped off the top.
    pub skipped: usize,
}

/// Truncate `text` to at most `max` visual lines counted **from the end**, wrapping each logical line
/// to `width` (`truncateToVisualLines`, `visual-truncate.ts:30-63`). Returns the visible tail and the
/// number of hidden lines. `width == 0` is treated as `1`.
pub fn truncate_to_visual_lines(text: &str, max: usize, width: usize) -> VisualTruncate {
    if text.is_empty() {
        return VisualTruncate { lines: Vec::new(), skipped: 0 };
    }
    let width = width.max(1);
    let mut visual: Vec<String> = Vec::new();
    for logical in text.split('\n') {
        let chars: Vec<char> = logical.chars().collect();
        if chars.is_empty() {
            visual.push(String::new());
            continue;
        }
        let mut start = 0;
        while start < chars.len() {
            let end = (start + width).min(chars.len());
            visual.push(chars.get(start..end).map(|s| s.iter().collect()).unwrap_or_default());
            start = end;
        }
    }
    if visual.len() <= max {
        return VisualTruncate { lines: visual, skipped: 0 };
    }
    let skipped = visual.len() - max;
    let lines = visual.split_off(skipped);
    VisualTruncate { lines, skipped }
}

/// A `DynamicBorder`-delimited spinner+message block (`bordered-loader.ts` `BorderedLoader`): a top
/// rule, the spinner glyph + accent message, an optional cancel hint, and a bottom rule. Used by the
/// extension-UI loader and any long inline op. Immediate-mode: the spinner frame is chosen from the
/// elapsed-tick index just like [`crate::status_indicator`].
pub struct BorderedLoader {
    message: String,
    cancellable: bool,
    /// The cancel-key label for the hint (live keymap), shown only when `cancellable`.
    cancel_key: Option<String>,
}

impl BorderedLoader {
    /// A cancellable loader with `message` and the `Esc`/cancel key label for its hint.
    pub fn cancellable(message: impl Into<String>, cancel_key: impl Into<String>) -> Self {
        BorderedLoader { message: message.into(), cancellable: true, cancel_key: Some(cancel_key.into()) }
    }

    /// A non-cancellable loader (no hint row).
    pub fn plain(message: impl Into<String>) -> Self {
        BorderedLoader { message: message.into(), cancellable: false, cancel_key: None }
    }

    /// The number of rows this loader occupies: top rule + spinner line + (cancel hint) + bottom rule.
    pub fn height(&self) -> u16 {
        if self.cancellable { 4 } else { 3 }
    }

    /// Render the loader into `area`, selecting the spinner frame from `tick` (the 80 ms phase index).
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &UiTheme, tick: usize) {
        let hint_h = u16::from(self.cancellable);
        let [top, body, hint, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(hint_h),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(border_rule(top.width, theme), top);
        let spin = SPINNER_FRAMES.get(tick % SPINNER_FRAMES.len()).copied().unwrap_or("⠋");
        let body_line = Line::from(vec![
            Span::styled(format!(" {spin} "), theme.accent_style()),
            Span::styled(self.message.clone(), theme.accent_style()),
        ]);
        frame.render_widget(Paragraph::new(body_line).style(theme.base_style()), body);
        if self.cancellable {
            let key = self.cancel_key.clone().unwrap_or_else(|| "esc".into());
            let hint_line = Line::from({
                let mut spans = vec![Span::raw(" ")];
                spans.extend(key_hint_spans(&key, "cancel", theme));
                spans
            });
            frame.render_widget(Paragraph::new(hint_line).style(theme.base_style()), hint);
        }
        frame.render_widget(border_rule(bottom.width, theme), bottom);
    }
}

/// A full-width `─` rule styled `border` (Pi `DynamicBorder`, mirrors `selector::border_rule`).
fn border_rule(width: u16, theme: &UiTheme) -> Paragraph<'static> {
    let rule = "─".repeat(width.max(1) as usize);
    Paragraph::new(Line::from(Span::styled(rule, theme.border_style())))
}
