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

/// Which limit triggered truncation.
#[derive(serde::Serialize, serde::Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TruncatedBy {
    Lines,
    Bytes,
}

/// Truncation report (arch-03 §3.2) — serialized into a tool's `details`.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Truncation {
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
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
        Self { max_lines, max_bytes }
    }
    /// Byte-cap only: row count already bounded by the caller (grep/find/ls).
    pub fn bytes_only(max_bytes: usize) -> Self {
        Self { max_lines: usize::MAX, max_bytes }
    }
}

/// Truncated content + its report.
#[derive(Clone, Debug)]
pub struct Truncated {
    pub content: String,
    pub info: Truncation,
}

/// Split into logical lines, popping a single trailing newline (Pi parity).
fn split_lines(content: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = content.split('\n').collect();
    if content.ends_with('\n') {
        lines.pop();
    }
    lines
}

/// Head truncation — keep the first N (read/grep/find/ls). Never returns a partial line. If the
/// first line alone exceeds `max_bytes`, output is empty and `first_line_exceeds_limit` is set.
pub fn truncate_head(content: &str, opts: TruncOpts) -> Truncated {
    let lines = split_lines(content);
    let total_lines = lines.len();
    let total_bytes = content.len();

    let mut out = String::new();
    let mut output_lines = 0usize;
    let mut truncated_by: Option<TruncatedBy> = None;
    let mut first_line_exceeds_limit = false;

    for line in &lines {
        if output_lines >= opts.max_lines {
            truncated_by = Some(TruncatedBy::Lines);
            break;
        }
        let added = if out.is_empty() { line.len() } else { line.len() + 1 };
        if out.len() + added > opts.max_bytes {
            if output_lines == 0 {
                first_line_exceeds_limit = true;
            }
            truncated_by = Some(TruncatedBy::Bytes);
            break;
        }
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(line);
        output_lines += 1;
    }

    let truncated = output_lines < total_lines;
    if !truncated {
        truncated_by = None;
        first_line_exceeds_limit = false;
    }

    let output_bytes = out.len();
    Truncated {
        content: out,
        info: Truncation {
            truncated,
            truncated_by,
            total_lines,
            total_bytes,
            output_lines,
            output_bytes,
            last_line_partial: false,
            first_line_exceeds_limit,
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
pub fn truncate_tail(content: &str, opts: TruncOpts) -> Truncated {
    let lines = split_lines(content);
    let total_lines = lines.len();
    let total_bytes = content.len();

    let mut selected: Vec<&str> = Vec::new();
    let mut bytes = 0usize;
    let mut truncated_by: Option<TruncatedBy> = None;
    let mut last_line_partial = false;

    for line in lines.iter().rev() {
        if selected.len() >= opts.max_lines {
            truncated_by = Some(TruncatedBy::Lines);
            break;
        }
        let added = if selected.is_empty() { line.len() } else { line.len() + 1 };
        if bytes + added > opts.max_bytes {
            if selected.is_empty() {
                let cut = utf8_tail(line, opts.max_bytes);
                selected.push(cut);
                last_line_partial = true;
            }
            truncated_by = Some(TruncatedBy::Bytes);
            break;
        }
        bytes += added;
        selected.push(line);
    }

    selected.reverse();
    let out = selected.join("\n");
    let output_lines = selected.len();

    let truncated = output_lines < total_lines || last_line_partial;
    if !truncated {
        truncated_by = None;
    }

    let output_bytes = out.len();
    Truncated {
        content: out,
        info: Truncation {
            truncated,
            truncated_by,
            total_lines,
            total_bytes,
            output_lines,
            output_bytes,
            last_line_partial,
            first_line_exceeds_limit: false,
            max_lines: opts.max_lines,
            max_bytes: opts.max_bytes,
        },
    }
}

/// Per-line cap (~`max_chars` chars) + a `... [truncated]` marker. Borrows when under cap.
pub fn truncate_line(line: &str, max_chars: usize) -> (Cow<'_, str>, bool) {
    if line.chars().count() <= max_chars {
        return (Cow::Borrowed(line), false);
    }
    let mut s: String = line.chars().take(max_chars).collect();
    s.push_str("... [truncated]");
    (Cow::Owned(s), true)
}

/// Human-readable size for notices ("50KB", "1MB", "12B").
pub fn format_size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{}MB", bytes / (1024 * 1024))
    } else if bytes >= 1024 {
        format!("{}KB", bytes / 1024)
    } else {
        format!("{bytes}B")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn head_keeps_first_n_lines() {
        let content = (1..=10).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
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
    fn head_no_truncation() {
        let t = truncate_head("a\nb\n", TruncOpts::new(2000, DEFAULT_MAX_BYTES));
        assert!(!t.info.truncated);
        assert_eq!(t.info.total_lines, 2);
        assert_eq!(t.content, "a\nb");
    }

    #[test]
    fn tail_keeps_last_n_lines() {
        let content = (1..=10).map(|i| i.to_string()).collect::<Vec<_>>().join("\n");
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
        assert_eq!(out.chars().count(), GREP_MAX_LINE_LENGTH + "... [truncated]".chars().count());
    }

    #[test]
    fn sizes_format() {
        assert_eq!(format_size(50 * 1024), "50KB");
        assert_eq!(format_size(12), "12B");
    }
}
