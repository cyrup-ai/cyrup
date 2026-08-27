use super::*;

/// Pi's default read/write byte + line truncation limits (`truncate.ts:11-12`).
const DEFAULT_MAX_BYTES: u64 = 50 * 1024;

/// Shared head-N list body for grep/find/ls (`\n` + first N output lines + a `… more` hint).
pub(super) fn push_list_output(
    run: &ToolRun,
    expanded: bool,
    head: usize,
    theme: &UiTheme,
    expand_key: &str,
    out: &mut Vec<Line<'static>>,
) {
    let Some(result) = &run.result else { return };
    let output = result_text(result);
    let output = output.trim();
    if output.is_empty() {
        return;
    }
    let all: Vec<&str> = output.split('\n').collect();
    let total = all.len();
    let shown = if expanded { total } else { total.min(head) };
    out.push(Line::default());
    for l in all.iter().take(shown) {
        out.push(Line::styled((*l).to_string(), theme.tool_output_style()));
    }
    let remaining = total.saturating_sub(shown);
    if remaining > 0 {
        out.push(more_lines_hint(remaining, None, expand_key, theme));
    }
}

/// Push an error body (`\n` + the result text in the error color): edit/write on failure.
pub(super) fn push_error_body(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let text = result_text(result);
    if text.trim().is_empty() {
        return;
    }
    out.push(Line::default());
    for l in text.split('\n') {
        out.push(Line::styled(l.to_string(), theme.error_style()));
    }
}

/// Extract the tool result's display text (`getTextOutput`, render-utils.ts:39-63): join the `text`
/// blocks of `{content:[…]}`, else a `text`/`output`/`stdout`/`message` string field, else a bare
/// string/array.
///
/// Every branch goes through [`crate::ansi::sanitize_display_text`] — the full
/// `sanitizeBinaryOutput(stripAnsi(text)).replace(/\r/g, "")` of `render-utils.ts:48`, not just the
/// `\r` drop. Only `bash` output arrives pre-sanitized (at capture, `cyrup-session-svc/src/bash.rs`
/// `sanitize_chunk`); `read`/`ls`/`find`/`grep` and every extension tool reach here raw, and the
/// transform is idempotent so the pre-sanitized path is unaffected.
///
/// `image` blocks are NOT represented here — they are rendered by [`tool_lines`], either as an
/// inline half-block raster or as Pi's `[Image: …]` stand-in ([`push_image_fallbacks`]) — so this is
/// the `showImages`-on half of Pi's `getTextOutput`, whose image-indicator half lives there.
pub(super) fn result_text(result: &Value) -> String {
    match result {
        Value::String(s) => crate::ansi::sanitize_display_text(s),
        Value::Object(o) => {
            if let Some(content) = o.get("content") {
                return content_blocks_text(content);
            }
            for k in ["text", "output", "stdout", "message"] {
                if let Some(Value::String(s)) = o.get(k) {
                    return crate::ansi::sanitize_display_text(s);
                }
            }
            String::new()
        }
        Value::Array(_) => content_blocks_text(result),
        _ => String::new(),
    }
}

/// Join a `content` block array into text (`text` blocks concatenated with `\n`). `image` blocks are
/// skipped — [`tool_lines`] renders them (raster or `[Image: …]` stand-in).
fn content_blocks_text(content: &Value) -> String {
    match content {
        Value::Array(items) => {
            let mut parts = Vec::new();
            for it in items {
                if let Some(obj) = it.as_object() {
                    let ty = obj.get("type").and_then(Value::as_str);
                    if matches!(ty, Some("text") | None)
                        && let Some(Value::String(t)) = obj.get("text")
                    {
                        parts.push(crate::ansi::sanitize_display_text(t));
                        continue;
                    }
                } else if let Some(s) = it.as_str() {
                    parts.push(crate::ansi::sanitize_display_text(s));
                }
            }
            parts.join("\n")
        }
        Value::String(s) => crate::ansi::sanitize_display_text(s),
        _ => String::new(),
    }
}

/// Drop trailing empty lines (`trimTrailingEmptyLines`, read.ts:79-85 / write.ts:123-129).
pub(super) fn trim_trailing_empty(mut lines: Vec<&str>) -> Vec<&str> {
    while lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

/// The truncation object from `result.details.truncation` when `truncated` is set.
pub(super) fn truncation(result: &Value) -> Option<&Value> {
    let t = result.get("details")?.get("truncation")?;
    (t.get("truncated") == Some(&Value::Bool(true))).then_some(t)
}

fn tnum(t: &Value, key: &str) -> u64 {
    t.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// `formatSize` (truncate.ts:61-69): `B` / `KB` / `MB`.
fn format_size(bytes: u64) -> String {
    if bytes < 1024 {
        format!("{bytes}B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1}KB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1}MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// `formatDuration` (bash.ts:197-199): `{s}.{tenths}s`.
pub(super) fn format_duration(ms: u64) -> String {
    format!("{:.1}s", ms as f64 / 1000.0)
}

/// read `renderResult` truncation footer (read.ts:190-199).
pub(super) fn push_read_truncation(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(t) = truncation(result) else { return };
    let max_bytes = t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES);
    let msg = if t.get("firstLineExceedsLimit") == Some(&Value::Bool(true)) {
        format!("[First line exceeds {} limit]", format_size(max_bytes))
    } else if t.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
        format!(
            "[Truncated: showing {} of {} lines ({} line limit)]",
            tnum(t, "outputLines"),
            tnum(t, "totalLines"),
            tnum(t, "maxLines"),
        )
    } else {
        format!("[Truncated: {} lines shown ({} limit)]", tnum(t, "outputLines"), format_size(max_bytes))
    };
    out.push(Line::styled(msg, theme.warning_style()));
}

/// Strip the `\n\n[Showing lines … Full output: <path>]` footer bash bakes into the text but re-renders
/// as a warning (bash.ts:226-231): only when finished + truncated + a `fullOutputPath` is present.
pub(super) fn strip_bash_footer(output: &str, result: &Value, done: bool) -> String {
    let full = result.get("details").and_then(|d| d.get("fullOutputPath")).and_then(Value::as_str);
    if done
        && truncation(result).is_some()
        && let Some(path) = full
        && output.ends_with(']')
        && let Some(idx) = output.rfind("\n\n[")
        && let Some((head, tail)) = output.split_at_checked(idx)
        && tail.contains(path)
    {
        return head.trim_end().to_string();
    }
    output.to_string()
}

/// bash `renderResult` truncation + full-output warnings (bash.ts:267-282).
pub(super) fn push_bash_warnings(result: &Value, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let full = result.get("details").and_then(|d| d.get("fullOutputPath")).and_then(Value::as_str);
    let trunc = truncation(result);
    if trunc.is_none() && full.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(p) = full {
        warns.push(format!("Full output: {p}"));
    }
    if let Some(t) = trunc {
        if t.get("truncatedBy").and_then(Value::as_str) == Some("lines") {
            warns.push(format!("Truncated: showing {} of {} lines", tnum(t, "outputLines"), tnum(t, "totalLines")));
        } else {
            let max_bytes = t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES);
            warns.push(format!("Truncated: {} lines shown ({} limit)", tnum(t, "outputLines"), format_size(max_bytes)));
        }
    }
    // X10 — `bash.ts:311` is `new Text(`\n${theme.fg("warning", …)}`, 0, 0)`; the leading `\n` makes
    // `wrapTextWithAnsi` emit an empty first row (`utils.ts:839` splits on it), so the warning row
    // is always preceded by a blank.
    out.push(Line::default());
    out.push(Line::styled(format!("[{}]", warns.join(". ")), theme.warning_style()));
}

/// grep `renderResult` warnings (grep.ts:110-119).
pub(super) fn push_grep_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let details = result.get("details");
    let match_limit = details.and_then(|d| d.get("matchLimitReached")).and_then(Value::as_u64);
    let lines_trunc = details.and_then(|d| d.get("linesTruncated")) == Some(&Value::Bool(true));
    let trunc = truncation(result);
    if match_limit.is_none() && trunc.is_none() && !lines_trunc {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = match_limit {
        warns.push(format!("{n} matches limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    if lines_trunc {
        warns.push("some lines truncated".to_string());
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// find `renderResult` warnings (find.ts:98-105).
pub(super) fn push_find_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let result_limit =
        result.get("details").and_then(|d| d.get("resultLimitReached")).and_then(Value::as_u64);
    let trunc = truncation(result);
    if result_limit.is_none() && trunc.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = result_limit {
        warns.push(format!("{n} results limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// ls `renderResult` warnings (ls.ts:84-91).
pub(super) fn push_ls_warnings(result: Option<&Value>, theme: &UiTheme, out: &mut Vec<Line<'static>>) {
    let Some(result) = result else { return };
    let entry_limit =
        result.get("details").and_then(|d| d.get("entryLimitReached")).and_then(Value::as_u64);
    let trunc = truncation(result);
    if entry_limit.is_none() && trunc.is_none() {
        return;
    }
    let mut warns = Vec::new();
    if let Some(n) = entry_limit {
        warns.push(format!("{n} entries limit"));
    }
    if let Some(t) = trunc {
        warns.push(format!("{} limit", format_size(t.get("maxBytes").and_then(Value::as_u64).unwrap_or(DEFAULT_MAX_BYTES))));
    }
    out.push(Line::styled(format!("[Truncated: {}]", warns.join(", ")), theme.warning_style()));
}

/// `shortenPath` (render-utils.ts:10-17): replace a leading `$HOME` with `~`.
pub(super) fn shorten_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && !home.is_empty()
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{rest}");
    }
    path.to_string()
}
