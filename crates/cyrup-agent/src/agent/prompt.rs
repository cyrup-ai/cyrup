//! Prompt normalization — [`PromptInput`] and its conversions (Pi `normalizePromptInput`).

// `Agent` is in scope solely for the intra-doc link on `PromptInput` below, which resolved
// implicitly while this type lived in the same file as `Agent`.
#[allow(unused_imports)]
use super::Agent;
use super::util::now_millis;
use crate::event::AgentMessage;
use cyrup_core::Content;

// ---------------------------------------------------------------------------
// Public entry-point helpers
// ---------------------------------------------------------------------------

/// Input to [`Agent::prompt`]. Convertible from `&str`/`String`/`AgentMessage`/`Vec<AgentMessage>`.
pub struct PromptInput {
    pub messages: Vec<AgentMessage>,
}

impl PromptInput {
    /// A single user message carrying `text` followed by image attachments (Pi
    /// `normalizePromptInput`, agent.ts:379-383): `[{type:"text"}, ...images]`.
    pub fn text_with_images(text: impl Into<String>, images: Vec<Content>) -> Self {
        let mut content = vec![Content::text(text)];
        content.extend(images);
        // Pi `normalizePromptInput` stamps the string-input user message with `Date.now()`
        // (agent.ts:393); this value reaches the wire payload via `convert_to_llm`.
        Self { messages: vec![AgentMessage::User { content, timestamp: Some(now_millis()) }] }
    }

    /// The single message this input wraps (panics-free: returns an empty user message if empty).
    pub(super) fn into_one(mut self) -> AgentMessage {
        if self.messages.is_empty() {
            AgentMessage::user_text("")
        } else {
            self.messages.remove(0)
        }
    }
}

impl From<&str> for PromptInput {
    fn from(s: &str) -> Self {
        // Pi `normalizePromptInput` stamps a string prompt with `Date.now()` (agent.ts:389-393).
        Self { messages: vec![AgentMessage::User { content: vec![Content::text(s)], timestamp: Some(now_millis()) }] }
    }
}

impl From<String> for PromptInput {
    fn from(s: String) -> Self {
        // Pi `normalizePromptInput` stamps a string prompt with `Date.now()` (agent.ts:389-393).
        Self { messages: vec![AgentMessage::User { content: vec![Content::text(s)], timestamp: Some(now_millis()) }] }
    }
}

impl From<AgentMessage> for PromptInput {
    fn from(m: AgentMessage) -> Self {
        Self { messages: vec![m] }
    }
}

impl From<Vec<AgentMessage>> for PromptInput {
    fn from(messages: Vec<AgentMessage>) -> Self {
        Self { messages }
    }
}
