//! The message & content model (arch-00 §3.3; conformance: func-01 §4).
//!
//! Serde follows arch-00 §4: structs use `rename_all = "camelCase"`; tagged enums add
//! `rename_all_fields = "camelCase"` so payload fields are camelCase for Pi-interop (R-00-013).

use crate::diagnostics::AssistantMessageDiagnostic;
use crate::{ApiId, ModelId, ModelRef, ProviderId, ToolCallId};

/// Sentinel [`ApiId`] used when an [`AssistantMessage`] must be synthesized before any concrete
/// wire api is resolvable (a catalog miss, or a stream that terminated before a model was bound).
/// Pi declares `AssistantMessage.api: Api` as required (types.ts:386); rather than weaken the field
/// to `Option`, cyrup materialises these rare unresolved cases as this sentinel so the field is
/// always populated. It never participates in cross-provider handoff equality (those messages are
/// terminal errors, not appendable replay turns).
pub const UNRESOLVED_API: &str = "unknown";

/// serde default for [`AssistantMessage::api`]: the [`UNRESOLVED_API`] sentinel, used only when a
/// deserialized message omits `api` (legacy/foreign data — Pi's runtime accepts the same).
fn default_api() -> ApiId {
    ApiId::from(UNRESOLVED_API)
}

/// Reasoning effort *level* (Pi `ThinkingLevel`, types.ts:74) — the "on" levels only, with NO
/// `off`. This is the per-request reasoning intensity (Pi `SimpleStreamOptions.reasoning?:
/// ThinkingLevel`); the *absence* of a level (or [`ModelThinkingLevel::Off`]) means reasoning is
/// disabled. Kept distinct from [`ModelThinkingLevel`] so an `off`-bearing selection cannot be
/// confused with an on-level (func-01 §12).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThinkingLevel {
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    /// The top rung (Pi added `"max"` in fbdd4638). Declared LAST so the declaration order stays
    /// the ascending ladder `EXTENDED_THINKING_LEVELS` walks when clamping upward.
    Max,
}

/// A model's selectable reasoning level (Pi `ModelThinkingLevel = "off" | ThinkingLevel`,
/// types.ts:75) — the [`ThinkingLevel`] set PLUS `off`. This is the user-facing / session-local
/// selection and the key space of `ThinkingLevelMap`. `Off` is the default (reasoning disabled).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelThinkingLevel {
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    /// See [`ThinkingLevel::Max`]. Last, for the same ladder-ordering reason.
    Max,
}

impl ModelThinkingLevel {
    /// The on-level [`ThinkingLevel`], or `None` when `Off` (Pi: `reasoning` is `undefined` for off).
    pub fn level(self) -> Option<ThinkingLevel> {
        match self {
            ModelThinkingLevel::Off => None,
            ModelThinkingLevel::Minimal => Some(ThinkingLevel::Minimal),
            ModelThinkingLevel::Low => Some(ThinkingLevel::Low),
            ModelThinkingLevel::Medium => Some(ThinkingLevel::Medium),
            ModelThinkingLevel::High => Some(ThinkingLevel::High),
            ModelThinkingLevel::Xhigh => Some(ThinkingLevel::Xhigh),
            ModelThinkingLevel::Max => Some(ThinkingLevel::Max),
        }
    }

    /// `true` for any on-level (reasoning enabled).
    pub fn is_on(self) -> bool {
        !matches!(self, ModelThinkingLevel::Off)
    }
}

impl From<ThinkingLevel> for ModelThinkingLevel {
    fn from(l: ThinkingLevel) -> Self {
        match l {
            ThinkingLevel::Minimal => ModelThinkingLevel::Minimal,
            ThinkingLevel::Low => ModelThinkingLevel::Low,
            ThinkingLevel::Medium => ModelThinkingLevel::Medium,
            ThinkingLevel::High => ModelThinkingLevel::High,
            ThinkingLevel::Xhigh => ModelThinkingLevel::Xhigh,
            ThinkingLevel::Max => ModelThinkingLevel::Max,
        }
    }
}

/// How a generation ended (func-01 §9).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StopReason {
    Stop,
    Length,
    ToolUse,
    Error,
    Aborted,
}

/// The reasoning phase of a structured text signature (Pi `TextSignatureV1.phase`, types.ts:319):
/// distinguishes commentary from the final answer for OpenAI-responses replay.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextPhase {
    Commentary,
    FinalAnswer,
}

/// Structured text-signature payload (Pi `TextSignatureV1`, types.ts:316-320). A
/// [`Content::Text`]'s `text_signature` is EITHER a legacy opaque id string OR a JSON-encoded
/// `TextSignatureV1`; use [`TextSignatureV1::parse`]/[`TextSignatureV1::encode`] to round-trip the
/// structured form through that string field.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSignatureV1 {
    /// Schema version — always `1` (Pi `v: 1`).
    pub v: u8,
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub phase: Option<TextPhase>,
}

impl TextSignatureV1 {
    /// Build a V1 signature (`v` fixed to 1).
    pub fn new(id: impl Into<String>, phase: Option<TextPhase>) -> Self {
        Self { v: 1, id: id.into(), phase }
    }

    /// Parse a structured V1 signature from a `text_signature` string, or `None` for a legacy id
    /// string / non-V1 JSON (Pi reads `textSignature` as `legacy id string or TextSignatureV1 JSON`,
    /// types.ts:325).
    pub fn parse(text_signature: &str) -> Option<Self> {
        let parsed: TextSignatureV1 = serde_json::from_str(text_signature).ok()?;
        (parsed.v == 1).then_some(parsed)
    }

    /// Encode to the JSON string stored in `text_signature` (never panics; falls back to the id).
    pub fn encode(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| self.id.clone())
    }
}

/// A model-issued tool call (func-01 §4.4).
///
/// Pi's `ToolCall` data type ALWAYS carries `type: "toolCall"` (types.ts:344-345). cyrup makes the
/// bare struct self-tag via a manual [`serde::Serialize`] that emits `type` first, in Pi's
/// declaration order (`type`, `id`, `name`, `arguments`, `thoughtSignature?`). This is the single
/// source of truth for the discriminant: [`Content::ToolCall`] delegates here (so it does NOT
/// inject a second `type` — no duplicate key), and `StreamEvent::ToolCallEnd.tool_call` serializes
/// the bare struct directly. [`serde::Deserialize`] is derived (it tolerates the extra `type` key
/// present in Pi input — no field binds it — keeping read 1:1 with Pi).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    /// Tool arguments. Pi types this as `Record<string, any>` — always a JSON object (types.ts:348);
    /// cyrup mirrors that exactly with `serde_json::Map<String, Value>`, so the type can no longer
    /// hold a non-object (array/string/number/null). Decoders that tolerate streaming partial-JSON
    /// yield an empty object (`{}`) for incomplete/invalid input rather than a scalar.
    pub arguments: serde_json::Map<String, serde_json::Value>,
    /// Provider-opaque (Google); stripped on cross-provider handoff (func-01 R-01-030).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
}

impl serde::Serialize for ToolCall {
    /// Self-tagging serializer: emits `type: "toolCall"` first (Pi `ToolCall.type`, types.ts:345),
    /// then `id`, `name`, `arguments`, and `thoughtSignature` (only when present) — byte-1:1 with
    /// Pi's `ToolCall` interface (types.ts:344-350). Single source of the discriminant: callers that
    /// embed a `ToolCall` (the [`Content::ToolCall`] variant, `StreamEvent::ToolCallEnd.tool_call`)
    /// delegate here rather than injecting their own `type`, so the key is never duplicated.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        let has_sig = self.thought_signature.is_some();
        let len = 4 + usize::from(has_sig);
        let mut st = serializer.serialize_struct("ToolCall", len)?;
        st.serialize_field("type", "toolCall")?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("arguments", &self.arguments)?;
        match &self.thought_signature {
            Some(sig) => st.serialize_field("thoughtSignature", sig)?,
            None => st.skip_field("thoughtSignature")?,
        }
        st.end()
    }
}

/// A typed content block (func-01 §4.4).
///
/// Per-role typing (gap 9): Pi types content per role — assistant = `Text|Thinking|ToolCall`,
/// user/toolResult = `Text|Image` (types.ts:379/385/402). cyrup keeps one ergonomic `Content` enum
/// but enforces Pi's per-role unions at the wire boundary with validating deserializers
/// ([`de_assistant_content`], [`de_user_content`], [`de_tool_result_content`]): a payload carrying
/// an `Image` in an assistant turn — or a `ToolCall`/`Thinking` in a user/tool-result turn — is
/// REJECTED on deserialize, exactly as Pi's typed unions reject it. Producers still build the right
/// variants by construction.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Content {
    Text {
        text: String,
        /// Legacy opaque id string OR a JSON-encoded [`TextSignatureV1`] (Pi `textSignature`,
        /// types.ts:325). Use [`TextSignatureV1::parse`]/`encode` for the structured form.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        text_signature: Option<String>,
    },
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        thinking_signature: Option<String>,
        /// Pi `redacted?: boolean` (types.ts:335) — OMITTED when unset. Pi only ever emits
        /// `redacted: true` (a safety-redacted block); an un-redacted block leaves the key
        /// `undefined`, so `JSON.stringify` drops it. cyrup keeps the field a plain `bool`
        /// (`false` = not redacted); the manual [`Content`] serializer omits the `false` default, so
        /// a non-redacted block emits no `redacted` key — byte-1:1 with Pi — while `redacted: true`
        /// still writes.
        #[serde(default)]
        redacted: bool,
    },
    ToolCall(ToolCall),
    Image {
        /// base64-encoded.
        data: String,
        mime_type: String,
    },
}

impl serde::Serialize for Content {
    /// Internally-tagged serializer (`tag = "type"`, camelCase fields) written by hand so the
    /// `ToolCall` variant can DELEGATE to [`ToolCall`]'s own self-tagging serializer — the single
    /// source of the `type:"toolCall"` discriminant. A derived internally-tagged serializer would
    /// inject its own `type` on top of `ToolCall`'s, producing a duplicate key; delegating avoids
    /// that while keeping the other variants byte-1:1 with the prior derived output.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        match self {
            Content::Text { text, text_signature } => {
                let mut st = serializer
                    .serialize_struct("Content", 2 + usize::from(text_signature.is_some()))?;
                st.serialize_field("type", "text")?;
                st.serialize_field("text", text)?;
                match text_signature {
                    Some(sig) => st.serialize_field("textSignature", sig)?,
                    None => st.skip_field("textSignature")?,
                }
                st.end()
            }
            Content::Thinking { thinking, thinking_signature, redacted } => {
                let len =
                    2 + usize::from(thinking_signature.is_some()) + usize::from(*redacted);
                let mut st = serializer.serialize_struct("Content", len)?;
                st.serialize_field("type", "thinking")?;
                st.serialize_field("thinking", thinking)?;
                match thinking_signature {
                    Some(sig) => st.serialize_field("thinkingSignature", sig)?,
                    None => st.skip_field("thinkingSignature")?,
                }
                if *redacted {
                    st.serialize_field("redacted", &true)?;
                } else {
                    st.skip_field("redacted")?;
                }
                st.end()
            }
            // Single source of the `type:"toolCall"` discriminant: delegate to `ToolCall` so the tag
            // is emitted exactly once (no duplicate key), with `id`/`name`/`arguments`/
            // `thoughtSignature?` flattened — byte-1:1 with Pi's tool-call content.
            Content::ToolCall(tool_call) => tool_call.serialize(serializer),
            Content::Image { data, mime_type } => {
                let mut st = serializer.serialize_struct("Content", 3)?;
                st.serialize_field("type", "image")?;
                st.serialize_field("data", data)?;
                st.serialize_field("mimeType", mime_type)?;
                st.end()
            }
        }
    }
}

impl Content {
    pub fn text(s: impl Into<String>) -> Self {
        Content::Text { text: s.into(), text_signature: None }
    }
    pub fn thinking(s: impl Into<String>) -> Self {
        Content::Thinking { thinking: s.into(), thinking_signature: None, redacted: false }
    }
    /// A text block carrying a (legacy or [`TextSignatureV1`]-encoded) signature.
    pub fn text_with_signature(s: impl Into<String>, signature: impl Into<String>) -> Self {
        Content::Text { text: s.into(), text_signature: Some(signature.into()) }
    }
}

/// Token + cost accounting (func-01 §4.5).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub cache_write_1h: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub reasoning: Option<u64>,
    pub total_tokens: u64,
    pub cost: Cost,
}

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cost {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub total: f64,
}

/// A completed assistant turn (func-01 §4.3).
///
/// Pi-interop flat shape: `provider`/`model`/`api` are separate fields (not a nested `ModelRef`);
/// use [`AssistantMessage::model_ref`] for handoff-equality.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantMessage {
    #[serde(default, deserialize_with = "de_assistant_content")]
    pub content: Vec<Content>,
    pub provider: ProviderId,
    pub model: String,
    /// The wire-protocol api that produced this turn. Pi declares `api: Api` as REQUIRED
    /// (types.ts:386): a produced assistant turn always knows the concrete wire api that generated
    /// it. cyrup matches that exactly — `api: ApiId` is a required field. The not-yet-resolved
    /// user-facing selection (whose api is reconstructed at model-resolution time) is modelled
    /// separately by [`crate::ModelRef::api`] (`Option<ApiId>`), not here. `api` is the
    /// handoff-equality key (func-01 R-01-029).
    ///
    /// It ALWAYS serializes (Pi emits it on every assistant turn). On READ it tolerates an absent
    /// `api` — defaulting to the [`UNRESOLVED_API`] sentinel — because Pi's runtime (erased TS
    /// types, no schema validation) likewise accepts a legacy/foreign message that omits it; this
    /// keeps v1 session migration interpretable without weakening the in-memory type to `Option`.
    #[serde(default = "default_api")]
    pub api: ApiId,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub response_id: Option<String>,
    /// Redacted provider/runtime diagnostics for failures and recoveries (Pi
    /// `diagnostics?: AssistantMessageDiagnostic[]`, types.ts:391). Skipped when empty/none.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub diagnostics: Option<Vec<AssistantMessageDiagnostic>>,
    pub usage: Usage,
    pub stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    pub timestamp: i64,
}

impl serde::Serialize for AssistantMessage {
    /// Self-tagging serializer: emits `role: "assistant"` FIRST (Pi's `AssistantMessage` literal
    /// always carries it, `ai/src/types.ts:384`), then Pi's exact field order — role, content, api,
    /// provider, model, responseModel?, responseId?, diagnostics?, usage, stopReason, errorMessage?,
    /// timestamp. So every wire-serialized assistant turn — and every `StreamEvent` `partial`/
    /// `done.message`/`error.error` that embeds one — is byte-1:1 with Pi. Verified against captured
    /// Pi bytes (`text-turn.pi-captured` `start` partial begins `{"role":"assistant","content":[],
    /// "api":...}`). The derived `Deserialize` ignores the extra `role` key on read.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        let len = 8
            + usize::from(self.response_model.is_some())
            + usize::from(self.response_id.is_some())
            + usize::from(self.diagnostics.is_some())
            + usize::from(self.error_message.is_some());
        let mut st = serializer.serialize_struct("AssistantMessage", len)?;
        st.serialize_field("role", "assistant")?;
        st.serialize_field("content", &self.content)?;
        st.serialize_field("api", &self.api)?;
        st.serialize_field("provider", &self.provider)?;
        st.serialize_field("model", &self.model)?;
        match &self.response_model {
            Some(v) => st.serialize_field("responseModel", v)?,
            None => st.skip_field("responseModel")?,
        }
        match &self.response_id {
            Some(v) => st.serialize_field("responseId", v)?,
            None => st.skip_field("responseId")?,
        }
        match &self.diagnostics {
            Some(v) => st.serialize_field("diagnostics", v)?,
            None => st.skip_field("diagnostics")?,
        }
        st.serialize_field("usage", &self.usage)?;
        st.serialize_field("stopReason", &self.stop_reason)?;
        match &self.error_message {
            Some(v) => st.serialize_field("errorMessage", v)?,
            None => st.skip_field("errorMessage")?,
        }
        st.serialize_field("timestamp", &self.timestamp)?;
        st.end()
    }
}

impl AssistantMessage {
    pub fn model_ref(&self) -> ModelRef {
        ModelRef {
            provider: self.provider.clone(),
            api: Some(self.api.clone()),
            model: ModelId::from(self.model.as_str()),
        }
    }

    /// Build a terminal error/aborted assistant message (func-01 R-01-045). `api` is the producing
    /// wire-protocol id (Pi always sets `output.api = model.api`, even on the error path). Callers
    /// pass `Some(model.api)`; `None` is reserved for the few synthetic paths where no api is
    /// resolvable (a catalog miss, or a stream that ended before any model was bound) and is
    /// materialised as the sentinel api [`UNRESOLVED_API`] so `AssistantMessage.api` stays the
    /// required field Pi declares (types.ts:386).
    pub fn errored(
        provider: ProviderId,
        model: impl Into<String>,
        api: Option<ApiId>,
        stop_reason: StopReason,
        message: impl Into<String>,
    ) -> Self {
        Self {
            content: Vec::new(),
            provider,
            model: model.into(),
            api: api.unwrap_or_else(|| ApiId::from(UNRESOLVED_API)),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason,
            error_message: Some(message.into()),
            timestamp: 0,
        }
    }

    /// Append a redacted diagnostic to this message (Pi `appendAssistantMessageDiagnostic`,
    /// diagnostics.ts:40-45).
    pub fn append_diagnostic(&mut self, diagnostic: AssistantMessageDiagnostic) {
        crate::diagnostics::append_assistant_message_diagnostic(&mut self.diagnostics, diagnostic);
    }
}

/// A conversation message (func-01 §4.2). Custom (extension/app) message types live in
/// `cyrup-agent`'s `AgentMessage` wrapper and are filtered before the model call (func-02).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(tag = "role", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum Message {
    User {
        /// Pi `UserMessage.content: string | (TextContent | ImageContent)[]` (types.ts:379). On
        /// READ, a bare JSON string is accepted and promoted to a single text block, and the array
        /// form is validated to `Text|Image` only (gap 5/9 / R-00-013). On WRITE, the content array
        /// is ALWAYS emitted — every real Pi entry point that builds a `UserMessage` constructs the
        /// array form `[{type:"text",text}]` (`agent.ts:389`, `agent-harness.ts:38`,
        /// `agent-session.ts:1117`) and Pi's session write path (`session-manager.ts:940,952,959`)
        /// is a pure `JSON.stringify(entry)` with no shape transform, so Pi never collapses a
        /// single-text user turn to the bare-string shorthand on write. cyrup matches those bytes.
        #[serde(default, deserialize_with = "de_user_content")]
        content: Vec<Content>,
        timestamp: i64,
    },
    Assistant(AssistantMessage),
    ToolResult {
        tool_call_id: ToolCallId,
        tool_name: String,
        /// Pi `ToolResultMessage.content: (TextContent | ImageContent)[]` (types.ts:402): validated
        /// to `Text|Image` only on deserialize (gap 9).
        #[serde(default, deserialize_with = "de_tool_result_content")]
        content: Vec<Content>,
        #[serde(default)]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        details: Option<serde_json::Value>,
        /// Usage from the tool execution itself, if available. NOT part of main LLM context
        /// accounting (Pi `ToolResultMessage.usage`, types.ts:421-422). Absent when `None`, exactly
        /// as Pi's `JSON.stringify` drops the `undefined` key it assigns unconditionally
        /// (agent-loop.ts:782).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        usage: Option<Usage>,
        /// Names from the tool list that became available after this result (Pi
        /// `ToolResultMessage.addedToolNames`, types.ts:423-428). Providers with native deferred
        /// tool loading use it as the load point; others ignore it. Pi writes the key only when the
        /// array is non-empty (`...(x?.length ? {addedToolNames: x} : {})`, agent-loop.ts:783), so
        /// an empty vec is absent on the wire.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        added_tool_names: Vec<String>,
        timestamp: i64,
    },
}

impl serde::Serialize for Message {
    /// Manual serializer so the `role` discriminant appears EXACTLY ONCE and in Pi's field order.
    /// `Assistant` delegates to [`AssistantMessage`]'s self-tagging serializer (which emits
    /// `role:"assistant"` first, then Pi's order); `User`/`ToolResult` write their own `role` then
    /// their fields. A derived internally-tagged `Serialize` would DOUBLE the `role` key for the
    /// `Assistant` arm now that its struct self-tags. `Deserialize` stays derived (`tag = "role"`).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        match self {
            Message::User { content, timestamp } => {
                let mut st = serializer.serialize_struct("Message", 3)?;
                st.serialize_field("role", "user")?;
                // Always the content array — Pi's real entry points build `[{type:"text",text}]`
                // for every user turn and its write path (`JSON.stringify`, no transform) never
                // collapses a single-text turn to the bare-string shorthand. The bare-string form
                // is READ-tolerated (`de_user_content`) for legacy/foreign JSONL, not written.
                st.serialize_field("content", content)?;
                st.serialize_field("timestamp", timestamp)?;
                st.end()
            }
            Message::Assistant(m) => m.serialize(serializer),
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                usage,
                added_tool_names,
                timestamp,
            } => {
                // `usage` / `addedToolNames` are OMITTED when absent, reproducing Pi's bytes: it
                // assigns `usage: finalized.result.usage` (an `undefined` key `JSON.stringify`
                // drops) and spreads `addedToolNames` only when non-empty (agent-loop.ts:782-783).
                // Both sit next to `details` so every pre-existing key position is unchanged.
                let len = 6
                    + usize::from(details.is_some())
                    + usize::from(usage.is_some())
                    + usize::from(!added_tool_names.is_empty());
                let mut st = serializer.serialize_struct("Message", len)?;
                st.serialize_field("role", "toolResult")?;
                st.serialize_field("toolCallId", tool_call_id)?;
                st.serialize_field("toolName", tool_name)?;
                st.serialize_field("content", content)?;
                st.serialize_field("isError", is_error)?;
                match details {
                    Some(d) => st.serialize_field("details", d)?,
                    None => st.skip_field("details")?,
                }
                match usage {
                    Some(u) => st.serialize_field("usage", u)?,
                    None => st.skip_field("usage")?,
                }
                if added_tool_names.is_empty() {
                    st.skip_field("addedToolNames")?;
                } else {
                    st.serialize_field("addedToolNames", added_tool_names)?;
                }
                st.serialize_field("timestamp", timestamp)?;
                st.end()
            }
        }
    }
}

/// Deserialize `UserMessage.content` accepting Pi's bare-string shorthand OR the content array, and
/// validating the array to `Text|Image` only (Pi `content: string | (TextContent | ImageContent)[]`,
/// types.ts:379). A bare string becomes a single [`Content::Text`]; a `Thinking`/`ToolCall` block is
/// rejected.
fn de_user_content<'de, D>(deserializer: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum StringOrArray {
        Str(String),
        Arr(Vec<Content>),
    }
    // Pi runtime read-tolerance (see `de_assistant_content`): accept the bare-string shorthand or
    // the content array, with no role-union rejection. A JSON `null` (or an absent key, via
    // `#[serde(default)]`) normalizes to `[]` — see `de_assistant_content` for the Pi citation.
    Ok(match Option::<StringOrArray>::deserialize(deserializer)? {
        Some(StringOrArray::Str(s)) => vec![Content::text(s)],
        Some(StringOrArray::Arr(v)) => v,
        None => Vec::new(),
    })
}

/// Deserialize `ToolResultMessage.content`, validating to `Text|Image` only (Pi
/// `content: (TextContent | ImageContent)[]`, types.ts:402).
fn de_tool_result_content<'de, D>(deserializer: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    // Pi runtime read-tolerance (see `de_assistant_content`): no role-union rejection, and a
    // `null`/absent `content` normalizes to `[]`.
    Ok(Option::<Vec<Content>>::deserialize(deserializer)?.unwrap_or_default())
}

/// Deserialize `AssistantMessage.content`, validating to `Text|Thinking|ToolCall` only (Pi
/// `content: (TextContent | ThinkingContent | ToolCall)[]`, types.ts:385); an `Image` block is
/// rejected.
fn de_assistant_content<'de, D>(deserializer: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    // Pi's per-role content unions are COMPILE-TIME TS only; its runtime `JSON.parse` accepts any
    // block regardless of role (no schema validation, `ai/src/types.ts:385`). cyrup matches that
    // read tolerance 1:1 — no role-union rejection — so any session JSONL Pi loads, cyrup loads.
    //
    // That tolerance extends to a MISSING or `null` `content`: Pi normalizes it to `[]` rather than
    // dropping the message — `sessionEntryToContextMessages` (`session-manager.ts:383-395`):
    // "Session files are parsed without validation; old versions, forks, or hand-edited files can
    // contain messages with null/missing content", then
    // `if ((role === "user" || role === "assistant" || role === "toolResult") && content == null)
    //  return [{ ...message, content: [] }];` (`==` also catches `undefined`, i.e. an absent key —
    // hence `#[serde(default)]` on the three `content` fields). Without this, cyrup's strict
    // deserializer fails the whole `Message`, the session entry demotes to `Entry::Unknown`, and the
    // turn silently vanishes from LLM context, compaction input and token accounting. The
    // SERIALIZER is unchanged: cyrup, like Pi, always writes the array form back.
    Ok(Option::<Vec<Content>>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn content_serializes_camelcase_tagged() {
        let c = Content::Thinking {
            thinking: "hm".into(),
            thinking_signature: Some("sig".into()),
            redacted: false,
        };
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["thinking"], "hm");
        assert_eq!(v["thinkingSignature"], "sig");
    }

    #[test]
    fn tool_result_message_uses_camelcase_fields() {
        let m = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "read".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            timestamp: 0,
            usage: None,
            added_tool_names: Vec::new(),
        };
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["role"], "toolResult");
        assert_eq!(v["toolCallId"], "tc1");
        assert_eq!(v["toolName"], "read");
        assert_eq!(v["isError"], false);
    }

    /// AGENT-004/005 — a tool result carrying `usage` + `addedToolNames` writes both keys next to
    /// `details` and survives a serialize → deserialize → serialize cycle byte-identically. This is
    /// the on-disk shape (the JSONL session file persists this exact struct).
    #[test]
    fn tool_result_usage_and_added_tool_names_round_trip_byte_identically() {
        let m = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "loader".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: Some(serde_json::json!({ "d": 1 })),
            usage: Some(Usage { input: 11, output: 22, total_tokens: 33, ..Usage::default() }),
            added_tool_names: vec!["late".to_string()],
            timestamp: 7,
        };
        let first = serde_json::to_string(&m).expect("serialize");
        // Both keys present, and positioned after `details` (Pi keeps them adjacent, types.ts:419-428).
        assert!(first.contains(r#""usage":{"#), "{first}");
        assert!(first.contains(r#""addedToolNames":["late"]"#), "{first}");
        let details_at = first.find(r#""details""#).expect("details key");
        assert!(details_at < first.find(r#""usage""#).expect("usage key"));

        let back: Message = serde_json::from_str(&first).expect("deserialize");
        assert_eq!(back, m, "value round-trips");
        assert_eq!(serde_json::to_string(&back).expect("re-serialize"), first, "bytes round-trip");
    }

    /// BACKWARD compatibility — NEW code reading an OLD session file. The two keys are absent, so
    /// `#[serde(default)]` yields `None`/`[]`, and re-export reproduces the ORIGINAL bytes exactly:
    /// a pre-change session file is not rewritten or corrupted by the widened struct.
    #[test]
    fn old_shape_tool_result_reads_and_re_exports_unchanged() {
        let old = concat!(
            r#"{"role":"toolResult","toolCallId":"tc1","toolName":"read","#,
            r#""content":[{"type":"text","text":"ok"}],"isError":false,"timestamp":7}"#
        );
        let m: Message = serde_json::from_str(old).expect("old shape parses");
        match &m {
            Message::ToolResult { usage, added_tool_names, .. } => {
                assert_eq!(usage, &None, "absent `usage` defaults to None");
                assert!(added_tool_names.is_empty(), "absent `addedToolNames` defaults to []");
            }
            other => panic!("expected a tool result, got {other:?}"),
        }
        assert_eq!(serde_json::to_string(&m).expect("re-serialize"), old, "byte-identical re-export");
    }

    /// FORWARD compatibility — OLD code reading a NEW session file. `OldToolResult` mirrors the
    /// pre-change variant exactly (same serde attrs, no `usage`/`addedToolNames`); nothing in the
    /// message model carries `deny_unknown_fields`, so the two extra keys are silently DROPPED and
    /// the entry still parses. It does NOT fail to deserialize, which is what would demote the line
    /// to `Entry::Unknown` in the session reader. The loss is lossy-but-non-fatal, exactly as for
    /// any other forward-added key.
    #[test]
    fn new_shape_tool_result_still_parses_under_the_pre_change_shape() {
        #[derive(serde::Deserialize, serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct OldToolResult {
            role: String,
            tool_call_id: String,
            tool_name: String,
            content: Vec<Content>,
            #[serde(default)]
            is_error: bool,
            #[serde(skip_serializing_if = "Option::is_none", default)]
            details: Option<serde_json::Value>,
            timestamp: i64,
        }

        let new_msg = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "loader".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            usage: Some(Usage { input: 11, output: 22, total_tokens: 33, ..Usage::default() }),
            added_tool_names: vec!["late".to_string()],
            timestamp: 7,
        };
        let new_bytes = serde_json::to_string(&new_msg).expect("serialize");

        let old: OldToolResult =
            serde_json::from_str(&new_bytes).expect("pre-change shape still parses new bytes");
        assert_eq!(old.role, "toolResult");
        assert_eq!(old.tool_name, "loader");
        assert_eq!(old.timestamp, 7);
        // Old code re-exports without the two keys (lossy, non-fatal).
        let re = serde_json::to_string(&old).expect("re-serialize");
        assert!(!re.contains("usage"), "{re}");
        assert!(!re.contains("addedToolNames"), "{re}");
        // And the NEW reader recovers defaults from those old bytes without error.
        let back: Message = serde_json::from_str(&re).expect("new reader parses old bytes");
        match back {
            Message::ToolResult { usage, added_tool_names, .. } => {
                assert_eq!(usage, None);
                assert!(added_tool_names.is_empty());
            }
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    #[test]
    fn assistant_message_roundtrips_and_tags_role() {
        let m = Message::Assistant(AssistantMessage {
            content: vec![Content::text("hi")],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        });
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["role"], "assistant");
        assert_eq!(v["stopReason"], "stop");
        assert!(v.get("diagnostics").is_none()); // skipped when None
        let back: Message = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, m);
    }

    #[test]
    fn user_content_accepts_bare_string_shorthand() {
        // Pi-interop: a user message whose `content` is a bare JSON string.
        let json = serde_json::json!({ "role": "user", "content": "hello", "timestamp": 7 });
        let m: Message = serde_json::from_value(json).expect("deserialize");
        assert_eq!(m, Message::User { content: vec![Content::text("hello")], timestamp: 7 });
        // The array form still deserializes.
        let json2 = serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "hi" }],
            "timestamp": 0,
        });
        let m2: Message = serde_json::from_value(json2).expect("deserialize");
        assert_eq!(m2, Message::User { content: vec![Content::text("hi")], timestamp: 0 });
    }

    #[test]
    fn text_signature_v1_roundtrips_through_string_field() {
        let sig = TextSignatureV1::new("resp_123", Some(TextPhase::FinalAnswer));
        let encoded = sig.encode();
        // Encodes camelCase + snake_case phase, v:1.
        let v: serde_json::Value = serde_json::from_str(&encoded).expect("json");
        assert_eq!(v["v"], 1);
        assert_eq!(v["id"], "resp_123");
        assert_eq!(v["phase"], "final_answer");
        // Parses back; a legacy id string yields None.
        assert_eq!(TextSignatureV1::parse(&encoded), Some(sig));
        assert_eq!(TextSignatureV1::parse("legacy-opaque-id"), None);
    }

    #[test]
    fn model_thinking_level_splits_off_from_levels() {
        assert_eq!(ModelThinkingLevel::default(), ModelThinkingLevel::Off);
        assert_eq!(ModelThinkingLevel::Off.level(), None);
        assert_eq!(ModelThinkingLevel::High.level(), Some(ThinkingLevel::High));
        assert!(ModelThinkingLevel::Minimal.is_on());
        assert_eq!(ModelThinkingLevel::from(ThinkingLevel::Low), ModelThinkingLevel::Low);
    }

    /// PROV-002: the `max` rung Pi added in fbdd4638 (`types.ts:79`). It must be an ON level and
    /// must serialize to the bare `"max"` key the `thinkingLevelMap`, settings and session
    /// persistence all use.
    #[test]
    fn max_is_a_first_class_on_level() {
        assert_eq!(ModelThinkingLevel::Max.level(), Some(ThinkingLevel::Max));
        assert!(ModelThinkingLevel::Max.is_on());
        assert_eq!(ModelThinkingLevel::from(ThinkingLevel::Max), ModelThinkingLevel::Max);
        assert_eq!(serde_json::to_value(ModelThinkingLevel::Max).expect("ser"), serde_json::json!("max"));
        assert_eq!(serde_json::to_value(ThinkingLevel::Max).expect("ser"), serde_json::json!("max"));
        assert_eq!(
            serde_json::from_value::<ModelThinkingLevel>(serde_json::json!("max")).expect("de"),
            ModelThinkingLevel::Max
        );
    }

    #[test]
    fn user_content_serializes_single_text_as_array_like_pi() {
        // Every real Pi entry point builds the ARRAY form `[{type:"text",text}]` for a single-text
        // user turn (agent.ts:389, agent-harness.ts:38, agent-session.ts:1117) and Pi's write path
        // (session-manager.ts:940,952,959 — pure JSON.stringify) never collapses it to a bare
        // string. cyrup must emit the same bytes, even for a single signature-less text block.
        let m = Message::User { content: vec![Content::text("hi")], timestamp: 7 };
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["content"], serde_json::json!([{ "type": "text", "text": "hi" }]));
        let back: Message = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, m);
        // The bare-string shorthand is still READ-tolerated for legacy/foreign JSONL, promoting to
        // a single text block (Pi's `content: string | Content[]` union accepts it on load).
        let legacy: Message =
            serde_json::from_value(serde_json::json!({ "role": "user", "content": "hi", "timestamp": 7 }))
                .expect("deserialize bare-string legacy shorthand");
        assert_eq!(legacy, m);
        // A text block carrying a signature stays the array form (the signature must survive).
        let m2 = Message::User {
            content: vec![Content::text_with_signature("hi", "sig")],
            timestamp: 0,
        };
        let v2 = serde_json::to_value(&m2).expect("serialize");
        assert!(v2["content"].is_array());
        // Two blocks / an image stay the array form.
        let m3 = Message::User {
            content: vec![Content::text("a"), Content::text("b")],
            timestamp: 0,
        };
        assert!(serde_json::to_value(&m3).expect("serialize")["content"].is_array());
    }

    #[test]
    fn assistant_content_accepts_image_on_deserialize_like_pi() {
        // Pi's runtime is type-erased: `JSON.parse` accepts an image in an assistant turn even
        // though the compile-time TS union forbids it (types.ts:385). cyrup matches that read
        // tolerance 1:1 — a session JSONL Pi loads, cyrup loads.
        let json = serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "image", "data": "x", "mimeType": "image/png" }],
            "provider": "faux", "model": "m", "api": "faux",
            "usage": Usage::default(), "stopReason": "stop", "timestamp": 0,
        });
        let m = serde_json::from_value::<Message>(json).expect("Pi accepts an off-union block");
        match m {
            Message::Assistant(a) => assert!(matches!(a.content.as_slice(), [Content::Image { .. }])),
            other => panic!("expected assistant, got {other:?}"),
        }
    }

    #[test]
    fn user_and_tool_result_content_accept_off_union_blocks_like_pi() {
        // Pi runtime read-tolerance (see above): user/toolResult content is typed Text|Image at
        // compile time but `JSON.parse` accepts any block (types.ts:379,402). cyrup matches 1:1.
        let user = serde_json::json!({
            "role": "user",
            "content": [{ "type": "toolCall", "id": "t", "name": "n", "arguments": {} }],
            "timestamp": 0,
        });
        assert!(serde_json::from_value::<Message>(user).is_ok());
        let tr = serde_json::json!({
            "role": "toolResult", "toolCallId": "t", "toolName": "n",
            "content": [{ "type": "thinking", "thinking": "x" }],
            "isError": false, "timestamp": 0,
        });
        assert!(serde_json::from_value::<Message>(tr).is_ok());
    }

    #[test]
    fn bare_assistant_message_role_first_pi_order_single_role_key() {
        let m = AssistantMessage {
            content: vec![Content::text("hi")],
            provider: "faux".into(),
            model: "faux-1".into(),
            api: "faux".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::Stop,
            error_message: None,
            timestamp: 0,
        };
        // Standalone (as embedded in `StreamEvent.partial`): role:"assistant" FIRST, Pi field order.
        let s = serde_json::to_string(&m).expect("serialize");
        assert!(s.starts_with(r#"{"role":"assistant","content":"#), "role first: {s}");
        let order: Vec<usize> = [
            "\"role\"", "\"content\"", "\"api\"", "\"provider\"", "\"model\"", "\"usage\"",
            "\"stopReason\"", "\"timestamp\"",
        ]
        .iter()
        .map(|k| s.find(k).expect("key present"))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "Pi field order: {s}");
        assert_eq!(s.matches("\"role\"").count(), 1, "exactly one role key: {s}");
        let back: AssistantMessage = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back, m);
        // Wrapped in Message::Assistant (the JSONL form): STILL exactly one role key, role first.
        let wrapped = serde_json::to_string(&Message::Assistant(m)).expect("serialize");
        assert_eq!(wrapped.matches("\"role\"").count(), 1, "no duplicate role in Message: {wrapped}");
        assert!(wrapped.starts_with(r#"{"role":"assistant","content":"#), "{wrapped}");
    }

    #[test]
    fn tool_call_arguments_reject_non_object() {
        // gap 11: Pi types ToolCall.arguments as Record<string, any> — a scalar/array is rejected.
        let ok = serde_json::json!({
            "type": "toolCall", "id": "t", "name": "n", "arguments": { "a": 1 },
        });
        let tc: Content = serde_json::from_value(ok).expect("object arguments deserialize");
        assert!(matches!(tc, Content::ToolCall(_)));
        let bad = serde_json::json!({
            "type": "toolCall", "id": "t", "name": "n", "arguments": [1, 2, 3],
        });
        assert!(serde_json::from_value::<Content>(bad).is_err());
    }

    #[test]
    fn assistant_message_api_is_required_on_the_wire() {
        // gap 7: Pi declares api: Api as required — it always serializes and must be present on read.
        let m = AssistantMessage::errored(
            "faux".into(),
            "faux-1",
            Some("faux".into()),
            StopReason::Error,
            "boom",
        );
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["api"], "faux");
        // The synthetic no-api path materialises the sentinel rather than omitting the field.
        let synth = AssistantMessage::errored("x".into(), "y", None, StopReason::Error, "z");
        assert_eq!(synth.api.as_str(), UNRESOLVED_API);
    }

    #[test]
    fn thinking_redacted_omitted_when_false_emitted_when_true() {
        // gap 3: Pi `redacted?: boolean` (types.ts:335) is omitted when unset — Pi only ever
        // emits `redacted: true`. A non-redacted block must NOT serialize a `redacted` key.
        let not_redacted = Content::thinking("hm");
        let v = serde_json::to_value(&not_redacted).expect("serialize");
        assert!(v.get("redacted").is_none(), "false must be omitted: {v}");
        // Absent on the wire round-trips back to `false`.
        let back: Content = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, not_redacted);
        // A redacted block still writes `redacted: true`.
        let redacted = Content::Thinking {
            thinking: "x".into(),
            thinking_signature: Some("sig".into()),
            redacted: true,
        };
        let rv = serde_json::to_value(&redacted).expect("serialize");
        assert_eq!(rv["redacted"], true);
        assert_eq!(serde_json::from_value::<Content>(rv).expect("deserialize"), redacted);
    }

    #[test]
    fn bare_tool_call_self_tags_with_exactly_one_type_key() {
        // Req 1 + 4: Pi's `ToolCall` always carries `type:"toolCall"` (types.ts:344-345). The bare
        // struct self-tags exactly once, in Pi field order, and round-trips.
        let tc = ToolCall {
            id: "tc1".into(),
            name: "read".into(),
            arguments: serde_json::Map::new(),
            thought_signature: None,
        };
        let s = serde_json::to_string(&tc).expect("serialize");
        assert_eq!(s.matches("\"type\"").count(), 1, "exactly one type key: {s}");
        let v = serde_json::to_value(&tc).expect("serialize");
        assert_eq!(v["type"], "toolCall");
        assert_eq!(v["id"], "tc1");
        assert_eq!(v["name"], "read");
        assert!(v["arguments"].is_object());
        assert!(v.get("thoughtSignature").is_none(), "omitted when None: {v}");
        // Round-trip (req 4).
        assert_eq!(serde_json::from_value::<ToolCall>(v).expect("deserialize"), tc);
        // thoughtSignature is emitted (camelCase) when present and still round-trips.
        let tc_sig = ToolCall { thought_signature: Some("sig".into()), ..tc.clone() };
        let vs = serde_json::to_value(&tc_sig).expect("serialize");
        assert_eq!(vs["thoughtSignature"], "sig");
        assert_eq!(serde_json::from_value::<ToolCall>(vs).expect("deserialize"), tc_sig);
    }

    #[test]
    fn content_tool_call_flattens_single_type_key_no_duplicate() {
        // Req 2 + 4: `Content::ToolCall` delegates to `ToolCall`'s self-tag — exactly ONE
        // `type:"toolCall"` (no duplicate key), fields flattened, byte-1:1 with Pi tool-call content.
        let tc = ToolCall {
            id: "t".into(),
            name: "n".into(),
            arguments: serde_json::Map::new(),
            thought_signature: Some("g".into()),
        };
        let c = Content::ToolCall(tc);
        let s = serde_json::to_string(&c).expect("serialize");
        assert_eq!(s.matches("\"type\"").count(), 1, "no duplicate type key: {s}");
        assert!(s.starts_with("{\"type\":\"toolCall\""), "type emitted first: {s}");
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v["type"], "toolCall");
        assert_eq!(v["id"], "t");
        assert_eq!(v["name"], "n");
        assert_eq!(v["thoughtSignature"], "g");
        // Round-trip (req 4): Pi input (with `type` present) deserializes back to an equal value.
        assert_eq!(serde_json::from_value::<Content>(v).expect("deserialize"), c);
    }

    #[test]
    fn assistant_append_diagnostic_accumulates() {
        use crate::diagnostics::create_assistant_message_diagnostic_from;
        let mut m = AssistantMessage::errored(
            "faux".into(),
            "faux-1",
            Some("faux".into()),
            StopReason::Error,
            "boom",
        );
        assert!(m.diagnostics.is_none());
        m.append_diagnostic(create_assistant_message_diagnostic_from("retry", None, None));
        assert_eq!(m.diagnostics.as_ref().map(Vec::len), Some(1));
    }
}
