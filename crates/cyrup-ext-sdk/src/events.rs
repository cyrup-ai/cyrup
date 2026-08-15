//! Typed event payloads + per-event result shapes (arch-08 §3.3; Pi extensions/types.ts:503-1085).
//! A handler receives a typed payload (parsed from the host's JSON seam) and a [`crate::Ctx`], and
//! returns an [`crate::Outcome`]. The multi-field Pi results (`before_agent_start`, `tool_result`)
//! are expressed via the dedicated patch structs here, surfaced through `Outcome` constructors.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// `tool_call` (Pi `ToolCallEventBase`, types.ts:822-865) — block/mutate the in-flight tool call.
/// Byte-shape: `{toolCallId, toolName, input}` (the `type` discriminant is the event name).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallEvent {
    #[serde(rename = "toolCallId")]
    pub call_id: String,
    #[serde(rename = "toolName")]
    pub name: String,
    pub input: Value,
}

/// `tool_result` (Pi `ToolResultEventBase` + per-tool subtype, types.ts:883-929) — mutate the
/// result. Byte-shape: `{toolCallId, toolName, input, content, isError, details}` — carries the
/// executed tool `input` (arguments) and the per-tool `details` blob, not just content+isError.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultEvent {
    #[serde(rename = "toolCallId")]
    pub call_id: String,
    #[serde(rename = "toolName")]
    pub name: String,
    /// The executed tool's arguments (Pi `ToolResultEventBase.input`, types.ts:886).
    pub input: Value,
    pub content: Value,
    pub is_error: bool,
    /// The per-tool structured details (Pi `BashToolDetails | … | undefined`, types.ts:891-928).
    /// `None` (= Pi `undefined`) for tools that carry none (e.g. `write`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Usage from the tool execution itself, if available (Pi `ToolResultEventBase.usage`,
    /// types.ts:919-921, upstream `2fd38684`). `None` (= Pi `undefined`) for every ordinary tool.
    /// Open-shaped `{input, output, cacheRead, cacheWrite, cost, …}` — the guest SDK has no
    /// dependency on the host's `Usage` type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
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

/// `input` (Pi `InputEvent`, types.ts:800-810) — transform/handle a user input line. Byte-shape:
/// `{text, images?, source, streamingBehavior?}`: the submission text, the attached images
/// (`undefined` when none), the `source` ("interactive"|"rpc"|"extension"), and the in-flight
/// `streamingBehavior` ("steer"|"followUp"; `undefined` when the agent is idle).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InputEvent {
    pub text: String,
    /// Attached images (Pi `InputEvent.images?`, types.ts:805); `None` = Pi `undefined`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub images: Option<Value>,
    /// Where the input came from (Pi `InputEvent.source`, types.ts:807).
    pub source: String,
    /// How the input is delivered during streaming (Pi `InputEvent.streamingBehavior?`,
    /// types.ts:809); `None` = Pi `undefined` (agent idle).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub streaming_behavior: Option<String>,
}

/// `user_bash` (Pi `UserBashEvent`, types.ts:782-790) — block/transform/provide a `!`/`!!` bash
/// invocation. Byte-shape: `{command, excludeFromContext, cwd}`. The `operations`/`result` override
/// is RETURNED via [`UserBashResult`] (Pi `UserBashEventResult`), not carried on the event.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserBashEvent {
    pub command: String,
    /// True when the `!!` prefix was used (excluded from LLM context) (Pi types.ts:787).
    pub exclude_from_context: bool,
    /// The current working directory (Pi types.ts:789).
    pub cwd: String,
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
    /// pi `ModelSelectEvent.previousModel` (`extensions/types.ts:797` @v0.83.0) — a SIBLING field.
    /// cyrup used to nest it inside `model`, so a ported handler read `event.previousModel` and got
    /// `undefined` (EXT-042).
    pub previous_model: Option<Value>,
    /// pi `ModelSelectEvent.source` (`extensions/types.ts:798` @v0.83.0) — also a sibling.
    pub source: String,
}

/// `thinking_level_select` (Pi types.ts:1163).
#[derive(Clone, Debug)]
pub struct ThinkingLevelSelectEvent {
    pub level: String,
    /// pi `ThinkingLevelSelectEvent.previousLevel` (`extensions/types.ts:805` @v0.83.0) — EXT-042.
    /// Without it a handler that wants to react only to an INCREASE had to keep a shadow copy and
    /// was wrong on the first event of a session. `None` is that first event.
    pub previous_level: Option<String>,
}

/// `agent_end` (Pi types.ts:1138) — notify with the full final message list.
#[derive(Clone, Debug)]
pub struct AgentEndEvent {
    pub messages: Value,
}

/// `turn_start` (Pi `TurnStartEvent`, types.ts:688-693).
#[derive(Clone, Debug)]
pub struct TurnStartEvent {
    pub turn_index: u32,
    /// Wall-clock milliseconds at emit (Pi `Date.now()`, agent-session.ts:624).
    pub timestamp: u64,
}

/// `turn_end` (Pi `TurnEndEvent`, types.ts:703-709). Byte-shape: `{turnIndex, message, toolResults}`
/// — the finalized assistant `message` AND the `toolResults` produced this turn.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEndEvent {
    pub turn_index: u32,
    pub message: Value,
    /// The tool-result messages produced this turn (Pi `TurnEndEvent.toolResults`, types.ts:708).
    pub tool_results: Value,
}

/// `message_start` (Pi `MessageStartEvent`, types.ts:711-715). Byte-shape: `{message}` — the full
/// message (user|assistant|toolResult), not just its role.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageStartEvent {
    pub message: Value,
}

/// `message_update` (Pi `MessageUpdateEvent`, types.ts:717-722) — HIGH-FREQ. Byte-shape:
/// `{message, assistantMessageEvent}` — the full in-flight `message` AND the provider delta.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageUpdateEvent {
    pub message: Value,
    /// The provider stream delta (Pi `MessageUpdateEvent.assistantMessageEvent`, types.ts:721).
    pub assistant_message_event: Value,
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
    /// pi `ToolExecutionUpdateEvent.toolName` (`extensions/types.ts:773` @v0.83.0) — EXT-014.
    pub name: String,
    /// pi `ToolExecutionUpdateEvent.args` (`extensions/types.ts:774` @v0.83.0) — EXT-014.
    pub args: Value,
    pub chunk: Value,
}

/// `tool_execution_end` (Pi types.ts).
#[derive(Clone, Debug)]
pub struct ToolExecEndEvent {
    pub call_id: String,
    /// pi `ToolExecutionEndEvent.toolName` (`extensions/types.ts:782` @v0.83.0) — EXT-014. Without
    /// it an observer had to keep its own `callId -> toolName` map from `tool_execution_start`, and
    /// could not filter by tool at all if it missed the start.
    pub name: String,
    pub result: Value,
    pub is_error: bool,
}

/// `session_start` / `session_shutdown` — `reason` includes `"reload"`.
///
/// EXT-015: both upstream events carry a second, optional session-file field, and cyrup dropped
/// both. pi `SessionStartEvent {type, reason, previousSessionFile?}`
/// (`extensions/types.ts:562-569` @v0.83.0, "Present for \"new\", \"resume\", and \"fork\"")
/// and `SessionShutdownEvent {type, reason, targetSessionFile?}` (`:616-621`, "Destination session
/// file when shutting down due to session replacement"). The two fields play the same role on the
/// two sides of a replacement, so one struct carries it under one name.
#[derive(Clone, Debug)]
pub struct SessionLifecycleEvent {
    pub reason: String,
    /// `previousSessionFile` on `session_start`, `targetSessionFile` on `session_shutdown`.
    pub session_file: Option<String>,
}

/// `session_info_changed` (pi `SessionInfoChangedEvent`, `extensions/types.ts:571-575` @v0.83.0,
/// subscribed at `:1203`) — EXT-011.
#[derive(Clone, Debug)]
pub struct SessionInfoChangedEvent {
    /// "Current normalized session name. Undefined when the name is cleared" — so `None` is a
    /// CLEARED name, not an unknown one.
    pub name: Option<String>,
}

/// `resources_discover` (pi `ResourcesDiscoverEvent`, `extensions/types.ts:544-548` @v0.83.0) —
/// EXT-016.
#[derive(Clone, Debug)]
pub struct ResourcesDiscoverEvent {
    pub cwd: String,
    /// `"startup" | "reload"`.
    pub reason: String,
}

/// `project_trust` (pi `ProjectTrustEvent`, `extensions/types.ts:519-522` @v0.83.0) — EXT-043.
/// The verdict is per-DIRECTORY upstream: the store is keyed by this value
/// (`options.trustStore.set(options.cwd, trusted)`, `core/project-trust.ts:63-65`), so an
/// allowlist policy cannot be written without it.
#[derive(Clone, Debug)]
pub struct ProjectTrustEvent {
    pub cwd: String,
}

/// `before_provider_headers` (pi `BeforeProviderHeadersEvent`, `extensions/types.ts:686-689`
/// @v0.83.0) — EXT-009. Upstream's doc at `:681-685`: "Handlers mutate `headers` in place (e.g. to
/// inject tracing/session headers); the return value is ignored. A `null` value deletes that
/// header." Return the patch through [`crate::Outcome::mutate`] — a key mapped to `null` DELETES.
#[derive(Clone, Debug)]
pub struct BeforeProviderHeadersEvent {
    pub headers: Value,
}

/// `session_before_switch` (Pi types.ts:1148).
#[derive(Clone, Debug)]
pub struct SessionBeforeSwitchEvent {
    /// pi `SessionBeforeSwitchEvent.reason: "new" | "resume"` (`extensions/types.ts:580`
    /// @v0.83.0) — the field that distinguishes the two cases, dropped entirely before EXT-015.
    pub reason: String,
    /// pi `targetSessionFile?` (`extensions/types.ts:581`).
    pub target_session_file: Option<String>,
}

/// `session_before_fork` (Pi types.ts:1149).
#[derive(Clone, Debug)]
pub struct SessionBeforeForkEvent {
    pub entry_id: String,
    /// pi `SessionBeforeForkEvent.position: "before" | "at"` (`extensions/types.ts:588`
    /// @v0.83.0) — EXT-015. A fork BEFORE an entry and a fork AT it are different operations.
    pub position: String,
}

/// `session_before_compact` (Pi `SessionBeforeCompactEvent`, types.ts:577-587). Byte-shape:
/// `{preparation, branchEntries, customInstructions?, reason, willRetry}` — the computed
/// `preparation` (Pi `CompactionPreparation`: firstKeptEntryId/messagesToSummarize/
/// turnPrefixMessages/isSplitTurn/tokensBefore/previousSummary?/fileOps/settings — the two message
/// arrays are RAW `AgentMessage`s, so `bashExecution`/`custom`/`branchSummary`/`compactionSummary`
/// keep their `role` and their role-specific fields, and `excludeFromContext` bash commands are
/// present), the branch entries in scope, optional custom instructions, the trigger `reason`
/// (`"manual"|"threshold"|"overflow"`), and `willRetry`. The non-serializable `signal: AbortSignal`
/// is omitted from the seam. A handler returns [`SessionBeforeCompactResult`] via
/// [`crate::Outcome::compaction_override`] or vetoes via [`crate::Outcome::block`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeCompactEvent {
    /// The computed compaction preparation (Pi `preparation: CompactionPreparation`, types.ts:579).
    pub preparation: Value,
    /// The session entries in scope for this compaction (Pi `branchEntries`, types.ts:580).
    pub branch_entries: Value,
    /// Custom summarization instructions, if any (Pi `customInstructions?`, types.ts:581).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// What triggered the compaction (Pi `reason`, types.ts:583).
    pub reason: String,
    /// True when the aborted turn is retried after compaction (Pi `willRetry`, types.ts:585).
    pub will_retry: bool,
}

/// `session_before_tree` (Pi `SessionBeforeTreeEvent`, types.ts:623-628). Byte-shape:
/// `{preparation}` — the computed `TreePreparation` (targetId/oldLeafId/commonAncestorId/
/// entriesToSummarize/userWantsSummary/customInstructions?/replaceInstructions?/label?). The
/// non-serializable `signal: AbortSignal` is omitted. A handler returns [`SessionBeforeTreeResult`]
/// via [`crate::Outcome::tree_override`] or vetoes via [`crate::Outcome::block`].
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeTreeEvent {
    /// The computed tree-navigation preparation (Pi `preparation: TreePreparation`, types.ts:626).
    pub preparation: Value,
}

/// `session_compact` (Pi `SessionCompactEvent`, types.ts:588-597). Byte-shape:
/// `{compactionEntry, fromExtension, reason, willRetry}` — the produced compaction entry (which
/// carries the summary text), whether an extension drove it, and the trigger/retry flags. This is
/// the Pi shape; the prior `{summary}` flattened `compactionEntry.summary` and dropped the rest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionCompactEvent {
    /// The compaction entry written to the session (Pi `compactionEntry`, types.ts:591); its
    /// `summary` field carries the summary text.
    pub compaction_entry: Value,
    /// True when an extension drove the compaction (Pi `fromExtension`, types.ts:592).
    pub from_extension: bool,
    /// What triggered the compaction (Pi `reason`, types.ts:594).
    pub reason: String,
    /// True when the aborted turn is retried after compaction (Pi `willRetry`, types.ts:596).
    pub will_retry: bool,
}

/// `session_tree` (Pi types.ts:1156).
#[derive(Clone, Debug)]
pub struct SessionTreeEvent {
    pub tree: Value,
}

// --- Per-event multi-field result shapes ---

/// `tool_result` patch (Pi `ToolResultEventResult`, `extensions/types.ts:1085-1090` @v0.83.0; EXT-036 corrected `:1043`): replace-not-merge result fields.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolResultPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    /// Replaces the tool result's usage in full when present (Pi `ToolResultEventResult.usage`,
    /// types.ts:1089). There is no deep merge (types.ts:70-78); omitted = keep the tool's value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
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

/// Pi's tri-state `project_trust` decision (`ProjectTrustEventDecision`, types.ts:508): `"yes"` /
/// `"no"` are terminal, `"undecided"` falls through to the next handler. Serializes 1:1 with Pi's
/// string union (camelCase → lowercase single words).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProjectTrustDecision {
    Yes,
    No,
    #[default]
    Undecided,
}

/// `project_trust` result (Pi `ProjectTrustEventResult`, types.ts:508-513). `trusted` is Pi's
/// tri-state (`"yes"|"no"|"undecided"`), NOT a bool (sdk gap #21): collapsing `"undecided"` to a
/// boolean loses the fall-through semantics.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectTrustResult {
    pub trusted: ProjectTrustDecision,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remember: bool,
}

impl ProjectTrustResult {
    /// Trust the project (Pi `{trusted:"yes"}`); `remember` persists the decision.
    pub fn trust(remember: bool) -> Self {
        Self { trusted: ProjectTrustDecision::Yes, remember }
    }
    /// Distrust the project (Pi `{trusted:"no"}`).
    pub fn distrust(remember: bool) -> Self {
        Self { trusted: ProjectTrustDecision::No, remember }
    }
    /// Abstain — defer to the next handler / host prompt (Pi `{trusted:"undecided"}`).
    pub fn undecided() -> Self {
        Self { trusted: ProjectTrustDecision::Undecided, remember: false }
    }
}

/// `session_before_compact` compaction override (Pi `SessionBeforeCompactResult.compaction`, a
/// `CompactionResult`, types.ts:1079 + compaction.ts:103-110). Returned via
/// [`crate::Outcome::compaction_override`]: the `summary` (and optional `details`) replace the
/// default model summarization and the appended compaction entry is marked `fromExtension`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeCompactResult {
    /// The override summary text (Pi `CompactionResult.summary`).
    pub summary: String,
    /// The first kept entry id (Pi `CompactionResult.firstKeptEntryId`); `None` = keep the prepared cut.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_kept_entry_id: Option<String>,
    /// The pre-compaction token count (Pi `CompactionResult.tokensBefore`); `None` = keep the prepared value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<u64>,
    /// Extension-specific structured details (Pi `CompactionResult.details?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Token spend the guest reports for its own summarization (Pi `CompactionResult.usage?`,
    /// `compaction.ts:88-89`). The host persists it verbatim on the appended compaction entry
    /// (`CompactionEntry.usage`, `session-manager.ts:69-80`), so it shows up in per-session totals.
    /// Shape is Pi's `Usage` (`{input, output, cacheRead, cacheWrite, totalTokens, cost{…}}`); kept
    /// as raw JSON here so the guest crate stays free of the host's message types.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<Value>,
}

/// `session_before_tree` override (Pi `SessionBeforeTreeResult`, types.ts:1082-1094). Returned via
/// [`crate::Outcome::tree_override`]: an override summary (with optional details), and/or overridden
/// `customInstructions`/`replaceInstructions`/`label` for the branch summarization.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionBeforeTreeResult {
    /// An override branch summary (Pi `SessionBeforeTreeResult.summary?`, `{summary, details?}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Value>,
    /// Override custom summarization instructions (Pi `customInstructions?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_instructions: Option<String>,
    /// Override whether `customInstructions` replaces the default prompt (Pi `replaceInstructions?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replace_instructions: Option<bool>,
    /// Override label to attach to the branch summary entry (Pi `label?`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}
