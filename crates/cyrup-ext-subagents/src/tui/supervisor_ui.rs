//! SUBA-136 — the supervisor request card and the reply journal entry: pi
//! `src/intercom/supervisor-ui.ts` @v0.71.0 (new in `v0.57.0..v0.67.0`).
//!
//! A child's `need_decision`/`interview_request` is injected into the parent's transcript as a
//! `subagent_supervisor_request` custom message ([`SUPERVISOR_REQUEST_MESSAGE_TYPE`]), and every
//! `subagent_supervisor({ action: "reply" })` journals a `subagent_supervisor_reply` custom entry
//! ([`SUPERVISOR_REPLY_ENTRY_TYPE`]) so the session keeps a record of what the parent answered.
//! Both are drawn as the same bordered card: bounded fields, a sanitized body, and at most
//! [`MAX_RENDER_LINES`] interior rows while collapsed.
//!
//! The payloads are read the way upstream's `requestDetails`/`replyData` read them: a payload whose
//! typed fields do not hold their declared types is not drawn at all (`None`), and the host falls
//! back to its own framing — never a half-drawn card.

use std::sync::Arc;

use ratatui::text::{Line, Span};
use serde_json::Value;

use crate::tui::fleet_theme::{str_width, truncate_to_width, wrap_line};
use crate::tui::fleet_transcript::safe_display_text;

pub use crate::native_supervisor::SUPERVISOR_REQUEST_MESSAGE_TYPE;

/// pi `SUPERVISOR_REPLY_ENTRY_TYPE` (`supervisor-ui.ts:6`).
pub const SUPERVISOR_REPLY_ENTRY_TYPE: &str = "subagent_supervisor_reply";

/// pi `MAX_FIELD_CHARS` (`:44`) — one header field.
const MAX_FIELD_CHARS: usize = 512;
/// pi `MAX_BODY_CHARS` (`:45`) — the request body, the reply text and the reply hint.
const MAX_BODY_CHARS: usize = 8_000;
/// pi `MAX_INTERVIEW_CHARS` (`:46`) — the serialized interview shape.
const MAX_INTERVIEW_CHARS: usize = 4_000;
/// pi `MAX_RENDER_LINES` (`:47`) — interior rows drawn while collapsed.
const MAX_RENDER_LINES: usize = 36;
/// pi `TRUNCATION_MARKER` (`:48`).
const TRUNCATION_MARKER: &str = "[truncated]";

/// pi `supervisorReplyHint(requestId)` (`:50-52`) — the call that answers a request.
#[must_use]
pub fn supervisor_reply_hint(request_id: &str) -> String {
    format!(
        "{}({{ action: \"reply\", replyTo: \"{request_id}\", message: \"...\" }})",
        crate::native_supervisor::NATIVE_SUPERVISOR_TOOL_NAME
    )
}

/// JavaScript's `string.length`: UTF-16 code units, which is what upstream's bounds count.
fn js_length(text: &str) -> usize {
    text.chars().map(char::len_utf16).sum()
}

/// pi `boundedText` (`:54-64`): sanitize, then cut to `max_chars` UTF-16 units — a whole code
/// point at a time — and mark the cut.
fn bounded_text(value: &str, max_chars: usize) -> String {
    let safe = safe_display_text(value);
    if js_length(&safe) <= max_chars {
        return safe;
    }
    let prefix_length = max_chars.saturating_sub(js_length(TRUNCATION_MARKER) + 1);
    let mut prefix = String::new();
    let mut length = 0usize;
    for character in safe.chars() {
        if length + character.len_utf16() > prefix_length {
            break;
        }
        prefix.push(character);
        length += character.len_utf16();
    }
    format!("{prefix} {TRUNCATION_MARKER}")
}

/// pi `displayText` (`:66-68`): expanded shows everything, sanitized; collapsed is bounded.
fn display_text(value: &str, max_chars: usize, expanded: bool) -> String {
    if expanded {
        safe_display_text(value)
    } else {
        bounded_text(value, max_chars)
    }
}

/// pi `String(value)` for the scalar JSON values a header field holds.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// pi `boundedField` (`:70-72`): an absent field reads `unknown`.
fn bounded_field(value: Option<&Value>) -> String {
    bounded_text(
        &value.map_or_else(|| "unknown".to_string(), js_string),
        MAX_FIELD_CHARS,
    )
}

/// pi `contentText` (`:74-86`): a string, or the joined `text` of a content-part array.
fn content_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// pi `interviewText` (`:88-96`): the interview shape as two-space-indented JSON.
fn interview_text(interview: &Value, expanded: bool) -> String {
    let serialized =
        serde_json::to_string_pretty(interview).unwrap_or_else(|_| "[unavailable]".to_string());
    display_text(&serialized, MAX_INTERVIEW_CHARS, expanded)
}

/// pi `isSupervisorReason` (`:102-104`).
fn is_supervisor_reason(value: &Value) -> bool {
    matches!(
        value.as_str(),
        Some("need_decision" | "interview_request" | "progress_update")
    )
}

/// pi `optionalString` (`:106-108`).
fn optional_string(record: &serde_json::Map<String, Value>, key: &str) -> bool {
    record.get(key).is_none_or(Value::is_string)
}

/// A finite JSON number. serde_json admits no NaN or infinity, so every number is finite.
fn finite_number(value: Option<&Value>) -> bool {
    value.is_some_and(Value::is_number)
}

/// pi `requestDetails` (`:110-117`).
fn request_details(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    let record = value?.as_object()?;
    let strings_ok = [
        "id",
        "requestId",
        "replyHint",
        "requestBody",
        "runId",
        "agent",
        "childTarget",
    ]
    .iter()
    .all(|key| optional_string(record, key));
    let valid = strings_ok
        && record.get("reason").is_none_or(is_supervisor_reason)
        && record.get("expectsReply").is_none_or(Value::is_boolean)
        && record
            .get("childIndex")
            .is_none_or(|index| finite_number(Some(index)));
    valid.then_some(record)
}

/// pi `replyData` (`:119-137`): every required field present with its declared type.
fn reply_data(value: Option<&Value>) -> Option<&serde_json::Map<String, Value>> {
    let record = value?.as_object()?;
    let valid = ["requestId", "runId", "agent", "message"]
        .iter()
        .all(|key| record.get(*key).is_some_and(Value::is_string))
        && record.get("reason").is_none_or(is_supervisor_reason)
        && record.get("childTarget").is_none_or(Value::is_string)
        && finite_number(record.get("childIndex"))
        && finite_number(record.get("createdAt"));
    valid.then_some(record)
}

/// pi `requestHeading` (`:143-147`).
fn request_heading(reason: Option<&str>) -> &'static str {
    match reason {
        Some("interview_request") => "⚠ Supervisor interview request",
        Some("progress_update") => "ℹ Supervisor progress update",
        _ => "⚠ Supervisor decision request",
    }
}

/// A non-empty string field — upstream's truthiness test on an optional string.
fn truthy_str<'a>(record: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    record
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// pi `requestLines` (`:149-162`).
fn request_lines(
    content: Option<&Value>,
    details: &serde_json::Map<String, Value>,
    expanded: bool,
) -> Vec<String> {
    // `withRequestId` (`:139-141`): `requestId ?? id`.
    let request_id = bounded_field(details.get("requestId").or_else(|| details.get("id")));
    let mut lines = vec![
        format!("Reason: {}", bounded_field(details.get("reason"))),
        format!("Run: {}", bounded_field(details.get("runId"))),
        format!("Agent: {}", bounded_field(details.get("agent"))),
        format!("Child index: {}", bounded_field(details.get("childIndex"))),
    ];
    if let Some(target) = truthy_str(details, "childTarget") {
        lines.push(format!(
            "Child target: {}",
            bounded_field(Some(&Value::from(target)))
        ));
    }
    lines.push(format!("Request ID: {request_id}"));
    if details.get("expectsReply").and_then(Value::as_bool) == Some(true) {
        let hint = details
            .get("replyHint")
            .and_then(Value::as_str)
            .map_or_else(|| supervisor_reply_hint(&request_id), str::to_string);
        lines.push(format!(
            "Reply with: {}",
            display_text(&hint, MAX_BODY_CHARS, expanded)
        ));
    }
    let body = details
        .get("requestBody")
        .and_then(Value::as_str)
        .map_or_else(|| content_text(content), str::to_string);
    let body = if body.is_empty() {
        "(no request body)".to_string()
    } else {
        body
    };
    lines.extend([
        String::new(),
        "Request:".to_string(),
        display_text(&body, MAX_BODY_CHARS, expanded),
    ]);
    if let Some(interview) = details.get("interview") {
        lines.extend([
            String::new(),
            "Interview shape:".to_string(),
            interview_text(interview, expanded),
        ]);
    }
    lines
}

/// pi `replyLines` (`:164-176`).
fn reply_lines(data: &serde_json::Map<String, Value>, expanded: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(reason) = truthy_str(data, "reason") {
        lines.push(format!(
            "Reason: {}",
            bounded_field(Some(&Value::from(reason)))
        ));
    }
    lines.extend([
        format!("Run: {}", bounded_field(data.get("runId"))),
        format!("Agent: {}", bounded_field(data.get("agent"))),
        format!("Child index: {}", bounded_field(data.get("childIndex"))),
    ]);
    if let Some(target) = truthy_str(data, "childTarget") {
        lines.push(format!(
            "Child target: {}",
            bounded_field(Some(&Value::from(target)))
        ));
    }
    let message = truthy_str(data, "message").unwrap_or("(empty reply)");
    lines.extend([
        format!("Reply to: {}", bounded_field(data.get("requestId"))),
        String::new(),
        "Reply:".to_string(),
        display_text(message, MAX_BODY_CHARS, expanded),
    ]);
    lines
}

/// The plain text of a single-span-per-piece [`Line`].
fn line_text(line: &Line<'static>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// One text as a single-span [`Line`]. NOT `Line::raw`, which splits on newlines into adjacent
/// spans and so silently joins the pieces.
fn plain_line(text: &str) -> Line<'static> {
    Line::from(vec![Span::raw(text.to_string())])
}

/// pi `truncateToWidth(text, width, "")`.
fn clip(text: &str, width: usize) -> String {
    line_text(&truncate_to_width(&plain_line(text), width, ""))
}

/// pi `renderCard` (`:178-208`): a rounded accent border around the wrapped lines, the heading
/// in the top rule, and — collapsed — at most [`MAX_RENDER_LINES`] interior rows followed by a
/// [`TRUNCATION_MARKER`] row.
fn render_card(
    lines: &[String],
    heading: &str,
    theme: &dyn cyrup_ext::RenderTheme,
    width: usize,
    expanded: bool,
) -> Vec<String> {
    if width < 3 {
        return vec![clip(heading, width)];
    }
    let body_width = width - 2;
    let header_text = clip(&format!(" {heading} "), body_width);
    let header_padding = body_width.saturating_sub(str_width(&header_text));
    let border = |text: &str| theme.fg("accent", text);
    let row = |text: &str| {
        let text = clip(text, body_width);
        let padding = body_width.saturating_sub(str_width(&text));
        border(&format!("│{text}{}│", " ".repeat(padding)))
    };
    let mut rendered = vec![border(&format!(
        "╭{header_text}{}╮",
        "─".repeat(header_padding)
    ))];
    let mut hidden = false;
    let mut interior_lines = 0usize;
    'lines: for line in lines {
        for wrapped in wrap_line(&plain_line(line), body_width.max(1)) {
            if !expanded && interior_lines >= MAX_RENDER_LINES {
                hidden = true;
                break 'lines;
            }
            rendered.push(row(&line_text(&wrapped)));
            interior_lines += 1;
        }
    }
    if hidden {
        rendered.push(row(TRUNCATION_MARKER));
    }
    rendered.push(border(&format!("╰{}╯", "─".repeat(body_width))));
    rendered
}

/// What a card draws. The lines are rebuilt per frame because collapsed and expanded bodies are
/// bounded differently (`displayText`), and the expand flag is live.
#[derive(Debug)]
enum Card {
    Request {
        content: Option<Value>,
        details: serde_json::Map<String, Value>,
    },
    Reply(serde_json::Map<String, Value>),
}

/// pi `SupervisorCardComponent` (`:210-229`).
#[derive(Debug)]
pub struct SupervisorCardComponent {
    card: Card,
}

impl cyrup_ext::RenderedComponent for SupervisorCardComponent {
    fn render(&self, ctx: &cyrup_ext::RenderCtx<'_>) -> Vec<String> {
        match &self.card {
            Card::Request { content, details } => render_card(
                &request_lines(content.as_ref(), details, ctx.expanded),
                request_heading(details.get("reason").and_then(Value::as_str)),
                ctx.theme,
                ctx.width,
                ctx.expanded,
            ),
            Card::Reply(data) => render_card(
                &reply_lines(data, ctx.expanded),
                "↩ Supervisor reply to child",
                ctx.theme,
                ctx.width,
                ctx.expanded,
            ),
        }
    }
}

/// pi `renderSupervisorRequest` (`:231-239`) over the serialized custom message: `details` is the
/// structured request, `payload` (live) or `content` (persisted) its injected text.
#[must_use]
pub fn render_supervisor_request(message: &Value) -> Option<Arc<dyn cyrup_ext::RenderedComponent>> {
    let details = request_details(message.get("details"))?.clone();
    let content = message
        .get("payload")
        .or_else(|| message.get("content"))
        .cloned();
    Some(Arc::new(SupervisorCardComponent {
        card: Card::Request { content, details },
    }))
}

/// pi `renderSupervisorReply` (`:241-249`) over the serialized custom entry, whose payload is
/// `data`.
#[must_use]
pub fn render_supervisor_reply(entry: &Value) -> Option<Arc<dyn cyrup_ext::RenderedComponent>> {
    let data = reply_data(entry.get("data"))?.clone();
    Some(Arc::new(SupervisorCardComponent {
        card: Card::Reply(data),
    }))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
mod tests {
    use super::*;

    struct PlainTheme;

    impl cyrup_ext::RenderTheme for PlainTheme {
        fn fg(&self, _role: &str, text: &str) -> String {
            text.to_string()
        }
        fn bold(&self, text: &str) -> String {
            text.to_string()
        }
    }

    fn draw(
        component: &Arc<dyn cyrup_ext::RenderedComponent>,
        width: usize,
        expanded: bool,
    ) -> Vec<String> {
        component.render(&cyrup_ext::RenderCtx {
            width,
            expanded,
            theme: &PlainTheme,
        })
    }

    #[test]
    fn bounded_text_cuts_whole_code_points_and_marks_the_cut() {
        assert_eq!(bounded_text("short", 10), "short");
        // 20 chars into 16: 16 - "[truncated]".len() - 1 = 4 kept.
        assert_eq!(bounded_text("abcdefghijklmnopqrst", 16), "abcd [truncated]");
        // An astral char is two UTF-16 units, as upstream counts it.
        assert_eq!(bounded_text("😀😀😀😀😀😀😀😀😀", 16), "😀😀 [truncated]");
        // Sanitized before measuring: an ESC is escaped, never passed to the terminal.
        assert_eq!(bounded_text("a\u{1b}b", 64), "a[U+001B]b");
    }

    #[test]
    fn a_malformed_payload_is_not_drawn() {
        assert!(
            render_supervisor_request(&serde_json::json!({ "details": { "reason": "bogus" } }))
                .is_none()
        );
        assert!(
            render_supervisor_request(&serde_json::json!({ "details": { "childIndex": "2" } }))
                .is_none()
        );
        assert!(render_supervisor_request(&serde_json::json!({ "payload": "x" })).is_none());
        assert!(
            render_supervisor_reply(&serde_json::json!({ "data": { "requestId": "r" } })).is_none()
        );
    }

    #[test]
    fn the_card_is_bordered_at_the_live_width() {
        let component = render_supervisor_request(&serde_json::json!({
            "payload": "ignored when requestBody is set",
            "details": {
                "id": "req-1", "requestId": "req-1", "reason": "interview_request",
                "expectsReply": true, "runId": "run-1", "agent": "worker", "childIndex": 2,
                "requestBody": "Pick one.", "interview": { "choice": "a|b" },
            },
        }))
        .expect("drawable");
        let lines = draw(&component, 40, false);
        assert!(
            lines[0].starts_with("╭ ⚠ Supervisor interview request "),
            "{lines:?}"
        );
        assert!(lines.last().unwrap().starts_with("╰"), "{lines:?}");
        for line in &lines {
            assert_eq!(str_width(line), 40, "every row fills the width: {line:?}");
        }
        let body = lines.join("\n");
        assert!(body.contains("│Reason: interview_request"), "{body}");
        assert!(body.contains("│Request ID: req-1"), "{body}");
        assert!(body.contains("│Pick one."), "{body}");
        assert!(body.contains("│Interview shape:"), "{body}");
        assert!(!body.contains("ignored"), "{body}");

        // A width under three draws the heading alone, clipped.
        assert_eq!(draw(&component, 2, false), vec!["⚠ ".to_string()]);
    }

    #[test]
    fn a_collapsed_card_stops_at_the_row_limit_and_expanded_shows_everything() {
        let body = (0..60)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let component = render_supervisor_request(&serde_json::json!({
            "details": { "requestId": "req-2", "reason": "need_decision", "requestBody": body },
        }))
        .expect("drawable");
        let collapsed = draw(&component, 80, false);
        // Top rule + 36 interior rows + the marker row + bottom rule.
        assert_eq!(collapsed.len(), MAX_RENDER_LINES + 3, "{collapsed:?}");
        assert!(collapsed[collapsed.len() - 2].starts_with("│[truncated]"));
        let expanded = draw(&component, 80, true);
        assert!(
            expanded.iter().any(|line| line.starts_with("│line 59")),
            "{expanded:?}"
        );
        assert!(!expanded.iter().any(|line| line.starts_with("│[truncated]")));
    }
}
