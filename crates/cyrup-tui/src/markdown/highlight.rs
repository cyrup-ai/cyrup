use super::*;

use std::cell::RefCell;
use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

/// The shared default syntect syntax set (newline-terminated grammars), built once.
fn syntax_set() -> &'static SyntaxSet {
    static SS: OnceLock<SyntaxSet> = OnceLock::new();
    SS.get_or_init(SyntaxSet::load_defaults_newlines)
}

/// Highlight `code` as `lang` into 2-space-indented styled lines (spec/tui/06 §3). When the language
/// is unknown to syntect, every line renders flat in `mdCodeBlock` (auto-detect-off parity, §3.1); on
/// any syntect error the whole block falls back to flat (mirrors `theme.ts:1142-1146` try/catch).
///
/// `memoable` is the fence's CLOSED-ness, decided by the walker from the block's own source
/// ([`super::prepass::fence_is_closed`]) and threaded down through
/// [`MdRenderer::emit_fence_rows`](super::walk). It is a property of the BLOCK, not of this call
/// site: a closed fence's body is frozen and belongs in the memo, while the one fence still growing
/// is the only block that may claim the single resumable cursor (PERF-005 §3.0b rule (b)). Passing
/// a constant `false` here — which is what the first cut of §3.0b did — makes every fence in a turn
/// fight over that one cursor, so two fences evict each other on every frame and neither is ever
/// reused.
pub(super) fn highlight_lines(
    code: &str,
    lang: &str,
    theme: &UiTheme,
    memoable: bool,
) -> Vec<Line<'static>> {
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
    // No row bound: a fence renders in full, so `max_rows` has no meaning here (PERF-005 §3.0a).
    // `token`, not `lang`: it is the value `find_syntax_by_token` above resolved against, and it is
    // what the other caller passes, so both paths key `MemoKey` and the cursor's invalidation test
    // the same way instead of leaning on a trim performed three modules away.
    match highlight_inner(code, token, syntax, ss, theme, usize::MAX, memoable) {
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
/// `max_rows` bounds the parse to the rows the caller will actually read. `transcript::layout`'s
/// `body_line` indexes the returned vector with `.get(idx)` for `idx` in `0..shown`, and syntect is a
/// forward line-at-a-time state machine, so the first `max_rows` rows of a full-block highlight are
/// IDENTICAL to a highlight that stops there. A collapsed `read` block shows `total.min(10)` rows
/// and used to highlight all `total` of them: 356 ms/frame at a 2,000-line file, 99.5% of it
/// discarded, on EVERY frame of the rest of the turn because `push_assistant_delta` keeps bumping
/// the render generation (PERF-005 §3.0a).
///
/// **[CYRUP-DELTA] `max_rows` also narrows the FAULT window.** [`highlight_inner`] abandons the
/// whole block on a syntect error at any line, so today a fault at line 1500 of a 2000-line body
/// renders rows 0..10 flat. Bounded, that fault is never reached and those rows highlight. pi has
/// no equivalent: its `highlightCode` try/catches the whole string (`theme.ts:1275,1284`) and falls
/// back wholesale. The divergence is deliberate — re-parsing 1990 unread lines to decide whether to
/// discard 10 good ones is exactly the cost this bound removes.
pub(crate) fn highlight_code_lines(
    code: &str,
    lang: &str,
    theme: &UiTheme,
    max_rows: usize,
) -> Option<Vec<Line<'static>>> {
    let token = lang.trim();
    if token.is_empty() {
        return None;
    }
    let ss = syntax_set();
    let syntax = ss.find_syntax_by_token(token)?;
    match highlight_inner(code, token, syntax, ss, theme, max_rows, true) {
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

// --- incremental highlighting (PERF-005 §3.0b) -------------------------------------------------

// Incremental highlighting state, thread-local because the whole render path is single-threaded
// (`TranscriptView::render` runs on the run-loop task) and a thread-local costs no synchronisation
// on the hot path.
//
// Two dynamics, two structures, because only ONE block per frame ever grows — the last fence of
// the streaming partial. Everything else (closed fences, settled tool bodies) is immutable, which
// is what answers the "a text-keyed map only grows" objection: the growing block never enters the
// map, and the map holds only finished text.
thread_local! {
    static HL: RefCell<HighlightState> = RefCell::new(HighlightState::default());
}

/// 16 entries covers a turn's worth of closed fences plus the visible tool blocks; past that, the
/// evicted entry rebuilds at exactly today's cost.
const MEMO_CAP: usize = 16;

#[derive(Default)]
struct HighlightState {
    /// THE growing block: the last fence of the streaming partial.
    cursor: Option<HighlightCursor>,
    /// The immutable blocks, evicted by insertion order so it cannot grow unboundedly.
    memo: VecDeque<(MemoKey, Vec<Line<'static>>)>,
}

/// Invalidation key.
///
/// `hash` + `len` of the code text rather than the text itself: the body is immutable once keyed,
/// and storing the length alongside the hash costs one `usize` and removes the collision bet.
/// `lang` because a fence's info string can change while it streams (```` ```ru ```` →
/// ```` ```rust ````); `theme_generation` because the emitted spans carry RESOLVED colours, exactly
/// as `RenderCache` keys on it; `max_rows` because §3.0a makes the row bound part of the result.
#[derive(PartialEq, Eq)]
struct MemoKey {
    hash: u64,
    len: usize,
    lang: String,
    theme_generation: u64,
    max_rows: usize,
}

impl MemoKey {
    fn new(code: &str, lang: &str, theme_generation: u64, max_rows: usize) -> Self {
        let mut h = DefaultHasher::new();
        code.hash(&mut h);
        Self {
            hash: h.finish(),
            len: code.len(),
            lang: lang.to_string(),
            theme_generation,
            max_rows,
        }
    }
}

/// The invariant half of a highlight request: what to parse WITH, as opposed to what to parse.
/// Bundled because threading four `&` parameters through both cache paths pushes `rows_for` past
/// the argument-count lint, and these three always travel together.
struct Ctx<'a> {
    syntax: &'a syntect::parsing::SyntaxReference,
    ss: &'a SyntaxSet,
    theme: &'a UiTheme,
}

/// A resumable highlight of ONE growing code block. syntect is a line-at-a-time state machine and
/// `ParseState` derives `Clone`, so the state after line N stays valid when line N+1 arrives.
struct HighlightCursor {
    lang: String,
    theme_generation: u64,
    /// The row bound this cursor has been advanced under, and part of its identity — `MemoKey`
    /// carries the same field for the same reason.
    ///
    /// Two things depend on it. [`HighlightState::advance_cursor`] stops parsing at the bound but
    /// records the whole delta as consumed, so a cursor advanced under a SMALLER bound would claim
    /// text whose rows were never built; inheriting only at an equal bound is what keeps that
    /// unreachable. And it is what actually separates a growing fence from a settled tool body:
    /// `highlight_lines` passes `usize::MAX`, `highlight_code_lines` passes the rows its caller
    /// will read, so a settled body can no longer match a fence through an empty consumed prefix.
    max_rows: usize,
    /// The exact text already consumed, so the prefix test is a comparison rather than a guess.
    consumed_text: String,
    /// The syntect state after `consumed_text`. THE reason this type exists.
    parse: ParseState,
    rows: Vec<Line<'static>>,
}

impl HighlightState {
    /// Rows for `code`, reusing whatever is still valid.
    ///
    /// The two dynamics are routed apart rather than sharing state. An immutable block
    /// (`memoable` — a settled tool body, **or a closed fence**) goes memo-only: it must never touch
    /// the cursor, because several of them render per frame and each would reset the cursor the
    /// growing fence depends on. A growing block (the streaming fence that has not closed yet) goes
    /// cursor-only: it is never memoised, because its text changes every delta.
    ///
    /// `memoable` is a property of the BLOCK, and every caller must decide it per block rather than
    /// per call site.
    ///
    /// The scope of the one-unclosed-block rule is the DOCUMENT, not the frame: an unclosed fence
    /// swallows everything after it, so a document holds at most one. A frame renders several
    /// documents through this one thread-local — the live streaming partial
    /// (`transcript::cache`), a user message and each committed entry (`transcript::render`) — and
    /// a snippet pasted without its closing fence, or a turn that was aborted mid-fence, leaves a
    /// block that is unclosed FOREVER. So more than one block can reach the cursor path in a frame,
    /// and the honest guarantee is narrower than "nothing else can evict it".
    ///
    /// One slot is still the right number, for two reasons that hold whatever else is on screen.
    /// A committed entry is served by [`RenderCache`](crate::transcript), so a stale unclosed block
    /// reaches this path only on that entry's cache miss — a width change, a theme change, a
    /// generation bump — while the streaming partial is the one document re-rendered every frame.
    /// And a block that closes hands the slot straight back (see the `memoable` arm below), so the
    /// growing tail holds it for exactly as long as it is growing.
    fn rows_for(
        &mut self,
        code: &str,
        lang: &str,
        ctx: &Ctx<'_>,
        max_rows: usize,
        memoable: bool,
    ) -> Option<Vec<Line<'static>>> {
        let Ctx { syntax, ss, theme } = *ctx;
        let key = MemoKey::new(code, lang, theme.generation, max_rows);
        if let Some((_, rows)) = self.memo.iter().find(|(k, _)| *k == key) {
            return Some(rows.clone());
        }

        // Rule (a): never consume the last row into the cursor. `trim_partial_closing_fence` means
        // the final row of a streaming block can be a partial token that changes next delta, so the
        // cursor stops at the last `\n` and the tail is re-parsed each frame on a CLONE of the
        // state — ~130 µs flat, and the difference between correct and subtly-wrong colouring.
        //
        // The split mirrors `code.split('\n')` EXACTLY, which is what the uncached path emits:
        // `stable` holds the rows that are `\n`-terminated, and `tail` is the segment after the
        // final `\n` — always a row, even when empty ("a\n" is two rows, the second empty).
        let stable_len = code.rfind('\n').map_or(0, |i| i + 1);
        let (stable, tail) = code.split_at(stable_len);

        // Whether the cursor is ALREADY tracking this block. Asked before the `memoable` split, not
        // after it: an immutable block may INHERIT a matching cursor — it may just never CREATE one.
        let inherits = self.cursor.as_ref().is_some_and(|c| {
            c.lang == lang
                && c.theme_generation == theme.generation
                // The bound is part of the cursor's identity, and the prefix test alone is not
                // enough to separate two blocks: `stable.starts_with("")` holds for EVERY input,
                // and a cursor's consumed prefix is empty until its block's first newline — so
                // while a streaming fence is still on line one, any same-language block matches it.
                && c.max_rows == max_rows
                && stable.len() >= c.consumed_text.len()
                && stable.starts_with(c.consumed_text.as_str())
        });

        if memoable {
            // A fence that has just closed IS the block the cursor was tracking a frame ago — the
            // same text, now frozen — so it inherits the incremental parse instead of re-reading
            // its own body from line 1. Without this, closing a fence costs a full parse on the
            // frame the delimiter arrives: mid-turn, on the streaming path.
            //
            // Rule (a) survives as the narrower "never CREATE one", and it is the ROW BOUND that
            // enforces it: a fence is highlighted unbounded and a settled tool body is bounded to
            // the rows its caller reads, so a body can never match a growing fence's cursor. Do not
            // reduce that test to language plus prefix — an empty consumed prefix matches every
            // input, and several settled bodies render per frame, any one of which would then claim
            // the cursor and drop it in the arm below.
            //
            // The prefix test, not equality: one delta can carry both new body lines and the
            // closing delimiter, which advances `stable` past what the cursor consumed. Inheriting
            // on a prefix parses only what is new; demanding equality would fall back to a full
            // parse in exactly that case.
            let rows = match inherits.then(|| self.advance_cursor(stable, tail, ctx, max_rows)) {
                Some(Some(rows)) => {
                    // Frozen and about to be keyed: nothing will ask the cursor for this block
                    // again, so hand the slot back to whatever is still growing.
                    self.cursor = None;
                    rows
                }
                // Nothing to inherit, or the inherited cursor faulted and dropped itself.
                _ => highlight_uncached(code, syntax, ss, theme, max_rows)?,
            };
            if self.memo.len() >= MEMO_CAP {
                self.memo.pop_front();
            }
            self.memo.push_back((key, rows.clone()));
            return Some(rows);
        }

        if !inherits {
            self.cursor = Some(HighlightCursor {
                lang: lang.to_string(),
                theme_generation: theme.generation,
                max_rows,
                consumed_text: String::new(),
                parse: ParseState::new(syntax),
                rows: Vec::new(),
            });
        }
        match self.advance_cursor(stable, tail, ctx, max_rows) {
            Some(rows) => Some(rows),
            None => highlight_uncached(code, syntax, ss, theme, max_rows),
        }
    }

    /// Advance the cursor over `stable`, then re-parse `tail` on a CLONE of the state.
    ///
    /// `None` means this block could not be completed from the cursor and the caller must fall
    /// back to [`highlight_uncached`], which reproduces today's whole-block `None` exactly. A fault
    /// while consuming `stable` also drops the cursor, because that fault poisons its state; a
    /// fault on `tail` does not, because the tail is parsed on a clone.
    ///
    /// The caller guarantees a cursor exists and is valid for this block: either it was already
    /// tracking it (`inherits`) or the caller has just installed a fresh one.
    ///
    /// It also guarantees the cursor was built under THIS `max_rows`
    /// ([`HighlightCursor::max_rows`]). The loop below stops at the bound but records the whole
    /// delta as consumed, so advancing a cursor under a smaller bound than it was built for would
    /// leave `consumed_text` ahead of `rows` and make a later inherit splice onto state that was
    /// never built.
    fn advance_cursor(
        &mut self,
        stable: &str,
        tail: &str,
        ctx: &Ctx<'_>,
        max_rows: usize,
    ) -> Option<Vec<Line<'static>>> {
        let Ctx { ss, theme, .. } = *ctx;
        let cursor = self.cursor.as_mut()?;

        // Parse only what is new. A fault while consuming `stable` poisons the cursor's state, so
        // that arm drops it and answers `None`; the caller falls back to the uncached path, which
        // reproduces today's whole-block `None` exactly. (The tail is a separate case — see below.)
        let new_text = stable.get(cursor.consumed_text.len()..).unwrap_or("");
        if !new_text.is_empty() {
            // `new_text` always ends in `\n` here (it is a slice of `stable`), and
            // `"x\n".split('\n')` yields a trailing empty element that is NOT a row — strip the
            // terminator first. An empty `new_text` yields no rows at all, which `split` would
            // wrongly report as one.
            for raw in new_text.strip_suffix('\n').unwrap_or(new_text).split('\n') {
                if cursor.rows.len() >= max_rows {
                    break;
                }
                match highlight_one(raw, &mut cursor.parse, ss, theme) {
                    Some(line) => cursor.rows.push(line),
                    None => {
                        self.cursor = None;
                        return None;
                    }
                }
            }
            cursor.consumed_text.push_str(new_text);
        }

        let mut rows = cursor.rows.clone();
        if rows.len() < max_rows {
            let mut tail_parse = cursor.parse.clone();
            // The tail is parsed on a CLONE, so a fault here (the `?` returning `None`) does not
            // poison the cursor: its state is still exactly the state after `stable`. Keep it — the
            // tail is re-parsed every frame anyway, and the next delta may well parse cleanly.
            let line = highlight_one(tail, &mut tail_parse, ss, theme)?;
            rows.push(line);
        }
        rows.truncate(max_rows);
        Some(rows)
    }
}

/// One line, against a running [`ParseState`] — the body of today's loop, lifted so the cached and
/// uncached paths cannot drift apart. `None` on any parser/scope error, exactly as before.
fn highlight_one(
    raw: &str,
    parse: &mut ParseState,
    ss: &SyntaxSet,
    theme: &UiTheme,
) -> Option<Line<'static>> {
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
    Some(Line::from(spans))
}

/// Stateful syntect highlight: parse each line, walk the scope stack, map the top matching scope to a
/// theme syntax role (spec/tui/06 §3.2). Returns `None` on any parser/scope error → caller falls back.
///
/// Bounded to `max_rows` rows and served from the incremental caches in [`HighlightState`] where it
/// can be (PERF-005 §3.0a/§3.0b). The caches sit HERE, below `highlight_lines`/`highlight_code_lines`'
/// language and fault gates, so an unknown language never reaches them and both fallbacks are
/// untouched.
fn highlight_inner(
    code: &str,
    lang: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
    theme: &UiTheme,
    max_rows: usize,
    memoable: bool,
) -> Option<Vec<Line<'static>>> {
    HL.with(|hl| {
        // `try_borrow_mut`, not `borrow_mut`: `RefCell` panics on re-entrancy and the workspace
        // denies `clippy::panic`. The mermaid fallback re-enters `emit_fence_rows`, so the
        // defensive arm is not theoretical — and it is exactly today's uncached behaviour.
        let Ok(mut st) = hl.try_borrow_mut() else {
            return highlight_uncached(code, syntax, ss, theme, max_rows);
        };
        st.rows_for(code, lang, &Ctx { syntax, ss, theme }, max_rows, memoable)
    })
}

/// Today's loop, verbatim — the uncached path, and the body every cached path delegates to.
fn highlight_uncached(
    code: &str,
    syntax: &syntect::parsing::SyntaxReference,
    ss: &SyntaxSet,
    theme: &UiTheme,
    max_rows: usize,
) -> Option<Vec<Line<'static>>> {
    let mut parse = ParseState::new(syntax);
    let mut out: Vec<Line<'static>> = Vec::new();
    for raw in code.split('\n').take(max_rows) {
        out.push(highlight_one(raw, &mut parse, ss, theme)?);
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
