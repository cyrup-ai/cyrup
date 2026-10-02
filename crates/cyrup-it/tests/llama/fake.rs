//! A loopback `llama-server` in router mode, complete enough for the whole llama.cpp chain.
//!
//! `cyrup-llama`'s own fake (`crates/cyrup-llama/src/tests/fake_server.rs`) is a `#[cfg(test)]`
//! module of that crate and speaks only the router MANAGEMENT api; `cyrup-provider`'s classifier
//! fake is likewise private to its crate and speaks only the three classifier endpoints. This suite
//! needs both halves behind ONE port, because the live provider derives every URL it uses from one
//! server root (`provider.ts:25-27`, `llamaInferenceUrl`): the catalog from `<root>/models` and
//! `<root>/props`, the chat stream from `<root>/v1/chat/completions` and the classifier from
//! `<root>/tokenize`, `/apply-template` and `/completion`.
//!
//! | request                      | answer                                                                |
//! |------------------------------|-----------------------------------------------------------------------|
//! | `GET /models`                | `{"object":"list","data":[<catalog>]}`                                |
//! | `GET /props[?model=<id>]`    | that model's props object, else the default one                       |
//! | `POST /v1/chat/completions`  | an SSE stream: the scripted reply in two deltas, a usage chunk, DONE  |
//! | `POST /tokenize`             | one token per character (the character code)                          |
//! | `POST /apply-template`       | `<|role|>\ncontent\n` per message, then `<|assistant|>\n`             |
//! | `POST /completion`           | the scripted next-token log-probabilities                             |
//! | anything else                | `404 {"error":{"message":"not found"}}`                               |
//!
//! Every connection is `connection: close`. Every request is recorded (method, target, lower-cased
//! headers, body) so a test asserts on what the client actually put on the wire, which is the only
//! thing a seam test can know about it.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// A chat template that mentions `enable_thinking`: llama-server reports it on `GET /props` for a
/// thinking-capable model (`provider.ts` `toPiModel`: `props?.chat_template?.includes(
/// "enable_thinking")`, `llama-extension.test.ts:109` uses this exact string).
pub const THINKING_TEMPLATE: &str = "{% if enable_thinking %}think{% endif %}";

/// A chat template without it.
pub const PLAIN_TEMPLATE: &str = "{{ messages }}";

/// A catalog entry `{"id", "status": {"value"}}` (`client.ts` `LlamaModelInfo`).
pub fn model(id: &str, status: &str) -> Value {
    json!({ "id": id, "status": { "value": status } })
}

/// [`model`] with extra top-level members merged in (`source`, `meta`, `architecture`).
pub fn model_with(id: &str, status: &str, extra: Value) -> Value {
    let mut entry = model(id, status);
    if let (Some(entry), Value::Object(extra)) = (entry.as_object_mut(), extra) {
        entry.extend(extra);
    }
    entry
}

/// One request the server received.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    /// The request target as sent, query included: `/props?model=qwen3`.
    pub target: String,
    /// Header `(name, value)` pairs, names lower-cased.
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Recorded {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or(&self.target)
    }

    pub fn query(&self) -> Option<&str> {
        self.target.split_once('?').map(|(_, query)| query)
    }

    /// One query parameter by exact name (no percent-decoding: ids in these tests are plain).
    pub fn query_param(&self, name: &str) -> Option<&str> {
        self.query()?
            .split('&')
            .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(candidate, _)| *candidate == name)
            .map(|(_, value)| value.as_str())
    }

    /// The body as JSON (`Value::Null` when it is not).
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

struct State {
    models: Vec<Value>,
    default_props: Value,
    props_by_model: BTreeMap<String, Value>,
    reply: String,
    /// Next-token `(token, log-probability)` pairs `POST /completion` answers, in rank order.
    next_tokens: Vec<(String, f64)>,
    requests: Vec<Recorded>,
}

struct Inner {
    state: Mutex<State>,
}

impl Inner {
    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The fake. Dropping it stops the listener.
pub struct FakeLlama {
    url: String,
    inner: Arc<Inner>,
    accept: JoinHandle<()>,
}

impl Drop for FakeLlama {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl FakeLlama {
    /// Bind `127.0.0.1:0` and serve `models` as the router catalog.
    pub async fn start(models: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");
        let inner = Arc::new(Inner {
            state: Mutex::new(State {
                models,
                default_props: json!({ "models_autoload": true, "chat_template": PLAIN_TEMPLATE }),
                props_by_model: BTreeMap::new(),
                reply: "Hello from llama.cpp".to_string(),
                next_tokens: vec![("B".to_string(), -0.3), ("A".to_string(), -1.5)],
                requests: Vec::new(),
            }),
        });
        let served = inner.clone();
        let accept = tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                tokio::spawn(handle_connection(served.clone(), socket));
            }
        });
        Self {
            url: format!("http://{addr}"),
            inner,
            accept,
        }
    }

    /// `http://127.0.0.1:<port>`: what `LLAMA_BASE_URL` is set to.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// `GET /props?model=<id>` answers `{"chat_template": template, "models_autoload": true}`.
    pub fn set_chat_template(&self, id: &str, template: &str) {
        self.inner.state().props_by_model.insert(
            id.to_string(),
            json!({ "models_autoload": true, "chat_template": template }),
        );
    }

    /// The text the chat stream answers with.
    pub fn set_reply(&self, reply: &str) {
        self.inner.state().reply = reply.to_string();
    }

    /// Everything received so far, in arrival order.
    pub fn requests(&self) -> Vec<Recorded> {
        self.inner.state().requests.clone()
    }

    /// The requests for `method` + `path` (query ignored).
    pub fn requests_to(&self, method: &str, path: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.method == method && request.path() == path)
            .collect()
    }

    /// Wait (up to 10 s) until `count` requests for `method` + `path` have arrived.
    pub async fn wait_for(&self, method: &str, path: &str, count: usize) -> Vec<Recorded> {
        for _ in 0..2000 {
            let seen = self.requests_to(method, path);
            if seen.len() >= count {
                return seen;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        panic!(
            "expected {count} `{method} {path}` request(s) within 10s, saw {:?}",
            self.requests()
                .iter()
                .map(|r| format!("{} {}", r.method, r.target))
                .collect::<Vec<_>>()
        );
    }
}

async fn write_json(socket: &mut TcpStream, status: u16, body: &str) {
    let head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(body.as_bytes()).await;
    let _ = socket.shutdown().await;
}

/// Read one HTTP/1.1 request: head up to the blank line, then `content-length` body bytes.
async fn read_request(socket: &mut TcpStream) -> Option<Recorded> {
    let mut data: Vec<u8> = Vec::new();
    let head_end = loop {
        if let Some(position) = data.windows(4).position(|window| window == b"\r\n\r\n") {
            break position;
        }
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        data.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&data[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    let length = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = data[head_end + 4..].to_vec();
    while body.len() < length {
        let mut chunk = [0_u8; 4096];
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..read]);
    }
    Some(Recorded {
        method,
        target,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
    })
}

async fn handle_connection(inner: Arc<Inner>, mut socket: TcpStream) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    inner.state().requests.push(request.clone());
    let body = request.json();
    match (request.method.as_str(), request.path()) {
        ("GET", "/models") => {
            let models = inner.state().models.clone();
            write_json(
                &mut socket,
                200,
                &json!({ "object": "list", "data": models }).to_string(),
            )
            .await;
        }
        ("GET", "/props") => {
            let requested = request.query_param("model").map(str::to_string);
            let props = {
                let state = inner.state();
                requested
                    .and_then(|id| state.props_by_model.get(&id).cloned())
                    .unwrap_or_else(|| state.default_props.clone())
            };
            write_json(&mut socket, 200, &props.to_string()).await;
        }
        ("POST", "/v1/chat/completions") => {
            let reply = inner.state().reply.clone();
            serve_chat_stream(&mut socket, &reply).await;
        }
        ("POST", "/tokenize") => {
            let content = body["content"].as_str().unwrap_or_default();
            let tokens: Vec<Value> = content.chars().map(|c| json!(u32::from(c))).collect();
            write_json(&mut socket, 200, &json!({ "tokens": tokens }).to_string()).await;
        }
        ("POST", "/apply-template") => {
            let rendered: String = body["messages"]
                .as_array()
                .map(|messages| {
                    messages
                        .iter()
                        .map(|message| {
                            format!(
                                "<|{}|>\n{}\n",
                                message["role"].as_str().unwrap_or_default(),
                                message["content"].as_str().unwrap_or_default()
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            write_json(
                &mut socket,
                200,
                &json!({ "prompt": format!("{rendered}<|assistant|>\n") }).to_string(),
            )
            .await;
        }
        ("POST", "/completion") => {
            let next = inner.state().next_tokens.clone();
            let top: Vec<Value> = next
                .iter()
                .map(|(token, logprob)| {
                    json!({
                        "id": token.chars().next().map(u32::from),
                        "token": token,
                        "bytes": [],
                        "logprob": logprob,
                    })
                })
                .collect();
            write_json(
                &mut socket,
                200,
                &json!({
                    "content": "A",
                    "completion_probabilities": [{ "id": 65, "token": "A", "top_logprobs": top }],
                })
                .to_string(),
            )
            .await;
        }
        _ => {
            write_json(
                &mut socket,
                404,
                &json!({ "error": { "message": "not found" } }).to_string(),
            )
            .await;
        }
    }
}

/// An OpenAI-style SSE stream: the reply split in two content deltas, a `stop` chunk carrying the
/// usage, then `[DONE]`. The close delimits the body.
async fn serve_chat_stream(socket: &mut TcpStream, reply: &str) {
    let split = reply
        .char_indices()
        .nth(reply.chars().count() / 2)
        .map_or(reply.len(), |(index, _)| index);
    let (first, second) = reply.split_at(split);
    let chunks = [
        json!({ "id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [{
            "index": 0, "delta": { "role": "assistant", "content": first }, "finish_reason": null }] }),
        json!({ "id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [{
            "index": 0, "delta": { "content": second }, "finish_reason": null }] }),
        json!({ "id": "chatcmpl-1", "object": "chat.completion.chunk",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }],
            "usage": { "prompt_tokens": 3, "completion_tokens": 4, "total_tokens": 7 } }),
    ];
    let mut out = String::from(
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n",
    );
    for chunk in chunks {
        out.push_str(&format!("data: {chunk}\n\n"));
    }
    out.push_str("data: [DONE]\n\n");
    let _ = socket.write_all(out.as_bytes()).await;
    let _ = socket.shutdown().await;
}
