use super::*;

/// The shared default syntect syntax set (newline-terminated grammars), built once.
fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Highlight `code` as `lang` into 2-space-indented styled lines (spec/tui/06 §3). When the language
/// is unknown to syntect, every line renders flat in `mdCodeBlock` (auto-detect-off parity, §3.1); on
/// any syntect error the whole block falls back to flat (mirrors `theme.ts:1142-1146` try/catch).
pub(super) fn highlight_lines(code: &str, lang: &str, theme: &UiTheme) -> Vec<Line<'static>> {
    let flat = || -> Vec<Line<'static>> {
        code.split('\n')
            .map(|l| Line::styled(format!("  {l}"), theme.md_code_block_style()))
            .collect()
    };
    let token = lang.trim();
    if token.is_empty() {
        return flat();
    }
    let ss = syntax_set();
    let Some(syntax) = ss.find_syntax_by_token(token) else {
        return flat();
    };
    match highlight_inner(code, syntax, ss, theme) {
        Some(lines) if !lines.is_empty() => lines,
        _ => flat(),
    }
}

/// [`highlight_lines`] without the markdown code-block indent — Pi's bare
/// `highlightCode(text, lang)` (`theme.ts:1270-1285`), which is what the `read`/`write` tool bodies
/// call (`core/tools/read.ts:185`, `write.ts:152-154`). Those bodies are NOT inside a fenced block,
/// so they carry none of `markdown.ts`'s 2-space gutter.
///
/// `None` means "no highlighting applies" — an empty/unknown language token, or a syntect fault —
/// and the caller then renders the raw text in its own flat colour, exactly like Pi's
/// `lang ? … : theme.fg("toolOutput", …)` ternary. This deliberately does NOT fall back to
/// `mdCodeBlock`: that whole-block fallback belongs to the markdown path, and a `read` of a file
/// with an unknown extension must stay `toolOutput` grey.
pub(crate) fn highlight_code_lines(
    code: &str,
    lang: &str,
    theme: &UiTheme,
) -> Option<Vec<Line<'static>>> {
    let token = lang.trim();
    if token.is_empty() {
        return None;
    }
    let ss = syntax_set();
    let syntax = ss.find_syntax_by_token(token)?;
    match highlight_inner(code, syntax, ss, theme) {
        Some(lines) if !lines.is_empty() => Some(
            lines
                .into_iter()
                .map(|mut l| {
                    // `highlight_inner` opens every row with the markdown gutter (`Span::raw("  ")`,
                    // `:1786`); the tool bodies want the row flush.
                    if l.spans.first().is_some_and(|s| s.content.as_ref() == "  ") {
                        l.spans.remove(0);
                    }
                    l
                })
                .collect(),
        ),
        _ => None,
    }
}

/// Stateful syntect highlight: parse each line, walk the scope stack, map the top matching scope to a
/// theme syntax role (spec/tui/06 §3.2). Returns `None` on any parser/scope error → caller falls back.
fn highlight_inner(
    code: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
    theme: &UiTheme,
) -> Option<Vec<Line<'static>>> {
    let mut parse = ParseState::new(syntax);
    let mut out: Vec<Line<'static>> = Vec::new();
    for raw in code.split('\n') {
        let line_nl = format!("{raw}\n");
        let ops = parse.parse_line(&line_nl, ss).ok()?;
        let mut stack = ScopeStack::new();
        let mut spans: Vec<Span<'static>> = vec![Span::raw("  ")];
        let mut last = 0usize;
        for (idx, op) in ops {
            if idx > last
                && let Some(piece) = line_nl.get(last..idx)
            {
                push_code_span(&mut spans, piece, &stack, theme);
            }
            stack.apply(&op).ok()?;
            last = idx;
        }
        if let Some(piece) = line_nl.get(last..) {
            push_code_span(&mut spans, piece, &stack, theme);
        }
        out.push(Line::from(spans));
    }
    Some(out)
}

/// Push a highlighted span (newline-stripped) styled by the most specific matching scope.
///
/// T5 (TUI-FIDELITY §2): a scope the table does not classify gets **no style at all**, not
/// `mdCodeBlock`. Pi runs the block through cli-highlight and pushes the result verbatim —
/// `lines.push(`${indent}${hlLine}`)`, v0.84.1 `tui/src/components/markdown.ts:526` — and
/// cli-highlight only emits an escape for the 24 classes `buildCliHighlightTheme` defines
/// (`theme.ts:1119-1145`). Everything else (identifiers, whitespace, plain text) carries no escape
/// and renders at the terminal's default foreground. `mdCodeBlock` is a *whole-block* fallback in
/// Pi, reached only when the language is unknown or the highlighter throws (`theme.ts:1275`,
/// `:1284`); that path is [`highlight_lines`]'s `flat()`, not this one. Defaulting each unclassified
/// run to `mdCodeBlock` painted roughly half of every code block `#b5bd68` green.
fn push_code_span(spans: &mut Vec<Span<'static>>, piece: &str, stack: &ScopeStack, theme: &UiTheme) {
    let text = piece.trim_end_matches('\n');
    if text.is_empty() {
        return;
    }
    let style = scope_style(stack, theme).unwrap_or_default();
    spans.push(Span::styled(text.to_string(), style));
}

/// Map the scope stack to a theme syntax style.
///
/// Two passes, in this order:
/// 1. **Container scopes** (T6) — an enclosing `meta.annotation` / `meta.preprocessor` colours the
///    whole construct `muted`, because Pi's highlighter emits a `meta` class for a Rust attribute /
///    Python decorator / C preprocessor line and maps it to `muted` (v0.84.1 `theme.ts:1128`). This
///    has to beat the deepest-first walk: syntect nests `punctuation.definition.annotation.rust`
///    *inside* `meta.annotation.rust`, so a deepest-first match would recolour only the `#`.
///    A nested **string/comment literal escapes** the container and keeps its own colour, because
///    highlight.js's `meta` modes declare sub-modes that cli-highlight wraps in their own class —
///    see [`UiTheme::syntax_meta_nested_style`]. That is what keeps the `"wasm-host"` in
///    `#[cfg(feature = "wasm-host")]` and the `<stdio.h>` in `#include <stdio.h>` at
///    `syntaxString` while the annotation around them stays `muted`.
/// 2. **Deepest-first** — the innermost scope that the prefix table knows wins, so a `string` inside
///    a `meta.function` still comes out as a string.
fn scope_style(stack: &ScopeStack, theme: &UiTheme) -> Option<Style> {
    let container = stack
        .as_slice()
        .iter()
        .find_map(|scope| theme.syntax_meta_container_style(&scope.build_string()));
    if let Some(container) = container {
        for scope in stack.as_slice().iter().rev() {
            if let Some(style) = theme.syntax_meta_nested_style(&scope.build_string()) {
                return Some(style);
            }
        }
        return Some(container);
    }
    for scope in stack.as_slice().iter().rev() {
        let s = scope.build_string();
        if let Some(style) = theme.syntax_style_for_scope(&s) {
            return Some(style);
        }
    }
    None
}
