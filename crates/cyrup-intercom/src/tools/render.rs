//! `renderCall` / `renderResult` for the two intercom tools — `v0.10.1 index.ts:1743-1774`
//! (`contact_supervisor`) and `:2298-2331` (`intercom`).
//!
//! Both upstream renderers build a single `Text` node out of theme-coloured fragments and return it,
//! so the port is a pure string projection: cyrup's native renderer contract
//! (`cyrup_ext::NativeExtension::render_call` / `render_result`) returns a serialized widget the
//! host flattens, and every branch upstream returns exactly one `Text`. This mirrors
//! `cyrup-ext-subagents/src/extension.rs`, which took the same shape for `subagent`.
//!
//! **Three of upstream's inputs do not exist on this seam, and the branches that read them are
//! therefore unreachable rather than unported.** cyrup's `render_call`/`render_result` receive the
//! call/result payload and nothing else — no `theme`, no `{ isPartial }`, no
//! `context.isError`/`context.expanded`. So:
//!
//! * every `theme.fg(...)` / `theme.bold(...)` wrapper degrades to its plain content (the same
//!   carve-out `cyrup-ext-subagents` records for `subagent`, and the reason the ✓/✗/⚠ glyphs — which
//!   are content, not colour — ARE ported: they are the only part of the status prefix that survives
//!   a themeless render);
//! * `isPartial` is never true here, so the `Intercom working...` / `Waiting for supervisor...`
//!   placeholders (`:1758`, `:2317`) have no input to fire on;
//! * `context.isError` is not observable, so `failed` reduces to
//!   `details.error === true || details.delivered === false`, and `context.expanded` is not
//!   observable either, so the `!context.expanded` message-id suffix is drawn unconditionally (the
//!   collapsed tier, which is what a transcript row shows by default) and the `context.expanded`
//!   `Reason:` line is not drawn at all.

use serde_json::Value;

/// `previewText(value, maxLength = 72)` (`v0.10.1 index.ts:455-464`).
///
/// Returns `None` for a non-string, and for a string that normalizes to empty. The ellipsis branch
/// is `slice(0, maxLength - 1)` + `…`, i.e. the RESULT is `maxLength` chars, not `maxLength + 1`.
///
/// `String::replace(/\s+/g, " ")` is `split_whitespace().join(" ")`, which also trims — and here
/// that is exact rather than approximate, because upstream `.trim()`s immediately afterwards.
/// Lengths are counted in `chars()`, JS's UTF-16 code units being the one residual difference on
/// astral input (the same nit already recorded against `ICOM-007`'s pending-ask preview).
fn preview_text(value: Option<&Value>, max_length: usize) -> Option<String> {
    let raw = value?.as_str()?;
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.is_empty() {
        return None;
    }
    if normalized.chars().count() > max_length {
        let head: String = normalized
            .chars()
            .take(max_length.saturating_sub(1))
            .collect();
        Some(format!("{head}…"))
    } else {
        Some(normalized)
    }
}

/// `firstTextContent(result)` (`v0.10.1 index.ts:465-467`): the first `text` content item, with
/// **every** `**` stripped (`replace(/\*\*/g, "")` — a global replace, so the bold markers of a
/// `**Reply from …:**` header both go), defaulting to `""`.
fn first_text_content(result: &Value) -> String {
    result
        .get("content")
        .and_then(Value::as_array)
        .and_then(|items| {
            items.iter().find(|item| {
                item.get("type").and_then(Value::as_str) == Some("text")
                    && item.get("text").and_then(Value::as_str).is_some()
            })
        })
        .and_then(|item| item.get("text").and_then(Value::as_str))
        .unwrap_or_default()
        .replace("**", "")
}

/// `details.error === true || details.delivered === false` — the observable half of upstream's
/// `Boolean(context.isError || details?.error === true || details?.delivered === false)`
/// (`:1761`, `:2320`). Both comparisons are strict, so a missing key is NOT a failure.
fn failed(details: Option<&Value>) -> bool {
    let Some(details) = details else { return false };
    details.get("error").and_then(Value::as_bool) == Some(true)
        || details.get("delivered").and_then(Value::as_bool) == Some(false)
}

fn string_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// `renderCall` for `intercom` (`v0.10.1 index.ts:2298-2315`).
pub(crate) fn render_intercom_call(args: &Value) -> String {
    // `typeof args.action === "string" ? args.action : "intercom"`.
    let action = string_field(args, "action").unwrap_or("intercom");
    // `typeof args.to === "string" && args.to.trim() ? args.to.trim() : undefined`.
    let target = string_field(args, "to")
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let message_preview = preview_text(args.get("message"), 96);
    let attachment_count = args
        .get("attachments")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    let mut text = format!("intercom {action}");
    if let Some(target) = target {
        text.push_str(&format!(" → {target}"));
    }
    if attachment_count > 0 {
        // `${n} attachment${n === 1 ? "" : "s"}`.
        let plural = if attachment_count == 1 { "" } else { "s" };
        text.push_str(&format!(" ({attachment_count} attachment{plural})"));
    }
    if let Some(preview) = message_preview {
        text.push_str(&format!("\n  {preview}"));
    }
    text
}

/// `renderResult` for `intercom` (`v0.10.1 index.ts:2316-2331`).
pub(crate) fn render_intercom_result(result: &Value) -> String {
    let details = result.get("details").filter(|d| !d.is_null());
    let failed = failed(details);
    let mut text = String::from(if failed { "✗ " } else { "✓ " });
    text.push_str(&first_text_content(result));
    // `if (details?.messageId && !context.expanded)` — a truthiness test, so an empty id is skipped.
    // `context.expanded` is not observable here (see the module doc), so this draws the collapsed
    // tier a transcript row shows by default.
    if let Some(message_id) = details
        .and_then(|d| string_field(d, "messageId"))
        .filter(|id| !id.is_empty())
    {
        text.push_str(&format!(
            " ({})",
            crate::identity::short_session_id(message_id)
        ));
    }
    text
}

/// `renderCall` for `contact_supervisor` (`v0.10.1 index.ts:1743-1756`).
pub(crate) fn render_contact_supervisor_call(args: &Value) -> String {
    // `typeof args.reason === "string" ? args.reason : "contact"`.
    let reason = string_field(args, "reason").unwrap_or("contact");
    let message_preview = preview_text(args.get("message"), 96);
    // `args.interview && typeof args.interview === "object"` — an ARRAY passes upstream's test too
    // (`typeof [] === "object"`), but then `.title` is `undefined` and the branch is skipped, which
    // is what reading `title` off a non-object here also produces.
    let interview_title = args
        .get("interview")
        .and_then(|v| string_field(v, "title"))
        .map(str::trim)
        .filter(|t| !t.is_empty());

    let mut text = format!("contact_supervisor {reason}");
    if let Some(title) = interview_title {
        text.push_str(&format!(" {title}"));
    }
    if let Some(preview) = message_preview {
        text.push_str(&format!("\n  {preview}"));
    }
    text
}

/// `renderResult` for `contact_supervisor` (`v0.10.1 index.ts:1757-1773`).
pub(crate) fn render_contact_supervisor_result(result: &Value) -> String {
    let details = result.get("details").filter(|d| !d.is_null());
    let failed = failed(details);
    // `typeof details?.structuredReplyParseError === "string"` — presence of the KEY as a string,
    // not its truthiness, so an empty-string parse error still warns.
    let parse_warning = details
        .and_then(|d| d.get("structuredReplyParseError"))
        .and_then(Value::as_str);

    let mut text = String::from(match (failed, parse_warning.is_some()) {
        (true, _) => "✗ ",
        (false, true) => "⚠ ",
        (false, false) => "✓ ",
    });
    text.push_str(&first_text_content(result));
    if let Some(err) = parse_warning {
        text.push_str(&format!("\nStructured reply parse issue: {err}"));
    }
    text
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preview_text_ports_pis_normalize_trim_and_ellipsis() {
        // Non-string and empty-after-normalize both yield `undefined`.
        assert_eq!(preview_text(Some(&json!(7)), 96), None);
        assert_eq!(preview_text(Some(&json!("   \n\t ")), 96), None);
        assert_eq!(preview_text(None, 96), None);
        // `\s+` → one space, then trim.
        assert_eq!(
            preview_text(Some(&json!("  a \n\t b  ")), 96).unwrap(),
            "a b"
        );
        // The ellipsis branch is `slice(0, maxLength - 1)` + `…`, so the RESULT is `maxLength`
        // chars — an off-by-one that a `take(max)` + `…` port would get wrong.
        let long = "x".repeat(100);
        let cut = preview_text(Some(&json!(long)), 96).unwrap();
        assert_eq!(cut.chars().count(), 96);
        assert!(cut.ends_with('…'));
        assert_eq!(cut.chars().filter(|c| *c == 'x').count(), 95);
        // Exactly `maxLength` is NOT truncated (`>` not `>=`).
        let exact = "y".repeat(96);
        assert_eq!(preview_text(Some(&json!(exact)), 96).unwrap(), exact);
    }

    #[test]
    fn first_text_content_strips_every_bold_marker_and_skips_non_text() {
        let result = json!({
            "content": [
                { "type": "image" },
                { "type": "text", "text": "**Reply from reviewer:**\nlooks good" },
                { "type": "text", "text": "second" },
            ]
        });
        assert_eq!(
            first_text_content(&result),
            "Reply from reviewer:\nlooks good"
        );
        // No text content at all is upstream's `?? ""`.
        assert_eq!(first_text_content(&json!({ "content": [] })), "");
        assert_eq!(first_text_content(&json!({})), "");
    }

    #[test]
    fn failed_is_strict_on_both_keys() {
        assert!(!failed(None));
        assert!(!failed(Some(&json!({}))));
        // Strict equality: a MISSING `delivered` is not a failure, a `false` one is.
        assert!(!failed(Some(&json!({ "messageId": "m1" }))));
        assert!(failed(Some(&json!({ "delivered": false }))));
        assert!(!failed(Some(&json!({ "delivered": true }))));
        assert!(failed(Some(&json!({ "error": true }))));
        // `error === true` is strict, so a truthy non-boolean does NOT fail the row.
        assert!(!failed(Some(&json!({ "error": "yes" }))));
    }

    #[test]
    fn intercom_call_renders_action_target_attachments_and_preview() {
        let text = render_intercom_call(&json!({
            "action": "ask",
            "to": "  reviewer  ",
            "attachments": [{ "name": "a" }],
            "message": "please   review\nthis",
        }));
        // Upstream's guard is `if (attachmentCount > 0)` (`v0.10.1 index.ts:2308`), so ONE
        // attachment draws the segment too — singular, with no `s`. This assertion originally
        // omitted it while still passing an `attachments` array, contradicting both the test's own
        // name and the plural case two lines below.
        assert_eq!(
            text,
            "intercom ask → reviewer (1 attachment)\n  please review this"
        );
        // Zero attachments is the only count that draws nothing.
        assert_eq!(
            render_intercom_call(&json!({ "action": "ask", "to": "reviewer", "attachments": [] })),
            "intercom ask → reviewer"
        );
        // One attachment is singular; two are plural.
        let two = render_intercom_call(&json!({
            "action": "send",
            "attachments": [1, 2],
            "message": "hi",
        }));
        assert_eq!(two, "intercom send (2 attachments)\n  hi");
        // A blank `to` is dropped (`args.to.trim()` truthiness), and a missing action is "intercom".
        assert_eq!(
            render_intercom_call(&json!({ "to": "   " })),
            "intercom intercom"
        );
    }

    #[test]
    fn intercom_result_marks_failure_and_prints_the_message_id_prefix() {
        let ok = render_intercom_result(&json!({
            "content": [{ "type": "text", "text": "Message sent to reviewer" }],
            "details": { "messageId": "0192f3c1-9a10-7000-8000-aaaaaaaaaaaa", "delivered": true },
        }));
        assert_eq!(ok, "✓ Message sent to reviewer (0192f3c1)");
        // `delivered: false` is the failure marker even with no `error` key.
        let bad = render_intercom_result(&json!({
            "content": [{ "type": "text", "text": "not delivered" }],
            "details": { "messageId": "0192f3c1-9a10", "delivered": false, "reason": "gone" },
        }));
        assert!(bad.starts_with("✗ "), "got {bad}");
        // No details at all: success marker, no id suffix.
        let bare = render_intercom_result(&json!({
            "content": [{ "type": "text", "text": "No unresolved inbound asks." }],
            "details": {},
        }));
        assert_eq!(bare, "✓ No unresolved inbound asks.");
    }

    #[test]
    fn contact_supervisor_renderers_port_the_reason_title_and_parse_warning() {
        let call = render_contact_supervisor_call(&json!({
            "reason": "interview_request",
            "interview": { "title": "  Pick a plan  " },
            "message": "which one?",
        }));
        assert_eq!(
            call,
            "contact_supervisor interview_request Pick a plan\n  which one?"
        );
        // Missing reason is upstream's "contact".
        assert_eq!(
            render_contact_supervisor_call(&json!({})),
            "contact_supervisor contact"
        );

        let warn = render_contact_supervisor_result(&json!({
            "content": [{ "type": "text", "text": "**Reply from supervisor:**\nok" }],
            "details": { "structuredReplyParseError": "missing field `choice`" },
        }));
        assert_eq!(
            warn,
            "⚠ Reply from supervisor:\nok\nStructured reply parse issue: missing field `choice`"
        );
        // A failure outranks the parse warning (upstream's ternary tests `failed` first).
        let bad = render_contact_supervisor_result(&json!({
            "content": [{ "type": "text", "text": "nope" }],
            "details": { "error": true, "structuredReplyParseError": "x" },
        }));
        assert!(bad.starts_with("✗ "), "got {bad}");
    }
}
