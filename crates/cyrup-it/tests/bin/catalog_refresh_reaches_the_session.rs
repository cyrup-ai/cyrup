//! SEAM-170, end to end: the REAL binary's own background catalog refresh installs a row, the
//! running session's `/model` lists it, and a request on it streams it.
//!
//! pi has one `ModelRuntime` per run whose remote catalog every reader shares: the built-ins are
//! wrapped in `withRemoteCatalog` (`coding-agent/src/core/model-runtime.ts:228-233` @f1b2e77f5)
//! and the rpc host's detached `void modelRuntime.refresh()` (`main.ts:942`) refreshes that same
//! runtime, so `get_available_models` answers the refreshed catalog and `set_model` + `prompt`
//! stream it. Before SEAM-170 cyrup's background refresh installed into the PROCESS slot
//! (`provider.rs` `CATALOG_OVERLAY`) while every session read a private copy of the disk store
//! loaded at build, so the row was never listed.
//!
//! **How the binary's refresh is driven without the network.** The refresh fetches
//! `https://pi.dev/api/models/providers/<id>` through the provider-traffic client, which honours
//! `HTTPS_PROXY` (`RemoteCatalog::refresh_once` → `build_client_for_target`). The child is pointed
//! at a loopback proxy that HOLDS the `CONNECT pi.dev:443` — so the refresh is provably in flight
//! after the session exists — while the test writes the catalog a completed fetch would have left
//! in `<agent_dir>/models-store.json`, then refuses the tunnel (`502`). A refused fetch writes
//! nothing (a transport error returns before the store write), and `refresh_and_install` then
//! reloads the store and installs it: exactly the install a successful fetch ends with. Every other
//! `CONNECT` is refused at once. No byte leaves the machine.
//!
//! The refreshed row's `baseUrl` is the loopback fake chat server ([`FakeOpenAi`]), so the request
//! the session streams for it is observed, model id and all.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use crate::support::fake_openai::{FakeOpenAi, Turn};

/// The row only the refresh publishes.
const REFRESHED: &str = "groq-refreshed-only";

struct Fx {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
    home: PathBuf,
}

/// A project and agent dir with a stored `groq` key — the credential is what puts `groq` in the
/// background refresh's fetch list (pi `models.ts:296`, `if (!credential) return;`) — and no
/// `models-store.json`, so the refresh has a fetch to make.
fn fixture() -> Fx {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    let home = tmp.path().join("home");
    for dir in [&cwd, &agent_dir, &home] {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(
        agent_dir.join("auth.json"),
        json!({ "groq": { "type": "api_key", "key": "sk-groq-fixture" } }).to_string(),
    )
    .unwrap();
    Fx {
        _tmp: tmp,
        cwd,
        agent_dir,
        home,
    }
}

/// Write `<agent_dir>/models-store.json` as a completed groq fetch leaves it, its row pointed at
/// `base_url`, dated after groq's compiled-in catalog so the staleness guard keeps it. Atomic, so
/// the binary never reads half a file.
fn write_refreshed_store(agent_dir: &Path, base_url: &str) {
    let floor = cyrup_provider::builtin_model_data_generated_at_by_provider()
        .get("groq")
        .copied()
        .or_else(cyrup_provider::builtin_model_data_generated_at)
        .unwrap();
    let newer = floor + 1;
    let entry = json!({
        "groq": {
            "models": [{
                "id": REFRESHED,
                "name": "Groq Refreshed Only",
                "api": "openai-completions",
                "provider": "groq",
                "baseUrl": base_url,
                "reasoning": false,
                "input": ["text"],
                "cost": {"input": 1.0, "output": 2.0, "cacheRead": 0.0, "cacheWrite": 0.0},
                "contextWindow": 131_072,
                "maxTokens": 8192
            }],
            "lastModified": newer,
            "checkedAt": newer,
            "etag": "\"r1\""
        }
    });
    let path = agent_dir.join(cyrup_config::models_store::MODELS_STORE_FILE_NAME);
    let staged = agent_dir.join("models-store.json.staged");
    std::fs::write(&staged, serde_json::to_string_pretty(&entry).unwrap()).unwrap();
    std::fs::rename(&staged, &path).unwrap();
}

// ------------------------------------------------------------------------------ loopback proxy --

/// An HTTP `CONNECT` proxy on loopback that tunnels nothing: a `CONNECT pi.dev:443` is reported on
/// [`HoldingProxy::pi_dev`] and held until [`HoldingProxy::refuse_held`], every other `CONNECT` is
/// refused at once.
struct HoldingProxy {
    port: u16,
    pi_dev: Receiver<String>,
    release: Sender<()>,
}

impl HoldingProxy {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let (reached, pi_dev) = channel();
        let (release, released) = channel::<()>();
        std::thread::spawn(move || {
            let mut held: Vec<TcpStream> = Vec::new();
            listener.set_nonblocking(true).unwrap();
            loop {
                match listener.accept() {
                    Ok((conn, _)) => {
                        conn.set_nonblocking(false).unwrap();
                        let target = connect_target(&conn);
                        if target.starts_with("pi.dev") {
                            if reached.send(target).is_err() {
                                return;
                            }
                            held.push(conn);
                        } else {
                            refuse(conn);
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => return,
                }
                if released.try_recv().is_ok() {
                    held.drain(..).for_each(refuse);
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        });
        Self {
            port,
            pi_dev,
            release,
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    /// Refuse every held `pi.dev` tunnel (and any that arrives after).
    fn refuse_held(&self) {
        self.release.send(()).unwrap();
    }
}

/// The `host:port` of the `CONNECT` request on `conn` (its head read through the blank line).
fn connect_target(conn: &TcpStream) -> String {
    let mut reader = BufReader::new(conn.try_clone().unwrap());
    let mut first = String::new();
    let _ = reader.read_line(&mut first);
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).unwrap_or(0) == 0 || line.trim_end().is_empty() {
            break;
        }
    }
    first.split_whitespace().nth(1).unwrap_or("").to_string()
}

fn refuse(mut conn: TcpStream) {
    let _ = conn
        .write_all(b"HTTP/1.1 502 Bad Gateway\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
    let _ = conn.flush();
}

// ---------------------------------------------------------------------------------- rpc child --

/// A `cyrup --mode rpc` child read line by line on a thread.
struct Rpc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    seen: Vec<Value>,
    next_id: u64,
}

impl Rpc {
    fn spawn(fx: &Fx, proxy: &HoldingProxy) -> Self {
        let mut cmd = crate::support::env::hermetic(crate::support::bins::cyrup(), &fx.home);
        cmd.current_dir(&fx.cwd)
            .env("CYRUP_AGENT_DIR", &fx.agent_dir)
            // Online (no `CYRUP_OFFLINE`): the background refresh is what is under test. Its one
            // way out is the holding proxy.
            .env("HTTPS_PROXY", proxy.url())
            .env("CYRUP_SKIP_VERSION_CHECK", "1")
            .args(["--mode", "rpc", "--no-session", "--provider", "groq"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = cmd.spawn().expect("spawn cyrup --mode rpc");
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            lines,
            seen: Vec::new(),
            next_id: 0,
        }
    }

    fn send(&mut self, mut command: Value) -> String {
        self.next_id += 1;
        let id = self.next_id.to_string();
        command["id"] = json!(id);
        let stdin = self.stdin.as_mut().expect("stdin open");
        writeln!(stdin, "{command}").unwrap();
        stdin.flush().unwrap();
        id
    }

    /// Read events until one satisfies `done`, which is returned, or fail after 30 s.
    fn read_until(&mut self, what: &str, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) => {
                    let Ok(event) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    self.seen.push(event.clone());
                    if done(&event) {
                        return event;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    panic!("no {what} within 30s; saw {:#?}", self.seen)
                }
                Err(RecvTimeoutError::Disconnected) => {
                    panic!(
                        "the child closed stdout before {what}; saw {:#?}",
                        self.seen
                    )
                }
            }
        }
    }

    /// Send `command` and return its response.
    fn call(&mut self, command: Value) -> Value {
        let id = self.send(command);
        self.read_until("a response", |event| {
            event["type"] == "response" && event["id"] == json!(id)
        })
    }

    /// The `provider/id` of every model `/model` offers (rpc `get_available_models`).
    fn available(&mut self) -> Vec<String> {
        let response = self.call(json!({ "type": "get_available_models" }));
        assert_eq!(response["success"], json!(true), "{response}");
        response["data"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| {
                format!(
                    "{}/{}",
                    m["provider"].as_str().unwrap(),
                    m["id"].as_str().unwrap()
                )
            })
            .collect()
    }
}

impl Drop for Rpc {
    fn drop(&mut self) {
        // EOF on stdin is the protocol's shutdown; the kill is only for a wedged child.
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Done-means (1) of SEAM-170 through the real binary: the binary's background refresh installs a
/// new row for the session's provider (`groq`) after the session exists — `/model` lists it, and a
/// prompt on it streams a request for it.
///
/// RED at `01aba2bb` (`/model` never lists `groq/groq-refreshed-only`): the refresh installed into
/// the process slot, which no session read.
#[test]
fn the_binarys_background_refresh_reaches_the_running_session() {
    let fx = fixture();
    let chat = FakeOpenAi::start(vec![Turn::text("hello from the refreshed model")]);
    let proxy = HoldingProxy::start();
    let mut rpc = Rpc::spawn(&fx, &proxy);

    // The session exists, and the background refresh is in flight (its pi.dev tunnel is held).
    let target = proxy
        .pi_dev
        .recv_timeout(Duration::from_secs(30))
        .expect("the binary's background refresh asks pi.dev for the groq catalog");
    assert_eq!(target, "pi.dev:443");
    let refreshed = format!("groq/{REFRESHED}");
    assert!(
        !rpc.available().contains(&refreshed),
        "fixture: the row is not compiled in"
    );

    // The fetch's outcome lands in the store; the refresh then reloads and installs it.
    write_refreshed_store(&fx.agent_dir, &chat.base_url());
    proxy.refuse_held();

    let deadline = Instant::now() + Duration::from_secs(15);
    let mut listed = rpc.available();
    while !listed.contains(&refreshed) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        listed = rpc.available();
    }
    assert!(
        listed.contains(&refreshed),
        "/model lists the row the binary's background refresh installed; it listed {} groq rows: {:?}",
        listed.iter().filter(|m| m.starts_with("groq/")).count(),
        listed
            .iter()
            .filter(|m| m.starts_with("groq/"))
            .collect::<Vec<_>>()
    );

    // ...and the installed provider streams it.
    let selected =
        rpc.call(json!({ "type": "set_model", "provider": "groq", "modelId": REFRESHED }));
    assert_eq!(selected["success"], json!(true), "{selected}");
    rpc.send(json!({ "type": "prompt", "message": "hi" }));
    rpc.read_until("the run to settle", |event| {
        event["type"] == "agent_settled"
    });
    let requests = chat.requests();
    assert_eq!(
        requests.len(),
        1,
        "one request reached the refreshed row's server: {requests:?}"
    );
    assert_eq!(
        requests[0]["model"],
        json!(REFRESHED),
        "the request carries the refreshed model"
    );
}
