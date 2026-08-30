//! The block / mutate / notify reducer types (arch-08 §3.3). A single handler returns a
//! [`HookOutcome`]; the dispatcher folds outcomes left-to-right in load order. For `[mutate]`,
//! later handlers observe the folded value (chaining, R-08-011).

use std::sync::Arc;
use crate::event::HostEvent;
use cyrup_agent::AgentMessage;
use cyrup_core::{Content, Message};
use serde_json::Value;

/// What a single handler contributes (arch-08 §3.3).
#[derive(Clone, Debug)]
pub enum HookOutcome {
    /// notify-only events; return ignored (R-08-009).
    Noop,
    /// `[block]` — short-circuits the action with an optional reason. First block wins.
    Block {
        reason: Option<String>,
        /// `tool_call` only (EXT-049) — pi `ToolCallEventResult.terminate`
        /// (`pi/packages/coding-agent/src/core/extensions/types.ts:1072-1079` @v0.84.1, ABSENT at
        /// the ported v0.83.0 baseline): "Hint that the agent should stop after the current tool
        /// batch when this call is blocked. Early termination only happens when every finalized
        /// tool result in the batch sets this to true." Consumed at
        /// `packages/agent/src/agent-loop.ts:636-646`, folded by `shouldTerminateToolBatch` at
        /// `:583` into `hasMoreToolCalls = !executedToolBatch.terminate` at `:216` — the every()
        /// rule lives in the agent, not here, so a single blocking handler setting this does NOT
        /// end the run on its own.
        ///
        /// `false` is pi's `undefined`/`false`. Ignored on every non-`tool_call` seam, exactly as
        /// upstream ignores it (no other `*EventResult` declares the field).
        terminate: bool,
    },
    /// `[mutate]` — a typed patch applied to the in-flight value.
    Mutate(EventPatch),
    /// `input`/`user_bash` "handled"/"provide": the extension fully serviced it.
    Handled(HandledValue),
}

/// A fully-serviced result (arch-08 §3.3). Open-shaped; carried as JSON.
#[derive(Clone, Debug)]
pub struct HandledValue(pub Value);

/// Typed, event-specific patch payloads (arch-08 §3.3). `serde_json::Value` only for genuinely
/// open fields (tool args, custom payloads); fixed shapes stay typed.
#[derive(Clone, Debug)]
pub enum EventPatch {
    /// `tool_call`: rewrite the tool input (R-08-010).
    ToolInput(Value),
    /// `tool_result`: replace-not-merge override of result fields (R-08-011). `usage` mirrors Pi
    /// `ToolResultEventResult.usage` (types.ts:1085-1090): `Some` REPLACES the tool's usage in
    /// full — there is no deep merge (types.ts:70-78).
    ToolResult {
        content: Option<Vec<Content>>,
        details: Option<Value>,
        is_error: Option<bool>,
        usage: Option<cyrup_core::Usage>,
    },
    /// `context`: filter/replace the message list.
    Context { messages: Vec<Arc<AgentMessage>> },
    /// `message_end`: replace the message.
    Message(Box<Message>),
    /// `before_agent_start`: system-prompt replacement + optional injection.
    SystemPromptAndInject { system: Option<String>, inject: Option<Box<Message>> },
    /// `input` (Pi `action:"transform"`, runner.ts:1116-1119): rewrite the submission text and
    /// (optionally) its images. `images: None` keeps the current images (Pi `result.images ??
    /// currentImages`); `Some(_)` replaces them. Folds across handlers — a later handler observes
    /// the rewritten text/images (R-08-011).
    Input { text: String, images: Option<Vec<Content>> },
    /// `before_provider_request` (Pi runner.ts:946-978): a handler's return value REPLACES the
    /// outbound payload wholesale (`currentPayload = handlerResult`); later handlers observe the
    /// replacement. Open-shaped: the provider request body crosses as `serde_json::Value`.
    ProviderRequest(Value),
    /// `before_provider_headers` (EXT-009; pi `BeforeProviderHeadersEvent`,
    /// extensions/types.ts:686-689 @v0.83.0). Upstream handlers "mutate `headers` in place … the
    /// return value is ignored. A `null` value deletes that header" (:681-685), so this is a
    /// PATCH object rather than a replacement: each key is set to its value, and a key whose value
    /// is `null` is REMOVED. That asymmetry is the whole point of the event — a proxy or auth-shim
    /// extension deletes a header it must not send, and setting it to `""` would still send it.
    ProviderHeaders(Value),
    /// `session_before_compact` (Pi `SessionBeforeCompactResult.compaction`, types.ts:1079): an
    /// extension-supplied compaction override (a `CompactionResult`: `{summary, firstKeptEntryId?,
    /// tokensBefore?, details?}`). The LAST override wins across the chain; the producer threads its
    /// `summary`/`details` into the appended compaction entry (marked `fromExtension`).
    CompactionOverride(Value),
    /// `session_before_tree` (Pi `SessionBeforeTreeResult`, types.ts:1082-1094): an extension-supplied
    /// summary/customInstructions/label override for the branch summarization. Open-shaped.
    TreeOverride(Value),
}

impl HostEvent {
    /// Fold a `[mutate]` patch into this event so the NEXT handler observes it (R-08-011).
    /// A patch whose shape does not match the event is ignored (degrade, never panic — §8).
    pub fn apply_patch(&mut self, patch: EventPatch) {
        match (self, patch) {
            (HostEvent::ToolCall { input, .. }, EventPatch::ToolInput(v)) => *input = v,
            (
                HostEvent::ToolResult { content, details, is_error, usage, .. },
                EventPatch::ToolResult { content: c, details: d, is_error: e, usage: u },
            ) => {
                if let Some(c) = c {
                    *content = c;
                }
                if d.is_some() {
                    *details = d;
                }
                if let Some(e) = e {
                    *is_error = e;
                }
                // Pi `ToolResultEventResult.usage` (types.ts:1088): an omitted key keeps the
                // current value, a present one REPLACES it in full (no deep merge, types.ts:70-78).
                if u.is_some() {
                    *usage = u;
                }
            }
            (HostEvent::Context { messages }, EventPatch::Context { messages: m }) => *messages = m,
            // `message_end` (Pi runner.ts:785): a replacement message MUST keep the same role; a
            // mismatched role is rejected (the replacement is dropped, the original kept) — no panic.
            (HostEvent::MessageEnd { message }, EventPatch::Message(m)) => {
                if message_role(message) == message_role(&m) {
                    *message = *m;
                }
            }
            // `before_agent_start` (Pi runner.ts:980): replace the system prompt AND/OR accumulate
            // an injected message across the handler chain.
            (
                HostEvent::BeforeAgentStart { system_prompt, injected, .. },
                EventPatch::SystemPromptAndInject { system, inject },
            ) => {
                if let Some(s) = system {
                    *system_prompt = s;
                }
                if let Some(m) = inject {
                    injected.push(*m);
                }
            }
            // `input` (Pi runner.ts:1116-1119): always rewrite the text; replace images only when
            // the handler supplied them (`Some`), else keep the folded-so-far images.
            (HostEvent::Input { text, images, .. }, EventPatch::Input { text: t, images: i }) => {
                *text = t;
                if let Some(i) = i {
                    *images = i;
                }
            }
            // `before_provider_request` (Pi runner.ts:962): the handler's return value REPLACES the
            // payload wholesale; the next handler sees the replacement.
            (HostEvent::BeforeProviderRequest { payload }, EventPatch::ProviderRequest(v)) => {
                *payload = v;
            }
            // `before_provider_headers` (pi types.ts:681-685): in-place mutation semantics — set
            // each supplied key, DELETE the ones whose value is `null`. A non-object patch is
            // ignored (degrade, never panic).
            (HostEvent::BeforeProviderHeaders { headers }, EventPatch::ProviderHeaders(v)) => {
                if let (Some(dst), Some(src)) = (headers.as_object_mut(), v.as_object()) {
                    for (k, val) in src {
                        if val.is_null() {
                            dst.remove(k);
                        } else {
                            dst.insert(k.clone(), val.clone());
                        }
                    }
                }
            }
            // `session_before_compact` (Pi `SessionBeforeCompactResult.compaction`): capture the
            // extension-supplied compaction override on the event so the producer folds it back.
            (
                HostEvent::SessionBeforeCompact { override_result, .. },
                EventPatch::CompactionOverride(v),
            ) => *override_result = Some(v),
            // `session_before_tree` (Pi `SessionBeforeTreeResult`): capture the summary/label override.
            (HostEvent::SessionBeforeTree { override_result, .. }, EventPatch::TreeOverride(v)) => {
                *override_result = Some(v)
            }
            // Shape mismatch: ignore (degrade gracefully).
            _ => {}
        }
    }
}

/// The role discriminant of an LLM message (for the `message_end` same-role rule, R-08-011).
fn message_role(m: &Message) -> &'static str {
    match m {
        Message::User { .. } => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult { .. } => "toolResult",
    }
}

/// The reduced result of dispatching a `[block]`/`[mutate]` event (arch-08 §6.1).
#[derive(Debug)]
pub enum Reduced {
    /// No block; the (possibly folded) event proceeds. Boxed: `HostEvent` is much larger than the
    /// other variants, so boxing keeps `Reduced` small (clippy::large_enum_variant).
    Pass(Box<HostEvent>),
    /// First `Block` wins; carries the reason, the blocking extension id, and (on `tool_call`
    /// only) pi's `ToolCallEventResult.terminate` hint — see [`HookOutcome::Block::terminate`].
    Blocked { reason: Option<String>, terminate: bool, by: cyrup_core::ExtensionId },
    /// An extension fully serviced the action.
    Handled(HandledValue),
}

/// One terminal-input handler's answer (EXT-021; pi `TerminalInputHandler`'s return,
/// `packages/coding-agent/src/core/extensions/types.ts:113` @v0.83.0:
/// `{ consume?: boolean; data?: string } | undefined`).
///
/// Both members stay `Option` because upstream's fold (`packages/tui/src/tui.ts:773-788`) tests
/// `result?.consume` (truthy) and `result?.data !== undefined` — so `{data: ""}` REWRITES the
/// buffer to empty (and the keystroke is then dropped by the end-of-fold length check at `:784`),
/// while `{}` leaves it alone. Collapsing either to a bare `bool`/`String` would erase that
/// distinction.
///
/// A `None` return from a handler is upstream's `undefined`: "I looked at it and did nothing".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalInputResult {
    pub consume: Option<bool>,
    pub data: Option<String>,
}

/// What the host tells its caller to do with one raw terminal-input chunk, after folding every
/// subscriber (EXT-021). The Rust shape of pi's `TUI.handleInput` outcome
/// (`packages/tui/src/tui.ts:773-788`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalInputDecision {
    /// Deliver `data` to the editor. Equal to the input when no handler rewrote it.
    Deliver(String),
    /// Drop the keystroke entirely — either a handler returned `consume: true` (`:777-779`) or the
    /// fold ended with an empty string (`:784-786`).
    Consume,
}
