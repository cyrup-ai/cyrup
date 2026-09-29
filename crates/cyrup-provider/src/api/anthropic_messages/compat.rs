//! Compat resolution (Pi getAnthropicCompat, anthropic-messages.ts:170-181)

use crate::api::compat::AnthropicMessagesCompat;
use crate::model::Model;

/// Resolved Anthropic compat (Pi `Required<Omit<AnthropicMessagesCompat,"forceAdaptiveThinking">>`).
pub(super) struct ResolvedAnthropicCompat {
    pub(super) supports_eager_tool_input_streaming: bool,
    pub(super) supports_long_cache_retention: bool,
    pub(super) send_session_affinity_headers: bool,
    pub(super) supports_cache_control_on_tools: bool,
    pub(super) supports_temperature: bool,
    pub(super) allow_empty_signature: bool,
    /// Pi `supportsStrictTools: model.compat?.supportsStrictTools ?? false`
    /// (`anthropic-messages.ts:183` @v0.83.0, type at `types.ts:639`) — the model accepts
    /// `tools[].strict: true` plus the FULL JSON schema in `input_schema`. PROV-011.
    pub(super) supports_strict_tools: bool,
    /// DRIFT-001: emit `tool_reference` blocks + `defer_loading` tools. Defaults from
    /// [`default_supports_tool_references`], NOT to a constant.
    pub(super) supports_tool_references: bool,
}

/// 1:1 port of Pi `getAnthropicCompat` (anthropic-messages.ts:170-181): every field defaults on,
/// except `sendSessionAffinityHeaders`/`allowEmptySignature` which default off.
pub(super) fn get_anthropic_compat(model: &Model) -> ResolvedAnthropicCompat {
    let c: Option<&AnthropicMessagesCompat> = model.compat.as_ref();
    ResolvedAnthropicCompat {
        supports_eager_tool_input_streaming: c
            .and_then(|c| c.supports_eager_tool_input_streaming)
            .unwrap_or(true),
        supports_long_cache_retention: c
            .and_then(|c| c.supports_long_cache_retention)
            .unwrap_or(true),
        send_session_affinity_headers: c
            .and_then(|c| c.send_session_affinity_headers)
            .unwrap_or(false),
        supports_cache_control_on_tools: c
            .and_then(|c| c.supports_cache_control_on_tools)
            .unwrap_or(true),
        supports_temperature: c.and_then(|c| c.supports_temperature).unwrap_or(true),
        allow_empty_signature: c.and_then(|c| c.allow_empty_signature).unwrap_or(false),
        supports_strict_tools: c.and_then(|c| c.supports_strict_tools).unwrap_or(false),
        supports_tool_references: c
            .and_then(|c| c.supports_tool_references)
            .unwrap_or_else(|| default_supports_tool_references(model)),
    }
}

/// `getAnthropicCompat(model).supportsMidConvoSystemMessages` — the DECLARED value, else the runtime
/// default (PROV-083a). This is the boolean the adapter hands to
/// [`resolve_transcript`](crate::utils::transcript::resolve_transcript)
/// (`anthropic-messages.ts:517` @v0.87.1: `resolveTranscript(context, compat.supportsMidConvoSystemMessages)`).
///
/// A free function rather than a [`ResolvedAnthropicCompat`] field because the block emitter that
/// consumes it is PROV-083b; a field nothing reads yet would be dead code, and 083b folds both flags
/// into the struct with the emitter that reads them. The resolution PRECEDENCE — declared over
/// runtime default — is upstream's and is fixed here, not there.
pub fn supports_mid_convo_system_messages(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|c| c.supports_mid_convo_system_messages)
        .unwrap_or_else(|| default_supports_mid_convo_system_messages(model))
}

/// `getAnthropicCompat(model).supportsMidConvoToolChanges` — the DECLARED value, else the runtime
/// default (PROV-083a). See [`supports_mid_convo_system_messages`] for why this is a free function.
///
/// Upstream never reads this flag alone: the `tool_addition`/`tool_removal` emitter is gated on the
/// CONJUNCTION `supportsMidConvoSystemMessages && supportsMidConvoToolChanges`
/// (`anthropic-messages.ts:1052-1053` @v0.87.1), which is what *"Requires
/// `supportsMidConvoSystemMessages`"* (`types.ts:831`) means operationally.
pub fn supports_mid_convo_tool_changes(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|c| c.supports_mid_convo_tool_changes)
        .unwrap_or_else(|| default_supports_mid_convo_tool_changes(model))
}

/// `supportsAnthropicMidConvoSystemMessages` (`packages/ai/scripts/generate-models.ts:606-611`
/// @v0.87.1), hand-rolled exactly as [`default_supports_tool_references`] hand-rolls DRIFT-001's
/// regex, because cyrup carries no `regex` crate.
///
/// Upstream's two anchored alternatives, tested against the RAW model id (this predicate does NOT
/// lowercase or strip a `~anthropic/` prefix — `supportsAnthropicMidConvoEffort` at `:598-604` does,
/// and the difference is upstream's, not a transcription slip):
///
/// ```text
/// ^claude-opus-(?:4[.-]8|5(?:[.-]5)?)(?:-\d{8})?$
/// ^claude-(?:fable|mythos)-5(?:[.-]1)?(?:-\d{8})?$
/// ```
///
/// So: Opus 4.8, Opus 5, Opus 5.5, Fable 5, Fable 5.1, Mythos 5, Mythos 5.1 — each with an optional
/// 8-digit date suffix, each spelled with `.` or `-` between the major and minor. Everything else,
/// including Opus 4.6/4.7 and every Sonnet and Haiku, is false.
///
/// Both patterns are fully ANCHORED and the only quantified group is `\d{8}`, so there is no
/// backtracking to reproduce: each alternative is a fixed sequence of literal choices.
fn supports_anthropic_mid_convo_system_messages(id: &str) -> bool {
    /// `(?:-\d{8})?$` — the optional date suffix, then end of string.
    fn date_suffix_then_end(rest: &str) -> bool {
        if rest.is_empty() {
            return true;
        }
        let Some(digits) = rest.strip_prefix('-') else {
            return false;
        };
        digits.len() == 8 && digits.bytes().all(|b| b.is_ascii_digit())
    }
    /// `(?:[.-]<minor>)?` — the optional dot-or-dash minor, then the tail check.
    fn optional_minor_then_end(rest: &str, minor: &str) -> bool {
        for sep in ['.', '-'] {
            let mut spelled = String::with_capacity(1 + minor.len());
            spelled.push(sep);
            spelled.push_str(minor);
            if let Some(tail) = rest.strip_prefix(spelled.as_str())
                && date_suffix_then_end(tail)
            {
                return true;
            }
        }
        // The group did not participate.
        date_suffix_then_end(rest)
    }

    // `^claude-opus-(?:4[.-]8|5(?:[.-]5)?)(?:-\d{8})?$`
    if let Some(rest) = id.strip_prefix("claude-opus-") {
        // `4[.-]8` — the minor is MANDATORY in this alternative, so a bare `claude-opus-4` is out.
        for sep in ['.', '-'] {
            let mut spelled = String::from("4");
            spelled.push(sep);
            spelled.push('8');
            if let Some(tail) = rest.strip_prefix(spelled.as_str())
                && date_suffix_then_end(tail)
            {
                return true;
            }
        }
        // `5(?:[.-]5)?`
        if let Some(tail) = rest.strip_prefix('5')
            && optional_minor_then_end(tail, "5")
        {
            return true;
        }
    }

    // `^claude-(?:fable|mythos)-5(?:[.-]1)?(?:-\d{8})?$`
    if let Some(rest) = id.strip_prefix("claude-")
        && let Some(rest) = ["fable-", "mythos-"]
            .iter()
            .find_map(|p| rest.strip_prefix(p))
        && let Some(tail) = rest.strip_prefix('5')
        && optional_minor_then_end(tail, "1")
    {
        return true;
    }

    false
}

/// Default for `supportsMidConvoSystemMessages` on the anthropic route (PROV-083a).
///
/// Upstream assigns the flag in `getAnthropicMessagesCompat`
/// (`generate-models.ts:1208-1216` @v0.87.1) under two provider gates:
///
/// * `provider === "anthropic"` → both this flag and `supportsMidConvoToolChanges` (`:1208-1211`);
/// * `provider === "opencode" || provider === "github-copilot"` → THIS FLAG ONLY (`:1212-1216`),
///   because, in upstream's own words, those two *"forward mid-conversation system messages but
///   reject `tool_addition`/`tool_removal` blocks, so tool changes stay top-level there."*
///
/// It lives as a RUNTIME predicate rather than as catalog data for the reason DRIFT-001 recorded for
/// `supportsToolReferences`: not one of cyrup's 35 catalog files carries a `supportsMidConvo*` key,
/// so with a constant `false` default the entire mid-conversation port would be unreachable code.
pub fn default_supports_mid_convo_system_messages(model: &Model) -> bool {
    matches!(
        model.provider.as_str(),
        "anthropic" | "opencode" | "github-copilot"
    ) && supports_anthropic_mid_convo_system_messages(model.id.as_str())
}

/// Default for `supportsMidConvoToolChanges` on the anthropic route (PROV-083a) — the
/// `provider === "anthropic"` half of `generate-models.ts:1208-1211`, which is the only gate that
/// sets it. `opencode` and `github-copilot` reject the blocks, so they are absent here.
pub fn default_supports_mid_convo_tool_changes(model: &Model) -> bool {
    model.provider.as_str() == "anthropic"
        && supports_anthropic_mid_convo_system_messages(model.id.as_str())
}

/// Default for `supportsToolReferences` (1:1 port of Pi `defaultSupportsToolReferences`,
/// anthropic-messages.ts:193-199): first-party Anthropic models except Haiku (which rejects
/// client-side `tool_reference` blocks) and models that predate tool search (Claude 3.x,
/// Opus/Sonnet 4.0, Opus 4.1).
///
/// Pi's predicate is
/// `/^claude-(?:opus|sonnet|fable)-(\d+)(?:-(\d+))?(?:-|$)/`. cyrup has no `regex` dependency
/// (`utils/regexlite` is a case-insensitive substring matcher, not a capture engine), so the
/// capture is hand-rolled. Greedy-only scanning is EXACT here: if the greedy `(\d+)` overruns,
/// every backtracked position leaves a digit next, and a digit satisfies neither `-(\d+)` nor
/// `(?:-|$)`, so backtracking can never rescue a match.
///
/// The `version[2].length < 8` guard is the DATE-SUFFIX gate and is load-bearing:
/// `claude-sonnet-4-20250514` captures `"20250514"` (8 chars) → minor 0 → **false**, while
/// `claude-opus-4-5-20251101` captures `"5"` → minor 5 → **true**.
pub(super) fn default_supports_tool_references(model: &Model) -> bool {
    let id = model.id.as_str();
    if model.provider.as_str() != "anthropic" || id.contains("haiku") {
        return false;
    }
    let Some(rest) = id.strip_prefix("claude-") else {
        return false;
    };
    // `(?:opus|sonnet|fable)-`
    let Some(rest) = ["opus-", "sonnet-", "fable-"]
        .iter()
        .find_map(|p| rest.strip_prefix(p))
    else {
        return false;
    };

    // `(\d+)` — greedy.
    let major_len = rest.chars().take_while(char::is_ascii_digit).count();
    if major_len == 0 {
        return false;
    }
    let (Some(major_digits), Some(after_major)) = (rest.get(..major_len), rest.get(major_len..))
    else {
        return false;
    };
    let Ok(major) = major_digits.parse::<u32>() else {
        return false;
    };

    // `(?:-(\d+))?(?:-|$)`
    let mut minor: u32 = 0;
    if after_major.is_empty() {
        // `$` matches; the optional minor group did not participate.
    } else if let Some(tail) = after_major.strip_prefix('-') {
        let minor_len = tail.chars().take_while(char::is_ascii_digit).count();
        let minor_captured = tail.get(..minor_len).unwrap_or("");
        let remainder = tail.get(minor_len..).unwrap_or("");
        // The optional group participates only if it is followed by `-` or end of string;
        // otherwise the regex backtracks and `(?:-|$)` consumes the `-` we just stripped.
        if minor_len > 0 && (remainder.is_empty() || remainder.starts_with('-')) {
            // `version[2] && version[2].length < 8 ? Number(version[2]) : 0`
            minor = if minor_captured.len() < 8 {
                minor_captured.parse::<u32>().unwrap_or(0)
            } else {
                0
            };
        }
    } else {
        // Neither `-` nor end of string after the major version → no match at all.
        return false;
    }

    major > 4 || (major == 4 && minor >= 5)
}

/// `model.compat?.forceAdaptiveThinking === true` (Pi default false).
pub(super) fn force_adaptive_thinking(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|c| c.force_adaptive_thinking)
        .unwrap_or(false)
}

/// `model.compat?.supportsMidConvoEffort === true` (Pi default false) — the model accepts per-turn
/// `output_config` markers, so effort travels as one `{role:"system",content:[],output_config}`
/// message per recorded turn rather than as a single request-level `output_config.effort`.
///
/// Read DIRECTLY off `model.compat`, NOT through [`get_anthropic_compat`], because pi's
/// `getAnthropicCompat` (`anthropic-messages.ts:206-220` @v0.87.1) does not resolve it either: every
/// upstream reader — the `providerThinkingLevel` seed (`:521`), the beta push (`:1028`), the
/// `convertMessages` managed-provider argument (`:1061`), the `insertThinkingLevelMessages` gate
/// (`:1069`), the temperature gate (`:1108`) and the managed thinking branch (`:1153`) — goes
/// straight to `model.compat`, exactly as
/// [`should_use_server_side_fallback_beta`](super::headers::should_use_server_side_fallback_beta)
/// already documents for `allowedFallbackModels`. PROV-091.
pub(super) fn supports_mid_convo_effort(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|c| c.supports_mid_convo_effort)
        .unwrap_or(false)
}

/// `model.thinkingLevelMap?.off !== null` (a missing key is `undefined`, which `!== null`).
pub(super) fn off_is_not_null(model: &Model) -> bool {
    !matches!(
        model.thinking_level_map.as_ref().and_then(|m| m.get("off")),
        Some(None)
    )
}
