//! G143 — this session's presence frames must carry REAL context-window usage.
//!
//! # What upstream does
//!
//! `pi-intercom` v0.8.0 added it (`v0.9.2 CHANGELOG.md:33`: "Added live context-window usage to
//! session presence and list output. Thanks to iRonin for PR #59."). At v0.9.2 the producer is
//! `currentContextUsage()` (`index.ts:790-808`, 19 lines including its comment):
//!
//! ```text
//! // Snapshot the live session's context-window usage for presence. getContextUsage()
//! // (stock SDK) reports { tokens, contextWindow, percent }, with tokens/percent null
//! // right after a compaction (before the next assistant response). We emit null in
//! // that case to CLEAR a peer's stale value rather than freeze the old percentage.
//! function currentContextUsage() {
//!   const usage = getLiveContext()?.getContextUsage?.();
//!   if (!usage) return {};
//!   const result = {
//!     contextPct: typeof usage.percent === "number" && Number.isFinite(usage.percent) ? Math.round(usage.percent) : null,
//!     contextTokens: typeof usage.tokens === "number" && Number.isFinite(usage.tokens) ? usage.tokens : null,
//!   };
//!   if (typeof usage.contextWindow === "number" && usage.contextWindow > 0) result.contextWindow = usage.contextWindow;
//!   return result;
//! }
//! ```
//!
//! and it is spread into the presence heartbeat at `index.ts:842-848`:
//!
//! ```text
//! function syncPresenceStatus(): void {
//!   if (!client || !currentSessionId || !getLiveContext()) return;
//!   // context% rides the status heartbeat so peers see live usage at turn boundaries.
//!   client.updatePresence({ status: currentStatus(), ...currentContextUsage() });
//! }
//! ```
//!
//! `syncPresenceStatus` is called from four lifecycle handlers — `agent_start` (`index.ts:1429`),
//! `tool_execution_start` (`:1436`), `tool_execution_end` (`:1443`) and `agent_end` (`:1451`) —
//! which are exactly the four cyrup already subscribes and routes into `sync_presence`.
//!
//! # The gap this closes
//!
//! cyrup modelled the WHOLE consumer half: `SessionInfo.context_pct`/`context_tokens`/
//! `context_window` (`transport/protocol.rs:266-285`), the broker's tri-state apply ladder
//! (`broker/mod.rs:952-954`, ported from `v0.9.2 broker/broker.ts:918-950`), and
//! `IntercomClient::update_presence_with_context`. Nothing PRODUCED a value: `sync_presence` called
//! the three-argument `update_presence`, so every cyrup session advertised itself to its peers with
//! no context usage at all, forever. `update_presence_with_context`'s own doc comment said so
//! ("Nothing in cyrup calls this with a populated context yet"), and its only callers were tests.
//! VERSION-LAG: `git grep contextPct v0.7.0` (cyrup's ported baseline) returns nothing.
//!
//! # Why this drives the REAL entry point
//!
//! No test here touches `current_context_usage` or `update_presence_with_context` (the first is
//! private, the second is not called). Each drives `NativeExtension::on_event` with the same
//! `HostEvent` the session service dispatches when a turn ends or a tool runs, and then reads the
//! numbers back off a SEPARATE, REAL peer client over a REAL broker's `list` — i.e. what a user
//! actually sees in that peer's `/intercom` picker and `intercom({action:"list"})` output.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::common::{registration, spawn_broker, within, write_broker_command};
use cyrup_core::{CancelToken, Content, Tool, ToolCallId, ToolUpdate, ToolUpdateSink};
use cyrup_ext::{ExtMode, HostCtx, HostEvent, HostServices, NativeExtension};
use cyrup_intercom::config::load_config;
use cyrup_intercom::extension::IntercomExtension;
use cyrup_intercom::paths::{broker_socket_path, intercom_dir_path};
use cyrup_intercom::tools::intercom::IntercomTool;
use cyrup_intercom::transport::client::IntercomClient;
use cyrup_intercom::transport::protocol::SessionInfo;
use cyrup_intercom::transport::spawn::wait_for_broker;
use serde_json::Value;

const MY_SESSION_ID: &str = "session-aaaabbbbccccdddd";
const PEER_SESSION_ID: &str = "session-1111222233334444";
const PEER_NAME: &str = "watcher";

/// A `HostServices` whose `context_usage()` answers in cyrup's own shape,
/// `{usedTokens, contextWindow, fraction}` — the SAME shape the live backend produces
/// (`cyrup-session-svc/src/host_services.rs:690-702`), so what this test feeds in is what a real
/// session feeds in. Mutable so one session can be observed across a turn boundary.
struct UsageSink {
    usage: Mutex<Value>,
}

impl UsageSink {
    /// `used`/`window` in cyrup's spelling; `fraction` is computed exactly as the live backend does
    /// (`host_services.rs:692-696`), including its clamp to `[0, 1]`.
    fn new(used: u64, window: u64) -> Arc<Self> {
        let sink = Arc::new(Self {
            usage: Mutex::new(Value::Null),
        });
        sink.set(used, window);
        sink
    }

    fn set(&self, used: u64, window: u64) {
        let fraction = if window == 0 {
            0.0
        } else {
            (used as f64 / window as f64).clamp(0.0, 1.0)
        };
        *self.usage.lock().unwrap() = serde_json::json!({ "usedTokens": used, "contextWindow": window, "fraction": fraction });
    }

    /// The trait default — an EMPTY object, i.e. no session backend has anything to say
    /// (`cyrup-ext/src/host/services.rs:328-330`).
    fn clear(&self) {
        *self.usage.lock().unwrap() = serde_json::json!({});
    }
}

impl HostServices for UsageSink {
    fn context_usage(&self) -> Value {
        self.usage.lock().unwrap().clone()
    }
    fn session_id(&self) -> Option<String> {
        Some(MY_SESSION_ID.to_string())
    }
}

/// What the PEER sees for this session, over a real `list` round trip. `None` while the broker has
/// not published the row yet.
async fn peer_view(peer: &IntercomClient, my_id: &str) -> Option<SessionInfo> {
    peer.list_sessions()
        .await
        .ok()?
        .into_iter()
        .find(|s| s.id == my_id)
}

/// Poll the peer's view until `predicate` holds, then return the row it held on.
async fn peer_view_until<F: Fn(&SessionInfo) -> bool>(
    peer: &IntercomClient,
    my_id: &str,
    budget: Duration,
    predicate: F,
) -> Option<SessionInfo> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if let Some(info) = peer_view(peer, my_id).await
            && predicate(&info)
        {
            return Some(info);
        }
        if tokio::time::Instant::now() >= deadline {
            return peer_view(peer, my_id).await;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn num(field: Option<&serde_json::Number>) -> Option<u64> {
    field.and_then(serde_json::Number::as_u64)
}

/// Bring a real session up on a real broker, plus a real peer to observe it from.
async fn live_pair(
    agent_dir: &Path,
    sink: Arc<UsageSink>,
) -> (
    Arc<IntercomExtension>,
    HostCtx,
    Arc<IntercomClient>,
    String,
    tokio::process::Child,
) {
    let intercom_dir = intercom_dir_path(agent_dir);
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);

    let broker = spawn_broker(agent_dir);
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");

    let peer = Arc::new(
        IntercomClient::connect(
            &socket,
            registration(PEER_NAME),
            Some(PEER_SESSION_ID.to_string()),
        )
        .await
        .expect("the peer registers"),
    );

    let ext = Arc::new(
        IntercomExtension::new(
            agent_dir.to_path_buf(),
            PathBuf::from("/tmp/work"),
            load_config(&intercom_dir).expect("config loads"),
            None,
        )
        .expect("build the extension"),
    );
    ext.set_host_services(sink);
    let ctx = HostCtx::command(ExtMode::Tui, true, agent_dir.to_path_buf());
    let _ = ext
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    let state = ext.state().clone();
    assert!(
        within(Duration::from_secs(30), || state
            .client()
            .is_some_and(|c| c.is_connected()))
        .await,
        "the session connects on SessionStart"
    );
    let my_id = state
        .client()
        .and_then(|c| c.session_id())
        .expect("registered session id");
    (ext, ctx, peer, my_id, broker)
}

/// THE FIX. A turn ends; the peer's session list now shows this session's live context usage.
///
/// 144000/200000 is upstream's own fixture (`intercom.integration.test.ts:18` in
/// `format-context.test.ts`, and `:2597` in the integration suite), and 72% is what
/// `Math.round(144000 / 200000 * 100)` gives.
///
/// Against the pre-fix `extension.rs` — `sync_presence` calling the three-argument
/// `update_presence(None, Some(status), None)` — the FIRST assertion fails with
/// `contextPct: None`, because a presence frame that omits the key leaves the broker's copy
/// untouched and it was never set. The `status` assertion below it stays green either way, which is
/// what proves the failure is about the context fields and not about presence being broken.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_end_publishes_live_context_usage_to_peers() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let sink = UsageSink::new(144_000, 200_000);
    let (ext, ctx, peer, my_id, mut broker) = live_pair(agent_dir.path(), sink.clone()).await;

    // --- The turn ends. This is the production dispatch, not a call into the helper. ---
    let _ = ext
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &ctx,
        )
        .await;

    let info = peer_view_until(&peer, &my_id, Duration::from_secs(20), |s| {
        s.context_pct.is_some()
    })
    .await
    .expect("the peer sees this session in its list");

    assert_eq!(
        num(info.context_pct.as_ref()),
        Some(72),
        "pi `contextPct: Math.round(usage.percent)` — 144000/200000 = 72% ({info:?})"
    );
    assert_eq!(
        num(info.context_tokens.as_ref()),
        Some(144_000),
        "pi `contextTokens: usage.tokens` — the RAW count, not the percentage ({info:?})"
    );
    assert_eq!(
        num(info.context_window.as_ref()),
        Some(200_000),
        "pi sets `contextWindow` only when it is > 0 (`v0.9.2 index.ts:804-806`) ({info:?})"
    );
    // CONTROL (green pre- AND post-fix): the status half of the same heartbeat always worked, so a
    // red run above is about the context fields alone.
    assert_eq!(
        info.status.as_deref(),
        Some("idle"),
        "`agent_end` -> `currentStatus()` is unchanged by this fix ({info:?})"
    );

    peer.disconnect();
    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// The post-compaction CLEAR, which is the whole reason the wire field is a tri-state.
///
/// Upstream's comment is explicit (`v0.9.2 index.ts:791-793`): "with tokens/percent null right after
/// a compaction (before the next assistant response). We emit null in that case to CLEAR a peer's
/// stale value rather than freeze the old percentage." Upstream pins the broker half of this at
/// `intercom.integration.test.ts:2607-2615` — "null contextPct must CLEAR the field, not freeze the
/// old value" — with `contextWindow` (the denominator, not nulled) retained.
///
/// cyrup's `HostServices::context_usage()` has no `null`: it reports `usedTokens: 0` exactly when
/// there is no usable assistant usage to read (`ContextUsage::from_last_assistant`,
/// `cyrup-session-svc/src/state.rs:168-180` — `used = last.map(...).unwrap_or(0)`), which is the
/// same condition pi's post-compaction ladder tests (`pi v0.84.1 agent-session.ts:3196-3206`,
/// `contextTokens > 0` else `{ tokens: null, percent: null }`). So `usedTokens == 0` must translate
/// to the explicit `null`, NOT to a `0%` claim and NOT to an omitted key.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_token_count_clears_a_peers_stale_percentage_instead_of_freezing_it() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let sink = UsageSink::new(144_000, 200_000);
    let (ext, ctx, peer, my_id, mut broker) = live_pair(agent_dir.path(), sink.clone()).await;

    // Establish the stale-high value first.
    let _ = ext
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &ctx,
        )
        .await;
    let before = peer_view_until(&peer, &my_id, Duration::from_secs(20), |s| {
        s.context_pct.is_some()
    })
    .await
    .expect("the peer sees this session");
    assert_eq!(
        num(before.context_pct.as_ref()),
        Some(72),
        "precondition: 72% is published"
    );

    // A compaction lands: the window is still known, the occupancy is not.
    sink.set(0, 200_000);
    let _ = ext.on_event(&HostEvent::AgentStart, &ctx).await;

    let after = peer_view_until(&peer, &my_id, Duration::from_secs(20), |s| {
        s.context_pct.is_none()
    })
    .await
    .expect("the peer sees this session");
    assert_eq!(
        num(after.context_pct.as_ref()),
        None,
        "an unknown token count must CLEAR the peer's 72%, not freeze it ({after:?})"
    );
    assert_eq!(
        num(after.context_tokens.as_ref()),
        None,
        "`contextTokens` clears with it ({after:?})"
    );
    assert_eq!(
        num(after.context_window.as_ref()),
        Some(200_000),
        "the denominator is NOT nulled and must be retained \
         (`intercom.integration.test.ts:2614-2615`) ({after:?})"
    );

    peer.disconnect();
    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// The `if (!usage) return {}` branch (`v0.9.2 index.ts:797-799`) — and its cyrup translation, pi's
/// `if (contextWindow <= 0) return undefined` (`pi v0.84.1 agent-session.ts:3178-3179`).
///
/// A backend that reports nothing (the `HostServices` trait default, an empty object) must make the
/// heartbeat OMIT all three keys rather than send `null`s: omitting leaves the broker's copy alone,
/// whereas a `null` would wipe a value some other path had legitimately set. This is the control
/// that proves the CLEAR above is deliberate rather than "cyrup sends null whenever it is unsure".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_backend_with_no_usage_omits_the_keys_rather_than_nulling_them() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    let sink = UsageSink::new(144_000, 200_000);
    let (ext, ctx, peer, my_id, mut broker) = live_pair(agent_dir.path(), sink.clone()).await;

    let _ = ext
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &ctx,
        )
        .await;
    let before = peer_view_until(&peer, &my_id, Duration::from_secs(20), |s| {
        s.context_pct.is_some()
    })
    .await
    .expect("the peer sees this session");
    assert_eq!(
        num(before.context_pct.as_ref()),
        Some(72),
        "precondition: 72% is published"
    );

    // No model / no usage at all.
    sink.clear();
    let _ = ext
        .on_event(
            &HostEvent::ToolExecStart {
                call_id: "call-1".to_string().into(),
                name: "bash".to_string(),
                args: serde_json::json!({}),
            },
            &ctx,
        )
        .await;

    // The status DID change, which proves the heartbeat went out; the context fields did not.
    let after = peer_view_until(&peer, &my_id, Duration::from_secs(20), |s| {
        s.status.as_deref() == Some("tool:bash")
    })
    .await
    .expect("the peer sees this session");
    assert_eq!(
        after.status.as_deref(),
        Some("tool:bash"),
        "the heartbeat really was sent ({after:?})"
    );
    assert_eq!(
        num(after.context_pct.as_ref()),
        Some(72),
        "omitting the key leaves the broker's copy untouched — an absent backend must not wipe it \
         ({after:?})"
    );
    assert_eq!(
        num(after.context_tokens.as_ref()),
        Some(144_000),
        "{after:?}"
    );
    assert_eq!(
        num(after.context_window.as_ref()),
        Some(200_000),
        "{after:?}"
    );

    peer.disconnect();
    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// `percent` is NOT clamped upstream (`percent = (estimate.tokens / contextWindow) * 100`,
/// `pi v0.84.1 agent-session.ts:3211`), so a session over its window reports > 100.
///
/// cyrup's `HostServices::context_usage()` also carries a `fraction`, and that one IS clamped to
/// `[0, 1]` (`cyrup-session-svc/src/state.rs:164`). Deriving `contextPct` from `fraction` would have
/// been the obvious one-liner and would silently cap an over-window session at `100`; this pins that
/// the percentage comes from `usedTokens`/`contextWindow` instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_over_window_session_reports_more_than_one_hundred_percent() {
    let agent_dir = tempfile::tempdir().expect("tempdir");
    // fraction clamps to 1.0 here; the honest percentage is 104.
    let sink = UsageSink::new(208_000, 200_000);
    let (ext, ctx, peer, my_id, mut broker) = live_pair(agent_dir.path(), sink.clone()).await;

    let _ = ext
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &ctx,
        )
        .await;
    let info = peer_view_until(&peer, &my_id, Duration::from_secs(20), |s| {
        s.context_pct.is_some()
    })
    .await
    .expect("the peer sees this session");

    assert_eq!(
        num(info.context_pct.as_ref()),
        Some(104),
        "pi does not clamp `percent`; a clamped `fraction`-derived value would read 100 ({info:?})"
    );

    peer.disconnect();
    if let Some(c) = ext.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}

/// A second session's `HostServices`: its own id, and NO context usage of its own (the trait
/// default), so anything the list renders came from the peer it is looking at.
struct PlainSink(&'static str);

impl HostServices for PlainSink {
    fn session_id(&self) -> Option<String> {
        Some(self.0.to_string())
    }
}

/// THE USER-VISIBLE END OF THE FEATURE: session A's numbers must come out of session B's
/// `intercom({ action: "list" })` as ` · 72% ctx (144k/200k)`.
///
/// The producer tests above prove the numbers reach the broker. This proves they reach a HUMAN.
/// Upstream renders them in `formatSessionListRow` (`v0.9.2 index.ts:423-429`):
///
/// ```text
/// return `• ${name} (${shortSessionId(session.id)}) — ${session.cwd} (${session.model}${formatContextUsage(session)})${suffix}`;
/// ```
///
/// which was the second half of the same v0.8.0 change ("Added live context-window usage to session
/// presence **and list output**", `v0.9.2 CHANGELOG.md:33`). cyrup's `format_session_list_row` had no
/// `formatContextUsage` term at all — and no cyrup module rendered those fields anywhere — so even a
/// fully-wired producer would have published numbers no user could ever see. `144k/200k` → `72% ctx`
/// is upstream's own fixture (`v0.9.2 format-context.test.ts:16-21`).
///
/// TWO REAL SESSIONS, ONE REAL BROKER. A is driven through its production `AgentEnd` dispatch; B
/// answers through the production `Tool::execute` the agent loop calls when the model emits an
/// `intercom` tool call — the same `IntercomTool` instance `init` registers, over B's own live state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_peers_intercom_list_renders_this_sessions_context_usage() {
    const OBSERVER_SESSION_ID: &str = "session-9999888877776666";

    let agent_dir = tempfile::tempdir().expect("tempdir");
    let intercom_dir = intercom_dir_path(agent_dir.path());
    write_broker_command(&intercom_dir);
    let socket = broker_socket_path(&intercom_dir);
    let mut broker = spawn_broker(agent_dir.path());
    wait_for_broker(&socket, Duration::from_secs(20))
        .await
        .expect("broker up");

    let build = |services: Arc<dyn HostServices>| {
        let ext = Arc::new(
            IntercomExtension::new(
                agent_dir.path().to_path_buf(),
                PathBuf::from("/tmp/work"),
                load_config(&intercom_dir).expect("config loads"),
                None,
            )
            .expect("build the extension"),
        );
        ext.set_host_services(services);
        ext
    };
    let ctx = HostCtx::command(ExtMode::Tui, true, agent_dir.path().to_path_buf());

    // Session A — the one whose context usage is under observation.
    let sink = UsageSink::new(144_000, 200_000);
    let a = build(sink.clone());
    let _ = a
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    // Session B — the observer, with no context usage of its own.
    let b = build(Arc::new(PlainSink(OBSERVER_SESSION_ID)));
    let _ = b
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;

    for (label, ext) in [("A", &a), ("B", &b)] {
        let state = ext.state().clone();
        assert!(
            within(Duration::from_secs(30), || state
                .client()
                .is_some_and(|c| c.is_connected()))
            .await,
            "session {label} connects on SessionStart"
        );
    }
    let a_id = a
        .state()
        .client()
        .and_then(|c| c.session_id())
        .expect("A registered");

    // --- Session A finishes a turn. ---
    let _ = a
        .on_event(
            &HostEvent::AgentEnd {
                messages: Vec::new(),
            },
            &ctx,
        )
        .await;

    // --- Session B's model calls `intercom({ action: "list" })`. ---
    let b_tool = IntercomTool::new(b.state().clone());
    let mut text: String;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let noop: ToolUpdateSink = Box::new(|_u: ToolUpdate| {});
        let result = b_tool
            .execute(
                ToolCallId::from("tc-list"),
                serde_json::json!({ "action": "list" }),
                CancelToken::new(),
                noop,
            )
            .await
            .expect("the list action succeeds");
        text = result
            .content
            .iter()
            .map(|c| match c {
                Content::Text { text, .. } => text.to_string(),
                _ => String::new(),
            })
            .collect();
        if text.contains("% ctx") || tokio::time::Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    assert!(
        text.contains(" · 72% ctx (144k/200k)"),
        "pi `formatSessionListRow` renders `formatContextUsage(session)` — ` · 72% ctx (144k/200k)` \
         for 144000/200000 (`v0.9.2 format-context.test.ts:18-20`). Session A is {a_id}. Got:\n{text}"
    );
    assert!(
        text.contains(" · 72% ctx (144k/200k))"),
        "the term sits INSIDE the model parentheses (`v0.9.2 index.ts:428`), so the row's closing \
         `)` must follow it directly — not be appended after the whole row:\n{text}"
    );
    assert!(
        !text.contains("% ctx (0/0)") && text.matches("% ctx").count() == 1,
        "only the session that reported usage renders it; B reported none:\n{text}"
    );

    if let Some(c) = a.state().client() {
        c.disconnect();
    }
    if let Some(c) = b.state().client() {
        c.disconnect();
    }
    let _ = broker.kill().await;
}
