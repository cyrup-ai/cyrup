//! The block / mutate / notify reducer types (arch-08 §3.3). A single handler returns a
//! [`HookOutcome`]; the dispatcher folds outcomes left-to-right in load order. For `[mutate]`,
//! later handlers observe the folded value (chaining, R-08-011).

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
    Block { reason: Option<String> },
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
    /// `tool_result`: replace-not-merge override of result fields (R-08-011).
    ToolResult { content: Option<Vec<Content>>, details: Option<Value>, is_error: Option<bool> },
    /// `context`: filter/replace the message list.
    Context { messages: Vec<AgentMessage> },
    /// `message_end`: replace the message.
    Message(Box<Message>),
    /// `before_agent_start`: system-prompt replacement + optional injection.
    SystemPromptAndInject { system: Option<String>, inject: Option<Box<Message>> },
    /// `input` (Pi `action:"transform"`, runner.ts:1116-1119): rewrite the submission text and
    /// (optionally) its images. `images: None` keeps the current images (Pi `result.images ??
    /// currentImages`); `Some(_)` replaces them. Folds across handlers — a later handler observes
    /// the rewritten text/images (R-08-011).
    Input { text: String, images: Option<Vec<Content>> },
}

impl HostEvent {
    /// Fold a `[mutate]` patch into this event so the NEXT handler observes it (R-08-011).
    /// A patch whose shape does not match the event is ignored (degrade, never panic — §8).
    pub fn apply_patch(&mut self, patch: EventPatch) {
        match (self, patch) {
            (HostEvent::ToolCall { input, .. }, EventPatch::ToolInput(v)) => *input = v,
            (
                HostEvent::ToolResult { content, details, is_error, .. },
                EventPatch::ToolResult { content: c, details: d, is_error: e },
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
            (HostEvent::Input { text, images }, EventPatch::Input { text: t, images: i }) => {
                *text = t;
                if let Some(i) = i {
                    *images = i;
                }
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
    /// First `Block` wins; carries the reason and the blocking extension id.
    Blocked { reason: Option<String>, by: cyrup_core::ExtensionId },
    /// An extension fully serviced the action.
    Handled(HandledValue),
}
