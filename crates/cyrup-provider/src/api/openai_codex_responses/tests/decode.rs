//! End-to-end decode.

use super::*;

async fn drain(sse: &'static str, request_tier: Option<&str>) -> Vec<StreamEvent> {
    let model = codex_model("gpt-5.5-codex");
    let api = ApiId::from(API_ID);
    let (sink, mut rx) = channel(64);
    let frames = map_codex_frames(
        decode_sse_bytes(sse.as_bytes().to_vec()),
        request_tier.map(str::to_string),
    );
    decode_stream(frames, &model, &api, &sink).await;
    drop(sink);
    let mut out = Vec::new();
    while let Some(ev) = rx.recv().await {
        out.push(ev);
    }
    out
}

const CODEX_TEXT_TURN: &str = concat!(
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_c1\"}}\n\n",
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hello\"}\n\n",
    "data: {\"type\":\"response.done\",\"response\":{\"id\":\"resp_c1\",\"status\":\"completed\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"total_tokens\":15}}}\n\n",
);

#[tokio::test]
async fn codex_response_done_completes_the_turn() {
    // `response.done` is a Codex-only spelling; without the mapping the shared decoder would
    // never see a terminal and would report the turn as truncated.
    let events = drain(CODEX_TEXT_TURN, None).await;
    let last = events.last().expect("terminal");
    let msg = last.terminal_message().expect("terminal message");
    assert_eq!(msg.stop_reason, StopReason::Stop);
    assert_eq!(msg.response_id.as_deref(), Some("resp_c1"));
    assert_eq!(msg.usage.output, 5);
    assert!(
        matches!(last, StreamEvent::Done { .. }),
        "expected a done terminal, got {last:?}"
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, StreamEvent::TextDelta { .. })),
        "no text delta decoded"
    );
}

/// Codex reaches `mapStopReason` through the shared `processResponsesStream`
/// (v0.84.1 `openai-codex-responses.ts:52,665`), so the v0.84.1 split of `incomplete` applies
/// here too: only `incomplete_details.reason === "max_output_tokens"` is a clean `length` stop
/// (`openai-responses-shared.ts:751-753`); a bare `incomplete` is an error terminal
/// (`:754-759`). `mapCodexEvents` spreads the response (`openai-codex-responses.ts:745-747`),
/// so `incomplete_details` survives the `response.done` → `response.completed` rename.
#[tokio::test]
async fn incomplete_status_splits_on_the_provider_reason() {
    macro_rules! head {
        () => {
            concat!(
                "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
                "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
            )
        };
    }
    async fn terminal(sse: &'static str) -> AssistantMessage {
        drain(sse, None)
            .await
            .last()
            .and_then(StreamEvent::terminal_message)
            .map(|m| (**m).clone())
            .expect("terminal")
    }

    let capped = terminal(concat!(
        head!(),
        "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
    ))
    .await;
    assert_eq!(capped.stop_reason, StopReason::Length);
    assert_eq!(capped.error_message, None);
    assert_eq!(
        capped.raw_stop_reason.as_deref(),
        Some("incomplete.max_output_tokens")
    );

    let bare = terminal(concat!(
        head!(),
        "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"incomplete\"}}\n\n",
    ))
    .await;
    assert_eq!(bare.stop_reason, StopReason::Error);
    assert_eq!(
        bare.error_message.as_deref(),
        Some("Response incomplete without a provider reason")
    );
    assert_eq!(bare.raw_stop_reason.as_deref(), Some("incomplete"));
}

#[tokio::test]
async fn requested_priority_tier_survives_a_default_response_tier() {
    // The pricing consequence of resolveCodexServiceTier: `priority` doubles the cost
    // (getServiceTierCostMultiplier, :598-610). The backend answering `"default"` must not
    // erase the requested tier.
    const SSE: &str = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
        "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
        "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"service_tier\":\"default\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":1000,\"total_tokens\":2000}}}\n\n",
    );
    let baseline = drain(SSE, None)
        .await
        .last()
        .and_then(StreamEvent::terminal_message)
        .map(|m| (**m).clone())
        .expect("terminal");
    let priority = drain(SSE, Some("priority"))
        .await
        .last()
        .and_then(StreamEvent::terminal_message)
        .map(|m| (**m).clone())
        .expect("terminal");
    // The baseline must be genuinely non-zero, or the ratio assertion below is vacuous — the
    // model now carries real rates precisely so this can be checked.
    assert!(
        baseline.usage.cost.total > 0.0,
        "the baseline must cost something for the ratio to mean anything (got {})",
        baseline.usage.cost.total
    );
    assert!(
        (priority.usage.cost.total - baseline.usage.cost.total * 2.0).abs() < 1e-9,
        "a requested `priority` tier must survive a `\"default\"` response tier and price at 2x \
         (getServiceTierCostMultiplier, :598-610) — priority {} vs baseline {}",
        priority.usage.cost.total,
        baseline.usage.cost.total
    );
}

/// MIRROR for the test above: the resolution is a real decision, not a blanket doubling. With
/// NO requested tier the response's own `"default"` prices at 1x, so the 2x assertion is about
/// `resolve_codex_service_tier` keeping the request's tier rather than about the arithmetic.
#[tokio::test]
async fn an_unrequested_tier_prices_at_the_responses_own_default() {
    const SSE: &str = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"m1\"}}\n\n",
        "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"completed\",\"service_tier\":\"default\",\"usage\":{\"input_tokens\":1000,\"output_tokens\":1000,\"total_tokens\":2000}}}\n\n",
    );
    let msg = drain(SSE, None)
        .await
        .last()
        .and_then(StreamEvent::terminal_message)
        .map(|m| (**m).clone())
        .expect("terminal");
    // 1000 input @ $1/1e6 + 1000 output @ $2/1e6 = 0.003, undoubled.
    assert!(
        (msg.usage.cost.total - 0.003).abs() < 1e-9,
        "an unrequested tier must price at 1x, got {}",
        msg.usage.cost.total
    );
}

#[tokio::test]
async fn events_after_the_terminal_are_not_decoded() {
    // pi's generator `return`s immediately after yielding the terminal (:747).
    const SSE: &str = concat!(
        "data: {\"type\":\"response.done\",\"response\":{\"id\":\"r\",\"status\":\"completed\"}}\n\n",
        "data: {\"type\":\"error\",\"message\":\"late\"}\n\n",
    );
    let events = drain(SSE, None).await;
    let last = events.last().expect("terminal");
    assert!(
        matches!(last, StreamEvent::Done { .. }),
        "a post-terminal event leaked through: {last:?}"
    );
}

#[tokio::test]
async fn a_codex_error_event_ends_the_turn_with_upstream_text() {
    const SSE: &str =
        "data: {\"type\":\"error\",\"code\":\"rate_limit\",\"message\":\"slow down\"}\n\n";
    let msg = drain(SSE, None)
        .await
        .last()
        .and_then(StreamEvent::terminal_message)
        .map(|m| (**m).clone())
        .expect("terminal");
    assert_eq!(msg.stop_reason, StopReason::Error);
    let text = msg.error_message.unwrap_or_default();
    assert!(
        text.contains("Codex error: slow down"),
        "expected pi's `Codex error: …` text, got {text}"
    );
}

#[tokio::test]
async fn malformed_sse_json_reports_the_codex_protocol_error() {
    const SSE: &str = "data: {not json}\n\n";
    let msg = drain(SSE, None)
        .await
        .last()
        .and_then(StreamEvent::terminal_message)
        .map(|m| (**m).clone())
        .expect("terminal");
    let text = msg.error_message.unwrap_or_default();
    assert!(
        text.contains("Invalid Codex SSE JSON"),
        "expected pi's CodexProtocolError text, got {text}"
    );
}

/// PROV-051. A Codex endpoint that accepts the TCP connection and never writes a byte must
/// terminate with pi's exact header-phase message (`openai-codex-responses.ts:412-413`
/// @v0.83.0), not with an unattributable transport error. Red before the fix: `opts.timeout_ms`
/// only became a reqwest `read_timeout`, so the terminal was
/// `transport error: … operation timed out` with no mention of the configured value.
#[tokio::test]
async fn a_stalled_header_phase_names_the_timeout_and_its_value() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    // Accept and hold the connection open, writing nothing.
    let held = tokio::spawn(async move {
        let mut sockets = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            sockets.push(sock);
        }
    });

    let mut model = codex_model("gpt-5-codex");
    model.base_url = format!("http://{addr}");
    let (sink, mut rx) = channel(64);
    // The key MUST be a real Codex JWT carrying the namespaced account claim: pi runs
    // `const accountId = extractAccountId(apiKey)` at `openai-codex-responses.ts:276` @v0.83.0,
    // BEFORE the request is built, and it throws on anything else. A bare `sk-test` therefore
    // terminates with "Failed to extract accountId from token" and the header phase is never
    // reached — the request under test never leaves the process.
    let token = fake_jwt(&json!({
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_timeout" },
    }));
    let opts = StreamOptions {
        api_key: Some(token.clone()),
        timeout_ms: Some(200),
        // One attempt, so the assertion is on the FIRST failure's text.
        max_retries: Some(0),
        ..Default::default()
    };
    let task = tokio::spawn(async move {
        CodexResponsesApi::new()
            .run(
                &model,
                &Context {
                    system_prompt: None,
                    messages: vec![cyrup_core::Message::User {
                        content: vec![cyrup_core::Content::text("hi")],
                        timestamp: 0,
                    }],
                    tools: Vec::new(),
                },
                &AuthResult::from_key(&token, "test"),
                &opts,
                CancelToken::new(),
                sink,
            )
            .await;
    });
    let mut events = Vec::new();
    while let Some(ev) = rx.recv().await {
        events.push(ev);
    }
    let _ = task.await;
    held.abort();

    let Some(StreamEvent::Error { error, .. }) = events.last() else {
        panic!("expected an error terminal, got {events:?}");
    };
    assert_eq!(
        error.error_message.as_deref(),
        Some("Codex SSE response headers timed out after 200ms")
    );
}
