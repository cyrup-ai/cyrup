//! An in-process fake OpenAI chat-completions server: a loopback `std::net` listener that answers
//! `POST /v1/chat/completions` with a scripted SSE stream and records every request body it
//! receives, so a test can assert on the bytes a provider would see.
//!
//! This is the durable form of the live harness (`fake_openai.py` + `jsrun.py`) the codemode work
//! was verified with. The point of driving the real `cyrup` binary against it is that the request
//! the model receives (system message, tools array, tool schemas) and the tool-result message the
//! next request carries are the contract — and no in-process test looked at them, which is how an
//! empty system prompt shipped (CODE-014).
//!
//! One thread per connection, `Connection: close`, no external dependency: a test using it stays a
//! plain `#[test]` with a blocking `Command::output()`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use serde_json::{Value, json};

/// What the scripted model does on one request.
#[derive(Clone, Debug)]
pub enum Turn {
    /// Answer with assistant text and stop.
    Text(String),
    /// Call these tools in one assistant message: `(tool name, arguments)`.
    Tools(Vec<(String, Value)>),
}

impl Turn {
    pub fn text(text: &str) -> Self {
        Self::Text(text.to_owned())
    }

    /// One call to the `codemode` tool with this script.
    pub fn codemode(code: &str) -> Self {
        Self::Tools(vec![("codemode".to_owned(), json!({ "code": code }))])
    }
}

/// The running server. Dropping it stops the accept loop.
pub struct FakeOpenAi {
    port: u16,
    requests: Arc<Mutex<Vec<Value>>>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl FakeOpenAi {
    /// Start on an ephemeral loopback port. Request `n` is answered with `turns[n]`; once the
    /// script runs out every further request is answered with the text `done`.
    pub fn start(turns: Vec<Turn>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let turns = Arc::new(turns);
        let accept = {
            let (requests, stop) = (requests.clone(), stop.clone());
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        return;
                    }
                    let Ok(conn) = conn else { continue };
                    let (requests, turns) = (requests.clone(), turns.clone());
                    std::thread::spawn(move || serve(conn, &requests, &turns));
                }
            })
        };
        Self {
            port,
            requests,
            stop,
            accept: Some(accept),
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    /// Every chat-completions request body received so far, in arrival order.
    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().expect("requests lock").clone()
    }

    /// Write a `models.json` declaring provider `fake`, model `m1`, into an agent directory.
    pub fn write_models_json(&self, agent_dir: &Path) {
        let models = json!({ "providers": { "fake": {
            "name": "Fake",
            "baseUrl": self.base_url(),
            "api": "openai-completions",
            "apiKey": "x",
            "models": [{ "id": "m1", "name": "M1", "contextWindow": 200000, "input": ["text", "image"] }],
        } } });
        std::fs::write(agent_dir.join("models.json"), models.to_string())
            .expect("write models.json");
    }
}

impl Drop for FakeOpenAi {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock `accept()` so the loop sees the flag.
        drop(TcpStream::connect(("127.0.0.1", self.port)));
        if let Some(handle) = self.accept.take() {
            drop(handle.join());
        }
    }
}

fn serve(conn: TcpStream, requests: &Mutex<Vec<Value>>, turns: &[Turn]) {
    let Ok(mut writer) = conn.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(conn);
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).unwrap_or(0) == 0 {
        return;
    }
    let mut content_length = 0usize;
    let mut chunked = false;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 {
            return;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        let lower = line.to_ascii_lowercase();
        if let Some(v) = lower.strip_prefix("content-length:") {
            content_length = v.trim().parse().unwrap_or(0);
        }
        if lower.starts_with("transfer-encoding:") && lower.contains("chunked") {
            chunked = true;
        }
    }
    let body = if chunked {
        read_chunked(&mut reader)
    } else {
        let mut body = vec![0u8; content_length];
        if reader.read_exact(&mut body).is_err() {
            return;
        }
        body
    };

    if request_line.starts_with("GET ") {
        let payload = json!({ "data": [{ "id": "m1" }] }).to_string();
        let head = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            payload.len()
        );
        drop(writer.write_all(head.as_bytes()));
        drop(writer.write_all(payload.as_bytes()));
        drop(writer.shutdown(Shutdown::Both));
        return;
    }

    let index = {
        let mut guard = requests.lock().expect("requests lock");
        guard.push(serde_json::from_slice(&body).unwrap_or(Value::Null));
        guard.len() - 1
    };
    let turn = turns
        .get(index)
        .cloned()
        .unwrap_or_else(|| Turn::text("done"));
    let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncache-control: no-cache\r\nconnection: close\r\n\r\n";
    drop(writer.write_all(head.as_bytes()));
    for event in sse_events(index, &turn) {
        drop(writer.write_all(format!("data: {event}\n\n").as_bytes()));
        drop(writer.flush());
    }
    drop(writer.write_all(b"data: [DONE]\n\n"));
    drop(writer.flush());
    drop(writer.shutdown(Shutdown::Both));
}

fn read_chunked(reader: &mut BufReader<TcpStream>) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut size = String::new();
        if reader.read_line(&mut size).unwrap_or(0) == 0 {
            return out;
        }
        let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if size == 0 {
            let mut trailer = String::new();
            drop(reader.read_line(&mut trailer));
            return out;
        }
        let mut chunk = vec![0u8; size];
        if reader.read_exact(&mut chunk).is_err() {
            return out;
        }
        out.extend_from_slice(&chunk);
        let mut crlf = String::new();
        drop(reader.read_line(&mut crlf));
    }
}

fn sse_events(index: usize, turn: &Turn) -> Vec<Value> {
    let base = json!({
        "id": format!("c{index}"),
        "object": "chat.completion.chunk",
        "created": 1,
        "model": "m1",
    });
    let with = |choices: Value, usage: bool| {
        let mut event = base.clone();
        event["choices"] = choices;
        if usage {
            event["usage"] =
                json!({ "prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15 });
        }
        event
    };
    match turn {
        Turn::Text(text) => vec![
            with(
                json!([{ "index": 0, "delta": { "role": "assistant", "content": text }, "finish_reason": null }]),
                false,
            ),
            with(
                json!([{ "index": 0, "delta": {}, "finish_reason": "stop" }]),
                true,
            ),
        ],
        Turn::Tools(calls) => {
            let calls: Vec<Value> = calls
                .iter()
                .enumerate()
                .map(|(i, (name, args))| {
                    json!({
                        "index": i,
                        "id": format!("call_{index}_{i}"),
                        "type": "function",
                        "function": { "name": name, "arguments": args.to_string() },
                    })
                })
                .collect();
            vec![
                with(
                    json!([{ "index": 0, "delta": { "role": "assistant", "content": null, "tool_calls": calls }, "finish_reason": null }]),
                    false,
                ),
                with(
                    json!([{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]),
                    true,
                ),
            ]
        }
    }
}
