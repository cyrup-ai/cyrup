//! A loopback HTTP server for driver-level tests: it answers every request with the same
//! `text/event-stream` body and records each request it was sent (head and body).
//!
//! The workspace `tokio` has `net` only as a dev feature of this crate, so this is `#[cfg(test)]`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// The recorded requests, each as `(head, body)`.
pub(crate) type Seen = Arc<Mutex<Vec<(String, String)>>>;

/// Serve `body` as an SSE response to every connection. Returns the server's origin
/// (`http://127.0.0.1:<port>`) and the requests seen so far.
pub(crate) async fn serve_sse(body: String) -> (String, Seen) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut chunk = vec![0u8; 16384];
            let start = loop {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break None;
                }
                raw.extend_from_slice(&chunk[..n]);
                if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    break Some(at + 4);
                }
            };
            let Some(start) = start else { continue };
            let head = String::from_utf8_lossy(&raw[..start]).to_string();
            let length: usize = head
                .to_lowercase()
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").map(str::to_string))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0);
            while raw.len() < start + length {
                let n = sock.read(&mut chunk).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&chunk[..n]);
            }
            let request_body = String::from_utf8_lossy(&raw[start..]).to_string();
            sink.lock().unwrap().push((head, request_body));
            let reply = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(reply.as_bytes()).await;
            let _ = sock.flush().await;
        }
    });
    (format!("http://{addr}"), seen)
}

/// The Responses SSE for one grammar-tool call: `custom_tool_call` `call_1|ctc_1` to
/// `sample_tool` whose raw text `abc` arrives as one delta.
pub(crate) fn custom_tool_call_sse(terminal: &str) -> String {
    let item = |input: &str| {
        serde_json::json!({
            "type": "custom_tool_call", "id": "ctc_1", "call_id": "call_1",
            "name": "sample_tool", "input": input,
        })
    };
    let events = [
        serde_json::json!({ "type": "response.created", "response": { "id": "resp_1" } }),
        serde_json::json!({ "type": "response.output_item.added", "output_index": 0, "item": item("") }),
        serde_json::json!({ "type": "response.custom_tool_call_input.delta", "output_index": 0, "delta": "abc" }),
        serde_json::json!({ "type": "response.custom_tool_call_input.done", "output_index": 0, "input": "abc" }),
        serde_json::json!({ "type": "response.output_item.done", "output_index": 0, "item": item("abc") }),
        serde_json::json!({
            "type": terminal,
            "response": { "id": "resp_1", "status": "completed",
                          "usage": { "input_tokens": 1, "output_tokens": 1, "total_tokens": 2 } },
        }),
    ];
    events.iter().map(|e| format!("data: {e}\n\n")).collect()
}

/// The `sample_tool` declaration the driver tests send: a single required string `payload`, with a
/// Lark grammar.
pub(crate) fn grammar_tool_def() -> crate::context::ToolDef {
    use crate::context::{
        ConstrainedSampling, ConstrainedSamplingConfig, GrammarVariants, ToolDef,
    };
    ToolDef {
        name: "sample_tool".into(),
        description: "d".into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": { "payload": { "type": "string" } },
            "required": ["payload"],
        }),
        constrained_sampling: Some(ConstrainedSampling::Config(
            ConstrainedSamplingConfig::Grammar {
                variants: GrammarVariants {
                    openai_lark: Some("start: /[a-z]+/".to_string()),
                    openai_regex: None,
                },
            },
        )),
    }
}
