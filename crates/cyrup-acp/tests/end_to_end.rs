//! End-to-end: the real ACP connection, driven over an in-memory transport, against a session
//! whose provider is scripted.
//!
//! Every other test in this crate is a unit test over fixture events. This file is the one that
//! answers "does an editor actually get what the guide says it gets" — it builds a real
//! [`AgentSessionRuntime`] over [`FauxProvider`], installs it behind the real [`AcpHost`] trait,
//! and runs [`cyrup_acp::serve`] over a `Lines` transport whose two halves are `futures` channels
//! this file holds. Frames on those channels are the bytes a client would see.
//!
//! No network, no credential, no spawned process, and deterministic: `serve` takes an arbitrary
//! transport for exactly this reason (see its doc in `connection.rs`), and `--model faux/faux-1`
//! is the same offline provider `crates/cyrup-it/tests/bin/acp_session.rs` drives.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::sync::Arc;
use std::time::Duration;

use cyrup_acp::{AcpError, AcpHost, AgentSessionRuntime, BoxFuture, RuntimeRequest, SessionsRoot};
use cyrup_core::{AssistantMessage, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text, faux_tool_call};
use cyrup_session_svc::{SessionConfig, SessionFactory};
use futures::channel::mpsc;
use futures::{SinkExt as _, StreamExt as _};
use serde_json::{Value, json};

/// A host over one already-built scripted runtime.
///
/// The real host (`crates/cyrup/src/acp_host.rs`) builds a runtime per `session/new` from the
/// binary's factory ladder. Here the runtime is built once, in the test, so its provider queue is
/// under the test's control — which is the whole point.
struct ScriptedHost {
    runtime: Arc<AgentSessionRuntime>,
    root: SessionsRoot,
}

impl AcpHost for ScriptedHost {
    fn build_runtime<'a>(
        &'a self,
        _req: &'a RuntimeRequest,
    ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>> {
        let runtime = Arc::clone(&self.runtime);
        Box::pin(async move { Ok(runtime) })
    }

    fn runtime_ready(&self, _runtime: &Arc<AgentSessionRuntime>) {}

    fn sessions_root(&self) -> SessionsRoot {
        self.root.clone()
    }
}

/// One connected ACP client: a line sink into the agent and a line stream out of it.
struct Client {
    to_agent: mpsc::UnboundedSender<String>,
    from_agent: mpsc::UnboundedReceiver<String>,
    next_id: i64,
}

impl Client {
    async fn send(&mut self, method: &str, id: Option<i64>, params: Value) {
        let mut frame = json!({"jsonrpc": "2.0", "method": method, "params": params});
        if let Some(id) = id {
            frame["id"] = json!(id);
        }
        self.to_agent.send(frame.to_string()).await.unwrap();
    }

    async fn request(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        self.send(method, Some(id), params).await;
        id
    }

    /// Collect frames until `stop` says one is the end, or the deadline passes.
    async fn drain_until(&mut self, stop: impl Fn(&Value) -> bool) -> Vec<Value> {
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

    async fn response_to(&mut self, id: i64) -> Value {
        let frames = self
            .drain_until(|v| v.get("id").and_then(Value::as_i64) == Some(id))
            .await;
        frames
            .into_iter()
            .find(|v| v.get("id").and_then(Value::as_i64) == Some(id))
            .unwrap_or_else(|| panic!("no response to request {id}"))
    }
}

/// Build a scripted session, serve it, and hand back a connected client.
///
/// `persist` is on: `session/list`, `session/load` and `session/delete` are about files on disk,
/// so a test that turned persistence off would be testing nothing.
async fn connect(responses: Vec<AssistantMessage>) -> (Client, tempfile::TempDir, tempfile::TempDir) {
    let cwd = tempfile::tempdir().unwrap();
    // A file for the scripted `edit` to hit. `edit` is one of the four tools pi activates by
    // default (`read`, `write`, `edit`, `bash`); `ls` is registered but NOT active, which is
    // parity, so scripting `ls` here would only ever prove "Tool ls not found".
    std::fs::write(cwd.path().join("hello.txt"), "one\ntwo\nthree\n").unwrap();
    let agent_dir = tempfile::tempdir().unwrap();

    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(responses);
    let provider: Arc<dyn Provider> = faux;

    let config = SessionConfig::new(cwd.path(), agent_dir.path());
    let target = config.target.clone();
    let factory = Arc::new(SessionFactory::new(provider, config));
    let runtime = AgentSessionRuntime::create(factory, target).await.unwrap();

    let host: Arc<dyn AcpHost> = Arc::new(ScriptedHost {
        runtime,
        root: SessionsRoot(agent_dir.path().join("sessions")),
    });

    let (to_agent, agent_reads) = mpsc::unbounded::<String>();
    let (agent_writes, from_agent) = mpsc::unbounded::<String>();

    let transport = agent_client_protocol::Lines::new(
        agent_writes.sink_map_err(|_| std::io::Error::other("client hung up")),
        agent_reads.map(Ok::<String, std::io::Error>),
    );

    tokio::task::spawn_local(async move {
        let _ = cyrup_acp::serve(host, transport).await;
    });

    (
        Client { to_agent, from_agent, next_id: 0 },
        cwd,
        agent_dir,
    )
}

/// `initialize` + `session/new`, returning the new session id.
async fn open_session(client: &mut Client, cwd: &std::path::Path) -> String {
    let id = client
        .request(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": true, "writeTextFile": true}, "terminal": true}}),
        )
        .await;
    client.response_to(id).await;

    let id = client
        .request(
            "session/new",
            json!({"cwd": cwd.to_string_lossy(), "mcpServers": []}),
        )
        .await;
    let resp = client.response_to(id).await;
    resp["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("session/new returned no sessionId: {resp}"))
        .to_string()
}

fn updates_of<'a>(frames: &'a [Value], kind: &str) -> Vec<&'a Value> {
    frames
        .iter()
        .filter(|v| v.get("method").and_then(Value::as_str) == Some("session/update"))
        .filter(|v| v["params"]["update"]["sessionUpdate"].as_str() == Some(kind))
        .collect()
}

/// A tool-calling turn reaches the client as an announce followed by updates, and the prompt
/// resolves once.
///
/// This is the path the guide promises ("each call shown as it runs") and the one no test
/// exercised end to end before: `translate.rs` proved the mapping over fixture events, but nothing
/// drove a real scripted turn through a real connection.
#[test]
fn a_tool_calling_turn_streams_to_the_client() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let (mut client, cwd, _agent) = connect(vec![
            faux_assistant_message(
                vec![faux_tool_call(
                    "edit",
                    json!({"path": "hello.txt", "edits": [{"oldText": "two", "newText": "TWO"}]}),
                )],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ])
        .await;

        let session = open_session(&mut client, cwd.path()).await;

        let id = client
            .request(
                "session/prompt",
                json!({"sessionId": session, "prompt": [{"type": "text", "text": "uppercase the second line"}]}),
            )
            .await;
        let frames = client
            .drain_until(|v| v.get("id").and_then(Value::as_i64) == Some(id))
            .await;

        let announces = updates_of(&frames, "tool_call");
        assert!(
            !announces.is_empty(),
            "no tool_call announce reached the client; frames: {frames:#?}"
        );

        let response = frames
            .iter()
            .find(|v| v.get("id").and_then(Value::as_i64) == Some(id))
            .expect("the prompt must resolve");
        assert!(
            response.get("error").is_none(),
            "the prompt failed: {response}"
        );
        assert_eq!(
            response["result"]["stopReason"], "end_turn",
            "a scripted tool turn then a text turn must settle as end_turn: {response}"
        );

        // The tool must actually RUN. A `failed` here means the session had no such tool, which
        // is what an empty registry looks like from the wire.
        let statuses: Vec<&str> = updates_of(&frames, "tool_call_update")
            .iter()
            .filter_map(|v| v["params"]["update"]["status"].as_str())
            .collect();
        assert!(
            statuses.contains(&"completed"),
            "the edit never completed; statuses were {statuses:?} in {frames:#?}"
        );
        assert!(
            !statuses.contains(&"failed"),
            "the edit failed; statuses were {statuses:?} in {frames:#?}"
        );

        // The guide promises edits arrive as diffs. This is that promise.
        let has_diff = updates_of(&frames, "tool_call_update").iter().any(|v| {
            v["params"]["update"]["content"]
                .as_array()
                .is_some_and(|cs| cs.iter().any(|c| c["type"] == "diff"))
        });
        assert!(has_diff, "no diff content block reached the client: {frames:#?}");

        // …and the edit is on disk.
        assert_eq!(
            std::fs::read_to_string(cwd.path().join("hello.txt")).unwrap(),
            "one\nTWO\nthree\n",
            "the tool call reported success without changing the file"
        );

        // The announce is always first for an id: a client that saw an update before the announce
        // would have nothing to attach it to.
        let first_tool_frame = frames.iter().find(|v| {
            v["params"]["update"]["sessionUpdate"]
                .as_str()
                .is_some_and(|k| k.starts_with("tool_call"))
        });
        assert_eq!(
            first_tool_frame.map(|v| v["params"]["update"]["sessionUpdate"].as_str()),
            Some(Some("tool_call")),
            "the first tool frame for a call must be the announce, not an update"
        );
    });
}

/// `session/list` and `session/delete` operate on the real session tree.
#[test]
fn sessions_are_listed_and_deleted_through_the_wire() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let (mut client, cwd, _agent) =
            connect(vec![faux_assistant_message(vec![faux_text("hi")], StopReason::Stop)]).await;
        let session = open_session(&mut client, cwd.path()).await;

        let id = client.request("session/list", json!({})).await;
        let listed = client.response_to(id).await;
        assert!(
            listed.get("error").is_none(),
            "session/list failed: {listed}"
        );

        let id = client
            .request("session/delete", json!({"sessionId": session}))
            .await;
        let deleted = client.response_to(id).await;
        assert!(
            deleted.get("error").is_none(),
            "session/delete of the live session failed: {deleted}"
        );
    });
}

/// The exact ordered sequence a client sees for a tool-calling turn.
///
/// The other tests assert properties; this one pins the shape, so a regression that reorders or
/// drops a frame is named rather than merely failing somewhere downstream.
#[test]
fn the_frame_sequence_is_stable() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let (mut client, cwd, _agent) = connect(vec![
            faux_assistant_message(
                vec![faux_tool_call("edit", json!({"path": "hello.txt", "edits": [{"oldText": "two", "newText": "TWO"}]}))],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ]).await;
        let session = open_session(&mut client, cwd.path()).await;
        let id = client
            .request("session/prompt", json!({"sessionId": session, "prompt": [{"type":"text","text":"uppercase the second line"}]}))
            .await;
        let frames = client.drain_until(|v| v.get("id").and_then(Value::as_i64) == Some(id)).await;

        let kinds: Vec<String> = frames
            .iter()
            .map(|f| {
                f["params"]["update"]["sessionUpdate"]
                    .as_str()
                    .map_or_else(|| "<response>".to_string(), |k| {
                        f["params"]["update"]["status"]
                            .as_str()
                            .map_or_else(|| k.to_string(), |st| format!("{k}:{st}"))
                    })
            })
            .collect();

        assert_eq!(
            kinds,
            vec![
                "available_commands_update",
                "session_info_update",
                "tool_call",
                "tool_call_update:pending",
                "tool_call_update:in_progress",
                "tool_call_update:completed",
                "agent_message_chunk",
                "session_info_update",
                "<response>",
            ],
            "the client-visible frame sequence changed: {frames:#?}"
        );
    });
}

/// A bash tool call reaches the client as a terminal, and its output arrives.
#[test]
fn a_bash_call_streams_terminal_output() {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let local = tokio::task::LocalSet::new();
    local.block_on(&rt, async {
        let (mut client, cwd, _agent) = connect(vec![
            faux_assistant_message(
                vec![faux_tool_call("bash", json!({"command": "echo cyrup-acp-e2e"}))],
                StopReason::ToolUse,
            ),
            faux_assistant_message(vec![faux_text("ok")], StopReason::Stop),
        ]).await;
        let session = open_session(&mut client, cwd.path()).await;
        let id = client
            .request("session/prompt", json!({"sessionId": session, "prompt": [{"type":"text","text":"echo"}]}))
            .await;
        let frames = client.drain_until(|v| v.get("id").and_then(Value::as_i64) == Some(id)).await;

        let statuses: Vec<&str> = updates_of(&frames, "tool_call_update")
            .iter()
            .filter_map(|v| v["params"]["update"]["status"].as_str())
            .collect();
        assert!(
            statuses.contains(&"completed"),
            "bash never completed; statuses {statuses:?} in {frames:#?}"
        );

        let blob = serde_json::to_string(&frames).unwrap_or_default();
        assert!(
            blob.contains("cyrup-acp-e2e"),
            "the command output never reached the client: {frames:#?}"
        );
    });
}
