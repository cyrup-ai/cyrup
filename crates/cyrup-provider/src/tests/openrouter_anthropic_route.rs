//! PROV-099 — OpenRouter `anthropic/*` models are routed over `anthropic-messages`.
//!
//! pi's generator sends every OpenRouter `anthropic/*` row that is not a `:batch` row over
//! `anthropic-messages` at `https://openrouter.ai/api`, and every other row over
//! `openai-completions` at `https://openrouter.ai/api/v1`
//! (`packages/ai/scripts/openrouter-catalog.ts:66,78-79` @v0.87.1: `useAnthropicMessages =
//! /^anthropic\//.test(model.id) && !model.id.endsWith(":batch")`). `providers/openrouter.ts:8,21-25`
//! registers the provider with both apis.
//!
//! cyrup's embedded `openrouter.json` carries that routing, `providers/fleet.rs` declares
//! `openrouter` as `FleetWire::MessagesAndCompletions`, and `anthropic_messages::headers::messages_url`
//! appends `/v1/messages` to the base, which is why the base has no `/v1`. These tests pin it:
//!
//!  * the catalog-data test fails if a row's api or base URL drifts (a catalog refresh that
//!    regresses the split, or a hand edit);
//!  * the wire test takes a real `anthropic/*` row from the embedded catalog, points its base URL at
//!    a loopback origin and asserts the request is `POST /api/v1/messages` with an Anthropic
//!    Messages body, which is the ledger row's own Verify.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::collection::{CreateModelsOptions, Models, create_models};
use crate::{AuthResult, Context, Model, StreamOptions, all_providers, channel};
use cyrup_core::{ApiId, CancelToken, Content, Message};

const ANTHROPIC_BASE: &str = "https://openrouter.ai/api";
const COMPLETIONS_BASE: &str = "https://openrouter.ai/api/v1";
/// A real, non-`:batch` row of the embedded catalog.
const SAMPLE_ROW: &str = "anthropic/claude-sonnet-5";

fn selection() -> Models {
    let mut models = create_models(CreateModelsOptions::default());
    for p in all_providers() {
        models.set_provider(p);
    }
    models
}

fn openrouter_rows() -> Vec<Model> {
    selection().get_models(Some("openrouter"))
}

// ------------------------------------------------------------------------------ catalog data --

#[test]
fn openrouter_anthropic_rows_use_messages_and_batch_rows_stay_on_completions() {
    let rows = openrouter_rows();
    assert!(!rows.is_empty(), "the embedded openrouter catalog is empty");

    let mut messages_rows = 0usize;
    let mut batch_rows = 0usize;
    for m in &rows {
        let id = m.id.as_str();
        // `/^anthropic\//` — a leading `~` (the `~anthropic/claude-*-latest` aliases) does not match.
        let is_anthropic = id.starts_with("anthropic/");
        let is_batch = id.ends_with(":batch");
        if is_anthropic && !is_batch {
            messages_rows += 1;
            assert_eq!(
                m.api.as_str(),
                "anthropic-messages",
                "{id}: openrouter-catalog.ts:78 sends every non-:batch anthropic/* row over anthropic-messages"
            );
            assert_eq!(
                m.base_url, ANTHROPIC_BASE,
                "{id}: openrouter-catalog.ts:79 — base has no /v1; messages_url appends /v1/messages"
            );
        } else {
            if is_anthropic && is_batch {
                batch_rows += 1;
            }
            assert_eq!(
                m.api.as_str(),
                "openai-completions",
                "{id}: every other openrouter row stays on openai-completions"
            );
            assert_eq!(m.base_url, COMPLETIONS_BASE, "{id}: completions base URL");
        }
    }
    // The split must actually be exercised on both sides, not vacuously satisfied.
    assert!(
        messages_rows > 0,
        "no non-:batch anthropic/* row in the catalog"
    );
    assert!(batch_rows > 0, "no anthropic/*:batch row in the catalog");
}

// -------------------------------------------------------------------------------- the wire --

/// `true` once `acc` holds a complete HTTP/1.1 request (head + declared `Content-Length` body).
fn request_is_complete(acc: &[u8]) -> bool {
    let Some(head_end) = acc.windows(4).position(|w| w == b"\r\n\r\n") else {
        return false;
    };
    let head_end = head_end + 4;
    let head = String::from_utf8_lossy(&acc[..head_end]).to_lowercase();
    let len = head.lines().find_map(|line| {
        line.strip_prefix("content-length:")
            .and_then(|v| v.trim().parse::<usize>().ok())
    });
    match len {
        Some(n) => acc.len() >= head_end + n,
        None => true,
    }
}

/// Record every full request (head and body) and answer each with an empty `text/event-stream`.
fn spawn_capture_origin() -> (String, Arc<Mutex<Vec<Vec<u8>>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}");
    let requests: Arc<Mutex<Vec<Vec<u8>>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = requests.clone();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(20)));
            let mut buf = [0u8; 8192];
            let mut acc: Vec<u8> = Vec::new();
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        if request_is_complete(&acc) {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            if let Ok(mut g) = sink.lock() {
                g.push(acc);
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            );
            let _ = stream.flush();
            let _ = stream.shutdown(std::net::Shutdown::Write);
        }
    });
    (url, requests)
}

/// PROV-099 Verify. A real `anthropic/*` OpenRouter row, base URL swapped for a loopback origin's
/// `/api`, streams through its registered api impl: the request line is `POST /api/v1/messages`
/// and the body is an Anthropic Messages body.
#[tokio::test]
async fn an_openrouter_anthropic_row_streams_to_api_v1_messages_with_a_messages_body() {
    let mut model = selection()
        .get_model("openrouter", SAMPLE_ROW)
        .unwrap_or_else(|| panic!("the embedded catalog has no openrouter/{SAMPLE_ROW}"));
    let (origin, requests) = spawn_capture_origin();
    // The real row's base is `https://openrouter.ai/api`; keep its path, swap the origin.
    let real_base = model.base_url.clone();
    let path = real_base
        .strip_prefix("https://openrouter.ai")
        .unwrap_or_else(|| panic!("unexpected openrouter base URL {real_base}"));
    model.base_url = format!("{origin}{path}");
    assert!(
        model.base_url.ends_with("/api"),
        "the catalog base should be `.../api`, got {real_base}"
    );

    let registry = crate::api::builtin_registry();
    let api = registry
        .get(&ApiId::from(model.api.as_str()))
        .unwrap_or_else(|| panic!("api '{}' is registered", model.api.as_str()));

    // Pin proxy resolution off so an ambient `HTTP_PROXY` cannot send the request off-box.
    let mut env = crate::ProviderEnv::new();
    env.insert("no_proxy".to_string(), "*".to_string());
    let mut auth = AuthResult::from_key("test-key-not-a-real-credential", "test");
    auth.env = Some(env);

    let ctx = Context {
        system_prompt: Some("be brief".to_string()),
        messages: vec![Message::User {
            content: vec![Content::text("hello")],
            timestamp: 0,
        }],
        tools: Vec::new(),
    };
    let opts = StreamOptions::default();
    let (sink, mut rx) = channel(64);
    let task_model = model.clone();
    let task = tokio::spawn(async move {
        api.run(&task_model, &ctx, &auth, &opts, CancelToken::new(), sink)
            .await;
    });
    while rx.recv().await.is_some() {}
    task.await.expect("api task");

    let raw = requests
        .lock()
        .ok()
        .and_then(|g| g.first().cloned())
        .unwrap_or_default();
    assert!(
        !raw.is_empty(),
        "no request reached the loopback origin (api '{}')",
        model.api.as_str()
    );
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let request_line = head.lines().next().unwrap_or_default();
    assert_eq!(
        request_line, "POST /api/v1/messages HTTP/1.1",
        "anthropic-messages posts to <base>/v1/messages; request head was:\n{head}"
    );

    let json: serde_json::Value = serde_json::from_str(body)
        .unwrap_or_else(|e| panic!("request body is not JSON ({e}):\n{body}"));
    assert_eq!(json["model"], SAMPLE_ROW);
    assert_eq!(
        json["stream"],
        serde_json::Value::Bool(true),
        "Messages body: stream"
    );
    assert!(
        json["max_tokens"].is_u64(),
        "Messages body: top-level max_tokens, got {json}"
    );
    // The Messages `system` is top-level (string or block list); Chat Completions would put it in
    // `messages[0]` with role `system`/`developer`.
    assert!(
        json.get("system").is_some(),
        "Messages body: top-level system, got {json}"
    );
    let messages = json["messages"].as_array().expect("messages array");
    assert_eq!(messages.len(), 1, "only the user turn: {json}");
    assert_eq!(messages[0]["role"], "user");
    let first_block = &messages[0]["content"];
    let text_of_user = if first_block.is_string() {
        first_block.as_str().map(str::to_string)
    } else {
        first_block
            .as_array()
            .and_then(|a| a.first())
            .filter(|b| b["type"] == "text")
            .and_then(|b| b["text"].as_str().map(str::to_string))
    };
    assert_eq!(
        text_of_user.as_deref(),
        Some("hello"),
        "user content: {json}"
    );
    // No Chat Completions-only members.
    for key in ["stream_options", "max_completion_tokens", "store"] {
        assert!(
            json.get(key).is_none(),
            "Chat Completions member '{key}' in a Messages body: {json}"
        );
    }
}
