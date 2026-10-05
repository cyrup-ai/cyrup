//! PROV-118 — "Sign in with ChatGPT" request shaping and the ChatGPT-usage hint.
//!
//! Translations of pi v1.0.1's own two test files:
//! `test/openai-responses-chatgpt-sign-in.test.ts` ("omits request fields that token sharing
//! rejects", "omits prompt_cache_options on models with explicit prompt cache mode", "keeps those
//! fields for OpenAI API keys / other OpenAI-compatible endpoints") and
//! `test/openai-responses-usage-limit.test.ts` ("links to ChatGPT usage when the request is
//! rejected", "links to ChatGPT usage when the stream fails").
//!
//! Nothing here touches the network: the two end-to-end cases keep `model.base_url` at
//! `https://api.openai.com/v1` — which is what `isChatGPTSignIn` reads — and point the request at a
//! loopback socket through the auth base-url override (`url::resolve_url`).

use super::*;

/// The four fields a token-sharing request must not carry (`openai-responses.ts:336`, `:337`,
/// `:341`, `:345` @v1.0.1).
const OMITTED_FIELDS: [&str; 4] = [
    "prompt_cache_retention",
    "prompt_cache_options",
    "max_output_tokens",
    "temperature",
];

/// A non-reasoning OpenAI model, so `temperature` is not stripped for the unrelated PERM-012
/// reason. `model()` is `reasoning: true`, which would make the api-key CONTROL below pass
/// vacuously.
fn sampled_model() -> Model {
    let mut m = model();
    m.reasoning = false;
    m
}

/// Everything the four fields need in order to be written at all.
fn full_opts() -> StreamOptions {
    StreamOptions {
        max_tokens: Some(1000),
        temperature: Some(0.5),
        cache_retention: Some(CacheRetention::Long),
        session_id: Some("session-1".into()),
        ..Default::default()
    }
}

/// `"omits request fields that token sharing rejects"` — all four at once.
#[test]
fn a_chatgpt_sign_in_token_omits_all_four_unsupported_fields() {
    let mut m = sampled_model();
    // explicit prompt-cache mode so `prompt_cache_options` would otherwise be written too
    m.compat = Some(ModelCompat {
        supports_explicit_prompt_cache_mode: Some(true),
        ..Default::default()
    });
    let body = build_params_for(
        &m,
        &user_ctx("hi"),
        &full_opts(),
        None,
        ResponsesTokenKind::ChatGptSignIn,
    );
    for field in OMITTED_FIELDS {
        assert!(
            body.get(field).is_none(),
            "a ChatGPT sign-in request must not carry {field}: {body}"
        );
    }
    // The omission is surgical: `prompt_cache_key`, `store` and `model` are untouched (`:335`,
    // `:338`, `:332`). Dropping those would be "never send" rather than "omit".
    assert_eq!(body["prompt_cache_key"], "session-1");
    assert_eq!(body["store"], false);
    assert_eq!(body["model"], "gpt-5");
}

/// THE CONTROL, without which "omit" could silently degrade into "never send": the same model and
/// the same options under an **api-key** credential carry all four.
///
/// `"keeps those fields for OpenAI API keys"`, plus the `prompt_cache_options` half of
/// `"omits prompt_cache_options on models with explicit prompt cache mode"` (upstream asserts the
/// api-key payload equals `{ttl:"30m"}` there).
#[test]
fn an_api_key_request_keeps_all_four_fields() {
    let mut m = sampled_model();
    m.compat = Some(ModelCompat {
        supports_explicit_prompt_cache_mode: Some(true),
        ..Default::default()
    });
    let explicit = build_params_for(
        &m,
        &user_ctx("hi"),
        &full_opts(),
        None,
        ResponsesTokenKind::ApiKey,
    );
    assert_eq!(explicit["prompt_cache_options"], json!({"ttl": "30m"}));
    assert_eq!(explicit["max_output_tokens"], 1000);
    assert_eq!(explicit["temperature"], 0.5);

    // A non-explicit-mode model takes `prompt_cache_retention: "24h"` instead, which is the fourth
    // field's control.
    let plain = build_params_for(
        &sampled_model(),
        &user_ctx("hi"),
        &full_opts(),
        None,
        ResponsesTokenKind::ApiKey,
    );
    assert_eq!(plain["prompt_cache_retention"], "24h");
    assert_eq!(plain["max_output_tokens"], 1000);
    assert_eq!(plain["temperature"], 0.5);

    // ...and the same model under a sign-in token drops the retention field.
    let signed_in = build_params_for(
        &sampled_model(),
        &user_ctx("hi"),
        &full_opts(),
        None,
        ResponsesTokenKind::ChatGptSignIn,
    );
    assert!(signed_in.get("prompt_cache_retention").is_none());
}

/// Serve one canned HTTP/1.1 response off `127.0.0.1:0` and record the request. No test here may
/// reach a real host.
async fn serve_once(
    status_line: &'static str,
    content_type: &'static str,
    body: &'static str,
) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink = seen.clone();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = vec![0u8; 16384];
            let n = sock.read(&mut buf).await.unwrap_or(0);
            sink.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).to_string());
            let head = format!(
                "{status_line}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(head.as_bytes()).await;
            let _ = sock.write_all(body.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (format!("http://{addr}"), seen)
}

/// `AuthResult` with `key` as the resolved credential and the request redirected to `base`.
/// `model.base_url` stays OpenAI's own, which is what `isChatGPTSignIn` reads (`:43`).
fn auth_to(key: &str, base: &str) -> AuthResult {
    AuthResult {
        auth: crate::auth::ModelAuth {
            api_key: Some(key.to_string()),
            headers: None,
            base_url: Some(format!("{base}/v1")),
        },
        env: None,
        source: Some("test".to_string()),
    }
}

async fn drain(rx: tokio::sync::mpsc::Receiver<crate::stream::StreamEvent>) -> AssistantMessage {
    let stream = Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx));
    collect_message(stream).await
}

/// Capture the body `ApiImpl::run` actually posts for `key`, end to end through
/// `ResponsesCredential::resolve`. This is what proves the classification is WIRED, not merely
/// implemented: `build_params_for` could be correct while `run` passed the wrong variant.
async fn posted_body_for(key: &'static str) -> Value {
    let (base, seen) = serve_once(
        "HTTP/1.1 500 Internal Server Error",
        "application/json",
        "{}",
    )
    .await;
    let mut m = sampled_model();
    m.compat = Some(ModelCompat {
        supports_explicit_prompt_cache_mode: Some(true),
        ..Default::default()
    });
    let (sink, rx) = crate::api::channel(64);
    OpenAiResponsesApi::new()
        .run(
            &m,
            &user_ctx("hi"),
            &auth_to(key, &base),
            &full_opts(),
            cyrup_core::CancelToken::new(),
            sink,
        )
        .await;
    let _ = drain(rx).await;
    let req = seen.lock().unwrap().first().cloned().unwrap_or_default();
    let body = req.split("\r\n\r\n").nth(1).unwrap_or_default().to_string();
    serde_json::from_str(&body).unwrap_or_else(|e| panic!("json body ({e}): {req}"))
}

/// End to end: the credential that auth resolution produced decides the body.
#[tokio::test]
async fn run_omits_the_four_fields_only_for_a_sign_in_token() {
    let signed_in = posted_body_for("chatgpt-access-token").await;
    for field in OMITTED_FIELDS {
        assert!(
            signed_in.get(field).is_none(),
            "run() sent {field} on a ChatGPT sign-in token: {signed_in}"
        );
    }
    assert_eq!(signed_in["prompt_cache_key"], "session-1");

    let api_key = posted_body_for("sk-proj-test").await;
    assert_eq!(api_key["max_output_tokens"], 1000);
    assert_eq!(api_key["temperature"], 0.5);
    assert_eq!(api_key["prompt_cache_options"], json!({"ttl": "30m"}));
}

// ---------------------------------------------------------------------------
// The ChatGPT-usage hint — `openai-responses.ts:226-229`
// ---------------------------------------------------------------------------

/// `"Check your ChatGPT usage: …"` as upstream spells it (`:34`, `:229`).
const USAGE_HINT: &str = "Check your ChatGPT usage: https://chatgpt.com/settings/usage";

/// `"links to ChatGPT usage when the stream fails"` — a mid-stream `response.failed` carrying the
/// shared-subscription limit code.
#[tokio::test]
async fn a_mid_stream_usage_limit_carries_the_usage_hint() {
    let raw = concat!(
        "data: {\"type\":\"response.failed\",\"sequence_number\":0,\"response\":",
        "{\"id\":\"resp_failed\",\"status\":\"failed\",\"error\":",
        "{\"code\":\"subscription_sharing_usage_limit_exceeded\",\"message\":\"Usage limit reached.\"}}}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    let text = msg.error_message.unwrap_or_default();
    assert!(
        text.contains("subscription_sharing_usage_limit_exceeded: Usage limit reached."),
        "{text}"
    );
    assert!(text.contains(USAGE_HINT), "{text}");
}

/// MIRROR: any other mid-stream failure keeps its message unchanged. Without this the hint could be
/// appended to everything and the test above would still pass.
#[tokio::test]
async fn an_unrelated_stream_failure_carries_no_usage_hint() {
    let raw = concat!(
        "data: {\"type\":\"response.failed\",\"sequence_number\":0,\"response\":",
        "{\"id\":\"resp_failed\",\"status\":\"failed\",\"error\":",
        "{\"code\":\"server_error\",\"message\":\"Boom.\"}}}\n\n",
    );
    let msg = run_decode(raw).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    assert_eq!(msg.error_message.as_deref(), Some("server_error: Boom."));
}

/// `"links to ChatGPT usage when the request is rejected"` — the HTTP 429 path, which in cyrup is
/// `connect_sse`'s own terminal event rather than the decoder's. Upstream's hint sits in the catch
/// around the whole of `stream` (`:215-232`), so it has to cover this too.
#[tokio::test]
async fn a_rejected_request_carries_the_usage_hint() {
    let (base, _seen) = serve_once(
        "HTTP/1.1 429 Too Many Requests",
        "application/json",
        r#"{"error":{"code":"subscription_sharing_usage_limit_exceeded","message":"Usage limit reached.","type":"rate_limit_error"}}"#,
    )
    .await;
    let (sink, rx) = crate::api::channel(64);
    OpenAiResponsesApi::new()
        .run(
            &sampled_model(),
            &user_ctx("hi"),
            &auth_to("chatgpt-access-token", &base),
            &StreamOptions::default(),
            cyrup_core::CancelToken::new(),
            sink,
        )
        .await;
    let msg = drain(rx).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    let text = msg.error_message.unwrap_or_default();
    assert!(
        text.contains("subscription_sharing_usage_limit_exceeded"),
        "{text}"
    );
    assert!(text.contains(USAGE_HINT), "{text}");
}

/// MIRROR for the HTTP path: a 401 keeps its own message.
#[tokio::test]
async fn an_unrelated_http_failure_carries_no_usage_hint() {
    let (base, _seen) = serve_once(
        "HTTP/1.1 401 Unauthorized",
        "application/json",
        r#"{"error":{"code":"invalid_api_key","message":"bad token"}}"#,
    )
    .await;
    let (sink, rx) = crate::api::channel(64);
    OpenAiResponsesApi::new()
        .run(
            &sampled_model(),
            &user_ctx("hi"),
            &auth_to("chatgpt-access-token", &base),
            &StreamOptions::default(),
            cyrup_core::CancelToken::new(),
            sink,
        )
        .await;
    let msg = drain(rx).await;
    assert_eq!(msg.stop_reason, StopReason::Error);
    let text = msg.error_message.unwrap_or_default();
    assert!(!text.contains("Check your ChatGPT usage"), "{text}");
}
