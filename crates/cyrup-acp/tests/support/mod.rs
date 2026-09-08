//! The in-process ACP harness: a real [`cyrup_acp::serve`] over an in-memory transport, against a
//! session whose provider is scripted.
//!
//! It exists because `agent_client_protocol::Responder` has a private constructor
//! (agent-client-protocol-2.1.0 `src/jsonrpc.rs:4536`) and `ConnectionTo<Client>` comes only out of
//! `connect_to`, so the three assertions this module unblocks — `ACP-212`'s rebuild-and-evict,
//! `ACP-217`'s cancel-during-replay and `ACP-005`'s dispose — cannot be written as unit tests at
//! any price. See `tests/wire_gaps.rs`.
//!
//! [`HarnessHost`] is deliberately shaped like `crates/cyrup/src/acp_host.rs`'s `BinaryAcpHost`:
//! one FRESH runtime per `build_runtime`, built with `create_unannounced_at` at the cwd the client
//! named. A host that returns one pre-built runtime for every call — which is what this file
//! replaced — makes [`cyrup_acp::SessionManager::install`] dispose the runtime it has just
//! published, because the outgoing and the incoming are then the same `Arc`.
//!
//! No network, no credential, no spawned process, and deterministic: `serve` takes an arbitrary
//! transport for exactly this reason (see its doc in `connection.rs`).
//!
//! `dead_code` is allowed because this module is compiled separately INTO each test binary, so
//! anything only one of them needs — `seed_session` and `wait_for` are `wire_gaps.rs`'s alone — is
//! unused in the other. It is not a blanket silencer: every item here has a caller.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_acp::{AcpError, AcpHost, AgentSessionRuntime, BoxFuture, RuntimeRequest, SessionsRoot};
use cyrup_core::AssistantMessage;
use cyrup_provider::Provider;
use cyrup_provider::faux::FauxProvider;
use cyrup_session::layout::encode_cwd;
use cyrup_session_svc::{AgentSessionEvent, SessionConfig, SessionFactory};
use futures::channel::mpsc;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};

/// A host that builds one fresh runtime per request, counts the builds, and records the lifecycle
/// of every runtime it has built.
pub struct HarnessHost {
    factory: Arc<SessionFactory>,
    root: SessionsRoot,
    builds: AtomicUsize,
    /// `(nth build, reason)` for every `session_shutdown` any built runtime emitted.
    shutdowns: Arc<Mutex<Vec<(usize, String)>>>,
}

impl HarnessHost {
    /// How many times `session/new`, `session/load` or the prompt-restore path asked for a runtime.
    ///
    /// This is the observable `ACP-212`'s "exactly one `factory.build`" is measured through: the
    /// host is the only seam between the manager and the factory.
    pub fn builds(&self) -> usize {
        self.builds.load(Ordering::SeqCst)
    }

    /// Every `session_shutdown{reason}` observed so far, tagged with the runtime that emitted it.
    pub fn shutdowns(&self) -> Vec<(usize, String)> {
        self.shutdowns
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

impl AcpHost for HarnessHost {
    fn build_runtime<'a>(
        &'a self,
        req: &'a RuntimeRequest,
    ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>> {
        Box::pin(async move {
            let nth = self.builds.fetch_add(1, Ordering::SeqCst);
            // `BinaryAcpHost::build_runtime` verbatim (`crates/cyrup/src/acp_host.rs:100-108`):
            // unannounced, at the cwd the CLIENT named, not the factory's base cwd.
            let runtime = AgentSessionRuntime::create_unannounced_at(
                Arc::clone(&self.factory),
                req.target.clone(),
                Some(req.cwd.as_path().to_path_buf()),
            )
            .await
            .map_err(AcpError::Session)?;

            // The fan-out is a BOUNDED channel whose sends are awaited
            // (`cyrup-session-svc/src/subscriber.rs:63-73`: "backpressure -> slows the agent, never
            // drops"), so a subscription nobody polls makes `dispose()` BLOCK rather than merely
            // lose events. This drain task is not optional.
            let mut events = runtime.session().await.subscribe();
            let log = Arc::clone(&self.shutdowns);
            tokio::spawn(async move {
                while let Some(event) = events.next().await {
                    if let AgentSessionEvent::SessionShutdown { reason } = event {
                        log.lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push((nth, reason));
                    }
                }
            });
            Ok(runtime)
        })
    }

    fn runtime_ready(&self, _runtime: &Arc<AgentSessionRuntime>) {}

    fn sessions_root(&self) -> SessionsRoot {
        self.root.clone()
    }
}

/// One connected ACP client: a line sink into the agent and a line stream out of it.
pub struct Client {
    to_agent: mpsc::UnboundedSender<String>,
    from_agent: mpsc::UnboundedReceiver<String>,
    next_id: i64,
}

impl Client {
    pub async fn send(&mut self, method: &str, id: Option<i64>, params: Value) {
        let mut frame = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if let Some(id) = id {
            frame["id"] = json!(id);
        }
        self.to_agent.send(frame.to_string()).await.unwrap();
    }

    /// Send a request, returning its id.
    pub async fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(method, Some(id), params).await;
        id
    }

    /// Send a notification — no id, and no response will ever come back for it.
    pub async fn notify(&mut self, method: &str, params: Value) {
        self.send(method, None, params).await;
    }

    /// Close the client half of the transport, which is stdin EOF as `serve` sees it — the ACP
    /// host's NORMAL termination (`connection.rs:243-262`). The read half is kept, so a test can
    /// still drain whatever the agent wrote on its way out.
    pub fn hang_up(&mut self) {
        self.to_agent.close_channel();
    }

    /// Collect frames until `stop` says one is the end, or the deadline passes.
    pub async fn drain_until(&mut self, stop: impl Fn(&Value) -> bool) -> Vec<Value> {
        let mut seen = Vec::new();
        let deadline = tokio::time::sleep(Duration::from_secs(60));
        tokio::pin!(deadline);
        loop {
            tokio::select! {
                line = self.from_agent.next() => {
                    let Some(line) = line else { return seen };
                    let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
                    let done = stop(&v);
                    seen.push(v);
                    if done { return seen }
                }
                () = &mut deadline => return seen,
            }
        }
    }

    pub async fn response_to(&mut self, id: i64) -> Value {
        let frames = self.drain_until(|v| is_response_to(v, id)).await;
        frames
            .into_iter()
            .find(|v| is_response_to(v, id))
            .unwrap_or_else(|| panic!("no response to request {id}"))
    }
}

/// A live connection plus everything it is rooted in.
///
/// Hold it for the whole test: dropping the `TempDir`s deletes the cwd and the agent dir out from
/// under the running session.
pub struct Harness {
    pub client: Client,
    pub host: Arc<HarnessHost>,
    pub cwd: tempfile::TempDir,
    pub agent_dir: tempfile::TempDir,
    /// The `serve()` task. Awaiting it after [`Client::hang_up`] is what makes `ACP-005`'s teardown
    /// observable: `serve` runs `SessionManager::shutdown` before it returns.
    pub served: tokio::task::JoinHandle<()>,
}

impl Harness {
    /// Build a scripted session, serve it, and hand back a connected client.
    ///
    /// `persist` is on ([`SessionConfig::new`]'s default): `session/list`, `session/load` and
    /// `session/delete` are about files on disk, so a harness that turned it off would be testing
    /// nothing. Must be called inside a `LocalSet` — `serve` is spawned with `spawn_local`; see
    /// [`in_local_set`].
    pub async fn start(responses: Vec<AssistantMessage>) -> Self {
        let cwd = tempfile::tempdir().unwrap();
        // A file for a scripted `edit` to hit. `edit` is one of the four tools pi activates by
        // default (`read`, `write`, `edit`, `bash`); `ls` is registered but NOT active, which is
        // parity, so scripting `ls` here would only ever prove "Tool ls not found".
        std::fs::write(cwd.path().join("hello.txt"), "one\ntwo\nthree\n").unwrap();
        let agent_dir = tempfile::tempdir().unwrap();

        let faux = Arc::new(FauxProvider::new());
        faux.set_responses(responses);
        // The turbofish is load-bearing: `Arc::clone(&faux)` would infer its return type from the
        // argument and leave no coercion site, so the unsized coercion to `dyn Provider` has to
        // happen at this `let`.
        let provider: Arc<dyn Provider> = Arc::<FauxProvider>::clone(&faux);

        let config = SessionConfig::new(cwd.path(), agent_dir.path());
        let factory = Arc::new(SessionFactory::new(provider, config));
        let host = Arc::new(HarnessHost {
            factory,
            // `SessionConfig::session_dir` is `None`, which resolves to `<agent_dir>/sessions`
            // (`cyrup-session-svc/src/builder.rs:80-81`). These two must agree, or `session/list`
            // and `session/load` look in a directory nothing writes to.
            root: SessionsRoot(agent_dir.path().join("sessions")),
            builds: AtomicUsize::new(0),
            shutdowns: Arc::new(Mutex::new(Vec::new())),
        });

        let (to_agent, agent_reads) = mpsc::unbounded::<String>();
        let (agent_writes, from_agent) = mpsc::unbounded::<String>();
        let transport = agent_client_protocol::Lines::new(
            agent_writes.sink_map_err(|_| std::io::Error::other("client hung up")),
            agent_reads.map(Ok::<String, std::io::Error>),
        );

        let serving: Arc<dyn AcpHost> = Arc::<HarnessHost>::clone(&host);
        let served = tokio::task::spawn_local(async move {
            let _ = cyrup_acp::serve(serving, transport).await;
        });

        Self {
            client: Client {
                to_agent,
                from_agent,
                next_id: 0,
            },
            host,
            cwd,
            agent_dir,
            served,
        }
    }

    /// The sessions root this harness serves, for a test that seeds a transcript into it.
    ///
    /// Read off the host rather than recomposed, so a test seeds into the directory the running
    /// agent actually scans; two constructions of the same path is how those come to disagree.
    pub fn root(&self) -> SessionsRoot {
        self.host.sessions_root()
    }

    /// `initialize` + `session/new`, returning the new session id.
    pub async fn open_session(&mut self) -> String {
        let cwd = self.cwd.path().to_path_buf();
        let id = self
            .client
            .request(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true}}),
            )
            .await;
        self.client.response_to(id).await;

        let id = self
            .client
            .request(
                "session/new",
                json!({"cwd": cwd.to_string_lossy(), "mcpServers": []}),
            )
            .await;
        let resp = self.client.response_to(id).await;
        resp["result"]["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("session/new returned no sessionId: {resp}"))
            .to_string()
    }
}

// --- frame predicates ---------------------------------------------------------------------------

/// A JSON-RPC **response** to `id`. The `method` check matters: an agent-to-client REQUEST
/// (`session/request_permission`) also carries an `id`, drawn from the agent's own counter, and
/// would otherwise be mistaken for the answer to a client request of the same number.
pub fn is_response_to(frame: &Value, id: i64) -> bool {
    frame.get("id").and_then(Value::as_i64) == Some(id) && frame.get("method").is_none()
}

pub fn updates_of<'a>(frames: &'a [Value], kind: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|v| v.get("method").and_then(Value::as_str) == Some("session/update"))
        .filter(|v| v["params"]["update"]["sessionUpdate"].as_str() == Some(kind))
        .collect()
}

/// Position of the response to `id` in `frames`.
///
/// Panics rather than returning `Option`: every call site asserts an ORDER, and a missing frame
/// must name itself rather than silently compare as `None < None`.
pub fn index_of_response(frames: &[Value], id: i64) -> usize {
    frames
        .iter()
        .position(|v| is_response_to(v, id))
        .unwrap_or_else(|| {
            panic!(
                "no response to {id} in {} frames:\n{}",
                frames.len(),
                dump(frames)
            )
        })
}

pub fn index_of_update(frames: &[Value], kind: &str) -> usize {
    frames
        .iter()
        .position(|v| v["params"]["update"]["sessionUpdate"].as_str() == Some(kind))
        .unwrap_or_else(|| {
            panic!(
                "no {kind} in {} frames:\n{}",
                frames.len(),
                dump(frames)
            )
        })
}

/// A readable frame dump for an assertion message: one line per frame, in wire order.
pub fn dump(frames: &[Value]) -> String {
    frames
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let kind = f["params"]["update"]["sessionUpdate"].as_str().map_or_else(
                || {
                    f.get("method").and_then(Value::as_str).map_or_else(
                        || format!("<response {}>", f["id"]),
                        ToString::to_string,
                    )
                },
                ToString::to_string,
            );
            format!("{i:>5}  {kind}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Spin the scheduler until `cond` holds, or give up.
///
/// Every observation of a spawned task's effect needs this: it is not synchronous with the call
/// that caused it. Same shape as `sessions.rs`'s own `settles` helper.
pub async fn wait_for(cond: impl Fn() -> bool) -> bool {
    for _ in 0..1000 {
        if cond() {
            return true;
        }
        tokio::task::yield_now().await;
    }
    cond()
}

/// Run `body` on a current-thread runtime inside a `LocalSet`.
///
/// `serve` is spawned with `spawn_local`, so every test needs exactly this preamble.
pub fn in_local_set<F: std::future::Future<Output = ()>>(body: impl FnOnce() -> F) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, body());
}

// --- seeding --------------------------------------------------------------------------------------

/// Write a session transcript of `pairs` user/assistant exchanges where `find_stored` will find it.
///
/// The directory name is [`encode_cwd`]'s, CALLED rather than re-implemented: `find_stored` scans
/// `session_dirs(root)` and matches the header id (`sessions.rs:357-395`), so a hand-rolled encoder
/// that drifts by one character makes every load answer `Unknown sessionId`.
/// [`cyrup_acp::replay_updates`] emits one update per non-empty user message and one per non-empty
/// assistant message (`sessions.rs:772-800`), so this seeds `2 * pairs` replay frames.
///
/// # The `parentId` chain is load-bearing
///
/// `AgentSession::replay_items` walks `SessionManager::context_entries`
/// (`cyrup-session-svc/src/session/accessors.rs:295`) — the BRANCH projection, i.e. the ancestry of
/// the active leaf — not the raw file. Entries written with `parentId: null` are therefore sibling
/// ROOTS, and a transcript seeded that way replays as **one** exchange no matter how many lines it
/// has. Each entry here parents the previous one, so the whole file is a single branch.
pub fn seed_session(root: &SessionsRoot, cwd: &Path, id: &str, pairs: usize) -> PathBuf {
    let dir = root.path().join(encode_cwd(cwd));
    std::fs::create_dir_all(&dir).unwrap();
    let ts = "2026-01-01T00:00:00.000Z";
    let usage = json!({
        "input": 10, "output": 2, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 12,
        "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
    });
    let mut lines = vec![
        json!({"type": "session", "version": 3, "id": id, "timestamp": ts, "cwd": cwd}).to_string(),
        json!({"type": "session_info", "id": "aaaaaaa0", "parentId": Value::Null,
               "timestamp": ts, "name": "seeded"})
        .to_string(),
    ];
    // The first user message roots the branch; every later one hangs off the previous assistant.
    let mut parent = Value::Null;
    for n in 0..pairs {
        let (u, a) = (format!("u{n:06}"), format!("a{n:06}"));
        lines.push(
            json!({"type": "message", "id": u, "parentId": parent, "timestamp": ts,
                   "message": {"role": "user", "content": [{"type": "text", "text": format!("ask {n}")}],
                               "timestamp": 1_767_600_000_000u64}})
            .to_string(),
        );
        lines.push(
            json!({"type": "message", "id": a, "parentId": u, "timestamp": ts,
                   "message": {"role": "assistant", "content": [{"type": "text", "text": format!("answer {n}")}],
                               "api": "anthropic-messages", "provider": "anthropic", "model": "claude",
                               "usage": usage, "stopReason": "stop",
                               "timestamp": 1_767_600_000_000u64}})
            .to_string(),
        );
        parent = Value::String(a);
    }
    let path = dir.join(format!("2026-01-01T00-00-00-000Z_{id}.jsonl"));
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    path
}
