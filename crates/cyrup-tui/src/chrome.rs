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
use ratatui::widgets::{Paragraph, Wrap};
use ratatui::Frame;

use crate::keymap::{Action, Keymap};
use crate::selector::border_rule;
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
/// (v0.84.1 `interactive-mode.ts:936-942` `compactInstructions`), with the interrupt/clear/exit keys
/// resolved from the **live** keymap (so a rebind is reflected). `/` and `!` are literal affordances.
pub fn compact_hints(keymap: &Keymap) -> Vec<(String, String)> {
    // `hint(kb, desc)` is `keyHint` → `keyText`, and the clear/exit pair is
    // `rawKeyHint(`${keyText("app.clear")}/${keyText("app.exit")}`, …)` — every key here resolves
    // through `keyText`, which joins ALL bound keys with `/` (`keybinding-hints.ts:29-36`). So these
    // are [`Keymap::keys_label`], not the first-key `key_label`: a two-key rebind must show both.
    let interrupt = keymap.keys_label(Action::Interrupt).unwrap_or_else(|| "escape".into());
    let clear = keymap.keys_label(Action::Clear).unwrap_or_else(|| "ctrl+c".into());
    let exit = keymap.keys_label(Action::Quit).unwrap_or_else(|| "ctrl+d".into());
    let expand = keymap.keys_label(Action::ToolsExpand).unwrap_or_else(|| "ctrl+o".into());
    vec![
        (interrupt, "interrupt".to_string()),
        (format!("{clear}/{exit}"), "clear/exit".to_string()),
        ("/".to_string(), "commands".to_string()),
        ("!".to_string(), "bash".to_string()),
        (expand, "more".to_string()),
    ]
}

/// The onboarding line printed directly under the compact hint bar — verbatim
/// `interactive-mode.ts:943-946`:
/// `theme.fg("dim", \`Press ${keyText("app.tools.expand")} to show full startup help and loaded
/// resources.\`)`. It is the only place a new user is told the expanded help exists, and cyrup
/// had no counterpart at all (`grep -rn "to show full startup help" crates/` found nothing).
pub fn compact_onboarding(keymap: &Keymap) -> String {
    // `keyText("app.tools.expand")` — all bound keys joined with `/` (`keybinding-hints.ts:29-36`).
    let expand = keymap.keys_label(Action::ToolsExpand).unwrap_or_else(|| "ctrl+o".into());
    format!("Press {expand} to show full startup help and loaded resources.")
}

/// The block's closing sentence — `onboarding`, `interactive-mode.ts:947-950`:
/// `theme.fg("dim", \`Pi can explain its own features and look up its docs. Ask it how to use or
/// extend Pi.\`)`.
///
/// It is the FIFTH part of the same `ExpandableText` body the hint bar comes from
/// (`${logo}\n${compactInstructions}\n${compactOnboarding}\n\n${onboarding}`, `:952`) — one
/// statement, one theme call — and it is also the one part upstream keeps in BOTH the collapsed and
/// the expanded body (`:953`), so dropping it removed the line unconditionally. The product name is
/// rebranded pi→cyrup exactly as `terminal_title.rs` and `resume_hint.rs` rebrand `APP_NAME`.
pub const STARTUP_ONBOARDING: &str =
    "Cyrup can explain its own features and look up its docs. Ask it how to use or extend Cyrup.";

/// The `paddingX` of the startup `ExpandableText` — `new ExpandableText(…, 1, 0)`
/// (`interactive-mode.ts:951-957`). `Text.render` emits `leftMargin + line + rightMargin` around
/// EVERY wrapped row and wraps at `contentWidth = max(1, width - paddingX * 2)` (`text.ts:64-76`).
const HINT_PADDING_X: u16 = 1;

/// Rows the block occupies when nothing wraps: a framing blank, the hint bar, the compact
/// onboarding line, the body's own blank, the closing onboarding line, and a second framing blank.
///
/// The block is upstream's startup `ExpandableText` — body
/// `${logo}\n${compactInstructions}\n${compactOnboarding}\n\n${onboarding}`
/// (`interactive-mode.ts:936-957`) — framed by a `Spacer(1)` on each side (`:960-962`). cyrup does
/// not draw the `logo` part, so 1 + 4 + 1 = **6**.
///
/// This is the UNWRAPPED count. On a narrow terminal the text rows wrap and the block grows, which
/// is why the layout must ask [`compact_hint_height`] rather than use this constant directly.
pub const COMPACT_HINT_ROWS: u16 = 6;

/// One row group of the startup hint block: its logical lines, its WRAPPED height at the block's
/// content width, and the order in which it is given up when the area is too short.
struct HintEntry {
    lines: Vec<Line<'static>>,
    rows: u16,
    /// Higher is dropped first; the hint bar is `0` and is never dropped.
    drop_rank: u8,
}

/// The block's content width — `contentWidth = Math.max(1, width - paddingX * 2)` (`text.ts:64`).
fn hint_content_width(width: u16) -> u16 {
    width.saturating_sub(HINT_PADDING_X.saturating_mul(2)).max(1)
}

/// The six entries, each already measured against `width`'s wrapping.
///
/// The drop ranks degrade the block from its EDGES INWARD, so the hint bar — the only row carrying
/// information the user cannot get anywhere else — is the last thing standing: trailing blank,
/// leading blank, closing onboarding, the body's inner blank, the compact onboarding line, and only
/// then the bar itself. A previous revision put the framing blank FIRST in a fixed-height,
/// top-aligned `Paragraph`, so a one-row budget drew the blank and the bar vanished entirely.
fn compact_hint_entries(theme: &UiTheme, keymap: &Keymap, width: u16) -> Vec<HintEntry> {
    let content = hint_content_width(width);
    let blank = |rank: u8| HintEntry { lines: vec![Line::default()], rows: 1, drop_rank: rank };
    let text = |lines: Vec<Line<'static>>, rank: u8| HintEntry {
        rows: crate::transcript::wrapped_height(&lines, content as usize)
            .min(u16::MAX as usize) as u16,
        lines,
        drop_rank: rank,
    };

    // `compactInstructions.join(theme.fg("muted", " · "))` (`interactive-mode.ts:942`).
    let mut bar: Vec<Span<'static>> = Vec::new();
    for (i, (key, desc)) in compact_hints(keymap).into_iter().enumerate() {
        if i > 0 {
            bar.push(Span::styled(" · ", theme.muted_style()));
        }
        bar.extend(key_hint_spans(&key, &desc, theme));
    }

    vec![
        blank(4),
        text(vec![Line::from(bar)], 0),
        text(
            vec![Line::styled(compact_onboarding(keymap), theme.dim_style())],
            1,
        ),
        blank(2),
        text(vec![Line::styled(STARTUP_ONBOARDING.to_string(), theme.dim_style())], 3),
        blank(5),
    ]
}

/// The block's height at `width` with pi's wrapping applied (`wrapTextWithAnsi(normalizedText,
/// contentWidth)`, `text.ts:67`), so the layout reserves the rows the block will actually need.
///
/// cyrup previously reserved a fixed row count and rendered a fixed-height `Paragraph` with no
/// `.wrap()`, so on a narrow terminal the overflowing half of each line was simply lost.
pub fn compact_hint_height(theme: &UiTheme, keymap: &Keymap, width: u16) -> u16 {
    compact_hint_entries(theme, keymap, width).iter().map(|e| e.rows).fold(0u16, u16::saturating_add)
}

/// Render the compact hint block into `area`, wrapping each row group at
/// [`content width`](hint_content_width) and insetting it by the `paddingX 1` margin.
///
/// If `area` is shorter than [`compact_hint_height`] the block degrades from its edges inward (see
/// [`compact_hint_entries`]) until it fits, so the hint bar survives down to a one-row budget.
pub fn render_compact_hints(frame: &mut Frame, area: Rect, theme: &UiTheme, keymap: &Keymap) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut entries = compact_hint_entries(theme, keymap, area.width);
    let total = |es: &[HintEntry]| es.iter().map(|e| e.rows).fold(0u16, u16::saturating_add);
    while total(&entries) > area.height {
        // Give up the outermost droppable group; `drop_rank == 0` (the bar) is never a candidate.
        let Some(idx) = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.drop_rank > 0)
            .max_by_key(|(_, e)| e.drop_rank)
            .map(|(i, _)| i)
        else {
            break;
        };
        entries.remove(idx);
    }

    // Paint the block's own background first: each group renders into an INSET rect, so the padding
    // columns would otherwise keep whatever was in the buffer.
    frame.render_widget(Paragraph::new(Vec::<Line<'static>>::new()).style(theme.base_style()), area);

    // `paddingX` only fits once the row is at least 3 columns wide; below that upstream's
    // `Math.max(1, width - 2)` collapses the content anyway.
    let pad = if area.width >= 3 { HINT_PADDING_X } else { 0 };
    let mut y = area.y;
    let end = area.y.saturating_add(area.height);
    for entry in entries {
        if y >= end {
            break;
        }
        let height = entry.rows.min(end.saturating_sub(y));
        let rect = Rect {
            x: area.x.saturating_add(pad),
            y,
            width: hint_content_width(area.width),
            height,
        };
        frame.render_widget(
            Paragraph::new(entry.lines).wrap(Wrap { trim: false }).style(theme.base_style()),
            rect,
        );
        y = y.saturating_add(height);
    }
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
/// to `width` (`truncateToVisualLines`, `visual-truncate.ts:30-53`). Returns the visible tail and the
/// number of hidden lines. `width == 0` is treated as `1`.
///
/// Upstream this function owns no wrapping of its own — it is literally
/// ```text
/// const tempText = new Text(text, paddingX, 0);
/// const allVisualLines = tempText.render(width);
/// ```
/// (`visual-truncate.ts:37-38`), i.e. `wrapTextWithAnsi(text, width - paddingX * 2)`
/// (`text.ts:64`, `:67`). cyrup's callers pass the already-reduced content width and add the margin
/// themselves, so `width` here is upstream's `width - paddingX * 2`; the WRAP is the shared
/// [`crate::transcript::wrap_line`] — the same `wrapSingleLine` port `Text`, `Box` and
/// [`crate::markdown`] use.
///
/// It used to be a `chars()`-indexed hard chunker: every logical line was sliced into fixed
/// `width`-*char* pieces, so it broke mid-word (`… output tha` / `t certainly …`), miscounted every
/// CJK ideograph, emoji and box-drawing glyph as one column, and could split a ZWJ sequence or
/// detach a combining mark. That is the same char-vs-grapheme defect already fixed in `wrap_line`,
/// `wrap_cell` and `word_wrap_line`, and — because the hidden-line count is derived from the row
/// count — it also reported the wrong `... N more lines`.
pub fn truncate_to_visual_lines(text: &str, max: usize, width: usize) -> VisualTruncate {
    if text.is_empty() {
        return VisualTruncate { lines: Vec::new(), skipped: 0 };
    }
    let width = width.max(1);
    let mut visual: Vec<String> = Vec::new();
    // `wrapTextWithAnsi` splits on `/\r\n|\r|\n/` first (`utils.ts:839`) and wraps each piece,
    // returning `[""]` for an empty one (`:858-860`) so a blank output line keeps its row.
    for logical in text.split('\n') {
        for row in crate::transcript::wrap_line(&Line::from(Span::raw(logical.to_string())), width) {
            visual.push(row.spans.iter().map(|s| s.content.as_ref()).collect());
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
    ///
    /// Upstream this is `keyHint("tui.select.cancel", "cancel")` (`bordered-loader.ts:36`), so it
    /// must come from [`crate::keymap::SelectKeymap::keys_label`]`(SelectAction::Cancel)` — **all**
    /// bound keys joined with `/` (`keybinding-hints.ts:29-36`), stock `escape/ctrl+c` — never the
    /// first key of a different action.
    cancel_key: Option<String>,
}

impl BorderedLoader {
    /// A cancellable loader with `message` and the `tui.select.cancel` key label for its hint.
    pub fn cancellable(message: impl Into<String>, cancel_key: impl Into<String>) -> Self {
        BorderedLoader { message: message.into(), cancellable: true, cancel_key: Some(cancel_key.into()) }
    }

    /// A non-cancellable loader (no hint row).
    pub fn plain(message: impl Into<String>) -> Self {
        BorderedLoader { message: message.into(), cancellable: false, cancel_key: None }
    }

    /// The number of rows this loader occupies: **7** cancellable, **5** plain.
    ///
    /// `BorderedLoader`'s children (`bordered-loader.ts:16-39`) are `DynamicBorder` (1 row),
    /// `Loader` (**2** rows — `render` returns `["", ...super.render(width)]`, `loader.ts:43-45`),
    /// then when cancellable `Spacer(1)` + `Text(keyHint, 1, 0)` (`:35-36`), then `Spacer(1)` (`:38`)
    /// and the closing `DynamicBorder` (`:39`). cyrup drew 4/3 — missing the loader's own leading
    /// blank and both `Spacer(1)` rows, so the spinner sat against the top rule and the hint against
    /// the bottom one.
    pub fn height(&self) -> u16 {
        if self.cancellable {
            7
        } else {
            5
        }
    }

    /// Render the loader into `area`, selecting the spinner frame from `tick` (the 80 ms phase index).
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &UiTheme, tick: usize) {
        let hint_h = u16::from(self.cancellable);
        // top rule / loader blank / loader body / spacer / hint / spacer / bottom rule.
        let [top, lead, body, gap, hint, tail, bottom] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(hint_h),
            Constraint::Length(hint_h),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        let _ = (lead, gap, tail); // `Spacer(1)` rows: deliberately blank (`spacer.ts:21-27`).
        frame.render_widget(border_rule(top.width, theme), top);
        let spin = SPINNER_FRAMES.get(tick % SPINNER_FRAMES.len()).copied().unwrap_or("⠋");
        let body_line = Line::from(vec![
            // `spinnerColorFn = (s) => theme.fg("accent", s)` but
            // `messageColorFn = (s) => theme.fg("muted", s)` (`bordered-loader.ts:20-21`, `:28-29`)
            // — in BOTH the cancellable and the plain branch. cyrup painted the message accent too,
            // so `Creating gist...` came out bright teal, and disagreed with the status band
            // (`status_indicator.rs`), which already had it right.
            Span::styled(format!(" {spin} "), theme.accent_style()),
            Span::styled(self.message.clone(), theme.muted_style()),
            Span::styled(" ", theme.muted_style()),
        ]);
        frame.render_widget(Paragraph::new(body_line).style(theme.base_style()), body);
        if self.cancellable {
            let key = self.cancel_key.clone().unwrap_or_else(|| "escape/ctrl+c".into());
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
