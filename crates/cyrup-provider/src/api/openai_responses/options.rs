//! Per-API typed options for `openai-responses` (Pi `OpenAIResponsesOptions`,
//! openai-responses.ts:78-82).

/// Reasoning-summary verbosity (Pi `reasoningSummary`, openai-responses.ts:80:
/// `"auto" | "detailed" | "concise" | null`). `Null` reproduces Pi's explicit `null`, which — like
/// an absent value — falls back to `"auto"` (`options?.reasoningSummary || "auto"`, line 257).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningSummary {
    Auto,
    Detailed,
    Concise,
    Null,
}

impl ReasoningSummary {
    /// The wire string for the `reasoning.summary` field, or `None` for `null`.
    fn as_wire(self) -> Option<&'static str> {
        match self {
            ReasoningSummary::Auto => Some("auto"),
            ReasoningSummary::Detailed => Some("detailed"),
            ReasoningSummary::Concise => Some("concise"),
            ReasoningSummary::Null => None,
        }
    }
}

/// Per-API typed options for the `openai-responses` wire protocol (Pi `OpenAIResponsesOptions`,
/// openai-responses.ts:78-82). `reasoningEffort` already maps onto `StreamOptions.reasoning`; the
/// remaining fields live here, carried via
/// [`StreamOptions::api_options`](crate::StreamOptions::api_options). Defaults reproduce Pi exactly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OpenAiResponsesOptions {
    /// Reasoning-summary verbosity (Pi `reasoningSummary`, openai-responses.ts:80). `None` = Pi
    /// default (`"auto"` when a reasoning request is built).
    pub reasoning_summary: Option<ReasoningSummary>,
    /// Service tier (Pi `serviceTier`, openai-responses.ts:81). `None` = omit the field (Pi's
    /// default — `params.service_tier` is set only when `serviceTier` is defined, line 242).
    pub service_tier: Option<String>,
}

/// The wire `reasoning.summary` value for an optional [`OpenAiResponsesOptions`] (Pi
/// `options?.reasoningSummary || "auto"`, openai-responses.ts:257): a concrete non-null value wins;
/// `None`/`Null` fall back to `"auto"`.
pub(super) fn reasoning_summary_or_auto(opts: Option<&OpenAiResponsesOptions>) -> &'static str {
    reasoning_summary_wire(opts).unwrap_or("auto")
}

/// The truthy value of Pi's `options?.reasoningSummary` — `Some(wire)` when a summary was requested,
/// `None` when unset or explicitly `null` (both falsy in the `||` at `openai-responses.ts:313`).
pub(super) fn reasoning_summary_wire(
    opts: Option<&OpenAiResponsesOptions>,
) -> Option<&'static str> {
    opts.and_then(|o| o.reasoning_summary)
        .and_then(ReasoningSummary::as_wire)
}
