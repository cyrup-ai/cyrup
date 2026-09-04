//! The assistant turn: [`AssistantMessage`], the [`DeferredHandle`] it carries for a deferred
//! stop reason, and the [`UNRESOLVED_API`] sentinel (func-01 §4.3).

use super::content::{Content, de_assistant_content};
use super::stop_reason::StopReason;
use super::usage::Usage;
use crate::diagnostics::AssistantMessageDiagnostic;
use crate::{ApiId, ModelId, ModelRef, ProviderId};

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
    /// The durable provider handle that accompanies [`StopReason::Deferred`] (Pi
    /// `deferred?: DeferredHandle`, `v0.84.1 ai/src/types.ts:424`). Present iff `stop_reason` is
    /// `Deferred` in practice; the type does not enforce that because Pi does not either — its
    /// harness reducer *validates* the pairing at load time and hard-fails a session whose deferred
    /// entry lacks a handle (`v0.84.1 agent/src/harness/reducer.ts:274-281`), rather than making it
    /// unrepresentable.
    ///
    /// **Boxed** because it is large (~150 bytes) and, in a port that cannot yet produce one,
    /// essentially always `None`: inline it would push `Message::Assistant` 247 bytes past its
    /// nearest sibling variant and trip `clippy::large_enum_variant`, taxing every `Message` in
    /// every transcript for a field nothing sets. `Box` is transparent to serde, so the wire bytes
    /// are unchanged.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub deferred: Option<Box<DeferredHandle>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub error_message: Option<String>,
    /// The provider's own terminal-reason token, verbatim and unmapped (Pi
    /// `rawStopReason?: string`, `v0.84.1 ai/src/types.ts:426` — and already present, at the same
    /// line, in `v0.83.0 ai/src/types.ts:411`).
    ///
    /// Pi populates it on essentially every settled turn from a streaming api: Anthropic
    /// `event.delta.stop_reason` (`anthropic-messages.ts:709`), Google `candidate.finishReason`
    /// (`google-generative-ai.ts:215`), Mistral `choice.finishReason`
    /// (`mistral-conversations.ts:356`), OpenAI-completions `choice.finish_reason`
    /// (`openai-completions.ts:459`) and OpenAI-responses `response.status`
    /// (`openai-responses-shared.ts:567,721`) — line numbers at v0.83.0, unchanged in kind at
    /// v0.84.1.
    ///
    /// cyrup populates it from every decoder that has a raw reason to carry: `anthropic_messages`,
    /// `google_generative_ai` (shared by `google_vertex`), `openai_completions`,
    /// `openai_responses` (shared by `azure_openai_responses`), `openai_codex_responses`,
    /// `mistral_conversations` and `bedrock_converse_stream`. The one decoder that leaves it `None`
    /// is `pi_messages`, and that is parity, not a gap: pi's own `api/pi-messages.ts` @v0.83.0
    /// never assigns `rawStopReason` — its wire `event.reason` IS the canonical `stopReason`
    /// (`:193`, `:201`), so there is no vendor string underneath it to preserve.
    ///
    /// It MUST also round-trip regardless of producer: a Pi-written session file carries it on
    /// every assistant entry, and dropping it on re-export is silent loss of the only record of
    /// what the provider actually said (R-00-013).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub raw_stop_reason: Option<String>,
    pub timestamp: i64,
}

/// A durable provider token identifying an in-flight request that will be completed later (Pi
/// `DeferredHandle`, `v0.84.1 ai/src/types.ts:395-404`; the type does not exist at v0.83.0).
///
/// Carried by an [`AssistantMessage`] whose `stop_reason` is [`StopReason::Deferred`]. Every field
/// is Pi's, in Pi's declaration order; unknown keys are tolerated on read the way Pi's erased TS
/// types tolerate them, and `data` preserves whatever provider-specific conversion payload the
/// upstream stored (`JsonValue` in Pi).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeferredHandle {
    pub provider: String,
    pub model_id: String,
    pub api: String,
    /// Provider token, such as a response id or batch id plus row id (Pi's own wording,
    /// `types.ts:398`).
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub poll_after_ms: Option<i64>,
    /// Provider conversion data required to reconstruct the final assistant message (Pi
    /// `data?: JsonValue`, `types.ts:403`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub data: Option<serde_json::Value>,
}

impl serde::Serialize for AssistantMessage {
    /// Self-tagging serializer: emits `role: "assistant"` FIRST (Pi's `AssistantMessage` literal
    /// always carries it, `ai/src/types.ts:384`), then Pi's exact field order — role, content, api,
    /// provider, model, responseModel?, responseId?, diagnostics?, usage, stopReason, deferred?,
    /// errorMessage?, rawStopReason?, timestamp (`v0.84.1 ai/src/types.ts:413-428`, read field for
    /// field). So every wire-serialized assistant turn — and every `StreamEvent` `partial`/
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
            + usize::from(self.deferred.is_some())
            + usize::from(self.error_message.is_some())
            + usize::from(self.raw_stop_reason.is_some());
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
        match &self.deferred {
            Some(v) => st.serialize_field("deferred", v)?,
            None => st.skip_field("deferred")?,
        }
        match &self.error_message {
            Some(v) => st.serialize_field("errorMessage", v)?,
            None => st.skip_field("errorMessage")?,
        }
        match &self.raw_stop_reason {
            Some(v) => st.serialize_field("rawStopReason", v)?,
            None => st.skip_field("rawStopReason")?,
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
            deferred: None,
            error_message: Some(message.into()),
            raw_stop_reason: None,
            timestamp: 0,
        }
    }

    /// Append a redacted diagnostic to this message (Pi `appendAssistantMessageDiagnostic`,
    /// diagnostics.ts:40-45).
    pub fn append_diagnostic(&mut self, diagnostic: AssistantMessageDiagnostic) {
        crate::diagnostics::append_assistant_message_diagnostic(&mut self.diagnostics, diagnostic);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::message::*;

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
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
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
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 0,
        };
        // Standalone (as embedded in `StreamEvent.partial`): role:"assistant" FIRST, Pi field order.
        let s = serde_json::to_string(&m).expect("serialize");
        assert!(
            s.starts_with(r#"{"role":"assistant","content":"#),
            "role first: {s}"
        );
        let order: Vec<usize> = [
            "\"role\"",
            "\"content\"",
            "\"api\"",
            "\"provider\"",
            "\"model\"",
            "\"usage\"",
            "\"stopReason\"",
            "\"timestamp\"",
        ]
        .iter()
        .map(|k| s.find(k).expect("key present"))
        .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "Pi field order: {s}");
        assert_eq!(
            s.matches("\"role\"").count(),
            1,
            "exactly one role key: {s}"
        );
        let back: AssistantMessage = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(back, m);
        // Wrapped in Message::Assistant (the JSONL form): STILL exactly one role key, role first.
        let wrapped = serde_json::to_string(&Message::Assistant(m)).expect("serialize");
        assert_eq!(
            wrapped.matches("\"role\"").count(),
            1,
            "no duplicate role in Message: {wrapped}"
        );
        assert!(
            wrapped.starts_with(r#"{"role":"assistant","content":"#),
            "{wrapped}"
        );
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
        m.append_diagnostic(create_assistant_message_diagnostic_from(
            "retry", None, None,
        ));
        assert_eq!(m.diagnostics.as_ref().map(Vec::len), Some(1));
    }
}
