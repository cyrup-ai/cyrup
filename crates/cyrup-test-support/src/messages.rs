//! Message builders (Pi `userMsg`/`assistantMsg`, utilities.ts:130-155; `createUserMessage`/
//! `createAssistantMessage`, session-test-utils.ts:7-33). The exact default usage/cost shapes Pi
//! uses, adapted to cyrup's [`cyrup_core::Message`] / [`cyrup_agent::AgentMessage`].

use cyrup_agent::AgentMessage;
use cyrup_core::{ApiId, AssistantMessage, Content, Cost, Message, ProviderId, StopReason, Usage};

/// A minimal persisted user message (Pi `userMsg`, utilities.ts:130-132). `timestamp` is fixed at 0
/// for deterministic snapshots (Pi uses `Date.now()`; [CYRUP-DELTA] zeroes it).
pub fn user_msg(text: impl Into<String>) -> Message {
    Message::User { content: vec![Content::text(text)], timestamp: 0 }
}

/// A minimal persisted assistant message (Pi `assistantMsg`, utilities.ts:137-155): provider
/// `anthropic`, model `test`, api `anthropic-messages`, usage `{input:1, output:1, total:2}`.
pub fn assistant_msg(text: impl Into<String>) -> Message {
    Message::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        provider: ProviderId::from("anthropic"),
        model: "test".to_string(),
        api: ApiId::from("anthropic-messages"),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage {
            input: 1,
            output: 1,
            cache_read: 0,
            cache_write: 0,
            cache_write_1h: None,
            reasoning: None,
            total_tokens: 2,
            cost: Cost::default(),
        },
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
}

/// A user [`AgentMessage`] (Pi `createUserMessage`, session-test-utils.ts:7-13).
pub fn create_user_message(text: impl Into<String>) -> AgentMessage {
    AgentMessage::user_text(text)
}

/// An assistant [`AgentMessage`] (Pi `createAssistantMessage`, session-test-utils.ts:15-33):
/// provider `anthropic`, model `claude-sonnet-4-5`, api `anthropic-messages`, zero usage.
pub fn create_assistant_message(text: impl Into<String>) -> AgentMessage {
    AgentMessage::Assistant(AssistantMessage {
        content: vec![Content::text(text)],
        provider: ProviderId::from("anthropic"),
        model: "claude-sonnet-4-5".to_string(),
        api: ApiId::from("anthropic-messages"),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: Usage::default(),
        stop_reason: StopReason::Stop,
        error_message: None,
        timestamp: 0,
    })
}
