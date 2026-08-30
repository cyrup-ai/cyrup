//! Line-ending/BOM handling + diff/patch generation for `edit` (R-03-018/019, arch-03 §6.4).

use similar::TextDiff;
use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

const BOM: &str = "\u{feff}";

/// Original line-ending style, restored on write (R-03-018). Pi only ever distinguishes CRLF from
/// LF (edit-diff.ts:9-16): there is **no** lone-CR variant — `normalizeToLF` folds a bare `\r` to
/// `\n` and `restoreLineEndings` only re-expands LF→CRLF, so a CR-only file round-trips as LF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::Crlf => "\r\n",
        }
    }
}

/// Strip a leading UTF-8 BOM, returning `(had_bom, body)`.
pub fn strip_bom(s: &str) -> (bool, &str) {
    match s.strip_prefix(BOM) {
        Some(rest) => (true, rest),
        None => (false, s),
    }
}

/// Detect the line ending — **first ending wins** (edit-diff.ts:9-16). If the very first newline in
/// the file is part of a `\r\n` pair the file is CRLF; otherwise (a lone `\n` appears first, or
/// there are no `\n` at all, or there is no `\r\n` anywhere) it is LF. A lone CR earlier than the
/// first CRLF does NOT make the file CRLF, and a CR-only file reports LF (folded by `normalizeToLF`).
pub fn detect_line_ending(s: &str) -> LineEnding {
    let lf_idx = s.find('\n');
    let crlf_idx = s.find("\r\n");
    match (lf_idx, crlf_idx) {
        (None, _) => LineEnding::Lf,
        (Some(_), None) => LineEnding::Lf,
        (Some(lf), Some(crlf)) => {
            if crlf < lf {
                LineEnding::Crlf
            } else {
                LineEnding::Lf
            }
        }
    }
}

/// Normalize any line ending to LF for matching.
pub fn normalize_to_lf(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n")
}

/// Restore LF text to the original ending.
pub fn restore_line_endings(s: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => s.to_string(),
        other => s.replace('\n', other.as_str()),
    }
}

/// A unified patch for `path` between LF-normalized `old` and `new`.
///
/// Pi uses `createTwoFilesPatch(path, path, …, { context: 4, FILE_HEADERS_ONLY })`
/// (edit-diff.ts:369-374): a context radius of **4** and bare `--- path` / `+++ path` headers
/// (no `a/` `b/` prefixes).
pub fn unified_patch(path: &str, old: &str, new: &str) -> String {
    TextDiff::from_lines(old, new)
        .unified_diff()
        .context_radius(4)
        .header(path, path)
        .to_string()
}

/// Number of display-diff context lines around a change (edit-diff.ts:383).
const DIFF_CONTEXT_LINES: usize = 4;

/// Line-numbered, context-limited display diff between LF-normalized `old` and `new`
/// (edit-diff.ts:380-503). Returns the diff string and the 1-indexed first changed line in the
/// **new** file (for editor navigation). Lines are prefixed `+NN`/`-NN`/` NN`; runs of unchanged
/// lines longer than the context window collapse to a `…`-style ` ...` skip marker.
pub fn generate_diff_string(old: &str, new: &str) -> (String, Option<usize>) {
    struct Part {
        added: bool,
        removed: bool,
        value: String,
    }
    let diff = TextDiff::from_lines(old, new);
    // Coalesce per-line changes into added/removed/equal blocks (like jsdiff `diffLines`).
    let mut parts: Vec<Part> = Vec::new();
    for change in diff.iter_all_changes() {
        let (added, removed) = match change.tag() {
            similar::ChangeTag::Insert => (true, false),
            similar::ChangeTag::Delete => (false, true),
            similar::ChangeTag::Equal => (false, false),
        };
        match parts.last_mut() {
            Some(last) if last.added == added && last.removed == removed => {
                last.value.push_str(change.value());
            }
            _ => parts.push(Part {
                added,
                removed,
                value: change.value().to_string(),
            }),
        }
    }

    let old_count = old.split('\n').count();
    let new_count = new.split('\n').count();
    let line_num_width = old_count.max(new_count).to_string().len();
    let pad = |n: usize| -> String {
        let s = n.to_string();
        if s.len() >= line_num_width {
            s
        } else {
            format!("{}{}", " ".repeat(line_num_width - s.len()), s)
        }
    };
    let blank = " ".repeat(line_num_width);

    let mut output: Vec<String> = Vec::new();
    let mut old_line = 1usize;
    let mut new_line = 1usize;
    let mut last_was_change = false;
    let mut first_changed_line: Option<usize> = None;
    let len = parts.len();

    for (i, part) in parts.iter().enumerate() {
        let mut raw: Vec<&str> = part.value.split('\n').collect();
        if raw.last() == Some(&"") {
            raw.pop();
        }

        if part.added || part.removed {
            if first_changed_line.is_none() {
                first_changed_line = Some(new_line);
            }
            for line in &raw {
                if part.added {
                    output.push(format!("+{} {line}", pad(new_line)));
                    new_line += 1;
                } else {
                    output.push(format!("-{} {line}", pad(old_line)));
                    old_line += 1;
                }
            }
            last_was_change = true;
        } else {
            let next_is_change =
                parts.get(i + 1).is_some_and(|p| p.added || p.removed) && i < len.saturating_sub(1);
            let has_leading = last_was_change;
            let has_trailing = next_is_change;

            if has_leading && has_trailing {
                if raw.len() <= DIFF_CONTEXT_LINES * 2 {
                    for line in &raw {
                        output.push(format!(" {} {line}", pad(old_line)));
                        old_line += 1;
                        new_line += 1;
                    }
                } else {
                    let leading = raw.get(..DIFF_CONTEXT_LINES).unwrap_or(&[]);
                    let trailing = raw.get(raw.len() - DIFF_CONTEXT_LINES..).unwrap_or(&[]);
                    let skipped = raw.len() - leading.len() - trailing.len();
                    for line in leading {
                        output.push(format!(" {} {line}", pad(old_line)));
                        old_line += 1;
                        new_line += 1;
                    }
                    output.push(format!(" {blank} ..."));
                    old_line += skipped;
                    new_line += skipped;
                    for line in trailing {
                        output.push(format!(" {} {line}", pad(old_line)));
                        old_line += 1;
                        new_line += 1;
                    }
                }
            } else if has_leading {
                let shown = raw.get(..DIFF_CONTEXT_LINES.min(raw.len())).unwrap_or(&[]);
                let skipped = raw.len() - shown.len();
                for line in shown {
                    output.push(format!(" {} {line}", pad(old_line)));
                    old_line += 1;
                    new_line += 1;
                }
                if skipped > 0 {
                    output.push(format!(" {blank} ..."));
                    old_line += skipped;
                    new_line += skipped;
                }
            } else if has_trailing {
                let skipped = raw.len().saturating_sub(DIFF_CONTEXT_LINES);
                if skipped > 0 {
                    output.push(format!(" {blank} ..."));
                    old_line += skipped;
                    new_line += skipped;
                }
                for line in raw.get(skipped..).unwrap_or(&[]) {
                    output.push(format!(" {} {line}", pad(old_line)));
                    old_line += 1;
                    new_line += 1;
                }
            } else {
                old_line += raw.len();
                new_line += raw.len();
            }
            last_was_change = false;
        }
    }

    (output.join("\n"), first_changed_line)
}

// ---------------------------------------------------------------------------------------------
// Fuzzy multi-edit engine (edit-diff.ts:33-366).
//
// Pi tries an exact `indexOf` match first; on a miss it normalizes both the haystack and the
// `oldText` via `normalizeForFuzzyMatch` and retries in normalized space. When any edit matched
// fuzzily, replacements are computed in the normalized buffer and overlaid back onto the original
// content line-block by line-block so untouched lines keep their original bytes.
//
// Like Pi (edit-diff.ts:36), `normalize_for_fuzzy` runs `NFKC` first via the workspace-declared
// `unicode-normalization` crate, then applies the explicit transforms (trailing-whitespace strip,
// smart quotes, Unicode dashes, Unicode spaces). This makes compatibility-form text — ligatures
// (`ﬁ` → `fi`), full-width Latin, etc. — fuzzy-match its ASCII equivalents exactly as Pi does.
// ---------------------------------------------------------------------------------------------

/// An edit error, carrying Pi's exact model-facing message text (edit-diff.ts:257-293).
#[derive(Debug, Clone)]
pub struct EditError(pub String);

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One source replacement to apply (offsets are byte offsets into the matched buffer).
#[derive(Clone)]
struct Replacement {
    edit_index: usize,
    match_index: usize,
    match_length: usize,
    new_text: String,
}

/// Result of an `apply_edits_to_normalized_content` run.
#[derive(Debug)]
pub struct AppliedEdits {
    pub base_content: String,
    pub new_content: String,
    /// Edit indices that were applied, ascending.
    pub applied: Vec<usize>,
    /// `(index, message)` for each edit that was NOT applied, ascending by index.
    ///
    /// **A non-empty `failed` means the write is partial** — every caller must inspect it. The
    /// messages are the `err_*` strings verbatim, so a failing edit reads exactly as it does
    /// today, near-miss region included.
    pub failed: Vec<(usize, String)>,
}

/// `normalizeForFuzzyMatch` (edit-diff.ts:33-54): NFKC first (edit-diff.ts:36), then strip trailing
/// whitespace per line, then fold smart quotes / Unicode dashes / Unicode spaces.
fn normalize_for_fuzzy(text: &str) -> String {
    // Pi leads with `text.normalize("NFKC")` (edit-diff.ts:36). NFKC never inserts/removes `\n`, so
    // the per-line split below and the overlay's line-count invariant are preserved.
    let nfkc: String = text.nfkc().collect();
    let stripped: Vec<&str> = nfkc.split('\n').map(str::trim_end).collect();
    stripped
        .join("\n")
        .chars()
        .map(|c| match c {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

struct FuzzyMatch {
    found: bool,
    index: usize,
    match_length: usize,
    used_fuzzy: bool,
    /// Indentation the searched buffer carries that `old_text` did not. `None` for every tier
    /// except the line-anchored one, and [`reindent`] leaves the replacement untouched for
    /// `None`, so tiers 1 and 2 stay bit-for-bit unaffected.
    ///
    /// It must NOT be an empty `String` for tiers 1 and 2: `Some("")` is a real line-anchored
    /// outcome — a needle authored MORE indented than the file dedents to a zero delta — and
    /// that case still has to re-base the replacement, which an empty-string sentinel would
    /// silently skip.
    indent: Option<String>,
}

/// One line-anchored match: a run of whole lines whose bodies equal the needle's bodies after
/// the fuzzy fold, `trim_end`, and removal of a **uniform** indentation prefix.
struct LineAnchored {
    /// Byte offset of the first matched line, in the buffer that was searched.
    index: usize,
    /// Byte length of the matched region in that same buffer.
    match_length: usize,
    /// The indentation the buffer carries that the dedented needle does not.
    indent: String,
}

fn leading_ws_len(s: &str) -> usize {
    s.len() - s.trim_start().len()
}

/// Every line-anchored match of `old_text` in `content`.
///
/// [CYRUP-DELTA] Pi has no equivalent: `fuzzyFindText` (edit-diff.ts:206-244) stops after the
/// normalized-buffer pass, so an `oldText` at the wrong indent depth is *not found* upstream and
/// is found here. This is the tier every production applier ships — codex `apply_patch`'s third
/// pass (seek_sequence.rs:58), aider's `match_but_for_leading_whitespace`
/// (editblock_coder.py:276), git's `--ignore-whitespace` (apply.c:2544) — and it carries their
/// safety rule: the indentation delta must be a SINGLE value across every non-blank line, so a
/// block whose tokens coincide at ragged indentation is rejected rather than mangled.
///
/// Offsets are reported in the buffer that was searched, so `apply_replacements` splices the
/// ORIGINAL lines and the fuzzy overlay — with its line-count invariant — is never involved.
fn line_anchored_matches(content: &str, old_text: &str) -> Vec<LineAnchored> {
    let (needle_body, needle_had_eol) = match old_text.strip_suffix('\n') {
        Some(rest) => (rest, true),
        None => (old_text, false),
    };
    // Per-line folding is equivalent to folding the buffer: NFKC never inserts or removes `\n`
    // (see `normalize_for_fuzzy`) and `trim_end` is already line-local.
    let folded: Vec<String> = needle_body.split('\n').map(normalize_for_fuzzy).collect();
    // An all-whitespace needle stays on pi's path: `count_occurrences` drives it into the
    // duplicate error (edit-diff.ts:333), and matching it here would change that remediation.
    if folded.iter().all(|l| l.trim().is_empty()) {
        return Vec::new();
    }
    // Uniformly outdent the needle by its own minimum indent, so a needle carrying *some* of the
    // file's indentation normalizes the same as one carrying none
    // (aider `replace_part_with_missing_leading_whitespace`, editblock_coder.py:248-255).
    let common = folded
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading_ws_len(l))
        .min()
        .unwrap_or(0);
    let needle: Vec<&str> = folded
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                l.as_str()
            } else {
                l.get(common..).unwrap_or(l.as_str())
            }
        })
        .collect();

    let spans = get_line_spans(content);
    let width = needle.len();
    let mut out: Vec<LineAnchored> = Vec::new();
    if width == 0 || spans.len() < width {
        return out;
    }
    for i in 0..=spans.len().saturating_sub(width) {
        let mut indents: Vec<String> = Vec::new();
        let mut ok = true;
        for (k, n) in needle.iter().enumerate() {
            let Some((s, e)) = spans.get(i + k) else {
                ok = false;
                break;
            };
            let raw = content.get(*s..*e).unwrap_or("");
            let body = normalize_for_fuzzy(raw.strip_suffix('\n').unwrap_or(raw));
            if body.trim_start() != n.trim_start() {
                ok = false;
                break;
            }
            if !n.trim().is_empty() {
                // The prefix the file carries that the needle does not. Requires the file line to
                // be at least as long, i.e. only the outdented-needle direction is allowed.
                let Some(prefix) = body.len().checked_sub(n.len()).and_then(|w| body.get(..w))
                else {
                    ok = false;
                    break;
                };
                if !prefix.chars().all(char::is_whitespace) {
                    ok = false;
                    break;
                }
                indents.push(prefix.to_string());
            }
        }
        if !ok {
            continue;
        }
        // THE false-apply bound: one indentation delta, or no match at all.
        let Some(first) = indents.first() else {
            continue;
        };
        if indents.iter().any(|d| d != first) {
            continue;
        }
        let (Some((start, _)), Some((_, last_end))) = (spans.get(i), spans.get(i + width - 1))
        else {
            continue;
        };
        let last_body_end = content
            .get(..*last_end)
            .and_then(|s| s.strip_suffix('\n'))
            .map_or(*last_end, str::len);
        let end = if needle_had_eol {
            *last_end
        } else {
            last_body_end
        };
        out.push(LineAnchored {
            index: *start,
            match_length: end.saturating_sub(*start),
            indent: first.clone(),
        });
    }
    out
}

/// Re-indent `new_text` to the depth the matched region carries.
///
/// Mirrors aider's `replace_part_with_missing_leading_whitespace` (editblock_coder.py:269): the
/// replacement is outdented by its own uniform margin and then re-indented by the prefix
/// captured from the file, so a replacement authored at the wrong depth still lands at the
/// file's depth. `indent` is `None` for every match that did not come from the line-anchored
/// tier, and `None` returns `new_text` untouched — tiers 1 and 2 are bit-for-bit unaffected.
///
/// `Some("")` is NOT the same as `None`. A needle authored *more* indented than the file
/// dedents to a zero delta, and the replacement — authored at that same too-deep margin — still
/// has to be re-based, or the edit writes correct text at an indentation the file never had.
fn reindent(new_text: &str, indent: Option<&str>) -> String {
    let Some(indent) = indent else {
        return new_text.to_string();
    };
    let lines: Vec<&str> = new_text.split('\n').collect();
    let common = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| leading_ws_len(l))
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|l| {
            if l.trim().is_empty() {
                (*l).to_string()
            } else {
                format!("{indent}{}", l.get(common..).unwrap_or(l))
            }
        })
        .collect::<Vec<String>>()
        .join("\n")
}

/// `fuzzyFindText` (edit-diff.ts:206-244): exact match first, then normalized-space match. The
/// caller decides which buffer to replace against (Pi's `contentForReplacement`).
fn fuzzy_find_text(content: &str, old_text: &str) -> FuzzyMatch {
    if let Some(idx) = content.find(old_text) {
        return FuzzyMatch {
            found: true,
            index: idx,
            match_length: old_text.len(),
            used_fuzzy: false,
            indent: None,
        };
    }
    let fuzzy_content = normalize_for_fuzzy(content);
    let fuzzy_old = normalize_for_fuzzy(old_text);
    // `const fuzzyIndex = fuzzyContent.indexOf(fuzzyOldText);` then `if (fuzzyIndex === -1)`
    // (edit-diff.ts:222/:225). There is NO empty-needle guard upstream, and JS `indexOf("")`
    // returns 0 — i.e. FOUND, at index 0, with `matchLength` 0. Rust's `str::find("")` returns
    // `Some(0)` identically, so the guard cyrup carried here was a pure divergence: it sent an
    // `oldText` that is non-empty but NORMALIZES to empty (an all-whitespace needle) down the
    // not-found arm, where pi falls through to `countOccurrences` and raises the DUPLICATE error
    // instead — different remediation advice for the same input.
    match fuzzy_content.find(&fuzzy_old) {
        Some(idx) => FuzzyMatch {
            found: true,
            index: idx,
            match_length: fuzzy_old.len(),
            used_fuzzy: true,
            indent: None,
        },
        None => {
            // [CYRUP-DELTA] Tier 3, tried only once both of pi's passes have failed: a
            // line-anchored match that ignores a UNIFORM indentation difference. `used_fuzzy`
            // stays false because the offsets are in the buffer that was searched, so the
            // ordinary `apply_replacements` path handles it.
            if let Some(m) = line_anchored_matches(content, old_text).into_iter().next() {
                return FuzzyMatch {
                    found: true,
                    index: m.index,
                    match_length: m.match_length,
                    used_fuzzy: false,
                    indent: Some(m.indent),
                };
            }
            FuzzyMatch {
                found: false,
                index: 0,
                match_length: 0,
                used_fuzzy: false,
                indent: None,
            }
        }
    }
}

/// `countOccurrences` over the fuzzy-normalized buffer (edit-diff.ts:252-256):
/// `return fuzzyContent.split(fuzzyOldText).length - 1;`
///
/// JS `split` has two distinct behaviours and the empty-separator one is load-bearing here:
/// * a NON-empty separator splits on non-overlapping occurrences, so `length - 1` is exactly the
///   occurrence count — `str::matches().count()`;
/// * an EMPTY separator splits into one element per **UTF-16 code unit**, so `length - 1` is the
///   code-unit count minus one, which for any content longer than two units is `> 1` and drives
///   the caller (edit-diff.ts:333) into the duplicate-occurrences error. Returning 0 here sent
///   that input to the not-found arm instead.
///
/// The one place the ports differ is unobservable: for EMPTY content and an empty separator JS
/// yields `[]`, so pi returns `-1` where the `saturating_sub` below returns `0`. The only consumer
/// is `occurrences > 1` (edit-diff.ts:333), false for both, and the duplicate message that
/// interpolates the count is unreachable unless that test passes.
fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy(content);
    let fuzzy_old = normalize_for_fuzzy(old_text);
    if fuzzy_old.is_empty() {
        return fuzzy_content.encode_utf16().count().saturating_sub(1);
    }
    fuzzy_content.matches(&fuzzy_old).count()
}

/// `splitLinesWithEndings` (edit-diff.ts:56-58): each line keeps its trailing `\n`.
fn split_lines_with_endings(content: &str) -> Vec<&str> {
    let mut out: Vec<&str> = Vec::new();
    let mut start = 0usize;
    for (i, b) in content.bytes().enumerate() {
        if b == b'\n'
            && let Some(s) = content.get(start..=i)
        {
            out.push(s);
            start = i + 1;
        }
    }
    if start < content.len()
        && let Some(s) = content.get(start..)
    {
        out.push(s);
    }
    out
}

/// `getLineSpans` (edit-diff.ts:74-81): byte `(start, end)` per line.
fn get_line_spans(content: &str) -> Vec<(usize, usize)> {
    let mut offset = 0usize;
    split_lines_with_endings(content)
        .iter()
        .map(|line| {
            let span = (offset, offset + line.len());
            offset = span.1;
            span
        })
        .collect()
}

/// `getReplacementLineRange` (edit-diff.ts:83-108): `(start_line, end_line_exclusive)`.
fn get_replacement_line_range(
    lines: &[(usize, usize)],
    rep_start: usize,
    rep_end: usize,
) -> Result<(usize, usize), EditError> {
    let mut start_line: Option<usize> = None;
    for (i, (s, e)) in lines.iter().enumerate() {
        if rep_start >= *s && rep_start < *e {
            start_line = Some(i);
            break;
        }
    }
    let start_line = start_line
        .ok_or_else(|| EditError("Replacement range is outside the base content.".to_string()))?;
    let mut end_line = start_line;
    while end_line < lines.len() && lines.get(end_line).is_some_and(|(_, e)| *e < rep_end) {
        end_line += 1;
    }
    if end_line >= lines.len() {
        return Err(EditError(
            "Replacement range is outside the base content.".to_string(),
        ));
    }
    Ok((start_line, end_line + 1))
}

/// `applyReplacements` (edit-diff.ts:110-119): apply in reverse so byte offsets stay stable.
fn apply_replacements(content: &str, reps: &[Replacement], offset: usize) -> String {
    let mut result = content.to_string();
    for rep in reps.iter().rev() {
        let mi = rep.match_index.saturating_sub(offset);
        let before = result.get(..mi).unwrap_or("").to_string();
        let after = result
            .get(mi + rep.match_length..)
            .unwrap_or("")
            .to_string();
        result = format!("{before}{}{after}", rep.new_text);
    }
    result
}

/// `applyReplacementsPreservingUnchangedLines` (edit-diff.ts:131-172).
fn apply_replacements_preserving_unchanged_lines(
    original: &str,
    base: &str,
    reps: &[Replacement],
) -> Result<String, EditError> {
    let original_lines = split_lines_with_endings(original);
    let base_lines = get_line_spans(base);
    if original_lines.len() != base_lines.len() {
        return Err(EditError(
            "Cannot preserve unchanged lines because the base content has a different line count."
                .to_string(),
        ));
    }

    struct Group {
        start_line: usize,
        end_line: usize,
        reps: Vec<Replacement>,
    }
    let mut sorted = reps.to_vec();
    sorted.sort_by_key(|r| r.match_index);
    let mut groups: Vec<Group> = Vec::new();
    for rep in sorted {
        let (rs, re) = get_replacement_line_range(
            &base_lines,
            rep.match_index,
            rep.match_index + rep.match_length,
        )?;
        match groups.last_mut() {
            Some(cur) if rs < cur.end_line => {
                cur.end_line = cur.end_line.max(re);
                cur.reps.push(rep);
            }
            _ => groups.push(Group {
                start_line: rs,
                end_line: re,
                reps: vec![rep],
            }),
        }
    }

    let mut original_line_index = 0usize;
    let mut result = String::new();
    for group in &groups {
        for line in original_lines
            .get(original_line_index..group.start_line)
            .unwrap_or(&[])
        {
            result.push_str(line);
        }
        let group_start = base_lines.get(group.start_line).map_or(0, |(s, _)| *s);
        let group_end = base_lines
            .get(group.end_line.saturating_sub(1))
            .map_or(0, |(_, e)| *e);
        let slice = base.get(group_start..group_end).unwrap_or("");
        result.push_str(&apply_replacements(slice, &group.reps, group_start));
        original_line_index = group.end_line;
    }
    for line in original_lines.get(original_line_index..).unwrap_or(&[]) {
        result.push_str(line);
    }
    Ok(result)
}

/// The failure messages of a batch, in edit order, as one block.
///
/// Identical messages collapse to one. Only the overlap message is shared by construction — it
/// names both indices and is pushed once per member — so this is exactly the "print the overlap
/// sentence once" rule and nothing else: every other `err_*` message embeds its own `edits[i]`,
/// so two distinct failures can never collide.
///
/// A single failure joins to itself, byte for byte. Two assertions pin that:
/// `a_far_miss_keeps_pis_bare_sentence` below and
/// `literally_empty_old_text_still_takes_the_dedicated_error` in `pi_tool_semantics.rs`. Do not add a header,
/// a bullet or a trailing newline here.
pub(crate) fn join_failures(failed: &[(usize, String)]) -> String {
    let mut seen: Vec<&str> = Vec::new();
    for (_, m) in failed {
        if !seen.contains(&m.as_str()) {
            seen.push(m);
        }
    }
    seen.join("\n\n")
}

fn err_empty(path: &str, i: usize, total: usize) -> EditError {
    EditError(if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{i}].oldText must not be empty in {path}.")
    })
}

/// Similarity floor for showing a near-miss. Below this the "closest" region is noise and naming
/// it would mislead more than the bare failure does. Aider uses the same 0.6 over
/// `difflib.SequenceMatcher` (editblock_coder.py:602).
const NEAR_MISS_THRESHOLD: f32 = 0.6;
/// Extra original lines shown either side of the near-miss window (editblock_coder.py:623).
const NEAR_MISS_CONTEXT: usize = 5;

/// The window of `content` most similar to `old_text`, as a 1-indexed start line and the lines
/// themselves, or `None` when nothing clears [`NEAR_MISS_THRESHOLD`].
///
/// Scores every same-length line window with `similar::TextDiff::ratio()`. This drives **only**
/// the not-found message: no caller may use it to select a replacement target.
fn nearest_region(content: &str, old_text: &str) -> Option<(usize, String)> {
    let hay: Vec<&str> = content.split('\n').collect();
    let needle: Vec<&str> = old_text.split('\n').collect();
    let width = needle.len();
    if width == 0 || hay.len() < width {
        return None;
    }
    // `ratio()` is `2 * matched / (old_len + new_len)`; both sides are `width` lines here, so it
    // reduces to `matched / width`. `matched` is Myers' matching subsequence, which is a
    // sub-multiset of the window's multiset overlap with the needle — so that overlap, divided by
    // `width`, is an upper bound on the score, and it slides in O(1) per window.
    //
    // A window whose bound clears neither the floor nor the running best can never be the one
    // reported, so it never pays for a diff. The window chosen and the score it carries are
    // identical to scoring every window; on a large file almost none of them are scored at all.
    let mut want: HashMap<&str, i32> = HashMap::new();
    for line in &needle {
        *want.entry(line).or_insert(0) += 1;
    }
    let mut have: HashMap<&str, i32> = HashMap::new();
    let mut overlap: i32 = 0;
    let mut best: Option<(f32, usize)> = None;
    for i in 0..=hay.len().saturating_sub(width) {
        if i == 0 {
            for &line in hay.get(..width).unwrap_or(&[]) {
                let slot = have.entry(line).or_insert(0);
                *slot += 1;
                if *slot <= want.get(line).copied().unwrap_or(0) {
                    overlap += 1;
                }
            }
        } else {
            if let Some(&out) = hay.get(i - 1) {
                let slot = have.entry(out).or_insert(0);
                *slot -= 1;
                if *slot < want.get(out).copied().unwrap_or(0) {
                    overlap -= 1;
                }
            }
            if let Some(&inc) = hay.get(i + width - 1) {
                let slot = have.entry(inc).or_insert(0);
                *slot += 1;
                if *slot <= want.get(inc).copied().unwrap_or(0) {
                    overlap += 1;
                }
            }
        }
        let bound = f64::from(overlap) / width as f64;
        if bound < f64::from(NEAR_MISS_THRESHOLD)
            || best.is_some_and(|(b, _)| bound <= f64::from(b))
        {
            continue;
        }
        let Some(window) = hay.get(i..i + width) else {
            continue;
        };
        // `from_slices` over the line slices rather than `from_lines` over a joined String: the
        // ratio is computed from element counts either way, so the score is unchanged, and no
        // String is allocated to score a window.
        let ratio = TextDiff::from_slices(window, &needle).ratio();
        if best.is_none_or(|(b, _)| ratio > b) {
            best = Some((ratio, i));
        }
    }
    let (ratio, start) = best?;
    if ratio < NEAR_MISS_THRESHOLD {
        return None;
    }
    let from = start.saturating_sub(NEAR_MISS_CONTEXT);
    let to = (start + width + NEAR_MISS_CONTEXT).min(hay.len());
    let shown = hay.get(from..to)?.join("\n");
    Some((from + 1, shown))
}

fn err_not_found(path: &str, i: usize, total: usize, content: &str, old: &str) -> EditError {
    let head = if total == 1 {
        format!(
            "Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines."
        )
    } else {
        format!(
            "Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    };
    // [CYRUP-DELTA] Pi stops at the sentence above (edit-diff.ts:258-267). Cyrup appends the
    // closest region so the caller repairs the needle in one round instead of re-reading and
    // guessing. Purely additive: no match/apply decision consults `nearest_region`.
    EditError(match nearest_region(content, old) {
        Some((line, shown)) => {
            format!("{head}\nClosest region in {path} starts at line {line}:\n{shown}")
        }
        None => head,
    })
}

fn err_duplicate(path: &str, i: usize, total: usize, occ: usize) -> EditError {
    EditError(if total == 1 {
        format!(
            "Found {occ} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occ} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    })
}

fn err_no_change(path: &str, total: usize) -> EditError {
    EditError(if total == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    })
}

/// `applyEditsToNormalizedContent` (edit-diff.ts:304-366): the multi-edit core. `edits` are raw
/// `(oldText, newText)` pairs; they are LF-normalized internally. All edits match against the same
/// original; when any needs the fuzzy fallback the whole batch runs in fuzzy space and is overlaid
/// back to preserve untouched lines.
pub fn apply_edits_to_normalized_content(
    normalized_content: &str,
    edits: &[(String, String)],
    path: &str,
) -> Result<AppliedEdits, EditError> {
    let total = edits.len();
    let normalized_edits: Vec<(String, String)> = edits
        .iter()
        .map(|(o, n)| (normalize_to_lf(o), normalize_to_lf(n)))
        .collect();

    // [CYRUP-DELTA] `used_fuzzy` is decided over ALL edits, before any failure is known, so an
    // edit that matches fuzzily and is then dropped for duplicate-count or overlap still puts the
    // batch in fuzzy space. Deliberate: recomputing it over the survivors needs a re-match whose
    // termination is not obvious, and the only difference is trailing whitespace on the edited
    // lines that landed. A not-found edit cannot cause it — that arm leaves `used_fuzzy` false,
    // as do tier 1 and the line-anchored tier; only the tier-2 hit sets it.
    //
    // This now also sees empty needles, because the pre-loop that rejected them is gone. That is
    // deliberate and inert: `str::find("")` is `Some(0)`, so an empty needle takes tier 1 and
    // contributes `used_fuzzy: false` — the batch's fuzzy decision is unchanged.
    let used_fuzzy = normalized_edits
        .iter()
        .any(|(old, _)| fuzzy_find_text(normalized_content, old).used_fuzzy);
    let replacement_base = if used_fuzzy {
        normalize_for_fuzzy(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched: Vec<Replacement> = Vec::new();
    let mut failed: Vec<(usize, String)> = Vec::new();
    for (i, (old, new)) in normalized_edits.iter().enumerate() {
        // [CYRUP-DELTA] Pi pre-checks every `oldText` and throws before matching anything
        // (edit-diff.ts:310-314), discarding the whole call. Here an empty needle is one
        // malformed edit among possibly-good ones, so it fails only itself.
        if old.is_empty() {
            failed.push((i, err_empty(path, i, total).0));
            continue;
        }
        let mr = fuzzy_find_text(&replacement_base, old);
        if !mr.found {
            failed.push((i, err_not_found(path, i, total, &replacement_base, old).0));
            continue;
        }
        // `count_occurrences` only sees normalized SUBSTRING occurrences, so it reports 0 for a
        // match that came from the line-anchored tier. Taking the count from the tier that
        // actually matched keeps the uniqueness rule — the false-apply bound — in force there.
        let occ = match count_occurrences(&replacement_base, old) {
            0 => line_anchored_matches(&replacement_base, old).len(),
            n => n,
        };
        if occ > 1 {
            failed.push((i, err_duplicate(path, i, total, occ).0));
            continue;
        }
        matched.push(Replacement {
            edit_index: i,
            match_index: mr.index,
            match_length: mr.match_length,
            new_text: reindent(new, mr.indent.as_deref()),
        });
    }

    // [CYRUP-DELTA] An overlapping pair drops BOTH members. Each matched uniquely, so applying
    // either would invent a winner the model never asked for and cannot predict. An edit that
    // overlaps any other is dropped, so a chain of three drops entirely. Pi aborts the call.
    //
    // Comparing only ADJACENT pairs is sufficient. Sorted by `match_index`, suppose survivors
    // `P < Q` overlap, i.e. `P.end > Q.start`. Let `R` be the element immediately after `P`;
    // since `P.start <= R.start <= Q.start`, we get `P.end > Q.start >= R.start`, so the adjacent
    // pair `(P, R)` overlaps and `P` was collided — contradicting `P` surviving. After `retain`,
    // survivors are therefore pairwise disjoint, which is what makes the reverse-offset splice in
    // `apply_replacements` sound over a subset of the batch.
    matched.sort_by_key(|m| m.match_index);
    let mut collided: Vec<usize> = Vec::new();
    for pair in matched.windows(2) {
        if let [prev, cur] = pair
            && prev.match_index + prev.match_length > cur.match_index
        {
            let msg = format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                prev.edit_index, cur.edit_index
            );
            collided.push(prev.edit_index);
            collided.push(cur.edit_index);
            failed.push((prev.edit_index, msg.clone()));
            failed.push((cur.edit_index, msg));
        }
    }
    matched.retain(|m| !collided.contains(&m.edit_index));
    failed.sort_by_key(|(i, _)| *i);
    // An edit that collides with BOTH neighbours is reported once, naming the first collision.
    // `sort_by_key` is stable, so the surviving entry is the earlier push.
    failed.dedup_by_key(|(i, _)| *i);

    if matched.is_empty() {
        // Nothing survived: write nothing and report every failure, exactly as an
        // all-or-nothing call does today.
        //
        // `failed` is empty here only when `edits` itself was empty. `EditTool::execute` rejects
        // that upstream (edit.rs:260-268) and the TUI preview never builds one, but this is a
        // `pub` entry point and before partial application it fell through to the no-change arm —
        // so keep that wording rather than handing a caller an empty message.
        return Err(if failed.is_empty() {
            err_no_change(path, total)
        } else {
            EditError(join_failures(&failed))
        });
    }

    let base_content = normalized_content.to_string();
    // The `?` is deliberate: the overlay's line-count invariant is a bug in the matcher, not a
    // per-edit failure, and it writes nothing. Do not route it into `failed`.
    let new_content = if used_fuzzy {
        apply_replacements_preserving_unchanged_lines(
            normalized_content,
            &replacement_base,
            &matched,
        )?
    } else {
        apply_replacements(&replacement_base, &matched, 0)
    };

    if base_content == new_content {
        let mut msg = err_no_change(path, total).0;
        if !failed.is_empty() {
            msg.push_str("\n\n");
            msg.push_str(&join_failures(&failed));
        }
        return Err(EditError(msg));
    }

    let mut applied: Vec<usize> = matched.iter().map(|m| m.edit_index).collect();
    applied.sort_unstable();
    Ok(AppliedEdits {
        base_content,
        new_content,
        applied,
        failed,
    })
}

/// The successful shape of [`compute_edits_diff`] — Pi `EditDiffResult` (edit-diff.ts:505-508).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditDiffPreview {
    pub diff: String,
    pub first_changed_line: Option<usize>,
    /// One line per edit that will NOT apply — the first line of its failure message, which for
    /// a not-found is pi's sentence without the near-miss region (`err_not_found` joins head to
    /// region with a single `\n`, and every other `err_*` message is one line already). The
    /// preview names the shortfall; the full region belongs in the tool result, not a diff header.
    pub unapplied: Vec<String>,
}

/// `computeEditsDiff` (edit-diff.ts:514-547) — compute the diff one or more edits WOULD produce
/// **without applying them**.
///
/// Pi's doc on that function is literally "Used for preview rendering in the TUI before the tool
/// executes": `edit`'s `renderCall` fires it as soon as the streamed arguments are complete
/// (`context.argsComplete`, edit.ts:377-386) so the diff is on screen while the call is still
/// pending — through the permission prompt, and before a single byte is written. It reads the file
/// but never writes one.
///
/// The steps are exactly [`super::edit::EditTool`]'s `execute` minus the write: resolve against
/// `cwd`, check readability, [`strip_bom`] → [`normalize_to_lf`] →
/// [`apply_edits_to_normalized_content`] → [`generate_diff_string`]. That shared core is why the
/// preview and the post-write `details.diff` agree byte-for-byte on the ordinary path, which is
/// what lets a caller suppress the duplicate (Pi `formatEditResult`, edit.ts:220-226).
///
/// `edits` are raw `(oldText, newText)` pairs, LF-normalized internally. Errors are returned as the
/// message string Pi puts in `EditDiffError.error`: the unreadable-file case keeps Pi's
/// `Could not edit file: {path}. {…}.` wording (edit-diff.ts:527-531), and everything else is the
/// `applyEditsToNormalizedContent` message verbatim (not-found / duplicate / overlap / no-change),
/// so a preview failure reads identically to the failure the real tool would report.
///
/// **Blocking.** Pi's is `async` because Node has no other kind of file read; this one uses
/// `std::fs` so a synchronous renderer can call it. Callers on a UI thread should bound the file
/// size themselves.
pub fn compute_edits_diff(
    path: &str,
    edits: &[(String, String)],
    cwd: &std::path::Path,
) -> Result<EditDiffPreview, String> {
    let absolute = crate::path::resolve_to_cwd(path, cwd);
    // Pi checks `access(absolutePath, R_OK)` first and reports the failure as
    // `Could not edit file: {path}. {…}.` (edit-diff.ts:527-531). A failing read is the same
    // condition, and the wording matches what `EditTool::execute` already emits (edit.rs:194).
    let bytes =
        std::fs::read(&absolute).map_err(|e| format!("Could not edit file: {path}. {e}."))?;
    let raw = String::from_utf8_lossy(&bytes).into_owned();
    let (_had_bom, body) = strip_bom(&raw);
    let normalized = normalize_to_lf(body);
    let applied = apply_edits_to_normalized_content(&normalized, edits, path).map_err(|e| e.0)?;
    let (diff, first_changed_line) =
        generate_diff_string(&applied.base_content, &applied.new_content);
    // A partial batch reaches here as `Ok` where it used to be `Err`, so the TUI renders the
    // survivors' diff during the permission prompt for a call that will ultimately fail. That is
    // the point: the user sees exactly what is about to be written, plus what will not land.
    let unapplied = applied
        .failed
        .iter()
        .map(|(_, m)| m.lines().next().unwrap_or_default().to_string())
        .collect();
    Ok(EditDiffPreview {
        diff,
        first_changed_line,
        unapplied,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn bom_and_crlf_roundtrip() {
        let raw = "\u{feff}a\r\nb\r\n";
        let (bom, body) = strip_bom(raw);
        assert!(bom);
        assert_eq!(detect_line_ending(body), LineEnding::Crlf);
        let norm = normalize_to_lf(body);
        assert_eq!(norm, "a\nb\n");
        let restored = restore_line_endings(&norm, LineEnding::Crlf);
        assert_eq!(restored, "a\r\nb\r\n");
    }

    #[test]
    fn detect_line_ending_first_wins_and_cr_only_folds() {
        // First newline is a lone LF (before any CRLF) ⇒ LF wins (edit-diff.ts:9-16).
        assert_eq!(detect_line_ending("a\nb\r\nc"), LineEnding::Lf);
        // First newline is part of a CRLF ⇒ CRLF.
        assert_eq!(detect_line_ending("a\r\nb\nc"), LineEnding::Crlf);
        // A lone CR earlier than the first CRLF does NOT make it CRLF.
        assert_eq!(detect_line_ending("a\rb\r\nc\nd"), LineEnding::Crlf);
        // CR-only file: no `\n` at all ⇒ LF, and `normalizeToLF` folds the bare CRs to LF.
        assert_eq!(detect_line_ending("a\rb\rc"), LineEnding::Lf);
        assert_eq!(normalize_to_lf("a\rb\rc"), "a\nb\nc");
        // No newline at all ⇒ LF.
        assert_eq!(detect_line_ending("abc"), LineEnding::Lf);
    }

    #[test]
    fn patch_and_first_line() {
        let old = "one\ntwo\nthree\n";
        let new = "one\nTWO\nthree\n";
        let patch = unified_patch("f.txt", old, new);
        assert!(patch.contains("@@"));
        assert!(patch.contains("-two"));
        assert!(patch.contains("+TWO"));
        // Pi-style bare header (no a/ b/ prefixes).
        assert!(patch.contains("--- f.txt"), "patch: {patch}");
        assert!(patch.contains("+++ f.txt"), "patch: {patch}");
        let (diff, first) = generate_diff_string(old, new);
        assert_eq!(first, Some(2));
        // Line-numbered display diff.
        assert!(diff.contains("-2 two"), "diff: {diff}");
        assert!(diff.contains("+2 TWO"), "diff: {diff}");
        assert!(diff.contains(" 1 one"), "diff: {diff}");
    }

    #[test]
    fn exact_multi_edit() {
        let content = "alpha\nbeta\ngamma\n";
        let edits = vec![
            ("alpha".to_string(), "ALPHA".to_string()),
            ("gamma".to_string(), "GAMMA".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "ALPHA\nbeta\nGAMMA\n");
    }

    #[test]
    fn fuzzy_curly_quote_and_dash_and_trailing_ws() {
        // Disk has a curly apostrophe, em-dash, and trailing whitespace; the model sends ASCII.
        let content = "let x = \u{2019}hi\u{2019};   \nval \u{2014} y\n";
        let edits = vec![
            ("let x = 'hi';".to_string(), "let x = 'bye';".to_string()),
            ("val - y".to_string(), "val - z".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        // Untouched original bytes (the trailing spaces) are preserved on lines we did not edit;
        // edited lines are rewritten from the normalized base.
        assert!(
            r.new_content.contains("let x = 'bye';"),
            "got: {:?}",
            r.new_content
        );
        assert!(
            r.new_content.contains("val - z"),
            "got: {:?}",
            r.new_content
        );
    }

    #[test]
    fn fuzzy_nfkc_ligature_and_fullwidth() {
        // Disk has the `ﬁ` ligature (U+FB01) and a full-width digit `２` (U+FF12); the model sends
        // the plain ASCII forms. NFKC (edit-diff.ts:36) folds both so the fuzzy pass matches.
        let content = "const \u{FB01}le2 = \u{FF12};\n";
        let edits = vec![(
            "const file2 = 2;".to_string(),
            "const file2 = 9;".to_string(),
        )];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert!(
            r.new_content.contains("const file2 = 9;"),
            "got: {:?}",
            r.new_content
        );
    }

    /// [CYRUP-DELTA] This batch used to abort whole: `edits[1]` failing discarded `edits[0]`
    /// even though it matched uniquely. It now applies what matched and reports the rest, so the
    /// call returns `Ok` carrying a failure. The indexed wording it was written to pin is
    /// unchanged — it just travels in `failed` instead of in an `Err`.
    #[test]
    fn not_found_error_is_indexed_for_multi() {
        let content = "one\ntwo\n";
        let edits = vec![
            ("one".to_string(), "1".to_string()),
            ("zzz".to_string(), "9".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.applied, vec![0], "applied: {:?}", r.applied);
        assert_eq!(r.new_content, "1\ntwo\n", "new_content: {}", r.new_content);
        assert_eq!(r.failed.len(), 1, "failed: {:?}", r.failed);
        assert_eq!(r.failed[0].0, 1, "failed: {:?}", r.failed);
        assert!(
            r.failed[0].1.contains("Could not find edits[1] in f.txt"),
            "got: {}",
            r.failed[0].1
        );
    }

    #[test]
    fn duplicate_error() {
        let content = "dup\ndup\n";
        let edits = vec![("dup".to_string(), "x".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(
            e.0.contains("Found 2 occurrences of the text in f.txt"),
            "got: {}",
            e.0
        );
    }

    #[test]
    fn empty_old_text_error() {
        let content = "x\n";
        let edits = vec![(String::new(), "y".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(
            e.0.contains("oldText must not be empty in f.txt"),
            "got: {}",
            e.0
        );
    }

    #[test]
    fn no_change_error() {
        let content = "same\n";
        let edits = vec![("same".to_string(), "same".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(e.0.contains("No changes made to f.txt"), "got: {}", e.0);
    }

    #[test]
    fn overlap_error() {
        let content = "abcdef\n";
        let edits = vec![
            ("abcd".to_string(), "X".to_string()),
            ("cdef".to_string(), "Y".to_string()),
        ];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(e.0.contains("overlap in f.txt"), "got: {}", e.0);
    }

    /// [CYRUP-DELTA] Tier 3 (`line_anchored_matches`). Pi's `fuzzyFindText`
    /// (edit-diff.ts:206-244) stops after the normalized-buffer pass, so an `oldText` that is
    /// the right code at the wrong indent depth is *not found* upstream and is found here.
    #[test]
    fn line_anchored_tier_rebases_to_the_files_own_indent() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let want = "mod a {\n    fn foo() {\n        baz();\n    }\n}\n";
        // Authored with no indentation at all.
        let edits = vec![(
            "fn foo() {\n    bar();\n}".to_string(),
            "fn foo() {\n    baz();\n}".to_string(),
        )];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.new_content, want);
        // Authored carrying only SOME of it — 2 spaces where the file has 4. The needle is
        // outdented by its own minimum first, so both spellings land the same
        // (aider `replace_part_with_missing_leading_whitespace`, editblock_coder.py:248-255).
        let edits = vec![(
            "  fn foo() {\n      bar();\n  }".to_string(),
            "  fn foo() {\n      baz();\n  }".to_string(),
        )];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.new_content, want);
    }

    /// The false-apply bound. The indentation delta must be ONE value across every non-blank
    /// line; a block whose tokens coincide at ragged depths is refused, never rewritten.
    #[test]
    fn ragged_indentation_is_refused_rather_than_mangled() {
        // Same tokens as the needle, but at depths 4, 12 and 6 — no single offset fits.
        let content = "mod a {\n    fn foo() {\n            bar();\n      }\n}\n";
        let edits = vec![("fn foo() {\n    bar();\n}".to_string(), "X".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert!(
            e.0.starts_with("Could not find the exact text in f.rs."),
            "got: {}",
            e.0
        );
    }

    /// `count_occurrences` only sees normalized SUBSTRING occurrences and reports 0 for a
    /// line-anchored match, so the uniqueness rule — the other half of the false-apply bound —
    /// has to be fed from the tier that actually matched.
    #[test]
    fn two_indent_different_copies_are_a_duplicate_not_a_write() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\nmod b {\n        fn foo() {\n            bar();\n        }\n}\n";
        let edits = vec![("fn foo() {\n    bar();\n}".to_string(), "X".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert!(
            e.0.contains("Found 2 occurrences of the text in f.rs"),
            "got: {}",
            e.0
        );
    }

    /// A needle authored MORE indented than the file dedents to a zero delta. That is a real
    /// line-anchored match, not the absence of one: the replacement — authored at the same
    /// too-deep margin — still has to be re-based, or correct text lands at an indentation the
    /// file never had.
    #[test]
    fn a_zero_indent_delta_still_rebases_the_replacement() {
        let content = "a\n  b\n";
        let edits = vec![("  a\n    b".to_string(), "  a\n    c".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "a\n  c\n");
    }

    /// Tiers 1 and 2 are pi's, and the new tier must leave them bit-for-bit alone.
    #[test]
    fn exact_and_fuzzy_tiers_are_untouched_by_the_line_anchored_tier() {
        // Tier 1: the needle is a substring INSIDE an indented line. Pi replaces exactly those
        // bytes and re-indents nothing; a `reindent` that fired here would mangle the result.
        let content = "fn a() {\n\t\tone();\n}\n";
        let edits = vec![("one();".to_string(), "\tone();\n\ttwo();".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.new_content, "fn a() {\n\t\t\tone();\n\ttwo();\n}\n");
        // Tier 2: a trailing-whitespace-only difference still takes the fuzzy overlay.
        let content = "alpha   \nbeta\n";
        let edits = vec![("alpha\nbeta".to_string(), "gamma\ndelta".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "gamma\ndelta\n");
    }

    /// Pi sends an all-whitespace `oldText` into `countOccurrences` (edit-diff.ts:333), which
    /// raises the DUPLICATE error — different remediation advice from "not found". The
    /// line-anchored count must stay a FALLBACK, consulted only when the substring count is 0,
    /// or this input changes its answer.
    #[test]
    fn whitespace_only_old_text_still_reports_duplicates() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![("   ".to_string(), "X".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert!(e.0.contains("occurrences"), "got: {}", e.0);
        assert!(!e.0.contains("Could not find"), "got: {}", e.0);
    }

    /// [CYRUP-DELTA] Pi stops at the sentence (edit-diff.ts:258-267); cyrup appends the closest
    /// region so the caller repairs the needle in one round. Similarity picks what to SHOW and
    /// never what to write.
    #[test]
    fn not_found_names_the_closest_region() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![(
            "    fn foo() {\n        quux();\n    }".to_string(),
            "X".to_string(),
        )];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        // Pi's sentence still LEADS the message.
        assert!(
            e.0.starts_with(
                "Could not find the exact text in f.rs. The old text must match exactly including all whitespace and newlines."
            ),
            "got: {}",
            e.0
        );
        // The appended region names its 1-indexed start line and shows the file's real bytes.
        assert!(
            e.0.contains("Closest region in f.rs starts at line 1:"),
            "got: {}",
            e.0
        );
        assert!(e.0.contains("bar();"), "got: {}", e.0);
    }

    /// The delta is ADDITIVE: below the similarity floor the message is byte-identical to pi's.
    /// Without this, lowering the floor would decorate every failure with a bogus region.
    #[test]
    fn a_far_miss_keeps_pis_bare_sentence() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![(
            "totally unrelated content here\nnot in the file at all\nnope".to_string(),
            "X".to_string(),
        )];
        let e = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap_err();
        assert_eq!(
            e.0,
            "Could not find the exact text in f.rs. The old text must match exactly including all whitespace and newlines."
        );
    }
    // ---------------------------------------------------------------------------------------
    // [CYRUP-DELTA] Partial batch application. Pi discards every edit in a call when one fails
    // (edit.ts:369-374); cyrup applies what matched uniquely and reports the rest. Aider does the
    // same in both engines (editblock_coder.py:41-43, :120-122). These pin the new contract.
    // ---------------------------------------------------------------------------------------

    /// The headline case: one bad needle among five no longer discards the other four.
    #[test]
    fn one_unmatchable_edit_no_longer_discards_the_rest_of_the_batch() {
        let content = "a\nb\nc\nd\ne\n";
        let edits = vec![
            ("a".to_string(), "A".to_string()),
            ("b".to_string(), "B".to_string()),
            ("c".to_string(), "C".to_string()),
            ("d".to_string(), "D".to_string()),
            ("zzz".to_string(), "Z".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "A\nB\nC\nD\ne\n", "got: {}", r.new_content);
        assert_eq!(r.applied, vec![0, 1, 2, 3], "applied: {:?}", r.applied);
        assert_eq!(r.failed.len(), 1, "failed: {:?}", r.failed);
        assert_eq!(r.failed[0].0, 4, "failed: {:?}", r.failed);
        assert!(
            r.failed[0].1.contains("Could not find edits[4] in f.txt"),
            "got: {}",
            r.failed[0].1
        );
    }

    /// EVERY failure is named, not just the first — otherwise a caller repairs one needle per
    /// round trip instead of all of them at once.
    #[test]
    fn every_failing_edit_is_reported_ascending_by_index() {
        let content = "one\ntwo\n";
        let edits = vec![
            ("one".to_string(), "1".to_string()),
            ("xxx".to_string(), "X".to_string()),
            ("yyy".to_string(), "Y".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.applied, vec![0], "applied: {:?}", r.applied);
        let indices: Vec<usize> = r.failed.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![1, 2], "failed: {:?}", r.failed);
        assert!(
            r.failed[0].1.contains("edits[1]") && r.failed[1].1.contains("edits[2]"),
            "failed: {:?}",
            r.failed
        );
    }

    /// An overlapping pair drops BOTH members — each matched uniquely, so applying either would
    /// invent a winner the caller never asked for — while the rest of the call still lands.
    #[test]
    fn an_overlapping_pair_drops_both_and_the_rest_of_the_batch_still_lands() {
        let content = "abcdef\nzz\n";
        let edits = vec![
            ("abcd".to_string(), "X".to_string()),
            ("cdef".to_string(), "Y".to_string()),
            ("zz".to_string(), "Z".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "abcdef\nZ\n", "got: {}", r.new_content);
        assert_eq!(r.applied, vec![2], "applied: {:?}", r.applied);
        let indices: Vec<usize> = r.failed.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![0, 1], "failed: {:?}", r.failed);
    }

    /// An empty `oldText` fails only itself. The sibling here needs the FUZZY tier, which also
    /// pins that the batch-level `used_fuzzy` decision still lands correctly now that its scan
    /// sees empty needles: `str::find("")` is `Some(0)`, so an empty needle takes tier 1 and
    /// contributes `used_fuzzy: false` rather than dragging the batch out of fuzzy space.
    #[test]
    fn an_empty_old_text_fails_only_itself_and_leaves_the_fuzzy_tier_intact() {
        let content = "alpha   \nbeta\n";
        let edits = vec![
            (String::new(), "x".to_string()),
            ("alpha\nbeta".to_string(), "gamma\ndelta".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "gamma\ndelta\n", "got: {}", r.new_content);
        assert_eq!(r.applied, vec![1], "applied: {:?}", r.applied);
        assert_eq!(r.failed.len(), 1, "failed: {:?}", r.failed);
        assert_eq!(r.failed[0].0, 0, "failed: {:?}", r.failed);
        assert!(
            r.failed[0].1.contains("edits[0].oldText must not be empty"),
            "got: {}",
            r.failed[0].1
        );
    }

    /// Nothing matched ⇒ nothing written, reading exactly as an all-or-nothing call does today.
    #[test]
    fn a_batch_in_which_every_edit_fails_writes_nothing_and_reports_all_of_them() {
        let content = "one\ntwo\n";
        let edits = vec![
            ("xxx".to_string(), "X".to_string()),
            ("yyy".to_string(), "Y".to_string()),
        ];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(e.0.contains("Could not find edits[0]"), "got: {}", e.0);
        assert!(e.0.contains("Could not find edits[1]"), "got: {}", e.0);
    }

    /// The ordinary path is untouched: a batch in which everything matches is byte-identical to
    /// what it produced before partial application existed, and reports no failures.
    #[test]
    fn a_fully_successful_batch_is_unchanged_and_reports_no_failures() {
        let content = "alpha\nbeta\ngamma\n";
        let edits = vec![
            ("alpha".to_string(), "A".to_string()),
            ("gamma".to_string(), "G".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "A\nbeta\nG\n", "got: {}", r.new_content);
        assert_eq!(r.applied, vec![0, 1], "applied: {:?}", r.applied);
        assert!(r.failed.is_empty(), "failed: {:?}", r.failed);
    }

    /// A failure carries its FULL near-miss region even when it travels in `failed` rather than
    /// in an `Err` — the report is not abbreviated because siblings succeeded.
    #[test]
    fn a_failing_edit_in_a_partial_batch_still_carries_its_near_miss_region() {
        let content = "mod a {\n    fn foo() {\n        bar();\n    }\n}\n";
        let edits = vec![
            ("mod a {".to_string(), "mod z {".to_string()),
            (
                "    fn foo() {\n        quux();\n    }".to_string(),
                "X".to_string(),
            ),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.rs").unwrap();
        assert_eq!(r.applied, vec![0], "applied: {:?}", r.applied);
        assert_eq!(r.failed.len(), 1, "failed: {:?}", r.failed);
        let msg = &r.failed[0].1;
        assert!(
            msg.starts_with("Could not find edits[1] in f.rs."),
            "got: {msg}"
        );
        assert!(
            msg.contains("Closest region in f.rs starts at line 1:"),
            "got: {msg}"
        );
        assert!(msg.contains("bar();"), "got: {msg}");
    }

    /// The preview names the shortfall in one line per edit that will not land, and does NOT
    /// carry the near-miss region — that belongs in the tool result, not in a diff header.
    #[test]
    fn the_preview_names_the_shortfall_without_the_near_miss_region() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("f.rs"),
            "mod a {\n    fn foo() {\n        bar();\n    }\n}\n",
        )
        .unwrap();
        let edits = [
            ("mod a {".to_string(), "mod z {".to_string()),
            (
                "    fn foo() {\n        quux();\n    }".to_string(),
                "X".to_string(),
            ),
        ];
        let preview = compute_edits_diff("f.rs", &edits, dir.path()).unwrap();
        assert!(!preview.diff.is_empty(), "no diff for the surviving edit");
        assert_eq!(
            preview.unapplied,
            vec![
                "Could not find edits[1] in f.rs. The oldText must match exactly including all whitespace and newlines."
                    .to_string()
            ],
            "unapplied: {:?}",
            preview.unapplied
        );
        assert!(
            !preview.unapplied.join("\n").contains("Closest region"),
            "the preview must not carry the region: {:?}",
            preview.unapplied
        );
    }

    /// Overlap is detected by comparing ADJACENT pairs only, which is sound because a span that
    /// reaches a later edit necessarily reaches its own successor first. Here `edits[0]` covers
    /// both others; it collides with `edits[1]` and both drop, and `edits[2]` — never compared
    /// against `edits[0]` at all — still lands, because the span that covered it is gone.
    #[test]
    fn a_span_covering_two_later_edits_leaves_a_disjoint_survivor() {
        let content = "abcdefgh\n";
        let edits = vec![
            ("abcdefgh".to_string(), "X".to_string()),
            ("bc".to_string(), "Y".to_string()),
            ("fg".to_string(), "Z".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "abcdeZh\n", "got: {}", r.new_content);
        assert_eq!(r.applied, vec![2], "applied: {:?}", r.applied);
        let indices: Vec<usize> = r.failed.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![0, 1], "failed: {:?}", r.failed);
    }

    /// The overlap message names both indices and is recorded against each member, so joining it
    /// naively prints the same sentence twice. `join_failures` collapses identical messages.
    #[test]
    fn an_overlapping_pairs_one_sentence_is_reported_once_not_per_member() {
        let content = "abcdef\n";
        let edits = vec![
            ("abcd".to_string(), "X".to_string()),
            ("cdef".to_string(), "Y".to_string()),
        ];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert_eq!(
            e.0.matches("overlap in f.txt").count(),
            1,
            "the overlap sentence is duplicated: {}",
            e.0
        );
    }
    /// [CYRUP-DELTA] An empty batch is rejected by `EditTool::execute` upstream, but this is a
    /// `pub` entry point: before partial application it fell through to the no-change arm, and it
    /// must not regress to an EMPTY error message now that `matched.is_empty()` returns first.
    #[test]
    fn an_empty_batch_keeps_the_no_change_wording_instead_of_an_empty_message() {
        let e = apply_edits_to_normalized_content("x\n", &[], "f.txt").unwrap_err();
        assert_eq!(
            e.0,
            "No changes made to f.txt. The replacements produced identical content."
        );
    }

    /// The no-change arm composes TWO messages: pi's "no changes made" sentence and the failures
    /// of the edits that never matched. Reaching it needs every SURVIVING edit to be a no-op and
    /// at least one sibling to fail — the one composition in this change no other guard reaches.
    /// Nothing is written: the core returns `Err`, so the survivors' no-op never lands either.
    #[test]
    fn a_no_change_batch_still_reports_the_edits_that_failed() {
        let content = "same\nother\n";
        let edits = vec![
            ("same".to_string(), "same".to_string()),
            ("zzz".to_string(), "9".to_string()),
        ];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        // Both halves, in order, separated by a blank line — the composition IS the behaviour.
        assert_eq!(
            e.0,
            "No changes made to f.txt. The replacements produced identical content.\n\n\
             Could not find edits[1] in f.txt. The oldText must match exactly including all \
             whitespace and newlines."
        );
    }

    /// A chain of three: `edits[1]` collides with BOTH neighbours and so is pushed twice. It must
    /// appear in `failed` ONCE, naming the first collision. `join_failures` would mask a duplicate
    /// inside the message, so this asserts `failed` directly — and `compute_edits_diff` maps
    /// `failed` into `unapplied`, where a duplicate becomes a repeated line in the preview.
    #[test]
    fn an_edit_colliding_with_both_neighbours_is_reported_once() {
        let content = "abcdefgh\nzz\n";
        let edits = vec![
            ("abcd".to_string(), "W".to_string()),
            ("cdef".to_string(), "X".to_string()),
            ("efgh".to_string(), "Y".to_string()),
            ("zz".to_string(), "Z".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        // The whole chain drops; the disjoint edit still lands.
        assert_eq!(r.new_content, "abcdefgh\nZ\n", "got: {}", r.new_content);
        assert_eq!(r.applied, vec![3], "applied: {:?}", r.applied);
        let indices: Vec<usize> = r.failed.iter().map(|(i, _)| *i).collect();
        assert_eq!(indices, vec![0, 1, 2], "each collider once: {:?}", r.failed);
        // ...and edits[1]'s entry names the FIRST collision, not the second.
        assert!(
            r.failed[1].1.contains("edits[0] and edits[1] overlap"),
            "got: {}",
            r.failed[1].1
        );
    }
    /// `applied` is documented ascending, but `matched` is sorted by MATCH POSITION, so
    /// `sort_unstable` is the only thing making that true. These edits are listed in the opposite
    /// order to their positions in the file — the one shape where the two orders differ. Every
    /// other guard's survivors happen to coincide, so the sort is a no-op in all of them.
    #[test]
    fn applied_is_ascending_by_edit_index_not_by_match_position() {
        let content = "beta\nalpha\n";
        let edits = vec![
            ("alpha".to_string(), "A".to_string()),
            ("beta".to_string(), "B".to_string()),
        ];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert_eq!(r.new_content, "B\nA\n", "got: {}", r.new_content);
        // `matched` is [edits[1] @0, edits[0] @5]; `applied` must still read [0, 1].
        assert_eq!(r.applied, vec![0, 1], "applied: {:?}", r.applied);
    }
}
