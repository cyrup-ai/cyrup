//! ICOM-065 — `list` reports a Herdr-hosted session's current workspace / tab / pane
//! (pi-intercom v0.14.0 `0ffe1d5`, #129).
//!
//! Driven end to end: a real `cyrup-intercom-broker` process whose environment points
//! `HERDR_SOCKET_PATH` at a fake Herdr — a `UnixListener` speaking Herdr's one-line framing and
//! answering `session.snapshot` from a snapshot the test can move — sessions registered over the
//! broker's socket, and the production `intercom` tool's `list` action reading the answer. Ports
//! the two broker cases of `intercom.integration.test.ts` @v0.14.0 (`:919-1005`) and the join cases
//! of `herdr-location.test.ts` that need a broker to be meaningful: no Herdr contact for a plain
//! roster, one snapshot per `list` and none retained, a moved pane re-resolved by session path, the
//! older-client pane-id arm going `pane_missing`, and an unreachable Herdr keeping the roster.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolUpdate};
use cyrup_intercom::config::IntercomConfig;
use cyrup_intercom::session_state::SharedIntercomState;
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::protocol::{HerdrLabelRef, HerdrLocation};
use cyrup_intercom::transport::spawn::wait_for_broker;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

use super::common::{broker_bin, registration};

/// A fake Herdr API socket answering `session.snapshot` with [`Self::set`]'s current layout.
struct FakeHerdr {
    path: PathBuf,
    snapshot: Arc<Mutex<serde_json::Value>>,
    snapshot_calls: Arc<Mutex<usize>>,
    accept: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl FakeHerdr {
    fn start(snapshot: serde_json::Value) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).expect("bind the fake herdr socket");
        let snapshot = Arc::new(Mutex::new(snapshot));
        let snapshot_calls = Arc::new(Mutex::new(0usize));
        let (current, calls) = (Arc::clone(&snapshot), Arc::clone(&snapshot_calls));
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (current, calls) = (Arc::clone(&current), Arc::clone(&calls));
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let request: serde_json::Value =
                        serde_json::from_str(line.trim_end()).unwrap_or_default();
                    let result = if request["method"] == "session.snapshot" {
                        *calls.lock().unwrap() += 1;
                        serde_json::json!({
                            "type": "session_snapshot",
                            "snapshot": current.lock().unwrap().clone(),
                        })
                    } else {
                        serde_json::json!({ "type": "ok" })
                    };
                    let answer = serde_json::json!({ "id": request["id"], "result": result });
                    let stream = reader.get_mut();
                    let _ = stream.write_all(format!("{answer}\n").as_bytes()).await;
                    let _ = stream.flush().await;
                });
            }
        });
        Self {
            path,
            snapshot,
            snapshot_calls,
            accept,
            _dir: dir,
        }
    }

    fn set(&self, snapshot: serde_json::Value) {
        *self.snapshot.lock().unwrap() = snapshot;
    }

    fn snapshot_calls(&self) -> usize {
        *self.snapshot_calls.lock().unwrap()
    }
}

/// One pane of a Herdr `session.snapshot`, shaped as herdr's `PaneInfo`.
fn pane(pane_id: &str, tab: &str, ws: &str, pi_session: Option<&str>) -> serde_json::Value {
    let mut pane = serde_json::json!({
        "pane_id": pane_id, "terminal_id": format!("term-{pane_id}"), "tab_id": tab,
        "workspace_id": ws, "focused": false, "agent_status": "idle", "revision": 0,
    });
    if let Some(path) = pi_session {
        pane["agent_session"] = serde_json::json!({
            "source": "herdr:pi", "agent": "pi", "kind": "path", "value": path,
        });
    }
    pane
}

fn snapshot(
    ws: (&str, &str),
    tab: (&str, &str),
    panes: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "version": "0.9.1", "protocol": 1, "layouts": [], "agents": [],
        "panes": panes,
        "tabs": [{ "tab_id": tab.0, "workspace_id": ws.0, "number": 1, "label": tab.1,
                   "focused": true, "pane_count": 1, "agent_status": "idle" }],
        "workspaces": [{ "workspace_id": ws.0, "number": 1, "label": ws.1, "focused": true,
                         "pane_count": 1, "tab_count": 1, "active_tab_id": tab.0,
                         "agent_status": "idle" }],
    })
}

/// A real broker whose environment names `herdr_socket` as Herdr's API socket.
struct HerdrBroker {
    socket: PathBuf,
    child: tokio::process::Child,
    _dir: tempfile::TempDir,
}

impl Drop for HerdrBroker {
    fn drop(&mut self) {
        let _ = self.child.start_kill();
    }
}

impl HerdrBroker {
    async fn start(herdr_socket: &Path) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("intercom").join("broker.sock");
        let child = tokio::process::Command::new(broker_bin())
            .env("CYRUP_CODING_AGENT_DIR", dir.path())
            .env("HERDR_SOCKET_PATH", herdr_socket)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the real intercom broker subprocess");
        wait_for_broker(&socket, Duration::from_secs(5))
            .await
            .expect("broker is health-connectable");
        Self {
            socket,
            child,
            _dir: dir,
        }
    }

    async fn connect(
        &self,
        name: &str,
        herdr_pane_id: Option<&str>,
        herdr_session_path: Option<&str>,
    ) -> Arc<IntercomClient> {
        let mut reg = registration(name);
        reg.herdr_pane_id = herdr_pane_id.map(str::to_string);
        reg.herdr_session_path = herdr_session_path.map(str::to_string);
        Arc::new(
            IntercomClient::connect(&self.socket, reg, Some(format!("{name}-session")))
                .await
                .expect("connects"),
        )
    }
}

/// The production `intercom{action:"list"}` text, as `me`.
async fn list_text(me: &Arc<IntercomClient>) -> String {
    let state = Arc::new(SharedIntercomState::new(
        IntercomConfig::default(),
        600_000,
        PathBuf::from("/tmp/work"),
    ));
    state.set_client(Some(Arc::clone(me)));
    let sink: Box<dyn FnMut(ToolUpdate) + Send + 'static> = Box::new(|_| {});
    let result = IntercomTool::new(state)
        .execute(
            ToolCallId::from("list"),
            serde_json::json!({ "action": "list" }),
            CancelToken::new(),
            sink,
        )
        .await
        .expect("list succeeds");
    result
        .content
        .iter()
        .map(|c| match c {
            Content::Text { text, .. } => text.to_string(),
            _ => String::new(),
        })
        .collect()
}

fn row<'a>(text: &'a str, name: &str) -> &'a str {
    text.lines()
        .find(|l| l.starts_with(&format!("• {name} (")))
        .unwrap_or_else(|| panic!("no row for {name}: {text}"))
}

/// "all-non-Herdr rosters preserve upstream structured and text output without invoking Herdr"
/// (`intercom.integration.test.ts:919-957`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_roster_without_herdr_sessions_never_contacts_herdr() {
    let herdr = FakeHerdr::start(snapshot(("w1", "Main"), ("w1:t1", "Tab"), vec![]));
    let broker = HerdrBroker::start(&herdr.path).await;
    let me = broker.connect("me", None, None).await;
    let _peer = broker.connect("peer", None, None).await;

    let structured = me.list_sessions().await.expect("list");
    assert!(structured.iter().all(|s| s.herdr_location.is_none()));
    let text = list_text(&me).await;
    assert!(text.contains("Current session:") && text.contains("Other sessions:"));
    assert!(
        !text.contains("Herdr") && !text.contains("not under"),
        "{text}"
    );
    assert_eq!(herdr.snapshot_calls(), 0, "Herdr is never contacted");
}

/// "broker resolves a registered Herdr pane through one live snapshot"
/// (`intercom.integration.test.ts:959-1005`), plus the moved-pane and older-client arms.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hosted_sessions_are_located_from_one_live_snapshot_per_list() {
    let herdr = FakeHerdr::start(snapshot(
        ("w5", "Platform"),
        ("w5:t2", "API"),
        vec![
            pane("w5:p4", "w5:t2", "w5", None),
            pane("w5:p7", "w5:t2", "w5", Some("/sessions/worker.jsonl")),
        ],
    ));
    let broker = HerdrBroker::start(&herdr.path).await;
    // `me` is a cyrup session: a pane id and no session path.
    let me = broker.connect("me", Some("w5:p4"), None).await;
    // `worker` is registered the way a pi v0.14 client registers: a launch alias plus its session
    // file, which the broker joins on and never returns.
    let _worker = broker
        .connect(
            "worker",
            Some("pane-at-launch"),
            Some("/sessions/worker.jsonl"),
        )
        .await;
    let _plain = broker.connect("plain", None, None).await;

    let text = list_text(&me).await;
    assert!(
        row(&text, "me").ends_with("· Herdr Platform [w5] / API [w5:t2] / pane w5:p4 [self, idle]"),
        "{text}"
    );
    assert!(
        row(&text, "worker")
            .contains("· Herdr Platform [w5] / API [w5:t2] / pane w5:p7 [same cwd]"),
        "the session-path join reports the CURRENT pane, not the launch alias: {text}"
    );
    assert!(
        row(&text, "plain").ends_with(") · not under Herdr [same cwd]"),
        "{text}"
    );
    assert_eq!(
        herdr.snapshot_calls(),
        1,
        "one snapshot for the whole roster"
    );

    // The structured roster: `current` with the snapshot time, and the session path never leaks.
    let sessions = me.list_sessions().await.expect("list");
    assert_eq!(
        herdr.snapshot_calls(),
        2,
        "never cached: every list asks again"
    );
    let worker = sessions
        .iter()
        .find(|s| s.name.as_deref() == Some("worker"))
        .unwrap();
    assert!(matches!(
        &worker.herdr_location,
        Some(HerdrLocation::Current { workspace, tab, pane_id, .. })
            if *workspace == HerdrLabelRef { id: "w5".into(), label: "Platform".into() }
                && *tab == HerdrLabelRef { id: "w5:t2".into(), label: "API".into() }
                && pane_id == "w5:p7"
    ));
    assert_eq!(worker.herdr_pane_id.as_deref(), Some("pane-at-launch"));
    assert!(
        sessions
            .iter()
            .all(|s| !s.extra.contains_key("herdrSessionPath")),
        "herdrSessionPath is broker-private"
    );

    // Both panes move to another workspace and get new ids. The session-path join follows the
    // worker; the pane-id join cannot follow `me` and says so rather than retaining the old place.
    herdr.set(snapshot(
        ("w9", "Research"),
        ("w9:t1", "Review"),
        vec![
            pane("w9:p1", "w9:t1", "w9", None),
            pane("w9:p2", "w9:t1", "w9", Some("/sessions/worker.jsonl")),
        ],
    ));
    let moved = list_text(&me).await;
    assert!(
        row(&moved, "worker").contains("· Herdr Research [w9] / Review [w9:t1] / pane w9:p2"),
        "{moved}"
    );
    assert!(
        row(&moved, "me").contains("· Herdr location unavailable: pane_missing (pane w5:p4) [self"),
        "{moved}"
    );
}

/// "preserves list data and marks location unavailable when the Herdr command fails"
/// (`herdr-location.test.ts:152-166`), against a broker whose Herdr socket does not exist.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_unreachable_herdr_keeps_the_roster_and_marks_hosted_sessions_unavailable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let broker = HerdrBroker::start(&dir.path().join("no-herdr.sock")).await;
    let me = broker.connect("me", None, None).await;
    let _hosted = broker.connect("hosted", Some("w1:p1"), None).await;

    let text = list_text(&me).await;
    assert!(
        row(&text, "hosted")
            .contains("· Herdr location unavailable: herdr_unavailable (pane w1:p1)"),
        "{text}"
    );
    assert!(
        row(&text, "me").contains("· not under Herdr [self"),
        "{text}"
    );
}
