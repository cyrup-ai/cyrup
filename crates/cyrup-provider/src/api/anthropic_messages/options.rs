//! Typed options (Pi `AnthropicThinkingDisplay` / `AnthropicOptions`,
//! anthropic-messages.ts:165-230).

/// How thinking content is returned (Pi `AnthropicThinkingDisplay`, anthropic-messages.ts:165).
/// `"summarized"` returns summarized thinking text; `"omitted"` returns an empty thinking field
/// (the encrypted signature still travels back for multi-turn continuity).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnthropicThinkingDisplay {
    Summarized,
    Omitted,
}

impl AnthropicThinkingDisplay {
    /// The wire string for the `thinking.display` field.
    pub fn as_wire(self) -> &'static str {
        match self {
            AnthropicThinkingDisplay::Summarized => "summarized",
            AnthropicThinkingDisplay::Omitted => "omitted",
        }
    }
}

/// Per-API typed options for the `anthropic-messages` wire protocol (Pi `AnthropicOptions`,
/// anthropic-messages.ts:183-230). Only the fields cyrup does not already carry on the unified
/// [`StreamOptions`](crate::stream::StreamOptions) live here; the rest (`thinkingEnabled`,
/// `thinkingBudgetTokens`, `effort`) map onto `StreamOptions.reasoning`/`thinking_budgets`. Carried
/// via [`StreamOptions::api_options`](crate::stream::StreamOptions::api_options). All fields default to
/// `None`, reproducing Pi's defaults exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnthropicOptions {
    /// Request the interleaved-thinking beta header for non-adaptive thinking models (Pi
    /// `interleavedThinking`, anthropic-messages.ts:230). `None` = Pi default (`true`).
    pub interleaved_thinking: Option<bool>,
    /// How thinking content is returned (Pi `thinkingDisplay`, anthropic-messages.ts:223). `None` =
    /// Pi default (`"summarized"`).
    pub thinking_display: Option<AnthropicThinkingDisplay>,
}
