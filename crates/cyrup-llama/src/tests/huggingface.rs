//! Tests for the `huggingface` module: port of the Hugging Face cases of pi's
//! `packages/coding-agent/test/llama-extension.test.ts:446-484` plus the token-file precedence,
//! the 429 header forms and the quantization-pattern table the upstream suite leaves implicit.
//!
//! Every request goes to a raw `tokio::net::TcpListener` on `127.0.0.1:0` defined in this file
//! (the technique of `cyrup-provider/src/tests/remote_catalog.rs`); the base URL is the only
//! transport seam, as upstream's `baseUrl` constructor argument is, and the HTTP client is built
//! with `no_proxy()` so an ambient `HTTP_PROXY` cannot reroute a loopback request.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

use crate::error::LlamaError;
use crate::huggingface::{
    HuggingFaceClient, HuggingFaceGated, HuggingFaceModel, HuggingFaceModelDetails,
    HuggingFaceQuantization, find_huggingface_token_with_home, quantization_of_file,
};

// ------------------------------------------------------------------------------ loopback server --

/// One request as the fake server saw it.
#[derive(Debug, Clone)]
struct Seen {
    /// Request target: path plus query, exactly as sent.
    target: String,
    /// Header names lower-cased.
    headers: BTreeMap<String, String>,
}

/// What the fake server answers.
#[derive(Clone)]
enum Reply {
    Json(String),
    Status {
        code: u16,
        headers: Vec<(&'static str, &'static str)>,
        body: String,
    },
    /// Accept the request and never answer.
    Hang,
}

struct FakeHuggingFace {
    url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl FakeHuggingFace {
    async fn spawn(handler: impl Fn(&str) -> Reply + Send + Sync + 'static) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler = Arc::new(handler);
        let recorded = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let handler = handler.clone();
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let mut buffer = Vec::new();
                    let mut chunk = [0u8; 4096];
                    while !buffer.windows(4).any(|window| window == b"\r\n\r\n") {
                        match socket.read(&mut chunk).await {
                            Ok(0) | Err(_) => return,
                            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let head = String::from_utf8_lossy(&buffer).to_string();
                    let mut lines = head.split("\r\n");
                    let target = lines
                        .next()
                        .and_then(|line| line.split(' ').nth(1))
                        .unwrap_or_default()
                        .to_string();
                    let headers = lines
                        .filter_map(|line| line.split_once(':'))
                        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
                        .collect();
                    recorded.lock().unwrap().push(Seen {
                        target: target.clone(),
                        headers,
                    });
                    let (code, headers, body) = match handler(&target) {
                        Reply::Json(body) => {
                            (200, vec![("content-type", "application/json")], body)
                        }
                        Reply::Status {
                            code,
                            headers,
                            body,
                        } => (code, headers, body),
                        Reply::Hang => {
                            tokio::time::sleep(Duration::from_secs(60)).await;
                            return;
                        }
                    };
                    let mut response = format!("HTTP/1.1 {code} X\r\n");
                    for (name, value) in headers {
                        response.push_str(&format!("{name}: {value}\r\n"));
                    }
                    response.push_str(&format!(
                        "content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    ));
                    let _ = socket.write_all(response.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        Self { url, seen }
    }

    fn requests(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn client(&self, token: Option<&str>) -> HuggingFaceClient {
        client_for(&self.url, token)
    }
}

fn client_for(url: &str, token: Option<&str>) -> HuggingFaceClient {
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    HuggingFaceClient::with_http_client(token.map(str::to_string), Some(url), http)
}

fn never() -> CancellationToken {
    CancellationToken::new()
}

/// A fake that answers every request with `reply`.
async fn fake_replying(reply: Reply) -> FakeHuggingFace {
    FakeHuggingFace::spawn(move |_| reply.clone()).await
}

fn status(code: u16, headers: Vec<(&'static str, &'static str)>, body: &str) -> Reply {
    Reply::Status {
        code,
        headers,
        body: body.to_string(),
    }
}

fn details_reply(body: serde_json::Value) -> Reply {
    Reply::Json(body.to_string())
}

fn quant(name: &str, size: Option<f64>) -> HuggingFaceQuantization {
    HuggingFaceQuantization {
        name: name.to_string(),
        size,
    }
}

fn error_text(error: LlamaError) -> String {
    error.to_string()
}

// ------------------------------------------------------------- llama-extension.test.ts:446-484 --

/// `llama-extension.test.ts:446-484`: the search and details calls against the upstream fixture,
/// with the query parameters, the exact paths and the bearer token asserted on the wire.
#[tokio::test]
async fn searches_hugging_face_and_reads_quantizations_plus_access_requirements() {
    let server = FakeHuggingFace::spawn(|target| {
        if target.starts_with("/api/models?") {
            return Reply::Json(r#"[{"id":"owner/model-GGUF","downloads":1200}]"#.to_string());
        }
        if target == "/api/models/owner/model-GGUF?blobs=true" {
            return details_reply(serde_json::json!({
                "id": "owner/model-GGUF",
                "gated": "manual",
                "siblings": [
                    { "rfilename": "model-Q5_K_M.gguf", "size": 6000 },
                    { "rfilename": "model-Q4_K_M-00001-of-00002.gguf", "size": 2000 },
                    { "rfilename": "model-Q4_K_M-00002-of-00002.gguf", "size": 3000 },
                    { "rfilename": "mmproj-F16.gguf", "size": 1000 },
                ],
            }));
        }
        status(404, vec![], "")
    })
    .await;
    let client = server.client(Some("hf-secret"));

    let models = client.search("qwen coder", &never()).await.unwrap();
    assert_eq!(
        models,
        vec![HuggingFaceModel {
            id: "owner/model-GGUF".to_string(),
            downloads: 1200.0
        }]
    );
    let details = client.details("owner/model-GGUF", &never()).await.unwrap();
    assert_eq!(
        details,
        HuggingFaceModelDetails {
            id: "owner/model-GGUF".to_string(),
            gated: HuggingFaceGated::Manual,
            quantizations: vec![quant("Q4_K_M", Some(5000.0)), quant("Q5_K_M", Some(6000.0))],
        }
    );

    let requests = server.requests();
    assert_eq!(requests.len(), 2);
    for request in &requests {
        assert_eq!(
            request.headers.get("authorization").map(String::as_str),
            Some("Bearer hf-secret"),
            "{}",
            request.target
        );
    }
    // `huggingface.ts:101-107`: parameter order and URLSearchParams encoding (space is `+`).
    assert_eq!(
        requests[0].target,
        "/api/models?search=qwen+coder&filter=gguf&sort=downloads&direction=-1&limit=20"
    );
    assert_eq!(
        requests[1].target,
        "/api/models/owner/model-GGUF?blobs=true"
    );
}

/// `llama-extension.test.ts:483`: `HF_TOKEN` is trimmed.
#[tokio::test]
async fn hf_token_from_the_environment_is_trimmed() {
    let env = env_of(&[("HF_TOKEN", " hf-secret ")]);
    assert_eq!(
        find_huggingface_token_with_home(&env, None)
            .await
            .as_deref(),
        Some("hf-secret")
    );
}

// ------------------------------------------------------------------------------------- requests --

/// `huggingface.ts:74`: no token (absent or empty) sends no `Authorization` header.
#[tokio::test]
async fn no_token_sends_no_authorization_header() {
    for token in [None, Some("")] {
        let server = fake_replying(Reply::Json("[]".to_string())).await;
        server.client(token).search("x", &never()).await.unwrap();
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert!(
            !requests[0].headers.contains_key("authorization"),
            "token {token:?} must not produce a header: {:?}",
            requests[0].headers
        );
    }
}

/// `huggingface.ts:69`: trailing slashes of the base URL are stripped before the path is appended.
#[tokio::test]
async fn base_url_trailing_slashes_are_stripped() {
    let server = fake_replying(Reply::Json("[]".to_string())).await;
    let client = client_for(&format!("{}///", server.url), None);
    assert_eq!(client.base_url(), server.url);
    client.search("x", &never()).await.unwrap();
    let target = server.requests()[0].target.clone();
    assert!(target.starts_with("/api/models?"), "{target}");
}

/// `huggingface.ts:67`: the default base URL is `https://huggingface.co`.
#[tokio::test]
async fn the_default_base_url_is_huggingface_co() {
    let client = HuggingFaceClient::new(None, None, None).await.unwrap();
    assert_eq!(client.base_url(), "https://huggingface.co");
}

/// `huggingface.ts:119`: the id is encoded per path segment, the `/` between owner and repository
/// survives.
#[tokio::test]
async fn details_encodes_each_id_segment_but_keeps_the_slash() {
    let server = fake_replying(details_reply(serde_json::json!({}))).await;
    server
        .client(None)
        .details("own er/mo del+é", &never())
        .await
        .unwrap();
    assert_eq!(
        server.requests()[0].target,
        "/api/models/own%20er/mo%20del%2B%C3%A9?blobs=true"
    );
}

/// The token must never reach a log line: `Debug` of the client redacts it.
#[test]
fn debug_output_never_contains_the_token() {
    let client = client_for("http://127.0.0.1:1", Some("hf-super-secret"));
    let text = format!("{client:?}");
    assert!(!text.contains("hf-super-secret"), "{text}");
    assert!(text.contains("<redacted>"), "{text}");
}

// --------------------------------------------------------------------------------------- search --

/// `huggingface.ts:109-115`: a non-array body is an error; entries without a string `id` are
/// dropped; a missing or non-numeric `downloads` is `0`.
#[tokio::test]
async fn search_validates_and_filters_the_payload() {
    let server = fake_replying(Reply::Json(
        r#"[{"id":"a/one","downloads":5},{"id":"a/two"},{"id":"a/three","downloads":"9"},
            {"downloads":7},{"id":3},null,"text",{"id":"a/four","downloads":2.5}]"#
            .to_string(),
    ))
    .await;
    let models = server.client(None).search("q", &never()).await.unwrap();
    let flat: Vec<(String, f64)> = models.into_iter().map(|m| (m.id, m.downloads)).collect();
    assert_eq!(
        flat,
        vec![
            ("a/one".to_string(), 5.0),
            ("a/two".to_string(), 0.0),
            ("a/three".to_string(), 0.0),
            ("a/four".to_string(), 2.5),
        ]
    );

    for body in [r#"{"id":"a/b"}"#, "null", "not json", ""] {
        let server = fake_replying(Reply::Json(body.to_string())).await;
        let error = server.client(None).search("q", &never()).await.unwrap_err();
        assert_eq!(
            error_text(error),
            "Hugging Face returned invalid search results",
            "body {body:?}"
        );
    }
}

// -------------------------------------------------------------------------------------- details --

/// `huggingface.ts:121-123`: only an object (an array passes `typeof === "object"`) is details.
#[tokio::test]
async fn details_rejects_a_non_object_payload() {
    for body in ["5", r#""x""#, "null", "true", "garbage", ""] {
        let server = fake_replying(Reply::Json(body.to_string())).await;
        let error = server
            .client(None)
            .details("a/b", &never())
            .await
            .unwrap_err();
        assert_eq!(
            error_text(error),
            "Hugging Face returned invalid model details",
            "body {body:?}"
        );
    }
    let server = fake_replying(Reply::Json("[]".to_string())).await;
    let details = server.client(None).details("a/b", &never()).await.unwrap();
    assert_eq!(
        details,
        HuggingFaceModelDetails {
            id: "a/b".to_string(),
            gated: HuggingFaceGated::Open,
            quantizations: vec![],
        }
    );
}

/// `huggingface.ts:152-156`: the requested id stands in for a missing `id`; only `"auto"` and
/// `"manual"` gate; a body without siblings has no quantizations.
#[tokio::test]
async fn details_id_gated_and_missing_siblings() {
    let cases = [
        (
            serde_json::json!({ "gated": "auto" }),
            HuggingFaceGated::Auto,
        ),
        (
            serde_json::json!({ "gated": "manual" }),
            HuggingFaceGated::Manual,
        ),
        (
            serde_json::json!({ "gated": false }),
            HuggingFaceGated::Open,
        ),
        (serde_json::json!({ "gated": true }), HuggingFaceGated::Open),
        (
            serde_json::json!({ "gated": "yes" }),
            HuggingFaceGated::Open,
        ),
        (
            serde_json::json!({ "gated": ["auto"] }),
            HuggingFaceGated::Open,
        ),
        (serde_json::json!({}), HuggingFaceGated::Open),
    ];
    for (body, gated) in cases {
        let server = fake_replying(details_reply(body.clone())).await;
        let details = server
            .client(None)
            .details("requested/id", &never())
            .await
            .unwrap();
        assert_eq!(details.gated, gated, "{body}");
        assert_eq!(details.id, "requested/id", "{body}");
        assert!(details.quantizations.is_empty(), "{body}");
    }
    assert!(HuggingFaceGated::Auto.is_gated());
    assert!(HuggingFaceGated::Manual.is_gated());
    assert!(!HuggingFaceGated::Open.is_gated());

    let server = fake_replying(details_reply(serde_json::json!({ "id": "canonical/Id" }))).await;
    let details = server
        .client(None)
        .details("requested/id", &never())
        .await
        .unwrap();
    assert_eq!(details.id, "canonical/Id");
    let server = fake_replying(details_reply(serde_json::json!({ "id": 7 }))).await;
    let details = server
        .client(None)
        .details("requested/id", &never())
        .await
        .unwrap();
    assert_eq!(details.id, "requested/id");
}

/// `huggingface.ts:126-141`: shard sums, `mmproj*` (also in a subdirectory) excluded, non-GGUF and
/// non-quantized files ignored, names merged case-insensitively, a missing size on any shard makes
/// the whole quantization's size unknown, non-object siblings skipped.
#[tokio::test]
async fn details_sums_shards_and_applies_the_file_filters() {
    let server = fake_replying(details_reply(serde_json::json!({
        "siblings": [
            { "rfilename": "model-Q4_K_M-00001-of-00003.gguf", "size": 10 },
            { "rfilename": "model-Q4_K_M-00002-of-00003.gguf", "size": 20 },
            { "rfilename": "model-Q4_K_M-00003-of-00003.gguf", "size": 30 },
            { "rfilename": "split/Model-q8_0-00001-of-00002.GGUF", "size": 100 },
            { "rfilename": "split/Model-q8_0-00002-of-00002.GGUF" },
            { "rfilename": "sub/mmproj-F16.gguf", "size": 1 },
            { "rfilename": "MMPROJ-model-F32.gguf", "size": 1 },
            { "rfilename": "README.md", "size": 1 },
            { "rfilename": "model-Q2_K.bin", "size": 1 },
            { "rfilename": "model.gguf", "size": 1 },
            { "rfilename": "model-Q3_K_S.gguf", "size": "9" },
            { "rfilename": 5, "size": 1 },
            { "size": 1 },
            "not an object",
            null,
        ],
    })))
    .await;
    let details = server.client(None).details("a/b", &never()).await.unwrap();
    assert_eq!(
        details.quantizations,
        vec![
            quant("Q4_K_M", Some(60.0)),
            quant("Q3_K_S", None),
            quant("Q8_0", None),
        ]
    );
}

/// `huggingface.ts:142-151`: `Q4_K_M` first wherever it would sort, then ascending size, unknown
/// sizes last, equal sizes by name.
#[tokio::test]
async fn details_sorts_q4_k_m_first_then_size_then_name() {
    let server = fake_replying(details_reply(serde_json::json!({
        "siblings": [
            { "rfilename": "m-Q8_0.gguf", "size": 800 },
            { "rfilename": "m-Q2_K.gguf", "size": 200 },
            { "rfilename": "m-IQ3_XS.gguf" },
            { "rfilename": "m-Q4_K_M.gguf", "size": 99999 },
            { "rfilename": "m-F16.gguf", "size": 1600 },
            { "rfilename": "m-Q6_K.gguf", "size": 200 },
            { "rfilename": "m-BF16.gguf" },
            { "rfilename": "m-Q5_K_M.gguf", "size": 500 },
        ],
    })))
    .await;
    let details = server.client(None).details("a/b", &never()).await.unwrap();
    let names: Vec<&str> = details
        .quantizations
        .iter()
        .map(|q| q.name.as_str())
        .collect();
    assert_eq!(
        names,
        // Q4_K_M, then 200 (Q2_K before Q6_K by name), 500, 800, 1600, then the unknown sizes by name.
        [
            "Q4_K_M", "Q2_K", "Q6_K", "Q5_K_M", "Q8_0", "F16", "BF16", "IQ3_XS"
        ]
    );
}

/// `huggingface.ts:149`, `localeCompare`: among equal sizes `_` collates before letters, which a
/// code-point comparison gets backwards (`_` is 0x5F, after `A-Z`).
#[tokio::test]
async fn equal_sizes_use_locale_collation_for_the_name() {
    let server = fake_replying(details_reply(serde_json::json!({
        "siblings": [
            { "rfilename": "m-Q3_KS.gguf", "size": 7 },
            { "rfilename": "m-Q3_K_S.gguf", "size": 7 },
        ],
    })))
    .await;
    let details = server.client(None).details("a/b", &never()).await.unwrap();
    let names: Vec<&str> = details
        .quantizations
        .iter()
        .map(|q| q.name.as_str())
        .collect();
    assert_eq!(names, ["Q3_K_S", "Q3_KS"]);
}

// ------------------------------------------------------------------------------ quantizations --

/// `huggingface.ts:6-8`, `:130-135`: the file name to quantization table.
#[test]
fn quantization_pattern_table() {
    let cases: &[(&str, Option<&str>)] = &[
        // Common K-quants and legacy quants.
        ("model-Q4_K_M.gguf", Some("Q4_K_M")),
        ("model-Q8_0.gguf", Some("Q8_0")),
        ("model-Q2_K.gguf", Some("Q2_K")),
        ("model-Q6_K_L.gguf", Some("Q6_K_L")),
        ("model-Q4_1.gguf", Some("Q4_1")),
        // I-quants.
        ("model-IQ4_XS.gguf", Some("IQ4_XS")),
        ("model-IQ3_XXS.gguf", Some("IQ3_XXS")),
        ("model-IQ1_S.gguf", Some("IQ1_S")),
        // Unsloth dynamic prefix is part of the name.
        ("model-UD-Q4_K_XL.gguf", Some("UD-Q4_K_XL")),
        ("model-UD-IQ2_M.gguf", Some("UD-IQ2_M")),
        // Float formats.
        ("model-BF16.gguf", Some("BF16")),
        ("model-F16.gguf", Some("F16")),
        ("model-F32.gguf", Some("F32")),
        // MXFP, with and without a suffix.
        ("model-MXFP4.gguf", Some("MXFP4")),
        ("model-MXFP4_MOE.gguf", Some("MXFP4_MOE")),
        // Case is folded: the match is case-insensitive and the name upper-cased.
        ("model-q4_k_m.gguf", Some("Q4_K_M")),
        ("Model-Bf16.GGUF", Some("BF16")),
        // `-`, `_`, `.` or the start of the stem may precede the token.
        ("model_Q5_K_S.gguf", Some("Q5_K_S")),
        ("model.Q5_K_S.gguf", Some("Q5_K_S")),
        ("Q5_K_S.gguf", Some("Q5_K_S")),
        ("F16.gguf", Some("F16")),
        // Directory part is dropped, shard suffix is stripped.
        ("dir/sub/model-Q4_K_M.gguf", Some("Q4_K_M")),
        ("model-Q4_K_M-00001-of-00002.gguf", Some("Q4_K_M")),
        ("Q4_K_M/model-F16-00012-of-00099.gguf", Some("F16")),
        // Not a quantization.
        ("modelQ4_K_M.gguf", None),
        ("model-Q4.gguf", None),
        ("model-Q4_K_M-extra.gguf", None),
        ("model-F8.gguf", None),
        ("model-FP16.gguf", None),
        ("model-MXFP.gguf", None),
        ("model-IQ.gguf", None),
        ("model.gguf", None),
        (".gguf", None),
        // A shard suffix of the wrong width stays on the stem and so blocks the match.
        ("model-Q4_K_M-0001-of-0002.gguf", None),
        // Only `.gguf` files, and never a multimodal projector.
        ("model-Q4_K_M.bin", None),
        ("model-Q4_K_M.gguf.part", None),
        // A five-character extension other than `.gguf` leaves the same stem, so only the extension
        // check rejects it.
        ("model-Q4_K_M.ggml", None),
        ("mmproj-F16.gguf", None),
        ("MMPROJ-model-F16.gguf", None),
        ("dir/mmproj-model-BF16.gguf", None),
    ];
    for (file, expected) in cases {
        assert_eq!(
            quantization_of_file(file).as_deref(),
            *expected,
            "file {file:?}"
        );
    }
}

// --------------------------------------------------------------------------------------- errors --

/// `huggingface.ts:87`, `:95`, `payloadError` (`:26-30`): the payload's non-empty string `error`,
/// else `Hugging Face returned HTTP <status>`.
#[tokio::test]
async fn http_errors_use_the_payload_error_or_the_status() {
    let cases: &[(u16, &str, &str)] = &[
        (500, r#"{"error":"boom"}"#, "boom"),
        (
            401,
            r#"{"error":"Invalid credentials in Authorization header"}"#,
            "Invalid credentials in Authorization header",
        ),
        (500, r#"{"error":""}"#, "Hugging Face returned HTTP 500"),
        (
            500,
            r#"{"error":{"message":"x"}}"#,
            "Hugging Face returned HTTP 500",
        ),
        (500, r#"{"error":7}"#, "Hugging Face returned HTTP 500"),
        (
            502,
            r#"{"message":"bad gateway"}"#,
            "Hugging Face returned HTTP 502",
        ),
        (
            404,
            "<html>not found</html>",
            "Hugging Face returned HTTP 404",
        ),
        (403, "", "Hugging Face returned HTTP 403"),
        (500, "null", "Hugging Face returned HTTP 500"),
        (500, "[1,2]", "Hugging Face returned HTTP 500"),
    ];
    for (code, body, expected) in cases {
        let server = fake_replying(status(*code, vec![], body)).await;
        let search = server.client(None).search("q", &never()).await.unwrap_err();
        assert_eq!(error_text(search), *expected, "search {code} {body}");
        let details = server
            .client(None)
            .details("a/b", &never())
            .await
            .unwrap_err();
        assert_eq!(error_text(details), *expected, "details {code} {body}");
    }
}

/// `huggingface.ts:88-94`, `parseRateLimitDelay` (`:32-35`): HTTP 429 reads the delay from
/// `retry-after`, else from the first `t=` of `ratelimit`; the payload is ignored.
#[tokio::test]
async fn rate_limit_message_reads_retry_after_then_the_ratelimit_header() {
    let cases: &[(Vec<(&'static str, &'static str)>, &str)] = &[
        (
            vec![("retry-after", "30")],
            "Hugging Face rate limit reached; retry in 30s",
        ),
        (
            vec![("retry-after", " 12 ")],
            "Hugging Face rate limit reached; retry in 12s",
        ),
        (
            vec![("ratelimit", "\"api\";r=0;t=45")],
            "Hugging Face rate limit reached; retry in 45s",
        ),
        (
            vec![("ratelimit", "t=7")],
            "Hugging Face rate limit reached; retry in 7s",
        ),
        (
            vec![("ratelimit", "\"pages\";r=1;t=3;x=1")],
            "Hugging Face rate limit reached; retry in 3s",
        ),
        // retry-after wins over ratelimit.
        (
            vec![("retry-after", "5"), ("ratelimit", "\"api\";r=0;t=45")],
            "Hugging Face rate limit reached; retry in 5s",
        ),
        // A retry-after that is not a number (an HTTP date) falls through to ratelimit.
        (
            vec![
                ("retry-after", "Wed, 21 Oct 2015 07:28:00 GMT"),
                ("ratelimit", "\"api\";r=0;t=9"),
            ],
            "Hugging Face rate limit reached; retry in 9s",
        ),
        // Zero and empty are falsy, as is a `t=` that is not at the start or after `;`.
        (
            vec![("retry-after", "0")],
            "Hugging Face rate limit reached",
        ),
        (vec![("retry-after", "")], "Hugging Face rate limit reached"),
        (
            vec![("ratelimit", "\"api\";r=0;t=0")],
            "Hugging Face rate limit reached",
        ),
        (
            vec![("ratelimit", "\"api\";r=0;t=")],
            "Hugging Face rate limit reached",
        ),
        (
            vec![("ratelimit", "\"api\";r=0;t=abc")],
            "Hugging Face rate limit reached",
        ),
        (
            vec![("ratelimit", "x t=5")],
            "Hugging Face rate limit reached",
        ),
        (
            vec![("ratelimit", "\"api\";r=0;at=5")],
            "Hugging Face rate limit reached",
        ),
        (vec![], "Hugging Face rate limit reached"),
    ];
    for (headers, expected) in cases {
        let server = fake_replying(status(429, headers.clone(), r#"{"error":"slow down"}"#)).await;
        let search = server.client(None).search("q", &never()).await.unwrap_err();
        assert_eq!(error_text(search), *expected, "search {headers:?}");
        let details = server
            .client(None)
            .details("a/b", &never())
            .await
            .unwrap_err();
        assert_eq!(error_text(details), *expected, "details {headers:?}");
    }
}

/// `huggingface.ts:75`: the request is bounded by a timeout.
#[tokio::test]
async fn a_server_that_never_answers_times_out() {
    let server = fake_replying(Reply::Hang).await;
    let client = server
        .client(None)
        .with_request_timeout(Duration::from_millis(150));
    let error = client.search("q", &never()).await.unwrap_err();
    assert_eq!(error, LlamaError::Timeout);
    assert_eq!(
        error.to_string(),
        "The operation was aborted due to timeout"
    );
}

/// `huggingface.ts:100`, `:118` (`signal?`): the caller's cancellation aborts the request.
#[tokio::test]
async fn cancellation_aborts_the_request() {
    let server = fake_replying(Reply::Hang).await;
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        trigger.cancel();
    });
    let error = server
        .client(None)
        .details("a/b", &cancel)
        .await
        .unwrap_err();
    assert_eq!(error, LlamaError::Cancelled);

    // Already cancelled: no hang, no success.
    let error = server.client(None).search("q", &cancel).await.unwrap_err();
    assert_eq!(error, LlamaError::Cancelled);
}

/// A refused connection is a transport failure with Node's `fetch failed` text.
#[tokio::test]
async fn a_refused_connection_is_a_fetch_failure() {
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        listener.local_addr().unwrap().port()
    };
    let client = client_for(&format!("http://127.0.0.1:{port}"), None);
    let error = client.search("q", &never()).await.unwrap_err();
    assert!(
        matches!(&error, LlamaError::Transport(text) if text.starts_with("fetch failed")),
        "{error:?}"
    );
}

// --------------------------------------------------------------------------------- token lookup --

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn write_token(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `huggingface.ts:47-48`: `HF_TOKEN` beats every file; a blank one does not count.
#[tokio::test]
async fn hf_token_environment_beats_token_files_and_blank_falls_through() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("token");
    write_token(&file, "from-file");
    let path = text(&file);

    let env = env_of(&[("HF_TOKEN", "from-env"), ("HF_TOKEN_PATH", &path)]);
    assert_eq!(
        find_huggingface_token_with_home(&env, None)
            .await
            .as_deref(),
        Some("from-env")
    );
    for blank in ["", "   ", "\n\t"] {
        let env = env_of(&[("HF_TOKEN", blank), ("HF_TOKEN_PATH", &path)]);
        assert_eq!(
            find_huggingface_token_with_home(&env, None)
                .await
                .as_deref(),
            Some("from-file"),
            "blank {blank:?}"
        );
    }
}

/// `huggingface.ts:50-59`: the files are tried in the order `$HF_TOKEN_PATH`, `$HF_HOME/token`,
/// `$XDG_CACHE_HOME/huggingface/token`, `~/.cache/huggingface/token`.
#[tokio::test]
async fn token_files_are_tried_in_precedence_order() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let explicit = root.join("explicit-token");
    let hf_home = root.join("hf-home");
    let xdg = root.join("xdg");
    let home = root.join("home");
    write_token(&explicit, "from-explicit");
    write_token(&hf_home.join("token"), "from-hf-home");
    write_token(&xdg.join("huggingface").join("token"), "from-xdg");
    write_token(
        &home.join(".cache").join("huggingface").join("token"),
        "from-home",
    );
    let (explicit_s, hf_home_s, xdg_s) = (text(&explicit), text(&hf_home), text(&xdg));
    let all = [
        ("HF_TOKEN_PATH", explicit_s.as_str()),
        ("HF_HOME", hf_home_s.as_str()),
        ("XDG_CACHE_HOME", xdg_s.as_str()),
    ];

    let lookup = |pairs: &[(&str, &str)]| {
        let env = env_of(pairs);
        let home = home.clone();
        async move { find_huggingface_token_with_home(&env, Some(&home)).await }
    };
    assert_eq!(lookup(&all).await.as_deref(), Some("from-explicit"));
    assert_eq!(lookup(&all[1..]).await.as_deref(), Some("from-hf-home"));
    assert_eq!(lookup(&all[2..]).await.as_deref(), Some("from-xdg"));
    assert_eq!(lookup(&[]).await.as_deref(), Some("from-home"));
    assert_eq!(
        find_huggingface_token_with_home(&env_of(&[]), None).await,
        None
    );
}

/// `huggingface.ts:37-44`, `:56-59`: a missing, unreadable or white-space-only file is skipped and
/// the next candidate is tried; the first non-empty one is trimmed; the files are only read.
#[tokio::test]
async fn unusable_token_files_are_skipped_and_the_token_is_trimmed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let missing = root.join("missing");
    let blank = root.join("blank");
    let directory = root.join("a-directory");
    let hf_home = root.join("hf-home");
    write_token(&blank, "  \r\n\t ");
    std::fs::create_dir_all(&directory).unwrap();
    write_token(&hf_home.join("token"), "\u{feff}  hf_abc123 \r\n");
    let xdg = root.join("xdg");
    write_token(&xdg.join("huggingface").join("token"), "never-reached");

    for skipped in [&missing, &blank, &directory] {
        let skipped = text(skipped);
        let hf_home_s = text(&hf_home);
        let xdg_s = text(&xdg);
        let env = env_of(&[
            ("HF_TOKEN_PATH", &skipped),
            ("HF_HOME", &hf_home_s),
            ("XDG_CACHE_HOME", &xdg_s),
        ]);
        assert_eq!(
            find_huggingface_token_with_home(&env, None)
                .await
                .as_deref(),
            Some("hf_abc123"),
            "skipping {skipped}"
        );
    }
    assert_eq!(
        std::fs::read_to_string(hf_home.join("token")).unwrap(),
        "\u{feff}  hf_abc123 \r\n",
        "the token file must be left untouched"
    );

    // Nothing usable anywhere: no token.
    let blank_s = text(&blank);
    let env = env_of(&[("HF_TOKEN_PATH", &blank_s)]);
    assert_eq!(
        find_huggingface_token_with_home(&env, Some(root)).await,
        None
    );
}

/// Empty `HF_TOKEN_PATH` / `HF_HOME` / `XDG_CACHE_HOME` are not paths (`huggingface.ts:51-55`,
/// `Boolean(path)` and the truthiness of the env value).
#[tokio::test]
async fn empty_path_variables_are_not_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    write_token(
        &home.join(".cache").join("huggingface").join("token"),
        "from-home",
    );
    let env = env_of(&[
        ("HF_TOKEN_PATH", ""),
        ("HF_HOME", ""),
        ("XDG_CACHE_HOME", ""),
    ]);
    assert_eq!(
        find_huggingface_token_with_home(&env, Some(&home))
            .await
            .as_deref(),
        Some("from-home")
    );
}

/// `huggingface.ts:46`: the default environment is the process's.
#[test]
fn process_environment_is_the_process_environment() {
    let snapshot = crate::huggingface::process_environment();
    assert!(!snapshot.is_empty());
    for (key, value) in std::env::vars() {
        if key.contains('=') || key.is_empty() {
            continue;
        }
        assert_eq!(snapshot.get(&key), Some(&value), "{key}");
    }
}

/// A search response larger than the cap is refused instead of buffered for the length of the
/// timeout.
#[tokio::test]
async fn a_response_body_beyond_the_cap_is_refused() {
    let big = "x".repeat(crate::client::MAX_BODY_BYTES + 1);
    let server = fake_replying(Reply::Json(big)).await;
    let error = server
        .client(None)
        .search("qwen", &never())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        format!(
            "Hugging Face response exceeds {} bytes",
            crate::client::MAX_BODY_BYTES
        )
    );
}

/// The Hugging Face client resolves its proxy through the same path as the llama.cpp client and the
/// classifier: the provider env overlay's `HTTP_PROXY` routes the request through it.
#[tokio::test]
async fn the_client_honours_the_provider_env_overlays_proxy() {
    let proxy = fake_replying(Reply::Json("[]".to_string())).await;
    let env: BTreeMap<String, String> = [
        ("HTTP_PROXY".to_string(), proxy.url.clone()),
        ("NO_PROXY".to_string(), String::new()),
        ("no_proxy".to_string(), String::new()),
    ]
    .into();
    let client = HuggingFaceClient::new(None, Some("http://hf.invalid"), Some(&env))
        .await
        .unwrap();
    client.search("qwen", &never()).await.unwrap();
    let targets: Vec<String> = proxy
        .requests()
        .into_iter()
        .map(|seen| seen.target)
        .collect();
    assert!(
        targets
            .iter()
            .any(|target| target.starts_with("http://hf.invalid/api/models")),
        "the proxy must have been asked for the absolute URL: {targets:?}"
    );
}
