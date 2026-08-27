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

/// `model.thinkingLevelMap?.off !== null` (a missing key is `undefined`, which `!== null`).
pub(super) fn off_is_not_null(model: &Model) -> bool {
    !matches!(
        model.thinking_level_map.as_ref().and_then(|m| m.get("off")),
        Some(None)
    )
}
