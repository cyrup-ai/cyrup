//! Typed per-API options.

use crate::stream::{StreamOptions, ToolChoice};

/// Reasoning-summary verbosity (pi `OpenAICodexResponsesOptions.reasoningSummary`, `:88`:
/// `"auto" | "concise" | "detailed" | "off" | "on" | null`). An absent value and an explicit `null`
/// both fall back to `"auto"` (`options.reasoningSummary ?? "auto"`, `:590`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexReasoningSummary {
    Auto,
    Concise,
    Detailed,
    Off,
    On,
}

impl CodexReasoningSummary {
    /// The wire string for this summary level.
    pub fn as_str(self) -> &'static str {
        match self {
            CodexReasoningSummary::Auto => "auto",
            CodexReasoningSummary::Concise => "concise",
            CodexReasoningSummary::Detailed => "detailed",
            CodexReasoningSummary::Off => "off",
            CodexReasoningSummary::On => "on",
        }
    }
}

/// The `tool_choice` values Codex accepts (pi `OpenAICodexResponsesOptions.toolChoice`, `:91`:
/// `"auto" | "none" | "required"` — note it has **no** named-function form, unlike the
/// `openai-completions` option of the same name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexToolChoice {
    Auto,
    None,
    Required,
}

impl CodexToolChoice {
    pub fn as_str(self) -> &'static str {
        match self {
            CodexToolChoice::Auto => "auto",
            CodexToolChoice::None => "none",
            CodexToolChoice::Required => "required",
        }
    }

    /// Narrow cyrup's unified [`ToolChoice`] to the three values Codex's option type admits. The
    /// named-function form is not representable in `OpenAICodexResponsesOptions["toolChoice"]`, so
    /// it yields `None` and the caller falls back to upstream's `?? "auto"` default (`:562`).
    fn from_unified(choice: &ToolChoice) -> Option<Self> {
        match choice {
            ToolChoice::Auto => Some(CodexToolChoice::Auto),
            ToolChoice::None => Some(CodexToolChoice::None),
            ToolChoice::Required => Some(CodexToolChoice::Required),
            ToolChoice::Function { .. } => None,
        }
    }
}

/// Per-API typed options for `openai-codex-responses` (pi `OpenAICodexResponsesOptions`,
/// `openai-codex-responses.ts:86-92`).
///
/// `reasoningEffort` is not modelled here: cyrup carries the unified reasoning level on
/// [`StreamOptions::reasoning`] and [`build_request_body`](super::request::build_request_body) clamps it exactly as upstream's
/// `streamSimple` does (`:516-517`), matching how `openai-responses` and `azure-openai-responses`
/// already handle it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenAiCodexResponsesOptions {
    /// pi `reasoningSummary` (`:88`); `None` ⇒ `"auto"`.
    pub reasoning_summary: Option<CodexReasoningSummary>,
    /// pi `serviceTier` (`:89`); omitted from the body when `None` (`:570-572`).
    pub service_tier: Option<String>,
    /// pi `textVerbosity` (`:90`); `None` ⇒ `"low"` (`:559`).
    pub text_verbosity: Option<String>,
    /// pi `toolChoice` (`:91`); `None` ⇒ `"auto"` (`:562`).
    pub tool_choice: Option<CodexToolChoice>,
}

impl OpenAiCodexResponsesOptions {
    /// Derive the typed options reachable through cyrup's unified [`StreamOptions`].
    ///
    /// Only `toolChoice` has a unified spelling; `serviceTier`, `textVerbosity` and
    /// `reasoningSummary` are typed-options-only on every ported Responses api (the same position
    /// `azure-openai-responses` documents for its `reasoningSummary`) and keep upstream's defaults
    /// here. Note pi's own `buildBaseOptions` (`simple-options.ts`) forwards **no** `toolChoice`
    /// either, so upstream's `streamSimple` path is always `"auto"`.
    pub fn from_stream_options(opts: &StreamOptions) -> Self {
        // Typed options first, exactly as every sibling Responses api does. Without this branch
        // three of upstream's four options — `reasoningSummary`, `serviceTier`, `textVerbosity` —
        // were UNREACHABLE: they had no unified spelling, and nothing read
        // `StreamOptions::api_options`, so a caller could construct them and they would be
        // silently discarded.
        let typed = opts
            .api_options
            .as_ref()
            .and_then(crate::stream::ApiStreamOptions::openai_codex_responses);

        Self {
            reasoning_summary: typed.and_then(|t| t.reasoning_summary),
            service_tier: typed.and_then(|t| t.service_tier.clone()),
            text_verbosity: typed.and_then(|t| t.text_verbosity.clone()),
            // `toolChoice` is the one option with a unified spelling, so the unified value wins and
            // the typed one is the fallback — matching how the other Responses apis rank them.
            tool_choice: opts
                .tool_choice
                .as_ref()
                .and_then(CodexToolChoice::from_unified)
                .or_else(|| typed.and_then(|t| t.tool_choice)),
        }
    }
}
