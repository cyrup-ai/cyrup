//! The bytes a fake must put on the wire, written out literally — the PIN the drift guards
//! compare against.
//!
//! Each constant is what one definition in [`crate::router`] or [`crate::classify`] serializes to
//! for one fixed input, spelled the way llama.cpp spells it: compact JSON with object keys in
//! INSERTION order (`common_json`, `common/json.h:24` @b11436: "object keys keep the order in which
//! they are added"; `safe_json_to_str` dumps compact), which is also what this workspace's
//! `serde_json` writes, because it is built with `preserve_order` (root `Cargo.toml`).
//!
//! Every fake that answers from this crate has a drift guard that drives the fake over a real
//! socket and compares what came back with these bytes. Changing a definition therefore fails
//! each fake's guard, and this crate's own `definitions_serialize_to_their_golden_bytes`, until the
//! golden here is changed too — on purpose, with a re-derived citation.

/// [`crate::router::file_not_found`]: the httplib error handler's literal
/// (`server-http.cpp:199-212` @b11436).
pub const FILE_NOT_FOUND: &str =
    r#"{"error":{"message":"File Not Found","type":"not_found_error","code":404}}"#;

/// [`crate::router::invalid_api_key`]: the API-key middleware's literal
/// (`server-http.cpp:289-300` @b11436).
pub const INVALID_API_KEY: &str =
    r#"{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}"#;

/// [`crate::router::invalid_request`]`("model is not running")`: `res_err(format_error_response(
/// ..., ERROR_TYPE_INVALID_REQUEST))` (`server-models.cpp:2155`, `server-common.cpp:36-77`).
pub const MODEL_IS_NOT_RUNNING: &str =
    r#"{"error":{"code":400,"message":"model is not running","type":"invalid_request_error"}}"#;

/// [`crate::router::success`] (`server-models.cpp:2078`, `:2159`, `:2225`, `:2239`).
pub const SUCCESS: &str = r#"{"success":true}"#;

/// [`crate::router::models_envelope`] over an empty catalog (`server-models.cpp:2138-2141`).
pub const EMPTY_MODELS: &str = r#"{"data":[],"object":"list"}"#;

/// [`crate::router::models_envelope`] over [`crate::router::decision_entry`]`("kev", "loaded")`
/// and `("laya", "unloaded")` (EXT-110): the decision-only `output_modalities` (`server-common.cpp:
/// 150-163`), `meta` merged last for the running entry only (`server-models.cpp:2128-2134`).
pub const MODELS_DECISION_KEV_LOADED_LAYA_UNLOADED: &str = concat!(
    r#"{"data":[{"id":"kev","aliases":[],"tags":[],"object":"model","owned_by":"llamacpp","#,
    r#""created":1759000000,"status":{"value":"loaded","args":[]},"#,
    r#""architecture":{"input_modalities":["text"],"output_modalities":["decisions"]},"#,
    r#""source":"models_dir","can_remove":false,"meta":{"vocab_type":2,"n_vocab":151936,"#,
    r#""n_ctx":8192,"n_ctx_train":32768,"n_embd":1024,"n_params":600000000,"size":400000000,"#,
    r#""ftype":"Q8_0"}},{"id":"laya","aliases":[],"tags":[],"object":"model","#,
    r#""owned_by":"llamacpp","created":1759000000,"status":{"value":"unloaded","args":[]},"#,
    r#""architecture":{"input_modalities":["text"],"output_modalities":["decisions"]},"#,
    r#""source":"models_dir","can_remove":false}],"object":"list"}"#,
);

/// [`crate::router::router_props`]`(true)` (`server-models.cpp:1992-2011`).
pub const ROUTER_PROPS_AUTOLOAD: &str = concat!(
    r#"{"role":"router","max_instances":1,"models_autoload":true,"model_alias":"llama-server","#,
    r#""model_path":"none","default_generation_settings":{"params":{},"n_ctx":0},"#,
    r#""ui_settings":{},"build_info":"b11436","cors_proxy_enabled":false}"#,
);

/// [`crate::router::sse_frame`] over [`crate::router::sse_event`]`("*", "models_reload", None)`
/// (`server-models.cpp:2175`, `:686-697`, `:1026`).
pub const SSE_MODELS_RELOAD_FRAME: &str = "data: {\"model\":\"*\",\"event\":\"models_reload\"}\n\n";

/// [`crate::classify::tokenize`]`(&[65, 66])` (`server-context.cpp:5473-5477`).
pub const TOKENIZE_65_66: &str = r#"{"tokens":[65,66]}"#;

/// [`crate::classify::apply_template`] over a prompt holding two newlines
/// (`server-context.cpp:5424`).
pub const APPLY_TEMPLATE_USER_HI: &str = r#"{"prompt":"<|user|>\nhi\n<|assistant|>\n"}"#;

/// [`crate::classify::completion_probabilities`] for a prediction of `B` (id 66, logprob -0.3)
/// ranked above `A` (id 65, logprob -1.5) (`server-task.cpp:264-301`).
pub const COMPLETION_PROBABILITIES_B_OVER_A: &str = concat!(
    r#"[{"id":66,"token":"B","bytes":[66],"logprob":-0.3,"top_logprobs":["#,
    r#"{"id":66,"token":"B","bytes":[66],"logprob":-0.3},"#,
    r#"{"id":65,"token":"A","bytes":[65],"logprob":-1.5}]}]"#,
);

/// The top-level keys of [`crate::classify::completion`], in llama.cpp's order
/// (`to_json_non_oaicompat`, `server-task.cpp:340-361`).
pub const COMPLETION_KEYS: [&str; 17] = [
    "index",
    "content",
    "tokens",
    "id_slot",
    "stop",
    "model",
    "tokens_predicted",
    "tokens_evaluated",
    "generation_settings",
    "prompt",
    "has_new_line",
    "truncated",
    "stop_type",
    "stopping_word",
    "tokens_cached",
    "timings",
    "completion_probabilities",
];
