//! A loopback `llama-server` for the `llama-cpp-classify` tests, and the fixtures they share.
//!
//! pi's tests (`packages/ai/test/llama-cpp-classify.test.ts:30-79` @v0.99.2-17) hand `classify` a
//! fake `fetch`. cyrup has no fetch seam, so the base URL is the seam: this serves the three
//! endpoints the api uses (`/tokenize`, `/apply-template`, `/completion`) over a raw loopback
//! `TcpListener` on `127.0.0.1:0`, records every request, and lets a test intercept any request to
//! answer it differently (an error status, a malformed body, a hang).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::api::llama_cpp_classify::llama_cpp_classify_api;
use crate::classifier::{
    BoolCriteria, ClassifierContext, ClassifierModel, ClassifierOptions, ClassifierQuestion,
    ClassifierResult, OrderedMap,
};
use crate::{Modality, ModelCost, ProviderEnv};

/// The api id of every model in these tests.
pub(super) const API: &str = "llama-cpp-classify";

/// One request the fake server received.
#[derive(Clone, Debug)]
pub(super) struct Recorded {
    pub path: String,
    pub body: Value,
    /// Lower-cased header names.
    pub headers: Vec<(String, String)>,
}

impl Recorded {
    pub(super) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate == name)
            .map(|(_, value)| value.as_str())
    }
}

/// How the fake server answers one request.
#[derive(Clone, Debug)]
pub(super) enum Reply {
    /// `200` with this JSON body.
    Json(Value),
    /// Any status, extra headers and a verbatim body.
    Raw {
        status: u16,
        headers: Vec<(String, String)>,
        body: String,
    },
    /// Accept the request and never answer.
    Hang,
}

impl Reply {
    pub(super) fn status(status: u16, body: &str) -> Self {
        Reply::Raw {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    pub(super) fn status_with(status: u16, headers: &[(&str, &str)], body: &str) -> Self {
        Reply::Raw {
            status,
            headers: headers
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
            body: body.to_string(),
        }
    }
}

type NextFn = Arc<dyn Fn(&str, u64) -> Vec<(String, f64)> + Send + Sync>;
type TemplateFn = Arc<dyn Fn(&[(String, String)]) -> String + Send + Sync>;
type TokenizeFn = Arc<dyn Fn(&str) -> Vec<u32> + Send + Sync>;
/// `(request, how many requests to the same path came before it)`.
type InterceptFn = Arc<dyn Fn(&Recorded, usize) -> Option<Reply> + Send + Sync>;

/// What the fake server does; every field has pi's default (`fakeServer`, test:48-79).
#[derive(Clone, Default)]
pub(super) struct Behavior {
    /// Log-probabilities of the next token by token text, in rank order, given the prompt and
    /// `n_probs`. Default: `A -0.1`, `B -2.5`.
    next: Option<NextFn>,
    template: Option<TemplateFn>,
    tokenize: Option<TokenizeFn>,
    /// Answer `/tokenize` with `{ id }` objects instead of bare ids.
    token_objects: bool,
    intercept: Option<InterceptFn>,
}

impl Behavior {
    pub(super) fn next(
        mut self,
        next: impl Fn(&str, u64) -> Vec<(String, f64)> + Send + Sync + 'static,
    ) -> Self {
        self.next = Some(Arc::new(next));
        self
    }

    pub(super) fn template(
        mut self,
        template: impl Fn(&[(String, String)]) -> String + Send + Sync + 'static,
    ) -> Self {
        self.template = Some(Arc::new(template));
        self
    }

    pub(super) fn tokenize(
        mut self,
        tokenize: impl Fn(&str) -> Vec<u32> + Send + Sync + 'static,
    ) -> Self {
        self.tokenize = Some(Arc::new(tokenize));
        self
    }

    pub(super) fn token_objects(mut self) -> Self {
        self.token_objects = true;
        self
    }

    /// Answer a request differently; `None` falls through to the normal behaviour.
    pub(super) fn intercept(
        mut self,
        intercept: impl Fn(&Recorded, usize) -> Option<Reply> + Send + Sync + 'static,
    ) -> Self {
        self.intercept = Some(Arc::new(intercept));
        self
    }
}

/// Token ids: one per character, the character code (test:43-46).
pub(super) fn char_tokens(content: &str) -> Vec<u32> {
    content.chars().map(u32::from).collect()
}

/// Maps multi-character labels to single tokens, as a real vocabulary would (test:105-114).
pub(super) fn word_tokens(content: &str) -> Vec<u32> {
    let mut tokens = Vec::new();
    for (index, part) in content.split('\n').enumerate() {
        if index > 0 {
            tokens.push(u32::from('\n'));
        }
        match part {
            "" => {}
            "Yes" => tokens.push(89),
            "No" => tokens.push(78),
            other => tokens.extend(char_tokens(other)),
        }
    }
    tokens
}

/// Completion log-probabilities for bool (Yes/No) and letter labels; the fake tokenizer maps a
/// label to its first character (test:81-86).
pub(super) fn answer_by_prompt(prompt: &str, _depth: u64) -> Vec<(String, f64)> {
    let entries: &[(&str, f64)] = if prompt.contains("Answer Yes or No.") {
        &[("Y", -0.05), ("N", -3.0)]
    } else if prompt.contains("Answer with one level number.") {
        &[("2", -0.2), ("1", -1.8), ("0", -4.0)]
    } else {
        &[("B", -0.3), ("A", -1.5), ("C", -3.0)]
    };
    entries
        .iter()
        .map(|(token, logprob)| ((*token).to_string(), *logprob))
        .collect()
}

/// A running fake server.
pub(super) struct FakeServer {
    /// `http://127.0.0.1:<port>/v1`, the shape pi's llama.cpp models use.
    pub base_url: String,
    requests: Arc<Mutex<Vec<Recorded>>>,
}

/// Listeners kept bound for the life of the process. The label-token cache is keyed by server root
/// (pi's tests use a fresh host per test for the same reason, test:12-14), so a port freed by one
/// test and handed to another would replay the first test's tokenizer.
static HELD_PORTS: Mutex<Vec<std::net::TcpListener>> = Mutex::new(Vec::new());

impl FakeServer {
    pub(super) async fn start() -> Self {
        Self::with(Behavior::default()).await
    }

    pub(super) async fn with(behavior: Behavior) -> Self {
        let std_listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        std_listener.set_nonblocking(true).expect("nonblocking");
        let addr = std_listener.local_addr().expect("local addr");
        HELD_PORTS
            .lock()
            .unwrap()
            .push(std_listener.try_clone().expect("clone listener"));
        let listener = TcpListener::from_std(std_listener).expect("tokio listener");
        let requests: Arc<Mutex<Vec<Recorded>>> = Arc::new(Mutex::new(Vec::new()));
        let log = requests.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                tokio::spawn(serve(stream, behavior.clone(), log.clone()));
            }
        });
        Self {
            base_url: format!("http://{addr}/v1"),
            requests,
        }
    }

    /// Every request received so far, in arrival order.
    pub(super) fn requests(&self) -> Vec<Recorded> {
        self.requests.lock().unwrap().clone()
    }

    /// The requests to one path.
    pub(super) fn requests_to(&self, path: &str) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|request| request.path == path)
            .collect()
    }

    /// `n_probs` of every `/completion` request (test:401-405).
    pub(super) fn completion_depths(&self) -> Vec<u64> {
        self.requests_to("/completion")
            .iter()
            .map(|request| request.body["n_probs"].as_u64().unwrap())
            .collect()
    }
}

async fn serve(mut stream: TcpStream, behavior: Behavior, log: Arc<Mutex<Vec<Recorded>>>) {
    let Some(request) = read_request(&mut stream).await else {
        return;
    };
    let nth = {
        let mut log = log.lock().unwrap();
        let nth = log
            .iter()
            .filter(|earlier| earlier.path == request.path)
            .count();
        log.push(request.clone());
        nth
    };
    let reply = behavior
        .intercept
        .as_ref()
        .and_then(|intercept| intercept(&request, nth))
        .unwrap_or_else(|| respond(&behavior, &request));
    let (status, headers, body) = match reply {
        Reply::Json(value) => (200, Vec::new(), value.to_string()),
        Reply::Raw {
            status,
            headers,
            body,
        } => (status, headers, body),
        Reply::Hang => {
            tokio::time::sleep(Duration::from_secs(60)).await;
            return;
        }
    };
    let mut head = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Read one HTTP/1.1 request: the head, then `Content-Length` bytes of body.
async fn read_request(stream: &mut TcpStream) -> Option<Recorded> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let path = request_line.split(' ').nth(1)?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_lowercase(), value.trim().to_string()))
        .collect();
    let length: usize = headers
        .iter()
        .find(|(name, _)| name == "content-length")
        .and_then(|(_, value)| value.parse().ok())
        .unwrap_or(0);
    while buffer.len() < head_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body = serde_json::from_slice(&buffer[head_end..head_end + length]).unwrap_or(Value::Null);
    Some(Recorded {
        path,
        body,
        headers,
    })
}

/// The normal answer of each endpoint (test:54-76).
fn respond(behavior: &Behavior, request: &Recorded) -> Reply {
    match request.path.as_str() {
        "/tokenize" => {
            let content = request.body["content"].as_str().unwrap_or_default();
            let ids = match &behavior.tokenize {
                Some(tokenize) => tokenize(content),
                None => char_tokens(content),
            };
            let tokens: Vec<Value> = ids
                .into_iter()
                .map(|id| {
                    if behavior.token_objects {
                        json!({ "id": id })
                    } else {
                        json!(id)
                    }
                })
                .collect();
            Reply::Json(json!({ "tokens": tokens }))
        }
        "/apply-template" => {
            let messages: Vec<(String, String)> = request.body["messages"]
                .as_array()
                .map(|messages| {
                    messages
                        .iter()
                        .map(|message| {
                            (
                                message["role"].as_str().unwrap_or_default().to_string(),
                                message["content"].as_str().unwrap_or_default().to_string(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            let prompt = match &behavior.template {
                Some(template) => template(&messages),
                None => {
                    let rendered: String = messages
                        .iter()
                        .map(|(role, content)| format!("<|{role}|>\n{content}\n"))
                        .collect();
                    format!("{rendered}<|assistant|>\n")
                }
            };
            Reply::Json(json!({ "prompt": prompt }))
        }
        "/completion" => {
            let prompt = request.body["prompt"].as_str().unwrap_or_default();
            let depth = request.body["n_probs"].as_u64().unwrap_or_default();
            let next = match &behavior.next {
                Some(next) => next(prompt, depth),
                None => vec![("A".to_string(), -0.1), ("B".to_string(), -2.5)],
            };
            let top_logprobs: Vec<Value> = next
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
            Reply::Json(json!({
                "content": "A",
                "completion_probabilities": [{ "id": 65, "token": "A", "top_logprobs": top_logprobs }],
            }))
        }
        _ => Reply::status(404, "not found"),
    }
}

// ----------------------------------------------------------------------------------- fixtures --

/// A classifier model on `base_url`; the id is `qwen` as in pi's tests (test:15-28).
pub(super) fn model(base_url: &str) -> ClassifierModel {
    ClassifierModel {
        id: "qwen".into(),
        name: "qwen".into(),
        api: API.into(),
        provider: "llama.cpp".into(),
        base_url: base_url.to_string(),
        input: vec![Modality::Text],
        input_limits: None,
        cost: ModelCost::default(),
        headers: None,
        context_window: 32768,
    }
}

/// Default options with proxy resolution pinned off, so a developer's ambient `HTTP_PROXY` cannot
/// send the loopback request off-box (`tests/transform_headers_on_the_wire.rs`, `env_with`).
pub(super) fn options() -> ClassifierOptions {
    let mut env = ProviderEnv::new();
    env.insert("no_proxy".to_string(), "*".to_string());
    ClassifierOptions {
        env: Some(env),
        ..ClassifierOptions::default()
    }
}

/// Run the api under test.
pub(super) async fn classify(
    model: &ClassifierModel,
    context: &ClassifierContext,
    options: &ClassifierOptions,
) -> ClassifierResult {
    llama_cpp_classify_api()
        .classify(model, context, options)
        .await
}

pub(super) fn choice(instructions: &str, criteria: &[(&str, &str)]) -> ClassifierQuestion {
    ClassifierQuestion::Choice {
        instructions: instructions.to_string(),
        criteria: criteria
            .iter()
            .map(|(key, description)| (*key, (*description).to_string()))
            .collect::<OrderedMap<String>>(),
    }
}

pub(super) fn score(instructions: &str, levels: &[&str]) -> ClassifierQuestion {
    ClassifierQuestion::Score {
        instructions: instructions.to_string(),
        criteria: levels.iter().map(|level| (*level).to_string()).collect(),
    }
}

pub(super) fn boolean(instructions: &str, when_true: &str, when_false: &str) -> ClassifierQuestion {
    ClassifierQuestion::Bool {
        instructions: instructions.to_string(),
        criteria: BoolCriteria {
            when_true: when_true.to_string(),
            when_false: when_false.to_string(),
        },
    }
}

/// A context with an empty state and these questions.
pub(super) fn context_of(questions: Vec<(&str, ClassifierQuestion)>) -> ClassifierContext {
    ClassifierContext {
        state: serde_json::Map::new(),
        images: None,
        questions: questions.into_iter().collect(),
    }
}

/// An empty-state context with one two-option choice question named `pick` (test:218-220).
pub(super) fn pick_context() -> ClassifierContext {
    context_of(vec![("pick", choice("Pick one", &[("a", ""), ("b", "")]))])
}

/// The context of pi's main test (test:88-103).
pub(super) fn ticket_context() -> ClassifierContext {
    let mut state = serde_json::Map::new();
    state.insert(
        "message".to_string(),
        json!("Help! My payouts have been failing for 3 days."),
    );
    ClassifierContext {
        state,
        images: None,
        questions: vec![
            (
                "team",
                choice(
                    "Which team should handle this?",
                    &[
                        ("billing", "Payments and refunds"),
                        ("technical", "Bugs and outages"),
                        ("sales", ""),
                    ],
                ),
            ),
            (
                "urgent",
                boolean(
                    "Does this convey urgency?",
                    "The user needs help soon",
                    "No time pressure",
                ),
            ),
            (
                "severity",
                score("How severe is this?", &["low", "medium", "high"]),
            ),
        ]
        .into_iter()
        .collect(),
    }
}
