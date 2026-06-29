//! The MUTATING hook seam (arch-02 §3.3 / func-02 §8).
//!
//! Distinct from the notify-only [`crate::subscriber::EventSubscriber`]. Hooks may rewrite the LLM
//! context, block/mutate tool calls, override tool results, and steer the loop. Each is invoked on
//! the single loop task (never concurrently) and a returned `Err` degrades per the failure-mode map
//! (func-02 R-02-050) rather than panicking.

use crate::error::HookError;
use crate::event::AgentMessage;
use cyrup_core::{CancelToken, Content, Message, ModelRef, ModelThinkingLevel, ToolCallId};
use serde_json::Value;

/// Per-call context for [`Hooks::before_tool_call`]. `args` is mutable so a hook may rewrite the
/// arguments in place; mutated args are executed as-is WITHOUT re-validation (func-02 R-02-022).
pub struct BeforeToolCall<'a> {
    pub tool_name: &'a str,
    pub tool_call_id: &'a ToolCallId,
    pub args: &'a mut Value,
    pub messages: &'a [AgentMessage],
}

/// Outcome of [`Hooks::before_tool_call`] (func-02 R-02-021).
pub enum BeforeOutcome {
    Proceed,
    Block { reason: Option<String> },
}

/// Per-call context for [`Hooks::after_tool_call`].
pub struct AfterToolCall<'a> {
    pub tool_name: &'a str,
    pub tool_call_id: &'a ToolCallId,
    pub args: &'a Value,
    pub content: &'a [Content],
    pub details: Option<&'a Value>,
    pub is_error: bool,
    pub terminate: bool,
}

/// Replace-not-merge override returned by [`Hooks::after_tool_call`] (func-02 R-02-025): each
/// `Some(_)` field replaces the whole corresponding result field; `None` keeps the original.
#[derive(Clone, Debug, Default)]
pub struct AfterOverride {
    pub content: Option<Vec<Content>>,
    pub details: Option<Value>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

/// Post-turn context for [`Hooks::prepare_next_turn`] and [`Hooks::should_stop_after_turn`].
pub struct PostTurn<'a> {
    pub messages: &'a [AgentMessage],
    pub turn_index: usize,
}

/// Next-turn-only overrides returned by [`Hooks::prepare_next_turn`] (func-02 R-02-031, not sticky).
#[derive(Clone, Debug, Default)]
pub struct TurnUpdate {
    pub context: Option<Vec<AgentMessage>>,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<ModelThinkingLevel>,
}

/// The default `convert_to_llm`: keep only `user`/`assistant`/`toolResult`, drop `Custom`
/// (func-02 R-02-029/052).
pub fn default_convert_to_llm(msgs: &[AgentMessage]) -> Vec<Message> {
    msgs.iter()
        .filter_map(|m| match m {
            AgentMessage::User { content, timestamp } => {
                Some(Message::User { content: content.clone(), timestamp: timestamp.unwrap_or(0) })
            }
            AgentMessage::Assistant(a) => Some(Message::Assistant(a.clone())),
            AgentMessage::ToolResult(t) => Some(Message::ToolResult {
                tool_call_id: t.tool_call_id.clone(),
                tool_name: t.tool_name.clone(),
                content: t.content.clone(),
                is_error: t.is_error,
                details: t.details.clone(),
                timestamp: t.timestamp,
            }),
            AgentMessage::Custom { .. } => None,
        })
        .collect()
}

/// The mutating lifecycle seam (arch-02 §3.3). All methods have defaults so an implementor only
/// overrides what it needs; the default `convert_to_llm` keeps `user`/`assistant`/`toolResult`.
#[async_trait::async_trait]
pub trait Hooks: Send + Sync {
    /// Runs after `transform_context`; converts `AgentMessage[]` to LLM `Message[]`, dropping
    /// custom roles (func-02 R-02-029/030). Default = [`default_convert_to_llm`].
    async fn convert_to_llm(&self, msgs: &[AgentMessage]) -> Result<Vec<Message>, HookError> {
        Ok(default_convert_to_llm(msgs))
    }

    /// Per-request, BEFORE `convert_to_llm` (func-02 R-02-028). Default = identity.
    async fn transform_context(
        &self,
        msgs: Vec<AgentMessage>,
        _cancel: CancelToken,
    ) -> Result<Vec<AgentMessage>, HookError> {
        Ok(msgs)
    }

    /// After validation, before execute (func-02 R-02-021).
    async fn before_tool_call(
        &self,
        _ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<BeforeOutcome, HookError> {
        Ok(BeforeOutcome::Proceed)
    }

    /// After execute, before `tool_execution_end` (func-02 R-02-025).
    async fn after_tool_call(
        &self,
        _ctx: AfterToolCall<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<AfterOverride>, HookError> {
        Ok(None)
    }

    /// After `turn_end`, before `should_stop_after_turn` (func-02 R-02-031). Next-turn-only.
    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
    ) -> Result<Option<TurnUpdate>, HookError> {
        Ok(None)
    }

    /// After `turn_end` subscribers settle (func-02 R-02-032). `true` => emit `agent_end` & exit.
    async fn should_stop_after_turn(&self, _ctx: PostTurn<'_>) -> Result<bool, HookError> {
        Ok(false)
    }
}

/// All-defaults hooks (standard `convert_to_llm`, identity everything else).
pub struct DefaultHooks;

impl Hooks for DefaultHooks {}
