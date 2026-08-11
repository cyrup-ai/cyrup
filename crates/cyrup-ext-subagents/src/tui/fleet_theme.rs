//! Width/style primitives the three FleetView modules share — cyrup's stand-in for the four
//! `@earendil-works/pi-tui` helpers `src/tui/fleet*.ts` import (`fleet.ts:5`,
//! `fleet-status.ts:2`, `fleet-transcript.ts:4`): `theme.fg(role, text)` / `theme.bold(text)`,
//! `visibleWidth`, `truncateToWidth` and `wrapTextWithAnsi`.
//!
//! # Why this module exists at all (the transport difference, stated once)
//!
//! pi hand-rolls a `render(width): string[]` framework: every component returns **strings with
//! embedded ANSI escapes**, so its width helpers must parse `\x1b[` sequences back out to count
//! visible columns (`pi/packages/tui/src/utils.ts:240` `visibleWidth`, `:832`
//! `wrapTextWithAnsi`, `:1053` `truncateToWidth`), and its `theme.fg("accent", text)` returns a
//! string wrapped in escape codes.
//!
//! cyrup does not own a terminal in this crate (arch-SA §1.1/§6.1 — `cyrup-ext-subagents` never
//! depends on `cyrup-tui`) and paints through **ratatui**, where style is structural: a
//! [`Line`] is a list of [`Span`]s, each carrying its own [`Style`]. That makes the ANSI-parsing
//! half of pi's helpers vanish outright — a span's visible width is just its text's display
//! width, with no escapes to skip — while the *behaviour* those helpers define (clip at N
//! columns, pad to N columns, right-align a suffix, word-wrap with long-word breaking) is ported
//! verbatim below and is what the FleetView modules actually depend on.
//!
//! This module is therefore **cyrup-original support code with no 1:1 upstream file**: every
//! function names the pi helper whose behaviour it reproduces, and nothing here invents a
//! rendering rule pi does not have. The same convention `tui/render.rs` already follows (see its
//! own module doc) — this crate emits renderable [`Line`] values and whichever crate owns the live
//! terminal paints them.
//!
//! # Colour roles
//!
//! [`Role`] enumerates exactly the theme roles `fleet.ts`/`fleet-status.ts`/`fleet-transcript.ts`
//! pass to `theme.fg(...)` — nothing more. The concrete [`Color`] each maps to is this crate's
//! choice (pi resolves them against the user's active `MarkdownTheme`/`Theme`, a `cyrup-tui`-owned
//! surface out of reach here), chosen to match the palette `tui/render.rs` already established for
//! this crate's other renderable output.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// One of the theme roles the FleetView modules pass to pi's `theme.fg(role, text)`.
///
/// The set is closed and exhaustive against upstream usage: `accent`, `muted`, `dim`, `success`,
/// `warning`, `error` (`fleet.ts:232-237`, `fleet-status.ts:89-95`), `border`
/// (`fleet.ts:807-828`), `borderMuted`, `toolTitle`, `toolOutput` (`fleet-transcript.ts:441-563`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// pi `"accent"` — the selected row marker, the live-run glyph, section headings.
    Accent,
    /// pi `"muted"` — secondary but still readable text (stat rows, agent names).
    Muted,
    /// pi `"dim"` — tertiary text (identity lines, hints, footers).
    Dim,
    /// pi `"success"` — a completed child.
    Success,
    /// pi `"warning"` — a paused/stopped/detached child, and the running-tool glyph.
    Warning,
    /// pi `"error"` — a failed child, an errored tool, a stderr notice.
    Error,
    /// pi `"border"` — the inspector frame's box-drawing characters.
    Border,
    /// pi `"borderMuted"` — the transcript's left rail.
    BorderMuted,
    /// pi `"toolTitle"` — a tool invocation's name/command headline.
    ToolTitle,
    /// pi `"toolOutput"` — a tool invocation's captured output body.
    ToolOutput,
}

/// The [`Style`] one [`Role`] renders with.
#[must_use]
pub fn style(role: Role) -> Style {
    match role {
        Role::Accent => Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Role::Muted => Style::default().fg(Color::Gray),
        Role::Dim => Style::default().add_modifier(Modifier::DIM),
        Role::Success => Style::default().fg(Color::Green),
        Role::Warning => Style::default().fg(Color::Yellow),
        Role::Error => Style::default().fg(Color::Red),
        Role::Border => Style::default().fg(Color::DarkGray),
        Role::BorderMuted => Style::default().fg(Color::DarkGray).add_modifier(Modifier::DIM),
        Role::ToolTitle => Style::default().fg(Color::Blue),
        Role::ToolOutput => Style::default().fg(Color::Gray),
    }
}

/// pi `theme.fg(role, text)` — one styled [`Span`].
#[must_use]
pub fn fg(role: Role, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), style(role))
}

/// pi `theme.bold(text)` — one bold, otherwise-unstyled [`Span`].
#[must_use]
pub fn bold(text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), Style::default().add_modifier(Modifier::BOLD))
}

/// pi `theme.fg(role, theme.bold(text))` — bold AND role-coloured, as
/// `fleet-transcript.ts:444,466,472,524` composes them.
#[must_use]
pub fn fg_bold(role: Role, text: impl Into<String>) -> Span<'static> {
    Span::styled(text.into(), style(role).add_modifier(Modifier::BOLD))
}

/// An unstyled [`Span`] — pi's bare string concatenation between styled fragments.
#[must_use]
pub fn raw(text: impl Into<String>) -> Span<'static> {
    Span::raw(text.into())
}

/// pi `visibleWidth(str)` (`pi/packages/tui/src/utils.ts:240`) — the number of terminal columns a
/// line occupies. Where pi must strip ANSI escapes first, ratatui's spans carry style
/// structurally, so this is the plain sum of the spans' display widths.
#[must_use]
pub fn line_width(line: &Line<'_>) -> usize {
    line.spans.iter().map(|span| span_width(span)).sum()
}

/// [`line_width`] for one span.
#[must_use]
pub fn span_width(span: &Span<'_>) -> usize {
    str_width(&span.content)
}

/// The display width of a plain string, in terminal columns.
#[must_use]
pub fn str_width(text: &str) -> usize {
    Span::raw(text).width()
}

/// pi `truncateToWidth(text, maxWidth, ellipsis)` (`utils.ts:1053-1088`), with pi's own
/// `pad = false` default: clip to at most `max_width` columns, appending `ellipsis` when anything
/// was dropped. `max_width == 0` yields an empty line, and an already-short line is returned
/// untouched (no ellipsis) — both pi's early returns.
///
/// pi's default `ellipsis` is `"..."`; every FleetView call site EXCEPT
/// `fleet-transcript.ts:541` relies on that default, and that one passes `"…"`.
#[must_use]
pub fn truncate_to_width(line: &Line<'static>, max_width: usize, ellipsis: &str) -> Line<'static> {
    if max_width == 0 {
        return Line::from(Vec::<Span<'static>>::new());
    }
    if line_width(line) <= max_width {
        return line.clone();
    }
    let ellipsis_width = str_width(ellipsis);
    // pi: when the ellipsis alone is at least as wide as the budget it replaces the whole body
    // (`utils.ts:1067-1077`).
    if ellipsis_width >= max_width {
        return Line::from(vec![Span::raw(clip_str(ellipsis, max_width))]);
    }
    let target = max_width.saturating_sub(ellipsis_width);
    let mut kept: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in &line.spans {
        if used >= target {
            break;
        }
        let remaining = target.saturating_sub(used);
        let clipped = clip_str(&span.content, remaining);
        if clipped.is_empty() {
            continue;
        }
        used = used.saturating_add(str_width(&clipped));
        kept.push(Span::styled(clipped, span.style));
    }
    kept.push(Span::raw(ellipsis.to_string()));
    Line::from(kept)
}

/// pi's `truncateToWidth(text, width)` with its DEFAULT `"..."` ellipsis — the form every
/// FleetView call site but one uses.
#[must_use]
pub fn clip(line: &Line<'static>, max_width: usize) -> Line<'static> {
    truncate_to_width(line, max_width, "...")
}

/// Clip a plain string to at most `max_width` display columns, never splitting a character.
#[must_use]
pub fn clip_str(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let w = str_width(ch.encode_utf8(&mut [0u8; 4]));
        if used.saturating_add(w) > max_width {
            break;
        }
        used = used.saturating_add(w);
        out.push(ch);
    }
    out
}

/// pi `fit(text, width)` (`fleet.ts:442-445`) — clip to `width`, then right-pad with spaces so the
/// result is EXACTLY `width` columns wide.
#[must_use]
pub fn fit(line: &Line<'static>, width: usize) -> Line<'static> {
    let mut clipped = clip(line, width);
    let pad = width.saturating_sub(line_width(&clipped));
    if pad > 0 {
        clipped.spans.push(Span::raw(" ".repeat(pad)));
    }
    clipped
}

/// pi `rightAligned(left, right, width)` (`fleet.ts:447-451`): `left` occupies
/// `width - visibleWidth(right) - 1` columns, then at least one space, then `right`.
///
/// The `fleet-status.ts:63-69` `rightAlign` is a SEPARATE, subtly different helper — see
/// [`right_align_status`].
#[must_use]
pub fn right_aligned(
    left: &Line<'static>,
    right: &Line<'static>,
    width: usize,
) -> Line<'static> {
    let right_width = line_width(right);
    let left_width = width.saturating_sub(right_width.saturating_add(1));
    let gap = width
        .saturating_sub(left_width)
        .saturating_sub(right_width)
        .max(1);
    let mut spans = fit(left, left_width).spans;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(fit(right, right_width).spans);
    Line::from(spans)
}

/// pi `rightAlign(left, right, width)` (`fleet-status.ts:63-69`) — the fleet-status widget's own
/// variant: clip `left` to `width - visibleWidth(right) - 1`, join with a gap of at least one
/// space computed from the CLIPPED left width, then clip the whole row to `width`. It differs from
/// [`right_aligned`] in that it does not pad `left` out to its budget, so a short left side
/// pushes `right` further left rather than pinning it to the right edge.
#[must_use]
pub fn right_align_status(
    left: &Line<'static>,
    right: &Line<'static>,
    width: usize,
) -> Line<'static> {
    let right_width = line_width(right);
    let max_left_width = width.saturating_sub(right_width.saturating_add(1));
    let left_clamped = clip(left, max_left_width);
    let gap = width
        .saturating_sub(line_width(&left_clamped))
        .saturating_sub(right_width)
        .max(1);
    let mut spans = left_clamped.spans;
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right.spans.iter().cloned());
    clip(&Line::from(spans), width)
}

/// pi `wrapTextWithAnsi(text, width)` (`utils.ts:832-853` + `wrapSingleLine`, `:857-...`):
/// word-wrap to `width` columns, breaking a single token that is itself wider than `width`, and
/// splitting on embedded newlines first. An empty input yields `[""]` (pi's `return [""]`), and a
/// line that already fits is returned unchanged.
#[must_use]
pub fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let width = width.max(1);
    // pi splits the raw text on `\r\n|\r|\n` and wraps each piece; a span whose content carries a
    // newline is split into separate logical lines here, preserving its style on every piece.
    let mut logical: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    for span in &line.spans {
        let mut first = true;
        for piece in span.content.split('\n') {
            if !first {
                logical.push(Vec::new());
            }
            first = false;
            let piece = piece.strip_suffix('\r').unwrap_or(piece);
            if !piece.is_empty()
                && let Some(current) = logical.last_mut()
            {
                current.push(Span::styled(piece.to_string(), span.style));
            }
        }
    }

    let mut out: Vec<Line<'static>> = Vec::new();
    for spans in logical {
        let logical_line = Line::from(spans);
        if line_width(&logical_line) <= width {
            out.push(logical_line);
            continue;
        }
        out.extend(wrap_single_line(&logical_line, width));
    }
    if out.is_empty() {
        out.push(Line::from(Vec::<Span<'static>>::new()));
    }
    out
}

/// pi `wrapSingleLine` — greedy word wrap over whitespace-delimited tokens, breaking any token
/// wider than `width` across lines character by character (pi `breakLongWord`).
fn wrap_single_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    let mut wrapped: Vec<Line<'static>> = Vec::new();
    let mut current: Vec<Span<'static>> = Vec::new();
    let mut current_width = 0usize;

    for (token, style, is_whitespace) in tokenize(line) {
        let token_width = str_width(&token);
        if token_width > width && !is_whitespace {
            if !current.is_empty() {
                wrapped.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
            // Break the oversized token; the trailing fragment stays open for more tokens.
            let mut fragment = String::new();
            let mut fragment_width = 0usize;
            for ch in token.chars() {
                let w = str_width(ch.encode_utf8(&mut [0u8; 4]));
                if fragment_width.saturating_add(w) > width {
                    wrapped.push(Line::from(vec![Span::styled(
                        std::mem::take(&mut fragment),
                        style,
                    )]));
                    fragment_width = 0;
                }
                fragment.push(ch);
                fragment_width = fragment_width.saturating_add(w);
            }
            if !fragment.is_empty() {
                current.push(Span::styled(fragment, style));
                current_width = fragment_width;
            }
            continue;
        }
        if current_width.saturating_add(token_width) > width {
            // pi drops the whitespace token that would have started a new line.
            if is_whitespace {
                if !current.is_empty() {
                    wrapped.push(Line::from(std::mem::take(&mut current)));
                    current_width = 0;
                }
                continue;
            }
            if !current.is_empty() {
                wrapped.push(Line::from(std::mem::take(&mut current)));
                current_width = 0;
            }
        }
        if current.is_empty() && is_whitespace {
            continue;
        }
        current.push(Span::styled(token, style));
        current_width = current_width.saturating_add(token_width);
    }
    if !current.is_empty() {
        wrapped.push(Line::from(current));
    }
    if wrapped.is_empty() {
        wrapped.push(Line::from(Vec::<Span<'static>>::new()));
    }
    wrapped
}

/// pi `splitIntoTokensWithAnsi` — split a styled line into alternating word/whitespace tokens,
/// each keeping the style of the span it came from.
fn tokenize(line: &Line<'static>) -> Vec<(String, ratatui::style::Style, bool)> {
    let mut tokens: Vec<(String, ratatui::style::Style, bool)> = Vec::new();
    for span in &line.spans {
        let mut buffer = String::new();
        let mut buffer_is_ws: Option<bool> = None;
        for ch in span.content.chars() {
            let is_ws = ch.is_whitespace();
            if buffer_is_ws != Some(is_ws) {
                if !buffer.is_empty()
                    && let Some(was_ws) = buffer_is_ws
                {
                    tokens.push((std::mem::take(&mut buffer), span.style, was_ws));
                }
                buffer_is_ws = Some(is_ws);
            }
            buffer.push(ch);
        }
        if !buffer.is_empty()
            && let Some(was_ws) = buffer_is_ws
        {
            tokens.push((buffer, span.style, was_ws));
        }
    }
    tokens
}

/// The plain-text projection of a rendered [`Line`] — what a caller with no terminal (a slash
/// command's textual output, a test assertion) reads instead of painting spans.
#[must_use]
pub fn line_text(line: &Line<'_>) -> String {
    line.spans.iter().map(|span| span.content.as_ref()).collect()
}

/// [`line_text`] over a whole rendered frame, newline-joined.
#[must_use]
pub fn lines_text(lines: &[Line<'_>]) -> String {
    lines.iter().map(line_text).collect::<Vec<_>>().join("\n")
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
    fn line_width_sums_span_widths() {
        let line = Line::from(vec![raw("ab"), fg(Role::Accent, "cde")]);
        assert_eq!(line_width(&line), 5);
    }

    #[test]
    fn truncate_appends_ellipsis_only_when_clipping() {
        let line = Line::from(vec![raw("abcdefgh")]);
        assert_eq!(line_text(&clip(&line, 20)), "abcdefgh");
        assert_eq!(line_text(&clip(&line, 6)), "abc...");
        assert_eq!(line_text(&truncate_to_width(&line, 6, "…")), "abcde…");
    }

    #[test]
    fn truncate_to_zero_width_is_empty() {
        let line = Line::from(vec![raw("abc")]);
        assert_eq!(line_text(&clip(&line, 0)), "");
    }

    #[test]
    fn ellipsis_wider_than_budget_replaces_the_body() {
        let line = Line::from(vec![raw("abcdef")]);
        assert_eq!(line_text(&truncate_to_width(&line, 2, "...")), "..");
    }

    #[test]
    fn fit_pads_to_exact_width() {
        let line = Line::from(vec![raw("ab")]);
        let fitted = fit(&line, 5);
        assert_eq!(line_width(&fitted), 5);
        assert_eq!(line_text(&fitted), "ab   ");
    }

    #[test]
    fn right_aligned_pins_the_suffix_to_the_right_edge() {
        let left = Line::from(vec![raw("L")]);
        let right = Line::from(vec![raw("RR")]);
        let row = right_aligned(&left, &right, 10);
        assert_eq!(line_width(&row), 10);
        assert_eq!(line_text(&row), "L       RR");
    }

    #[test]
    fn right_align_status_does_not_pad_the_left_side() {
        let left = Line::from(vec![raw("L")]);
        let right = Line::from(vec![raw("RR")]);
        // fleet-status' variant leaves exactly the computed gap, then clips to width.
        let row = right_align_status(&left, &right, 10);
        assert_eq!(line_text(&row), "L       RR");
    }

    #[test]
    fn wrap_breaks_on_words_and_preserves_style() {
        let line = Line::from(vec![fg(Role::Accent, "alpha beta gamma")]);
        let wrapped = wrap_line(&line, 10);
        assert_eq!(
            wrapped.iter().map(line_text).collect::<Vec<_>>(),
            vec!["alpha beta".to_string(), "gamma".to_string()]
        );
        assert_eq!(wrapped[0].spans[0].style, style(Role::Accent));
    }

    #[test]
    fn wrap_breaks_an_oversized_token() {
        let line = Line::from(vec![raw("abcdefghij")]);
        let wrapped = wrap_line(&line, 4);
        assert_eq!(
            wrapped.iter().map(line_text).collect::<Vec<_>>(),
            vec!["abcd".to_string(), "efgh".to_string(), "ij".to_string()]
        );
    }

    #[test]
    fn wrap_of_empty_text_is_one_empty_line() {
        let wrapped = wrap_line(&Line::from(Vec::<Span<'static>>::new()), 10);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(line_text(&wrapped[0]), "");
    }

    #[test]
    fn wrap_splits_on_embedded_newlines() {
        let line = Line::from(vec![raw("a\nb\nc")]);
        let wrapped = wrap_line(&line, 40);
        assert_eq!(
            wrapped.iter().map(line_text).collect::<Vec<_>>(),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
}
