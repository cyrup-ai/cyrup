//! `renderCall` / `renderResult` for the two intercom tools — `v0.10.1 index.ts:1743-1774`
//! (`contact_supervisor`) and `:2298-2331` (`intercom`).
//!
//! Both upstream renderers build a single `Text` node out of theme-coloured fragments and return it,
//! so the port is a pure string projection: cyrup's native renderer contract
//! (`cyrup_ext::NativeExtension::render_call` / `render_result`) returns a serialized widget the
//! host flattens, and every branch upstream returns exactly one `Text`. This mirrors
//! `cyrup-ext-subagents/src/extension.rs`, which took the same shape for `subagent`.
//!
//! **Of upstream's renderer inputs, the options bag arrives and the theme does not.**
//! [`cyrup_ext::NativeExtension::render_result_under`] and
//! [`cyrup_ext::NativeExtension::render_call_under`] carry [`cyrup_ext::RenderOptions`] —
//! `expanded` (upstream's `context.expanded` on `renderCall`, and on `renderResult`) and
//! `isPartial` — and the host re-invokes both whenever either moves
//! (`cyrup_tui::App::refresh_extension_renders`), so every branch that reads them is ported live,
//! including `renderCall`'s Ctrl+O full-message arm (ICOM-083, `d5a8fd1` #153). What is still
//! absent:
//!
//! * every `theme.fg(...)` / `theme.bold(...)` wrapper degrades to its plain content (the same
//!   carve-out `cyrup-ext-subagents` records for `subagent`, and the reason the ✓/✗/⚠ glyphs — which
//!   are content, not colour — ARE ported: they are the only part of the status prefix that survives
//!   a themeless render);
//! * `context.isError` is not observable, so `failed` reduces to
//!   `details.error === true || details.delivered === false`. Nothing is lost by it on these two
//!   tools: an arm that fails returns a `ToolError`, whose result carries no `details` at all, so
//!   neither the roster collapse nor the message-id suffix can fire on it either way.

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

/// The message line of both `renderCall`s (`index.ts:2396-2400` / `:2894-2899@v0.16.1`, `d5a8fd1`
/// #153): `context.expanded && typeof args.message === "string" ? args.message :
/// previewText(args.message, 96)`, drawn only `if (messageText)`.
///
/// Expanded, the RAW string is shown — not whitespace-normalized, not truncated — so Ctrl+O reveals
/// the outgoing body exactly as the model wrote it. JS truthiness carries over exactly: an expanded
/// `""` draws nothing, while an expanded all-whitespace string is truthy and IS drawn raw. A
/// non-string message draws nothing in either state (`previewText` returns `undefined` for it).
fn call_message_text(args: &Value, expanded: bool) -> Option<String> {
    let message = args.get("message");
    let text = if expanded && let Some(raw) = message.and_then(Value::as_str) {
        Some(raw.to_string())
    } else {
        preview_text(message, 96)
    };
    text.filter(|t| !t.is_empty())
}

/// `renderCall` for `intercom` (`index.ts:2891-2914@v0.16.1`).
pub(crate) fn render_intercom_call(args: &Value, expanded: bool) -> String {
    // `typeof args.action === "string" ? args.action : "intercom"`.
    let action = string_field(args, "action").unwrap_or("intercom");
    // `typeof args.to === "string" && args.to.trim() ? args.to.trim() : undefined`.
    let target = string_field(args, "to")
        .map(str::trim)
        .filter(|t| !t.is_empty());
    let message_preview = call_message_text(args, expanded);
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

/// `renderResult` for `intercom` (`v0.14.0 index.ts:2723-2744`).
///
/// ICOM-066 (`a0cc5a1`, #127): a `list` / `list-cwd` result carries `details.roster`, and while the
/// row is collapsed it draws as one line — `N other sessions[ in <cwd>] (M connected)` — instead of
/// the whole roster. Display only: the model still receives the full roster text.
pub(crate) fn render_intercom_result(result: &Value, opts: &cyrup_ext::RenderOptions) -> String {
    if opts.is_partial {
        return "Intercom working...".to_string();
    }
    let details = result.get("details").filter(|d| !d.is_null());
    let failed = failed(details);
    let mut text = String::from(if failed { "✗ " } else { "✓ " });
    // `if (details?.roster && !failed && !context.expanded)` — "Collapsed rows are display-only; the
    // model still receives the full roster text." `roster` is truthiness-tested upstream and then
    // destructured, so an object is required and its fields are read as they are.
    if let Some(roster) = details
        .and_then(|d| d.get("roster"))
        .filter(|r| r.is_object())
        .filter(|_| !failed && !opts.expanded)
    {
        let peers = roster.get("peers").and_then(Value::as_u64).unwrap_or(0);
        let total = roster.get("total").and_then(Value::as_u64).unwrap_or(0);
        // `const where = rosterCwd ? ` in ${rosterCwd}` : ""` — JS-truthy, so `""` draws nothing.
        let place = string_field(roster, "cwd")
            .filter(|c| !c.is_empty())
            .map_or_else(String::new, |c| format!(" in {c}"));
        if peers == 0 {
            text.push_str(&format!("no other sessions{place}"));
        } else {
            let plural = if peers == 1 { "" } else { "s" };
            text.push_str(&format!("{peers} other session{plural}{place}"));
        }
        text.push_str(&format!(" ({total} connected)"));
        return text;
    }
    text.push_str(&first_text_content(result));
    // `if (details?.messageId && !context.expanded)` — a truthiness test, so an empty id is skipped.
    if let Some(message_id) = details
        .and_then(|d| string_field(d, "messageId"))
        .filter(|id| !id.is_empty() && !opts.expanded)
    {
        text.push_str(&format!(
            " ({})",
            crate::identity::short_session_id(message_id)
        ));
    }
    // `if (details?.reason && context.expanded)`.
    if let Some(reason) = details
        .and_then(|d| string_field(d, "reason"))
        .filter(|r| !r.is_empty() && opts.expanded)
    {
        text.push_str(&format!("\nReason: {reason}"));
    }
    text
}

/// `renderCall` for `contact_supervisor` (`index.ts:2394-2411@v0.16.1`).
pub(crate) fn render_contact_supervisor_call(args: &Value, expanded: bool) -> String {
    // `typeof args.reason === "string" ? args.reason : "contact"`.
    let reason = string_field(args, "reason").unwrap_or("contact");
    let message_preview = call_message_text(args, expanded);
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

/// `renderResult` for `contact_supervisor` (`v0.14.0 index.ts:2157-2175`).
pub(crate) fn render_contact_supervisor_result(
    result: &Value,
    opts: &cyrup_ext::RenderOptions,
) -> String {
    if opts.is_partial {
        return "Waiting for supervisor...".to_string();
    }
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
        let text = render_intercom_call(
            &json!({
                "action": "ask",
                "to": "  reviewer  ",
                "attachments": [{ "name": "a" }],
                "message": "please   review\nthis",
            }),
            false,
        );
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
            render_intercom_call(
                &json!({ "action": "ask", "to": "reviewer", "attachments": [] }),
                false
            ),
            "intercom ask → reviewer"
        );
        // One attachment is singular; two are plural.
        let two = render_intercom_call(
            &json!({
                "action": "send",
                "attachments": [1, 2],
                "message": "hi",
            }),
            false,
        );
        assert_eq!(two, "intercom send (2 attachments)\n  hi");
        // A blank `to` is dropped (`args.to.trim()` truthiness), and a missing action is "intercom".
        assert_eq!(
            render_intercom_call(&json!({ "to": "   " }), false),
            "intercom intercom"
        );
    }

    /// `intercom.integration.test.ts` "outgoing tool calls preserve full messages when expanded"
    /// (`d5a8fd1` #153, v0.16.1): the same leading-space, blank-line, CJK + emoji body, rendered
    /// raw when expanded and as the 96-char normalized preview when collapsed, for BOTH tools.
    #[test]
    fn outgoing_tool_calls_preserve_full_messages_when_expanded() {
        let message = format!("  {}\n\n    中文🧪 tail", "a".repeat(100));
        let cases = [
            (
                render_intercom_call as fn(&Value, bool) -> String,
                json!({ "action": "send", "to": "planner", "message": message }),
                "intercom send → planner",
            ),
            (
                render_contact_supervisor_call,
                json!({ "reason": "progress_update", "message": message }),
                "contact_supervisor progress_update",
            ),
        ];
        for (render, args, title) in cases {
            assert_eq!(render(&args, true), format!("{title}\n  {message}"));
            assert_eq!(
                render(&args, false),
                format!("{title}\n  {}…", "a".repeat(95))
            );
            // The renderer reads the args; it never rewrites them.
            assert_eq!(args.get("message"), Some(&json!(message)));
        }
    }

    /// JS truthiness on `messageText`: an expanded `""` draws nothing, an expanded all-whitespace
    /// string is truthy and is drawn raw (collapsed, it normalizes to empty and draws nothing), and
    /// a non-string message draws nothing in either state.
    #[test]
    fn expanded_call_message_follows_upstreams_truthiness() {
        for expanded in [false, true] {
            assert_eq!(
                render_intercom_call(&json!({ "action": "send", "message": "" }), expanded),
                "intercom send"
            );
            assert_eq!(
                render_intercom_call(&json!({ "action": "send", "message": 42 }), expanded),
                "intercom send"
            );
            assert_eq!(
                render_contact_supervisor_call(&json!({ "message": ["x"] }), expanded),
                "contact_supervisor contact"
            );
        }
        assert_eq!(
            render_intercom_call(&json!({ "action": "send", "message": "   " }), false),
            "intercom send"
        );
        assert_eq!(
            render_intercom_call(&json!({ "action": "send", "message": "   " }), true),
            "intercom send\n     "
        );
    }

    fn collapsed() -> cyrup_ext::RenderOptions {
        cyrup_ext::RenderOptions::default()
    }

    fn expanded() -> cyrup_ext::RenderOptions {
        cyrup_ext::RenderOptions {
            expanded: true,
            ..Default::default()
        }
    }

    /// ICOM-066 — `a0cc5a1`'s own render case (`intercom.integration.test.ts:1797-1804` @v0.14.0):
    /// collapsed, a roster is one line; expanded, it is the full text the model received.
    #[test]
    fn a_roster_result_collapses_to_one_line_and_expands_to_the_full_roster() {
        let roster = json!({
            "content": [{ "type": "text", "text": "**Current session:**\n• me\n\n**Other sessions (cwd: /repo):**\n• peer-a\n• peer-b" }],
            "details": { "roster": { "peers": 2, "total": 3, "cwd": "/repo" } },
        });
        assert_eq!(
            render_intercom_result(&roster, &collapsed()),
            "✓ 2 other sessions in /repo (3 connected)"
        );
        let full = render_intercom_result(&roster, &expanded());
        assert!(full.contains("peer-a\n• peer-b"), "{full}");

        // `list` carries no `cwd`; one peer is singular; zero peers is `no other sessions`.
        let one = json!({ "content": [], "details": { "roster": { "peers": 1, "total": 2 } } });
        assert_eq!(
            render_intercom_result(&one, &collapsed()),
            "✓ 1 other session (2 connected)"
        );
        let none = json!({ "content": [], "details": { "roster": { "peers": 0, "total": 1, "cwd": "/x" } } });
        assert_eq!(
            render_intercom_result(&none, &collapsed()),
            "✓ no other sessions in /x (1 connected)"
        );
        // A failed result never collapses (`!failed`).
        let failed_roster = json!({
            "content": [{ "type": "text", "text": "broken" }],
            "details": { "error": true, "roster": { "peers": 1, "total": 2 } },
        });
        assert_eq!(
            render_intercom_result(&failed_roster, &collapsed()),
            "✗ broken"
        );
    }

    /// `isPartial` and `context.expanded` on the non-roster arms (`v0.14.0 index.ts:2724-2743`,
    /// `:2157-2160`): the placeholders while a call runs, the message-id suffix only collapsed, and
    /// the `Reason:` line only expanded.
    #[test]
    fn partial_and_expanded_options_select_upstreams_branches() {
        let partial = cyrup_ext::RenderOptions::default().partial(true);
        assert_eq!(
            render_intercom_result(&json!({}), &partial),
            "Intercom working..."
        );
        assert_eq!(
            render_contact_supervisor_result(&json!({}), &partial),
            "Waiting for supervisor..."
        );
        let undelivered = json!({
            "content": [{ "type": "text", "text": "not delivered" }],
            "details": { "messageId": "0192f3c1-9a10", "delivered": false, "reason": "gone" },
        });
        assert_eq!(
            render_intercom_result(&undelivered, &collapsed()),
            "✗ not delivered (0192f3c1)"
        );
        assert_eq!(
            render_intercom_result(&undelivered, &expanded()),
            "✗ not delivered\nReason: gone"
        );
    }

    #[test]
    fn intercom_result_marks_failure_and_prints_the_message_id_prefix() {
        let ok = render_intercom_result(
            &json!({
                "content": [{ "type": "text", "text": "Message sent to reviewer" }],
                "details": { "messageId": "0192f3c1-9a10-7000-8000-aaaaaaaaaaaa", "delivered": true },
            }),
            &collapsed(),
        );
        assert_eq!(ok, "✓ Message sent to reviewer (0192f3c1)");
        // `delivered: false` is the failure marker even with no `error` key.
        let bad = render_intercom_result(
            &json!({
                "content": [{ "type": "text", "text": "not delivered" }],
                "details": { "messageId": "0192f3c1-9a10", "delivered": false, "reason": "gone" },
            }),
            &collapsed(),
        );
        assert!(bad.starts_with("✗ "), "got {bad}");
        // No details at all: success marker, no id suffix.
        let bare = render_intercom_result(
            &json!({
                "content": [{ "type": "text", "text": "No unresolved inbound asks." }],
                "details": {},
            }),
            &collapsed(),
        );
        assert_eq!(bare, "✓ No unresolved inbound asks.");
    }

    #[test]
    fn contact_supervisor_renderers_port_the_reason_title_and_parse_warning() {
        let call = render_contact_supervisor_call(
            &json!({
                "reason": "interview_request",
                "interview": { "title": "  Pick a plan  " },
                "message": "which one?",
            }),
            false,
        );
        assert_eq!(
            call,
            "contact_supervisor interview_request Pick a plan\n  which one?"
        );
        // Missing reason is upstream's "contact".
        assert_eq!(
            render_contact_supervisor_call(&json!({}), false),
            "contact_supervisor contact"
        );

        let warn = render_contact_supervisor_result(
            &json!({
                "content": [{ "type": "text", "text": "**Reply from supervisor:**\nok" }],
                "details": { "structuredReplyParseError": "missing field `choice`" },
            }),
            &collapsed(),
        );
        assert_eq!(
            warn,
            "⚠ Reply from supervisor:\nok\nStructured reply parse issue: missing field `choice`"
        );
        // A failure outranks the parse warning (upstream's ternary tests `failed` first).
        let bad = render_contact_supervisor_result(
            &json!({
                "content": [{ "type": "text", "text": "nope" }],
                "details": { "error": true, "structuredReplyParseError": "x" },
            }),
            &collapsed(),
        );
        assert!(bad.starts_with("✗ "), "got {bad}");
    }
}
