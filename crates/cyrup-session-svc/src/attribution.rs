//! `merge_provider_attribution_headers` — the attribution + session headers Pi attaches to every
//! provider request (Pi `provider-attribution.ts`, used by `sdk.ts:323-330`). A 1:1 port of the
//! host-match + telemetry-gated default-attribution + opencode session-affinity logic.
//!
//! Pi computes these per-request inside `streamFn`, dispatched on the model the request is actually
//! going to (`sdk.ts:318-327`).
//!
//! This module's doc used to claim the inputs "are stable across a run, so the facade computes the
//! merged map once". That premise was FALSE: the active [`cyrup_provider::Model`] changes on every
//! `/model` switch, and the merge is host-matched on it. Computing once at session build and pinning
//! it via `AgentBuilder::headers` meant a cross-provider switch kept sending the previous provider's
//! attribution — an OpenRouter `HTTP-Referer`/`X-Title` on an Anthropic request, or a stale opencode
//! session-affinity header.
//!
//! The merged map now lives in `StateInner::headers` and is recomputed by
//! [`crate::AgentSession::attribution_headers`] on BOTH model-change paths, so each turn reads the
//! overlay for its own provider — matching Pi's per-request merge in behaviour, not just in wording.

use cyrup_core::SessionId;
use cyrup_provider::{HeaderMap, Model};

const OPENROUTER_HOST: &str = "openrouter.ai";
const NVIDIA_NIM_HOST: &str = "integrate.api.nvidia.com";
const CLOUDFLARE_API_HOST: &str = "api.cloudflare.com";
const CLOUDFLARE_AI_GATEWAY_HOST: &str = "gateway.ai.cloudflare.com";
const OPENCODE_HOST: &str = "opencode.ai";
const VERCEL_GATEWAY_HOST: &str = "ai-gateway.vercel.sh";

/// Whether `base_url`'s host equals `expected_host` (Pi `matchesHost`, provider-attribution.ts:12).
/// A non-URL base falls through to `false` (Pi `new URL(...)` throws → `catch` returns false).
fn matches_host(base_url: &str, expected_host: &str) -> bool {
    host_of(base_url).is_some_and(|h| h == expected_host)
}

/// Extract the host component from a URL string without a URL-parsing dependency: take the authority
/// between `scheme://` and the first `/`, `?`, or `#`, then strip any `user@` and `:port`.
fn host_of(base_url: &str) -> Option<String> {
    let after_scheme = base_url.split_once("://").map(|(_, rest)| rest)?;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, hp)| hp);
    // Strip a `:port` suffix (ignore the colon inside an IPv6 literal — not used by our hosts).
    let host = host_port.split_once(':').map_or(host_port, |(h, _)| h);
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn is_openrouter(model: &Model) -> bool {
    model.provider.as_str() == "openrouter" || model.base_url.contains(OPENROUTER_HOST)
}

fn is_nvidia_nim(model: &Model) -> bool {
    model.provider.as_str() == "nvidia" || matches_host(&model.base_url, NVIDIA_NIM_HOST)
}

fn is_cloudflare(model: &Model) -> bool {
    matches!(
        model.provider.as_str(),
        "cloudflare-workers-ai" | "cloudflare-ai-gateway"
    ) || matches_host(&model.base_url, CLOUDFLARE_API_HOST)
        || matches_host(&model.base_url, CLOUDFLARE_AI_GATEWAY_HOST)
}

fn is_vercel_gateway(model: &Model) -> bool {
    model.provider.as_str() == "vercel-ai-gateway"
        || matches_host(&model.base_url, VERCEL_GATEWAY_HOST)
}

/// Telemetry-gated default attribution headers (Pi `getDefaultAttributionHeaders`,
/// provider-attribution.ts:40-77). Returns `None` when telemetry is disabled or no host matches.
fn default_attribution_headers(model: &Model, telemetry_enabled: bool) -> Option<Vec<(&'static str, &'static str)>> {
    if !telemetry_enabled {
        return None;
    }
    if is_openrouter(model) {
        return Some(vec![
            ("HTTP-Referer", "https://pi.dev"),
            ("X-OpenRouter-Title", "pi"),
            ("X-OpenRouter-Categories", "cli-agent"),
        ]);
    }
    if is_nvidia_nim(model) {
        return Some(vec![("X-BILLING-INVOKE-ORIGIN", "Pi")]);
    }
    if is_cloudflare(model) {
        return Some(vec![("User-Agent", "pi-coding-agent")]);
    }
    if is_vercel_gateway(model) {
        return Some(vec![("http-referer", "https://pi.dev"), ("x-title", "pi")]);
    }
    None
}

/// Opencode session-affinity headers (Pi `getSessionHeaders`, provider-attribution.ts:79-88).
fn session_headers(model: &Model, session_id: Option<&SessionId>) -> Option<Vec<(&'static str, String)>> {
    let sid = session_id?;
    let is_opencode = matches!(model.provider.as_str(), "opencode" | "opencode-go")
        || matches_host(&model.base_url, OPENCODE_HOST);
    if !is_opencode {
        return None;
    }
    Some(vec![
        ("x-opencode-session", sid.as_str().to_string()),
        ("x-opencode-client", "pi".to_string()),
    ])
}

/// Merge the opencode session headers + telemetry-gated default attribution headers + any caller
/// `extra` overlays (later overlays win), returning `None` when the merged map is empty (Pi
/// `mergeProviderAttributionHeaders`, provider-attribution.ts:90-108). The value type matches the
/// provider [`HeaderMap`] (`Option<String>`; a `None` value suppresses a default header).
pub fn merge_provider_attribution_headers(
    model: &Model,
    telemetry_enabled: bool,
    session_id: Option<&SessionId>,
    extra: &[&HeaderMap],
) -> Option<HeaderMap> {
    let mut merged: HeaderMap = HeaderMap::new();
    if let Some(sh) = session_headers(model, session_id) {
        for (k, v) in sh {
            merged.insert(k.to_string(), Some(v));
        }
    }
    if let Some(da) = default_attribution_headers(model, telemetry_enabled) {
        for (k, v) in da {
            merged.insert(k.to_string(), Some(v.to_string()));
        }
    }
    for overlay in extra {
        for (k, v) in overlay.iter() {
            merged.insert(k.clone(), v.clone());
        }
    }
    if merged.is_empty() {
        None
    } else {
        Some(merged)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_provider::Model;

    fn model(provider: &str, base_url: &str) -> Model {
        Model {
            id: "m1".into(),
            name: "m1".to_string(),
            api: "openai-completions".into(),
            provider: provider.into(),
            base_url: base_url.to_string(),
            reasoning: false,
            input: Vec::new(),
            cost: Default::default(),
            context_window: 1000,
            max_tokens: 1000,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    #[test]
    fn openrouter_attribution_only_when_telemetry_on() {
        let m = model("openrouter", "https://openrouter.ai/api/v1");
        assert!(merge_provider_attribution_headers(&m, false, None, &[]).is_none());
        let h = merge_provider_attribution_headers(&m, true, None, &[]).unwrap();
        assert_eq!(h.get("X-OpenRouter-Title"), Some(&Some("pi".to_string())));
        assert_eq!(h.get("HTTP-Referer"), Some(&Some("https://pi.dev".to_string())));
    }

    #[test]
    fn opencode_session_headers_present_even_without_telemetry() {
        let m = model("opencode", "https://opencode.ai/v1");
        let sid = SessionId::from("sess-42");
        let h = merge_provider_attribution_headers(&m, false, Some(&sid), &[]).unwrap();
        assert_eq!(h.get("x-opencode-session"), Some(&Some("sess-42".to_string())));
        assert_eq!(h.get("x-opencode-client"), Some(&Some("pi".to_string())));
    }

    #[test]
    fn host_match_detects_provider_from_base_url() {
        // Provider id differs but the base_url host matches → attribution still applies.
        let m = model("custom", "https://integrate.api.nvidia.com/v1");
        let h = merge_provider_attribution_headers(&m, true, None, &[]).unwrap();
        assert_eq!(h.get("X-BILLING-INVOKE-ORIGIN"), Some(&Some("Pi".to_string())));
    }

    #[test]
    fn caller_overlay_wins_and_empty_is_none() {
        let m = model("anthropic", "https://api.anthropic.com");
        // No host match, no session → empty → None.
        assert!(merge_provider_attribution_headers(&m, true, None, &[]).is_none());
        // Overlay still surfaces.
        let mut overlay = HeaderMap::new();
        overlay.insert("X-Custom".to_string(), Some("v".to_string()));
        let h = merge_provider_attribution_headers(&m, true, None, &[&overlay]).unwrap();
        assert_eq!(h.get("X-Custom"), Some(&Some("v".to_string())));
    }
}
