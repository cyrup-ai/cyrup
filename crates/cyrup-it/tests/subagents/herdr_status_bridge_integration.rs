//! The herdr status bridge, driven end to end against a REAL Unix socket.
//!
//! This is the test that answers "does the maintainer's sidebar actually say which pane is blocked
//! on them?". It binds a `UnixListener` that speaks herdr's real framing, points
//! `HERDR_SOCKET_PATH` at it, arms the **production** bridge
//! (`cyrup_ext_subagents::herdr::HerdrBridge::start` — the same function
//! `native_impl.rs`'s `SessionStart` arm calls), drives real cyrup lifecycle edges through it, and
//! asserts on the bytes the server received.
//!
//! **Nothing here needs herdr.** herdr is not installed in this container and neither is ghostty.
//! Every contract asserted below was read out of herdr's own source at `tmp/herdr` @ `d59d060`
//! (v0.9.1), and the fake reproduces `handle_connection_with_stop`
//! (`tmp/herdr/src/api/server.rs:156-317`) exactly where it matters: accept, read **one**
//! `\n`-terminated line, write **one**, close. A fake with a read loop would let a client that
//! pipelines pass here and hang against a real herdr — the socket is one request per connection,
//! and the whole reporter is shaped by that.
//!
//! The human-wait half is driven through the session's `HumanInteractionLock`, reached the way
//! production reaches it: `HostServices::human_interaction_lock()`. The bridge is armed with the
//! lock that expression yields (exactly `native_impl.rs`'s `SessionStart` arm), and a dialog is
//! raised by a SECOND, independent `human_interaction_lock()` + `acquire()` — exactly what the
//! permission dialog does (`crates/cyrup-permission-system/src/extension/prompt.rs:174-177`),
//! MCP's dialog owner (`crates/cyrup-mcp/src/owner.rs:658-659`), intercom's clarify
//! (`crates/cyrup-intercom/src/seams.rs:368-369`) and flux's ask tool
//! (`crates/cyrup-flux/src/ask_tool.rs:177-185`).
//!
//! The two ends never share an object by construction; they share one only because the backend
//! hands out one. That is the point. The earlier shape of these tests built ONE `HostCtx` and used
//! its `human_wait_gate()` for both ends — which passes against a product where the observer and
//! the raiser can never be the same object, because `HostCtx::event` mints a fresh
//! `Arc<HumanWaitGate>` per native registration (`crates/cyrup-ext/src/native.rs:170-180`, one ctx
//! per native at `crates/cyrup-ext/src/facade.rs:541`). A `blocked` observed here is now a
//! `blocked` a user would see.

#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cyrup_ext::host::{HostServices, HumanInteractionGuard, HumanInteractionLock};
use cyrup_ext_subagents::herdr::runtime::{HUMAN_WAIT_POLL, HerdrBridge};
use cyrup_ext_subagents::herdr::{arm_in, bridge, shutdown};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

/// How long an assertion waits for a report to reach the fake. Generous: this is a real socket, a
/// real task wake-up and a real JSON round trip, and the failure this test exists to catch is
/// "nothing was ever sent", not "it was 40 ms late".
const SETTLE: Duration = Duration::from_secs(5);

// =================================================================================================
// The fake herdr
// =================================================================================================

/// A `UnixListener` speaking herdr's framing, recording every request line it read.
struct FakeHerdr {
    path: PathBuf,
    received: Arc<Mutex<Vec<String>>>,
    accepts: Arc<Mutex<usize>>,
    accept: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for FakeHerdr {
    fn drop(&mut self) {
        self.accept.abort();
    }
}

impl FakeHerdr {
    /// Start a fake that answers every non-streaming request with `{"type":"ok"}`.
    ///
    /// `events.subscribe` is the one exception, because it is the one method whose connection
    /// stays open past the first answer (`tmp/herdr/src/api/server.rs:229-250`): it is
    /// acknowledged with `{"type":"subscription_started"}` (`schema/response.rs:130`, written at
    /// `server.rs:749-754`) and then held, writing nothing further. `session.snapshot` answers
    /// with a snapshot naming the pane, so the bootstrap completes.
    fn start(pane_id: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("herdr.sock");
        let listener = UnixListener::bind(&path).expect("bind the fake herdr socket");
        let received = Arc::new(Mutex::new(Vec::new()));
        let accepts = Arc::new(Mutex::new(0usize));
        let pane_id = pane_id.to_string();

        let sink = Arc::clone(&received);
        let counter = Arc::clone(&accepts);
        let accept = tokio::spawn(async move {
            loop {
                let Ok((stream, _addr)) = listener.accept().await else {
                    return;
                };
                *counter.lock().unwrap() += 1;
                let sink = Arc::clone(&sink);
                let pane_id = pane_id.clone();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end_matches(['\r', '\n']).to_string();
                    sink.lock().unwrap().push(line.clone());
                    let request: serde_json::Value =
                        serde_json::from_str(&line).unwrap_or(serde_json::Value::Null);
                    let id = request["id"].as_str().unwrap_or("").to_string();
                    let method = request["method"].as_str().unwrap_or("").to_string();

                    let result = match method.as_str() {
                        "events.subscribe" => {
                            serde_json::json!({ "type": "subscription_started" })
                        }
                        "session.snapshot" => serde_json::json!({
                            "type": "session_snapshot",
                            "snapshot": snapshot(&pane_id),
                        }),
                        // `agent.view.set` / `agent.view.clear` answer `type: "agent_view"` and
                        // report `active`, `source` and an optional `label`
                        // (`socket-api.mdx:494-495`) — NOT the bare `{"type":"ok"}` every write
                        // verb answers. A fake that answered `ok` here would let a client that
                        // decoded the wrong result type pass.
                        "agent.view.set" => serde_json::json!({
                            "type": "agent_view",
                            "active": true,
                            "source": request["params"]["source"],
                            "label": request["params"]["label"],
                        }),
                        "agent.view.clear" => serde_json::json!({
                            "type": "agent_view",
                            "active": false,
                        }),
                        _ => serde_json::json!({ "type": "ok" }),
                    };
                    let answer = serde_json::json!({ "id": id, "result": result }).to_string();
                    let stream = reader.get_mut();
                    let _ = stream.write_all(answer.as_bytes()).await;
                    let _ = stream.write_all(b"\n").await;
                    let _ = stream.flush().await;
                    if method == "events.subscribe" {
                        // herdr keeps this ONE connection open and pushes events down it. Nothing
                        // is pushed here; holding it is what makes the bridge's stream stay live
                        // instead of re-bootstrapping in a loop.
                        std::future::pending::<()>().await;
                    }
                });
            }
        });

        Self {
            path,
            received,
            accepts,
            accept,
            _dir: dir,
        }
    }

    /// The environment herdr injects into a pane it owns (`tmp/herdr/src/pane.rs:148-174`,
    /// `tmp/herdr/src/integration/env.rs:8-33`), pointed at this fake.
    fn pane_env(&self, pane_id: &str) -> BTreeMap<String, String> {
        BTreeMap::from([
            ("HERDR_ENV".to_string(), "1".to_string()),
            ("HERDR_PANE_ID".to_string(), pane_id.to_string()),
            (
                "HERDR_SOCKET_PATH".to_string(),
                self.path.display().to_string(),
            ),
            ("HERDR_TAB_ID".to_string(), "w1:t1".to_string()),
        ])
    }

    /// The same pane, with the sidebar projection opted in.
    fn pane_env_with_agent_view(&self, pane_id: &str) -> BTreeMap<String, String> {
        let mut env = self.pane_env(pane_id);
        env.insert(
            cyrup_ext_subagents::herdr::AGENT_VIEW_ENV.to_string(),
            "1".to_string(),
        );
        env
    }

    /// Every request line the fake has read, in arrival order.
    fn lines(&self) -> Vec<String> {
        self.received.lock().unwrap().clone()
    }

    /// Every request line for one method, decoded.
    fn calls(&self, method: &str) -> Vec<serde_json::Value> {
        self.lines()
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|value| value["method"] == method)
            .collect()
    }

    /// How many connections the fake has accepted.
    fn accept_count(&self) -> usize {
        *self.accepts.lock().unwrap()
    }

    /// Poll until `predicate` holds, or fail after [`SETTLE`].
    async fn until(&self, what: &str, predicate: impl Fn(&Self) -> bool) {
        let deadline = std::time::Instant::now() + SETTLE;
        while std::time::Instant::now() < deadline {
            if predicate(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "timed out waiting for {what}; the fake received: {:#?}",
            self.lines()
        );
    }
}

/// A `session.snapshot` result carrying exactly the pane under test, shaped as
/// `tmp/herdr/src/api/schema/session.rs`'s `SessionSnapshot` and `panes.rs:527-561`'s `PaneInfo`.
fn snapshot(pane_id: &str) -> serde_json::Value {
    serde_json::json!({
        "version": "0.9.1",
        "protocol": 1,
        "workspaces": [],
        "tabs": [],
        "panes": [{
            "pane_id": pane_id,
            "terminal_id": "t-1",
            "workspace_id": "w1",
            "tab_id": "w1:t1",
            "focused": true,
            "agent_status": "idle",
        }],
        "layouts": [],
        "agents": [],
    })
}

/// A capability backend that owns ONE [`HumanInteractionLock`] and hands out that same instance on
/// every call — which is what the live backend does
/// (`crates/cyrup-session-svc/src/host_services.rs:1547`, `Arc::clone(&self.human_interaction)`),
/// and what makes every native that was handed that backend Arc
/// (`crates/cyrup-session-svc/src/builder.rs:1221-1224`) observe one slot.
///
/// Everything else is the trait's deny-by-default.
#[derive(Debug, Default)]
struct SessionHost {
    human: Arc<HumanInteractionLock>,
}

impl HostServices for SessionHost {
    fn human_interaction_lock(&self) -> Option<Arc<HumanInteractionLock>> {
        Some(Arc::clone(&self.human))
    }
}

fn session_host() -> Arc<dyn HostServices> {
    Arc::new(SessionHost::default())
}

/// What the bridge is armed with — `native_impl.rs`'s own expression, verbatim in shape.
fn armed_lock(host: &Arc<dyn HostServices>) -> Option<Arc<HumanInteractionLock>> {
    host.human_interaction_lock()
}

/// What a COMPANION does to put a human dialog up — `prompt.rs:174-177`, reaching the backend on
/// its own rather than being handed the bridge's object.
async fn open_dialog(host: &Arc<dyn HostServices>) -> HumanInteractionGuard {
    host.human_interaction_lock()
        .expect("the backend hands out the session lock")
        .acquire()
        .await
}

/// The `params` of the last `pane.report_agent` the fake received.
///
/// Strict on purpose: a direct assertion that reaches this has already waited for the report it
/// is about, so an empty call list there is the failure. A PREDICATE fed to [`FakeHerdr::until`]
/// must use [`last_state_word`] instead — `until` polls from the instant it is called, which is
/// before the bridge's first report has crossed the socket, and an `expect` in the predicate
/// turns "not yet" into a panic that no amount of waiting could have avoided.
fn last_state(fake: &FakeHerdr) -> serde_json::Value {
    fake.calls("pane.report_agent")
        .last()
        .cloned()
        .expect("at least one pane.report_agent")["params"]
        .clone()
}

/// The `state` word of the last `pane.report_agent`, or `None` while none has arrived.
fn last_state_word(fake: &FakeHerdr) -> Option<String> {
    Some(
        fake.calls("pane.report_agent").last()?["params"]["state"]
            .as_str()?
            .to_owned(),
    )
}

// =================================================================================================
// The tests
// =================================================================================================

/// **The headline outcome.** A human dialog goes up, and the pane's row in herdr's sidebar says
/// `blocked` — under cyrup's own source and agent, so herdr routes it rather than dropping it.
///
/// The dialog is raised the way a companion raises one: a SECOND, independent
/// `HostServices::human_interaction_lock()` followed by `acquire()`. The bridge was armed from a
/// separate call to the same accessor. Neither end is handed the other's object, so this passes
/// only because the backend hands out ONE lock — which is the fact production depends on and the
/// fact the old per-`HostCtx` `HumanWaitGate` shape did not have.
///
/// *Gutted by*: removing the human-wait watcher from `HerdrBridge::start`; making `SessionHost`
/// mint a fresh lock per call instead of cloning one (the shape of the bug this replaced);
/// ordering `is_running`
/// above `human_waiting` in `StateModel::desired` (the report becomes `working`); reporting under
/// a `herdr:<known-agent>` source, which herdr silently drops
/// (`is_reserved_native_state_source`, `tmp/herdr/src/agent_resume.rs:100-113`) — the assertion on
/// `source`/`agent` is what catches that one, because the *wire* still carries a report.
#[tokio::test(flavor = "multi_thread")]
async fn a_human_wait_puts_the_pane_at_blocked() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host))
        .expect("the bridge arms inside a herdr pane with a UI");

    bridge.agent_start();
    fake.until("the pane to say working", |fake| {
        fake.calls("pane.report_agent")
            .iter()
            .any(|call| call["params"]["state"] == "working")
    })
    .await;

    let guard = open_dialog(&host).await;
    fake.until("the pane to say blocked", |fake| {
        fake.calls("pane.report_agent")
            .iter()
            .any(|call| call["params"]["state"] == "blocked")
    })
    .await;

    let blocked = fake
        .calls("pane.report_agent")
        .into_iter()
        .find(|call| call["params"]["state"] == "blocked")
        .expect("a blocked report");
    assert_eq!(
        blocked["params"]["source"], "cyrup:subagents",
        "a herdr:<known-agent> source has its state SILENTLY DROPPED \
         (tmp/herdr/src/agent_resume.rs:100-113)"
    );
    assert_eq!(blocked["params"]["agent"], "cyrup");
    assert_eq!(blocked["params"]["message"], "awaiting approval");

    // Dropping the last guard takes the pane back off blocked — and NOT to idle, because the root
    // turn is still in flight. A bridge that treated the gate as a latch would leave the sidebar
    // claiming the human is still needed.
    drop(guard);
    fake.until("the pane to go back to working", |fake| {
        last_state_word(fake).as_deref() == Some("working")
    })
    .await;

    bridge.release().await;
}

/// The whole of §12's first row: **outside a herdr pane nothing runs**. The listener here panics
/// on `accept`, so any connection at all fails the test loudly — an assertion on absence that can
/// actually fail.
///
/// *Gutted by*: gating on `HERDR_SOCKET_PATH` instead of the `HERDR_ENV` + `HERDR_PANE_ID`
/// conjunction; arming before the gate is answered.
#[tokio::test(flavor = "multi_thread")]
async fn no_herdr_env_means_no_socket() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let hostile = tokio::spawn(async move {
        let _accepted = listener.accept().await;
        panic!("nothing may connect to herdr outside a herdr pane");
    });

    let host = session_host();
    // HERDR_SOCKET_PATH set and live; HERDR_ENV absent. This is a developer who once ran herdr and
    // still has the variable exported.
    let not_a_pane = BTreeMap::from([
        ("HERDR_PANE_ID".to_string(), "w1:p1".to_string()),
        ("HERDR_SOCKET_PATH".to_string(), path.display().to_string()),
    ]);
    assert!(
        HerdrBridge::start(&not_a_pane, true, armed_lock(&host)).is_none(),
        "HERDR_ENV != \"1\" means inert"
    );

    // herdr's real `PaneLaunchIdentity::OmitPane` shape (`tmp/herdr/src/pane.rs:170-172`):
    // HERDR_ENV=1 and the socket path set, and NO pane id. This is the one a reviewer would not
    // think to write, and it is a shape herdr actually produces.
    let omit_pane = BTreeMap::from([
        ("HERDR_ENV".to_string(), "1".to_string()),
        ("HERDR_SOCKET_PATH".to_string(), path.display().to_string()),
    ]);
    assert!(
        HerdrBridge::start(&omit_pane, true, armed_lock(&host)).is_none(),
        "HERDR_ENV=1 with no pane id means inert"
    );

    // A real human wait, with no bridge armed. Nothing may connect.
    let guard = open_dialog(&host).await;
    tokio::time::sleep(HUMAN_WAIT_POLL * 5).await;
    drop(guard);
    assert!(!hostile.is_finished(), "nothing connected");
    hostile.abort();
}

/// §12's second row: inside a pane but headless (`cyrup -p`), the pane is **detected** and the
/// bridge is never armed. A headless cyrup is usually a child sharing its interactive parent's
/// pane — `HERDR_PANE_ID` is inherited by every descendant — and two authorities on one pane make
/// the sidebar flicker between them (pi `herdr-status.ts:377`).
///
/// *Gutted by*: dropping the `has_ui` check, which makes the fake accept a connection.
#[tokio::test(flavor = "multi_thread")]
async fn a_headless_session_in_a_pane_never_arms() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    assert!(
        HerdrBridge::start(&fake.pane_env("w1:p1"), false, armed_lock(&host)).is_none(),
        "has_ui == false must not arm"
    );
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        fake.accept_count(),
        0,
        "a headless session must not open a socket at all"
    );
}

/// A finishing child must not flip the pane idle mid-turn. This is the exact defect the refcount
/// exists to prevent (`reporter.py:26-28`).
///
/// *Gutted by*: making `edge_depth` a boolean; counting children instead of runs; dropping the
/// saturating subtract (which would wrap and pin the pane at `working` for ever).
#[tokio::test(flavor = "multi_thread")]
async fn a_finishing_child_does_not_flip_the_pane_idle() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host)).unwrap();

    bridge.agent_start();
    bridge.foreground_run_started();
    bridge.foreground_run_started();
    fake.until("working", |fake| {
        last_state_word(fake).as_deref() == Some("working")
    })
    .await;

    bridge.foreground_run_finished();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        fake.calls("pane.report_agent")
            .iter()
            .all(|call| call["params"]["state"] != "idle"),
        "one of three in-flight units ending must not take the pane to idle"
    );
    assert_eq!(last_state(&fake)["state"], "working");

    bridge.foreground_run_finished();
    bridge.agent_end();
    fake.until("idle once everything has ended", |fake| {
        last_state_word(fake).as_deref() == Some("idle")
    })
    .await;

    bridge.release().await;
}

/// `done` is never reported. herdr has **two** enums and only one is reportable:
/// `PaneAgentState` is `{idle, working, blocked, unknown}`
/// (`tmp/herdr/src/api/schema/common.rs:149-156`) while `AgentStatus` adds `done`
/// (`:158-166`) and herdr **derives** it from idle-and-unseen
/// (`tmp/herdr/src/app/api_helpers.rs:96-107`). A `"state":"done"` on the wire is a serde failure
/// at herdr, answered `invalid_request`.
///
/// *Gutted by*: widening the reported enum to five variants.
#[tokio::test(flavor = "multi_thread")]
async fn the_wire_never_carries_a_state_herdr_cannot_accept() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host)).unwrap();

    bridge.agent_start();
    let guard = open_dialog(&host).await;
    fake.until("blocked", |fake| {
        last_state_word(fake).as_deref() == Some("blocked")
    })
    .await;
    drop(guard);
    bridge.agent_end();
    fake.until("idle", |fake| {
        last_state_word(fake).as_deref() == Some("idle")
    })
    .await;
    bridge.release().await;

    for call in fake.calls("pane.report_agent") {
        let state = call["params"]["state"].as_str().unwrap_or_default();
        assert!(
            matches!(state, "idle" | "working" | "blocked" | "unknown"),
            "{state:?} is not one of herdr's four REPORTABLE states"
        );
    }
}

/// `seq` must be strictly increasing across a mixed critical/decorative burst, and every line must
/// carry the SAME source. herdr ignores a report whose `seq` is not greater than the last accepted
/// one for that source (`socket-api.mdx:798`), and a pane accepts sequenced reports from at most
/// **32 distinct sources** for its whole lifetime — clearing and expiry release nothing — so a
/// per-report source would burn that budget in under a minute.
///
/// *Gutted by*: assigning `seq` at enqueue instead of at wire time (`client.py:353-356`); deriving
/// the source per report.
#[tokio::test(flavor = "multi_thread")]
async fn seq_is_monotonic_and_the_source_is_stable() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host)).unwrap();

    for _ in 0..6 {
        bridge.agent_start();
        let guard = open_dialog(&host).await;
        fake.until("blocked", |fake| {
            last_state_word(fake).as_deref() == Some("blocked")
        })
        .await;
        drop(guard);
        fake.until("working", |fake| {
            last_state_word(fake).as_deref() == Some("working")
        })
        .await;
        bridge.agent_end();
        fake.until("idle", |fake| {
            last_state_word(fake).as_deref() == Some("idle")
        })
        .await;
    }
    bridge.release().await;

    let mut previous = 0u64;
    let mut sequenced = 0usize;
    for line in fake.lines() {
        let Ok(call) = serde_json::from_str::<serde_json::Value>(&line) else {
            continue;
        };
        let Some(seq) = call["params"]["seq"].as_u64() else {
            continue;
        };
        assert_eq!(
            call["params"]["source"], "cyrup:subagents",
            "every sequenced report shares ONE source — herdr caps a pane at 32 for its lifetime"
        );
        assert!(seq > previous, "seq {seq} must exceed {previous}");
        previous = seq;
        sequenced += 1;
    }
    assert!(
        sequenced >= 6,
        "the burst produced {sequenced} sequenced reports"
    );
}

/// A clean shutdown releases the pane, and the release is the **last** line on the wire.
///
/// This is correctness rather than hygiene: herdr never reclaims agent state from a `cyrup:`
/// source. `hook_authority_is_effective` short-circuits to `true` for a non-`full_lifecycle`
/// source (`tmp/herdr/src/terminal/state.rs:1812-1817`), `HookAuthority` carries no expiry at all
/// (`:23`), and the process-exit override only fires for an agent herdr can name (`:401-407`).
/// Without the release, one `kill` leaves a permanent phantom row.
///
/// *Gutted by*: removing the release; letting a decorative report race past it (the `biased`
/// select in `reporter::drain` is what forbids that, and the Python client drops its decorative
/// lane on release for the same reason, `client.py:26-28`).
#[tokio::test(flavor = "multi_thread")]
async fn a_clean_shutdown_releases_the_pane_last() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host)).unwrap();

    bridge.agent_start();
    fake.until("working", |fake| {
        last_state_word(fake).as_deref() == Some("working")
    })
    .await;
    bridge.release().await;

    fake.until("a release", |fake| {
        !fake.calls("pane.release_agent").is_empty()
    })
    .await;

    let release = fake.calls("pane.release_agent").pop().unwrap();
    assert_eq!(release["params"]["source"], "cyrup:subagents");
    assert_eq!(release["params"]["agent"], "cyrup");
    assert_eq!(release["params"]["pane_id"], "w1:p1");

    let last = fake
        .lines()
        .into_iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        // The subscription connection is opened once at arm time and never carries another
        // request; it is not part of the report ordering.
        .rfind(|call| call["method"] != "events.subscribe" && call["method"] != "session.snapshot")
        .unwrap();
    assert_eq!(
        last["method"], "pane.release_agent",
        "nothing may land on the wire after the release"
    );

    // Idempotent: a `SessionShutdown` racing the signal guard must not release twice.
    bridge.release().await;
    assert_eq!(fake.calls("pane.release_agent").len(), 1);
}

/// A rejected report never disturbs the agent — pi's own contract, *"Herdr integration is best
/// effort"* (`herdr-status.ts:188-191`), and the Python client's, *"reporting agent state must
/// never be able to disturb the agent itself"* (`client.py:17-19`).
///
/// The fake answers `pane_not_found`, then `invalid_agent`, then closes mid-write. Every edge
/// still returns, and the bridge keeps reporting afterwards — the state that failed is re-sent
/// because a failed send calls `StateModel::invalidate_last_report` rather than pretending the
/// report landed.
///
/// *Gutted by*: `?`-propagating out of the reporter; panicking on a closed socket; de-duplicating
/// against a report herdr never received (drop `invalidate_last_report` and the retry never
/// happens, so no second `working` is ever seen).
#[tokio::test(flavor = "multi_thread")]
async fn a_rejected_report_never_disturbs_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("herdr.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let received = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&received);
    // Keyed on the METHOD, not on the connection ordinal. The ordinal counted `events.subscribe`
    // and `session.snapshot` too, so which report got which refusal depended on a race — and the
    // three refusals had to land on three DIFFERENT states, which is what let the retry go
    // unproven: every `working` line the old assertion saw was a state CHANGE, which the
    // de-duplicator would have passed through whether or not the failed report was invalidated.
    let reports = Arc::new(Mutex::new(0usize));
    let report_counter = Arc::clone(&reports);
    let accept = tokio::spawn(async move {
        loop {
            let Ok((stream, _addr)) = listener.accept().await else {
                return;
            };
            let sink = Arc::clone(&sink);
            let report_counter = Arc::clone(&report_counter);
            tokio::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                sink.lock()
                    .unwrap()
                    .push(line.trim_end_matches(['\r', '\n']).to_string());
                let request: serde_json::Value =
                    serde_json::from_str(line.trim()).unwrap_or(serde_json::Value::Null);
                let id = request["id"].as_str().unwrap_or("").to_string();
                let method = request["method"].as_str().unwrap_or("").to_string();
                let answer = if method == "pane.report_agent" {
                    let nth = {
                        let mut seen = report_counter.lock().unwrap();
                        *seen += 1;
                        *seen
                    };
                    match nth {
                        1 => serde_json::json!({
                            "id": id,
                            "error": {"code": "pane_not_found", "message": "no such pane"},
                        })
                        .to_string(),
                        2 => serde_json::json!({
                            "id": id,
                            "error": {"code": "invalid_agent", "message": "bad label"},
                        })
                        .to_string(),
                        // Close without writing anything at all.
                        3 => return,
                        _ => serde_json::json!({ "id": id, "result": {"type": "ok"} }).to_string(),
                    }
                } else {
                    serde_json::json!({ "id": id, "result": {"type": "ok"} }).to_string()
                };
                let stream = reader.get_mut();
                let _ = stream.write_all(answer.as_bytes()).await;
                let _ = stream.write_all(b"\n").await;
                let _ = stream.flush().await;
            });
        }
    });

    let host = session_host();
    let env = BTreeMap::from([
        ("HERDR_ENV".to_string(), "1".to_string()),
        ("HERDR_PANE_ID".to_string(), "w1:p1".to_string()),
        ("HERDR_SOCKET_PATH".to_string(), path.display().to_string()),
    ]);
    let bridge = HerdrBridge::start(&env, true, armed_lock(&host)).unwrap();

    // THREE edges that all settle on the SAME state. `agent_start` raises the refcount to 1
    // (`working`); `foreground_run_started` raises it to 2 and `foreground_run_finished` returns
    // it to 1 — `desired()` is `working` throughout, so the de-duplicator would swallow the
    // second and third outright. They reach the wire only because the first and second were
    // REFUSED and `send_state` called `StateModel::invalidate_last_report`, which is the whole
    // retry contract. Each refusal is a different shape: an API error, a second API error, and a
    // socket closed mid-write.
    bridge.agent_start();
    let seen_working = |want: usize| {
        let lines = received.lock().unwrap().clone();
        lines
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|call| {
                call["method"] == "pane.report_agent" && call["params"]["state"] == "working"
            })
            .count()
            >= want
    };
    let wait_for = |want: usize| async move {
        let deadline = std::time::Instant::now() + SETTLE;
        while std::time::Instant::now() < deadline {
            if seen_working(want) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    };
    assert!(wait_for(1).await, "the first report reaches the wire");
    bridge.foreground_run_started();
    assert!(
        wait_for(2).await,
        "a refused report must be re-sent on the next edge even though the STATE did not change; \
         received: {:#?}",
        received.lock().unwrap()
    );
    bridge.foreground_run_finished();
    assert!(
        wait_for(3).await,
        "and again after a socket that closed mid-write; received: {:#?}",
        received.lock().unwrap()
    );

    // Every edge returned, nothing panicked, and the bridge still answers for a real transition.
    bridge.agent_end();
    let deadline = std::time::Instant::now() + SETTLE;
    let mut saw_idle = false;
    while std::time::Instant::now() < deadline {
        let lines = received.lock().unwrap().clone();
        if lines
            .iter()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .any(|call| call["method"] == "pane.report_agent" && call["params"]["state"] == "idle")
        {
            saw_idle = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        saw_idle,
        "the bridge must keep reporting after a rejection, a refusal and a closed socket; \
         received: {:#?}",
        received.lock().unwrap()
    );

    bridge.release().await;
    accept.abort();
}

/// The subscription and the reports use **different connections**, and the subscribed one never
/// carries a second request line.
///
/// herdr's socket is one request per connection (`handle_connection_with_stop` reads one line,
/// answers and returns, `tmp/herdr/src/api/server.rs:156-317`); only `events.subscribe`,
/// `pane.graphics.stream` and the four in-band waits hold a connection open. A multiplexing client
/// would hang here exactly as it would against a real herdr, because this fake reads one line per
/// connection and no more.
///
/// It also pins the no-gap bootstrap's own shape: `events.subscribe` is sent BEFORE
/// `session.snapshot` (`socket-api.mdx:118-130`), on a connection the snapshot cannot share.
///
/// *Gutted by*: reusing the subscription connection for the snapshot (the snapshot is never
/// answered and the bridge never reaches its first report); calling `session.snapshot` before the
/// subscribe ack, which this ordering assertion catches.
#[tokio::test(flavor = "multi_thread")]
async fn subscribe_and_request_use_different_connections() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();
    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host)).unwrap();

    bridge.agent_start();
    fake.until("a snapshot and a report", |fake| {
        !fake.calls("session.snapshot").is_empty() && !fake.calls("pane.report_agent").is_empty()
    })
    .await;

    let methods: Vec<String> = fake
        .lines()
        .iter()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|call| call["method"].as_str().map(str::to_string))
        .collect();
    let subscribe = methods
        .iter()
        .position(|m| m == "events.subscribe")
        .expect("the bridge subscribes");
    let snapshot = methods
        .iter()
        .position(|m| m == "session.snapshot")
        .expect("the bridge snapshots");
    assert!(
        subscribe < snapshot,
        "the ack must precede the snapshot, or the gap between them loses events \
         (socket-api.mdx:118-130); saw {methods:?}"
    );
    assert!(
        fake.accept_count() >= 2,
        "a subscription and a request cannot share a connection"
    );
    assert_eq!(
        fake.calls("events.subscribe").len(),
        1,
        "one subscription, held open — a re-subscribe loop means the stream is not being kept"
    );

    bridge.release().await;
}

/// **The teardown the product actually runs**, over the process-global slot.
///
/// Every other test in this file drives a `HerdrBridge` it holds by hand. Production never does:
/// `HostEvent::SessionStart` calls `crate::herdr::arm(..)`, which parks the bridge in a
/// process-global `RwLock` slot, and `HostEvent::SessionShutdown` calls `crate::herdr::shutdown()`
/// — which TAKES that slot and releases what it finds
/// (`crates/cyrup-ext-subagents/src/extension/host/native_impl.rs:480,630`). A bridge that armed
/// into the slot and a `shutdown()` that failed to find it would leave a `working` row in the
/// user's sidebar that only a herdr RESTART clears, and nothing in this file would have noticed.
///
/// This is also the `kill` path: cyrup's own signal handler ends an interactive SIGTERM in
/// `session_shutdown{quit}` (`crates/cyrup/src/signals.rs:317-330`), which fans out to that same
/// arm. What a signal adds over this test is cyrup's run loop, not the bridge.
///
/// `arm_in` rather than `arm` for the reason `EnvSource` exists: the environment is a parameter so
/// the test never mutates the real one. The slot is process-wide, which is safe here because
/// nextest runs each test in its own process.
///
/// *Gutted by*: dropping the `BRIDGE` store in `arm_in` (`bridge()` then answers `None` and the
/// release never happens); or making `shutdown()` not `take()` the slot.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_shutdown_releases_the_bridge_in_the_global_slot() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();

    let armed = arm_in(&fake.pane_env("w1:p1"), true, armed_lock(&host));
    assert!(armed.is_some(), "a pane env with a UI arms the bridge");
    assert!(
        bridge().is_some(),
        "arming must publish into the process-global slot, which is the only handle \
         HostEvent::SessionShutdown has"
    );

    bridge().expect("armed").agent_start();
    fake.until("working", |fake| {
        last_state_word(fake).as_deref() == Some("working")
    })
    .await;

    // The SessionShutdown arm, verbatim.
    shutdown().await;

    assert!(
        bridge().is_none(),
        "shutdown must clear the slot so a rebuilt session does not report through a dead bridge"
    );
    let released = fake.calls("pane.release_agent");
    assert_eq!(
        released.len(),
        1,
        "exactly one release reached herdr; saw: {:#?}",
        fake.lines()
    );
    assert_eq!(released[0]["params"]["pane_id"], "w1:p1");

    // Idempotent: a second shutdown (the signal guard racing the quit path) sends nothing more.
    shutdown().await;
    assert_eq!(fake.calls("pane.release_agent").len(), 1);
}

// =================================================================================================
// `[CYRUP-EXCEEDS-UPSTREAM]` — the opt-in sidebar projection
// =================================================================================================

/// **`agent.view.set` reaches herdr, and only when the session opted in.**
///
/// pi sends neither verb — its whole herdr integration drives `pane report-metadata`
/// (`integrations/herdr-status.ts:206,221` @v0.68.0). These are herdr's own
/// (`socket-api.mdx:418-495`) and they answer the question this bridge exists for: *which pane
/// needs me first*. The projection is a SORT with no filter — attention, then the most recent
/// state transition — because the view is server-wide and a filter would hide the user's other
/// agents from their own sidebar; see `cyrup_ext_subagents::herdr::view`.
///
/// Driven through the production arm (`arm_in`, which is `SessionStart`'s call) so the whole
/// chain is real: the env gate, `HerdrBridge::start`, `Reporter::spawn`, the drain, and the bytes
/// on the socket. The fake answers `type: "agent_view"` rather than `ok`, which is herdr's own
/// shape for both verbs.
///
/// *Gutted by*: hard-coding `agent_view: false` in `HerdrBridge::start`; dropping the
/// `set_agent_view` call from the drain's prologue; dropping the `clear_agent_view` from its
/// teardown; making `view::enabled` accept any value (row 2 of the sibling test below).
#[tokio::test(flavor = "multi_thread")]
async fn the_opt_in_sidebar_projection_is_installed_and_cleared() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();

    let bridge = HerdrBridge::start(
        &fake.pane_env_with_agent_view("w1:p1"),
        true,
        armed_lock(&host),
    )
    .expect("a pane env with a UI arms the bridge");

    fake.until("agent.view.set", |fake| {
        !fake.calls("agent.view.set").is_empty()
    })
    .await;

    let set = fake.calls("agent.view.set");
    assert_eq!(set.len(), 1, "installed once, not per edge");
    assert_eq!(set[0]["params"]["source"], "cyrup:subagents");
    assert_eq!(set[0]["params"]["label"], "cyrup fleet");
    assert_eq!(
        set[0]["params"]["sort"],
        serde_json::json!([
            {"field": "attention", "order": "desc"},
            {"field": "state_change_seq", "order": "desc"},
        ]),
        "attention first, most recent transition second — and the built-in fields are BARE \
         strings, which is what `#[serde(untagged)]` on AgentViewSortField buys"
    );
    assert!(
        set[0]["params"].get("filter").is_none(),
        "a filter would hide the user's other agents; the projection is a sort"
    );

    bridge.release().await;
    fake.until("agent.view.clear", |fake| {
        !fake.calls("agent.view.clear").is_empty()
    })
    .await;
    let cleared = fake.calls("agent.view.clear");
    assert_eq!(cleared.len(), 1);
    assert_eq!(
        cleared[0]["params"]["source"], "cyrup:subagents",
        "the clear is ownership-scoped, so a view another program installed in the meantime \
         survives this session's shutdown (socket-api.mdx:494)"
    );
}

/// **A session that did not opt in sends neither verb.**
///
/// The default has to be inert: there is exactly ONE view server-wide
/// (`tmp/herdr/src/app/api/agent_view.rs:88-89`) and a set atomically replaces whatever was there,
/// so a cyrup that installed one unasked would silently re-order — and on a later change, could
/// hide — a user's whole Agents sidebar, and would stamp on any view another program owns.
///
/// *Gutted by*: making the projection unconditional; defaulting `view::enabled` to `true`.
#[tokio::test(flavor = "multi_thread")]
async fn without_the_opt_in_no_agent_view_verb_is_ever_sent() {
    let fake = FakeHerdr::start("w1:p1");
    let host = session_host();

    let bridge = HerdrBridge::start(&fake.pane_env("w1:p1"), true, armed_lock(&host))
        .expect("a pane env with a UI arms the bridge");
    bridge.agent_start();
    // Wait for something this bridge DOES send, so "nothing yet" cannot pass for "never".
    fake.until("working", |fake| {
        last_state_word(fake).as_deref() == Some("working")
    })
    .await;

    bridge.release().await;
    fake.until("the release", |fake| {
        !fake.calls("pane.release_agent").is_empty()
    })
    .await;

    assert!(
        fake.calls("agent.view.set").is_empty(),
        "the sidebar projection must be opt-in; saw {:#?}",
        fake.lines()
    );
    assert!(fake.calls("agent.view.clear").is_empty());
}
