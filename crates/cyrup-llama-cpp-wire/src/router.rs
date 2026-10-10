//! The ROUTER MANAGEMENT api: `GET /models`, `GET /props`, `POST /models/load`,
//! `POST /models/unload`, `POST /models` and `GET /models/sse` (registered at
//! `tools/server/server.cpp:256-268` @b11436), plus the two error bodies every route can answer
//! with. This is the part of the wire `cyrup_llama::client` speaks.
//!
//! Moved here verbatim from `cyrup-llama/src/tests/llama_cpp_wire.rs` (EXT-100) by EXT-108, every
//! citation re-derived against b11436 on the way; the ones that had drifted are corrected in place.

use serde_json::{Value, json};

// ------------------------------------------------------------------------------------ envelopes --

/// `GET /models` and `GET /models?reload=1` (`server-models.cpp:2138-2141` @b11436,
/// `res_ok(res, {{"data", models_json}, {"object", "list"}})`). `?reload=<anything non-empty>`
/// rescans the model directory first and answers the same shape (`:2083-2086`).
pub fn models_envelope(entries: Vec<Value>) -> Value {
    json!({ "data": entries, "object": "list" })
}

/// The body of every router mutation that succeeds: `POST /models/load`
/// (`server-models.cpp:2078`), `POST /models/unload` (`:2159`), `POST /models`, the Hugging Face
/// download (`:2225`) and `DELETE /models` (`:2239`) all answer exactly
/// `res_ok(res, {{"success", true}})` with status 200.
pub fn success() -> Value {
    json!({ "success": true })
}

/// `format_error_response` + `res_err` (`server-common.cpp:36-78`, `server-models.cpp:1915-1918`
/// @b11436): the body is `{"error": {"code", "message", "type"}}` and the HTTP status IS that
/// `code`. `cyrup_llama::client::LlamaClient` reads `error.message` and ignores the rest
/// (`client.rs:183` upstream).
pub fn error_body(code: u16, message: &str, kind: &str) -> Value {
    json!({ "error": { "code": code, "message": message, "type": kind } })
}

/// `ERROR_TYPE_INVALID_REQUEST` → `400 invalid_request_error` (`server-common.cpp:40-43`). The
/// router answers this for an unknown model on `/models/unload` (`server-models.cpp:2151`), for a
/// model that is not running (`:2155`) and for `/props?model=<not loaded>` (`:1932-1934`, through
/// `router_validate_model`).
pub fn invalid_request(message: &str) -> (u16, Value) {
    (400, error_body(400, message, "invalid_request_error"))
}

/// `ERROR_TYPE_NOT_FOUND` → `404 not_found_error` (`server-common.cpp:48-51`). `POST /models/load`
/// answers it for a model that is not in the catalog (`server-models.cpp:2069-2072`).
pub fn not_found(message: &str) -> (u16, Value) {
    (404, error_body(404, message, "not_found_error"))
}

/// The answer to a route the server does not have, which is NOT `format_error_response` but the
/// httplib error handler's own literal (`server-http.cpp:199-212` @b11436):
/// `404 {"error":{"message":"File Not Found","type":"not_found_error","code":404}}`.
///
/// **Every llama-server fake in this workspace had this wrong.** Two answered
/// `{"error":{"message":"not found"}}` (`cyrup-llama/src/tests/fake_server.rs`, fixed by EXT-100;
/// `cyrup-it/tests/llama/fake.rs`, fixed by EXT-108) and the third answered a plain-text
/// `not found` (`cyrup-provider/src/tests/llama_cpp_classify_fake_server.rs`, fixed by EXT-108).
/// Nothing asserted on the text, so no test was lying, but it is exactly the drift
/// fakes written from one reading of the client produce: one wrong reading, every fake wrong,
/// every test green.
pub fn file_not_found() -> (u16, Value) {
    (
        404,
        json!({ "error": {
            "message": "File Not Found",
            "type": "not_found_error",
            "code": 404,
        }}),
    )
}

/// The API-key rejection, which is NOT `format_error_response` but a literal written in the HTTP
/// middleware (`server-http.cpp:290-300` @b11436): status 401 with
/// `{"error":{"message":"Invalid API Key","type":"authentication_error","code":401}}`.
pub fn invalid_api_key() -> (u16, Value) {
    (
        401,
        json!({ "error": {
            "message": "Invalid API Key",
            "type": "authentication_error",
            "code": 401,
        }}),
    )
}

// --------------------------------------------------------------------------------------- catalog --

/// Every `status.value` a router can report (`server_model_status_to_string`,
/// `server-models.h:52-62` @b11436).
///
/// `"downloaded"` is NOT in pi's TypeScript union (`client.ts:1`: `unloaded | loading | loaded |
/// downloading | sleeping`) but the router does emit it: `update_download_progress` sets
/// `SERVER_MODEL_STATUS_DOWNLOADED` when a download finishes, before the next `load_models()`
/// rescan erases the transient entry (`server-models.cpp:1385-1388`).
pub const STATUS_VALUES: [&str; 6] = [
    "downloading",
    "downloaded",
    "unloaded",
    "loading",
    "loaded",
    "sleeping",
];

/// Every `source` a router can report (`server_model_source_to_string`, `server-models.h:64-71`
/// @b11436). `cyrup_llama::model::model_is_selectable` offers an `unloaded` model only when this is
/// `"preset"`.
pub const SOURCE_VALUES: [&str; 4] = ["preset", "models_dir", "cache", "unknown"];

/// One catalog entry as `get_router_models` builds it for a LOADED model
/// (`server-models.cpp:2095-2136` @b11436), with every field the real router sends — including the
/// ones this client ignores (`tags`, `object`, `owned_by`, `created`, `can_remove`,
/// `status.preset`) and the child's `meta`, which reaches the catalog through the `loaded_info`
/// merge at `:2128-2135` (`get_res_model_info`, `server-context.cpp:4894-4919`).
///
/// The merge copies a child key only when the router's entry does NOT already hold it
/// (`!model_info.contains(it.key())`, `:2131`), and the entry always holds `architecture`
/// (`:2121`, the router's own `meta.architecture`), so the child's `architecture` does NOT arrive
/// through this merge. It arrives earlier: `update_status` copies the child's
/// `input_modalities`/`output_modalities` into `meta.architecture` whenever the child reports
/// `loaded_info` (`:1330-1350`), and that copy OUTLIVES the child, so an entry unloaded after a
/// load keeps the child's modalities. (Re-read at b11436 for EXT-108; the EXT-100 text credited
/// the merge.) The merge itself runs only when `meta.is_running()` — `loaded`, `loading` or
/// `sleeping` (`server-models.h:94-96`) — so an `unloaded` or `downloading` entry carries NO
/// `meta`.
pub fn loaded_entry(id: &str, args: &[&str]) -> Value {
    json!({
        "id": id,
        "aliases": [],
        "tags": [],
        "object": "model",
        "owned_by": "llamacpp",
        "created": 1_759_000_000_i64,
        "status": { "value": "loaded", "args": args },
        "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
        "source": "models_dir",
        "can_remove": false,
        // ---- merged from the child's `get_res_model_info` (`server-context.cpp:4908-4918`) ----
        "meta": {
            "vocab_type": 2,
            "n_vocab": 151_936,
            "n_ctx": 32_768,
            "n_ctx_train": 262_144,
            "n_embd": 2048,
            "n_params": 1_721_000_000_i64,
            "size": 1_070_000_000_i64,
            "ftype": "Q4_K_M",
        },
    })
}

/// A DECISION model's entry (EXT-110): a GGUF whose decision type is set (OpenJev, lev, Kev,
/// Nimble, Laya, Clef) reports `architecture.output_modalities: ["decisions"]` — exactly that one
/// value, never `"text"` beside it (`server_model_output_modalities`,
/// `server-common.cpp:150-163` @b11436). The router reads it from the GGUF metadata while
/// discovering the model (`server-models.cpp:567-571`), so a `sleeping` or `unloaded` entry
/// carries it as well as a `loaded` one; a running child's own `architecture`
/// (`get_res_model_info`, `server-context.cpp:4894-4919`) replaces both arrays in full
/// (`server-models.cpp:1330-1350`) with the same value. `meta` is merged only when the entry is
/// running (`:2128-2134`, `server-models.h:94-96`), so `status` other than
/// `loaded`/`loading`/`sleeping` gets none.
///
/// Checked against a live b11436 router (`--models-dir` holding `tinylaya-for-testing-Q8_0.gguf`):
/// unloaded and then loaded, it answered this key order and these `architecture` arrays, with
/// `meta` last once loaded. Pinned by [`crate::golden::MODELS_DECISION_KEV_LOADED_LAYA_UNLOADED`].
pub fn decision_entry(id: &str, status: &str) -> Value {
    let mut entry = json!({
        "id": id,
        "aliases": [],
        "tags": [],
        "object": "model",
        "owned_by": "llamacpp",
        "created": 1_759_000_000_i64,
        "status": { "value": status, "args": [] },
        "architecture": { "input_modalities": ["text"], "output_modalities": ["decisions"] },
        "source": "models_dir",
        "can_remove": false,
    });
    if matches!(status, "loaded" | "loading" | "sleeping")
        && let Some(object) = entry.as_object_mut()
    {
        object.insert(
            "meta".to_string(),
            json!({
                "vocab_type": 2,
                "n_vocab": 151_936,
                "n_ctx": 8192,
                "n_ctx_train": 32_768,
                "n_embd": 1024,
                "n_params": 600_000_000_i64,
                "size": 400_000_000_i64,
                "ftype": "Q8_0",
            }),
        );
    }
    entry
}

/// An `unloaded` preset entry, the autoload candidate `model_is_selectable` accepts
/// (`server-models.cpp:2095-2126`): `source: "preset"`, `status.preset` holding the rendered ini
/// (`:2099-2107`) and NO `meta`, because the `loaded_info` merge is gated on `is_running()`.
pub fn unloaded_preset_entry(id: &str, args: &[&str]) -> Value {
    json!({
        "id": id,
        "aliases": [],
        "tags": [],
        "object": "model",
        "owned_by": "llamacpp",
        "created": 1_759_000_000_i64,
        "status": {
            "value": "unloaded",
            "args": args,
            "preset": format!("[{id}]\nmodel = /models/{id}.gguf\n"),
        },
        "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
        "source": "preset",
        "can_remove": false,
    })
}

/// A failed entry: `status.failed` and `status.exit_code` appear together and ONLY when
/// `meta.is_failed()`, which is `status == UNLOADED && exit_code != 0`
/// (`server-models.cpp:2108-2111`, `server-models.h:102-104` @b11436).
pub fn failed_entry(id: &str, exit_code: i64) -> Value {
    json!({
        "id": id,
        "aliases": [],
        "tags": [],
        "object": "model",
        "owned_by": "llamacpp",
        "created": 1_759_000_000_i64,
        "status": { "value": "unloaded", "args": [], "failed": true, "exit_code": exit_code },
        "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
        "source": "models_dir",
        "can_remove": false,
    })
}

/// A `downloading` entry. **It carries no progress anywhere** — see
/// [`THE_CATALOG_CARRIES_NO_DOWNLOAD_PROGRESS`].
pub fn downloading_entry(id: &str) -> Value {
    json!({
        "id": id,
        "aliases": [],
        "tags": [],
        "object": "model",
        "owned_by": "llamacpp",
        "created": 1_759_000_000_i64,
        "status": { "value": "downloading", "args": [] },
        "architecture": { "input_modalities": ["text"], "output_modalities": ["text"] },
        "source": "cache",
        "can_remove": true,
    })
}

/// **A shape upstream pi declares and no llama.cpp release emits.**
///
/// `LlamaModelInfo.status.progress?: Record<string, { done, total }>` (pi `client.ts:11` @v0.99.2-17)
/// says a catalog entry's `status` can carry per-file download progress, and
/// `downloadAndWait`/`cyrup_llama::client::LlamaClient::download_and_wait` read it
/// (`client.ts:334-338`, `client.rs:1345-1352`). The router never puts it there:
///
/// * `get_router_models` builds `status` out of `value` and `args`, plus `preset` for a preset
///   entry and `exit_code`/`failed` for a failed one, and nothing else
///   (`server-models.cpp:2095-2111` @b11436);
/// * the per-file progress exists server-side as `server_model_meta::progress` /
///   `loaded_info["progress"]` (`server-models.h:83-84`, written at
///   `server-models.cpp:1389-1397`), but the only path from `loaded_info` into the catalog is the
///   merge at `:2128-2135`, which is gated on `meta.is_running()` — `LOADED`, `LOADING` or
///   `SLEEPING` (`server-models.h:94-96`), never `DOWNLOADING` — and lands keys at the entry's TOP
///   level, not inside `status`;
/// * checked unchanged at `b9000`, `b10000`, `b10700`, `b11000` and `b11436`.
///
/// Download progress reaches a real client ONLY through the SSE stream, as
/// [`download_progress_event`]. The code is kept: it is a faithful port of pi, and pi is the
/// contract this crate ports. What is NOT true is that a test feeding `status.progress` exercises
/// anything a real llama.cpp can produce.
pub const THE_CATALOG_CARRIES_NO_DOWNLOAD_PROGRESS: &str = concat!(
    "llama.cpp@b11436 server-models.cpp:2095-2111 builds `status` from ",
    "value/args/preset/exit_code/failed only",
);

// ----------------------------------------------------------------------------------------- props --

/// `GET /props` with NO `model` parameter: the router's own props
/// (`get_router_props`, `server-models.cpp:1992-2015` @b11436). `models_autoload` is the one field
/// `cyrup_llama::provider::router_autoload_enabled` reads; the rest is sent too and must not disturb it.
pub fn router_props(models_autoload: bool) -> Value {
    json!({
        "role": "router",
        "max_instances": 1,
        "models_autoload": models_autoload,
        "model_alias": "llama-server",
        "model_path": "none",
        "default_generation_settings": { "params": {}, "n_ctx": 0 },
        "ui_settings": {},
        "build_info": "b11436",
        "cors_proxy_enabled": false,
    })
}

/// `GET /props?model=<id>&autoload=false`: a non-empty `model` makes the router PROXY the request
/// to that child (`server-models.cpp:2015` → `proxy_get`, `:2017-2030`), so the answer is the
/// CHILD's props (`get_res_props`, `server-context.cpp:4955-4998` @b11436).
///
/// Two consequences for `cyrup_llama::client::LlamaClient::props`, which reads `models_autoload` and
/// `chat_template` from whichever of the two it gets:
///
/// * a child's props carry **no `models_autoload`** — it is a router-only field — so
///   `props(Some(id))` always yields `models_autoload: None`;
/// * `chat_template` is a child field only, which is why `to_model`'s `reasoning` test
///   (`model.rs:166-169`) is fed the per-model props and not the router's.
///
/// `autoload=false` is read by `is_autoload` (`server-models.cpp:1940-1947`) and makes
/// `router_validate_model` refuse a model that is not running rather than start it
/// (`:1920-1938`) — which is why the provider asks only for `loaded` models
/// (`provider.rs:578-584`).
pub fn child_props(chat_template: &str) -> Value {
    json!({
        "default_generation_settings": { "params": {}, "n_ctx": 32_768 },
        "total_slots": 1,
        "model_alias": "qwen",
        "model_ftype": "Q4_K_M",
        "model_path": "/models/qwen.gguf",
        "modalities": { "vision": false, "video": false, "audio": false },
        "media_marker": "<__media__>",
        "endpoint_slots": false,
        "endpoint_props": false,
        "endpoint_metrics": false,
        "ui": true,
        "ui_settings": {},
        "chat_template": chat_template,
        "chat_template_caps": {},
        "bos_token": "<|im_start|>",
        "eos_token": "<|im_end|>",
        "build_info": "b11436",
        "is_sleeping": false,
        "cors_proxy_enabled": false,
    })
}

// ------------------------------------------------------------------------------------------- SSE --

/// One `GET /models/sse` frame on the wire: `"data: " + json + "\n\n"`
/// (`server-models.cpp:2175` @b11436, inside the `res->next` generator at `:2167-2178`). The
/// response head is `200` with `content-type: text/event-stream`, and the stream ends when the
/// client disconnects or the router stops (`:2169-2174`).
pub fn sse_frame(event: &Value) -> String {
    format!("data: {event}\n\n")
}

/// The payload of every event: `{"model": <id>, "event": <name>}` plus `"data"` when the notifier
/// was given any (`notify_sse`, `server-models.cpp:686-697` @b11436). A catalog rescan broadcasts
/// `models_reload` for the pseudo-model `"*"` with no `data` at all (`:1026`).
pub fn sse_event(model: &str, event: &str, data: Option<Value>) -> Value {
    match data {
        Some(data) => json!({ "model": model, "event": event, "data": data }),
        None => json!({ "model": model, "event": event }),
    }
}

/// `status_change`, the event `update_status` broadcasts (`server-models.cpp:1358-1375` @b11436):
/// `data.status` always, `data.exit_code` when the new status is `unloaded`, `data.info` when the
/// child sent `loaded_info`, and `data.progress` when the child sent progress.
pub fn status_change_event(model: &str, status: &str, progress: Option<Value>) -> Value {
    let mut data = json!({ "status": status });
    if let (Some(progress), Some(object)) = (progress, data.as_object_mut()) {
        object.insert("progress".to_string(), progress);
    }
    sse_event(model, "status_change", Some(data))
}

/// The LOAD progress a child reports while it is loading its weights
/// (`server-context.cpp:1095-1101` @b11436, the `load_progress_callback`): `stages` (every stage
/// it will run), `current` (the one running) and `value` (that stage's 0..1 ratio). It reaches the
/// client as `status_change`'s `data.progress` (`server-models.cpp:1371-1373`).
pub fn load_progress(stages: &[&str], current: &str, value: f64) -> Value {
    json!({ "stages": stages, "current": current, "value": value })
}

/// The OTHER load-progress payload, and the reason
/// `cyrup_llama::client::parse_load_progress` falls back from `current` to `stage`: starting the
/// multimodal projector reports `{"stage": "mmproj_model"}` and NOTHING else — no `stages`, no
/// `value` (`server-context.cpp:1270-1272` @b11436).
pub fn mmproj_load_progress() -> Value {
    json!({ "stage": "mmproj_model" })
}

/// `download_progress` (`update_download_progress`, `server-models.cpp:1380-1409` @b11436): the
/// event's `data` is the whole `loaded_info` copy, so the per-file map sits NESTED under
/// `progress`, keyed by download URL, each `{done, total}` in bytes (`:1389-1396`).
pub fn download_progress_event(model: &str, files: &[(&str, u64, u64)]) -> Value {
    let mut progress = serde_json::Map::new();
    for (url, done, total) in files {
        progress.insert((*url).to_string(), json!({ "done": done, "total": total }));
    }
    sse_event(
        model,
        "download_progress",
        Some(json!({ "progress": Value::Object(progress) })),
    )
}

/// `download_finished` / `download_failed` (`server-models.cpp:1403-1405` @b11436): `notify_sse`
/// is called with `{}`, which is not null, so the event carries an EMPTY `data` object — no
/// message, which is why `download_and_wait` composes its own `"Download failed"`
/// (`client.rs:1289`).
pub fn download_settled_event(model: &str, ok: bool) -> Value {
    sse_event(
        model,
        if ok {
            "download_finished"
        } else {
            "download_failed"
        },
        Some(json!({})),
    )
}
