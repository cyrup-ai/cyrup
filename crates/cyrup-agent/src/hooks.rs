//! The MUTATING hook seam (arch-02 §3.3 / func-02 §8).
//!
//! Distinct from the notify-only [`crate::subscriber::EventSubscriber`]. Hooks may rewrite the LLM
//! context, block/mutate tool calls, override tool results, and steer the loop. Each is invoked on
//! the single loop task (never concurrently) and a returned `Err` degrades per the failure-mode map
//! (func-02 R-02-050) rather than panicking.

use crate::error::HookError;
use crate::event::{AgentMessage, ToolResultMessage};
use cyrup_core::{
    AssistantMessage, CancelToken, Content, Message, ModelRef, ModelThinkingLevel, TerminateHint,
    Tool, ToolCall, ToolCallId, ToolLoadout, Usage,
};
use serde_json::Value;
use std::sync::Arc;

/// A read-only view of the loop's live `AgentContext` (Pi `AgentContext`, types.ts:25-30): the
/// active system prompt, the full transcript at the time of the hook, and the tools available to
/// the model. Mirrors the `context` field Pi threads into `beforeToolCall`/`afterToolCall`/
/// `finishTurn`/`prepareNextTurn`/`prepareRequest` (`types.ts` `AgentTurnContext.context`,
/// `PrepareRequestContext.context` @v0.87.1). Borrowed (no clone) so a hook can inspect the system
/// prompt / tools / messages without the runtime copying the transcript.
///
/// `Copy`: every field is a shared reference, so the view is a fat-pointer bundle. AGENT-047 needs
/// to hand the SAME context to `before_tool_call` and then to `after_tool_call` out of one stored
/// narrowing, which is what upstream's `currentContext` parameter is (`agent-loop.ts:708`, `:853`
/// @v1.0.1).
#[derive(Clone, Copy)]
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

/// Outcome of [`Hooks::before_tool_call`] (func-02 R-02-021). Every variant is an EXPECTED per-call
/// outcome: none of them aborts the run. pi's `beforeToolCall` returns `BeforeToolCallResult |
/// undefined` (`packages/agent/src/types.ts:61-69`, `:277`), and a throw is caught per call
/// (`agent-loop.ts:657-662` @v0.83.0) into an error tool result — which is why [`Self::Failed`]
/// is a variant here and not a `Result::Err`: the four run-aborting hooks keep `Result`, and the
/// signature now says which is which.
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
        /// [`TerminateHint::Unspecified`] is pi's `undefined` — the blocked result carries no
        /// `terminate` key at all.
        terminate: TerminateHint,
    },
    /// The hook itself failed. The loop degrades exactly as for a pi `beforeToolCall` throw: an
    /// error tool result carrying the error's own text (`createErrorToolResult(error.message)`,
    /// `agent-loop.ts:657-662`), no `terminate`, no abort check first (func-02 R-02-050).
    Failed(HookError),
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
    /// → `AgentToolResult.terminate?`, types.ts:354-368). [`TerminateHint::Unspecified`] is pi's
    /// `undefined` — the tool did not express an opinion, which is distinct from an explicit
    /// [`TerminateHint::Continue`] (AGENT-009).
    pub terminate: TerminateHint,
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
    /// Replaces [`cyrup_core::ToolResult::structured_content`] when `Some` (Pi
    /// `AfterToolCallResult.structuredContent`, `agent/src/types.ts:85-88,95` @v1.0.0).
    ///
    /// `None` does NOT simply mean "keep": upstream's rule is *"if `content` is provided without
    /// it, the structured content is dropped, because it may no longer match the content. Return
    /// it along with `content` to keep it."* So the three cases are decided jointly with
    /// [`Self::content`], and the fold implements that — see `fold_tool_outcome`. AGENT-045.
    pub structured_content: Option<Value>,
    /// Replaces the tool result's usage in full when `Some` (Pi `AfterToolCallResult.usage`,
    /// types.ts:83-84: "if provided, replaces the tool result usage … There is no deep merge for
    /// `content`, `details`, or `usage`").
    pub usage: Option<Usage>,
    pub is_error: Option<bool>,
    /// `None` = the hook has no opinion and the tool's own hint stands (pi
    /// `afterResult.terminate ?? result.terminate`, agent-loop.ts:739). `Some(hint)` replaces it —
    /// including `Some(TerminateHint::Unspecified)`, which CLEARS a hint the tool set. That is the
    /// distinction a plain `Option<bool>` could not draw.
    pub terminate: Option<TerminateHint>,
}

/// Outcome of [`Hooks::after_tool_call`] (func-02 R-02-025). pi's `afterToolCall` returns
/// `AfterToolCallResult | undefined` (`types.ts:84-93`, `:292`) and a throw is caught per call
/// (`agent-loop.ts:747-750`) — three outcomes, all expected, none run-aborting.
pub enum AfterOutcome {
    /// `undefined` upstream: the tool's own result stands.
    Keep,
    /// Replace-not-merge per field (`afterResult.x ?? result.x`, `agent-loop.ts:738-745`).
    ///
    /// Boxed for the same reason as `Prep::Immediate`: [`AfterOverride`] carries six optional
    /// replacement fields and dwarfs both `Keep` (empty) and `Failed` (one error), so an unboxed
    /// variant makes the COMMON `Keep` answer — what every hook-less call and every hook with no
    /// opinion returns — pay for the largest one. AGENT-045's `structured_content` is what tipped
    /// it past the lint.
    Override(Box<AfterOverride>),
    /// The hook itself failed: the WHOLE result becomes an error result carrying the error's own
    /// text, with `usage` and `added_tool_names` dropped and `terminate` cleared
    /// (`createErrorToolResult(error.message)`, `agent-loop.ts:747-750`; R-02-050).
    Failed(HookError),
}

/// Completed-turn context for [`Hooks::finish_turn`] and [`Hooks::prepare_next_turn`] (Pi
/// `AgentTurnContext` / `PrepareNextTurnContext extends AgentTurnContext`, `types.ts` @v0.87.1).
pub struct PostTurn<'a> {
    /// The new messages this run will return if it exits here (Pi `newMessages` — cyrup's
    /// pre-existing field name).
    pub messages: &'a [Arc<AgentMessage>],
    /// How many turns this run has completed, this one included.
    pub turn_index: usize,
    /// The assistant message that completed the turn (Pi `message`).
    pub message: &'a AssistantMessage,
    /// The turn's tool-result messages — the ones its `turn_end` event carries (Pi `toolResults`).
    pub tool_results: &'a [ToolResultMessage],
    /// The live agent context after the turn's assistant message and tool results were appended
    /// (Pi `context`).
    pub context: AgentContextView<'a>,
}

/// What [`Hooks::finish_turn`] decided (Pi `AgentTurnDecision`, `types.ts` @v0.87.1). Returning
/// `None` from the hook is pi's `undefined`: normal scheduling.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TurnDecision {
    /// Ensure one more provider request. A tool-result, steering or follow-up request satisfies it
    /// and adds nothing; otherwise the loop makes one request on the current context.
    Continue,
    /// End the run right after this turn's `turn_end`, without polling the queues or preparing
    /// another request.
    End,
}

/// Per-request context for [`Hooks::prepare_request`] (Pi `PrepareRequestContext`, `types.ts`
/// @v0.87.1): the runtime state the next provider request would use as things stand.
pub struct PrepareRequestCtx<'a> {
    /// The loop's working context, with this turn's pending messages already appended.
    pub context: AgentContextView<'a>,
    pub model: &'a ModelRef,
    /// Pi `thinkingLevel: config.reasoning ?? "off"` — cyrup's level is never absent.
    pub thinking_level: ModelThinkingLevel,
}

/// Replacement runtime state returned by [`Hooks::prepare_request`] (Pi `AgentRequestUpdate =
/// Omit<AgentLoopTurnUpdate, "messages">`, `types.ts` @v0.87.1). Folded into the run baseline
/// exactly like a [`TurnUpdate`], so each `Some(_)` field applies to this request AND every later
/// one in the run. `tools`/`system_prompt` are the parts of pi's `context` cyrup carries as their
/// own fields — see [`TurnUpdate::tools`].
#[derive(Clone, Default)]
pub struct RequestUpdate {
    pub context: Option<Vec<Arc<AgentMessage>>>,
    pub model: Option<ModelRef>,
    pub thinking_level: Option<ModelThinkingLevel>,
    pub tools: Option<ToolLoadout>,
    pub system_prompt: Option<String>,
}

/// Replacement runtime state returned by [`Hooks::prepare_next_turn`] (Pi `AgentLoopTurnUpdate`,
/// `types.ts` @v0.87.1). Each `Some(_)` field is folded into the run's running baseline and is
/// STICKY: it persists as the default for EVERY later turn in the run (Pi `config = {...config,
/// model, reasoning}` / `currentContext = snapshot.context ?? currentContext`, `agent-loop.ts:186-198`
/// @v0.87.1), not a one-shot. A `None` field keeps the current baseline. `context` replaces the
/// working transcript.
#[derive(Clone, Default)]
pub struct TurnUpdate {
    pub context: Option<Vec<Arc<AgentMessage>>>,
    /// Messages to append before the next provider request, ahead of any queued steering or
    /// follow-up, each with the normal `message_start`/`message_end` pair (Pi
    /// `AgentLoopTurnUpdate.messages`, added v0.87.0; `agent-loop.ts:188`, `:209-214` @v0.87.1).
    /// One-shot, unlike every other field: they are appended once, not folded into a baseline.
    pub messages: Vec<AgentMessage>,
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
    pub tools: Option<ToolLoadout>,
    /// Replacement system prompt for the rest of the run (Pi `context.systemPrompt`, same return).
    /// The tool-set rebuild rewrites the prompt (`_rebuildSystemPrompt`, agent-session.ts:2304), so
    /// refreshing tools without it would leave the run advertising a tool whose guidance is missing.
    pub system_prompt: Option<String>,
}

impl TurnUpdate {
    /// Split into the baseline fold (the fields pi's `AgentRequestUpdate` shares with
    /// `AgentLoopTurnUpdate`) and the one-shot messages to append.
    pub(crate) fn into_parts(self) -> (RequestUpdate, Vec<AgentMessage>) {
        let TurnUpdate {
            context,
            messages,
            model,
            thinking_level,
            tools,
            system_prompt,
        } = self;
        (
            RequestUpdate {
                context,
                model,
                thinking_level,
                tools,
                system_prompt,
            },
            messages,
        )
    }
}

fn tool_names(tools: Option<&ToolLoadout>) -> Option<Vec<String>> {
    tools.map(|ts| ts.executable().iter().map(|t| t.name().to_string()).collect())
}

/// Hand-written because `Arc<dyn Tool>` is not `Debug` (`Tool: Send + Sync` only) — tools print as
/// their names, which is the only part of them a diagnostic wants.
impl std::fmt::Debug for TurnUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TurnUpdate")
            .field("context", &self.context)
            .field("messages", &self.messages)
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("tools", &tool_names(self.tools.as_ref()))
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

/// Same reason as [`TurnUpdate`]'s: tools print as their names.
impl std::fmt::Debug for RequestUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RequestUpdate")
            .field("context", &self.context)
            .field("model", &self.model)
            .field("thinking_level", &self.thinking_level)
            .field("tools", &tool_names(self.tools.as_ref()))
            .field("system_prompt", &self.system_prompt)
            .finish()
    }
}

/// The default `convert_to_llm`: keep only `system`/`user`/`assistant`/`toolResult`, drop `Custom`
/// (func-02 R-02-029/052).
///
/// PROV-083a added the `system` role. pi's `defaultConvertToLlm` is a four-way filter that names it
/// FIRST — `message.role === "system" || … "user" || … "assistant" || … "toolResult"`
/// (`packages/agent/src/agent.ts:38-46` @v0.87.1) — and so does the coding-agent's own override
/// (`coding-agent/src/core/messages.ts:184-188`, `packages/agent/src/harness/messages.ts:159-163`).
/// A system message IS an LLM message: it carries the prompt and the tool declarations.
pub fn default_convert_to_llm(msgs: &[Arc<AgentMessage>]) -> Vec<Message> {
    msgs.iter()
        .filter_map(|m| match m.as_ref() {
            AgentMessage::System(s) => Some(Message::System(s.clone())),
            AgentMessage::User { content, timestamp } => Some(Message::User {
                content: content.clone(),
                timestamp: timestamp.unwrap_or(0),
            }),
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

    /// After validation, before execute (func-02 R-02-021). Cannot abort the run — every outcome,
    /// [`BeforeOutcome::Failed`] included, is a per-call result (func-02 R-02-050).
    async fn before_tool_call(
        &self,
        _ctx: BeforeToolCall<'_>,
        _cancel: CancelToken,
    ) -> BeforeOutcome {
        BeforeOutcome::Proceed
    }

    /// After execute, before `tool_execution_end` (func-02 R-02-025). Cannot abort the run — every
    /// outcome, [`AfterOutcome::Failed`] included, is a per-call result (func-02 R-02-050).
    async fn after_tool_call(&self, _ctx: AfterToolCall<'_>, _cancel: CancelToken) -> AfterOutcome {
        AfterOutcome::Keep
    }

    /// After the tool batch and every tool-result `message_end`, immediately BEFORE `turn_end` (Pi
    /// `finishTurn`, added v0.87.0 in place of `shouldStopAfterTurn`; awaited at
    /// `agent-loop.ts:285` @v0.87.1). [`TurnDecision::End`] ends the run after `turn_end` without
    /// polling the queues or preparing another request; [`TurnDecision::Continue`] ensures one more
    /// provider request; `None` keeps normal scheduling.
    ///
    /// Also called on an errored or aborted assistant turn (`:251`), whose decision is ignored:
    /// those remain hard exits. An `Err` is pi's bare-await throw — the run fails through
    /// `handleRunFailure`.
    ///
    /// `_cancel` is the run's abort signal (pi passes `signal` as the second argument).
    ///
    /// Host/extension seam with no in-tree producer yet: `PolicyHooks` only forwards it, and pi's
    /// producer — the coding-agent's actionable `turn_end`/`agent_before_settle` extension
    /// boundaries (`_installAgentBoundaryHooks`, `agent-session.ts:675-684` @v0.87.1, EXT-078) —
    /// is not ported.
    async fn finish_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnDecision>, HookError> {
        Ok(None)
    }

    /// After `turn_end`, only when the loop will run another turn, immediately before that turn's
    /// `turn_start` (Pi `prepareNextTurn`, `agent-loop.ts:184-205` @v0.87.1 — moved there from
    /// straight after `turn_end` in v0.84.2). A returned [`TurnUpdate`] is STICKY: it becomes the new
    /// running baseline for all later turns; its [`TurnUpdate::messages`] are appended once.
    ///
    /// AGENT-024 — `_cancel` is the run's abort signal. pi's *loop*-level `prepareNextTurn`
    /// (`packages/agent/src/types.ts:229-231`) takes no signal, but the Agent-options layer above it
    /// binds one into the closure it hands the loop:
    /// `prepareNextTurn: async (context) => { if (this.prepareNextTurnWithContext) { return await
    /// this.prepareNextTurnWithContext(context, this.signal); } return await
    /// this.prepareNextTurn?.(this.signal); }` (`packages/agent/src/agent.ts:480-487` @v0.87.1).
    /// cyrup has no separate options wrapper, so the run's token enters here — the loop passes
    /// `self.cancel.child()`, the same shape as `before_tool_call`/`after_tool_call`.
    async fn prepare_next_turn(
        &self,
        _ctx: PostTurn<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<TurnUpdate>, HookError> {
        Ok(None)
    }

    /// Immediately before EVERY provider request, including the run's first, after that turn's
    /// pending messages were appended and emitted (Pi `prepareRequest`, added v0.87.0;
    /// `agent-loop.ts:218-238` @v0.87.1). A returned [`RequestUpdate`] replaces the runtime values
    /// for this and every later request in the run. It does not poll the queues: steering queued
    /// while it runs waits for the next turn.
    ///
    /// Host/extension seam with no in-tree producer yet: `PolicyHooks` only forwards it, and pi's
    /// producer — the coding-agent's canonical-context request projection
    /// (`_installAgentRequestProjection`, `agent-session.ts:608-632` @v0.87.1) — is not ported.
    async fn prepare_request(
        &self,
        _ctx: PrepareRequestCtx<'_>,
        _cancel: CancelToken,
    ) -> Result<Option<RequestUpdate>, HookError> {
        Ok(None)
    }
}

/// All-defaults hooks (standard `convert_to_llm`, identity everything else).
pub struct DefaultHooks;

impl Hooks for DefaultHooks {}
