//! Request encoding — endpoint resolution and request headers.

use super::claude_code::is_github_copilot;
use super::compat::{force_adaptive_thinking, get_anthropic_compat, supports_mid_convo_effort};
use crate::HeaderMap;
use crate::auth::AuthResult;
use crate::context::Context;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::provider_plumbing::{EnvSource, resolve_cache_retention};

/// The Anthropic API version header value the SDK pins by default.
pub(super) const ANTHROPIC_VERSION: &str = "2023-06-01";

/// Beta header tokens (Pi anthropic-messages.ts:167-168).
pub(super) const FINE_GRAINED_TOOL_STREAMING_BETA: &str = "fine-grained-tool-streaming-2025-05-14";
pub(super) const INTERLEAVED_THINKING_BETA: &str = "interleaved-thinking-2025-05-14";
/// Pi `SERVER_SIDE_FALLBACK_BETA` (`anthropic-messages.ts:183` @v0.87.1) — sent whenever the model
/// declares at least one `allowedFallbackModels` entry (PROV-090).
pub(super) const SERVER_SIDE_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";
/// Pi `MID_CONVERSATION_OUTPUT_CONFIG_BETA` (`anthropic-messages.ts:184` @v0.87.1) — the per-turn
/// `{role:"system",content:[],output_config:{effort}}` markers (PROV-091).
pub(super) const MID_CONVERSATION_OUTPUT_CONFIG_BETA: &str =
    "mid-conversation-output-config-2026-07-01";
/// Pi `THINKING_BINDING_CONTROLS_BETA` (`anthropic-messages.ts:185` @v0.87.1) — the
/// `thinking.block_binding.prefix_mismatch_behavior` control the managed branch always sets
/// (PROV-091). Pushed TOGETHER with [`MID_CONVERSATION_OUTPUT_CONFIG_BETA`], never alone.
pub(super) const THINKING_BINDING_CONTROLS_BETA: &str = "thinking-binding-controls-2026-08-01";

/// Stealth-mode Claude Code identity (Pi anthropic-messages.ts:73).
const CLAUDE_CODE_VERSION: &str = "2.1.75";

/// Resolve the `POST` target: an auth base-url override wins over `model.base_url`. The endpoint is
/// `{base}/v1/messages`.
pub(super) fn resolve_url(model: &Model, auth: &AuthResult) -> Option<String> {
    let base = auth
        .auth
        .base_url
        .as_deref()
        .unwrap_or(model.base_url.as_str());
    Some(messages_url(base))
}

/// Normalize a base URL to the `/v1/messages` endpoint.
pub(crate) fn messages_url(base: &str) -> String {
    let trimmed = base.trim_end_matches('/');
    if trimmed.ends_with("/v1/messages") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1/messages")
    }
}

/// `true` if the request should send the `fine-grained-tool-streaming` beta (Pi
/// `shouldUseFineGrainedToolStreamingBeta`, anthropic-messages.ts:1184-1186).
fn should_use_fine_grained_beta(model: &Model, ctx: &Context) -> bool {
    !ctx.tools.is_empty() && !get_anthropic_compat(model).supports_eager_tool_input_streaming
}

/// `true` if the request should send the `server-side-fallback` beta (1:1 port of Pi
/// `shouldUseServerSideFallbackBeta`, `anthropic-messages.ts:202-204` @v0.87.1:
/// `(model.compat?.allowedFallbackModels?.length ?? 0) > 0`).
///
/// Read straight off `model.compat` rather than through
/// [`get_anthropic_compat`](super::compat::get_anthropic_compat): pi's `getAnthropicCompat`
/// (`:206-220`) does NOT resolve this key either — both of its readers (this predicate and
/// `buildParams` `:1199-1201`) go to `model.compat` directly, because the value is a list with no
/// default rather than a flag with one.
pub(super) fn should_use_server_side_fallback_beta(model: &Model) -> bool {
    model
        .compat
        .as_ref()
        .and_then(|c| c.allowed_fallback_models.as_ref())
        .is_some_and(|f| !f.is_empty())
}

/// Build the request headers (1:1 port of Pi `createClient`, anthropic-messages.ts:813-899). The
/// auth/model/opts header overlays layer last (a `None` value suppresses a default).
pub(crate) fn build_headers(
    model: &Model,
    ctx: &Context,
    auth: &AuthResult,
    opts: &StreamOptions,
    is_oauth: bool,
) -> HeaderMap {
    // Pi `options?.interleavedThinking ?? true` (anthropic-messages.ts:520).
    let interleaved = opts
        .anthropic_options()
        .and_then(|o| o.interleaved_thinking)
        .unwrap_or(true);
    // Pi `:1019-1025` @v0.87.1, all four terms:
    //   model.reasoning && options?.thinkingEnabled === true &&
    //   (options.interleavedThinking ?? true) && model.compat?.forceAdaptiveThinking !== true
    // cyrup's `ModelThinkingLevel::Off` IS pi's `thinkingEnabled: false` — there is no third state,
    // so `opts.reasoning.is_on()` is an exact port of the second term. Before PROV-091 both the
    // `model.reasoning` and the thinking-enabled terms were missing, so the beta shipped on every
    // non-adaptive request, including reasoning-off ones that never used it.
    let needs_interleaved =
        model.reasoning && opts.reasoning.is_on() && interleaved && !force_adaptive_thinking(model);
    let mut betas: Vec<&str> = Vec::new();
    if should_use_fine_grained_beta(model, ctx) {
        betas.push(FINE_GRAINED_TOOL_STREAMING_BETA);
    }
    if needs_interleaved {
        betas.push(INTERLEAVED_THINKING_BETA);
    }
    // Pi `:1027` — pushed after the interleaved-thinking beta, in pi's order (PROV-090).
    if should_use_server_side_fallback_beta(model) {
        betas.push(SERVER_SIDE_FALLBACK_BETA);
    }
    // Pi `:1028-1030` — both, in this slot, gated on the same single flag (PROV-091).
    if supports_mid_convo_effort(model) {
        betas.push(MID_CONVERSATION_OUTPUT_CONFIG_BETA);
        betas.push(THINKING_BINDING_CONTROLS_BETA);
    }

    let mut headers = HeaderMap::new();
    headers.insert(
        "anthropic-version".to_string(),
        Some(ANTHROPIC_VERSION.to_string()),
    );
    headers.insert(
        "content-type".to_string(),
        Some("application/json".to_string()),
    );
    headers.insert("accept".to_string(), Some("application/json".to_string()));
    headers.insert(
        "anthropic-dangerous-direct-browser-access".to_string(),
        Some("true".to_string()),
    );

    if is_github_copilot(model) {
        // PROV-027 — Copilot: Bearer auth, SELECTIVE betas (Pi anthropic-messages.ts:867-888).
        // `new Anthropic({ apiKey: null, authToken: apiKey })` sends `Authorization: Bearer …`,
        // never `x-api-key`. Note what this branch deliberately does NOT send: the
        // `claude-code-20250219`/`oauth-2025-04-20` betas, the `claude-cli` user-agent, `x-app`, and
        // the session-affinity header — Copilot's edge is not Anthropic's.
        if let Some(key) = &auth.auth.api_key {
            headers.insert("authorization".to_string(), Some(format!("Bearer {key}")));
        }
        if !betas.is_empty() {
            headers.insert("anthropic-beta".to_string(), Some(betas.join(",")));
        }
    } else if is_oauth {
        // OAuth: Bearer auth + Claude Code identity headers (Pi anthropic-messages.ts:855-872).
        if let Some(key) = &auth.auth.api_key {
            headers.insert("authorization".to_string(), Some(format!("Bearer {key}")));
        }
        let mut oauth_betas = vec![
            "claude-code-20250219".to_string(),
            "oauth-2025-04-20".to_string(),
        ];
        oauth_betas.extend(betas.iter().map(|b| b.to_string()));
        headers.insert("anthropic-beta".to_string(), Some(oauth_betas.join(",")));
        headers.insert(
            "user-agent".to_string(),
            Some(format!("claude-cli/{CLAUDE_CODE_VERSION}")),
        );
        headers.insert("x-app".to_string(), Some("cli".to_string()));
    } else {
        // API key auth (Pi anthropic-messages.ts:877-896).
        if let Some(key) = &auth.auth.api_key {
            headers.insert("x-api-key".to_string(), Some(key.clone()));
        }
        if !betas.is_empty() {
            headers.insert("anthropic-beta".to_string(), Some(betas.join(",")));
        }
        // Session-affinity header when caching is enabled and the compat flag is set.
        let cache =
            resolve_cache_retention(opts.cache_retention, EnvSource::new(auth.env.as_ref()));
        if cache != CacheRetention::None
            && get_anthropic_compat(model).send_session_affinity_headers
            && let Some(sid) = &opts.session_id
        {
            headers.insert(
                "x-session-affinity".to_string(),
                Some(sid.as_str().to_string()),
            );
        }
    }

    // Auth overlay < model.headers < opts.headers (a `None` suppresses a default).
    if let Some(overlay) = &auth.auth.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    if let Some(overlay) = &model.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    // PROV-028: the per-request Copilot headers, in Pi's merge slot — `mergeHeaders(defaults,
    // model.headers, dynamicHeaders, optionsHeaders)` (anthropic-messages.ts:875-884; `model.headers`
    // at `:881`, `dynamicHeaders` at `:882`), computed at `:525-531`. No-op for every other provider.
    crate::api::github_copilot_headers::apply_copilot_dynamic_headers(
        &mut headers,
        model.provider.as_str(),
        &ctx.messages,
    );
    if let Some(overlay) = &opts.headers {
        for (name, value) in overlay {
            headers.insert(name.clone(), value.clone());
        }
    }
    headers
}
