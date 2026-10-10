//! The llama.cpp router wire, ONE definition, grounded in llama.cpp's own server source
//! (EXT-100).
//!
//! # Why this file exists
//!
//! Every llama.cpp test in this workspace runs against a FAKE server, and so does pi's
//! (`packages/coding-agent/test/llama-extension.test.ts:16`, an in-process `node:http`
//! `createServer`). A fake is written from a reading of the client, so a wire shape the fake and
//! the client agree on passes every test even when the real server does something else. This
//! module is the fix available without a live server: the shapes llama.cpp's own handlers build,
//! transcribed with a file:line citation each, in ONE place, so
//! [`super::fake_server::FakeLlamaServer`] cannot drift from them and an auditor can re-derive
//! every one of them from the pin below.
//!
//! # Upstream pin
//!
//! Every `llama.cpp@b11436` citation in this file means tag `b11436`
//! (`b9a5a00b86fd285a445916086a0b1dc35bee6d66`, 2026-10-06), the current llama.cpp release at the
//! time of writing. That is the version this repo targets: `docs/guide/llama-cpp.md:16-19` tells
//! the operator to "use a current llama.cpp build with router support", and cyrup pins no other
//! llama.cpp version anywhere. Read it the way the other upstream pins in this workspace are read,
//! never from a working tree:
//!
//! ```text
//! git -C tmp/llama.cpp show b11436:tools/server/server-models.cpp
//! ```
//!
//! **Floor.** The router's management API arrived in `b9688`, "server: (router) add model
//! management API (#23976)" (2026-06-17) — the first release whose `GET /models` carries the
//! `source`, `architecture` and merged child-`meta` fields [`crate::model`] reads. `b9000`'s
//! router answered `id`/`aliases`/`tags`/`status` only, so no release before `b9688` can drive
//! this client. Every shape below was re-checked unchanged at `b10000`, `b10700` and `b11000`.
//!
//! # Scope
//!
//! This covers the ROUTER MANAGEMENT API, the only part of the wire `crate::client` speaks:
//! `GET /models`, `GET /props`, `POST /models/load`, `POST /models/unload`, `POST /models` and
//! `GET /models/sse` (registered at `tools/server/server.cpp:256-269` @b11436). The classifier
//! wire (`/tokenize`, `/apply-template`, `/completion` + `n_probs`) belongs to
//! `cyrup-provider`'s `llama-cpp-classify` api and has its own fake there, which this crate cannot
//! share a definition with: `cyrup-llama` depends on `cyrup-provider` (`Cargo.toml:19`), so the
//! definition cannot live here and be used there.
//!
//! That classifier wire WAS audited against the same pin, and the result is a clean bill:
//!
//! * `POST /tokenize` answers `{"tokens": [<int>, ...]}` (`server-context.cpp:5440-5479`, the
//!   `with_pieces: false` branch at `:5472-5473` and `res->ok` at `:5476`), which is what
//!   `cyrup_provider::api::llama_cpp_classify` reads (`llama_cpp_classify.rs:926`);
//! * `POST /apply-template` answers `{"prompt": <string>}` (`server-context.cpp:5416-5425`), read
//!   at `llama_cpp_classify.rs:1088`;
//! * `POST /completion` with `post_sampling_probs: false` answers
//!   `completion_probabilities: [{id, token, bytes, logprob, top_logprobs: [{id, token, bytes,
//!   logprob}]}]` (`server-task.cpp:282-301`, attached to the non-streamed final result at
//!   `:359-360`), and the api reads `completion_probabilities[0].top_logprobs[].{id, logprob}`
//!   (`llama_cpp_classify.rs:1123-1140`) having sent `post_sampling_probs: false` itself
//!   (`:1116`), which is the switch between `logprob`/`top_logprobs` and `prob`/`top_probs`.
//!
//! Both classifier fakes (`cyrup-provider/src/tests/llama_cpp_classify_fake_server.rs:383-398`
//! and `cyrup-it/tests/llama/fake.rs:329-351`, byte-identical in shape) are faithful on every
//! field the api reads. They omit the outer entry's own `bytes` and `logprob`, which a real server
//! always sends and nothing reads.

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

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
/// `code`. [`crate::client::LlamaClient`] reads `error.message` and ignores the rest
/// (`client.rs:183` upstream).
pub fn error_body(code: u16, message: &str, kind: &str) -> Value {
    json!({ "error": { "code": code, "message": message, "type": kind } })
}

/// `ERROR_TYPE_INVALID_REQUEST` → `400 invalid_request_error` (`server-common.cpp:40-43`). The
/// router answers this for an unknown model on `/models/unload` (`server-models.cpp:2152`), for a
/// model that is not running (`:2156`) and for `/props?model=<not loaded>` (`:1952-1956`, through
/// `router_validate_model`).
pub fn invalid_request(message: &str) -> (u16, Value) {
    (400, error_body(400, message, "invalid_request_error"))
}

/// `ERROR_TYPE_NOT_FOUND` → `404 not_found_error` (`server-common.cpp:48-51`). `POST /models/load`
/// answers it for a model that is not in the catalog (`server-models.cpp:2070-2073`).
pub fn not_found(message: &str) -> (u16, Value) {
    (404, error_body(404, message, "not_found_error"))
}

/// The answer to a route the server does not have, which is NOT `format_error_response` but the
/// httplib error handler's own literal (`server-http.cpp:199-211` @b11436):
/// `404 {"error":{"message":"File Not Found","type":"not_found_error","code":404}}`.
///
/// **Both of this workspace's llama-server fakes had this wrong, identically** — they answered
/// `{"error":{"message":"not found"}}` (`cyrup-llama/src/tests/fake_server.rs` before EXT-100 and
/// `cyrup-it/tests/llama/fake.rs:357` still). Nothing asserts on the text today, so no test was
/// lying, but it is exactly the drift two fakes written from one reading of the client produce:
/// one wrong reading, two wrong fakes, every test green.
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
/// @b11436). `crate::model::model_is_selectable` offers an `unloaded` model only when this is
/// `"preset"`.
pub const SOURCE_VALUES: [&str; 4] = ["preset", "models_dir", "cache", "unknown"];

/// One catalog entry as `get_router_models` builds it for a LOADED model
/// (`server-models.cpp:2095-2136` @b11436), with every field the real router sends — including the
/// ones this client ignores (`tags`, `object`, `owned_by`, `created`, `can_remove`,
/// `status.preset`) and the child's `meta`/`architecture`, which reach the catalog through the
/// `loaded_info` merge at `:2128-2135` (`get_res_model_info`, `server-context.cpp:4894-4919`).
///
/// The merge happens only when `meta.is_running()` — `loaded`, `loading` or `sleeping`
/// (`server-models.h:94-96`) — so an `unloaded` or `downloading` entry carries NO `meta` and no
/// child `architecture`.
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
/// (`server-models.cpp:1330-1345`) with the same value. `meta` is merged only when the entry is
/// running (`:2128-2135`), so `status` other than `loaded`/`loading`/`sleeping` gets none.
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
/// `downloadAndWait`/[`crate::client::LlamaClient::download_and_wait`] read it
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
/// `crate::provider::router_autoload_enabled` reads; the rest is sent too and must not disturb it.
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
/// to that child (`server-models.cpp:2013-2014` → `proxy_get`, `:2017-2030`), so the answer is the
/// CHILD's props (`get_res_props`, `server-context.cpp:4955-4998` @b11436).
///
/// Two consequences for [`crate::client::LlamaClient::props`], which reads `models_autoload` and
/// `chat_template` from whichever of the two it gets:
///
/// * a child's props carry **no `models_autoload`** — it is a router-only field — so
///   `props(Some(id))` always yields `models_autoload: None`;
/// * `chat_template` is a child field only, which is why `to_model`'s `reasoning` test
///   (`model.rs:166-169`) is fed the per-model props and not the router's.
///
/// `autoload=false` is read by `is_autoload` (`server-models.cpp:2005-2012`) and makes
/// `router_validate_model` refuse a model that is not running rather than start it
/// (`:1947-1962`) — which is why the provider asks only for `loaded` models
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
/// [`crate::client::parse_load_progress`] falls back from `current` to `stage`: starting the
/// multimodal projector reports `{"stage": "mmproj_model"}` and NOTHING else — no `stages`, no
/// `value` (`server-context.cpp:1270-1272` @b11436).
pub fn mmproj_load_progress() -> Value {
    json!({ "stage": "mmproj_model" })
}

/// `download_progress` (`update_download_progress`, `server-models.cpp:1380-1404` @b11436): the
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

/// `download_finished` / `download_failed` (`server-models.cpp:1400-1402` @b11436): `notify_sse`
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

// ================================================================================================
// Conformance: the real shapes above, through the real client.
// ================================================================================================

#[cfg(test)]
mod conformance {
    use std::time::Duration;

    use serde_json::json;
    use tokio_util::sync::CancellationToken;

    use super::{
        child_props, download_settled_event, downloading_entry, failed_entry, invalid_request,
        load_progress, loaded_entry, mmproj_load_progress, models_envelope, router_props,
        sse_event, sse_frame, status_change_event, success, unloaded_preset_entry,
    };
    use crate::client::{
        LlamaClient, LlamaModelStatus, ProgressField, parse_download_progress, parse_load_progress,
    };
    use crate::model::model_is_selectable;
    use crate::tests::fake_server::{FakeLlamaServer, Reply};

    fn never() -> CancellationToken {
        CancellationToken::new()
    }

    async fn client(server: &FakeLlamaServer) -> LlamaClient {
        LlamaClient::new(server.url(), None, None)
            .await
            .expect("a client")
            .with_request_timeout(Duration::from_secs(5))
    }

    /// A whole real catalog — every field `get_router_models` sends, for a loaded model, an
    /// unloaded preset, a failed entry and a downloading one — parses, and nothing the client
    /// ignores disturbs what it reads.
    ///
    /// The existing tests feed `{"id":…,"status":{"value":…}}`; this one feeds what the router
    /// actually writes, so a field the server adds in a shape the deserializer cannot take is
    /// caught here.
    #[tokio::test]
    async fn the_whole_real_catalog_parses() {
        let server = FakeLlamaServer::start().await;
        server.set_models(vec![
            loaded_entry("qwen", &["--ctx-size", "16384"]),
            unloaded_preset_entry("preset-model", &["-c", "8192"]),
            failed_entry("broken", 3),
            downloading_entry("owner/repo:Q4_K_M"),
        ]);
        let models = client(&server).await.list(false, &never()).await.unwrap();
        assert_eq!(models.len(), 4, "{models:?}");

        let loaded = &models[0];
        assert_eq!(loaded.id, "qwen");
        assert_eq!(loaded.status.value, LlamaModelStatus::Loaded);
        assert_eq!(
            loaded.status.args.as_deref(),
            Some(["--ctx-size".to_string(), "16384".to_string()].as_slice())
        );
        assert_eq!(loaded.source.as_deref(), Some("models_dir"));
        assert_eq!(
            loaded.meta.as_ref().and_then(|meta| meta.n_ctx),
            Some(32_768),
            "the child's `meta` reaches the catalog through the loaded_info merge"
        );
        assert_eq!(
            loaded.meta.as_ref().and_then(|meta| meta.n_ctx_train),
            Some(262_144)
        );
        assert_eq!(
            loaded
                .architecture
                .as_ref()
                .and_then(|a| a.input_modalities.as_deref()),
            Some(["text".to_string()].as_slice())
        );

        let preset = &models[1];
        assert_eq!(preset.status.value, LlamaModelStatus::Unloaded);
        assert_eq!(preset.source.as_deref(), Some("preset"));
        assert!(
            preset.meta.is_none(),
            "an unloaded entry carries no child meta; got {:?}",
            preset.meta
        );
        assert!(
            model_is_selectable(preset, true),
            "an unloaded `preset` entry is the autoload candidate"
        );

        let failed = &models[2];
        assert_eq!(failed.status.failed, Some(true));
        assert!(failed.status.exit_code.is_some(), "{failed:?}");
        assert!(!model_is_selectable(failed, true));

        let downloading = &models[3];
        assert_eq!(downloading.status.value, LlamaModelStatus::Downloading);
        assert!(
            downloading.status.progress.is_none(),
            "{}",
            super::THE_CATALOG_CARRIES_NO_DOWNLOAD_PROGRESS
        );
        assert!(!model_is_selectable(downloading, true));
    }

    /// `"downloaded"` is a status the router emits (`server-models.h:52-62`) and pi's union does
    /// not name (`client.ts:1`). It must survive as an unknown status rather than collapse onto a
    /// named one, or a transient post-download entry would be offered as a model.
    #[tokio::test]
    async fn every_real_status_string_round_trips_and_only_three_are_selectable() {
        let server = FakeLlamaServer::start().await;
        server.set_models(
            super::STATUS_VALUES
                .iter()
                .map(|status| {
                    json!({
                        "id": *status,
                        "status": { "value": *status, "args": [] },
                        "source": "preset",
                    })
                })
                .collect(),
        );
        let models = client(&server).await.list(false, &never()).await.unwrap();
        for (model, status) in models.iter().zip(super::STATUS_VALUES) {
            assert_eq!(
                model.status.value.as_str(),
                status,
                "{status} must survive the round trip"
            );
        }
        assert_eq!(
            models
                .iter()
                .filter(|model| model_is_selectable(model, true))
                .map(|model| model.id.clone())
                .collect::<Vec<_>>(),
            vec![
                "unloaded".to_string(),
                "loaded".to_string(),
                "sleeping".to_string()
            ],
            "`downloaded` and `downloading` are not offerable; `unloaded` is, as an autoload preset"
        );
    }

    /// The ROUTER's `/props` (no `model`) yields `models_autoload`; the CHILD's (`?model=`, a
    /// proxied answer) yields `chat_template` and NO `models_autoload`, because that field is
    /// router-only (`server-models.cpp:1996-2002` vs `server-context.cpp:4969-4997`).
    #[tokio::test]
    async fn router_props_and_child_props_each_yield_their_own_fields() {
        let server = FakeLlamaServer::start().await;
        server.set_props(router_props(true));
        let client = client(&server).await;
        let router = client.props(None, &never()).await.unwrap();
        assert_eq!(router.models_autoload, Some(true));
        assert_eq!(
            router.chat_template, None,
            "the router's props carry no chat template"
        );

        server.set_props(child_props("{%- if enable_thinking %}<think>{%- endif %}"));
        let child = client.props(Some("qwen"), &never()).await.unwrap();
        assert_eq!(
            child.models_autoload, None,
            "a child's props have no models_autoload"
        );
        assert!(
            child
                .chat_template
                .as_deref()
                .is_some_and(|template| template.contains("enable_thinking")),
            "{child:?}"
        );
        let asked = server.requests_to("GET", "/props");
        assert!(
            asked
                .last()
                .and_then(|request| request.query().map(str::to_string))
                .is_some_and(
                    |query| query.contains("model=qwen") && query.contains("autoload=false")
                ),
            "a per-model props asks the router to proxy WITHOUT loading; got {asked:?}"
        );
    }

    /// A router error body is `{"error":{code,message,type}}` with the status equal to `code`
    /// (`server-common.cpp:36-78`, `server-models.cpp:1915-1918`), and the client surfaces
    /// `error.message` verbatim — not its `llama.cpp returned HTTP <n>` fallback.
    #[tokio::test]
    async fn a_real_router_error_surfaces_its_message() {
        let server = FakeLlamaServer::start().await;
        // `post_router_models_unload` for a model that is not running (`server-models.cpp:2156`).
        let (status, body) = invalid_request("model is not running");
        server.respond("POST", "/models/unload", Reply::Json(status, body));
        let error = client(&server)
            .await
            .unload("qwen", &never())
            .await
            .expect_err("a 400 fails");
        assert_eq!(error.to_string(), "model is not running", "{error:?}");
    }

    /// The two load-progress payloads a child really sends, through the real SSE framing. The
    /// `mmproj` one carries `stage` alone — no `stages`, no `value` — which is why the parser
    /// falls back from `current` to `stage`.
    #[test]
    fn both_real_load_progress_payloads_are_understood() {
        let staged = status_change_event(
            "qwen",
            "loading",
            Some(load_progress(
                &["text_model", "mmproj_model"],
                "text_model",
                0.5,
            )),
        );
        let data = staged.get("data").expect("data");
        let parsed = parse_load_progress(data).expect("staged progress");
        assert_eq!(parsed.message, "Loading text model");
        assert_eq!(
            parsed.ratio,
            ProgressField::Set(0.25),
            "stage 0 of 2, half done"
        );

        let mmproj = status_change_event("qwen", "loading", Some(mmproj_load_progress()));
        let data = mmproj.get("data").expect("data");
        let parsed = parse_load_progress(data).expect("mmproj progress");
        assert_eq!(
            parsed.message, "Loading mmproj model",
            "the child names the stage in `stage`, not `current` (server-context.cpp:1270-1272)"
        );
    }

    /// `download_progress`'s `data` is the whole `loaded_info`, so the per-file map is NESTED under
    /// `progress` (`server-models.cpp:1389-1396`) — the branch of `parseDownloadProgress` that
    /// looks one level down.
    #[test]
    fn the_real_download_progress_event_is_the_nested_form() {
        let event = super::download_progress_event(
            "owner/repo",
            &[
                ("https://example/a.gguf", 100, 400),
                ("https://example/b.gguf", 100, 400),
            ],
        );
        let data = event.get("data").expect("data");
        assert!(
            data.get("progress").is_some(),
            "the real event nests its file map under `progress`: {data}"
        );
        let parsed = parse_download_progress(data).expect("download progress");
        assert_eq!(parsed.ratio, ProgressField::Set(0.25));
        assert_eq!(
            parsed.detail,
            ProgressField::Set("200 B / 800 B".to_string())
        );
    }

    /// The three real SSE payloads, through the real reader on a real socket: a rescan broadcasts
    /// `models_reload` for the pseudo-model `"*"` with NO `data` key at all
    /// (`server-models.cpp:1026`, `:686-696`), and a settled download broadcasts an EMPTY `data`
    /// object (`:1400-1402`). The decoder must take all three; a `data`-less frame in particular is
    /// the one upstream's `isModelEvent` guard could reject.
    #[tokio::test]
    async fn the_real_sse_payloads_decode_including_the_one_without_data() {
        let server = FakeLlamaServer::start().await;
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = std::sync::Arc::clone(&seen);
        let client = client(&server).await;
        let cancel = never();
        let watching = tokio::spawn({
            let cancel = cancel.clone();
            async move {
                client
                    .watch(
                        move |event| {
                            sink.lock().expect("the sink").push((
                                event.model,
                                event.event,
                                event.data,
                            ));
                        },
                        &cancel,
                    )
                    .await
            }
        });
        server.wait_for_sse(1).await;
        server.run(vec![
            crate::tests::fake_server::Step::Sse(sse_event("*", "models_reload", None)),
            crate::tests::fake_server::Step::Sse(download_settled_event("owner/repo", true)),
            crate::tests::fake_server::Step::Sse(download_settled_event("owner/repo", false)),
            crate::tests::fake_server::Step::CloseSse,
        ]);
        watching.await.expect("the watcher").expect("a clean end");
        let seen = seen.lock().expect("the sink").clone();
        assert_eq!(
            seen,
            vec![
                ("*".to_string(), "models_reload".to_string(), None),
                (
                    "owner/repo".to_string(),
                    "download_finished".to_string(),
                    Some(json!({}))
                ),
                (
                    "owner/repo".to_string(),
                    "download_failed".to_string(),
                    Some(json!({}))
                ),
            ],
            "a `models_reload` frame has no `data` key, and a settled download has an empty one"
        );
    }

    /// Because `download_failed` carries an empty `data` (`server-models.cpp:1400-1402`), the
    /// failure a real router produces has NO message of its own, and what the user sees is the
    /// client's own `"Download failed"` (`client.rs:1289`) — not a server string.
    #[tokio::test]
    async fn a_real_download_failure_reports_the_clients_own_message() {
        let server = FakeLlamaServer::start().await;
        server.on_download(
            "owner/repo",
            vec![
                crate::tests::fake_server::Step::WaitForSse(1),
                crate::tests::fake_server::Step::Sse(download_settled_event("owner/repo", false)),
            ],
        );
        let error = client(&server)
            .await
            .download_and_wait("owner/repo", &|_| {}, &never())
            .await
            .expect_err("a failed download fails");
        assert_eq!(
            error.to_string(),
            "Download failed",
            "the router says nothing, so the message is the client's"
        );
    }

    /// The fake this crate's tests run against ANSWERS with the definitions above rather than its
    /// own transcription of them, so the fake cannot drift from llama.cpp without this module
    /// changing. Driving the real client through every router mutation and reading the fake's
    /// answers back is the proof of that wiring.
    #[tokio::test]
    async fn the_fake_server_answers_with_these_definitions() {
        let server = FakeLlamaServer::start().await;
        server.set_models(vec![loaded_entry("qwen", &[])]);
        server.set_props(router_props(true));
        let client = client(&server).await;

        // `GET /models`: the `{data, object: "list"}` envelope of `models_envelope`. A different
        // envelope fails the parse with `llama.cpp returned an invalid model catalog`.
        let listed = client.list(false, &never()).await.unwrap();
        assert_eq!(listed.len(), 1, "{listed:?}");

        // The three mutations: `{"success": true}`, status 200, or these would be errors.
        client.load("qwen", &never()).await.unwrap();
        client.unload("qwen", &never()).await.unwrap();
        client.download("owner/repo", &never()).await.unwrap();
        assert_eq!(success(), serde_json::json!({ "success": true }));

        // An unknown route: `format_error_response`'s shape, with the status equal to its `code`,
        // so the client reports the router's own message.
        let error = client
            .props(Some("nope"), &never())
            .await
            .expect("the fake answers /props for any model");
        assert_eq!(error.models_autoload, Some(true), "{error:?}");

        // The SSE framing is `sse_frame`'s: `data: <json>\n\n`.
        assert_eq!(
            sse_frame(&sse_event("*", "models_reload", None)),
            "data: {\"model\":\"*\",\"event\":\"models_reload\"}\n\n"
        );
        assert_eq!(
            models_envelope(vec![serde_json::json!({ "id": "x" })]),
            serde_json::json!({ "data": [{ "id": "x" }], "object": "list" })
        );
    }
}

// ================================================================================================
// EXT-110: decision models, through the real controller.
// ================================================================================================

#[cfg(test)]
mod decision_models {
    use std::sync::{Arc, Mutex};

    use serde_json::json;

    use cyrup_core::CancelToken;
    use cyrup_provider::auth::{Credential, InMemoryCredentialStore, ProviderEnv};
    use cyrup_provider::{AnyModel, Provider};

    use super::{decision_entry, loaded_entry};
    use crate::client::LlamaModelInfo;
    use crate::error::LlamaError;
    use crate::provider::{
        CatalogEntry, CatalogPublication, CatalogPublisher, LlamaController,
        LlamaControllerOptions, LlamaRefreshContext, RegisterProviderFn, SetCatalogOptions,
    };
    use crate::tests::fake_server::FakeLlamaServer;

    /// Persist, then update, then answer `true` — pi's test `publish`.
    #[derive(Default)]
    struct Publisher(Mutex<Option<CatalogEntry>>);

    #[async_trait::async_trait]
    impl CatalogPublisher for Publisher {
        async fn publish(&self, publication: CatalogPublication) -> Result<bool, LlamaError> {
            if let Some(entry) = publication.persist {
                *self.0.lock().unwrap() = Some(entry);
            }
            if let Some(update) = publication.update {
                update();
            }
            Ok(true)
        }
    }

    fn controller() -> LlamaController {
        let register: RegisterProviderFn = Arc::new(|_: Arc<dyn Provider>| Ok(()));
        LlamaController::new(LlamaControllerOptions::new(
            Arc::new(InMemoryCredentialStore::new()),
            register,
        ))
    }

    fn chat_ids(controller: &LlamaController) -> Vec<String> {
        controller
            .provider()
            .models()
            .iter()
            .map(|model| model.id.as_str().to_string())
            .collect()
    }

    fn classifier_ids(controller: &LlamaController) -> Vec<String> {
        controller
            .provider()
            .get_all_models()
            .iter()
            .filter(|model| matches!(model, AnyModel::Classifier(_)))
            .map(|model| model.id().to_string())
            .collect()
    }

    /// pi `exposes decision models reported by the catalog as native System One classifiers`
    /// (`llama-extension.test.ts`, f6127a1bf), its catalog half: a real router catalog with a
    /// text model, a loaded and a sleeping decision model and a pre-0.6.0 entry with no
    /// `architecture`. The decision models are not chat models, their `/props` is never asked
    /// for, and they stay classifiers.
    #[tokio::test]
    async fn a_refresh_lists_decision_models_as_classifiers_only_and_never_reads_their_props() {
        let server = FakeLlamaServer::start().await;
        let mut legacy = loaded_entry("legacy", &[]);
        legacy.as_object_mut().unwrap().remove("architecture");
        server.set_models(vec![
            loaded_entry("qwen", &[]),
            decision_entry("kev", "loaded"),
            decision_entry("laya", "sleeping"),
            legacy,
        ]);
        server.set_props(json!({}));
        let mut env = ProviderEnv::new();
        env.insert("LLAMA_BASE_URL".to_string(), server.url().to_string());
        let credential = Credential::ApiKey {
            key: Some("local".to_string()),
            env: Some(env),
        };
        let publisher = Publisher::default();
        let controller = controller();
        let cancel = CancelToken::new();
        controller
            .provider()
            .refresh(&LlamaRefreshContext {
                credential: Some(&credential),
                stored: None,
                publisher: &publisher,
                allow_network: true,
                cancel: &cancel,
            })
            .await
            .unwrap();

        let mut props: Vec<String> = server
            .requests_to("GET", "/props")
            .iter()
            .map(|request| request.query().unwrap_or_default().to_string())
            .collect();
        props.sort();
        assert_eq!(
            props,
            [
                "model=legacy&autoload=false".to_string(),
                "model=qwen&autoload=false".to_string()
            ],
            "only the chat models' props are read; `kev` is loaded and still not asked"
        );
        assert_eq!(chat_ids(&controller), ["qwen", "legacy"]);
        assert_eq!(
            classifier_ids(&controller),
            ["qwen", "kev", "laya", "legacy"]
        );
        let persisted: Vec<(String, &'static str)> = publisher
            .0
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .models
            .iter()
            .map(|model| {
                let kind = match model {
                    AnyModel::Chat(_) => "chat",
                    AnyModel::Classifier(_) => "classifier",
                    AnyModel::Image(_) => "image",
                };
                (model.id().to_string(), kind)
            })
            .collect();
        assert_eq!(
            persisted,
            [
                ("qwen".to_string(), "chat"),
                ("legacy".to_string(), "chat"),
                ("qwen".to_string(), "classifier"),
                ("kev".to_string(), "classifier"),
                ("laya".to_string(), "classifier"),
                ("legacy".to_string(), "classifier"),
            ]
        );
    }

    /// pi `lists decision models that also output text for chat` (`llama-extension.test.ts`,
    /// f6127a1bf): `isChatModel` keeps a model that reports `"text"` beside `"decisions"`. No
    /// llama.cpp release emits that pair (`server-common.cpp:150-163` @b11436 answers one or the
    /// other), so this pins pi's predicate rather than a router shape.
    #[test]
    fn set_catalog_keeps_a_text_and_decisions_model_in_both_lists() {
        let catalog: Vec<LlamaModelInfo> = [
            json!({ "id": "decide", "status": { "value": "sleeping" },
                    "architecture": { "output_modalities": ["decisions"] } }),
            json!({ "id": "hybrid", "status": { "value": "loaded" },
                    "architecture": { "output_modalities": ["text", "decisions"] } }),
        ]
        .into_iter()
        .map(|entry| serde_json::from_value(entry).unwrap())
        .collect();
        let controller = controller();
        controller
            .set_catalog(
                &catalog,
                "http://localhost:8080",
                SetCatalogOptions::default(),
            )
            .unwrap();
        assert_eq!(chat_ids(&controller), ["hybrid"]);
        assert_eq!(classifier_ids(&controller), ["decide", "hybrid"]);
    }
}
