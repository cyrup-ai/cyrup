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

/// Mistral `reasoningEffort` (Pi `MistralReasoningEffort`, mistral-conversations.ts:34). Read
/// verbatim from `MistralOptions.reasoningEffort` in `buildChatPayload`
/// (mistral-conversations.ts:257).
///
/// PROV-114 — `dc84c1ac0` widened this from `"none" | "high"` to
/// `"none" | "low" | "medium" | "high" | "max"` in the same commit that replaced the hardcoded
/// model-id rule with the `thinkingLevelMap` presence test. The three new values are reachable
/// from cyrup's own catalog: `providers/catalog/mistral.json` maps `zai-glm-5-3` to `low`/`high`/
/// `max` and `zai-glm-5-2` to `none`/`high`/`max`, so the computed lowering already emits them —
/// this is the per-request **override** surface catching up, which without it cannot express a
/// value the server accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MistralReasoningEffort {
    /// `"none"`.
    None,
    /// `"low"`.
    Low,
    /// `"medium"`.
    Medium,
    /// `"high"`.
    High,
    /// `"max"`.
    Max,
}

impl MistralReasoningEffort {
    /// The exact `reasoningEffort` wire string.
    pub fn as_wire(self) -> &'static str {
        match self {
            MistralReasoningEffort::None => "none",
            MistralReasoningEffort::Low => "low",
            MistralReasoningEffort::Medium => "medium",
            MistralReasoningEffort::High => "high",
            MistralReasoningEffort::Max => "max",
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
