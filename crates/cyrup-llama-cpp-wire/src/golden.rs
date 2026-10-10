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

/// [`crate::router::router_props`]`(true)` (`server-models.cpp:1992-2011`): `params` is `null`,
/// as a live b11436 router sends it.
pub const ROUTER_PROPS_AUTOLOAD: &str = concat!(
    r#"{"role":"router","max_instances":1,"models_autoload":true,"model_alias":"llama-server","#,
    r#""model_path":"none","default_generation_settings":{"params":null,"n_ctx":0},"#,
    r#""ui_settings":{},"build_info":"b11436","cors_proxy_enabled":false}"#,
);

/// [`crate::router::child_props`]`("{{ messages }}")`: the CHILD's own props, which the router
/// proxies back for `GET /props?model=<id>` (`get_res_props`, `server-context.cpp:4955-4998`).
pub const CHILD_PROPS_PLAIN: &str = concat!(
    r#"{"default_generation_settings":{"params":{},"n_ctx":32768},"total_slots":1,"#,
    r#""model_alias":"qwen","model_ftype":"Q4_K_M","model_path":"/models/qwen.gguf","#,
    r#""modalities":{"vision":false,"video":false,"audio":false},"media_marker":"<__media__>","#,
    r#""endpoint_slots":false,"endpoint_props":false,"endpoint_metrics":false,"ui":true,"#,
    r#""ui_settings":{},"chat_template":"{{ messages }}","chat_template_caps":{},"#,
    r#""bos_token":"<|im_start|>","eos_token":"<|im_end|>","build_info":"b11436","#,
    r#""is_sleeping":false,"cors_proxy_enabled":false}"#,
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

/// [`crate::classify::completion`]`("qwen", "p", 1, B, &[B, A])` with `B` and `A` as in
/// [`COMPLETION_PROBABILITIES_B_OVER_A`]: EVERY byte of the `/completion` answer, so a change to any
/// value it serves (`stop`, `stop_type`, `has_new_line`, `tokens_cached`, ...) fails, not only a
/// change of key order (`to_json_non_oaicompat`, `server-task.cpp:340-363` @b11436).
pub const COMPLETION_QWEN_P_B_OVER_A: &str = concat!(
    r#"{"index":0,"content":"B","tokens":[],"id_slot":0,"stop":true,"model":"qwen","#,
    r#""tokens_predicted":1,"tokens_evaluated":1,"generation_settings":{},"prompt":"p","#,
    r#""has_new_line":false,"truncated":false,"stop_type":"limit","stopping_word":"","#,
    r#""tokens_cached":1,"timings":{},"completion_probabilities":"#,
    r#"[{"id":66,"token":"B","bytes":[66],"logprob":-0.3,"top_logprobs":["#,
    r#"{"id":66,"token":"B","bytes":[66],"logprob":-0.3},"#,
    r#"{"id":65,"token":"A","bytes":[65],"logprob":-1.5}]}]}"#,
);

/// [`crate::classify::tokenize_with_pieces`]`(&[(65, "A")])`, the `with_pieces: true` form
/// (`server-context.cpp:5451-5470` @b11436; a live b11436 child answered
/// `{"tokens":[{"id":447,"piece":"A"},...]}` in this key order).
pub const TOKENIZE_WITH_PIECES_65_A: &str = r#"{"tokens":[{"id":65,"piece":"A"}]}"#;

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

/// The answer a LIVE b11436 router (`llama-server --models-dir`, `-t 1`) gave on 2026-10-10 to
/// `POST /v1/systemone` with `model: "tinylaya-for-testing-Q8_0"` (a decision-only GGUF,
/// `output_modalities: ["decisions"]`), the state `{"text": "The deployment succeeded, thank
/// you."}` and pi's own three test questions (`test/typesafe-system-one.test.ts` @f1b2e77f5): a
/// choice `category` over `success`/`failure`, a score `satisfaction` over three levels and a
/// noul `approved` (pi's `bool`, sent as `noul`). Captured verbatim, BYTE FOR BYTE: what
/// [`crate::systemone::response`] over [`crate::systemone::choice_answer`],
/// [`crate::systemone::score_answer`] and [`crate::systemone::noul_answer`] with these values
/// must serialize to (`post_systemone`, `server-context.cpp:5583-5667`; `format_answer`,
/// `server-decision.cpp:731-804`). The test model is a random-weight test fixture, so the
/// probabilities are near-uniform; the numbers are the server's, not chosen.
pub const SYSTEMONE_TINYLAYA_LIVE: &str = concat!(
    r#"{"model":"tinylaya-for-testing-Q8_0","answers":{"#,
    r#""category":{"type":"choice","choice":"failure","probabilities":"#,
    r#"{"success":0.4996767927733331,"failure":0.5003232072266669},"#,
    r#""confidence":0.000646414453333799},"#,
    r#""satisfaction":{"type":"score","score":1.0014472175540505,"#,
    r#""legend":{"0":"low","1":"neutral","2":"high"},"#,
    r#""probabilities":{"0":0.3335314403923897,"1":0.3314899016611701,"2":0.33497865794644016},"#,
    r#""confidence":0.0},"#,
    r#""approved":{"type":"noul","noul":0.500289248081911}},"#,
    r#""usage":{"input_tokens":103,"output_tokens":0}}"#,
);

/// [`crate::systemone::not_a_decision_model`]: what the live b11436 router answered (HTTP 501) to
/// the same request with `model: "stories260K"`, a text model (`server-context.cpp:5586-5588`).
pub const SYSTEMONE_NOT_A_DECISION_MODEL: &str = r#"{"error":{"code":501,"message":"This model is not a decision model","type":"not_supported_error"}}"#;
