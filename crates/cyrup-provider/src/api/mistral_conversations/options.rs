//! The per-request `MistralOptions` surface carried on `StreamOptions::api_options`: the
//! direct `promptMode` / `reasoningEffort` overrides (Pi `MistralOptions`,
//! mistral-conversations.ts:41-48).


/// Mistral `promptMode` (Pi `MistralOptions.promptMode`, mistral-conversations.ts:41). The only
/// value Pi defines is `"reasoning"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MistralPromptMode {
    /// `"reasoning"`.
    Reasoning,
}

impl MistralPromptMode {
    /// The exact `promptMode` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            MistralPromptMode::Reasoning => "reasoning",
        }
    }
}

/// Mistral `reasoningEffort` (Pi `MistralReasoningEffort = "none" | "high"`,
/// mistral-conversations.ts:37). Read verbatim from `MistralOptions.reasoningEffort` in
/// `buildChatPayload` (mistral-conversations.ts:257).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MistralReasoningEffort {
    /// `"none"`.
    None,
    /// `"high"`.
    High,
}

impl MistralReasoningEffort {
    /// The exact `reasoningEffort` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            MistralReasoningEffort::None => "none",
            MistralReasoningEffort::High => "high",
        }
    }
}

/// Per-API typed options for the `mistral-conversations` wire protocol (Pi `MistralOptions`,
/// mistral-conversations.ts:39-43). `toolChoice` folds onto `StreamOptions.tool_choice` and the
/// simple reasoning level onto `StreamOptions.reasoning`; only a direct `promptMode` per-request
/// override has no other home. Carried via
/// [`StreamOptions::api_options`](crate::StreamOptions::api_options); defaults to `None` (no
/// override), reproducing the streamSimple-driven behavior exactly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MistralOptions {
    /// Direct `promptMode` override (Pi `buildChatPayload` reads `options.promptMode`,
    /// mistral-conversations.ts:256). `None` = no override: the unified `reasoning` level drives
    /// `promptMode` as before.
    pub prompt_mode: Option<MistralPromptMode>,
    /// Direct `reasoningEffort` override (Pi `buildChatPayload` reads `options.reasoningEffort`
    /// verbatim, mistral-conversations.ts:257). `None` = no override: the unified `reasoning` level
    /// drives `reasoningEffort` via `lower_reasoning`. Set independently of `prompt_mode`, exactly
    /// like Pi's two independent `if (options?.…)` guards.
    pub reasoning_effort: Option<MistralReasoningEffort>,
}
