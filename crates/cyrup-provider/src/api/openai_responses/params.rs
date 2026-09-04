//! Request encoding: the Responses request body (Pi `buildParams`,
//! openai-responses.ts:231-279).

use super::convert::convert_responses_messages;
use super::options::{reasoning_summary_or_auto, reasoning_summary_wire};
use super::tools::{ConvertResponsesToolsOptions, convert_responses_tools};
use crate::api::compat::{
    clamp_openai_prompt_cache_key, get_responses_compat, mapped_effort_or, off_is_not_null,
    off_value_or, thinking_level_key,
};
use crate::auth::ProviderEnv;
use crate::collection::clamp_thinking_level;
use crate::context::Context;
use crate::model::Model;
use crate::stream::{CacheRetention, StreamOptions};
use crate::utils::constrained_sampling::ConstrainedSamplingError;
use crate::utils::deferred_tools::split_deferred_tools;
use crate::utils::provider_plumbing::resolve_cache_retention;
use cyrup_core::ModelThinkingLevel;
use serde_json::{Map, Value, json};

/// Providers whose tool-call ids carry the `call_id|item_id` Responses shape (Pi
/// `OPENAI_TOOL_CALL_PROVIDERS`, openai-responses.ts:26).
const OPENAI_TOOL_CALL_PROVIDERS: &[&str] = &["openai", "openai-codex", "opencode"];

/// OpenAI Responses rejects `max_output_tokens` below 16:
/// <https://github.com/earendil-works/pi/issues/6265>
/// (Pi `OPENAI_RESPONSES_MIN_OUTPUT_TOKENS`, `openai-responses.ts:32` @v0.83.0.)
pub(crate) const OPENAI_RESPONSES_MIN_OUTPUT_TOKENS: u64 = 16;

/// Test-only infallible view of [`try_build_params`]: every in-file fixture declares tools whose
/// `constrainedSampling` is absent, so the resolver cannot fail.
#[cfg(test)]
// Test-only fixture wrapper: the deny-list allowance the crate's `mod tests` blocks carry.
#[allow(clippy::expect_used)]
pub(crate) fn build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
) -> Value {
    try_build_params(model, ctx, opts, env)
        .expect("fixture declares no unsatisfiable constrained sampling")
}

/// Build the Responses request body (1:1 port of Pi `buildParams`, openai-responses.ts:231-279).
///
/// `[CYRUP-DELTA]` — fallible where pi's `buildParams` throws: `convertResponsesTools` rejects a
/// `strict: "require"` tool on a route without strict mode (`constrained-sampling.ts:91-95`
/// @v0.83.0). PROV-011.
pub(super) fn try_build_params(
    model: &Model,
    ctx: &Context,
    opts: &StreamOptions,
    env: Option<&ProviderEnv>,
) -> Result<Value, ConstrainedSamplingError> {
    let compat = get_responses_compat(model);

    // --- DRIFT-001 deferred-tool placement (Pi openai-responses.ts:267-274) ---
    //
    // The Responses rendering is the MIRROR IMAGE of the Anthropic one: there, deferred tools stay
    // in `params.tools` carrying `defer_loading: true`; here they are omitted from `tools`
    // ENTIRELY and reach the model only inside the synthetic `tool_search_output` anchored at
    // their marker. Sending them in both places would re-inflate the cache-unstable prefix this
    // feature exists to avoid; sending them in neither would hand the model tool calls it has no
    // schema for.
    //
    // Two deliberate divergences from the Anthropic caller, both verified against Pi:
    //  * the split runs over the RAW `ctx.messages` — Pi passes `context`, not a transformed list,
    //    because `convertResponsesMessages` does its own `transformMessages` internally (:267);
    //  * there is NO safety valve. Pi's "promote everything back when the prefix would be empty"
    //    rule is Anthropic-only (anthropic-messages.ts:955-959); openai-responses.ts:301 guards on
    //    `immediate.length > 0`, so an all-deferred request ships a body with no `tools` key at
    //    all and the definitions live purely in the transcript.
    // The name normalizer is identity: Pi calls `splitDeferredTools(context, enabled)` with the
    // default `identityToolName`, so the deferred map is keyed by the raw tool name and
    // `deferredTools.get(name)` at the anchor site matches on the raw `addedToolNames` entry.
    let placement = split_deferred_tools(
        &ctx.messages,
        &ctx.tools,
        compat.supports_tool_search,
        &|name: &str| name.to_string(),
    );

    let messages = convert_responses_messages(
        model,
        ctx,
        OPENAI_TOOL_CALL_PROVIDERS,
        &placement.deferred,
        ConvertResponsesToolsOptions {
            defer_loading: false,
            supports_strict_mode: compat.supports_strict_mode,
            default_strict: Some(false),
        },
    )?;
    let cache = resolve_cache_retention(opts.cache_retention, env);

    let mut obj = Map::new();
    obj.insert("model".to_string(), json!(model.id.as_str()));
    obj.insert("input".to_string(), Value::Array(messages));
    obj.insert("stream".to_string(), json!(true));

    // prompt_cache_key: omitted when retention is none or no session id is available.
    if cache != CacheRetention::None
        && let Some(sid) = &opts.session_id
    {
        obj.insert(
            "prompt_cache_key".to_string(),
            json!(clamp_openai_prompt_cache_key(sid.as_str())),
        );
    }
    // prompt_cache_retention: "24h" only for long retention on a long-cache-capable model.
    if cache == CacheRetention::Long && compat.supports_long_cache_retention {
        obj.insert("prompt_cache_retention".to_string(), json!("24h"));
    }
    // PROV-023. `const disableImplicitPromptCache = cacheRetention === "none" &&
    // compat.supportsExplicitPromptCacheMode` (openai-responses.ts:278 @v0.83.0), emitted at `:285`
    // in pi's literal — between `prompt_cache_retention` and `store`. Without it the endpoint
    // implicitly cache-WRITES one-shot prompts (compaction/branch summaries run with
    // `cacheRetention: "none"`) and bills the cache-write premium for a prefix nothing will re-read.
    if cache == CacheRetention::None && compat.supports_explicit_prompt_cache_mode {
        obj.insert(
            "prompt_cache_options".to_string(),
            json!({ "mode": "explicit" }),
        );
    }
    obj.insert("store".to_string(), json!(false));

    // PROV-019. `if (options?.maxTokens) params.max_output_tokens = Math.max(options.maxTokens,
    // OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)` (openai-responses.ts:289-290 @v0.83.0). The `.filter`
    // reproduces pi's JS truthiness gate, so `Some(0)` omits the key rather than sending `0`.
    if let Some(max) = opts.max_tokens.filter(|m| *m > 0) {
        obj.insert(
            "max_output_tokens".to_string(),
            json!(max.max(OPENAI_RESPONSES_MIN_OUTPUT_TOKENS)),
        );
    }
    // PERM-012. pi strips `temperature` for a `codex`-token id and for any reasoning model on this
    // api — `getUnsupportedTemperatureReason` (`pi-permission-system/src/
    // model-option-compatibility.ts:74-80` @v0.8.0), applied through a provider wrapper because a
    // JS extension cannot reach this builder. cyrup can, so the key simply is not written; see
    // [`crate::api::compat::unsupported_temperature_reason`] for the CYRUP-DELTA on the seam.
    if let Some(temp) = opts
        .temperature
        .filter(|_| crate::api::compat::temperature_is_supported(model))
    {
        obj.insert("temperature".to_string(), json!(temp));
    }

    // service_tier (Pi `if (options?.serviceTier !== undefined) params.service_tier = …`,
    // openai-responses.ts:242-244). Omitted when unset (Pi default).
    if let Some(tier) = opts
        .openai_responses_options()
        .and_then(|o| o.service_tier.as_deref())
    {
        obj.insert("service_tier".to_string(), json!(tier));
    }

    // Only the IMMEDIATE tools reach `body.tools` (Pi `if (toolPlacement.immediate.length > 0)`,
    // openai-responses.ts:301-306). With tool search off the split is a pass-through, so this is
    // byte-identical to the old `if !ctx.tools.is_empty()` for every model that does not opt in.
    if !placement.immediate.is_empty() {
        obj.insert(
            "tools".to_string(),
            Value::Array(convert_responses_tools(
                &placement.immediate,
                ConvertResponsesToolsOptions {
                    defer_loading: false,
                    supports_strict_mode: compat.supports_strict_mode,
                    default_strict: Some(false),
                },
            )?),
        );
    }

    if model.reasoning {
        // The unified `reasoning` level maps to Pi's `reasoningEffort` (clamped; `off` => none).
        let clamped = clamp_thinking_level(model, opts.reasoning);
        // PROV-045(a). Pi's first arm fires on `options?.reasoningEffort || options?.reasoningSummary`
        // (openai-responses.ts:313 @v0.83.0), so a caller setting ONLY a summary still gets
        // `reasoning: {effort: "medium", summary}` plus `include`. cyrup represents "no effort" as
        // `ModelThinkingLevel::Off`, so the effort half of pi's disjunction is `clamped != Off`.
        let summary_only = reasoning_summary_wire(opts.openai_responses_options()).is_some();
        if clamped != ModelThinkingLevel::Off || summary_only {
            // `const effort = options?.reasoningEffort ? (map?.[effort] ?? effort) : "medium"`
            // (openai-responses.ts:314-316).
            let effort = if clamped == ModelThinkingLevel::Off {
                "medium".to_string()
            } else {
                let key = thinking_level_key(clamped);
                mapped_effort_or(model.thinking_level_map.as_ref(), clamped, key)
            };
            // Pi `summary: options?.reasoningSummary || "auto"` (openai-responses.ts:318).
            let summary = reasoning_summary_or_auto(opts.openai_responses_options());
            obj.insert(
                "reasoning".to_string(),
                json!({ "effort": effort, "summary": summary }),
            );
            obj.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        } else if model.provider.as_str() != "github-copilot"
            && off_is_not_null(model.thinking_level_map.as_ref())
        {
            let effort = off_value_or(model.thinking_level_map.as_ref(), "none");
            obj.insert("reasoning".to_string(), json!({ "effort": effort }));
        }
        // PROV-045(b). `if (model.provider === "xai") params.include = [...]`
        // (openai-responses.ts:327) sits OUTSIDE the if/else, so an xAI reasoning model gets
        // `include` on the off path too — otherwise the next turn cannot replay its encrypted
        // reasoning content.
        if model.provider.as_str() == "xai" {
            obj.insert(
                "include".to_string(),
                json!(["reasoning.encrypted_content"]),
            );
        }
    }

    // Last so custom keys override the named request fields (Pi's own comment,
    // `openai-responses.ts:330-333` @v0.84.1). AGENT-026 — the merge with `Model.sampling_params`
    // happened in `build_base_options` (`simple-options.ts:27-33`); this is the assign.
    crate::api::openai_completions::apply_sampling_params(&mut obj, opts);

    Ok(Value::Object(obj))
}
