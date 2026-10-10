//! Tests for the `model` module.
//!
//! Upstream: `packages/coding-agent/src/extensions/llama/provider.ts` @v0.99.2-17 — `modelIsSelectable`
//! (`:37-43`), `configuredContextWindow` (`:58-67`), `contextWindowOf` (`:69-77`),
//! `toPiClassifierModel` (`:79-96`) and `toPiModel` (`:98-130`). Each test names the upstream line it
//! pins; a model built by hand is parsed from JSON exactly as the router sends it.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde_json::{Value, json};

use cyrup_provider::Modality;

use crate::client::{LlamaModelInfo, LlamaServerProps};
use crate::model::{
    DEFAULT_CONTEXT_WINDOW, configured_context_window, context_window_of, model_is_selectable,
    to_classifier_model, to_model,
};

/// A catalog entry as `GET /models` sends it.
fn info(value: Value) -> LlamaModelInfo {
    serde_json::from_value(value).unwrap()
}

fn with_args(args: &[&str]) -> LlamaModelInfo {
    info(json!({ "id": "m", "status": { "value": "unloaded", "args": args } }))
}

fn props(template: &str) -> LlamaServerProps {
    LlamaServerProps {
        models_autoload: None,
        chat_template: Some(template.to_string()),
    }
}

// ------------------------------------------------------------------------------ selectability --

/// `modelIsSelectable` (`provider.ts:37-43`): loaded and sleeping always, unloaded presets only
/// under router autoload and only when the last load did not fail.
#[test]
fn loaded_and_sleeping_models_are_selectable_with_or_without_autoload() {
    for status in ["loaded", "sleeping"] {
        let model = info(json!({ "id": "m", "status": { "value": status } }));
        assert!(
            model_is_selectable(&model, false),
            "{status} without autoload"
        );
        assert!(model_is_selectable(&model, true), "{status} with autoload");
    }
}

#[test]
fn other_lifecycle_states_are_never_selectable() {
    for status in ["loading", "downloading", "something-new"] {
        let model = info(json!({ "id": "m", "status": { "value": status }, "source": "preset" }));
        assert!(!model_is_selectable(&model, true), "{status}");
        assert!(!model_is_selectable(&model, false), "{status}");
    }
}

#[test]
fn an_unloaded_preset_needs_router_autoload() {
    let model = info(json!({ "id": "m", "status": { "value": "unloaded" }, "source": "preset" }));
    assert!(model_is_selectable(&model, true));
    assert!(!model_is_selectable(&model, false));
}

#[test]
fn a_failed_unloaded_preset_is_not_selectable_even_with_autoload() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "unloaded", "failed": true },
        "source": "preset",
    }));
    assert!(!model_is_selectable(&model, true));
    // `failed: false` is the same as no `failed` at all (`!model.status.failed`).
    let recovered = info(json!({
        "id": "m",
        "status": { "value": "unloaded", "failed": false },
        "source": "preset",
    }));
    assert!(model_is_selectable(&recovered, true));
}

#[test]
fn an_unloaded_model_of_another_source_is_not_selectable() {
    for source in [json!("cache"), json!("models_dir"), Value::Null] {
        let model = info(json!({ "id": "m", "status": { "value": "unloaded" }, "source": source }));
        assert!(!model_is_selectable(&model, true), "{source}");
    }
    let no_source = info(json!({ "id": "m", "status": { "value": "unloaded" } }));
    assert!(!model_is_selectable(&no_source, true));
}

// --------------------------------------------------------------------------- configured context --

/// `configuredContextWindow` (`provider.ts:58-67`): `--ctx-size`, `-c` and `-ctx`.
#[test]
fn the_context_size_flag_spellings_are_all_read() {
    for flag in ["--ctx-size", "-c", "-ctx"] {
        let model = with_args(&["llama-server", flag, "32768"]);
        assert_eq!(configured_context_window(&model), Some(32768), "{flag}");
    }
}

#[test]
fn other_flags_do_not_pin_a_context_window() {
    assert_eq!(
        configured_context_window(&with_args(&["llama-server", "--n-gpu-layers", "999"])),
        None
    );
    assert_eq!(
        configured_context_window(&with_args(&["llama-server", "--ctx-size=32768"])),
        None,
        "only the separate-value form is read upstream"
    );
    assert_eq!(
        configured_context_window(&info(
            json!({ "id": "m", "status": { "value": "unloaded" } })
        )),
        None
    );
}

/// The scan stops at `args.length - 1` (`provider.ts:60`): a trailing flag has no value.
#[test]
fn a_trailing_flag_without_a_value_is_ignored() {
    assert_eq!(
        configured_context_window(&with_args(&["llama-server", "--ctx-size"])),
        None
    );
}

/// A bad value does not end the scan: `continue` is only taken for a non-matching flag, and a
/// matching flag with a bad number falls through to the next index (`provider.ts:62-64`).
#[test]
fn a_flag_with_an_unusable_value_is_skipped_and_the_scan_goes_on() {
    for bad in ["0", "-5", "abc", "12.5", "", "9007199254740993"] {
        let model = with_args(&["--ctx-size", bad, "-c", "4096"]);
        assert_eq!(
            configured_context_window(&model),
            Some(4096),
            "after {bad:?}"
        );
        assert_eq!(
            configured_context_window(&with_args(&["--ctx-size", bad])),
            None,
            "{bad:?}"
        );
    }
}

/// `Number(...)` accepts what `parseInt` would not: exponent and radix literals.
#[test]
fn the_value_is_read_the_way_number_reads_it() {
    assert_eq!(
        configured_context_window(&with_args(&["-c", " 8192 "])),
        Some(8192)
    );
    assert_eq!(
        configured_context_window(&with_args(&["-c", "1e4"])),
        Some(10_000)
    );
    assert_eq!(
        configured_context_window(&with_args(&["-c", "0x2000"])),
        Some(8192)
    );
    // every radix prefix, in both cases (`Number("0o20000")`, `Number("0b10000000000000")`)
    for literal in [
        "0X2000",
        "0o20000",
        "0O20000",
        "0b10000000000000",
        "0B10000000000000",
    ] {
        assert_eq!(
            configured_context_window(&with_args(&["-c", literal])),
            Some(8192),
            "{literal}"
        );
    }
}

// --------------------------------------------------------------------------- context precedence --

/// `contextWindowOf` (`provider.ts:69-77`): n_ctx, then the command line, then the cached value,
/// then n_ctx_train, then 128000.
#[test]
fn the_runtime_context_window_wins_over_everything() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "loaded", "args": ["--ctx-size", "32768"] },
        "meta": { "n_ctx": 65536, "n_ctx_train": 131072 },
    }));
    assert_eq!(context_window_of(&model, Some(4096)), 65536);
}

#[test]
fn the_command_line_wins_over_the_cached_and_trained_windows() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "unloaded", "args": ["llama-server", "--ctx-size", "32768"] },
        "meta": { "n_ctx_train": 131072 },
    }));
    assert_eq!(context_window_of(&model, Some(4096)), 32768);
}

/// The fix for an unloaded autoload preset overwriting the cached window (CHANGELOG 0.99.0
/// `#10077`/`#10158`, `coding-agent/CHANGELOG.md:127`): no `n_ctx` and no flag, so the cache wins
/// over the trained length.
#[test]
fn the_cached_window_wins_over_the_trained_window() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "unloaded" },
        "meta": { "n_ctx_train": 128000 },
    }));
    assert_eq!(context_window_of(&model, Some(65536)), 65536);
    assert_eq!(context_window_of(&model, None), 128000);
}

#[test]
fn the_trained_window_then_the_default_close_the_chain() {
    let trained = info(json!({
        "id": "m",
        "status": { "value": "loaded" },
        "meta": { "n_ctx_train": 40960 },
    }));
    assert_eq!(context_window_of(&trained, None), 40960);
    let bare = info(json!({ "id": "m", "status": { "value": "loaded" } }));
    assert_eq!(context_window_of(&bare, None), DEFAULT_CONTEXT_WINDOW);
    assert_eq!(DEFAULT_CONTEXT_WINDOW, 128_000);
}

/// A zero is "absent" at every rung (`> 0`, `provider.ts:71`, `:74`, `:76`).
#[test]
fn a_zero_window_at_a_rung_falls_through_to_the_next() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "loaded" },
        "meta": { "n_ctx": 0, "n_ctx_train": 0 },
    }));
    assert_eq!(context_window_of(&model, Some(0)), DEFAULT_CONTEXT_WINDOW);
    let trained = info(json!({
        "id": "m",
        "status": { "value": "loaded" },
        "meta": { "n_ctx": 0, "n_ctx_train": 9000 },
    }));
    assert_eq!(context_window_of(&trained, Some(0)), 9000);
}

// ------------------------------------------------------------------------------------- to_model --

/// `toPiModel` (`provider.ts:98-130`) for a model whose template enables thinking: the serialized
/// shape is compared whole, so a renamed key, a flipped flag or a dropped null all show.
#[test]
fn a_thinking_template_gives_the_qwen_reasoning_model() {
    let model = info(json!({
        "id": "qwen",
        "status": { "value": "loaded" },
        "meta": { "n_ctx": 32768 },
    }));
    let chat = to_model(
        &model,
        "http://localhost:8080",
        Some(&props("{% if enable_thinking %}think{% endif %}")),
        None,
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(&chat).unwrap(),
        json!({
            "id": "qwen",
            "name": "qwen",
            "api": "openai-completions",
            "provider": "llama.cpp",
            "baseUrl": "http://localhost:8080/v1",
            "reasoning": true,
            "input": ["text"],
            "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0 },
            "contextWindow": 32768,
            "maxTokens": 32768,
            "thinkingLevelMap": {
                "off": "off",
                "minimal": null,
                "low": null,
                "medium": "medium",
                "high": null,
                "xhigh": null,
            },
            "compat": {
                "supportsStore": false,
                "supportsDeveloperRole": false,
                "supportsReasoningEffort": false,
                "supportsUsageInStreaming": true,
                "maxTokensField": "max_tokens",
                "thinkingFormat": "qwen-chat-template",
                "supportsStrictMode": false,
            },
        })
    );
}

/// The `#9528` fix (`provider.ts:105`, `coding-agent/CHANGELOG.md:310`): reasoning is the template
/// mentioning `enable_thinking`, nothing else.
#[test]
fn reasoning_is_the_template_mentioning_enable_thinking() {
    let model = info(json!({ "id": "m", "status": { "value": "loaded" } }));
    for (template, expected) in [
        ("{% if enable_thinking %}x{% endif %}", true),
        ("enable_thinking", true),
        ("{{ messages }}", false),
        ("", false),
        ("enable-thinking", false),
    ] {
        let chat = to_model(&model, "http://h:1", Some(&props(template)), None).unwrap();
        assert_eq!(chat.reasoning, expected, "{template:?}");
    }
}

/// Without props (an unloaded or sleeping model: reading them could load or wake it) the model is
/// a plain chat model: no reasoning, no thinking map, no thinking format.
#[test]
fn without_props_a_model_is_plain_chat() {
    let model = info(json!({ "id": "m", "status": { "value": "sleeping" } }));
    let chat = to_model(&model, "http://h:1", None, None).unwrap();
    assert!(!chat.reasoning);
    assert_eq!(chat.thinking_level_map, None);
    let value = serde_json::to_value(&chat).unwrap();
    assert_eq!(value.get("thinkingLevelMap"), None);
    assert_eq!(value["compat"].get("thinkingFormat"), None);
    // Props with no chat template read the same way.
    let empty = to_model(
        &model,
        "http://h:1",
        Some(&LlamaServerProps::default()),
        None,
    )
    .unwrap();
    assert!(!empty.reasoning);
}

#[test]
fn the_compat_flags_describe_a_llama_server() {
    let model = info(json!({ "id": "m", "status": { "value": "loaded" } }));
    let compat = to_model(&model, "http://h:1", None, None)
        .unwrap()
        .compat
        .unwrap();
    assert_eq!(compat.supports_store, Some(false));
    assert_eq!(compat.supports_developer_role, Some(false));
    assert_eq!(compat.supports_reasoning_effort, Some(false));
    assert_eq!(compat.supports_usage_in_streaming, Some(true));
    assert_eq!(compat.supports_strict_mode, Some(false));
    assert_eq!(
        serde_json::to_value(compat.max_tokens_field).unwrap(),
        json!("max_tokens")
    );
}

/// `input: architecture.input_modalities includes "image" ? ["text","image"] : ["text"]`
/// (`provider.ts:116`).
#[test]
fn an_image_modality_adds_image_input() {
    let vision = info(json!({
        "id": "m",
        "status": { "value": "loaded" },
        "architecture": { "input_modalities": ["text", "image"] },
    }));
    assert_eq!(
        to_model(&vision, "http://h:1", None, None).unwrap().input,
        vec![Modality::Text, Modality::Image]
    );
    let text = info(json!({
        "id": "m",
        "status": { "value": "loaded" },
        "architecture": { "input_modalities": ["text"] },
    }));
    assert_eq!(
        to_model(&text, "http://h:1", None, None).unwrap().input,
        vec![Modality::Text]
    );
    let unknown = info(json!({ "id": "m", "status": { "value": "loaded" } }));
    assert_eq!(
        to_model(&unknown, "http://h:1", None, None).unwrap().input,
        vec![Modality::Text]
    );
}

/// `maxTokens: contextWindow` and the cached window reaching the model (`provider.ts:104`,
/// `:119`).
#[test]
fn max_tokens_is_the_context_window_and_the_cache_is_used() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "unloaded" },
        "meta": { "n_ctx_train": 128000 },
    }));
    let chat = to_model(&model, "http://h:1", None, Some(65536)).unwrap();
    assert_eq!(chat.context_window, 65536);
    assert_eq!(chat.max_tokens, 65536);
}

#[test]
fn the_base_url_is_the_normalised_inference_url() {
    let model = info(json!({ "id": "m", "status": { "value": "loaded" } }));
    for given in [
        "http://h:1",
        "http://h:1/",
        "http://h:1/v1",
        "http://h:1/v1/",
    ] {
        assert_eq!(
            to_model(&model, given, None, None).unwrap().base_url,
            "http://h:1/v1",
            "{given}"
        );
    }
    assert_eq!(
        to_model(&model, "https://example.com/prefix/v1", None, None)
            .unwrap()
            .base_url,
        "https://example.com/prefix/v1"
    );
}

#[test]
fn a_server_url_that_is_not_http_is_an_error() {
    let model = info(json!({ "id": "m", "status": { "value": "loaded" } }));
    let error = to_model(&model, "file:///tmp/llama", None, None).unwrap_err();
    assert!(error.to_string().contains("http or https"), "{error}");
}

// ------------------------------------------------------------------------ to_classifier_model --

/// `toPiClassifierModel` (`provider.ts:79-96`): the same model as a classifier on the server root.
#[test]
fn the_classifier_twin_sits_on_the_server_root() {
    let model = info(json!({
        "id": "qwen",
        "status": { "value": "loaded" },
        "architecture": { "input_modalities": ["text", "image"] },
        "meta": { "n_ctx": 32768 },
    }));
    let twin = to_classifier_model(&model, "http://localhost:8080", None).unwrap();
    assert_eq!(
        serde_json::to_value(&twin).unwrap(),
        json!({
            "type": "classifier",
            "id": "qwen",
            "name": "qwen",
            "api": "llama-cpp-classify",
            "provider": "llama.cpp",
            "baseUrl": "http://localhost:8080",
            "input": ["text"],
            "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0 },
            "contextWindow": 32768,
        }),
        "no /v1, text-only input even for a vision model"
    );
}

/// EXT-110 (`toPiClassifierModel`, `provider.ts:102-117` @f1b2e77f5): a decision-only model's
/// twin answers natively through `typesafe-system-one` on `<server>/v1`, the base the api resolves
/// `systemone` against; a model reporting `text` beside `decisions` is a decision model too.
#[test]
fn a_decision_model_twin_is_typesafe_system_one_on_the_v1_base() {
    for modalities in [json!(["decisions"]), json!(["text", "decisions"])] {
        let model = info(json!({
            "id": "kev",
            "status": { "value": "sleeping" },
            "architecture": { "input_modalities": ["text"], "output_modalities": modalities },
            "meta": { "n_ctx": 8192 },
        }));
        let twin = to_classifier_model(&model, "http://localhost:8080/", None).unwrap();
        assert_eq!(
            serde_json::to_value(&twin).unwrap(),
            json!({
                "type": "classifier",
                "id": "kev",
                "name": "kev",
                "api": "typesafe-system-one",
                "provider": "llama.cpp",
                "baseUrl": "http://localhost:8080/v1",
                "input": ["text"],
                "cost": { "input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0 },
                "contextWindow": 8192,
            })
        );
    }
    // A decision model's base must be a URL, as a chat model's must (`llamaInferenceUrl`).
    let model = info(json!({
        "id": "kev",
        "status": { "value": "loaded" },
        "architecture": { "output_modalities": ["decisions"] },
    }));
    assert!(to_classifier_model(&model, "not a url", None).is_err());
}

#[test]
fn the_classifier_twin_uses_the_cached_window_like_the_chat_model() {
    let model = info(json!({
        "id": "m",
        "status": { "value": "unloaded" },
        "meta": { "n_ctx_train": 128000 },
    }));
    assert_eq!(
        to_classifier_model(&model, "http://h:1", Some(65536))
            .unwrap()
            .context_window,
        65536
    );
    assert_eq!(
        to_classifier_model(&model, "http://h:1", None)
            .unwrap()
            .context_window,
        128000
    );
}
