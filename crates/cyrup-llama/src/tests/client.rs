//! Tests for the `client` module (`client.ts`), over the loopback router fake in
//! [`super::fake_server`].
//!
//! Ports the client cases of pi's `packages/coding-agent/test/llama-extension.test.ts`:
//! "normalizes management and inference URLs" (`:56-59`), "loads with SSE progress and waits for
//! the loaded catalog state" (`:486-527`) and "downloads with byte progress and returns the
//! refreshed catalog" (`:529-573`). Everything else here pins behaviour upstream implements but
//! does not test: the wire shapes, the SSE reader's framing, the failure and cancellation paths,
//! and the polling fallbacks. Every HTTP request goes to a `127.0.0.1:0` listener through a client
//! with proxying disabled, so an ambient `HTTP_PROXY` cannot reroute a test.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::fake_server::{
    FakeLlamaServer, Reply, Step, event, model, model_with, model_with_status,
};
use crate::client::{
    LlamaClient, LlamaError, LlamaModelInfo, LlamaModelStatus, LlamaModelStatusInfo, LlamaProgress,
    ProgressField, format_bytes, llama_inference_url, normalize_llama_server_url,
    parse_download_progress, parse_load_progress,
};

fn http() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

fn client(server: &FakeLlamaServer) -> LlamaClient {
    LlamaClient::with_http_client(server.url(), None, http()).unwrap()
}

fn keyed_client(server: &FakeLlamaServer, key: &str) -> LlamaClient {
    LlamaClient::with_http_client(server.url(), Some(key.to_string()), http()).unwrap()
}

fn never() -> CancellationToken {
    CancellationToken::new()
}

/// A progress sink plus the closure to hand to `load_and_wait` / `download_and_wait`.
fn collector() -> (
    Arc<Mutex<Vec<LlamaProgress>>>,
    impl Fn(LlamaProgress) + Send + Sync,
) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (seen, move |progress| sink.lock().unwrap().push(progress))
}

fn entry(id: &str, status: &str) -> LlamaModelInfo {
    LlamaModelInfo {
        id: id.to_string(),
        status: LlamaModelStatusInfo {
            value: match status {
                "loaded" => LlamaModelStatus::Loaded,
                "unloaded" => LlamaModelStatus::Unloaded,
                other => LlamaModelStatus::Other(other.to_string()),
            },
            ..LlamaModelStatusInfo::default()
        },
        ..LlamaModelInfo::default()
    }
}

// ------------------------------------------------------------------------------- URL handling --

/// `llama-extension.test.ts:56-59`.
#[test]
fn normalizes_management_and_inference_urls() {
    assert_eq!(
        normalize_llama_server_url("http://127.0.0.1:8080/v1/").unwrap(),
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        normalize_llama_server_url("https://example.com/prefix/v1").unwrap(),
        "https://example.com/prefix"
    );
    let error = normalize_llama_server_url("file:///tmp/llama").unwrap_err();
    assert_eq!(error.to_string(), "Server URL must use http or https");
    assert_eq!(
        llama_inference_url("http://127.0.0.1:8080/v1/").unwrap(),
        "http://127.0.0.1:8080/v1"
    );
}

/// `client.ts:147-154`: trim, drop hash and search, strip trailing slashes and one `/v1`, keep
/// the rest of the path.
#[test]
fn normalization_strips_hash_search_slashes_and_only_a_trailing_v1() {
    for (input, expected) in [
        ("  http://localhost:8080  ", "http://localhost:8080"),
        ("http://localhost:8080/", "http://localhost:8080"),
        ("http://localhost:8080?x=1#frag", "http://localhost:8080"),
        ("http://localhost:8080/v1?x=1#frag", "http://localhost:8080"),
        ("http://localhost:8080/v1///", "http://localhost:8080"),
        ("http://h/a/b/", "http://h/a/b"),
        ("http://h/v1/proxy", "http://h/v1/proxy"),
        ("http://h/v1/v1", "http://h/v1"),
        ("HTTPS://Example.COM:443/x/v1", "https://example.com/x"),
    ] {
        assert_eq!(
            normalize_llama_server_url(input).unwrap(),
            expected,
            "{input}"
        );
    }
    assert_eq!(
        normalize_llama_server_url("ftp://host/")
            .unwrap_err()
            .to_string(),
        "Server URL must use http or https"
    );
    assert_eq!(
        normalize_llama_server_url("not a url")
            .unwrap_err()
            .to_string(),
        "Invalid URL"
    );
}

#[tokio::test]
async fn the_client_exposes_the_normalized_server_url() {
    let client = LlamaClient::new("http://127.0.0.1:9/v1/", None, None)
        .await
        .unwrap();
    assert_eq!(client.server_url(), "http://127.0.0.1:9");
    assert!(LlamaClient::new("gopher://x", None, None).await.is_err());
}

/// `client.ts:134-144`.
#[test]
fn format_bytes_matches_the_upstream_units_and_precision() {
    for (bytes, expected) in [
        (0.0, "0 B"),
        (512.0, "512 B"),
        (1023.0, "1023 B"),
        (1024.0, "1.00 KiB"),
        (1536.0, "1.50 KiB"),
        (10_240.0, "10.0 KiB"),
        (1024.0 * 1024.0, "1.00 MiB"),
        (1024.0 * 1024.0 * 12.34, "12.3 MiB"),
        (1024.0 * 1024.0 * 1024.0 * 3.0, "3.00 GiB"),
        (1024.0 * 1024.0 * 1024.0 * 1024.0 * 2.0, "2.00 TiB"),
        // Past the last unit the value keeps growing in TiB (`client.ts:139` stops at index 3).
        (1024.0_f64.powi(5), "1024.0 TiB"),
    ] {
        assert_eq!(format_bytes(bytes), expected, "{bytes}");
    }
}

/// JS `toFixed` rounds an exact tie up, Rust's `{:.2}` rounds it to even: `1.125` is `1.13` in
/// JS. 1152 B is 1.125 KiB, 1280 B is 1.25 KiB (no tie).
#[test]
fn format_bytes_rounds_exact_ties_up_like_to_fixed() {
    assert_eq!(format_bytes(1152.0), "1.13 KiB");
    assert_eq!(format_bytes(1024.0 * 10.25), "10.3 KiB");
    assert_eq!(format_bytes(1280.0), "1.25 KiB");
    // Rounding carries into the integer part; the precision was chosen before rounding
    // (`value >= 10`, `client.ts:143`), so this reads `10.00`.
    assert_eq!(format_bytes(1024.0 * 9.996), "10.00 KiB");
    assert_eq!(format_bytes(1024.0 * 1023.996), "1024.0 KiB");
}

// ------------------------------------------------------------------------- progress parsing --

/// `client.ts:91-111`: the ratio is `(stageIndex + value) / stages`, the message names the stage
/// with underscores as spaces.
#[test]
fn load_progress_combines_stage_index_and_value() {
    let progress = parse_load_progress(&json!({
        "status": "loading",
        "progress": { "stages": ["text_model", "mmproj_model"], "current": "mmproj_model", "value": 0.5 }
    }))
    .unwrap();
    assert_eq!(progress.message, "Loading mmproj model");
    assert_eq!(progress.ratio, ProgressField::Set(0.75));
    assert_eq!(progress.detail, ProgressField::Keep);
}

/// `client.ts:96-97`: `current` wins over `stage`; either is accepted.
#[test]
fn load_progress_accepts_stage_as_a_fallback_for_current() {
    let by_stage = parse_load_progress(&json!({
        "progress": { "stages": ["a", "b"], "stage": "b", "value": 0.0 }
    }))
    .unwrap();
    assert_eq!(by_stage.message, "Loading b");
    assert_eq!(by_stage.ratio, ProgressField::Set(0.5));
    let both = parse_load_progress(&json!({
        "progress": { "stages": ["a", "b"], "current": "a", "stage": "b", "value": 0.0 }
    }))
    .unwrap();
    assert_eq!(both.message, "Loading a");
    assert_eq!(both.ratio, ProgressField::Set(0.0));
}

/// `client.ts:101-106`: a stage outside `stages` (or no `stages`) falls back to the bare clamped
/// value; no value means no ratio; an empty stage name reads as no stage.
#[test]
fn load_progress_falls_back_to_the_bare_clamped_value() {
    let unknown_stage = parse_load_progress(&json!({
        "progress": { "stages": ["a"], "current": "zzz", "value": 7.0 }
    }))
    .unwrap();
    assert_eq!(unknown_stage.message, "Loading zzz");
    assert_eq!(unknown_stage.ratio, ProgressField::Set(1.0));
    let no_stages = parse_load_progress(&json!({ "progress": { "value": -3.0 } })).unwrap();
    assert_eq!(no_stages.message, "Loading model");
    assert_eq!(no_stages.ratio, ProgressField::Set(0.0));
    let bare = parse_load_progress(&json!({ "progress": { "current": "x" } })).unwrap();
    assert_eq!(bare.ratio, ProgressField::Clear);
    let empty_stage = parse_load_progress(
        &json!({ "progress": { "current": "", "stages": [""], "value": 0.5 } }),
    )
    .unwrap();
    assert_eq!(empty_stage.message, "Loading model");
    assert_eq!(empty_stage.ratio, ProgressField::Set(0.5));
    // Stage value without `value`: index / stages.
    let no_value = parse_load_progress(
        &json!({ "progress": { "current": "b", "stages": ["a", "b", "c", "d"] } }),
    )
    .unwrap();
    assert_eq!(no_value.ratio, ProgressField::Set(0.25));
}

#[test]
fn load_progress_needs_an_object_progress() {
    assert!(parse_load_progress(&json!({ "status": "loaded" })).is_none());
    assert!(parse_load_progress(&json!({ "progress": "50%" })).is_none());
    assert!(parse_load_progress(&json!("loading")).is_none());
    assert!(parse_load_progress(&Value::Null).is_none());
}

/// `client.ts:113-132`: nested `{progress: {file: {done,total}}}` and flat `{file: {done,total}}`
/// both sum across files; entries without numeric done/total are skipped; no total, no progress.
#[test]
fn download_progress_sums_files_in_nested_and_flat_shapes() {
    let nested = parse_download_progress(&json!({
        "progress": {
            "a.gguf": { "done": 512, "total": 1024 },
            "b.gguf": { "done": 512, "total": 1024 },
            "junk": { "done": "x", "total": 5 },
            "other": 7
        }
    }))
    .unwrap();
    assert_eq!(nested.message, "Downloading model");
    assert_eq!(nested.ratio, ProgressField::Set(0.5));
    assert_eq!(
        nested.detail,
        ProgressField::Set("1.00 KiB / 2.00 KiB".to_string())
    );
    let flat = parse_download_progress(&json!({
        "a.gguf": { "done": 1, "total": 4 },
    }))
    .unwrap();
    assert_eq!(flat.ratio, ProgressField::Set(0.25));
    assert_eq!(flat.detail, ProgressField::Set("1 B / 4 B".to_string()));
    assert!(parse_download_progress(&json!({ "a": { "done": 0, "total": 0 } })).is_none());
    assert!(parse_download_progress(&json!({})).is_none());
    assert!(parse_download_progress(&json!("x")).is_none());
}

// ----------------------------------------------------------------------------------- requests --

#[tokio::test]
async fn list_reads_the_catalog_and_reload_adds_the_query() {
    let server = FakeLlamaServer::with_models(vec![
        model_with(
            "a",
            "loaded",
            json!({
                "aliases": ["alpha"],
                "source": "cache",
                "architecture": { "input_modalities": ["text", "image"] },
                "meta": { "n_ctx": 4096, "n_ctx_train": 32768, "size": 1000, "ftype": "Q4_K_M" }
            }),
        ),
        model_with_status(
            "b",
            "downloading",
            json!({ "progress": { "f": { "done": 1, "total": 2 }, "bad": { "done": "x" } }, "args": ["-m", "x"] }),
        ),
        model_with_status("c", "unloaded", json!({ "failed": true, "exit_code": 3 })),
    ])
    .await;
    let client = client(&server);
    let models = client.list(false, &never()).await.unwrap();
    assert_eq!(models.len(), 3);
    let a = &models[0];
    assert_eq!(a.id, "a");
    assert_eq!(a.status.value, LlamaModelStatus::Loaded);
    assert_eq!(a.aliases, Some(vec!["alpha".to_string()]));
    assert_eq!(a.source.as_deref(), Some("cache"));
    assert_eq!(
        a.architecture.as_ref().unwrap().input_modalities,
        Some(vec!["text".to_string(), "image".to_string()])
    );
    let meta = a.meta.as_ref().unwrap();
    assert_eq!(
        (
            meta.n_ctx,
            meta.n_ctx_train,
            meta.size,
            meta.ftype.as_deref()
        ),
        (Some(4096), Some(32768), Some(1000), Some("Q4_K_M"))
    );
    let b = &models[1];
    assert_eq!(b.status.value, LlamaModelStatus::Downloading);
    assert_eq!(b.status.args, Some(vec!["-m".to_string(), "x".to_string()]));
    let progress = b.status.progress.as_ref().unwrap();
    assert_eq!(
        progress.len(),
        1,
        "the entry without numeric done/total is dropped"
    );
    assert_eq!(progress["f"].done, 1.0);
    assert_eq!(models[2].status.failed, Some(true));
    assert_eq!(models[2].status.exit_code, Some(json!(3)));

    client.list(true, &never()).await.unwrap();
    let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
    assert_eq!(targets, ["/models", "/models?reload=1"]);
}

/// An unknown status string is carried, not rejected (`isModelInfo` only wants a string); a
/// wrongly typed optional field is dropped instead of failing the catalog.
#[tokio::test]
async fn list_tolerates_unknown_statuses_and_odd_optional_fields() {
    let server = FakeLlamaServer::with_models(vec![json!({
        "id": "m",
        "status": { "value": "hibernating", "failed": "yes", "exit_code": "x", "args": 5 },
        "aliases": "nope",
        "meta": { "n_ctx": "big", "size": 12 },
        "extra": true
    })])
    .await;
    let models = client(&server).list(false, &never()).await.unwrap();
    assert_eq!(
        models[0].status.value,
        LlamaModelStatus::Other("hibernating".to_string())
    );
    assert_eq!(models[0].status.failed, None);
    // Only an absent key is "no exit code" (`entry?.status.exit_code === undefined`,
    // `client.ts:291`): a string is a code like any other.
    assert_eq!(models[0].status.exit_code, Some(json!("x")));
    assert_eq!(models[0].status.args, None);
    assert_eq!(models[0].aliases, None);
    assert_eq!(models[0].meta.as_ref().unwrap().n_ctx, None);
    assert_eq!(models[0].meta.as_ref().unwrap().size, Some(12));
}

/// `client.ts:189-193`.
#[tokio::test]
async fn list_validates_the_catalog() {
    let server = FakeLlamaServer::start().await;
    let client = client(&server);
    for body in [
        Reply::Json(200, json!({ "models": [] })),
        Reply::Json(200, json!({ "data": "no" })),
        Reply::Json(200, json!([{ "id": "x", "status": { "value": "loaded" } }])),
        Reply::Json(200, Value::Null),
        Reply::Raw(200, "not json".to_string()),
        Reply::Raw(200, String::new()),
    ] {
        server.clear_overrides();
        server.respond("GET", "/models", body.clone());
        let error = client.list(false, &never()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "llama.cpp returned an invalid model catalog",
            "{body:?}"
        );
    }
}

/// `client.ts:193`: an entry without `id` or `status.value` means a plain llama-server.
#[tokio::test]
async fn list_rejects_a_server_that_is_not_in_router_mode() {
    let server = FakeLlamaServer::start().await;
    let client = client(&server);
    for data in [
        json!([{ "id": "plain.gguf", "object": "model" }]),
        json!([{ "id": "a", "status": { "value": "loaded" } }, { "status": { "value": "loaded" } }]),
        json!([{ "id": "a", "status": "loaded" }]),
        json!([{ "id": 7, "status": { "value": "loaded" } }]),
        json!([{ "id": "a", "status": { "value": 1 } }]),
        json!([null]),
    ] {
        server.clear_overrides();
        server.respond("GET", "/models", Reply::Json(200, json!({ "data": data })));
        let error = client.list(false, &never()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Server is not running in llama.cpp router mode",
            "{data}"
        );
    }
    server.clear_overrides();
    server.respond("GET", "/models", Reply::Json(200, json!({ "data": [] })));
    assert!(client.list(false, &never()).await.unwrap().is_empty());
}

/// `client.ts:172-173`, `:183`.
#[tokio::test]
async fn requests_send_a_bearer_key_and_json_content_type() {
    let server = FakeLlamaServer::with_models(vec![model("m", "unloaded")]).await;
    server.require_bearer("sekret");
    let keyed = keyed_client(&server, "sekret");
    keyed.list(false, &never()).await.unwrap();
    keyed.load("m", &never()).await.unwrap();
    let requests = server.requests();
    assert_eq!(requests[0].header("authorization"), Some("Bearer sekret"));
    assert_eq!(
        requests[0].header("content-type"),
        None,
        "a GET has no body"
    );
    assert_eq!(requests[1].header("authorization"), Some("Bearer sekret"));
    assert_eq!(requests[1].header("content-type"), Some("application/json"));

    let error = client(&server).list(false, &never()).await.unwrap_err();
    assert_eq!(error.to_string(), "Invalid API Key");
    let error = keyed_client(&server, "wrong")
        .list(false, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Invalid API Key");
}

#[tokio::test]
async fn an_empty_api_key_sends_no_authorization_header() {
    let server = FakeLlamaServer::start().await;
    let client = LlamaClient::with_http_client(server.url(), Some(String::new()), http()).unwrap();
    client.list(false, &never()).await.unwrap();
    assert_eq!(server.requests()[0].header("authorization"), None);
}

/// `client.ts:183`, `:48-54`.
#[tokio::test]
async fn http_errors_use_the_server_message_or_the_status_fallback() {
    let server = FakeLlamaServer::start().await;
    let client = client(&server);
    for (reply, expected) in [
        (
            Reply::Json(400, json!({ "error": { "message": "model not found" } })),
            "model not found",
        ),
        (
            Reply::Raw(503, "<html>down</html>".to_string()),
            "llama.cpp returned HTTP 503",
        ),
        (
            Reply::Raw(500, String::new()),
            "llama.cpp returned HTTP 500",
        ),
        (
            Reply::Json(500, json!({ "error": { "message": "" } })),
            "llama.cpp returned HTTP 500",
        ),
        (
            Reply::Json(500, json!({ "error": "boom" })),
            "llama.cpp returned HTTP 500",
        ),
        (
            Reply::Json(404, json!({ "error": { "message": 7 } })),
            "llama.cpp returned HTTP 404",
        ),
    ] {
        server.clear_overrides();
        server.respond("GET", "/models", reply);
        let error = client.list(false, &never()).await.unwrap_err();
        assert_eq!(error, LlamaError::Message(expected.to_string()));
    }
    // A POST fails the same way.
    server.respond(
        "POST",
        "/models/load",
        Reply::Json(500, json!({ "error": { "message": "no memory" } })),
    );
    assert_eq!(
        client.load("m", &never()).await.unwrap_err().to_string(),
        "no memory"
    );
}

/// A connection that fails reads as `fetch failed` (`index.ts:14` classifies connection errors by
/// that text). The peer accepts and hangs up without a word, so the failure does not depend on
/// which ports are free while other tests run.
#[tokio::test]
async fn a_connection_that_fails_reads_as_fetch_failed() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hang_up = tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            drop(socket);
        }
    });
    let client = LlamaClient::with_http_client(&url, None, http()).unwrap();
    let error = client.list(false, &never()).await.unwrap_err();
    hang_up.abort();
    assert!(matches!(error, LlamaError::Transport(_)), "{error:?}");
    assert!(
        error.to_string().contains("fetch failed"),
        "index.ts:14 classifies connection errors by this text: {error}"
    );
    assert!(
        !error.to_string().contains(&url),
        "the request URL stays out of the message: {error}"
    );
}

#[tokio::test]
async fn props_sends_model_and_autoload_false_and_keeps_only_typed_fields() {
    let server = FakeLlamaServer::start().await;
    server.set_props(json!({
        "models_autoload": false,
        "chat_template": "{{ x }}",
        "default_generation_settings": {}
    }));
    let client = client(&server);
    let props = client.props(Some("owner/repo:Q4"), &never()).await.unwrap();
    assert_eq!(props.models_autoload, Some(false));
    assert_eq!(props.chat_template.as_deref(), Some("{{ x }}"));
    assert_eq!(
        server.requests()[0].target,
        "/props?model=owner%2Frepo%3AQ4&autoload=false"
    );

    client.props(None, &never()).await.unwrap();
    assert_eq!(server.requests()[1].target, "/props");

    server.set_props(json!({ "models_autoload": "yes", "chat_template": 5 }));
    let props = client.props(None, &never()).await.unwrap();
    assert_eq!((props.models_autoload, props.chat_template), (None, None));

    server.set_props(json!(null));
    assert_eq!(
        client.props(None, &never()).await.unwrap(),
        crate::client::LlamaServerProps::default()
    );
    server.respond("GET", "/props", Reply::Raw(200, "garbage".to_string()));
    assert_eq!(
        client.props(None, &never()).await.unwrap(),
        crate::client::LlamaServerProps::default()
    );
}

/// `client.ts:208-227`: the three POSTs carry `{"model": id}` to their own paths.
#[tokio::test]
async fn load_unload_and_download_post_the_model_id() {
    let server = FakeLlamaServer::start().await;
    let client = client(&server);
    client.load("a/b:Q4", &never()).await.unwrap();
    client.unload("a/b:Q4", &never()).await.unwrap();
    client.download("a/b:Q4", &never()).await.unwrap();
    let requests = server.requests();
    let seen: Vec<(&str, &str)> = requests
        .iter()
        .map(|r| (r.method.as_str(), r.target.as_str()))
        .collect();
    assert_eq!(
        seen,
        [
            ("POST", "/models/load"),
            ("POST", "/models/unload"),
            ("POST", "/models")
        ]
    );
    for request in &requests {
        assert_eq!(request.json(), json!({ "model": "a/b:Q4" }));
    }
}

// ------------------------------------------------------------------- timeout / cancellation --

/// `client.ts:174`: a request that never answers fails with a timeout, not a hang.
#[tokio::test]
async fn a_request_that_never_answers_times_out() {
    let server = FakeLlamaServer::start().await;
    server.respond("GET", "/models", Reply::Hang);
    let client = client(&server).with_request_timeout(Duration::from_millis(200));
    let started = Instant::now();
    let error = client.list(false, &never()).await.unwrap_err();
    assert_eq!(error, LlamaError::Timeout);
    assert!(error.to_string().contains("timeout"), "index.ts:14");
    assert!(started.elapsed() < Duration::from_secs(5));
}

/// The 15 s default is in force unless overridden (`client.ts:174`).
#[tokio::test]
async fn the_default_timeout_is_fifteen_seconds() {
    let server = FakeLlamaServer::start().await;
    let default = client(&server);
    assert_eq!(default.request_timeout(), Duration::from_secs(15));
    assert_eq!(
        default
            .with_request_timeout(Duration::from_millis(5))
            .request_timeout(),
        Duration::from_millis(5)
    );
}

#[tokio::test]
async fn cancelling_aborts_an_in_flight_request() {
    let server = FakeLlamaServer::start().await;
    server.respond("GET", "/models", Reply::Hang);
    let client = client(&server);
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let error = client.list(false, &cancel).await.unwrap_err();
    assert_eq!(error, LlamaError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test]
async fn an_already_cancelled_token_sends_nothing() {
    let server = FakeLlamaServer::start().await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client(&server).list(false, &cancel).await.unwrap_err(),
        LlamaError::Cancelled
    );
    assert_eq!(
        client(&server).load("m", &cancel).await.unwrap_err(),
        LlamaError::Cancelled
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(server.requests().is_empty());
}

// ------------------------------------------------------------------------------- unload_and_wait --

/// `client.ts:216-223`: unload, then poll until the entry is unloaded.
#[tokio::test]
async fn unload_and_wait_polls_until_unloaded() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loaded")]).await;
    // The transition waits for the client's SECOND catalog read, so "it polled until the model was
    // unloaded" holds whatever the machine's speed.
    server.on_unload(
        "m",
        vec![
            Step::SetModel(model("m", "loaded")),
            Step::WaitForRequests {
                method: "GET",
                path: "/models",
                count: 2,
            },
            Step::SetModel(model("m", "unloaded")),
        ],
    );
    client(&server)
        .unload_and_wait("m", &never())
        .await
        .unwrap();
    let polls = server.requests_to("GET", "/models").len();
    assert!(
        polls >= 2,
        "polled {polls} times: waited for the transition"
    );
    assert_eq!(server.requests_to("POST", "/models/unload").len(), 1);
}

/// `client.ts:220`: a model that left the catalog counts as unloaded.
#[tokio::test]
async fn unload_and_wait_returns_when_the_model_is_gone() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loaded")]).await;
    server.on_unload("m", vec![Step::RemoveModel("m".to_string())]);
    client(&server)
        .unload_and_wait("m", &never())
        .await
        .unwrap();
}

#[tokio::test]
async fn unload_and_wait_can_be_cancelled() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loaded")]).await;
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        trigger.cancel();
    });
    let error = client(&server)
        .unload_and_wait("m", &cancel)
        .await
        .unwrap_err();
    assert_eq!(error, LlamaError::Cancelled);
}

// -------------------------------------------------------------------------------------- watch --

/// Collect events from `watch` until the server closes the stream.
async fn watch_all(
    server: &FakeLlamaServer,
    steps: Vec<Step>,
) -> Vec<(String, String, Option<Value>)> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let client = client(server);
    let cancel = never();
    let watching = tokio::spawn({
        let cancel = cancel.clone();
        async move {
            client
                .watch(
                    move |event| {
                        sink.lock()
                            .unwrap()
                            .push((event.model, event.event, event.data))
                    },
                    &cancel,
                )
                .await
        }
    });
    server.wait_for_sse(1).await;
    server.run(steps);
    watching.await.unwrap().unwrap();
    seen.lock().unwrap().clone()
}

/// `client.ts:240-258`: frames are blank-line delimited, `\r\n` is normalised, multi-line `data:`
/// joins with `\n`, whitespace after `data:` is trimmed, non-`data:` lines are ignored, malformed
/// or incomplete events are dropped, and a frame split across writes is reassembled.
#[tokio::test]
async fn watch_decodes_frames_and_ignores_malformed_events() {
    let server = FakeLlamaServer::start().await;
    let raw = |text: &str| Step::SseRaw(text.to_string());
    let events = watch_all(
        &server,
        vec![
            // Plain frame, no space after `data:`.
            raw(&format!(
                "data:{}\n\n",
                event("m", "status_change", json!({ "status": "loading" }))
            )),
            // Comment + event-name + id lines are not `data:` lines.
            raw(&format!(
                ": ping\nevent: x\nid: 4\ndata: {}\n\n",
                event("m", "download_progress", json!({ "k": 1 }))
            )),
            // CRLF framing.
            raw("data: {\"model\":\"crlf\",\"event\":\"e\"}\r\n\r\n"),
            // JSON spread over two `data:` lines.
            raw("data: {\"model\":\"multi\",\n"),
            raw("data: \"event\":\"e\"}\n\n"),
            // The lines join with `\n`, not nothing: `1` + `2` is `1\n2`, invalid JSON, so the
            // frame is dropped (joined with "" it would read as the number 12 and be delivered).
            raw("data: {\"model\":\"joined\",\"event\":\"e\",\"data\":{\"n\":1\n"),
            raw("data: 2}}\n\n"),
            // A frame split mid-line across two writes.
            raw("data: {\"model\":\"split\",\"ev"),
            Step::delay_ms(30),
            raw("ent\":\"e\",\"data\":{\"n\":2}}\n\n"),
            // Malformed JSON, missing fields, wrong types, non-object JSON, empty data.
            raw("data: {not json}\n\n"),
            raw("data: {\"model\":\"x\"}\n\n"),
            raw("data: {\"event\":\"x\"}\n\n"),
            raw("data: {\"model\":1,\"event\":\"x\"}\n\n"),
            raw("data: [1,2]\n\n"),
            raw("data: null\n\n"),
            raw("data:\n\n"),
            raw("retry: 10\n\n"),
            // Still decoding after the garbage.
            raw("data: {\"model\":\"last\",\"event\":\"e\"}\n\n"),
            Step::CloseSse,
        ],
    )
    .await;
    let names: Vec<&str> = events.iter().map(|(model, _, _)| model.as_str()).collect();
    assert_eq!(names, ["m", "m", "crlf", "multi", "split", "last"]);
    assert_eq!(events[0].2, Some(json!({ "status": "loading" })));
    assert_eq!(events[1].1, "download_progress");
    assert_eq!(events[2].2, None, "no data field means no data");
    assert_eq!(events[4].2, Some(json!({ "n": 2 })));
}

/// A multi-byte UTF-8 character split across two chunks is decoded whole (`TextDecoder` with
/// `stream: true`, `client.ts:240`).
#[tokio::test]
async fn watch_reassembles_a_multibyte_character_split_across_chunks() {
    let server = FakeLlamaServer::start().await;
    let frame = "data: {\"model\":\"caf\u{e9}-\u{1f980}\",\"event\":\"e\"}\n\n".as_bytes();
    // Cut inside the 2-byte e-acute and again inside the 4-byte crab.
    let acute = frame.iter().position(|b| *b == 0xC3).unwrap();
    let crab = frame.iter().position(|b| *b == 0xF0).unwrap();
    let events = watch_all(
        &server,
        vec![
            Step::SseBytes(frame[..=acute].to_vec()),
            Step::delay_ms(40),
            Step::SseBytes(frame[acute + 1..=crab + 1].to_vec()),
            Step::delay_ms(40),
            Step::SseBytes(frame[crab + 2..].to_vec()),
            Step::CloseSse,
        ],
    )
    .await;
    assert_eq!(events[0].0, "caf\u{e9}-\u{1f980}");
}

#[tokio::test]
async fn watch_sends_the_bearer_key_and_fails_on_an_http_error() {
    let server = FakeLlamaServer::start().await;
    server.require_bearer("k");
    let cancel = never();
    // Authorised: connects (and we stop it).
    let keyed = keyed_client(&server, "k");
    let task = {
        let cancel = cancel.clone();
        tokio::spawn(async move { keyed.watch(|_| {}, &cancel).await })
    };
    server.wait_for_sse(1).await;
    assert_eq!(
        server.requests_to("GET", "/models/sse")[0].header("authorization"),
        Some("Bearer k")
    );
    cancel.cancel();
    assert_eq!(task.await.unwrap().unwrap_err(), LlamaError::Cancelled);

    // Unauthorised and server-error answers fail with the SSE message.
    let error = client(&server).watch(|_| {}, &never()).await.unwrap_err();
    assert_eq!(error.to_string(), "llama.cpp SSE returned HTTP 401");
    server.respond("GET", "/models/sse", Reply::Raw(500, String::new()));
    let error = keyed_client(&server, "k")
        .watch(|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "llama.cpp SSE returned HTTP 500");
}

// -------------------------------------------------------------------------------- load_and_wait --

/// `llama-extension.test.ts:486-527`: SSE stage progress, then `loaded`, with the catalog also
/// flipping to loaded.
#[tokio::test]
async fn loads_with_sse_progress_and_waits_for_the_loaded_catalog_state() {
    let server = FakeLlamaServer::with_models(vec![model("test-model", "unloaded")]).await;
    server.on_load(
        "test-model",
        vec![
            Step::WaitForSse(1),
            Step::SetModel(model("test-model", "loading")),
            Step::delay_ms(20),
            Step::Sse(event(
                "test-model",
                "status_change",
                json!({
                    "status": "loading",
                    "progress": { "stages": ["text_model", "mmproj_model"], "current": "text_model", "value": 0.5 }
                }),
            )),
            Step::SetModel(model("test-model", "loaded")),
            Step::Sse(event("test-model", "status_change", json!({ "status": "loaded" }))),
        ],
    );
    let (seen, on_progress) = collector();
    let loaded = client(&server)
        .load_and_wait("test-model", &on_progress, &never())
        .await
        .unwrap();
    assert_eq!(loaded.status.value, LlamaModelStatus::Loaded);
    assert_eq!(loaded.id, "test-model");
    let seen = seen.lock().unwrap().clone();
    let staged = seen
        .iter()
        .find(|p| p.message == "Loading text model")
        .unwrap_or_else(|| panic!("no stage progress in {seen:?}"));
    assert_eq!(
        staged.ratio,
        ProgressField::Set(0.25),
        "(0 + 0.5) / 2 stages"
    );
    assert_eq!(
        seen[0],
        LlamaProgress::message("Loading model"),
        "client.ts:283"
    );
    assert_eq!(
        server.requests_to("POST", "/models/load")[0].json(),
        json!({ "model": "test-model" })
    );
}

/// Upstream swallows a failed SSE connection (`client.ts:280`); polling alone finishes the load.
#[tokio::test]
async fn a_failed_sse_stream_falls_back_to_catalog_polling() {
    let server = FakeLlamaServer::with_models(vec![model("m", "unloaded")]).await;
    server.respond("GET", "/models/sse", Reply::Raw(500, "boom".to_string()));
    server.on_load(
        "m",
        vec![
            Step::SetModel(model("m", "loading")),
            Step::delay_ms(300),
            Step::SetModel(model("m", "loaded")),
        ],
    );
    let (seen, on_progress) = collector();
    let loaded = client(&server)
        .load_and_wait("m", &on_progress, &never())
        .await
        .unwrap();
    assert_eq!(loaded.status.value, LlamaModelStatus::Loaded);
    assert!(
        server.requests_to("GET", "/models/sse").len() == 1,
        "the SSE was attempted"
    );
    assert!(
        server.requests_to("GET", "/models").len() >= 2,
        "and polling carried the load"
    );
    assert_eq!(
        seen.lock().unwrap().len(),
        1,
        "no SSE progress, only the initial message"
    );
}

/// An SSE stream that ends early (server closes it) is likewise survivable.
#[tokio::test]
async fn an_sse_stream_that_closes_early_still_lets_polling_finish() {
    let server = FakeLlamaServer::with_models(vec![model("m", "unloaded")]).await;
    server.on_load(
        "m",
        vec![
            Step::CloseSse,
            Step::delay_ms(100),
            Step::SetModel(model("m", "loaded")),
        ],
    );
    let loaded = client(&server)
        .load_and_wait("m", &|_| {}, &never())
        .await
        .unwrap();
    assert_eq!(loaded.status.value, LlamaModelStatus::Loaded);
}

/// `client.ts:288`: SSE reports `loaded` while the catalog does not list the model (a preset
/// the router has not rescanned): synthesize the loaded entry.
#[tokio::test]
async fn an_sse_loaded_event_for_a_model_missing_from_the_catalog_returns_a_synthetic_entry() {
    let server = FakeLlamaServer::start().await;
    server.on_load(
        "ghost",
        vec![
            Step::WaitForSse(1),
            Step::Sse(event(
                "ghost",
                "model_status",
                json!({ "status": "loaded" }),
            )),
        ],
    );
    let loaded = client(&server)
        .load_and_wait("ghost", &|_| {}, &never())
        .await
        .unwrap();
    assert_eq!(loaded, entry("ghost", "loaded"));
}

/// `client.ts:274`: only this model's `model_status` / `status_change` events count.
#[tokio::test]
async fn load_ignores_events_for_other_models_and_other_event_types() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loading")]).await;
    server.on_load(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::Sse(event(
                "other",
                "status_change",
                json!({ "status": "unloaded" }),
            )),
            Step::Sse(event(
                "m",
                "download_progress",
                json!({ "status": "unloaded" }),
            )),
            Step::Sse(event("m", "mystery", json!({ "status": "unloaded" }))),
            Step::delay_ms(400),
            Step::SetModel(model("m", "loaded")),
        ],
    );
    let loaded = client(&server)
        .load_and_wait("m", &|_| {}, &never())
        .await
        .unwrap();
    assert_eq!(loaded.status.value, LlamaModelStatus::Loaded);
}

/// `client.ts:289-294`.
#[tokio::test]
async fn load_fails_when_the_entry_reports_failed() {
    let server = FakeLlamaServer::with_models(vec![model("m", "unloaded")]).await;
    server.on_load(
        "m",
        vec![Step::SetModel(model_with_status(
            "m",
            "unloaded",
            json!({ "failed": true, "exit_code": 3 }),
        ))],
    );
    let error = client(&server)
        .load_and_wait("m", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Model exited with code 3");

    server.on_load(
        "n",
        vec![Step::SetModel(model_with_status(
            "n",
            "unloaded",
            json!({ "failed": true }),
        ))],
    );
    server.set_model(model("n", "unloaded"));
    let error = client(&server)
        .load_and_wait("n", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Model failed to load");

    // exit_code 0 is a code, not an absence (`=== undefined`).
    server.on_load(
        "z",
        vec![Step::SetModel(model_with_status(
            "z",
            "unloaded",
            json!({ "failed": true, "exit_code": 0 }),
        ))],
    );
    let error = client(&server)
        .load_and_wait("z", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Model exited with code 0");
}

/// `client.ts:277`: an SSE `unloaded` status during a load is a failure.
#[tokio::test]
async fn load_fails_when_sse_reports_unloaded() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loading")]).await;
    server.on_load(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::Sse(event("m", "status_change", json!({ "status": "unloaded" }))),
        ],
    );
    let error = client(&server)
        .load_and_wait("m", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Model failed to load");
}

/// `client.ts:291-293`: the entry's exit code outranks the SSE message.
#[tokio::test]
async fn an_exit_code_outranks_the_sse_failure_message() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loading")]).await;
    server.on_load(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::SetModel(model_with_status(
                "m",
                "unloaded",
                json!({ "exit_code": 139 }),
            )),
            Step::Sse(event("m", "status_change", json!({ "status": "unloaded" }))),
        ],
    );
    let error = client(&server)
        .load_and_wait("m", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Model exited with code 139");
}

#[tokio::test]
async fn load_can_be_cancelled_mid_wait() {
    let server = FakeLlamaServer::with_models(vec![model("m", "loading")]).await;
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        trigger.cancel();
    });
    let started = Instant::now();
    let error = client(&server)
        .load_and_wait("m", &|_| {}, &cancel)
        .await
        .unwrap_err();
    assert_eq!(error, LlamaError::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(server.requests_to("POST", "/models/load").len(), 1);
}

#[tokio::test]
async fn load_surfaces_a_failed_load_request() {
    let server = FakeLlamaServer::with_models(vec![model("m", "unloaded")]).await;
    server.respond(
        "POST",
        "/models/load",
        Reply::Json(400, json!({ "error": { "message": "model not found" } })),
    );
    let error = client(&server)
        .load_and_wait("m", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "model not found");
}

// ----------------------------------------------------------------------------- download_and_wait --

/// `llama-extension.test.ts:529-573`.
#[tokio::test]
async fn downloads_with_byte_progress_and_returns_the_refreshed_catalog() {
    let id = "owner/repo:Q4_K_M";
    let server = FakeLlamaServer::start().await;
    server.on_download(
        id,
        vec![
            Step::WaitForSse(1),
            Step::SetModel(model(id, "downloading")),
            Step::delay_ms(20),
            Step::Sse(event(
                id,
                "download_progress",
                json!({ "progress": { "https://example/model.gguf": { "done": 512, "total": 1024 } } }),
            )),
            Step::SetModel(model(id, "unloaded")),
            Step::Sse(event(id, "download_finished", json!({}))),
        ],
    );
    let (seen, on_progress) = collector();
    let models = client(&server)
        .download_and_wait(id, &on_progress, &never())
        .await
        .unwrap();
    assert_eq!(models, vec![entry(id, "unloaded")]);
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.contains(&LlamaProgress {
            message: "Downloading model".to_string(),
            ratio: ProgressField::Set(0.5),
            detail: ProgressField::Set("512 B / 1.00 KiB".to_string()),
        }),
        "{seen:?}"
    );
    assert_eq!(
        seen[0],
        LlamaProgress::message("Downloading model"),
        "client.ts:327"
    );
    // The result is the `?reload=1` read (`client.ts:339`).
    let last = server
        .requests()
        .into_iter()
        .rev()
        .find(|r| r.path() == "/models" && r.method == "GET")
        .unwrap();
    assert_eq!(last.target, "/models?reload=1");
    assert_eq!(
        server.requests_to("POST", "/models")[0].json(),
        json!({ "model": id })
    );
}

/// `client.ts:318`: a `download_failed` event fails the wait with its message, or `Download
/// failed`.
#[tokio::test]
async fn download_fails_on_a_download_failed_event() {
    let server = FakeLlamaServer::start().await;
    server.on_download(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::SetModel(model("m", "downloading")),
            Step::Sse(event(
                "m",
                "download_failed",
                json!({ "error": { "message": "disk full" } }),
            )),
        ],
    );
    let error = client(&server)
        .download_and_wait("m", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "disk full");

    server.on_download(
        "n",
        vec![
            Step::WaitForSse(1),
            Step::SetModel(model("n", "downloading")),
            Step::Sse(event("n", "download_failed", json!({}))),
        ],
    );
    let error = client(&server)
        .download_and_wait("n", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Download failed");
}

/// `client.ts:317`: `download_finished` ends the wait even while the catalog still says
/// `downloading` was never observed and the entry is absent.
#[tokio::test]
async fn download_finished_ends_the_wait_without_a_catalog_entry() {
    let server = FakeLlamaServer::start().await;
    server.on_download(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::Sse(event("m", "download_finished", json!({}))),
        ],
    );
    // No entry ever appears, so only the `finished` flag can end the wait (`client.ts:338`).
    let models = tokio::time::timeout(
        Duration::from_secs(4),
        client(&server).download_and_wait("m", &|_| {}, &never()),
    )
    .await
    .expect("finished ends the wait")
    .unwrap();
    assert!(models.is_empty());
    let polls = server.requests_to("GET", "/models");
    assert_eq!(
        polls.last().map(|r| r.target.as_str()),
        Some("/models?reload=1")
    );
}

/// `client.ts:334-338`: with no SSE at all, a `downloading` entry reports bytes and the wait
/// continues, then finishes when the entry stops downloading after having been seen downloading.
#[tokio::test]
async fn download_polling_reports_entry_progress_and_finishes_after_it_was_seen_downloading() {
    let server = FakeLlamaServer::start().await;
    server.respond("GET", "/models/sse", Reply::Raw(500, String::new()));
    server.on_download(
        "m",
        vec![
            Step::SetModel(model_with_status(
                "m",
                "downloading",
                json!({ "progress": { "a": { "done": 100, "total": 400 }, "b": { "done": 100, "total": 400 } } }),
            )),
            Step::delay_ms(700),
            Step::SetModel(model("m", "unloaded")),
        ],
    );
    let (seen, on_progress) = collector();
    let models = client(&server)
        .download_and_wait("m", &on_progress, &never())
        .await
        .unwrap();
    assert_eq!(models, vec![entry("m", "unloaded")]);
    let seen = seen.lock().unwrap().clone();
    assert!(
        seen.iter().any(|p| p.ratio == ProgressField::Set(0.25)
            && p.detail == ProgressField::Set("200 B / 800 B".to_string())),
        "{seen:?}"
    );
}

/// `client.ts:338`: a cached model never shows `downloading`; with the entry present and no
/// events, the wait ends at the second poll, not the first.
#[tokio::test]
async fn download_of_a_cached_model_finishes_on_the_second_poll() {
    let server = FakeLlamaServer::with_models(vec![model("cached", "unloaded")]).await;
    server.respond("GET", "/models/sse", Reply::Raw(500, String::new()));
    let models = client(&server)
        .download_and_wait("cached", &|_| {}, &never())
        .await
        .unwrap();
    assert_eq!(models, vec![entry("cached", "unloaded")]);
    let polls: Vec<String> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.path() == "/models")
        .map(|r| r.target)
        .collect();
    assert_eq!(polls, ["/models", "/models", "/models?reload=1"]);
}

/// Without an entry and without events the wait goes on (and can be cancelled).
#[tokio::test]
async fn download_keeps_waiting_while_the_model_is_absent_and_can_be_cancelled() {
    let server = FakeLlamaServer::start().await;
    let cancel = CancellationToken::new();
    // Cancel once the client has read the catalog three times: ordered by what it has asked, not
    // by the clock.
    let client = client(&server);
    let (result, ()) = tokio::join!(client.download_and_wait("m", &|_| {}, &cancel), async {
        server.wait_for_requests("GET", "/models", 3).await;
        cancel.cancel();
    });
    let error = result.unwrap_err();
    assert_eq!(error, LlamaError::Cancelled);
    assert!(
        server.requests_to("GET", "/models").len() >= 3,
        "kept polling"
    );
}

/// `client.ts:330`: a failure observed before the next poll wins over a catalog entry that would
/// otherwise end the wait.
#[tokio::test]
async fn download_failure_is_checked_before_the_catalog_poll() {
    // The entry already exists, so the second poll would end the wait successfully; the failure
    // event arrives in between and must win (`client.ts:330` runs before the poll).
    let server = FakeLlamaServer::with_models(vec![model("m", "unloaded")]).await;
    server.on_download(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::delay_ms(100),
            Step::Sse(event(
                "m",
                "download_failed",
                json!({ "error": { "message": "bad token" } }),
            )),
        ],
    );
    let error = client(&server)
        .download_and_wait("m", &|_| {}, &never())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "bad token");
}

/// `client.ts:319-320`, `:338`: an SSE `download_progress` marks the download as seen, so an
/// entry present at the first poll ends the wait there instead of after two polls. The first
/// catalog read is held for 300 ms so the event lands before its answer is evaluated.
#[tokio::test]
async fn a_download_progress_event_lets_the_first_poll_finish() {
    let server = FakeLlamaServer::start().await;
    // The first catalog read is held until the client reports the progress event: the event is
    // always handled before that read is answered, whatever the machine's speed.
    let progress_seen = Arc::new(tokio::sync::Notify::new());
    server.respond_times(
        "GET",
        "/models",
        Reply::Gated(
            Arc::clone(&progress_seen),
            Box::new(Reply::Json(
                200,
                json!({ "data": [model("m", "unloaded")] }),
            )),
        ),
        1,
    );
    server.set_models(vec![model("m", "unloaded")]);
    server.on_download(
        "m",
        vec![
            Step::WaitForSse(1),
            Step::Sse(event(
                "m",
                "download_progress",
                json!({ "a": { "done": 1, "total": 2 } }),
            )),
        ],
    );
    let (seen, collect) = collector();
    let on_progress = move |progress: LlamaProgress| {
        if progress.ratio == ProgressField::Set(0.5) {
            progress_seen.notify_one();
        }
        collect(progress);
    };
    let models = client(&server)
        .download_and_wait("m", &on_progress, &never())
        .await
        .unwrap();
    assert_eq!(models, vec![entry("m", "unloaded")]);
    let polls: Vec<String> = server
        .requests()
        .into_iter()
        .filter(|r| r.method == "GET" && r.path() == "/models")
        .map(|r| r.target)
        .collect();
    assert_eq!(
        polls,
        ["/models", "/models?reload=1"],
        "one poll and the reload, with no second poll after a 500 ms sleep"
    );
    assert!(
        seen.lock()
            .unwrap()
            .iter()
            .any(|p| p.ratio == ProgressField::Set(0.5))
    );
}

// ------------------------------------------------------------- whole-element, whole-number reads --

/// `configuredContextWindow` (`provider.ts:59-66`) tests every `status.args` element on its own:
/// `args[index] !== flag` and `Number(args[index + 1])`. One odd element does not drop the field
/// and a numeric value still pins the window.
#[test]
fn status_args_keep_every_element_so_a_numeric_context_size_still_pins() {
    let info: LlamaModelInfo = serde_json::from_value(json!({
        "id": "m",
        "status": { "value": "loaded", "args": ["--ctx-size", 4096] }
    }))
    .unwrap();
    assert_eq!(
        info.status.args,
        Some(vec!["--ctx-size".to_string(), "4096".to_string()])
    );
    assert_eq!(crate::model::configured_context_window(&info), Some(4096));

    let odd: LlamaModelInfo = serde_json::from_value(json!({
        "id": "m",
        "status": { "value": "loaded", "args": ["-m", { "x": 1 }, null, "-c", 2048.0, true] }
    }))
    .unwrap();
    assert_eq!(odd.status.args.as_ref().map(Vec::len), Some(6));
    assert_eq!(
        crate::model::configured_context_window(&odd),
        Some(2048),
        "an object and a null among the arguments cost nothing"
    );
}

/// `runtimeContextWindow && runtimeContextWindow > 0` (`provider.ts:70-76`) takes any positive
/// number: `4096.0` and `4e3` are integers, a fraction truncates, anything else is absent.
#[test]
fn counts_read_any_positive_json_number() {
    let meta = |n_ctx: serde_json::Value| -> Option<u64> {
        let info: LlamaModelInfo = serde_json::from_value(json!({
            "id": "m",
            "status": { "value": "loaded" },
            "meta": { "n_ctx": n_ctx }
        }))
        .unwrap();
        info.meta.unwrap().n_ctx
    };
    assert_eq!(meta(json!(4096)), Some(4096));
    assert_eq!(meta(json!(4096.0)), Some(4096));
    assert_eq!(meta(json!(4e3)), Some(4000));
    assert_eq!(meta(json!(1.9)), Some(1));
    assert_eq!(meta(json!(-5)), None);
    assert_eq!(meta(json!(-5.5)), None);
    assert_eq!(meta(json!("4096")), None);
    assert_eq!(meta(json!(null)), None);
}

/// `entry?.status.exit_code === undefined` (`client.ts:291`) is the only "no code": a `null` or a
/// fractional code is a code, and the message prints it as JS would.
#[tokio::test]
async fn any_present_exit_code_is_reported_as_one() {
    let server = FakeLlamaServer::with_models(vec![
        model("a", "unloaded"),
        model("b", "unloaded"),
        model("c", "unloaded"),
    ])
    .await;
    for (id, code) in [("a", json!(null)), ("b", json!(1.5)), ("c", json!(2.0))] {
        server.on_load(
            id,
            vec![Step::SetModel(model_with_status(
                id,
                "unloaded",
                json!({ "failed": true, "exit_code": code }),
            ))],
        );
    }
    let message = |id: &'static str| {
        let client = client(&server);
        async move {
            client
                .load_and_wait(id, &|_| {}, &never())
                .await
                .unwrap_err()
                .to_string()
        }
    };
    assert_eq!(message("a").await, "Model exited with code null");
    assert_eq!(message("b").await, "Model exited with code 1.5");
    assert_eq!(message("c").await, "Model exited with code 2");
}

// ------------------------------------------------------------------------------------------ bom --

/// `new TextDecoder().decode(chunk, { stream: true })` consumes a leading U+FEFF (`ignoreBOM`
/// defaults to false), so a BOM-prefixed first frame is still the `data:` frame it is (`client.ts:240`).
#[tokio::test]
async fn watch_consumes_a_leading_byte_order_mark() {
    let server = FakeLlamaServer::start().await;
    let frame = format!(
        "data: {}\n\n",
        json!({ "model": "m", "event": "status_change", "data": { "status": "loaded" } })
    );
    let mut with_bom = vec![0xEF, 0xBB, 0xBF];
    with_bom.extend_from_slice(frame.as_bytes());
    let events = watch_all(&server, vec![Step::SseBytes(with_bom), Step::CloseSse]).await;
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].0, "m");
}

/// The BOM may itself arrive split across chunks; only the one at the start of the stream goes.
#[tokio::test]
async fn watch_consumes_a_byte_order_mark_split_across_chunks_but_only_the_first() {
    let server = FakeLlamaServer::start().await;
    let frame = |model: &str| {
        format!(
            "data: {}\n\n",
            json!({ "model": model, "event": "status_change", "data": {} })
        )
    };
    let first = vec![0xEF];
    let mut second = vec![0xBB, 0xBF];
    second.extend_from_slice(frame("one").as_bytes());
    // A second U+FEFF mid-stream is data, not a BOM: it spoils the frame it prefixes.
    let mut third = vec![0xEF, 0xBB, 0xBF];
    third.extend_from_slice(frame("two").as_bytes());
    let events = watch_all(
        &server,
        vec![
            Step::SseBytes(first),
            Step::SseBytes(second),
            Step::SseBytes(third),
            Step::SseBytes(frame("three").into_bytes()),
            Step::CloseSse,
        ],
    )
    .await;
    let models: Vec<&str> = events.iter().map(|event| event.0.as_str()).collect();
    assert_eq!(models, ["one", "three"], "{events:?}");
}

// ---------------------------------------------------------------------------------- credentials --

/// Node's `fetch` refuses a URL with credentials, so one never gets as far as being persisted in
/// `LLAMA_BASE_URL` or printed: the error does not repeat it.
#[test]
fn a_server_url_with_credentials_is_refused_without_echoing_them() {
    for url in [
        "http://user:hunter2@host:8080",
        "https://user@host",
        "http://:hunter2@host",
    ] {
        let error = normalize_llama_server_url(url).unwrap_err().to_string();
        assert_eq!(error, "Server URL must not include credentials", "{url}");
        assert!(!error.contains("hunter2"));
    }
    assert!(normalize_llama_server_url("http://host:8080/v1").is_ok());
}

// ----------------------------------------------------------------------------------------- caps --

/// A management response larger than the cap is refused instead of buffered for the length of the
/// timeout.
#[tokio::test]
async fn a_response_body_beyond_the_cap_is_refused() {
    let server = FakeLlamaServer::start().await;
    server.respond(
        "GET",
        "/models",
        Reply::Raw(200, "x".repeat(crate::client::MAX_BODY_BYTES + 1)),
    );
    let error = client(&server).list(false, &never()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "llama.cpp response exceeds {} bytes",
            crate::client::MAX_BODY_BYTES
        )
    );
}

/// An SSE frame that never reaches its blank line is cut off at the cap rather than grown without
/// limit.
#[tokio::test]
async fn an_sse_frame_that_never_ends_is_refused_at_the_cap() {
    let server = FakeLlamaServer::start().await;
    let client = client(&server);
    let cancel = never();
    let watching = tokio::spawn({
        let cancel = cancel.clone();
        async move { client.watch(|_| {}, &cancel).await }
    });
    server.wait_for_sse(1).await;
    server.run(vec![Step::SseRaw(format!(
        "data: {}",
        "a".repeat(1024 * 1024 + 1)
    ))]);
    let outcome = tokio::time::timeout(Duration::from_secs(20), watching)
        .await
        .expect("the watch ends")
        .unwrap();
    let error = outcome.unwrap_err();
    assert!(
        error.to_string().starts_with("llama.cpp SSE frame exceeds"),
        "{error}"
    );
}

// ------------------------------------------------------------------------------------- proxy --

/// The client resolves its proxy the way the classifier's HTTP path does
/// (`build_client_for_target`): the provider env overlay's `HTTP_PROXY` routes the request
/// through it, so the two HTTP paths of one extension agree.
#[tokio::test]
async fn the_client_honours_the_provider_env_overlays_proxy() {
    let proxy = FakeLlamaServer::start().await;
    let env: std::collections::BTreeMap<String, String> = [
        ("HTTP_PROXY".to_string(), proxy.url().to_string()),
        ("NO_PROXY".to_string(), String::new()),
        ("no_proxy".to_string(), String::new()),
    ]
    .into();
    let client = LlamaClient::new("http://llama.invalid:8080", None, Some(&env))
        .await
        .unwrap();
    let _ = client.list(false, &never()).await;
    let targets: Vec<String> = proxy.requests().into_iter().map(|r| r.target).collect();
    assert!(
        targets
            .iter()
            .any(|target| target.starts_with("http://llama.invalid:8080/models")),
        "the proxy must have been asked for the absolute URL: {targets:?}"
    );
}
