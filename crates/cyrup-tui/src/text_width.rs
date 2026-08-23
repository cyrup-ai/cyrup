//! The crate's **one** visible-width measurement and truncation primitive set — pi's
//! `visibleWidth` / `truncateToWidth` (`packages/tui/src/utils.ts:240-295`, `:1053-1092`).
//!
//! Everything that measures a terminal column count or cuts a string to one goes through here.
//! The canon previously lived in [`crate::settings_selector`] because `settings-list.ts` is the
//! densest caller in the port (four calls: `:108`, `:143`, `:149`, `:239`) and the other
//! `truncateToWidth` consumers in that group imported it rather than growing a second copy — but
//! two modules grew one anyway (`session_selector`'s and `status`'s were both `for ch in
//! s.chars()`), so the canon moved to a top-level module of its own where a second copy has no
//! excuse. **Never `chars().count()` and never `chars().take(n)`**: measurements must be
//! unicode-width correct and cuts must be grapheme-atomic, or a CJK label under-measures and a ZWJ
//! family emoji is reduced to its leading component.

use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;

/// Visible (terminal column) width of `s` — unicode-width correct via ratatui's `Span::width`,
/// which is Pi's `visibleWidth` (`packages/tui/src/utils.ts`). **Never `chars().count()`**: four
/// separate width measurements in this crate have carried that defect.
pub(crate) fn str_width(s: &str) -> usize {
    Span::raw(s).width()
}

/// The visible width of a span vector.
pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// The kept prefix of `text` that fits in `budget` visible columns, accumulated one **grapheme
/// cluster** at a time so a ZWJ sequence or a combining mark is never cut in half
/// (`utils.ts:1100-1110` keeps a cluster only whole).
fn clip(text: &str, budget: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for g in text.graphemes(true) {
        let gw = str_width(g);
        if w.saturating_add(gw) > budget {
            break;
        }
        out.push_str(g);
        w = w.saturating_add(gw);
    }
    out
}

/// [`truncate_to_width`] split into `(kept text, was it truncated)`, so a caller can style the
/// ellipsis differently from the body — which is what the third footer line needs
/// (`truncateToWidth(statusLine, width, theme.fg("dim", "..."))`, `footer.ts:240`: the ellipsis
/// carries the colour, the statuses do not). `ellipsis` is measured but never appended here.
///
/// This is the split primitive [`truncate_to_width`] is defined on top of; it does not carry that
/// function's `maxWidth <= 0` and clip-the-ellipsis arms, because a caller colouring the ellipsis
/// itself decides whether to emit one.
pub(crate) fn truncate_parts(s: &str, max: usize, ellipsis: &str) -> (String, bool) {
    if str_width(s) <= max {
        return (s.to_string(), false);
    }
    (clip(s, max.saturating_sub(str_width(ellipsis))), true)
}

/// `truncateToWidth(text, maxWidth, ellipsis)` — `packages/tui/src/utils.ts:1053-1092`.
///
/// Width-aware and **grapheme-atomic**: the kept prefix is accumulated one grapheme cluster at a
/// time, so a ZWJ sequence or a combining mark is never cut in half, and each cluster is measured
/// with [`str_width`] rather than counted. `maxWidth <= 0` yields `""` (`:1059-1061`); when the
/// ellipsis alone is at least as wide as the budget upstream clips the *ellipsis* and emits that
/// (`:1067-1079`), which is what the `ew >= max` arm reproduces.
pub(crate) fn truncate_to_width(s: &str, max: usize, ellipsis: &str) -> String {
    if max == 0 {
        return String::new();
    }
    let ew = str_width(ellipsis);
    if ew >= max && str_width(s) > max {
        return clip(ellipsis, max);
    }
    let (mut body, truncated) = truncate_parts(s, max, ellipsis);
    if truncated {
        body.push_str(ellipsis);
    }
    body
}

/// `truncateToWidth` applied to an already-styled row, preserving each span's own style across the
/// cut (`settings-list.ts:143` truncates the *composed* string, ANSI and all — pi's
/// `truncateToWidth` re-emits the pending codes with the next kept character, `utils.ts:1119-1122`).
/// Reducing the row to one span before truncating would flatten the accent/muted split.
///
/// The ellipsis is appended as a **bare [`Span::raw`]** — deliberately unlike
/// [`truncate_spans_to_width`], which inherits the last kept span's style. Both behaviours are
/// upstream-faithful for their own callers; see that function's note.
pub(crate) fn truncate_line_to_width(line: Line<'static>, max: usize, ellipsis: &str) -> Line<'static> {
    if line.width() <= max {
        return line;
    }
    if max == 0 {
        return Line::from("");
    }
    let budget = max.saturating_sub(str_width(ellipsis));
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut w = 0usize;
    'spans: for span in line.spans {
        let style = span.style;
        let mut kept = String::new();
        for g in span.content.graphemes(true) {
            let gw = str_width(g);
            if w.saturating_add(gw) > budget {
                if !kept.is_empty() {
                    out.push(Span::styled(kept, style));
                }
                break 'spans;
            }
            kept.push_str(g);
            w = w.saturating_add(gw);
        }
        if !kept.is_empty() {
            out.push(Span::styled(kept, style));
        }
    }
    if !ellipsis.is_empty() {
        out.push(Span::raw(ellipsis.to_string()));
    }
    Line::from(out)
}

/// [`truncate_to_width`] over a styled span vector, preserving each span's own style across the cut
/// — pi truncates the assembled ANSI string and its truncator carries the pending escapes forward
/// (`tui/src/utils.ts:1119-1122`), so the colours survive.
///
/// **Deliberate divergence from [`truncate_line_to_width`]:** the ellipsis here inherits the *last
/// kept span's* style rather than being emitted raw. That is what `session-selector.ts:509`
/// (`truncateToWidth(line, width)`) produces — the row it cuts is already wrapped in the row's own
/// colour, so pi's pending-ANSI carry-forward paints the `...` in it too, and a row queued for
/// deletion keeps a red ellipsis. The settings/config rows the [`Line`] form serves are composed of
/// differently-styled columns with no single enclosing colour, so their ellipsis stays unstyled.
/// The two are kept separate rather than unified so neither call site silently changes colour.
pub(crate) fn truncate_spans_to_width(
    spans: Vec<Span<'static>>,
    max: usize,
    ellipsis: &str,
) -> Vec<Span<'static>> {
    if spans_width(&spans) <= max {
        return spans;
    }
    if max == 0 {
        return Vec::new();
    }
    let budget = max.saturating_sub(str_width(ellipsis));
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0usize;
    for span in spans {
        if used >= budget {
            break;
        }
        let remaining = budget.saturating_sub(used);
        if span.width() <= remaining {
            used = used.saturating_add(span.width());
            out.push(span);
        } else {
            let kept = truncate_to_width(&span.content, remaining, "");
            if !kept.is_empty() {
                out.push(Span::styled(kept, span.style));
            }
            break;
        }
    }
    if !ellipsis.is_empty() {
        let style = out.last().map(|s| s.style).unwrap_or_default();
        out.push(Span::styled(ellipsis.to_string(), style));
    }
    out
}
