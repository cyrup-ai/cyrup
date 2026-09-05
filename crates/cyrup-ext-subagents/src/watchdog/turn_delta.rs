//! The turn-delta formatter — a 1:1 port of `pi-subagents/src/watchdog/turn-delta.ts` (161 lines
//! @v0.43.0).
//!
//! This is what the review model actually reads: the parent turn, flattened into plain text. Like
//! [`super::emission_guard`] it is **ordering-sensitive**, in two distinct ways that this port
//! preserves:
//!
//! * **Section order** (`formatWatchdogTurnDelta`, `:148-160`) is user prompt, then every entry of
//!   `messages` in order, then every message each entry of `events` expands to in order, then the
//!   `finalAssistantStop` sentinel — joined by `\n\n---\n\n`. Empty sections are dropped, not
//!   emitted blank, so a delta with no renderable content is the empty string and the runtime's
//!   `!delta.trim()` guards fire. The runtime feeds one event at a time (`runtime.ts:330-334`) and
//!   concatenates the results itself, so a reordering here would silently reorder the model's view
//!   of the turn.
//! * **Dispatch order inside `formatWatchdogReviewMessage`** (`:126-146`): the watchdog's own
//!   warning message is filtered out FIRST (`:129`) — before the `role` dispatch — which is what
//!   stops a review from reviewing its own previous warnings and looping. `messagesFromEvent`
//!   (`:104-124`) is likewise ordered: `turn_end` before the generic `input.message` arm, so a
//!   `turn_end` carrying both a message and tool results expands to all of them rather than just
//!   the message.
//!
//! Upstream types everything as `unknown` and duck-types with `typeof`/`in`; this port takes
//! [`serde_json::Value`] and reproduces those checks exactly. Two JS/JSON differences are handled
//! explicitly:
//!
//! * `??` is *nullish* coalescing, so an absent key and an explicit JSON `null` are the same input.
//!   [`nullish`] is that operator.
//! * `String.prototype.length` counts UTF-16 code units, which is what the redaction marker reports
//!   (`[omitted N chars; ...]`); [`redact_edit_write_input`] uses `encode_utf16().count()` so a
//!   non-BMP character produces the same N here as upstream.

use serde_json::Value;

use crate::watchdog::types::SUBAGENT_WATCHDOG_WARNING_TYPE;

/// `WatchdogTurnDeltaInput` (`turn-delta.ts:18-24`).
#[derive(Debug, Clone, Default)]
pub struct WatchdogTurnDeltaInput<'a> {
    /// The real user prompt that opened this turn.
    pub user_prompt: Option<&'a str>,
    /// Whether to prepend that prompt (the runtime sets it for the FIRST delta of a turn only).
    pub include_user_prompt: bool,
    /// Already-shaped messages.
    pub messages: &'a [Value],
    /// Raw harness events, expanded through [`messages_from_event`].
    pub events: &'a [Value],
    /// Append the "stop without tool call" sentinel.
    pub final_assistant_stop: bool,
}

/// The `??` nullish-coalescing operand test: `None` for an absent key *or* an explicit JSON `null`.
fn nullish(value: Option<&Value>) -> Option<&Value> {
    match value {
        None | Some(Value::Null) => None,
        other => other,
    }
}

/// `input.foo ?? input.bar` over an object, returning the first non-nullish of `keys`.
fn first_present<'a>(
    object: &'a serde_json::Map<String, Value>,
    keys: &[&str],
) -> Option<&'a Value> {
    keys.iter().find_map(|key| nullish(object.get(*key)))
}

/// JS truthiness for a JSON value: `false`, `0`, `""`, `null` and absent are falsy; everything else
/// (including `[]` and `{}`, which are truthy objects in JS) is truthy.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Number(n)) => n.as_f64().is_none_or(|f| f != 0.0),
        Some(_) => true,
    }
}

/// `JSON.stringify` of a scalar, as `formatValue` (`turn-delta.ts:45-47`) uses it.
fn json_scalar(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}

/// `formatValue` (`turn-delta.ts:44-61`): the recursive plain-text renderer for tool arguments and
/// unstructured content.
///
/// A string renders verbatim; `null`/number/boolean render as `JSON.stringify`; an array renders as
/// one `- `-prefixed line per item at the current indent with children indented two further spaces;
/// an object renders `key: value` per line, EXCEPT that a multi-line string value moves to its own
/// block (`key:\n<value>`) so a diff or a file body is not squashed onto the key's line.
///
/// [CYRUP-DELTA] upstream's `if (value === undefined) return "undefined"` arm is unreachable from a
/// JSON-sourced value (JSON has no `undefined`); a JSON `null` takes the `JSON.stringify` arm and
/// renders `null`, exactly as it does upstream.
#[must_use]
pub fn format_value(value: &Value, indent: &str) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null | Value::Number(_) | Value::Bool(_) => json_scalar(value),
        Value::Array(items) => {
            let child_indent = format!("{indent}  ");
            items
                .iter()
                .map(|item| format!("{indent}- {}", format_value(item, &child_indent)))
                .collect::<Vec<_>>()
                .join("\n")
        }
        Value::Object(map) => {
            let child_indent = format!("{indent}  ");
            let mut lines: Vec<String> = Vec::with_capacity(map.len());
            for (key, item) in map {
                match item {
                    Value::String(s) if s.contains('\n') => {
                        lines.push(format!("{indent}{key}:\n{s}"))
                    }
                    _ => lines.push(format!(
                        "{indent}{key}: {}",
                        format_value(item, &child_indent)
                    )),
                }
            }
            lines.join("\n")
        }
    }
}

/// `redactEditWriteInput` (`turn-delta.ts:63-74`): replace the three bulk-text fields of an
/// `edit`/`write` tool call with a length marker, recursively, so the delta carries the *shape* of
/// the edit without duplicating the file body the tool result's diff already shows.
#[must_use]
pub fn redact_edit_write_input(input: &Value) -> Value {
    match input {
        Value::Array(items) => Value::Array(items.iter().map(redact_edit_write_input).collect()),
        Value::Object(map) => {
            let mut sanitized = serde_json::Map::with_capacity(map.len());
            for (key, value) in map {
                let redacted = match value {
                    Value::String(s)
                        if matches!(key.as_str(), "oldText" | "newText" | "content") =>
                    {
                        Value::String(format!(
                            "[omitted {} chars; use tool result diff]",
                            s.encode_utf16().count()
                        ))
                    }
                    other => redact_edit_write_input(other),
                };
                sanitized.insert(key.clone(), redacted);
            }
            Value::Object(sanitized)
        }
        other => other.clone(),
    }
}

/// `formatToolArguments` (`turn-delta.ts:76-79`): only `edit` and `write` are redacted.
fn format_tool_arguments(name: &str, input: &Value) -> String {
    if name == "edit" || name == "write" {
        return format_value(&redact_edit_write_input(input), "");
    }
    format_value(input, "")
}

/// `formatToolCall` (`turn-delta.ts:81-84`). A missing or non-string name renders as `tool`; a
/// nullish input renders as the empty object (which `format_value` renders as the empty string).
#[must_use]
pub fn format_tool_call(name: Option<&Value>, input: Option<&Value>) -> String {
    let tool_name = match nullish(name) {
        Some(Value::String(s)) if !s.is_empty() => s.as_str(),
        _ => "tool",
    };
    let empty = Value::Object(serde_json::Map::new());
    let input = nullish(input).unwrap_or(&empty);
    format!(
        "Tool call: {tool_name}\nArguments:\n{}",
        format_tool_arguments(tool_name, input)
    )
}

/// `formatToolResult` (`turn-delta.ts:86-97`).
///
/// A failure prints an `Error:` line and then labels the body `Output:`; a success with a string
/// `details.diff` prints the diff INSTEAD of the content (that `else` is exclusive upstream — a
/// successful diff-carrying result never also prints its content), and any other success labels the
/// body `Result:`.
#[must_use]
pub fn format_tool_result(
    name: &str,
    content: Option<&Value>,
    details: Option<&Value>,
    error: Option<&Value>,
    is_error: Option<&Value>,
) -> String {
    let failed = matches!(nullish(is_error), Some(Value::Bool(true))) || truthy(error);
    let mut lines = vec![format!("Tool result: {name}")];
    if failed {
        // `error ? (typeof error === "string" ? error : formatValue(error)) : "tool reported an
        // error"` (`turn-delta.ts:88`). The gate is JS TRUTHINESS, not nullish: `failed` can be
        // reached via `isError === true` while `error` is a FALSY-but-present value (`""`, `0`,
        // `false`), and upstream falls back to the generic text for all three. Reading this with
        // `??` instead emits a bare `Error: ` for `{ isError: true, error: "" }`.
        let detail = match error.filter(|value| truthy(Some(value))) {
            Some(Value::String(s)) => s.clone(),
            Some(other) => format_value(other, ""),
            None => "tool reported an error".to_string(),
        };
        lines.push(format!("Error: {detail}"));
    }
    let diff = if failed {
        None
    } else {
        details
            .and_then(Value::as_object)
            .and_then(|d| d.get("diff"))
            .and_then(Value::as_str)
    };
    if let Some(diff) = diff {
        lines.push("Diff:".to_string());
        lines.push(diff.to_string());
    } else {
        let body = text_from_content(content);
        if !body.is_empty() {
            lines.push(if failed {
                "Output:".to_string()
            } else {
                "Result:".to_string()
            });
            lines.push(body);
        }
    }
    lines.join("\n")
}

/// `textFromContent` (`turn-delta.ts:27-42`): flatten a message `content` field.
///
/// The array arm recognizes pi's four content-block shapes in upstream's order — `text`,
/// `thinking` (prefixed with its own `Thinking:` line), `toolCall` (rendered through
/// [`format_tool_call`]), then any block with a string `content` — and drops everything else to the
/// empty string, which the subsequent `.filter(Boolean)` removes.
#[must_use]
pub fn text_from_content(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|block| match block {
                Value::String(s) => s.clone(),
                Value::Object(item) => {
                    let block_type = item.get("type").and_then(Value::as_str);
                    if block_type == Some("text")
                        && let Some(text) = item.get("text").and_then(Value::as_str)
                    {
                        return text.to_string();
                    }
                    if block_type == Some("thinking")
                        && let Some(thinking) = item.get("thinking").and_then(Value::as_str)
                    {
                        return format!("Thinking:\n{thinking}");
                    }
                    if block_type == Some("toolCall") {
                        return format_tool_call(
                            item.get("name"),
                            first_present(item, &["input", "args", "arguments"]),
                        );
                    }
                    if let Some(inner) = item.get("content").and_then(Value::as_str) {
                        return inner.to_string();
                    }
                    String::new()
                }
                _ => String::new(),
            })
            .filter(|section| !section.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        None | Some(Value::Null) => String::new(),
        Some(other) => format_value(other, ""),
    }
}

/// A synthesized `{ role, ... }` message object, as `messagesFromEvent` builds them.
fn message(fields: Vec<(&str, Value)>) -> Value {
    let mut map = serde_json::Map::with_capacity(fields.len());
    for (key, value) in fields {
        map.insert(key.to_string(), value);
    }
    Value::Object(map)
}

/// `input.type === "x" || input.event === "x"` — upstream accepts either discriminator key so the
/// same formatter serves both the harness event stream and a recorded event log.
fn is_event(object: &serde_json::Map<String, Value>, name: &str) -> bool {
    object.get("type").and_then(Value::as_str) == Some(name)
        || object.get("event").and_then(Value::as_str) == Some(name)
}

/// `messagesFromEvent` (`turn-delta.ts:104-124`) — expand one harness event into the messages it
/// contributes, in upstream's arm order.
#[must_use]
pub fn messages_from_event(event: &Value) -> Vec<Value> {
    let Some(input) = event.as_object() else {
        return Vec::new();
    };
    // `turn_end` FIRST: it also carries `message`, and the generic `input.message` arm below would
    // otherwise swallow the tool results.
    if is_event(input, "turn_end") {
        let mut out = Vec::new();
        if let Some(msg) = nullish(input.get("message")) {
            out.push(msg.clone());
        }
        if let Some(results) = input.get("toolResults").and_then(Value::as_array) {
            out.extend(results.iter().filter(|v| truthy(Some(v))).cloned());
        }
        return out;
    }
    if let Some(msg) = input.get("message")
        && truthy(Some(msg))
    {
        return vec![msg.clone()];
    }
    if is_event(input, "tool_execution_start") {
        return vec![message(vec![
            ("role", Value::String("toolCall".into())),
            (
                "name",
                first_present(input, &["toolName", "name"])
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "input",
                first_present(input, &["args", "input"])
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
        ])];
    }
    if is_event(input, "tool_execution_end") {
        // `input.result` counts as the nested result record only when it is a non-array object.
        let result = input.get("result").and_then(|v| match v {
            Value::Object(map) => Some(map),
            _ => None,
        });
        let content = result
            .and_then(|r| nullish(r.get("content")))
            .or_else(|| nullish(input.get("content")))
            .or_else(|| nullish(input.get("result")))
            .cloned()
            .unwrap_or(Value::Null);
        let details = result
            .and_then(|r| nullish(r.get("details")))
            .or_else(|| nullish(input.get("details")))
            .cloned()
            .unwrap_or(Value::Null);
        return vec![message(vec![
            ("role", Value::String("toolResult".into())),
            (
                "toolName",
                first_present(input, &["toolName", "name"])
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            ("content", content),
            ("details", details),
            ("error", input.get("error").cloned().unwrap_or(Value::Null)),
            (
                "isError",
                input.get("isError").cloned().unwrap_or(Value::Null),
            ),
        ])];
    }
    if is_event(input, "tool_result") {
        return vec![message(vec![
            ("role", Value::String("toolResult".into())),
            (
                "toolName",
                first_present(input, &["toolName", "name"])
                    .cloned()
                    .unwrap_or(Value::Null),
            ),
            (
                "content",
                input.get("content").cloned().unwrap_or(Value::Null),
            ),
            (
                "details",
                input.get("details").cloned().unwrap_or(Value::Null),
            ),
            ("error", input.get("error").cloned().unwrap_or(Value::Null)),
            (
                "isError",
                input.get("isError").cloned().unwrap_or(Value::Null),
            ),
        ])];
    }
    Vec::new()
}

/// `formatWatchdogReviewMessage` (`turn-delta.ts:126-146`) — one message's section, or `None` when
/// it contributes nothing.
///
/// The watchdog's own warning message is dropped first (`role: "custom"` +
/// [`SUBAGENT_WATCHDOG_WARNING_TYPE`]); without that arm every review would re-read its own prior
/// warnings and re-emit them, which the emission guard would then have to absorb.
#[must_use]
pub fn format_watchdog_review_message(message: &Value) -> Option<String> {
    let input = message.as_object()?;
    let role = input.get("role").and_then(Value::as_str);
    if role == Some("custom")
        && input.get("customType").and_then(Value::as_str) == Some(SUBAGENT_WATCHDOG_WARNING_TYPE)
    {
        return None;
    }
    match role {
        Some("assistant") => {
            let text = text_from_content(input.get("content"));
            let mut lines = if text.is_empty() {
                vec!["Assistant: (no text)".to_string()]
            } else {
                vec!["Assistant:".to_string(), text]
            };
            if input.get("stopReason").and_then(Value::as_str) == Some("stop") {
                lines.push("Assistant stop: stop".to_string());
            }
            Some(lines.join("\n"))
        }
        Some("toolCall") => Some(format_tool_call(
            first_present(input, &["name", "toolName"]),
            first_present(input, &["input", "args", "arguments"]),
        )),
        Some("toolResult" | "tool") => {
            let name = first_present(input, &["toolName", "name"])
                .and_then(Value::as_str)
                .unwrap_or("tool");
            Some(format_tool_result(
                name,
                input.get("content"),
                input.get("details"),
                input.get("error"),
                input.get("isError"),
            ))
        }
        Some("user") => {
            let text = text_from_content(input.get("content"));
            if text.is_empty() {
                None
            } else {
                Some(format!("User:\n{text}"))
            }
        }
        _ => None,
    }
}

/// The `{type:"turn_end", message, toolResults}` object the two watchdog roles hand
/// [`super::runtime::MainWatchdogRuntime::handle_turn_end`] — the same JSON pi's own `turn_end`
/// handlers receive (`register-main.ts:419-422`, `register-child.ts:98-101`).
///
/// **The `role` on each tool result is load-bearing.** `messagesFromEvent` expands a `turn_end`
/// into `[message, ...toolResults]` (`turn-delta.ts:106-108`) and hands each entry straight to
/// [`format_watchdog_review_message`], which dispatches on `role` ALONE and returns `undefined` for
/// anything else (`:146`). pi's `ToolResultMessage` declares `role: "toolResult"` as its first
/// field (`pi/packages/ai/src/types.ts:437-438`), so upstream's results land in the
/// `"toolResult" | "tool"` arm. cyrup's [`cyrup_agent::ToolResultMessage`] is the *payload* struct —
/// the discriminant lives on the [`cyrup_agent::AgentMessage::ToolResult`] wrapper (`event.rs:56-73`)
/// and a bare struct serializes without it — so serializing the `Vec<ToolResultMessage>` directly
/// produced role-less objects that `format_watchdog_review_message` silently dropped. Every tool
/// result of every turn was therefore invisible to the review: the model saw the assistant text and
/// never the diffs, errors or outputs the warnings are supposed to be evidence from.
///
/// `message` needs no such fixup — [`cyrup_agent::AgentMessage`] self-tags (`event.rs:32-74`).
#[must_use]
pub fn watchdog_turn_end_event(
    message: &cyrup_agent::AgentMessage,
    tool_results: &[cyrup_agent::ToolResultMessage],
) -> Value {
    let tool_results: Vec<Value> = tool_results
        .iter()
        .map(|result| {
            let mut value = serde_json::to_value(result).unwrap_or(Value::Null);
            if let Some(object) = value.as_object_mut() {
                object.insert("role".to_string(), Value::String("toolResult".to_string()));
            }
            value
        })
        .collect();
    serde_json::json!({
        "type": "turn_end",
        "message": message,
        "toolResults": tool_results,
    })
}

/// The section separator (`turn-delta.ts:160`), also the runtime's own delta separator.
pub const WATCHDOG_DELTA_SECTION_SEPARATOR: &str = "\n\n---\n\n";

/// `formatWatchdogTurnDelta` (`turn-delta.ts:148-160`) — the whole turn as review input, sections in
/// the fixed order documented at the top of this module.
#[must_use]
pub fn format_watchdog_turn_delta(input: &WatchdogTurnDeltaInput<'_>) -> String {
    let mut sections: Vec<String> = Vec::new();
    if input.include_user_prompt
        && let Some(prompt) = input.user_prompt
        && !prompt.trim().is_empty()
    {
        sections.push(format!("User prompt:\n{prompt}"));
    }
    for message in input.messages {
        if let Some(section) = format_watchdog_review_message(message) {
            sections.push(section);
        }
    }
    for event in input.events {
        for message in messages_from_event(event) {
            if let Some(section) = format_watchdog_review_message(&message) {
                sections.push(section);
            }
        }
    }
    if input.final_assistant_stop {
        sections.push("Final assistant stop: stop without tool call".to_string());
    }
    sections.join(WATCHDOG_DELTA_SECTION_SEPARATOR)
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
    use serde_json::json;

    #[test]
    fn section_order_is_prompt_then_messages_then_events_then_stop() {
        let messages = vec![json!({ "role": "assistant", "content": "from messages" })];
        let events = vec![json!({ "type": "tool_result", "toolName": "read", "content": "body" })];
        let delta = format_watchdog_turn_delta(&WatchdogTurnDeltaInput {
            user_prompt: Some("do the thing"),
            include_user_prompt: true,
            messages: &messages,
            events: &events,
            final_assistant_stop: true,
        });
        assert_eq!(
            delta,
            [
                "User prompt:\ndo the thing",
                "Assistant:\nfrom messages",
                "Tool result: read\nResult:\nbody",
                "Final assistant stop: stop without tool call",
            ]
            .join(WATCHDOG_DELTA_SECTION_SEPARATOR)
        );
    }

    #[test]
    fn a_blank_user_prompt_contributes_no_section() {
        let delta = format_watchdog_turn_delta(&WatchdogTurnDeltaInput {
            user_prompt: Some("   \n "),
            include_user_prompt: true,
            ..Default::default()
        });
        assert_eq!(delta, "");
    }

    #[test]
    fn the_watchdogs_own_warning_message_is_filtered_before_the_role_dispatch() {
        let message = json!({
            "role": "custom",
            "customType": SUBAGENT_WATCHDOG_WARNING_TYPE,
            "content": "<subagent_watchdog ...>",
        });
        assert_eq!(format_watchdog_review_message(&message), None);
        // A different customType on role "custom" still contributes nothing (no role arm matches).
        let other = json!({ "role": "custom", "customType": "slash_text_result", "content": "x" });
        assert_eq!(format_watchdog_review_message(&other), None);
    }

    #[test]
    fn turn_end_expands_to_its_message_and_all_tool_results_in_order() {
        let event = json!({
            "type": "turn_end",
            "message": { "role": "assistant", "content": "text", "stopReason": "stop" },
            "toolResults": [
                { "role": "toolResult", "toolName": "read", "content": "one" },
                { "role": "toolResult", "toolName": "grep", "content": "two" },
            ],
        });
        let expanded = messages_from_event(&event);
        assert_eq!(expanded.len(), 3);
        let delta = format_watchdog_turn_delta(&WatchdogTurnDeltaInput {
            events: std::slice::from_ref(&event),
            ..Default::default()
        });
        assert_eq!(
            delta,
            [
                "Assistant:\ntext\nAssistant stop: stop",
                "Tool result: read\nResult:\none",
                "Tool result: grep\nResult:\ntwo",
            ]
            .join(WATCHDOG_DELTA_SECTION_SEPARATOR)
        );
    }

    /// The wiring shape, end to end: a REAL [`cyrup_agent::ToolResultMessage`] (the type both
    /// `HostEvent::TurnEnd` arms carry) must survive `messagesFromEvent` -> the `role` dispatch and
    /// render its `Tool result:` section. Serialized bare it carries no `role` and is discarded.
    #[test]
    fn a_real_host_tool_result_reaches_the_delta_only_because_the_event_stamps_its_role() {
        let result = cyrup_agent::ToolResultMessage {
            tool_call_id: cyrup_core::ToolCallId::from("call-1"),
            tool_name: "read".to_string(),
            content: vec![cyrup_core::Content::Text {
                text: "file body".into(),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: false,
            timestamp: 0,
        };
        let message = cyrup_agent::AgentMessage::Custom {
            kind: "probe".to_string(),
            payload: json!({}),
            details: None,
            display: true,
            timestamp: Some(0),
        };

        // The defect, stated as an assertion: the bare struct has no `role` at all.
        let bare = serde_json::to_value(&result).expect("serializes");
        assert!(bare.get("role").is_none());
        assert_eq!(
            format_watchdog_review_message(&bare),
            None,
            "a role-less tool result is silently dropped by the review dispatch"
        );

        let event = watchdog_turn_end_event(&message, std::slice::from_ref(&result));
        assert_eq!(event["toolResults"][0]["role"], json!("toolResult"));
        assert_eq!(event["toolResults"][0]["toolName"], json!("read"));
        let delta = format_watchdog_turn_delta(&WatchdogTurnDeltaInput {
            events: std::slice::from_ref(&event),
            ..Default::default()
        });
        assert_eq!(delta, "Tool result: read\nResult:\nfile body");
    }

    /// An ERRORED tool result is the one the watchdog most needs to see, and it too only renders
    /// because the event stamps the role.
    #[test]
    fn a_failed_host_tool_result_renders_its_error_section_through_the_event_shape() {
        let result = cyrup_agent::ToolResultMessage {
            tool_call_id: cyrup_core::ToolCallId::from("call-2"),
            tool_name: "bash".to_string(),
            content: vec![cyrup_core::Content::Text {
                text: "command not found".into(),
                text_signature: None,
            }],
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            is_error: true,
            timestamp: 0,
        };
        let message = cyrup_agent::AgentMessage::Custom {
            kind: "probe".to_string(),
            payload: json!({}),
            details: None,
            display: true,
            timestamp: Some(0),
        };
        let event = watchdog_turn_end_event(&message, std::slice::from_ref(&result));
        let delta = format_watchdog_turn_delta(&WatchdogTurnDeltaInput {
            events: std::slice::from_ref(&event),
            ..Default::default()
        });
        assert_eq!(
            delta,
            "Tool result: bash\nError: tool reported an error\nOutput:\ncommand not found"
        );
    }

    #[test]
    fn tool_execution_end_prefers_the_nested_result_record() {
        let event = json!({
            "type": "tool_execution_end",
            "toolName": "edit",
            "result": { "content": "ignored", "details": { "diff": "@@ -1 +1 @@" } },
            "content": "outer",
        });
        let expanded = messages_from_event(&event);
        assert_eq!(expanded.len(), 1);
        assert_eq!(
            format_watchdog_review_message(&expanded[0]).unwrap(),
            "Tool result: edit\nDiff:\n@@ -1 +1 @@"
        );
    }

    #[test]
    fn a_failed_tool_result_prints_error_then_output_and_never_the_diff() {
        let message = json!({
            "role": "toolResult",
            "toolName": "bash",
            "isError": true,
            "error": "exit 1",
            "details": { "diff": "should not appear" },
            "content": "stderr tail",
        });
        assert_eq!(
            format_watchdog_review_message(&message).unwrap(),
            "Tool result: bash\nError: exit 1\nOutput:\nstderr tail"
        );
    }

    #[test]
    fn a_failure_with_no_error_field_still_reports_one() {
        let message = json!({ "role": "tool", "name": "bash", "isError": true });
        assert_eq!(
            format_watchdog_review_message(&message).unwrap(),
            "Tool result: bash\nError: tool reported an error"
        );
    }

    #[test]
    fn edit_and_write_arguments_are_redacted_by_utf16_length() {
        let message = json!({
            "role": "toolCall",
            "name": "write",
            "input": { "path": "a.rs", "content": "hello" },
        });
        // ACP-Q1 — key order here is the JSON object's INSERTION order (`serde_json/preserve_order`,
        // turned on graph-wide by `cyrup-acp`'s `agent-client-protocol` edge; see the workspace
        // `Cargo.toml`). It used to be alphabetical, because `serde_json::Map` was a `BTreeMap`.
        // This is the better of the two for a watchdog review: the fields reach the reviewing model
        // in the order the tool actually declared them, which is what upstream's
        // `Object.entries(...)` yields (`turn-delta.ts`) — so the flip moves this TOWARD pi rather
        // than away from it.
        assert_eq!(
            format_watchdog_review_message(&message).unwrap(),
            "Tool call: write\nArguments:\npath: a.rs\ncontent: [omitted 5 chars; use tool result diff]"
        );
        // A non-BMP character is 2 UTF-16 code units, as `String.prototype.length` reports it.
        let emoji = json!({ "content": "\u{1F600}" });
        assert_eq!(
            redact_edit_write_input(&emoji)["content"],
            json!("[omitted 2 chars; use tool result diff]")
        );
        // A tool that is not edit/write keeps its text.
        let read = json!({ "role": "toolCall", "name": "read", "input": { "content": "kept" } });
        assert!(
            format_watchdog_review_message(&read)
                .unwrap()
                .contains("kept")
        );
    }

    #[test]
    fn content_blocks_render_in_upstream_order_and_drop_unknown_shapes() {
        let message = json!({
            "role": "assistant",
            "content": [
                { "type": "text", "text": "answer" },
                { "type": "thinking", "thinking": "reasoning" },
                { "type": "toolCall", "name": "grep", "args": { "pattern": "x" } },
                { "type": "image", "url": "http://example" },
                { "content": "plain" },
            ],
        });
        assert_eq!(
            format_watchdog_review_message(&message).unwrap(),
            "Assistant:\nanswer\nThinking:\nreasoning\nTool call: grep\nArguments:\npattern: x\nplain"
        );
    }

    #[test]
    fn format_value_moves_multiline_strings_to_their_own_block() {
        let value = json!({ "one": "a\nb", "two": 3, "three": [true, null] });
        // An ARRAY value stays on the key's own line and its items are rendered at the CHILD
        // indent (`turn-delta.ts:56-57`'s `${indent}${key}: ${formatValue(item, indent+"  ")}`), so
        // the first item follows the `key: ` on the same line — only a MULTI-LINE STRING gets its
        // own block. Reproducing that exactly is the point of this test.
        // ACP-Q1 — insertion order, not alphabetical: see the note on
        // `edit_and_write_arguments_are_redacted_by_utf16_length`. `one`/`two`/`three` is the order
        // the `json!` literal declares them and the order `Object.entries` would yield upstream.
        assert_eq!(
            format_value(&value, ""),
            "one:\na\nb\ntwo: 3\nthree:   - true\n  - null"
        );
    }

    #[test]
    fn an_unnamed_tool_call_with_no_input_renders_the_bare_header() {
        assert_eq!(
            format_tool_call(None, None),
            "Tool call: tool\nArguments:\n"
        );
        assert_eq!(
            format_tool_call(Some(&json!("")), Some(&Value::Null)),
            "Tool call: tool\nArguments:\n"
        );
    }

    #[test]
    fn a_user_message_with_no_text_contributes_nothing() {
        assert_eq!(
            format_watchdog_review_message(&json!({ "role": "user", "content": [] })),
            None
        );
        assert_eq!(
            format_watchdog_review_message(&json!({ "role": "user", "content": "hi" })).unwrap(),
            "User:\nhi"
        );
    }

    #[test]
    fn a_non_object_event_or_message_expands_to_nothing() {
        assert!(messages_from_event(&json!("string")).is_empty());
        assert!(messages_from_event(&json!({ "type": "unknown_event" })).is_empty());
        assert_eq!(format_watchdog_review_message(&json!(42)), None);
    }

    /// `turn-delta.ts:88` gates the error detail on JS TRUTHINESS (`error ? … : "tool reported an
    /// error"`), not on nullish. A tool that sets `isError: true` and leaves `error` a falsy —
    /// but present — value reaches `failed` through the `isError` half of `:87`, so the detail
    /// branch must still fall back to the generic text. Reading it with `??` emitted a bare
    /// `Error: ` for the empty-string case, which is what the review model would have seen.
    #[test]
    fn a_falsy_but_present_error_still_reports_the_generic_error_text() {
        for falsy in [json!(""), json!(0), json!(false)] {
            assert_eq!(
                format_tool_result("read", None, None, Some(&falsy), Some(&json!(true))),
                "Tool result: read\nError: tool reported an error",
                "falsy error {falsy} must not blank out the detail"
            );
        }
        // `null` and absent take the same fallback.
        assert_eq!(
            format_tool_result("read", None, None, Some(&json!(null)), Some(&json!(true))),
            "Tool result: read\nError: tool reported an error"
        );
        assert_eq!(
            format_tool_result("read", None, None, None, Some(&json!(true))),
            "Tool result: read\nError: tool reported an error"
        );
    }

    /// The truthy half of the same ternary: a string error is used verbatim, a non-string error
    /// goes through `formatValue`.
    #[test]
    fn a_truthy_error_is_reported_verbatim_or_formatted() {
        assert_eq!(
            format_tool_result("bash", None, None, Some(&json!("boom")), None),
            "Tool result: bash\nError: boom"
        );
        assert_eq!(
            format_tool_result("bash", None, None, Some(&json!({ "code": 2 })), None),
            format!(
                "Tool result: bash\nError: {}",
                format_value(&json!({ "code": 2 }), "")
            )
        );
    }
}
