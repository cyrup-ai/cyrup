//! The message & content model (arch-00 §3.3; conformance: func-01 §4).
//!
//! Serde follows arch-00 §4: structs use `rename_all = "camelCase"`; tagged enums add
//! `rename_all_fields = "camelCase"` so payload fields are camelCase for Pi-interop (R-00-013).
//!
//! Split by concern; every public item is re-exported here, so `cyrup_core::message::X` and
//! `cyrup_core::X` resolve exactly as they did when this was one file:
//!
//! - `thinking` — the reasoning-effort ladder ([`ThinkingLevel`], [`ModelThinkingLevel`]).
//! - `stop_reason` — how a generation settled ([`StopReason`]).
//! - `text_signature` — the structured text-signature payload ([`TextPhase`],
//!   [`TextSignatureV1`]).
//! - `tool_call` — the self-tagging [`ToolCall`].
//! - `content` — the typed [`Content`] block and the per-role content deserializers.
//! - `usage` — token + cost accounting ([`Usage`], [`Cost`]).
//! - `assistant` — [`AssistantMessage`], [`DeferredHandle`], [`UNRESOLVED_API`].
//! - `conversation` — the role-tagged [`Message`] enum.

mod assistant;
mod content;
mod conversation;
mod stop_reason;
mod text_signature;
mod thinking;
mod tool_call;
mod usage;

pub use assistant::{AssistantMessage, DeferredHandle, UNRESOLVED_API};
pub use content::Content;
pub use conversation::Message;
pub use stop_reason::StopReason;
pub use text_signature::{TextPhase, TextSignatureV1};
pub use thinking::{ModelThinkingLevel, ThinkingLevel};
pub use tool_call::ToolCall;
pub use usage::{Cost, Usage};
