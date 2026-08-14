//! The per-request GitHub Copilot headers — a 1:1 port of pi v0.83.0
//! `packages/ai/src/api/github-copilot-headers.ts`.
//!
//! Distinct from the STATIC editor-identity headers
//! ([`crate::providers::github_copilot::COPILOT_HEADERS`]), which reach request traffic through
//! every catalog row's `model.headers`. These three are *dynamic*: they depend on the messages
//! being sent, so they are computed per request and layered on top of `model.headers` exactly
//! where pi's `Object.assign(headers, copilotHeaders)` puts them.
//!
//! Wired on all three of Copilot's wire routes, matching pi's three importers:
//!
//! | pi call site                              | cyrup                                     |
//! |-------------------------------------------|-------------------------------------------|
//! | `api/anthropic-messages.ts:525-531`        | `api/anthropic_messages.rs`'s `build_headers`     |
//! | `api/openai-completions.ts:638-645`        | `api/openai_completions.rs`'s `build_headers`     |
//! | `api/openai-responses.ts:223-230`          | `api/openai_responses.rs`'s `build_headers`       |

use cyrup_core::{Content, Message};

/// `X-Initiator` — whether the request was user-initiated or is an agent-loop follow-up
/// (`github-copilot-headers.ts:28`).
pub const X_INITIATOR: &str = "X-Initiator";

/// `Openai-Intent`, always `conversation-edits` (`github-copilot-headers.ts:29`).
pub const OPENAI_INTENT: &str = "Openai-Intent";

/// `Copilot-Vision-Request` — sent only when the turn carries an image
/// (`github-copilot-headers.ts:33`).
pub const COPILOT_VISION_REQUEST: &str = "Copilot-Vision-Request";

/// The provider id these headers are gated on (pi `model.provider === "github-copilot"`).
pub const GITHUB_COPILOT_PROVIDER: &str = "github-copilot";

/// 1:1 port of `inferCopilotInitiator` (`github-copilot-headers.ts:5-8`):
///
/// ```ts
/// const last = messages[messages.length - 1];
/// return last && last.role !== "user" ? "agent" : "user";
/// ```
///
/// An EMPTY message list is `"user"` (pi's `last` is `undefined`, so the ternary takes the `"user"`
/// arm) — not `"agent"`.
pub fn infer_copilot_initiator(messages: &[Message]) -> &'static str {
    match messages.last() {
        Some(Message::User { .. }) | None => "user",
        Some(_) => "agent",
    }
}

/// 1:1 port of `hasCopilotVisionInput` (`github-copilot-headers.ts:11-21`): any `image` block in a
/// `user` or `toolResult` turn. Assistant turns are not inspected — pi checks only those two roles.
///
/// `[CYRUP-DELTA]` pi additionally guards each arm with `Array.isArray(msg.content)`, because a
/// `UserMessage.content` may be the bare-string shorthand (`types.ts:379`). cyrup's
/// [`Message::User`] normalizes that shorthand to a single `Text` block on deserialize
/// (`cyrup-core/src/message.rs`, `de_user_content`), so there is no non-array form to skip and the
/// match set is identical: a bare string can never contain an image block.
pub fn has_copilot_vision_input(messages: &[Message]) -> bool {
    messages.iter().any(|msg| match msg {
        Message::User { content, .. } | Message::ToolResult { content, .. } => {
            content.iter().any(|c| matches!(c, Content::Image { .. }))
        }
        Message::Assistant(_) => false,
    })
}

/// 1:1 port of `buildCopilotDynamicHeaders` (`github-copilot-headers.ts:23-37`). Returns the pairs
/// in pi's insertion order; `Copilot-Vision-Request` is present only when `has_images`.
pub fn build_copilot_dynamic_headers(
    messages: &[Message],
    has_images: bool,
) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        (X_INITIATOR, infer_copilot_initiator(messages).to_string()),
        (OPENAI_INTENT, "conversation-edits".to_string()),
    ];
    if has_images {
        headers.push((COPILOT_VISION_REQUEST, "true".to_string()));
    }
    headers
}

/// The whole gate in one call: when `provider` is `github-copilot`, layer the dynamic headers into
/// `headers`; otherwise leave it untouched. Callers place this where pi's
/// `Object.assign(headers, copilotHeaders)` sits — after `model.headers`, before the per-request
/// `options.headers` overlay — so a caller-supplied header still wins.
pub(crate) fn apply_copilot_dynamic_headers(
    headers: &mut crate::HeaderMap,
    provider: &str,
    messages: &[Message],
) {
    if provider != GITHUB_COPILOT_PROVIDER {
        return;
    }
    let has_images = has_copilot_vision_input(messages);
    for (name, value) in build_copilot_dynamic_headers(messages, has_images) {
        headers.insert(name.to_string(), Some(value));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use cyrup_core::{AssistantMessage, ProviderId, StopReason, ToolCallId, Usage};

    fn user(content: Vec<Content>) -> Message {
        Message::User {
            content,
            timestamp: 0,
        }
    }

    fn text(t: &str) -> Content {
        Content::Text {
            text: t.to_string(),
            text_signature: None,
        }
    }

    fn image() -> Content {
        Content::Image {
            data: "aGk=".to_string(),
            mime_type: "image/png".to_string(),
        }
    }

    fn tool_result(content: Vec<Content>) -> Message {
        Message::ToolResult {
            tool_call_id: ToolCallId::from("call_1"),
            tool_name: "read".to_string(),
            content,
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: Vec::new(),
            timestamp: 0,
        }
    }

    fn assistant() -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![text("ok")],
            provider: ProviderId::from("github-copilot"),
            model: "claude-sonnet-4.5".into(),
            api: "anthropic-messages".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        })
    }

    /// `github-copilot-headers.ts:5-8`.
    #[test]
    fn initiator_is_user_only_when_the_last_turn_is_a_user_turn() {
        assert_eq!(infer_copilot_initiator(&[]), "user");
        assert_eq!(infer_copilot_initiator(&[user(vec![text("hi")])]), "user");
        assert_eq!(
            infer_copilot_initiator(&[user(vec![text("hi")]), assistant()]),
            "agent"
        );
        assert_eq!(
            infer_copilot_initiator(&[user(vec![text("hi")]), assistant(), tool_result(vec![])]),
            "agent"
        );
    }

    /// `github-copilot-headers.ts:11-21` — user and toolResult turns only.
    #[test]
    fn vision_input_detected_on_user_and_tool_result_turns() {
        assert!(!has_copilot_vision_input(&[user(vec![text("hi")])]));
        assert!(has_copilot_vision_input(&[user(vec![text("hi"), image()])]));
        assert!(has_copilot_vision_input(&[tool_result(vec![image()])]));
        assert!(!has_copilot_vision_input(&[assistant()]));
    }

    /// `github-copilot-headers.ts:23-37`.
    #[test]
    fn dynamic_headers_carry_intent_and_gate_vision() {
        let plain = build_copilot_dynamic_headers(&[user(vec![text("hi")])], false);
        assert_eq!(
            plain,
            vec![
                (X_INITIATOR, "user".to_string()),
                (OPENAI_INTENT, "conversation-edits".to_string()),
            ]
        );

        let with_image = build_copilot_dynamic_headers(&[user(vec![image()])], true);
        assert_eq!(
            with_image.last(),
            Some(&(COPILOT_VISION_REQUEST, "true".to_string()))
        );
    }

    /// The gate: a non-Copilot provider gets none of them.
    #[test]
    fn non_copilot_providers_are_untouched() {
        let mut headers = crate::HeaderMap::new();
        apply_copilot_dynamic_headers(&mut headers, "openai", &[user(vec![image()])]);
        assert!(headers.is_empty());

        apply_copilot_dynamic_headers(&mut headers, "github-copilot", &[user(vec![image()])]);
        assert_eq!(
            headers.get(X_INITIATOR).and_then(|v| v.as_deref()),
            Some("user")
        );
        assert_eq!(
            headers.get(OPENAI_INTENT).and_then(|v| v.as_deref()),
            Some("conversation-edits")
        );
        assert_eq!(
            headers.get(COPILOT_VISION_REQUEST).and_then(|v| v.as_deref()),
            Some("true")
        );
    }
}
