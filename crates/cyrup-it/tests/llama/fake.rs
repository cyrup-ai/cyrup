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
//! | `GET /models`                | `router::models_envelope` over the catalog                            |
//! | `GET /props`                 | `router::router_props` (autoload on): the ROUTER's own props          |
//! | `GET /props?model=<id>`      | `router::child_props` with that model's chat template                 |
//! | `POST /v1/chat/completions`  | an SSE stream: the scripted reply in two deltas, a usage chunk, DONE  |
//! | `POST /tokenize`             | `classify::tokenize`, one token per character (the character code)   |
//! | `POST /apply-template`       | `classify::apply_template` of `<|role|>\ncontent\n` per message, then `<|assistant|>\n` |
//! | `POST /completion`           | `classify::completion` over the scripted next-token log-probabilities |
//! | anything else                | `router::file_not_found`, llama.cpp's own unknown-route 404           |
//!
//! Every answer but the chat stream is built by `cyrup_llama_cpp_wire` (`router`, `classify`): ONE
//! definition of the llama.cpp wire, each shape cited into llama.cpp's server source at `b11436`,
//! shared with `cyrup-llama`'s fake (EXT-108). What the fake decides is only the CONTENT — which
//! models, which template, which tokens rank where; the shapes are not its own. The unknown-route
//! 404 was `{"error":{"message":"not found"}}` here until EXT-108, a shape no llama.cpp sends.
//! [`drift_guard`] fails if what this fake writes on the socket stops being that crate's pinned
//! bytes.
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

use cyrup_llama_cpp_wire::classify::{self, TokenLogprob};
use cyrup_llama_cpp_wire::router;
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
    /// The `chat_template` each model's (proxied, child) `GET /props?model=` reports;
    /// [`PLAIN_TEMPLATE`] for any model not in here.
    templates: BTreeMap<String, String>,
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
                templates: BTreeMap::new(),
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

    /// `GET /props?model=<id>` answers the child's props with this `chat_template`.
    pub fn set_chat_template(&self, id: &str, template: &str) {
        self.inner
            .state()
            .templates
            .insert(id.to_string(), template.to_string());
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
                &router::models_envelope(models).to_string(),
            )
            .await;
        }
        ("GET", "/props") => {
            // No `model`: the ROUTER answers its own props, which carry `models_autoload` and no
            // `chat_template`. A `model`: the router proxies to that child, whose props carry
            // `chat_template` and no `models_autoload` (`router::child_props`). A real router
            // refuses `?autoload=false` for a model that is not running; this fake answers for
            // any, because the provider only asks about `loaded` ones.
            let props = match request.query_param("model") {
                None => router::router_props(true),
                Some(id) => {
                    let template = inner
                        .state()
                        .templates
                        .get(id)
                        .cloned()
                        .unwrap_or_else(|| PLAIN_TEMPLATE.to_string());
                    router::child_props(&template)
                }
            };
            write_json(&mut socket, 200, &props.to_string()).await;
        }
        ("POST", "/v1/chat/completions") => {
            let reply = inner.state().reply.clone();
            serve_chat_stream(&mut socket, &reply).await;
        }
        ("POST", "/tokenize") => {
            let content = body["content"].as_str().unwrap_or_default();
            let tokens: Vec<i64> = content.chars().map(|c| i64::from(u32::from(c))).collect();
            write_json(&mut socket, 200, &classify::tokenize(&tokens).to_string()).await;
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
                &classify::apply_template(&format!("{rendered}<|assistant|>\n")).to_string(),
            )
            .await;
        }
        ("POST", "/completion") => {
            let next = inner.state().next_tokens.clone();
            // A token's id is its first character's code, the same vocabulary `/tokenize` uses.
            let top: Vec<TokenLogprob<'_>> = next
                .iter()
                .map(|(token, logprob)| TokenLogprob {
                    id: token.chars().next().map_or(0, |c| i64::from(u32::from(c))),
                    token,
                    logprob: *logprob,
                })
                .collect();
            let prompt = body["prompt"].as_str().unwrap_or_default();
            // At `temperature: 0` the sampled token is the top candidate.
            let answer = match top.first() {
                Some(sampled) => classify::completion(
                    body["model"].as_str().unwrap_or_default(),
                    prompt,
                    prompt.chars().count() as u64,
                    *sampled,
                    &top,
                ),
                None => json!({}),
            };
            write_json(&mut socket, 200, &answer.to_string()).await;
        }
        _ => {
            let (status, body) = router::file_not_found();
            write_json(&mut socket, status, &body.to_string()).await;
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

/// EXT-108's DRIFT GUARD for this fake: what [`FakeLlama`] writes on a real socket is, byte for
/// byte, the pinned llama.cpp answer in `cyrup_llama_cpp_wire::golden`. The fake answers from
/// `cyrup_llama_cpp_wire::{router, classify}`, so a change to any definition it serves fails this
/// until the golden moves with it.
mod drift_guard {
    use cyrup_llama_cpp_wire::golden;
    use serde_json::Value;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::FakeLlama;

    /// One raw HTTP/1.1 exchange; the fake closes every connection, so the close ends the body.
    async fn raw(url: &str, method: &str, path: &str, body: &str) -> (u16, String) {
        let addr = url.trim_start_matches("http://");
        let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
        let request = format!(
            "{method} {path} HTTP/1.1\r\nhost: {addr}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(request.as_bytes()).await.unwrap();
        let mut answer = String::new();
        socket.read_to_string(&mut answer).await.unwrap();
        let (head, body) = answer.split_once("\r\n\r\n").unwrap();
        (
            head.split(' ').nth(1).unwrap().parse().unwrap(),
            body.to_string(),
        )
    }

    #[tokio::test]
    async fn the_fake_writes_the_golden_bytes() {
        let fake = FakeLlama::start(Vec::new()).await;
        let url = fake.url().to_string();
        let answer = |status: u16, body: &str| (status, body.to_string());

        assert_eq!(
            raw(&url, "GET", "/no-such-route", "").await,
            answer(404, golden::FILE_NOT_FOUND),
            "an unknown route is llama.cpp's own 404 literal (server-http.cpp:199-212 @b11436)"
        );
        assert_eq!(
            raw(&url, "GET", "/models", "").await,
            answer(200, golden::EMPTY_MODELS)
        );
        assert_eq!(
            raw(&url, "GET", "/props", "").await,
            answer(200, golden::ROUTER_PROPS_AUTOLOAD),
            "the router's own props"
        );
        assert_eq!(
            raw(
                &url,
                "POST",
                "/tokenize",
                r#"{"model":"qwen","content":"AB"}"#
            )
            .await,
            answer(200, golden::TOKENIZE_65_66)
        );
        assert_eq!(
            raw(
                &url,
                "POST",
                "/apply-template",
                r#"{"model":"qwen","messages":[{"role":"user","content":"hi"}]}"#
            )
            .await,
            answer(200, golden::APPLY_TEMPLATE_USER_HI)
        );

        // The default script ranks `B` (-0.3) over `A` (-1.5).
        let (status, body) = raw(
            &url,
            "POST",
            "/completion",
            r#"{"model":"qwen","prompt":"p","n_predict":1,"n_probs":2,"post_sampling_probs":false}"#,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        let body: Value = serde_json::from_str(&body).unwrap();
        let keys: Vec<&str> = body
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            golden::COMPLETION_KEYS,
            "every top-level key, in order"
        );
        assert_eq!(
            body["completion_probabilities"].to_string(),
            golden::COMPLETION_PROBABILITIES_B_OVER_A
        );
    }
}
