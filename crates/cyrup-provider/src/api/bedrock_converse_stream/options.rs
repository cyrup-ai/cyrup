//! Typed options (pi `BedrockOptions`, `bedrock-converse-stream.ts:68-100`).

use crate::stream::StreamOptions;
use serde_json::{Value, json};
use std::collections::BTreeMap;

/// How Claude's thinking content is returned (pi `BedrockThinkingDisplay`,
/// `bedrock-converse-stream.ts:66`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BedrockThinkingDisplay {
    Summarized,
    Omitted,
}

impl BedrockThinkingDisplay {
    /// The wire string for `additionalModelRequestFields.thinking.display`.
    pub fn as_wire(self) -> &'static str {
        match self {
            BedrockThinkingDisplay::Summarized => "summarized",
            BedrockThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// Bedrock's `toolChoice` union (pi `BedrockOptions.toolChoice`,
/// `bedrock-converse-stream.ts:71`). Distinct from cyrup's unified
/// [`ToolChoice`](crate::stream::ToolChoice) because Bedrock spells "required" as `any` and its
/// named form is `{tool:{name}}`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BedrockToolChoice {
    Auto,
    Any,
    None,
    Tool { name: String },
}

impl BedrockToolChoice {
    /// Lower cyrup's unified tool choice onto Bedrock's union
    /// (`Required` → `any`, `Function` → `{tool:{name}}`).
    pub fn from_unified(tc: &crate::stream::ToolChoice) -> Self {
        use crate::stream::ToolChoice;
        match tc {
            ToolChoice::Auto => BedrockToolChoice::Auto,
            ToolChoice::None => BedrockToolChoice::None,
            ToolChoice::Required => BedrockToolChoice::Any,
            ToolChoice::Function { name } => BedrockToolChoice::Tool { name: name.clone() },
        }
    }

    /// The `toolConfig.toolChoice` wire JSON, or `None` for `none` (upstream returns no
    /// `toolConfig` at all for `"none"`, handled by [`super::convert::convert_tool_config`]).
    pub(super) fn to_wire(&self) -> Option<Value> {
        match self {
            BedrockToolChoice::Auto => Some(json!({ "auto": {} })),
            BedrockToolChoice::Any => Some(json!({ "any": {} })),
            BedrockToolChoice::Tool { name } => Some(json!({ "tool": { "name": name } })),
            BedrockToolChoice::None => None,
        }
    }
}

/// Per-API typed options for `bedrock-converse-stream` (pi `BedrockOptions`,
/// `bedrock-converse-stream.ts:68-100`).
///
/// `reasoning` and `thinkingBudgets` are NOT modelled here: cyrup carries them on the unified
/// [`StreamOptions::reasoning`] / [`StreamOptions::thinking_budgets`], and `build_params`
/// performs the same lowering upstream's `streamSimple` does (`:403-449`) — matching how
/// `anthropic-messages` already handles them. Every field defaults to `None`, reproducing pi's
/// defaults exactly. Carried via [`StreamOptions::api_options`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BedrockOptions {
    /// pi `region` (`:69`) — the highest-priority region source after an ARN-embedded region.
    pub region: Option<String>,
    /// pi `profile` (`:70`) — a shared-config profile that must beat ambient access keys.
    pub profile: Option<String>,
    /// pi `toolChoice` (`:71`).
    pub tool_choice: Option<BedrockToolChoice>,
    /// pi `interleavedThinking` (`:77`); `None` ⇒ pi default `true`.
    pub interleaved_thinking: Option<bool>,
    /// pi `thinkingDisplay` (`:88`); `None` ⇒ pi default `"summarized"`.
    pub thinking_display: Option<BedrockThinkingDisplay>,
    /// pi `requestMetadata` (`:93`) — cost-allocation tags echoed into the request body.
    pub request_metadata: Option<BTreeMap<String, String>>,
    /// pi `bearerToken` (`:99`) — Bedrock API-key auth, bypassing SigV4.
    pub bearer_token: Option<String>,
}

impl BedrockOptions {
    /// Resolve the typed options a caller can actually reach through cyrup's unified
    /// [`StreamOptions`].
    ///
    /// `toolChoice` is the one option with a unified spelling, so the unified value wins and the
    /// typed one is the fallback — the ranking every other ported api uses. The remaining six are
    /// typed-options-only and would be silently unreachable without this resolution.
    pub fn from_stream_options(opts: &StreamOptions) -> Self {
        let typed = opts
            .api_options
            .as_ref()
            .and_then(crate::stream::ApiStreamOptions::bedrock);

        Self {
            region: typed.and_then(|t| t.region.clone()),
            profile: typed.and_then(|t| t.profile.clone()),
            tool_choice: opts
                .tool_choice
                .as_ref()
                .map(BedrockToolChoice::from_unified)
                .or_else(|| typed.and_then(|t| t.tool_choice.clone())),
            interleaved_thinking: typed.and_then(|t| t.interleaved_thinking),
            thinking_display: typed.and_then(|t| t.thinking_display),
            request_metadata: typed.and_then(|t| t.request_metadata.clone()),
            bearer_token: typed.and_then(|t| t.bearer_token.clone()),
        }
    }
}
