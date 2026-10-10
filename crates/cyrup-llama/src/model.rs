//! router catalog entry → cyrup `Model` mapping (`provider.ts` `toPiModel`)
//!
//! Port of the pure half of `packages/coding-agent/src/extensions/llama/provider.ts` @v0.99.2-17
//! (EXT-027): which router catalog entries are selectable (`modelIsSelectable`, `:37-43`), where a
//! model's context window comes from (`configuredContextWindow` / `contextWindowOf`, `:58-77`) and
//! how one catalog entry becomes a chat [`Model`] (`toPiModel`, `:98-130`) and its classifier twin
//! (`toPiClassifierModel`, `:79-96`). Nothing here does I/O; [`crate::provider`] drives it.

use std::collections::BTreeMap;

use cyrup_provider::api::compat::{MaxTokensField, ModelCompat, ThinkingFormat};
use cyrup_provider::known_api::OPENAI_COMPLETIONS;
use cyrup_provider::{ClassifierModel, KnownClassifierApi, Modality, Model, ModelCost};

use crate::LLAMA_PROVIDER_ID;
use crate::client::{LlamaModelInfo, LlamaModelStatus, LlamaServerProps, llama_inference_url};
use crate::error::LlamaError;

/// The context window of a model that reports none (`: 128000`, `provider.ts:76`).
pub const DEFAULT_CONTEXT_WINDOW: u64 = 128_000;

/// Whether a catalog entry can be offered as a model (`modelIsSelectable`, `provider.ts:37-43`).
///
/// A `loaded` model always is, and so is a `sleeping` one: llama.cpp reports idle-slept models as
/// `sleeping` and a request wakes them (`:39-40`). An `unloaded` model is routable only when the
/// router's autoload can load it on first use, so it needs `router_autoload`, no failed last load
/// and a `preset` source (`:41-42`).
#[must_use]
pub fn model_is_selectable(model: &LlamaModelInfo, router_autoload: bool) -> bool {
    match model.status.value {
        LlamaModelStatus::Loaded | LlamaModelStatus::Sleeping => true,
        LlamaModelStatus::Unloaded => {
            router_autoload
                && model.status.failed != Some(true)
                && model.source.as_deref() == Some("preset")
        }
        _ => false,
    }
}

/// Whether llama.cpp reports a native decision model (`isDecisionModel`, `provider.ts:88-90`
/// @f1b2e77f5): `architecture.output_modalities` includes `"decisions"`. llama.cpp writes
/// `["decisions"]` for a GGUF whose decision type is set and `["text"]` otherwise
/// (`server_model_output_modalities`, `tools/server/server-common.cpp:150-163` @b11436), and the
/// router reads it from the GGUF metadata even for a model that is not running
/// (`server-models.cpp:567-571`), so unloaded and sleeping entries carry it too. An older server
/// sends `["text"]` or no `architecture`, and its models are chat models.
#[must_use]
pub fn is_decision_model(model: &LlamaModelInfo) -> bool {
    output_modalities_include(model, "decisions")
}

/// Whether a catalog entry is offered as a chat model (`isChatModel`, `provider.ts:93-95`
/// @f1b2e77f5): a decision-ONLY model cannot generate text, so it is a classifier and nothing else.
#[must_use]
pub fn is_chat_model(model: &LlamaModelInfo) -> bool {
    !is_decision_model(model) || output_modalities_include(model, "text")
}

fn output_modalities_include(model: &LlamaModelInfo, modality: &str) -> bool {
    model
        .architecture
        .as_ref()
        .and_then(|architecture| architecture.output_modalities.as_ref())
        .is_some_and(|modalities| modalities.iter().any(|entry| entry == modality))
}

/// `Number(text)` for the strings `configuredContextWindow` feeds it: surrounding whitespace is
/// ignored, an empty string is `0`, `0x`/`0o`/`0b` read as radix literals and everything else as a
/// decimal float. `NaN` stands for "not a number".
fn js_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    let radix = [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ]
    .into_iter()
    .find_map(|(prefix, radix)| text.strip_prefix(prefix).map(|digits| (digits, radix)));
    match radix {
        Some((digits, radix)) => u64::from_str_radix(digits, radix)
            .ok()
            .map_or(f64::NAN, u64_to_f64),
        None => text.parse::<f64>().unwrap_or(f64::NAN),
    }
}

#[allow(clippy::cast_precision_loss)]
fn u64_to_f64(value: u64) -> f64 {
    value as f64
}

/// The context window a launch command line pins (`configuredContextWindow`, `provider.ts:58-67`):
/// the first `--ctx-size`, `-c` or `-ctx` flag in `status.args` that is followed by a positive safe
/// integer. A flag with a bad value is skipped and the scan goes on (`:62-64`); the last argument
/// is never a flag with a value (`index < args.length - 1`, `:60`).
#[must_use]
pub fn configured_context_window(model: &LlamaModelInfo) -> Option<u64> {
    const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
    let args = model.status.args.as_deref().unwrap_or_default();
    args.windows(2).find_map(|pair| {
        let [flag, value] = pair else { return None };
        if !matches!(flag.as_str(), "--ctx-size" | "-c" | "-ctx") {
            return None;
        }
        let number = js_number(value);
        let safe_positive = number.is_finite()
            && number.fract() == 0.0
            && number > 0.0
            && number <= MAX_SAFE_INTEGER;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        safe_positive.then_some(number as u64)
    })
}

/// A model's context window (`contextWindowOf`, `provider.ts:69-77`), by precedence:
///
/// 1. `meta.n_ctx`, what the running instance reports (`:70-71`);
/// 2. the `--ctx-size` / `-c` / `-ctx` argument of its command line (`:72-73`);
/// 3. `cached`, the value stored by an earlier refresh (`:74`);
/// 4. `meta.n_ctx_train`, the trained length (`:75-76`);
/// 5. [`DEFAULT_CONTEXT_WINDOW`] (`:76`).
///
/// The cached value ranks below the command line and above the trained length, which is what keeps
/// an `unloaded` autoload preset (it reports no `n_ctx`) at the window it last ran with instead of
/// falling back to `n_ctx_train` (CHANGELOG 0.99.0 `#10077`/`#10158`, `coding-agent/CHANGELOG.md:127`).
#[must_use]
pub fn context_window_of(model: &LlamaModelInfo, cached: Option<u64>) -> u64 {
    let meta = model.meta.as_ref();
    if let Some(runtime) = meta.and_then(|meta| meta.n_ctx).filter(|value| *value > 0) {
        return runtime;
    }
    if let Some(configured) = configured_context_window(model) {
        return configured;
    }
    if let Some(cached) = cached.filter(|value| *value > 0) {
        return cached;
    }
    meta.and_then(|meta| meta.n_ctx_train)
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW)
}

/// The same llama.cpp model used as a classifier (`toPiClassifierModel`, `provider.ts:102-117`
/// @f1b2e77f5, EXT-110).
///
/// A DECISION model ([`is_decision_model`]) answers natively through llama.cpp's System One
/// endpoint: its api is `typesafe-system-one` and its base URL is `<server>/v1`
/// (`baseUrl: decision ? llamaInferenceUrl(serverUrl) : serverUrl`, `:114`), so the api posts to
/// `<server>/v1/systemone` (`tools/server/server.cpp:289` @b11436). Every other model falls back
/// to `llama-cpp-classify`, which reads answers from next-token label probabilities through
/// `/tokenize`, `/apply-template` and `/completion` on the server ROOT, with no `/v1` (`:112`,
/// `:114`).
///
/// # Errors
///
/// The URL errors of [`llama_inference_url`] when a decision model's `server_url` is not an
/// http(s) URL, as [`to_model`] has.
pub fn to_classifier_model(
    model: &LlamaModelInfo,
    server_url: &str,
    cached_context_window: Option<u64>,
) -> Result<ClassifierModel, LlamaError> {
    let decision = is_decision_model(model);
    let (api, base_url) = if decision {
        (
            KnownClassifierApi::TypesafeSystemOne,
            llama_inference_url(server_url)?,
        )
    } else {
        (KnownClassifierApi::LlamaCppClassify, server_url.to_string())
    };
    Ok(ClassifierModel {
        id: model.id.as_str().into(),
        name: model.id.clone(),
        api: api.into(),
        provider: LLAMA_PROVIDER_ID.into(),
        base_url,
        input: vec![Modality::Text],
        input_limits: None,
        cost: ModelCost::default(),
        headers: None,
        context_window: context_window_of(model, cached_context_window),
    })
}

/// Whether a stored classifier model is one of this provider's (`isLlamaClassifierModel`,
/// `provider.ts:121-124` @f1b2e77f5): a classifier on either api [`to_classifier_model`] produces,
/// `llama-cpp-classify` or `typesafe-system-one`.
#[must_use]
pub fn is_llama_classifier_api(api: &str) -> bool {
    api == KnownClassifierApi::LlamaCppClassify.as_str()
        || api == KnownClassifierApi::TypesafeSystemOne.as_str()
}

/// A router catalog entry as an `openai-completions` chat [`Model`] (`toPiModel`,
/// `provider.ts:98-130`).
///
/// `props` is the model's own `GET /props` answer when it was read: `reasoning` is on exactly when
/// its `chat_template` mentions `enable_thinking` (`:105`, the `#9528` fix,
/// `coding-agent/CHANGELOG.md:310`), and a reasoning model carries the Qwen thinking map
/// (`off`/`medium` supported, the rest `null`, `:113-115`) and the `qwen-chat-template` thinking
/// format (`:127`). Without props the model is a plain chat model.
///
/// # Errors
///
/// The URL errors of [`llama_inference_url`] when `server_url` is not an http(s) URL.
pub fn to_model(
    model: &LlamaModelInfo,
    server_url: &str,
    props: Option<&LlamaServerProps>,
    cached_context_window: Option<u64>,
) -> Result<Model, LlamaError> {
    let context_window = context_window_of(model, cached_context_window);
    let reasoning = props
        .and_then(|props| props.chat_template.as_deref())
        .is_some_and(|template| template.contains("enable_thinking"));
    let thinking_level_map: BTreeMap<String, Option<String>> = [
        ("off", Some("off")),
        ("minimal", None),
        ("low", None),
        ("medium", Some("medium")),
        ("high", None),
        ("xhigh", None),
    ]
    .into_iter()
    .map(|(level, value)| (level.to_string(), value.map(str::to_string)))
    .collect();
    let sees_images = model
        .architecture
        .as_ref()
        .and_then(|architecture| architecture.input_modalities.as_ref())
        .is_some_and(|modalities| modalities.iter().any(|modality| modality == "image"));
    Ok(Model {
        id: model.id.as_str().into(),
        name: model.id.clone(),
        api: OPENAI_COMPLETIONS.into(),
        provider: LLAMA_PROVIDER_ID.into(),
        base_url: llama_inference_url(server_url)?,
        reasoning,
        input: if sees_images {
            vec![Modality::Text, Modality::Image]
        } else {
            vec![Modality::Text]
        },
        cost: ModelCost::default(),
        context_window,
        max_tokens: context_window,
        sampling_params: None,
        input_limits: None,
        prompt_cache: None,
        thinking_level_map: reasoning.then_some(thinking_level_map),
        compat: Some(ModelCompat {
            supports_store: Some(false),
            supports_developer_role: Some(false),
            supports_reasoning_effort: Some(false),
            supports_usage_in_streaming: Some(true),
            supports_strict_mode: Some(false),
            max_tokens_field: Some(MaxTokensField::MaxTokens),
            thinking_format: reasoning.then_some(ThinkingFormat::QwenChatTemplate),
            ..ModelCompat::default()
        }),
        headers: None,
    })
}
