//! Typed event payloads + per-event result shapes (arch-08 §3.3; Pi extensions/types.ts:503-1085).
//! A handler receives a typed payload (parsed from the host's JSON seam) and a [`crate::Ctx`], and
//! returns an [`crate::Outcome`]. The multi-field Pi results (`before_agent_start`, `tool_result`)
//! are expressed via the dedicated patch structs here, surfaced through `Outcome` constructors.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `tool_call` (Pi types.ts:1145) — block/mutate the in-flight tool call.
#[derive(Clone, Debug)]
pub struct ToolCallEvent {
    pub call_id: String,
    pub name: String,
    pub input: Value,
}

/// `tool_result` (Pi types.ts:1146) — mutate the result.
#[derive(Clone, Debug)]
pub struct ToolResultEvent {
    pub call_id: String,
    pub name: String,
    pub content: Value,
    pub is_error: bool,
}

/// `context` (Pi types.ts:1144) — filter/replace the LLM message list.
#[derive(Clone, Debug)]
pub struct ContextEvent {
    pub messages: Value,
}

/// `message_end` (Pi types.ts:1143) — replace the just-finished message (same role).
#[derive(Clone, Debug)]
pub struct MessageEndEvent {
    pub message: Value,
}

/// `before_agent_start` (Pi types.ts:1135) — inject a message and/or replace the system prompt.
#[derive(Clone, Debug)]
pub struct BeforeAgentStartEvent {
    pub prompt: String,
    pub images: Value,
    pub system_prompt: String,
    pub options: Value,
}

/// `input` (Pi types.ts:1158) — transform/handle a user input line.
#[derive(Clone, Debug)]
pub struct InputEvent {
    pub text: String,
}

/// `user_bash` (Pi types.ts:1159) — block/transform/provide a `!`/`!!` bash invocation.
#[derive(Clone, Debug)]
pub struct UserBashEvent {
    pub command: String,
    pub operations: Value,
}

/// `before_provider_request` (Pi types.ts:1160) — mutate the outbound provider payload.
#[derive(Clone, Debug)]
pub struct BeforeProviderRequestEvent {
    pub payload: Value,
}

/// `after_provider_response` (Pi types.ts:1161) — notify with HTTP status + headers.
#[derive(Clone, Debug)]
pub struct AfterProviderResponseEvent {
    pub status: u32,
    pub headers: Value,
}

/// `model_select` (Pi types.ts:1162) — notify of a model change.
#[derive(Clone, Debug)]
pub struct ModelSelectEvent {
    pub model: Value,
}

/// `thinking_level_select` (Pi types.ts:1163).
#[derive(Clone, Debug)]
pub struct ThinkingLevelSelectEvent {
    pub level: String,
}

/// `agent_end` (Pi types.ts:1138) — notify with the full final message list.
#[derive(Clone, Debug)]
pub struct AgentEndEvent {
    pub messages: Value,
}

/// `turn_start` (Pi types.ts:1139).
#[derive(Clone, Debug)]
pub struct TurnStartEvent {
    pub turn_index: u32,
}

/// `turn_end` (Pi types.ts:1140).
#[derive(Clone, Debug)]
pub struct TurnEndEvent {
    pub turn_index: u32,
    pub message: Value,
}

/// `message_start` (Pi types.ts:1141).
#[derive(Clone, Debug)]
pub struct MessageStartEvent {
    pub role: String,
}

/// `message_update` (Pi types.ts:1142) — HIGH-FREQ assistant delta.
#[derive(Clone, Debug)]
pub struct MessageUpdateEvent {
    pub delta: Value,
}

/// `tool_execution_start` (Pi types.ts:1147).
#[derive(Clone, Debug)]
pub struct ToolExecStartEvent {
    pub call_id: String,
    pub name: String,
    pub args: Value,
}

/// `tool_execution_update` (Pi types.ts) — HIGH-FREQ.
#[derive(Clone, Debug)]
pub struct ToolExecUpdateEvent {
    pub call_id: String,
    pub chunk: Value,
}

/// `tool_execution_end` (Pi types.ts).
#[derive(Clone, Debug)]
pub struct ToolExecEndEvent {
    pub call_id: String,
    pub result: Value,
    pub is_error: bool,
}

/// `session_start` / `session_shutdown` (Pi types.ts:1136-1137) — `reason` includes `"reload"`.
#[derive(Clone, Debug)]
pub struct SessionLifecycleEvent {
    pub reason: String,
}

/// `session_before_switch` (Pi types.ts:1148).
#[derive(Clone, Debug)]
pub struct SessionBeforeSwitchEvent {
    pub target_id: String,
}

/// `session_before_fork` (Pi types.ts:1149).
#[derive(Clone, Debug)]
pub struct SessionBeforeForkEvent {
    pub entry_id: String,
}

/// `session_compact` (Pi types.ts:1153).
#[derive(Clone, Debug)]
pub struct SessionCompactEvent {
    pub summary: String,
}

/// `session_tree` (Pi types.ts:1156).
#[derive(Clone, Debug)]
pub struct SessionTreeEvent {
    pub tree: Value,
}

// --- Per-event multi-field result shapes ---

/// `tool_result` patch (Pi `ToolResultEventResult`, types.ts:1043): replace-not-merge result fields.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
}

/// `before_agent_start` dual result (Pi `BeforeAgentStartEventResult`, types.ts:1053-1057): inject a
/// message AND/OR replace the system prompt.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeAgentStartResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,
}

/// `resources_discover` result (Pi types.ts:528-539): skill/prompt/theme paths the extension provides.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourcesResult {
    #[serde(default)]
    pub skill_paths: Vec<String>,
    #[serde(default)]
    pub prompt_paths: Vec<String>,
    #[serde(default)]
    pub theme_paths: Vec<String>,
}

/// `project_trust` decision (Pi types.ts:503-525).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustResult {
    pub trusted: bool,
    #[serde(default)]
    pub remember: bool,
}
