//! The truncation model (R-03-001…004, arch-03 §6.1).
//!
//! Two independent limits — a **line** count and a **byte** count (UTF-8 byte length) — whichever
//! is hit first wins. `read`/`grep`/`find`/`ls` use head truncation; `bash` uses tail truncation
//! (with full-output spill, see [`crate::output`]). A single oversized line is reported explicitly
//! (`first_line_exceeds_limit` for read; the per-line cap for grep).

use std::borrow::Cow;

/// Default line cap for `read`/`bash` output.
pub const DEFAULT_MAX_LINES: usize = 2000;
/// Default byte cap (50 KB) shared by every tool.
pub const DEFAULT_MAX_BYTES: usize = 50 * 1024;
/// Per-line cap for `grep` match lines (~500 chars).
pub const GREP_MAX_LINE_LENGTH: usize = 500;
/// Match cap for `grep`.
pub const GREP_MAX_MATCHES: usize = 100;
/// Result cap for `find`.
pub const FIND_MAX_RESULTS: usize = 1000;
/// Entry cap for `ls`.
pub const LS_MAX_ENTRIES: usize = 500;
/// JavaScript's `Number.MAX_SAFE_INTEGER` (2^53 − 1), the "no line cap" sentinel pi's byte-only
/// truncation callers pass verbatim. See [`TruncOpts::bytes_only`] (TOOL-044).
pub const MAX_SAFE_INTEGER: usize = 9_007_199_254_740_991;

/// Which limit triggered truncation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// Truncation report (arch-03 §3.2) — serialized into a tool's `details`.
///
/// Pi `core/tools/truncate.ts:15-38` `TruncationResult` @v0.83.0. `details` is persisted on
/// `ToolResultMessage` (pi `packages/ai/src/types.ts:415-420`), so every field here reaches the
/// session file and is interop surface, not an internal.
///
/// **TOOL-044 (2026-08-14, sweep 9).** Two of the three divergences the mechanical diff of this
/// struct found are closed here; the third is the item's stated residual.
/// * `truncatedBy` was `skip_serializing_if = "Option::is_none"`, so the key VANISHED on an
///   untruncated result. pi's type is `"lines" | "bytes" | null` (`truncate.ts:21`) and the field
///   is always present — an explicit `null` is not the same record as an absent key to any reader
///   that distinguishes them. The skip is gone; `None` now serializes as `null`.
/// * `maxLines` — see [`TruncOpts::bytes_only`].
/// * `content` — **PORTED 2026-08-15, closing TOOL-044's residual.** pi's `TruncationResult`
///   declares `content` FIRST (`truncate.ts:17`), and every one of pi's five call sites puts the
///   object into `details.truncation` **whole**, so the text is in the record twice: `read.ts:294`
///   / `:305`, `grep.ts:348`, `find.ts:199` / `:336`, `ls.ts:193`, and for `bash` both the
///   streaming `details` (`bash.ts:356`) and the final one (`:409`), whose `snapshot.truncation`
///   is `{...tailTruncation, …}` (`output-accumulator.ts:100-107`) — the spread is what carries
///   `content` in. Re-derived at v0.83.0 this pass.
///
///   The prior pass left this unported on a cost argument (it duplicates up to `max_bytes` of text
///   per truncated result in the session file, and `git grep -n 'truncation\.content' v0.83.0 --
///   packages/` shows every hit is a LOCAL variable inside an `execute()` body, so no pi consumer
///   reads it). **That decision is reversed and the reasoning is recorded so it is not re-litigated
///   a third time:** the cost is exactly the cost pi pays for the identical record, so it is not a
///   divergence cyrup is *forced* into — and an unforced divergence is precisely what the port rule
///   does not permit. `details` is session-record interop, and interop that differs "only where
///   nobody currently looks" is a difference a future reader of BOTH implementations' records hits
///   first.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Truncation {
    /// The truncated content (pi `TruncationResult.content`, `truncate.ts:17` @v0.83.0) — the same
    /// text as [`Truncated::content`], carried into the serialized record because pi's object is
    /// one struct and cyrup's is two.
    ///
    /// `#[serde(default)]` on the READ side only: cyrup session files written before this field
    /// existed have no `content` key, and a hard failure to load an old session is a worse
    /// outcome than an empty string in a field nothing reads. The WRITE side is unconditional, so
    /// every record cyrup emits from here on carries pi's shape.
    #[serde(default)]
    pub content: String,
    pub truncated: bool,
    pub truncated_by: Option<TruncatedBy>,
    pub total_lines: usize,
    pub total_bytes: usize,
    pub output_lines: usize,
    pub output_bytes: usize,
    pub last_line_partial: bool,
    pub first_line_exceeds_limit: bool,
    pub max_lines: usize,
    pub max_bytes: usize,
}

/// Limits handed to a truncation call.
#[derive(Clone, Copy, Debug)]
pub struct TruncOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
}

impl TruncOpts {
    pub fn new(max_lines: usize, max_bytes: usize) -> Self {
        Self {
            max_lines,
            max_bytes,
        }
    }
    /// Byte-cap only: row count already bounded by the caller (grep/find/ls).
    ///
    /// The sentinel is pi's `Number.MAX_SAFE_INTEGER`, not `usize::MAX` (TOOL-044). All four of
    /// pi's byte-only call sites pass `{ maxLines: Number.MAX_SAFE_INTEGER }` literally —
    /// `grep.ts:335`, `find.ts:189`, `find.ts:324`, `ls.ts:182` @v0.83.0 — and the value is copied
    /// verbatim into the serialized report's `maxLines`, so `usize::MAX` wrote
    /// `18446744073709551615` into every `details.truncation` where pi writes
    /// `9007199254740991`. Both are "effectively unbounded" to the truncation arithmetic and
    /// neither is reachable as a real line count; the difference is only ever visible in the
    /// session record, which is exactly where interop is read.
    pub fn bytes_only(max_bytes: usize) -> Self {
        Self {
            max_lines: MAX_SAFE_INTEGER,
            max_bytes,
        }
    }
}

/// Truncated content + its report.
#[derive(Clone, Debug)]
pub struct Truncated {
    pub content: String,
    pub info: Truncation,
}

/// Split into logical lines, popping a single trailing newline (Pi `splitLinesForCounting`,
/// truncate.ts:47-56). Empty input yields zero lines (the explicit `content.length === 0` guard,
/// truncate.ts:48-50) — without it, `"".split('\n')` would report a phantom single empty line.
fn split_lines(content: &str) -> Vec<&str> {
    if content.is_empty() {
        return Vec::new();
    }
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Head truncation — keep the first N (read/grep/find/ls). Never returns a partial line. If the
/// first line alone exceeds `max_bytes`, output is empty and `first_line_exceeds_limit` is set.
///
/// Mirrors Pi's `truncateHead` control flow exactly (truncate.ts:78-160): the no-truncation branch
/// returns the **input content verbatim** (truncate.ts:87-101) — preserving any trailing newline —
/// rather than rebuilding from `split_lines` (which pops it). Only the genuinely-truncated branches
/// rebuild via `join("\n")`.
pub fn truncate_head(content: &str, opts: TruncOpts) -> Truncated {
    let total_bytes = content.len();
    let lines = split_lines(content);
    let total_lines = lines.len();

    // No truncation needed — return content unchanged (truncate.ts:87-101). This is load-bearing:
    // a newline-terminated file/window keeps its trailing `\n`, matching Pi byte-for-byte.
    if total_lines <= opts.max_lines && total_bytes <= opts.max_bytes {
        return Truncated {
            content: content.to_string(),
            info: Truncation {
                content: content.to_string(),
                truncated: false,
                truncated_by: None,
                total_lines,
                total_bytes,
                output_lines: total_lines,
                output_bytes: total_bytes,
                last_line_partial: false,
                first_line_exceeds_limit: false,
                max_lines: opts.max_lines,
                max_bytes: opts.max_bytes,
            },
        };
    }

    // First line alone exceeds the byte limit — empty content, flagged (truncate.ts:103-119).
    let first_line_bytes = lines.first().map_or(0, |l| l.len());
    if first_line_bytes > opts.max_bytes {
        return Truncated {
            content: String::new(),
            info: Truncation {
                content: String::new(),
                truncated: true,
                truncated_by: Some(TruncatedBy::Bytes),
                total_lines,
                total_bytes,
                output_lines: 0,
                output_bytes: 0,
                last_line_partial: false,
                first_line_exceeds_limit: true,
                max_lines: opts.max_lines,
                max_bytes: opts.max_bytes,
            },
        };
    }

    // Collect complete lines that fit (truncate.ts:121-145).
    let mut out_lines: Vec<&str> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;

    for (i, line) in lines.iter().enumerate() {
        if i >= opts.max_lines {
            break;
        }
        let line_bytes = line.len() + usize::from(i > 0); // +1 for the joining newline
        if output_bytes_count + line_bytes > opts.max_bytes {
            truncated_by = TruncatedBy::Bytes;
            break;
        }
        out_lines.push(line);
        output_bytes_count += line_bytes;
    }

    // Exited on the line limit (truncate.ts:139-142).
    if out_lines.len() >= opts.max_lines && output_bytes_count <= opts.max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let out = out_lines.join("\n");
    let output_bytes = out.len();
    Truncated {
        content: out.clone(),
        info: Truncation {
            content: out,
            truncated: true,
            truncated_by: Some(truncated_by),
            total_lines,
            total_bytes,
            output_lines: out_lines.len(),
            output_bytes,
            last_line_partial: false,
            first_line_exceeds_limit: false,
            max_lines: opts.max_lines,
            max_bytes: opts.max_bytes,
        },
    }
}

/// Slice the last `max_bytes` of `s` on a UTF-8 boundary (no panic).
fn utf8_tail(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut start = s.len().saturating_sub(max_bytes);
    while start < s.len() && !s.is_char_boundary(start) {
        start += 1;
    }
    s.get(start..).unwrap_or(s)
}

/// Tail truncation — keep the last N (bash live preview). May return a partial first line iff a
/// single last line exceeds `max_bytes` (`last_line_partial`), cut on a UTF-8 boundary.
///
/// Mirrors Pi's `truncateTail` control flow exactly (truncate.ts:168-241): the no-truncation branch
/// returns the **input content verbatim** (truncate.ts:177-191), preserving any trailing newline.
pub fn truncate_tail(content: &str, opts: TruncOpts) -> Truncated {
    let total_bytes = content.len();
    let lines = split_lines(content);
    let total_lines = lines.len();

    // No truncation needed — return content unchanged (truncate.ts:177-191).
    if total_lines <= opts.max_lines && total_bytes <= opts.max_bytes {
        return Truncated {
            content: content.to_string(),
            info: Truncation {
                content: content.to_string(),
                truncated: false,
                truncated_by: None,
                total_lines,
                total_bytes,
                output_lines: total_lines,
                output_bytes: total_bytes,
                last_line_partial: false,
                first_line_exceeds_limit: false,
                max_lines: opts.max_lines,
                max_bytes: opts.max_bytes,
            },
        };
    }

    // Work backwards from the end (truncate.ts:193-218).
    let mut selected: Vec<&str> = Vec::new();
    let mut output_bytes_count = 0usize;
    let mut truncated_by = TruncatedBy::Lines;
    let mut last_line_partial = false;

    for line in lines.iter().rev() {
        if selected.len() >= opts.max_lines {
            break;
        }
        let line_bytes = line.len() + usize::from(!selected.is_empty()); // +1 for the newline
        if output_bytes_count + line_bytes > opts.max_bytes {
            truncated_by = TruncatedBy::Bytes;
            // Edge case (truncate.ts:205-212): no line added yet and this one exceeds the limit —
            // take the end of the line, cut on a UTF-8 boundary, and flag it partial.
            if selected.is_empty() {
                let cut = utf8_tail(line, opts.max_bytes);
                output_bytes_count = cut.len();
                selected.push(cut);
                last_line_partial = true;
            }
            break;
        }
        selected.push(line);
        output_bytes_count += line_bytes;
    }

    selected.reverse();

    // Exited on the line limit (truncate.ts:220-223).
    if selected.len() >= opts.max_lines && output_bytes_count <= opts.max_bytes {
        truncated_by = TruncatedBy::Lines;
    }

    let out = selected.join("\n");
    let output_bytes = out.len();
    Truncated {
        content: out.clone(),
        info: Truncation {
            content: out,
            truncated: true,
            truncated_by: Some(truncated_by),
            total_lines,
            total_bytes,
            output_lines: selected.len(),
            output_bytes,
            last_line_partial,
            first_line_exceeds_limit: false,
            max_lines: opts.max_lines,
            max_bytes: opts.max_bytes,
        },
    }
}

/// Per-line cap (~`max_chars` chars) + a `... [truncated]` marker. Borrows when under cap.
///
/// Pi counts UTF-16 code units (`line.length`) and slices in UTF-16 space (`line.slice(0, max)`),
/// so astral characters count as 2 (truncate.ts:268-276). We mirror that exactly.
pub fn truncate_line(line: &str, max_chars: usize) -> (Cow<'_, str>, bool) {
    let utf16_len = line.encode_utf16().count();
    if utf16_len <= max_chars {
        return (Cow::Borrowed(line), false);
    }
    let units: Vec<u16> = line.encode_utf16().take(max_chars).collect();
    let mut s = String::from_utf16_lossy(&units);
    s.push_str("... [truncated]");
    (Cow::Owned(s), true)
}

/// Human-readable size for notices ("50.0KB", "1.5KB", "1.0MB", "12B").
///
/// Pi's `formatSize` (truncate.ts:61-69) uses `toFixed(1)` for KB/MB — so the 50 KB byte limit
/// renders as `50.0KB`, not `50KB`. This is load-bearing for footer/notice parity across every tool.
pub fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn head_keeps_first_n_lines() {
        let content = (1..=10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let t = truncate_head(&content, TruncOpts::new(3, DEFAULT_MAX_BYTES));
        assert_eq!(t.content, "1\n2\n3");
        assert!(t.info.truncated);
        assert_eq!(t.info.truncated_by, Some(TruncatedBy::Lines));
        assert_eq!(t.info.total_lines, 10);
        assert_eq!(t.info.output_lines, 3);
    }

    #[test]
    fn head_first_line_exceeds_bytes() {
        let content = "x".repeat(100);
        let t = truncate_head(&content, TruncOpts::new(2000, 10));
        assert!(t.info.first_line_exceeds_limit);
        assert_eq!(t.info.output_lines, 0);
    }

    #[test]
    fn head_no_truncation_preserves_trailing_newline() {
        // Pi returns the input verbatim on the no-truncation branch (truncate.ts:87-101), so a
        // newline-terminated input keeps its trailing `\n` — it is NOT rebuilt from split lines.
        let t = truncate_head("a\nb\n", TruncOpts::new(2000, DEFAULT_MAX_BYTES));
        assert!(!t.info.truncated);
        assert_eq!(t.info.total_lines, 2);
        assert_eq!(t.content, "a\nb\n");
        // output_bytes reflects the full input (incl. the trailing newline), like Pi (totalBytes).
        assert_eq!(t.info.output_bytes, 4);
        assert_eq!(t.info.output_lines, 2);
    }

    #[test]
    fn head_no_truncation_no_trailing_newline_is_verbatim() {
        let t = truncate_head("a\nb", TruncOpts::new(2000, DEFAULT_MAX_BYTES));
        assert!(!t.info.truncated);
        assert_eq!(t.content, "a\nb");
        assert_eq!(t.info.output_bytes, 3);
    }

    #[test]
    fn head_empty_is_verbatim() {
        let t = truncate_head("", TruncOpts::new(2000, DEFAULT_MAX_BYTES));
        assert!(!t.info.truncated);
        assert_eq!(t.content, "");
        assert_eq!(t.info.total_lines, 0);
        assert_eq!(t.info.output_bytes, 0);
    }

    #[test]
    fn tail_no_truncation_preserves_trailing_newline() {
        // Same verbatim short-circuit on tail (truncate.ts:177-191) — used by non-truncated bash.
        let t = truncate_tail("a\nb\n", TruncOpts::new(2000, DEFAULT_MAX_BYTES));
        assert!(!t.info.truncated);
        assert_eq!(t.content, "a\nb\n");
        assert_eq!(t.info.output_bytes, 4);
        assert_eq!(t.info.output_lines, 2);
    }

    #[test]
    fn tail_keeps_last_n_lines() {
        let content = (1..=10)
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let t = truncate_tail(&content, TruncOpts::new(3, DEFAULT_MAX_BYTES));
        assert_eq!(t.content, "8\n9\n10");
        assert!(t.info.truncated);
        assert_eq!(t.info.total_lines, 10);
    }

    #[test]
    fn tail_partial_last_line() {
        let content = "y".repeat(100);
        let t = truncate_tail(&content, TruncOpts::new(2000, 10));
        assert!(t.info.last_line_partial);
        assert_eq!(t.content.len(), 10);
    }

    #[test]
    fn line_cap_marks_truncation() {
        let line = "z".repeat(600);
        let (out, t) = truncate_line(&line, GREP_MAX_LINE_LENGTH);
        assert!(t);
        assert!(out.ends_with("... [truncated]"));
        assert_eq!(
            out.chars().count(),
            GREP_MAX_LINE_LENGTH + "... [truncated]".chars().count()
        );
    }

    #[test]
    fn sizes_format() {
        // Pi `toFixed(1)` parity: exact KB multiple still shows one decimal.
        assert_eq!(format_size(50 * 1024), "50.0KB");
        assert_eq!(format_size(1536), "1.5KB");
        assert_eq!(format_size(1024 * 1024), "1.0MB");
        assert_eq!(format_size(12), "12B");
    }

    #[test]
    fn line_cap_counts_utf16_units() {
        // An astral char ('𝄞', U+1D11E) is 2 UTF-16 units, like Pi's `String.length`.
        let line: String = "𝄞".repeat(300);
        let (_out, t) = truncate_line(&line, GREP_MAX_LINE_LENGTH);
        // 300 chars * 2 units = 600 > 500 ⇒ truncated.
        assert!(t);
        // Exactly 500 units == 250 astral chars retained.
        let units = line.encode_utf16().take(GREP_MAX_LINE_LENGTH).count();
        assert_eq!(units, GREP_MAX_LINE_LENGTH);
    }
}
