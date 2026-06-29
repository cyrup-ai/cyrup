//! Declarative scripted-response format (Pi `FauxResponse`/`FauxResponseInput`, test-harness.ts:67-98)
//! plus the `fauxModel` the session harness drives (test-harness.ts:50-61), with Pi's exact
//! defaulting rules (`normalizeResponse`, `buildUsage`, `buildAssistantMessage`; test-harness.ts:93-164).

use std::sync::atomic::{AtomicU64, Ordering};

use cyrup_core::{ApiId, AssistantMessage, Content, Cost, ProviderId, StopReason, ToolCall, ToolCallId, Usage};
use cyrup_provider::faux::FauxModelDefinition;
use cyrup_provider::{Modality, Model, ModelCost};

const FAUX_PROVIDER: &str = "faux";
const FAUX_MODEL_ID: &str = "faux-1";
/// Pi's session-harness faux model speaks the anthropic-messages api (test-harness.ts:48).
const FAUX_API: &str = "anthropic-messages";

static TOOL_CALL_COUNTER: AtomicU64 = AtomicU64::new(0);

/// The faux model the headless session harness drives (Pi `fauxModel`, test-harness.ts:50-61):
/// api `anthropic-messages`, input `[text, image]`, reasoning off, contextWindow 128000,
/// maxTokens 16384, zero cost.
pub fn faux_model() -> Model {
    Model {
        id: FAUX_MODEL_ID.into(),
        name: "Faux Model".into(),
        api: ApiId::from(FAUX_API),
        provider: ProviderId::from(FAUX_PROVIDER),
        base_url: "http://localhost:0".into(),
        reasoning: false,
        input: vec![Modality::Text, Modality::Image],
        cost: ModelCost::default(),
        context_window: 128_000,
        max_tokens: 16_384,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// The faux model with an overridden `context_window` (Pi `HarnessOptions.contextWindow`,
/// test-harness.ts:324,370: `{ ...baseModel, contextWindow }`). Lets compaction-threshold tests
/// drive a small window through the harness while keeping every other field identical to
/// [`faux_model`].
pub fn faux_model_with_context_window(context_window: u64) -> Model {
    Model { context_window, ..faux_model() }
}

/// Map a declarative [`FauxModelDefinition`] (Pi `models?: FauxModelDefinition[]`,
/// suite/harness.ts:64; faux.ts:37-45) onto a harness [`Model`]. Every model the harness advertises
/// speaks the session-harness faux api (`anthropic-messages`) under the `faux` provider — matching
/// [`faux_model`] — while honoring the definition's `id`/`name`/`reasoning`/`input`/`cost`/
/// `context_window`/`max_tokens`. This is what lets the harness advertise >1 model + look one up by
/// id (Pi `harness.models`/`getModel(modelId)`, suite/harness.ts:82-84,201-202).
pub fn faux_model_from_def(def: &FauxModelDefinition) -> Model {
    Model {
        id: def.id.clone().into(),
        name: def.name.clone().unwrap_or_else(|| def.id.clone()),
        api: ApiId::from(FAUX_API),
        provider: ProviderId::from(FAUX_PROVIDER),
        base_url: "http://localhost:0".into(),
        reasoning: def.reasoning,
        input: def.input.clone(),
        cost: def.cost,
        context_window: def.context_window,
        max_tokens: def.max_tokens,
        thinking_level_map: None,
        compat: None,
        headers: None,
    }
}

/// A scripted tool call inside a [`FauxResponse`] (Pi `toolCalls[]`, test-harness.ts:71).
#[derive(Clone, Debug, Default)]
pub struct FauxToolCall {
    pub id: Option<String>,
    pub name: String,
    pub args: serde_json::Value,
}

impl FauxToolCall {
    pub fn new(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self { id: None, name: name.into(), args }
    }
}

/// Per-response model identity override (Pi `resp.model`, test-harness.ts:82-83).
#[derive(Clone, Debug, Default)]
pub struct ModelOverride {
    pub provider: Option<String>,
    pub id: Option<String>,
}

/// Partial usage overlay (Pi `Partial<Usage>`, test-harness.ts:79). `None` fields take Pi's
/// defaults via [`build_usage`].
#[derive(Clone, Debug, Default)]
pub struct UsageOverride {
    pub input: Option<u64>,
    pub output: Option<u64>,
    pub cache_read: Option<u64>,
    pub cache_write: Option<u64>,
    pub total_tokens: Option<u64>,
}

/// The declarative response description (Pi `FauxResponse`, test-harness.ts:67-84). A bare string
/// becomes a single text block via [`From<&str>`]/[`From<String>`] (Pi `FauxResponseInput` shorthand,
/// test-harness.ts:87).
#[derive(Clone, Debug, Default)]
pub struct FauxResponse {
    /// Text content block.
    pub text: Option<String>,
    /// Tool calls to include.
    pub tool_calls: Vec<FauxToolCall>,
    /// Thinking content.
    pub thinking: Option<String>,
    /// Stop reason. Defaults to `stop`, or `toolUse` if `tool_calls` present, or `error` if `error`
    /// is set (Pi inference, test-harness.ts:142-151).
    pub stop_reason: Option<StopReason>,
    /// Error message. Sets stop reason to `error` if not explicitly set.
    pub error: Option<String>,
    /// Usage overlay (merged with defaults input:100/output:50; test-harness.ts:100-113).
    pub usage: Option<UsageOverride>,
    /// Delay before the response starts streaming (Pi `delayMs`, test-harness.ts:81).
    pub delay_ms: Option<u64>,
    /// Per-response model identity override.
    pub model: Option<ModelOverride>,
}

impl FauxResponse {
    /// A text-only response (shorthand).
    pub fn text(s: impl Into<String>) -> Self {
        Self { text: Some(s.into()), ..Default::default() }
    }

    /// A response with a single tool call.
    pub fn tool_call(name: impl Into<String>, args: serde_json::Value) -> Self {
        Self { tool_calls: vec![FauxToolCall::new(name, args)], ..Default::default() }
    }

    /// An error response.
    pub fn error(message: impl Into<String>) -> Self {
        Self { error: Some(message.into()), ..Default::default() }
    }
}

impl From<&str> for FauxResponse {
    fn from(s: &str) -> Self {
        FauxResponse::text(s)
    }
}

impl From<String> for FauxResponse {
    fn from(s: String) -> Self {
        FauxResponse::text(s)
    }
}

/// Merge a usage overlay with Pi's defaults (input:100, output:50, caches:0; test-harness.ts:100-113).
pub fn build_usage(partial: Option<&UsageOverride>) -> Usage {
    let p = partial.cloned().unwrap_or_default();
    let input = p.input.unwrap_or(100);
    let output = p.output.unwrap_or(50);
    let cache_read = p.cache_read.unwrap_or(0);
    let cache_write = p.cache_write.unwrap_or(0);
    Usage {
        input,
        output,
        cache_read,
        cache_write,
        cache_write_1h: None,
        reasoning: None,
        total_tokens: p.total_tokens.unwrap_or(input + output + cache_read + cache_write),
        cost: Cost::default(),
    }
}

/// Build the [`AssistantMessage`] for a declarative response (Pi `buildAssistantMessage`,
/// test-harness.ts:117-164): thinking → text → tool calls; empty content (without error) becomes a
/// single empty text block; stop-reason inference; usage defaults; model identity override.
pub fn build_assistant_message(resp: &FauxResponse) -> AssistantMessage {
    let mut content: Vec<Content> = Vec::new();
    if let Some(thinking) = &resp.thinking {
        content.push(Content::thinking(thinking.clone()));
    }
    if let Some(text) = &resp.text {
        content.push(Content::text(text.clone()));
    }
    for tc in &resp.tool_calls {
        let id = tc.id.clone().unwrap_or_else(|| {
            let n = TOOL_CALL_COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
            format!("faux_tc_{n}")
        });
        let arguments = match &tc.args {
            serde_json::Value::Object(map) => map.clone(),
            _ => serde_json::Map::new(),
        };
        content.push(Content::ToolCall(ToolCall {
            id: ToolCallId::from(id),
            name: tc.name.clone(),
            arguments,
            thought_signature: None,
        }));
    }
    if content.is_empty() && resp.error.is_none() {
        content.push(Content::text(String::new()));
    }

    let stop_reason = if let Some(sr) = resp.stop_reason {
        sr
    } else if resp.error.is_some() {
        StopReason::Error
    } else if !resp.tool_calls.is_empty() {
        StopReason::ToolUse
    } else {
        StopReason::Stop
    };

    let provider = resp
        .model
        .as_ref()
        .and_then(|m| m.provider.clone())
        .unwrap_or_else(|| FAUX_PROVIDER.to_string());
    let model = resp
        .model
        .as_ref()
        .and_then(|m| m.id.clone())
        .unwrap_or_else(|| FAUX_MODEL_ID.to_string());

    AssistantMessage {
        content,
        provider: ProviderId::from(provider),
        model,
        api: ApiId::from(FAUX_API),
        response_model: None,
        response_id: None,
        diagnostics: None,
        usage: build_usage(resp.usage.as_ref()),
        stop_reason,
        error_message: resp.error.clone(),
        timestamp: 0,
    }
}
