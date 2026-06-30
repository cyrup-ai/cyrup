//! Unified-diff renderer (spec/tui/06 §6; 1:1 port of `components/diff.ts`).
//!
//! Renders a pre-formatted diff string — each line shaped `"+123 content"`, `"-123 content"`,
//! `" 123 content"`, or a bare context/header line — into styled [`Line`]s:
//! - **context** lines in `toolDiffContext` (gray),
//! - **removed** (`-`) lines in `toolDiffRemoved` (red),
//! - **added** (`+`) lines in `toolDiffAdded` (green),
//! - and, when a hunk is exactly **one** removed line immediately followed by **one** added line, the
//!   changed word-runs are emphasized with reversed video (`renderIntraLineDiff`, `diff.ts:33-71`),
//!   leading indentation excluded from the highlight.
//!
//! Tabs are expanded to three spaces for stable rendering (`replaceTabs`, `diff.ts:18-20`).

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use crate::theme::UiTheme;

/// A parsed diff line: the leading sign, the (possibly empty) line-number field, and the content.
struct Parsed {
    prefix: char,
    line_num: String,
    content: String,
}

/// Parse one diff line per `diff.ts:8-12` regex `^([+-\s])(\s*\d*)\s(.*)$`: a sign, then a run of
/// spaces/digits forming the line-number column, then a single separating space, then the content.
fn parse_diff_line(line: &str) -> Option<Parsed> {
    let mut chars = line.char_indices();
    let (_, first) = chars.next()?;
    if !matches!(first, '+' | '-' | ' ') {
        return None;
    }
    // The number field is the maximal run of spaces/digits; it must be followed by exactly one space
    // (the separator), so we scan to the last space that precedes the content.
    let rest = line.get(1..)?;
    // Find the split: greedily take spaces+digits, then require a space before content.
    let bytes = rest.as_bytes();
    let mut i = 0usize;
    while let Some(&b) = bytes.get(i) {
        if b == b' ' || b.is_ascii_digit() {
            i += 1;
        } else {
            break;
        }
    }
    // `i` now points just past the spaces/digits run. The regex consumes `(\s*\d*)\s`, i.e. the run
    // then one trailing space. The run is greedy but a `\s` must remain — so back up to the last space.
    if i == 0 {
        // No number field and no separating space → not a structured line.
        return None;
    }
    // The separator space is the last char of the run when the run ends in a space; otherwise there is
    // no separator and this is not a structured line.
    if rest.get(i - 1..i) != Some(" ") {
        return None;
    }
    let num = rest.get(..i - 1).unwrap_or("");
    let content = rest.get(i..).unwrap_or("");
    Some(Parsed { prefix: first, line_num: num.to_string(), content: content.to_string() })
}

/// Expand tabs to three spaces (`diff.ts:18-20`).
fn replace_tabs(s: &str) -> String {
    s.replace('\t', "   ")
}

/// Render a pre-formatted `diff_text` into styled lines (`renderDiff`, `diff.ts:78-146`).
pub fn render_diff(diff_text: &str, theme: &UiTheme) -> Vec<Line<'static>> {
    let lines: Vec<&str> = diff_text.split('\n').collect();
    let ctx = theme.tool_diff_context_style();
    let rem = theme.tool_diff_removed_style();
    let add = theme.tool_diff_added_style();
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut i = 0usize;
    while let Some(&line) = lines.get(i) {
        let Some(parsed) = parse_diff_line(line) else {
            out.push(Line::styled(line.to_string(), ctx));
            i += 1;
            continue;
        };
        match parsed.prefix {
            '-' => {
                // Collect consecutive removed, then consecutive added lines.
                let mut removed: Vec<Parsed> = Vec::new();
                while let Some(&l) = lines.get(i) {
                    match parse_diff_line(l) {
                        Some(p) if p.prefix == '-' => {
                            removed.push(p);
                            i += 1;
                        }
                        _ => break,
                    }
                }
                let mut added: Vec<Parsed> = Vec::new();
                while let Some(&l) = lines.get(i) {
                    match parse_diff_line(l) {
                        Some(p) if p.prefix == '+' => {
                            added.push(p);
                            i += 1;
                        }
                        _ => break,
                    }
                }
                if let (1, 1, Some(r), Some(a)) =
                    (removed.len(), added.len(), removed.first(), added.first())
                {
                    let (rspans, aspans) =
                        intra_line(&replace_tabs(&r.content), &replace_tabs(&a.content), rem, add);
                    out.push(prefixed_line(format!("-{} ", r.line_num), rem, rspans));
                    out.push(prefixed_line(format!("+{} ", a.line_num), add, aspans));
                } else {
                    for r in &removed {
                        out.push(Line::styled(
                            format!("-{} {}", r.line_num, replace_tabs(&r.content)),
                            rem,
                        ));
                    }
                    for a in &added {
                        out.push(Line::styled(
                            format!("+{} {}", a.line_num, replace_tabs(&a.content)),
                            add,
                        ));
                    }
                }
            }
            '+' => {
                out.push(Line::styled(
                    format!("+{} {}", parsed.line_num, replace_tabs(&parsed.content)),
                    add,
                ));
                i += 1;
            }
            _ => {
                out.push(Line::styled(
                    format!(" {} {}", parsed.line_num, replace_tabs(&parsed.content)),
                    ctx,
                ));
                i += 1;
            }
        }
    }
    out
}

/// Build a `Line` from a styled prefix plus already-styled content spans.
fn prefixed_line(prefix: String, prefix_style: Style, mut spans: Vec<Span<'static>>) -> Line<'static> {
    let mut all = Vec::with_capacity(spans.len() + 1);
    all.push(Span::styled(prefix, prefix_style));
    all.append(&mut spans);
    Line::from(all)
}

/// Word-level intra-line diff (`renderIntraLineDiff`, `diff.ts:33-71`): common runs render in the
/// base line style, changed runs reversed. Leading whitespace of the first changed run is excluded
/// from the highlight. Returns `(removed_spans, added_spans)`.
fn intra_line(
    old: &str,
    new: &str,
    rem_style: Style,
    add_style: Style,
) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
    let parts = word_diff(old, new);
    let inverse = Style::default().add_modifier(ratatui::style::Modifier::REVERSED);
    let mut removed: Vec<Span<'static>> = Vec::new();
    let mut added: Vec<Span<'static>> = Vec::new();
    let mut first_removed = true;
    let mut first_added = true;
    for part in parts {
        match part {
            WordPart::Removed(mut v) => {
                if first_removed {
                    let lead = leading_ws(&v);
                    if !lead.is_empty() {
                        removed.push(Span::styled(lead.clone(), rem_style));
                        v = v.get(lead.len()..).unwrap_or("").to_string();
                    }
                    first_removed = false;
                }
                if !v.is_empty() {
                    removed.push(Span::styled(v, rem_style.patch(inverse)));
                }
            }
            WordPart::Added(mut v) => {
                if first_added {
                    let lead = leading_ws(&v);
                    if !lead.is_empty() {
                        added.push(Span::styled(lead.clone(), add_style));
                        v = v.get(lead.len()..).unwrap_or("").to_string();
                    }
                    first_added = false;
                }
                if !v.is_empty() {
                    added.push(Span::styled(v, add_style.patch(inverse)));
                }
            }
            WordPart::Common(v) => {
                removed.push(Span::styled(v.clone(), rem_style));
                added.push(Span::styled(v, add_style));
            }
        }
    }
    (removed, added)
}

fn leading_ws(s: &str) -> String {
    s.chars().take_while(|c| c.is_whitespace()).collect()
}

/// One segment of a word-level diff.
enum WordPart {
    Common(String),
    Removed(String),
    Added(String),
}

/// Tokenize into words with trailing whitespace attached (mirrors `Diff.diffWords` grouping of
/// whitespace with adjacent words, `diff.ts:34`), so `"foo bar"` → `["foo ", "bar"]`.
fn tokenize(s: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            cur.push(ch);
            in_word = false;
        } else {
            if !in_word && !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
            cur.push(ch);
            in_word = true;
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Word-level diff via token LCS, coalescing adjacent same-kind runs. Index-free (clippy NO-PANIC):
/// the DP table is a flat `Vec` accessed through `at`, and tokens through `slice::get`.
fn word_diff(old: &str, new: &str) -> Vec<WordPart> {
    let a = tokenize(old);
    let b = tokenize(new);
    let n = a.len();
    let m = b.len();
    let stride = m + 1;
    // LCS DP over tokens, stored row-major in a flat vec; `at(i,j)` reads it without indexing.
    let mut dp = vec![0usize; (n + 1) * stride];
    let at = |dp: &[usize], i: usize, j: usize| -> usize {
        dp.get(i.saturating_mul(stride).saturating_add(j)).copied().unwrap_or(0)
    };
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            let v = if a.get(i) == b.get(j) {
                at(&dp, i + 1, j + 1) + 1
            } else {
                at(&dp, i + 1, j).max(at(&dp, i, j + 1))
            };
            if let Some(slot) = dp.get_mut(i.saturating_mul(stride).saturating_add(j)) {
                *slot = v;
            }
        }
    }
    let mut parts: Vec<WordPart> = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    let push = |parts: &mut Vec<WordPart>, kind: u8, s: &str| match (parts.last_mut(), kind) {
        (Some(WordPart::Common(prev)), 0) => prev.push_str(s),
        (Some(WordPart::Removed(prev)), 1) => prev.push_str(s),
        (Some(WordPart::Added(prev)), 2) => prev.push_str(s),
        (_, 0) => parts.push(WordPart::Common(s.to_string())),
        (_, 1) => parts.push(WordPart::Removed(s.to_string())),
        (_, _) => parts.push(WordPart::Added(s.to_string())),
    };
    while let (Some(ta), Some(tb)) = (a.get(i), b.get(j)) {
        if ta == tb {
            push(&mut parts, 0, ta);
            i += 1;
            j += 1;
        } else if at(&dp, i + 1, j) >= at(&dp, i, j + 1) {
            push(&mut parts, 1, ta);
            i += 1;
        } else {
            push(&mut parts, 2, tb);
            j += 1;
        }
    }
    while let Some(ta) = a.get(i) {
        push(&mut parts, 1, ta);
        i += 1;
    }
    while let Some(tb) = b.get(j) {
        push(&mut parts, 2, tb);
        j += 1;
    }
    parts
}
