//! The per-request `GoogleOptions` surface carried on `StreamOptions::api_options`: the
//! direct `thinkingConfig` override Pi reads in `buildParams` (google-generative-ai.ts:373-384)
//! and the `GoogleThinkingLevel` wire strings (google-shared.ts:16).


/// A Gemini `thinkingLevel` value (Pi `GoogleThinkingLevel`, google-shared.ts:16). Serialized to the
/// exact wire string Pi passes through unchanged in `buildParams` (`options.thinking.level as any`,
/// google-generative-ai.ts:377-378).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GoogleThinkingLevel {
    /// `"THINKING_LEVEL_UNSPECIFIED"`.
    Unspecified,
    /// `"MINIMAL"`.
    Minimal,
    /// `"LOW"`.
    Low,
    /// `"MEDIUM"`.
    Medium,
    /// `"HIGH"`.
    High,
}

impl GoogleThinkingLevel {
    /// The exact `thinkingLevel` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            GoogleThinkingLevel::Unspecified => "THINKING_LEVEL_UNSPECIFIED",
            GoogleThinkingLevel::Minimal => "MINIMAL",
            GoogleThinkingLevel::Low => "LOW",
            GoogleThinkingLevel::Medium => "MEDIUM",
            GoogleThinkingLevel::High => "HIGH",
        }
    }
}

/// A direct per-request `thinking` override (Pi `GoogleOptions.thinking`,
/// google-generative-ai.ts:40-44). When present it is read verbatim by [`build_params`](super::params::build_params), bypassing
/// the unified-`reasoning`-driven lowering — mirroring Pi's `buildParams` reading `options.thinking`
/// directly (google-generative-ai.ts:373-384) rather than the value `streamSimple` would compute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GoogleThinking {
    /// `thinking.enabled` (google-generative-ai.ts:41). `false` lowers to the model's
    /// disabled-thinking config.
    pub enabled: bool,
    /// `thinking.budgetTokens` (google-generative-ai.ts:42): `-1` for dynamic, `0` to disable. Only
    /// honored when `level` is `None` (Pi prefers `level` over `budgetTokens`).
    pub budget_tokens: Option<i64>,
    /// `thinking.level` (google-generative-ai.ts:43). Takes precedence over `budget_tokens`.
    pub level: Option<GoogleThinkingLevel>,
}

/// Per-API typed options for the `google-generative-ai` wire protocol (Pi `GoogleOptions`,
/// google-generative-ai.ts:38-45). Only the fields cyrup does not already carry on the unified
/// [`StreamOptions`](crate::stream::StreamOptions) live here: `toolChoice` folds onto
/// `StreamOptions.tool_choice` and the simple reasoning level onto `StreamOptions.reasoning`, but a
/// direct `thinking.{budgetTokens,level}` per-request override has no other home. Carried via
/// [`StreamOptions::api_options`](crate::stream::StreamOptions::api_options); defaults to `None` (no
/// override), reproducing the streamSimple-driven behavior exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GoogleOptions {
    /// Direct `thinkingConfig` override (Pi `GoogleOptions.thinking`). `None` = no override: the
    /// unified `reasoning` level drives `thinkingConfig` as before.
    pub thinking: Option<GoogleThinking>,
}
