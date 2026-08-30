//! The MUTATING hook seam (arch-02 §3.3 / func-02 §8).
//!
//! Distinct from the notify-only [`crate::subscriber::EventSubscriber`]. Hooks may rewrite the LLM
//! context, block/mutate tool calls, override tool results, and steer the loop. Each is invoked on
//! the single loop task (never concurrently) and a returned `Err` degrades per the failure-mode map
//! (func-02 R-02-050) rather than panicking.

use crate::error::HookError;
use crate::event::{AgentMessage, ToolResultMessage};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, Message, ModelRef, ModelThinkingLevel, Tool, ToolCall,
    ToolCallId, Usage,
};
use serde_json::Value;
use std::sync::Arc;

/// A read-only view of the loop's live `AgentContext` (Pi `AgentContext`, types.ts:25-30): the
/// active system prompt, the full transcript at the time of the hook, and the tools available to
/// the model. Mirrors the `context` field Pi threads into `beforeToolCall`/`afterToolCall`/
/// `shouldStopAfterTurn`/`prepareNextTurn` (types.ts:96,113,124). Borrowed (no clone) so a hook can
/// inspect the system prompt / tools / messages without the runtime copying the transcript.
pub struct AgentContextView<'a> {
    pub system_prompt: &'a str,
    pub messages: &'a [Arc<AgentMessage>],
    pub tools: &'a [Arc<dyn Tool>],
}

/// Per-call context for [`Hooks::before_tool_call`]. `args` is mutable so a hook may rewrite the
/// arguments in place; mutated args are executed as-is WITHOUT re-validation (func-02 R-02-022).
///
/// Carries the triggering `assistant_message` and the raw `tool_call` block plus a `context` view
/// (Pi `BeforeToolCallContext`, types.ts:88-98) so permission/sub-agent hooks can inspect what
/// requested the call and the surrounding system prompt / tools / transcript.
pub struct BeforeToolCall<'a> {
    pub tool_name: &'a str,
    pub tool_call_id: &'a ToolCallId,
    pub args: &'a mut Value,
    /// The new messages produced so far this run (retained for backward-compat).
    pub messages: &'a [Arc<AgentMessage>],
    /// The assistant message that requested this tool call (Pi `assistantMessage`, types.ts:90).
    pub assistant_message: &'a AssistantMessage,
    /// The raw tool-call block from `assistant_message.content` (Pi `toolCall`, types.ts:92).
    pub tool_call: &'a ToolCall,
    /// The live agent context at preparation time (Pi `context`, types.ts:96).
    pub context: AgentContextView<'a>,
}

/// Outcome of [`Hooks::before_tool_call`] (func-02 R-02-021).
pub enum BeforeOutcome {
    Proceed,
    Block {
        reason: Option<String>,
        /// AGENT-022 — pi `BeforeToolCallResult.terminate`
        /// (`packages/agent/src/types.ts:61-69` @v0.84.1: "Hint that the agent should stop after the
        /// current tool batch when this call is blocked. Early termination only happens when every
        /// finalized tool result in the batch sets this to true"). Consumed at
        /// `agent-loop.ts:636-645` @v0.84.1, which builds the error result and then
        /// `if (beforeResult.terminate === true) { result.terminate = true; }` before returning it,
        /// so the blocked result participates in `shouldTerminateToolBatch` (`agent-loop.ts:582-584`).
        ///
        /// `false` is pi's `undefined`/`false` — the blocked result carries no `terminate` at all.
        terminate: bool,
    },
}

/// Per-call context for [`Hooks::after_tool_call`].
///
/// Carries the triggering `assistant_message`, the raw `tool_call` block, and a `context` view (Pi
/// `AfterToolCallContext`, types.ts:100-114) alongside the executed result fields.
pub struct AfterToolCall<'a> {
    pub tool_name: &'a str,
    pub tool_call_id: &'a ToolCallId,
    pub args: &'a Value,
    pub content: &'a [Content],
    pub details: Option<&'a Value>,
    /// Usage reported by the tool execution itself, if any (Pi `AfterToolCallContext.result.usage`,
    /// types.ts:107 → `AgentToolResult.usage`, types.ts:360-361). Present on the READ side so a
    /// hook can inspect what it is about to replace instead of overwriting blind.
    pub usage: Option<&'a Usage>,
    pub is_error: bool,
    /// The early-termination hint the tool set, if any (Pi `AfterToolCallContext.result.terminate`
    /// → `AgentToolResult.terminate?`, types.ts:354-368). `None` is pi's `undefined` — the tool did
    /// not express an opinion, which is distinct from an explicit `false` (AGENT-009).
    pub terminate: Option<bool>,
    /// The assistant message that requested this tool call (Pi `assistantMessage`, types.ts:102).
    pub assistant_message: &'a AssistantMessage,
    /// The raw tool-call block from `assistant_message.content` (Pi `toolCall`, types.ts:104).
    pub tool_call: &'a ToolCall,
    /// The live agent context at finalization time (Pi `context`, types.ts:113).
    pub context: AgentContextView<'a>,
}

/// Replace-not-merge override returned by [`Hooks::after_tool_call`] (func-02 R-02-025): each
/// `Some(_)` field replaces the whole corresponding result field; `None` keeps the original.
///
/// NB: there is deliberately NO `added_tool_names`. Pi's `AfterToolCallResult` has no such field
/// (types.ts:79-90); `finalizeExecutedToolCall` carries the tool's own value through the
/// `{...result, …}` spread (agent-loop.ts:736-742), so a hook can neither set nor clear it. Adding
/// it here would be a divergence, not a convenience.
#[derive(Clone, Debug, Default)]
pub struct AfterOverride {
    pub content: Option<Vec<Content>>,
    pub details: Option<Value>,
    /// Replaces the tool result's usage in full when `Some` (Pi `AfterToolCallResult.usage`,
    /// types.ts:83-84: "if provided, replaces the tool result usage … There is no deep merge for
    /// `content`, `details`, or `usage`").
    pub usage: Option<Usage>,
    pub is_error: Option<bool>,
    pub terminate: Option<bool>,
}

/// Post-turn context for [`Hooks::prepare_next_turn`] and [`Hooks::should_stop_after_turn`] (Pi
/// `ShouldStopAfterTurnContext` / `PrepareNextTurnContext`, types.ts:116-138).
pub struct PostTurn<'a> {
    /// The new messages this run will return if it exits here (Pi `newMessages`, types.ts:137 —
    /// cyrup's pre-existing field name).
    pub messages: &'a [Arc<AgentMessage>],
    pub turn_index: usize,
    /// The assistant message that completed the turn (Pi `message`, types.ts:119).
    pub message: &'a AssistantMessage,
    /// The tool-result messages passed to the preceding `turn_end` event (Pi `toolResults`,
    /// types.ts:121).
    pub tool_results: &'a [ToolResultMessage],
    /// The live agent context after the turn's messages were appended (Pi `context`, types.ts:124).
    pub context: AgentContextView<'a>,
}

/// Replacement runtime state returned by [`Hooks::prepare_next_turn`] (Pi `AgentLoopTurnUpdate`,
/// types.ts:128-136). Each `Some(_)` field is folded into the run's running baseline and is STICKY:
/// it persists as the default for EVERY later turn in the run (Pi `config = {...config, model,
/// reasoning}` / `currentContext = snapshot.context ?? currentContext`, agent-loop.ts:226-239), not a
/// one-shot. A `None` field keeps the current baseline. `context` replaces the working transcript.
#[derive(Clone, Default)]
pub struct TurnUpdate {
    pub context: Option<Vec<Arc<AgentMessage>>>,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<ModelThinkingLevel>,
    /// Replacement tool set for the rest of the run (Pi `AgentContext.tools`, carried inside the
    /// `context` this hook returns: `context: {...previousContext, systemPrompt, tools:
    /// this.agent.state.tools.slice()}`, agent-session.ts:519-540).
    ///
    /// The loop snapshots the tool array ONCE at run start, the way Pi's `createContextSnapshot`
    /// does. Without a per-turn refresh a tool that becomes active DURING a run — an extension
    /// registering one from a live handler (EXT-004), or a tool calling `setActiveTools` and
    /// reporting the difference as `ToolResult::added_tool_names` (DRIFT-001) — stays uncallable
    /// until the next prompt, which would make a mid-run anchor point at a tool the model cannot
    /// use. cyrup models it as its own field rather than folding it into `context` because
    /// [`Self::context`] here is the message list only, not Pi's whole `AgentContext`.
    pub tools: Option<Vec<Arc<dyn Tool>>>,
    /// Replacement system prompt for the rest of the run (Pi `context.systemPrompt`, same return).
    /// The tool-set rebuild rewrites the prompt (`_rebuildSystemPrompt`, agent-session.ts:2304), so
    /// refreshing tools without it would leave the run advertising a tool whose guidance is missing.
    pub system_prompt: Option<String>,
}

/// Hand-written because `Arc<dyn Tool>` is not `Debug` (`Tool: Send + Sync` only) — tools print as
/// their names, which is the only part of them a diagnostic wants.
impl std::fmt::Debug for TurnUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnUpdate")
            .field("context", &self.context)
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field(
                "tools",
                &self
                    .tools
                    .as_ref()
                    .map(|ts| ts.iter().map(|t| t.name().to_string()).collect::<Vec<_>>()),
            )
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

/// The default `convert_to_llm`: keep only `user`/`assistant`/`toolResult`, drop `Custom`
/// (func-02 R-02-029/052).
pub fn default_convert_to_llm(msgs: &[Arc<AgentMessage>]) -> Vec<Message> {
    msgs.iter()
        .filter_map(|m| match m.as_ref() {
            AgentMessage::User { content, timestamp } => {
                Some(Message::User { content: content.clone(), timestamp: timestamp.unwrap_or(0) })
            }
            AgentMessage::Assistant(a) => Some(Message::Assistant((**a).clone())),
            AgentMessage::ToolResult(t) => Some(Message::ToolResult {
                tool_call_id: t.tool_call_id.clone(),
                tool_name: t.tool_name.clone(),
                content: t.content.clone(),
                is_error: t.is_error,
                details: t.details.clone(),
                // Both must cross the agent→LLM boundary: `usage` so downstream accounting can see
                // it, `added_tool_names` because a provider with native deferred tool loading reads
                // it off the REQUEST transcript to place the definition (Pi keeps both on the single
                // `ToolResultMessage` that IS the LLM message, ai/src/types.ts:415-431).
                usage: t.usage.clone(),
                added_tool_names: t.added_tool_names.clone(),
                timestamp: t.timestamp,
            }),
            AgentMessage::Custom { .. } => None,
            // SESS-043 — a declaration-merged coding-agent role. pi's BASE `defaultConvertToLlm`
            // (`packages/agent/src/harness/messages.ts:120` @v0.83.0) likewise keeps only the three
            // LLM roles; the app supplies its own `convertToLlm` to render the merged ones, which
            // for cyrup is `PolicyHooks::convert_to_llm` (`cyrup-session-svc/src/hooks.rs`).
            AgentMessage::App { .. } => None,
        })
        .collect()
}

/// The mutating lifecycle seam (arch-02 §3.3). All methods have defaults so an implementor only
/// overrides what it needs; the default `convert_to_llm` keeps `user`/`assistant`/`toolResult`.
#[async_trait::async_trait]
pub trait Hooks: Send + Sync {
    /// Runs after `transform_context`; converts `AgentMessage[]` to LLM `Message[]`, dropping
    /// custom roles (func-02 R-02-029/030). Default = [`default_convert_to_llm`].
    async fn convert_to_llm(&self, msgs: &[Arc<AgentMessage>]) -> Result<Vec<Message>, HookError> {
        Ok(default_convert_to_llm(msgs))
    }

    /// Per-request, BEFORE `convert_to_llm` (func-02 R-02-028). Default = identity.
    async fn transform_context(
        &self,
        msgs: Vec<Arc<AgentMessage>>,
        _cancel: CancelToken,
    ) -> Result<Vec<Arc<AgentMessage>>, HookError> {
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

    /// After `turn_end`, before `should_stop_after_turn` (Pi `prepareNextTurn`, agent-loop.ts:226).
    /// A returned [`TurnUpdate`] is STICKY: it becomes the new running baseline for all later turns.
    ///
    /// AGENT-024 — `_cancel` is the run's abort signal. pi's *loop*-level `prepareNextTurn`
    /// (`packages/agent/src/types.ts:229-231`) takes no signal, but the Agent-options layer above it
    /// binds one into the closure it hands the loop:
    /// `prepareNextTurn: async (context) => { if (this.prepareNextTurnWithContext) { return await
    /// this.prepareNextTurnWithContext(context, this.signal); } return await
    /// this.prepareNextTurn?.(this.signal); }` (`packages/agent/src/agent.ts:463-471` @v0.84.1;
    /// identical `this.signal` argument at v0.83.0, so this half is not drift). cyrup has no
    /// separate options wrapper, so the run's token enters here — the loop passes
    /// `self.cancel.child()`, the same shape as `before_tool_call`/`after_tool_call`.
    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        Ok(None)
    }

    /// After `turn_end` subscribers settle (func-02 R-02-032). `true` => emit `agent_end` & exit.
    ///
    /// AGENT-024 — `_cancel` is the run's abort signal, bound the same way pi's Agent-options layer
    /// binds it: `shouldStopAfterTurn: shouldStopAfterTurn ? async (context) => await
    /// shouldStopAfterTurn(context, this.signal) : undefined` (`agent.ts:460-462` @v0.84.1, with the
    /// `AgentOptions.shouldStopAfterTurn` field at `:108` and the public field at `:193-196`).
    async fn should_stop_after_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<bool, HookError> {
        Ok(false)
    }
}

/// All-defaults hooks (standard `convert_to_llm`, identity everything else).
pub struct DefaultHooks;

impl Hooks for DefaultHooks {}
