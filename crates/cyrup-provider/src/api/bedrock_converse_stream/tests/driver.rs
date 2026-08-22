//! The request/retry/terminal driver, against a mock Bedrock endpoint.

use super::*;

/// Spawn a one-shot mock HTTP server that writes `raw_response` then closes. Returns its URL.
async fn spawn_mock(raw_response: &'static [u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(raw_response).await;
            let _ = sock.flush().await;
        }
    });
    format!("http://{addr}")
}

/// Spawn a mock server that answers each successive connection with the next entry of
/// `responses` (the last entry repeats), and report how many connections it accepted.
async fn spawn_mock_sequence(
    responses: &'static [&'static [u8]],
) -> (String, Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let body = responses.get(n).copied().unwrap_or_else(|| {
                responses.last().copied().unwrap_or(b"HTTP/1.1 500 x\r\n\r\n")
            });
            let mut buf = [0u8; 2048];
            let _ = sock.read(&mut buf).await;
            let _ = sock.write_all(body).await;
            let _ = sock.flush().await;
        }
    });
    (format!("http://{addr}"), hits)
}

/// PROV-043. pi's Bedrock client inherits the AWS SDK v3 **standard** retry mode — 3 attempts —
/// because its config never sets `maxAttempts`/`retryStrategy`
/// (`bedrock-converse-stream.ts:150-222` @v0.83.0). cyrup issued exactly ONE `send()`, so a
/// routine `ThrottlingException` that pi swallows became a visible turn failure.
///
/// Red before the fix: `hits == 1` and the terminal error was the 429.
#[tokio::test]
async fn a_throttled_bedrock_request_is_retried_to_the_sdk_attempt_count() {
    const THROTTLE: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nx-amzn-errortype: ThrottlingException\r\nretry-after-ms: 1\r\nConnection: close\r\n\r\n{\"message\":\"slow down\"}";
    // A 400 is NOT retryable (provider-retry.ts:22-34), so it terminates the loop and proves
    // the two preceding 429s were retried rather than returned.
    const VALIDATION: &[u8] = b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nx-amzn-errortype: ValidationException\r\nConnection: close\r\n\r\n{\"message\":\"bad input\"}";
    const SEQ: &[&[u8]] = &[THROTTLE, THROTTLE, VALIDATION];

    let (url, hits) = spawn_mock_sequence(SEQ).await;
    let events = run_against(url).await;
    assert_eq!(
        hits.load(std::sync::atomic::Ordering::SeqCst),
        3,
        "standard mode is 3 attempts: the first plus BEDROCK_STANDARD_MODE_RETRIES"
    );
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", kinds(&events));
    };
    assert!(
        error
            .error_message
            .as_deref()
            .is_some_and(|m| m.contains("bad input")),
        "the terminal must be the third response, not the first: {:?}",
        error.error_message
    );
}

/// PROV-043, exhaustion half: a throttle on every attempt fails after the SDK's attempt count,
/// not on the first response.
#[tokio::test]
async fn a_permanently_throttled_bedrock_request_stops_at_the_attempt_count() {
    const THROTTLE: &[u8] = b"HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nx-amzn-errortype: ThrottlingException\r\nretry-after-ms: 1\r\nConnection: close\r\n\r\n{\"message\":\"slow down\"}";
    const SEQ: &[&[u8]] = &[THROTTLE];
    let (url, hits) = spawn_mock_sequence(SEQ).await;
    let events = run_against(url).await;
    assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 3);
    assert!(matches!(events.last(), Some(StreamEvent::Error { .. })));
}

/// PROV-044. `AWS_BEDROCK_FORCE_HTTP1=1` must reach the client builder as `http1_only()`
/// (pi `bedrock-converse-stream.ts:206-209` @v0.83.0), and only when no proxy was resolved —
/// pi's `else if`. Red before the fix: `rg AWS_BEDROCK_FORCE_HTTP1 crates/` was empty and
/// cyrup's client negotiated h2 by ALPN with no override.
#[tokio::test]
async fn force_http1_builds_an_http1_only_client_only_without_a_proxy() {
    use crate::stream::sse::build_client_for_target_forcing_http1;
    let ctx = crate::auth::types::EnvAuthContext;
    // No proxy in the overlay ⇒ the override applies and the client still builds.
    let no_proxy = env_map(&[]);
    assert!(
        build_client_for_target_forcing_http1(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            &ctx,
            Some(&no_proxy),
            None,
            true,
        )
        .await
        .is_ok()
    );
    // With a proxy the override is suppressed (pi's `else if`), and the proxied client builds.
    let proxied = env_map(&[("HTTPS_PROXY", "http://127.0.0.1:3128")]);
    assert!(
        build_client_for_target_forcing_http1(
            "https://bedrock-runtime.us-east-1.amazonaws.com",
            &ctx,
            Some(&proxied),
            None,
            true,
        )
        .await
        .is_ok()
    );
}

/// Drive the real `ApiImpl::run` (so the ported catch arm runs) against `base_url`.
async fn run_against(base_url: String) -> Vec<StreamEvent> {
    let mut model = sonnet_45();
    model.base_url = base_url;
    let (sink, mut rx) = crate::api::channel(64);
    let auth = AuthResult {
        auth: Default::default(),
        env: Some(env_map(&[
            ("AWS_BEDROCK_SKIP_AUTH", "1"),
            ("AWS_REGION", "us-east-1"),
        ])),
        source: None,
    };
    let task = tokio::spawn(async move {
        BedrockConverseStreamApi::new()
            .run(
                &model,
                &user_ctx("hi"),
                &auth,
                &StreamOptions::default(),
                CancelToken::new(),
                sink,
            )
            .await;
    });
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    let _ = task.await;
    out
}

/// VERSION LAG (v0.83.0 → v0.84.1): the whole structured-diagnostic path is new in v0.84.1 —
/// `appendBedrockFailureDiagnostic` (`ai/src/api/bedrock-converse-stream.ts:398-421`), its
/// `normalizeDiagnosticValue`/`extractBedrockErrorCode` helpers (`:381-396`), the hoisted
/// `responseRequestId` (`:225`, assigned at `:254`) and the catch-side call (`:318-320`). None
/// of those four identifiers exists anywhere in `v0.83.0 ai/src/api/bedrock-converse-stream.ts`.
/// `errorMessage` must stay byte-identical, because the retry classifier matches against it.
#[tokio::test]
async fn a_bedrock_failure_carries_structured_diagnostics() {
    let url = spawn_mock(
        b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nx-amzn-errortype: com.amazon.coral.validate#ValidationException\r\nx-amzn-requestid: req-abc-123\r\nConnection: close\r\n\r\n{\"message\":\"bad input\"}",
    )
    .await;
    let events = run_against(url).await;
    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {:?}", kinds(&events));
    };
    // The message is untouched by the diagnostic (pi `:398-402`).
    assert_eq!(
        error.error_message.as_deref(),
        Some("Validation error: 400: {\"message\":\"bad input\"}")
    );
    let diags = error.diagnostics.as_ref().expect("diagnostics");
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].r#type, "bedrock_response_failure");
    // `details` only — the throw is not always an `Error` (pi `:400-402`).
    assert_eq!(diags[0].error, None);
    let details = diags[0].details.as_ref().expect("details");
    assert_eq!(details["status"], json!(400));
    assert_eq!(details["errorCode"], json!("ValidationException"));
    assert_eq!(details["requestId"], json!("req-abc-123"));
}

/// MIRROR: the helpers omit unknown fields rather than guessing them, and drop over-long values
/// instead of truncating (pi `:379-396`).
#[test]
fn diagnostic_values_are_normalized_not_guessed() {
    assert_eq!(
        normalize_diagnostic_value("  req-1  ").as_deref(),
        Some("req-1")
    );
    assert_eq!(normalize_diagnostic_value("   "), None);
    assert_eq!(normalize_diagnostic_value(&"x".repeat(200)).as_deref().map(str::len), Some(200));
    // 201 chars is DROPPED, not truncated: a truncated request id is not a request id.
    assert_eq!(normalize_diagnostic_value(&"x".repeat(201)), None);

    // ── The cap counts UTF-16 CODE UNITS, not scalar values ──────────────────────────────
    // pi v0.84.1 `ai/src/api/bedrock-converse-stream.ts:384`:
    //     `if (trimmed.length === 0 || trimmed.length > MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS)`
    // JS `.length` is UTF-16 code units. `\u{1F600}` is ONE scalar but TWO code units, so a
    // 101-emoji value sits in the window where the units disagree: 101 scalars (<= 200, would
    // be KEPT by a `chars().count()` cap) but 202 UTF-16 units (> 200, DROPPED by pi). This
    // case is what distinguishes the two measures — an ASCII string can never separate them.
    let astral_101 = "\u{1F600}".repeat(101);
    assert_eq!(astral_101.chars().count(), 101, "under the cap in SCALARS");
    assert_eq!(astral_101.encode_utf16().count(), 202, "over the cap in UTF-16 UNITS");
    assert_eq!(
        normalize_diagnostic_value(&astral_101),
        None,
        "pi measures `.length` in UTF-16 units, so 202 > 200 drops this value"
    );

    // MIRROR: 100 emoji is exactly 200 UTF-16 units, and pi's check is `>`, so it is KEPT.
    // This pins the boundary from below — a fix that simply dropped everything non-ASCII
    // would fail here.
    let astral_100 = "\u{1F600}".repeat(100);
    assert_eq!(astral_100.encode_utf16().count(), MAX_BEDROCK_DIAGNOSTIC_VALUE_CHARS);
    assert_eq!(
        normalize_diagnostic_value(&astral_100).as_deref(),
        Some(astral_100.as_str()),
        "exactly at the cap: pi's `>` keeps it"
    );

    // Only modeled Bedrock shapes (which all end in `Exception`) are error codes.
    assert_eq!(
        extract_bedrock_error_code("ThrottlingException").as_deref(),
        Some("ThrottlingException")
    );
    assert_eq!(extract_bedrock_error_code("TimeoutError"), None);

    // Nothing known → no diagnostic at all (pi `:419`).
    let mut msg = AssistantMessage::errored(
        "amazon-bedrock".into(),
        "m",
        None,
        StopReason::Error,
        "boom",
    );
    append_bedrock_failure_diagnostic(&mut msg, None, None, None);
    assert_eq!(msg.diagnostics, None);

    // A mid-stream modeled exception reaches upstream as a bare object literal: only the
    // hoisted request id lands (pi `:400-402`).
    append_bedrock_failure_diagnostic(&mut msg, None, None, Some("req-9"));
    let diags = msg.diagnostics.as_ref().expect("diagnostics");
    let details = diags[0].details.as_ref().expect("details");
    assert_eq!(details["requestId"], json!("req-9"));
    assert_eq!(details.get("status"), None);
    assert_eq!(details.get("errorCode"), None);
}

/// A stream that ends without `messageStop` is TRUNCATED, never a clean `stop`
/// (PROV-010 — the rule `StreamEvent::end_of_stream` exists to enforce).
#[tokio::test]
async fn a_stream_without_message_stop_is_an_error_terminal() {
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event(
                "contentBlockDelta",
                "{\"contentBlockIndex\":0,\"delta\":{\"text\":\"hi\"}}",
            ),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else {
        panic!("expected an error terminal, got {:?}", kinds(&events));
    };
    assert_eq!(error.stop_reason, StopReason::Error);
    assert_eq!(
        error.error_message.as_deref(),
        Some("Bedrock stream ended without a stop reason")
    );
}

/// pi `:298-300`: a settled `error` stop reason throws with the mapped diagnostic.
#[tokio::test]
async fn a_guardrail_stop_reaches_the_terminal_with_its_diagnostic() {
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event("messageStop", "{\"stopReason\":\"guardrail_intervened\"}"),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else {
        panic!("expected an error terminal");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Provider stopped with: guardrail_intervened")
    );

    // MIRROR: a `max_tokens` stop is a clean `done`, not an error.
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event("messageStop", "{\"stopReason\":\"max_tokens\"}"),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Done { message, .. } = events.last().unwrap() else {
        panic!("expected a done terminal");
    };
    assert_eq!(message.stop_reason, StopReason::Length);
}

/// PORT BUG (present at v0.83.0, never ported): pi writes
/// `output.rawStopReason = item.messageStop.stopReason`
/// (`v0.84.1 ai/src/api/bedrock-converse-stream.ts:276`; `v0.83.0 …:270`). Bedrock's mapping is
/// especially lossy — every unrecognized reason falls into the `_ => (Error, None)` arm of
/// [`map_stop_reason`] with no message at all, so the raw string is the ONLY record of it.
#[tokio::test]
async fn message_stop_records_the_providers_own_stop_reason() {
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event("messageStop", "{\"stopReason\":\"guardrail_intervened\"}"),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else {
        panic!("expected an error terminal");
    };
    assert_eq!(
        error.raw_stop_reason.as_deref(),
        Some("guardrail_intervened")
    );

    // MIRROR 1: a clean `end_turn` keeps its raw word on the `done` terminal.
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event("messageStop", "{\"stopReason\":\"end_turn\"}"),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Done { message, .. } = events.last().unwrap() else {
        panic!("expected a done terminal");
    };
    assert_eq!(message.stop_reason, StopReason::Stop);
    assert_eq!(message.raw_stop_reason.as_deref(), Some("end_turn"));

    // MIRROR 2: pi's assignment at `:276` is UNCONDITIONAL, so a `messageStop` with no
    // `stopReason` writes `undefined` — `None`, not a fabricated placeholder.
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            event("messageStop", "{}"),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else {
        panic!("expected an error terminal");
    };
    assert_eq!(error.raw_stop_reason, None);
}

/// pi `:278-288`: an in-stream exception frame is a throw, prefixed by the legacy label.
#[tokio::test]
async fn an_exception_frame_is_terminal_with_the_legacy_prefix() {
    let events = collect(
        vec![
            event("messageStart", "{\"role\":\"assistant\"}"),
            frame(
                &[
                    (":message-type", "exception"),
                    (":exception-type", "throttlingException"),
                    (":content-type", "application/json"),
                ],
                "{\"message\":\"Too many tokens\"}",
            ),
        ],
        &sonnet_45(),
    )
    .await;
    let StreamEvent::Error { error, .. } = events.last().unwrap() else {
        panic!("expected an error terminal");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Throttling error: Too many tokens")
    );
}

/// pi `:258-262`: a `messageStart` whose role is not `assistant` is fatal.
#[tokio::test]
async fn a_user_role_message_start_is_fatal() {
    let events = collect(
        vec![event("messageStart", "{\"role\":\"user\"}")],
        &sonnet_45(),
    )
    .await;
    // pi pushes `start` from `messageStart` (`:262`), so a rejected `messageStart` yields the
    // terminal ALONE — no `start` precedes it.
    assert_eq!(kinds(&events), vec!["error"]);
    let StreamEvent::Error { error, .. } = events.last().unwrap() else {
        panic!("expected an error terminal");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Unexpected assistant message start but got user message start instead")
    );
}

#[test]
fn the_command_input_carries_model_id_for_on_payload_then_leaves_the_body() {
    let model = sonnet_45();
    let body = payload(
        &model,
        &user_ctx("hi"),
        &StreamOptions::default(),
        &BedrockOptions::default(),
    );
    assert_eq!(body["modelId"], json!(model.id.as_str()));

    let (id, rest) = split_command_input(body, &model);
    assert_eq!(id, model.id.as_str());
    assert!(rest.get("modelId").is_none());
    assert!(rest.get("messages").is_some());

    // An `onPayload` replacement that rewrites `modelId` must retarget the URL.
    let replaced = json!({ "modelId": "other.model", "messages": [] });
    let (id, _) = split_command_input(replaced, &model);
    assert_eq!(id, "other.model");
}
