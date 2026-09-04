use super::*;

/// Placeholder delimiters for a tokenized math span: `\u{f0006}<index>\u{f0007}`.
///
/// Private-use code points, so CommonMark gives them no meaning at all and they survive the parse
/// as ordinary `Event::Text` — which is exactly the property upstream gets for free by owning its
/// tokenizer.
pub(super) const MATH_START: char = '\u{f0006}';
pub(super) const MATH_END: char = '\u{f0007}';

/// M12 — run Pi's `LATEX_MARKDOWN_EXTENSIONS` (`markdown.ts:123-144`) over the source and replace
/// every math span with a placeholder, returning the rewritten source plus the rendered text per
/// index.
///
/// **Mechanism divergence, stated because there is no way around it.** marked lets an extension
/// register a `block`-level and an `inline`-level tokenizer that the lexer consults *first* at each
/// position (`markdown.ts:123-144`, `:175`); pulldown-cmark has no such hook, and by the time it
/// emits events `\[x\]` has already been consumed as two CommonMark backslash escapes and printed
/// as `[x]`. So the tokenizers run as a pre-pass over the raw source instead. What that buys is the
/// same tokens; what it costs is the interleaving, which this pass reproduces by hand:
///
/// * **Fenced code blocks are skipped.** A fence is a block token, and marked never re-lexes its
///   body, so `$$` inside ```` ``` ```` is not math.
/// * **Inline code spans are skipped.** marked's inline extensions run before `codespan`, but
///   `tokenizeInlineLatex` is offered the text starting at the backtick and declines it (no `$`,
///   `\(` or `\[` prefix), after which `codespan` swallows the whole span. Same outcome, reached
///   by construction here.
/// * **A block token is only tried at a block start** — the start of the document or after a blank
///   line — which is where marked's block lexer would offer it.
///
/// Rendering happens here rather than at event time because the fallback is defined on the token's
/// `raw` (`markdown.ts:509`, `:650`), which the event stream no longer carries.
pub(super) fn latex_prepass(source: &str) -> (String, Vec<Vec<String>>) {
    let ch: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut math: Vec<Vec<String>> = Vec::new();
    let mut i = 0usize;
    // "At a block start": nothing but blank lines behind us on this line.
    let mut at_block_start = true;
    while i < ch.len() {
        let Some(c) = ch.get(i).copied() else { break };
        // ── fenced code block: copy through to the closing fence.
        if at_block_start && let Some((fence, indent)) = fence_at(&ch, i) {
            let end = fence_block_end(&ch, i, fence, indent);
            out.push_str(&chars_range(&ch, i, end));
            i = end;
            at_block_start = true;
            continue;
        }
        // ── inline code span: copy the whole span through untouched.
        if c == '`' {
            let end = code_span_end(&ch, i);
            out.push_str(&chars_range(&ch, i, end));
            i = end;
            at_block_start = false;
            continue;
        }
        // ── block-level math, offered only where marked's block lexer would offer it.
        //
        // `ch.get(i..)` and not `ch[i..]`: the no-panic policy denies `indexing_slicing`, and a
        // `skip(i).collect()` here would re-copy the tail of the document at EVERY position —
        // quadratic on a long assistant message that redraws on every stream delta.
        let Some(rest) = ch.get(i..) else { break };
        if at_block_start && let Some(token) = latex::tokenize_block(rest) {
            let rendered = latex::render_token(&token, true);
            // A block token is its own block upstream (`case "latexBlock"` pushes one line per `\n`,
            // `markdown.ts:511-513`), so the placeholder is followed by a blank line rather than
            // being allowed to run into whatever follows. Nothing is prepended: `at_block_start`
            // already means the parser is at one. The `{0,3}` indent the tokenizer swallowed is
            // re-emitted, or a `$$` block nested in a list item would fall out of its item.
            let mut indent = 0usize;
            while indent < 3 && ch.get(i + indent) == Some(&' ') {
                indent += 1;
            }
            out.push_str(&" ".repeat(indent));
            push_math_placeholder(&mut out, &mut math, &rendered);
            out.push_str("\n\n");
            i += token.raw_len;
            at_block_start = true;
            continue;
        }
        if matches!(c, '$' | '\\')
            && let Some(token) = latex::tokenize_inline(rest)
        {
            let rendered = latex::render_token(&token, false);
            push_math_placeholder(&mut out, &mut math, &rendered);
            i += token.raw_len;
            at_block_start = false;
            continue;
        }
        out.push(c);
        at_block_start = c == '\n' && (i == 0 || line_before_is_blank(&ch, i));
        i += 1;
    }
    (out, math)
}

/// Record `rendered` (split into rows) and emit its placeholder.
fn push_math_placeholder(out: &mut String, math: &mut Vec<Vec<String>>, rendered: &str) {
    let index = math.len();
    math.push(rendered.split('\n').map(str::to_string).collect());
    out.push(MATH_START);
    out.push_str(&index.to_string());
    out.push(MATH_END);
}

fn chars_range(ch: &[char], from: usize, to: usize) -> String {
    ch.iter().skip(from).take(to.saturating_sub(from)).collect()
}

/// Whether the line ending at `nl` (a `\n`) had nothing but whitespace on it.
fn line_before_is_blank(ch: &[char], nl: usize) -> bool {
    let mut i = nl;
    while i > 0 {
        let Some(c) = ch.get(i - 1) else { break };
        if *c == '\n' {
            break;
        }
        if !c.is_whitespace() {
            return false;
        }
        i -= 1;
    }
    true
}

/// A code fence opening at `i`: `(fence char + length, indent)`.
fn fence_at(ch: &[char], i: usize) -> Option<((char, usize), usize)> {
    let mut j = i;
    let mut indent = 0usize;
    while indent < 3 && ch.get(j) == Some(&' ') {
        indent += 1;
        j += 1;
    }
    let c = ch.get(j).copied()?;
    if c != '`' && c != '~' {
        return None;
    }
    let mut n = 0usize;
    while ch.get(j + n) == Some(&c) {
        n += 1;
    }
    if n < 3 { None } else { Some(((c, n), indent)) }
}

/// Index just past the closing fence (or end of input).
fn fence_block_end(ch: &[char], start: usize, fence: (char, usize), _indent: usize) -> usize {
    // Skip the opening fence's own line.
    let mut i = start;
    while i < ch.len() && ch.get(i) != Some(&'\n') {
        i += 1;
    }
    while i < ch.len() {
        i += 1; // past the newline
        let line_start = i;
        if let Some((f, _)) = fence_at(ch, line_start)
            && f.0 == fence.0
            && f.1 >= fence.1
        {
            let mut j = line_start;
            while j < ch.len() && ch.get(j) != Some(&'\n') {
                j += 1;
            }
            return j.min(ch.len());
        }
        while i < ch.len() && ch.get(i) != Some(&'\n') {
            i += 1;
        }
    }
    ch.len()
}

/// Index just past an inline code span opening at `i`, or past the backtick run when it never
/// closes (CommonMark then treats the run as literal text, and so do we).
fn code_span_end(ch: &[char], i: usize) -> usize {
    let mut n = 0usize;
    while ch.get(i + n) == Some(&'`') {
        n += 1;
    }
    let mut j = i + n;
    while j < ch.len() {
        if ch.get(j) == Some(&'`') {
            let mut m = 0usize;
            while ch.get(j + m) == Some(&'`') {
                m += 1;
            }
            if m == n {
                return j + m;
            }
            j += m;
            continue;
        }
        j += 1;
    }
    i + n
}

/// Trim a *partial* closing code fence from a streaming buffer so the live markdown block does not
/// flip between open/closed while the fence (`` ` `` → `` `` `` → ```` ``` ````) streams in
/// (`markdown.ts:25-48`, pi#5825). Only the **last** line is inspected; apply to the live buffer only.
pub fn trim_partial_closing_fence(text: &str) -> String {
    // Count fence markers (lines that are exactly N backticks/tildes after trimming). An *odd* count
    // means a code block is currently open; a trailing line that is a *short* run of the same fence
    // char is a partial closing fence and is stripped to keep the block stable.
    let mut fence_char: Option<char> = None;
    let mut open = false;
    let mut open_len = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if !open {
            // Opening fence: a leading run of ≥3 fence chars (an info string may follow, e.g. ```rust).
            if let Some((c, n)) = leading_fence(t) {
                open = true;
                fence_char = Some(c);
                open_len = n;
            }
        } else if let Some((c, n, pure)) = leading_fence(t).map(|(c, n)| (c, n, is_pure_fence(t))) {
            // Closing fence must be a *pure* run of the same char, at least as long as the opener.
            if pure && Some(c) == fence_char && n >= open_len {
                open = false;
                fence_char = None;
            }
        }
    }
    if !open {
        return text.to_string();
    }
    // A code block is open. If the final line is a *partial* fence (same char, shorter than the
    // opener), drop it so the renderer keeps showing the open block unchanged.
    let last_start = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
    let last = text.get(last_start..).unwrap_or("").trim();
    if !last.is_empty()
        && let Some(fc) = fence_char
        && last.chars().all(|c| c == fc)
        && last.chars().count() < open_len
    {
        return text
            .get(..last_start.saturating_sub(1))
            .unwrap_or("")
            .to_string();
    }
    text.to_string()
}

/// If `line` begins with a run of ≥3 fence chars (all `` ` `` or all `~`), return its char + run
/// length. An info string may follow (opening fence ```` ```rust ````).
fn leading_fence(line: &str) -> Option<(char, usize)> {
    let first = line.chars().next()?;
    if first != '`' && first != '~' {
        return None;
    }
    let n = line.chars().take_while(|c| *c == first).count();
    if n >= 3 { Some((first, n)) } else { None }
}

/// Whether `line` is *only* a fence run (a valid closing fence — no trailing info string).
fn is_pure_fence(line: &str) -> bool {
    match line.chars().next() {
        Some(c @ ('`' | '~')) => line.chars().all(|x| x == c) && line.chars().count() >= 3,
        _ => false,
    }
}

/// Whether a fenced block's source carries its CLOSING delimiter.
///
/// The text in front of a closing fence can never change in a later delta — the delimiter is what
/// puts it out of reach of an append — so "closed" is exactly "immutable", which is exactly the
/// memo-eligibility `super::highlight` routes on (PERF-005 §3.0b rule (b)). A fence that is still
/// growing is the one block a frame may not memoise, and it is also the only block that may claim
/// the single resumable cursor.
///
/// `raw` is `Start(Tag::CodeBlock)`'s source range — the opening delimiter through the closing one.
/// `MdRenderer::start` already relies on that range being the whole element for
/// `Start(Table)`; this is the same property read for the fence.
///
/// A *partial* closing fence cannot reach here to be mistaken for a complete one:
/// [`trim_partial_closing_fence`] only ever strips a short trailing run from a live buffer, and
/// never appends a balancing delimiter, so a fence caught mid-delimiter arrives open.
///
/// An indented code block has no delimiter and answers `false` — correct, and moot besides: its
/// language token is empty, so it never reaches a highlighter cache at all.
pub(super) fn fence_is_closed(raw: &str) -> bool {
    let mut lines = raw.trim_end_matches(['\n', '\r']).lines();
    // The opener, and the fence char + run length a closer has to match.
    let Some((ch, open_len)) = lines.next().map(str::trim).and_then(leading_fence) else {
        return false;
    };
    // `next_back` on what REMAINS after the opener: a block that is only its opening fence has no
    // remainder, so it is correctly still open rather than closed by its own first line.
    lines.next_back().map(str::trim).is_some_and(|last| {
        is_pure_fence(last) && last.starts_with(ch) && last.chars().count() >= open_len
    })
}

#[cfg(test)]
mod fence_closure {
    use super::fence_is_closed;
    use pulldown_cmark::{Event, Options, Parser, Tag};

    /// The predicate itself, over the cases that decide a fence's memo-eligibility.
    #[test]
    fn a_fence_is_closed_only_when_it_carries_its_own_delimiter() {
        assert!(
            fence_is_closed("```rust\nfn a() {}\n```"),
            "a plainly closed fence"
        );
        assert!(fence_is_closed("```rust\n```"), "empty, but closed");
        assert!(
            fence_is_closed("```rust\nfn a() {}\n```\n"),
            "a trailing newline is not content"
        );
        assert!(
            fence_is_closed("````\n```\n````"),
            "a 4-char opener; the interior ``` is body"
        );
        assert!(
            fence_is_closed("~~~\n```\n~~~"),
            "a ~ fence is not closed by a ` run"
        );

        assert!(
            !fence_is_closed("```rust\nfn a() {"),
            "the growing tail of a stream"
        );
        assert!(
            !fence_is_closed("```rust"),
            "the opener alone does not close itself"
        );
        assert!(
            !fence_is_closed("````\nfn a() {}\n```"),
            "a closer must be at least as long"
        );
        assert!(
            !fence_is_closed("    indented code"),
            "an indented block has no delimiter"
        );
    }

    /// The property the predicate is USELESS without, and whose failure would be silent.
    ///
    /// `fence_is_closed` reads `Start(Tag::CodeBlock)`'s source range. If that range did not span
    /// the closing delimiter, every fence would answer `false`, every fence would take the single
    /// resumable cursor, and two fences in a turn would evict each other on every frame — which is
    /// precisely the pre-PERF-005 cost, reached with no failing test and no visible symptom. So the
    /// range property is pinned here rather than assumed.
    #[test]
    fn the_code_block_start_range_spans_the_closing_delimiter() {
        // One settled fence, then prose, then the fence still streaming in.
        let doc = "text\n\n```rust\nfn a() {}\n```\n\nmore\n\n```rust\nfn b() {\n";
        let opts =
            Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
        let verdicts: Vec<bool> = Parser::new_ext(doc, opts)
            .into_offset_iter()
            .filter(|(ev, _)| matches!(ev, Event::Start(Tag::CodeBlock(_))))
            .map(|(_, range)| fence_is_closed(doc.get(range).unwrap_or("")))
            .collect();
        assert_eq!(
            verdicts,
            vec![true, false],
            "the settled fence must be memo-eligible and only the growing tail may claim the \
             cursor (PERF-005 §3.0b rule (b)) — got {verdicts:?}",
        );
    }
}
