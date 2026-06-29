//! Line-ending/BOM handling + diff/patch generation for `edit` (R-03-018/019, arch-03 §6.4).

use similar::TextDiff;
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
            _ => parts.push(Part { added, removed, value: change.value().to_string() }),
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
        };
    }
    let fuzzy_content = normalize_for_fuzzy(content);
    let fuzzy_old = normalize_for_fuzzy(old_text);
    match (fuzzy_old.is_empty(), fuzzy_content.find(&fuzzy_old)) {
        (false, Some(idx)) => {
            FuzzyMatch { found: true, index: idx, match_length: fuzzy_old.len(), used_fuzzy: true }
        }
        _ => FuzzyMatch { found: false, index: 0, match_length: 0, used_fuzzy: false },
    }
}

/// `countOccurrences` over the fuzzy-normalized buffer (edit-diff.ts:251-255).
fn count_occurrences(content: &str, old_text: &str) -> usize {
    let fuzzy_content = normalize_for_fuzzy(content);
    let fuzzy_old = normalize_for_fuzzy(old_text);
    if fuzzy_old.is_empty() {
        return 0;
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
        return Err(EditError("Replacement range is outside the base content.".to_string()));
    }
    Ok((start_line, end_line + 1))
}

/// `applyReplacements` (edit-diff.ts:110-119): apply in reverse so byte offsets stay stable.
fn apply_replacements(content: &str, reps: &[Replacement], offset: usize) -> String {
    let mut result = content.to_string();
    for rep in reps.iter().rev() {
        let mi = rep.match_index.saturating_sub(offset);
        let before = result.get(..mi).unwrap_or("").to_string();
        let after = result.get(mi + rep.match_length..).unwrap_or("").to_string();
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
        let (rs, re) =
            get_replacement_line_range(&base_lines, rep.match_index, rep.match_index + rep.match_length)?;
        match groups.last_mut() {
            Some(cur) if rs < cur.end_line => {
                cur.end_line = cur.end_line.max(re);
                cur.reps.push(rep);
            }
            _ => groups.push(Group { start_line: rs, end_line: re, reps: vec![rep] }),
        }
    }

    let mut original_line_index = 0usize;
    let mut result = String::new();
    for group in &groups {
        for line in original_lines.get(original_line_index..group.start_line).unwrap_or(&[]) {
            result.push_str(line);
        }
        let group_start = base_lines.get(group.start_line).map_or(0, |(s, _)| *s);
        let group_end = base_lines.get(group.end_line.saturating_sub(1)).map_or(0, |(_, e)| *e);
        let slice = base.get(group_start..group_end).unwrap_or("");
        result.push_str(&apply_replacements(slice, &group.reps, group_start));
        original_line_index = group.end_line;
    }
    for line in original_lines.get(original_line_index..).unwrap_or(&[]) {
        result.push_str(line);
    }
    Ok(result)
}

fn err_empty(path: &str, i: usize, total: usize) -> EditError {
    EditError(if total == 1 {
        format!("oldText must not be empty in {path}.")
    } else {
        format!("edits[{i}].oldText must not be empty in {path}.")
    })
}

fn err_not_found(path: &str, i: usize, total: usize) -> EditError {
    EditError(if total == 1 {
        format!("Could not find the exact text in {path}. The old text must match exactly including all whitespace and newlines.")
    } else {
        format!("Could not find edits[{i}] in {path}. The oldText must match exactly including all whitespace and newlines.")
    })
}

fn err_duplicate(path: &str, i: usize, total: usize, occ: usize) -> EditError {
    EditError(if total == 1 {
        format!("Found {occ} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique.")
    } else {
        format!("Found {occ} occurrences of edits[{i}] in {path}. Each oldText must be unique. Please provide more context to make it unique.")
    })
}

fn err_no_change(path: &str, total: usize) -> EditError {
    EditError(if total == 1 {
        format!("No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.")
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

    for (i, (old, _)) in normalized_edits.iter().enumerate() {
        if old.is_empty() {
            return Err(err_empty(path, i, total));
        }
    }

    let used_fuzzy = normalized_edits
        .iter()
        .any(|(old, _)| fuzzy_find_text(normalized_content, old).used_fuzzy);
    let replacement_base = if used_fuzzy {
        normalize_for_fuzzy(normalized_content)
    } else {
        normalized_content.to_string()
    };

    let mut matched: Vec<Replacement> = Vec::new();
    for (i, (old, new)) in normalized_edits.iter().enumerate() {
        let mr = fuzzy_find_text(&replacement_base, old);
        if !mr.found {
            return Err(err_not_found(path, i, total));
        }
        let occ = count_occurrences(&replacement_base, old);
        if occ > 1 {
            return Err(err_duplicate(path, i, total, occ));
        }
        matched.push(Replacement {
            edit_index: i,
            match_index: mr.index,
            match_length: mr.match_length,
            new_text: new.clone(),
        });
    }

    matched.sort_by_key(|m| m.match_index);
    for pair in matched.windows(2) {
        if let [prev, cur] = pair
            && prev.match_index + prev.match_length > cur.match_index
        {
            return Err(EditError(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                prev.edit_index, cur.edit_index
            )));
        }
    }

    let base_content = normalized_content.to_string();
    let new_content = if used_fuzzy {
        apply_replacements_preserving_unchanged_lines(normalized_content, &replacement_base, &matched)?
    } else {
        apply_replacements(&replacement_base, &matched, 0)
    };

    if base_content == new_content {
        return Err(err_no_change(path, total));
    }

    Ok(AppliedEdits { base_content, new_content })
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
        assert!(r.new_content.contains("let x = 'bye';"), "got: {:?}", r.new_content);
        assert!(r.new_content.contains("val - z"), "got: {:?}", r.new_content);
    }

    #[test]
    fn fuzzy_nfkc_ligature_and_fullwidth() {
        // Disk has the `ﬁ` ligature (U+FB01) and a full-width digit `２` (U+FF12); the model sends
        // the plain ASCII forms. NFKC (edit-diff.ts:36) folds both so the fuzzy pass matches.
        let content = "const \u{FB01}le2 = \u{FF12};\n";
        let edits = vec![("const file2 = 2;".to_string(), "const file2 = 9;".to_string())];
        let r = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap();
        assert!(r.new_content.contains("const file2 = 9;"), "got: {:?}", r.new_content);
    }

    #[test]
    fn not_found_error_is_indexed_for_multi() {
        let content = "one\ntwo\n";
        let edits =
            vec![("one".to_string(), "1".to_string()), ("zzz".to_string(), "9".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(e.0.contains("Could not find edits[1] in f.txt"), "got: {}", e.0);
    }

    #[test]
    fn duplicate_error() {
        let content = "dup\ndup\n";
        let edits = vec![("dup".to_string(), "x".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(e.0.contains("Found 2 occurrences of the text in f.txt"), "got: {}", e.0);
    }

    #[test]
    fn empty_old_text_error() {
        let content = "x\n";
        let edits = vec![(String::new(), "y".to_string())];
        let e = apply_edits_to_normalized_content(content, &edits, "f.txt").unwrap_err();
        assert!(e.0.contains("oldText must not be empty in f.txt"), "got: {}", e.0);
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
}
