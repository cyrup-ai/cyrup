//! FULLY-WIRED PROOF for G106 — the NATIVE supervisor channel
//! (`pi-subagents/src/intercom/native-supervisor-channel.ts`, added in upstream `3ac0ef5` "Make
//! supervisor coordination native", 2026-07-03).
//!
//! The gap this closes: cyrup's only child→supervisor route was `cyrup-intercom`'s broker-backed
//! `contact_supervisor`, and a plain orchestrator session registers NO broker presence at all
//! (`cyrup_intercom::is_installed` gates a non-child session on `CYRUP_INTERCOM` or an
//! `intercom/config.json`). A child's ask then addressed a supervisor the broker had never heard of.
//! Upstream stopped depending on an installed intercom package entirely: the channel is a directory
//! of JSON files under the shared temp root, always available.
//!
//! The three proofs here drive the WHOLE interleaved lifecycle, not one rendered block each:
//!
//! * **(a) the spawn seam** lives in `src/exec/mod.rs`'s own unit tests
//!   (`build_attempt_spawn_plan_writes_the_native_supervisor_channel_env`), which own the
//!   `base_opts` fixture; it proves the single spawn-plan chokepoint writes
//!   `CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID` + `CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR` and creates
//!   the `requests/`+`replies/` directories.
//! * **(b) the full round trip** — a child writes a blocking request into the very directory (a) put
//!   in its env; the PARENT's real poller adopts it, injects it into a live transcript backend, and
//!   the REAL `subagent_supervisor` tool answers it; the blocked child's `wait_for_reply` returns
//!   the answer. Then the sequence continues: a second poll must NOT re-surface the answered
//!   request, and `pending` must be empty.
//! * **(c) session scoping + the child gate** — a request raised by a DIFFERENT orchestrator session
//!   is never adopted, and the child-side native `contact_supervisor` registers exactly when
//!   `cyrup-intercom` will not be supplying its own.
//!
//! Every root and every environment answer this file needs is supplied EXPLICITLY —
//! `NativeSupervisorChannel::with_root`, `resolve_supervisor_channel_dir_in`, and
//! `SubagentExtensionConfig::{roots, env_overrides}` — so it mutates no process-global state,
//! contains no `unsafe`, and needs no lock.
//! under edition 2024 — the same rationale every other `tests/*_integration.rs` file here carries.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::Path;
use std::sync::{Arc, Mutex};


use cyrup_core::{CancelToken, Tool, ToolCallId};
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::native_supervisor::{
    self, ChildChannelMetadata, NativeSupervisorChannel, SubagentSupervisorTool, SupervisorReason,
};


/// Point the shared subagents temp root at a private tempdir for the duration of a test, so the
/// A channel-directory tree owned by one test.
///
/// `NativeSupervisorChannel::with_root` and `resolve_supervisor_channel_dir_in` take this
/// explicitly, so no test here moves `CYRUP_SUBAGENTS_TEMP_ROOT` on a process the rest of the
/// binary shares — which also means there is nothing to restore and no lock to hold.
fn channel_root() -> tempfile::TempDir {
    tempfile::tempdir().expect("real tempdir")
}

/// A live-capability backend stand-in exposing exactly the two seams the channel uses:
/// `session_id()` (which requests belong to THIS orchestrator) and `inject_message` (the transcript
/// hand-off). Records every injection so the test can assert the parent actually saw the request.
/// One recorded `inject_message` call.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Injection {
    content: String,
    custom_type: Option<String>,
    display: bool,
    trigger_turn: bool,
}

#[derive(Default)]
struct RecordingServices {
    session_id: String,
    injected: Mutex<Vec<Injection>>,
}

impl cyrup_ext::host::HostServices for RecordingServices {
    fn session_id(&self) -> Option<String> {
        Some(self.session_id.clone())
    }

    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        _details: Option<&serde_json::Value>,
        trigger_turn: bool,
    ) -> Result<(), String> {
        self.injected.lock().unwrap().push(Injection {
            content: content.to_string(),
            custom_type: custom_type.map(str::to_string),
            display,
            trigger_turn,
        });
        Ok(())
    }
}

fn child_metadata(channel_dir: &Path, session_id: &str) -> ChildChannelMetadata {
    ChildChannelMetadata {
        channel_dir: channel_dir.to_path_buf(),
        run_id: "run-XYZ".to_string(),
        agent: "reviewer".to_string(),
        child_index: 2,
        orchestrator_target: Some("subagent-chat-abcd1234".to_string()),
        orchestrator_session_id: session_id.to_string(),
        child_target: Some("subagent-reviewer-run-xyz-3".to_string()),
    }
}

async fn call_tool(tool: &SubagentSupervisorTool, args: serde_json::Value) -> String {
    let result = tool
        .execute(
            ToolCallId::from("call-1"),
            args,
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .expect("the supervisor tool must succeed");
    result
        .content
        .iter()
        .filter_map(|c| match c {
            cyrup_core::Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// =================================================================================================
// (b) the full interleaved round trip
// =================================================================================================

/// The whole lifecycle, in order: child writes → parent polls and injects → parent replies through
/// the REAL tool → child's `wait_for_reply` returns → a second poll does NOT re-surface it.
///
/// This is the sequence check the row-level tests cannot give: an answered request that stayed in
/// `pending`, or a request file the reply did not delete, would re-inject on every 500 ms tick.
#[tokio::test]
async fn a_blocking_child_request_is_surfaced_answered_and_then_never_re_surfaced() {
    let root_dir = channel_root();

    let services = Arc::new(RecordingServices {
        session_id: "session-parent-1".to_string(),
        ..Default::default()
    });
    let channel = Arc::new(NativeSupervisorChannel::with_root(root_dir.path().to_path_buf()));
    channel.bind_services(services.clone());
    let tool = SubagentSupervisorTool::new(channel.clone());

    // With nothing on the channel the tool still answers, and says so.
    let empty = call_tool(&tool, serde_json::json!({ "action": "pending" })).await;
    assert_eq!(empty, "No pending supervisor requests.");
    let status = call_tool(&tool, serde_json::json!({ "action": "status" })).await;
    assert!(status.contains("Pending replies: 0."), "got: {status}");

    // --- the child writes a BLOCKING request into its own channel directory ---
    let channel_dir = native_supervisor::resolve_supervisor_channel_dir_in(root_dir.path(), "run-XYZ", "reviewer", 2);
    native_supervisor::ensure_supervisor_channel_dir(&channel_dir).expect("channel dirs");
    let metadata = child_metadata(&channel_dir, "session-parent-1");

    let child = tokio::spawn(async move {
        native_supervisor::send_supervisor_request(
            &metadata,
            SupervisorReason::NeedDecision,
            Some("main or develop?"),
            None,
            &CancelToken::new(),
        )
        .await
    });

    // --- the parent's REAL poller adopts it and injects it into the transcript ---
    let mut adopted = Vec::new();
    for _ in 0..80 {
        adopted = channel.poll_once();
        if !adopted.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(adopted.len(), 1, "the parent must adopt the child's request");
    assert!(adopted[0].contains("main or develop?"), "got: {}", adopted[0]);
    assert!(
        adopted[0].contains("Reply with: subagent_supervisor("),
        "a blocking request must carry its reply recipe: {}",
        adopted[0]
    );

    {
        let injected = services.injected.lock().unwrap();
        assert_eq!(injected.len(), 1, "exactly one transcript injection");
        assert_eq!(
            injected[0].custom_type.as_deref(),
            Some("subagent_supervisor_request")
        );
        assert!(injected[0].display, "the request must be displayed");
        assert!(
            injected[0].trigger_turn,
            "a supervisor request must TRIGGER A TURN — the orchestrator has to act on it"
        );
    }

    // `pending` now lists it with the id the reply recipe named.
    let listed = call_tool(&tool, serde_json::json!({ "action": "pending" })).await;
    assert!(listed.starts_with("- "), "got: {listed}");
    assert!(listed.contains("reviewer [run-XYZ#2] need_decision."), "got: {listed}");
    let pending = channel.pending();
    assert_eq!(pending.len(), 1);
    let request_id = pending[0].request.id.clone();
    let request_file = pending[0].request_file.clone();

    // --- the parent answers through the REAL tool ---
    let replied = call_tool(
        &tool,
        serde_json::json!({ "action": "reply", "replyTo": request_id, "message": "use develop" }),
    )
    .await;
    assert_eq!(replied, format!("Replied to supervisor request {request_id}."));

    // --- the BLOCKED child unblocks with the supervisor's answer ---
    let (_request, reply) = child
        .await
        .expect("the child task joins")
        .expect("the child's ask resolves");
    let reply = reply.expect("a blocking ask returns a reply");
    assert_eq!(reply.message, "use develop");

    // --- and the sequence settles: the request file is gone and no later poll re-surfaces it ---
    assert!(
        !request_file.exists(),
        "answering must delete the request file, or every later tick re-injects it"
    );
    assert!(channel.pending().is_empty(), "an answered request must leave the pending map");
    let after = channel.poll_once();
    assert!(after.is_empty(), "a second poll must adopt nothing: {after:?}");
    assert_eq!(
        services.injected.lock().unwrap().len(),
        1,
        "the answered request must never be injected a second time"
    );

    channel.dispose();
}

/// A fire-and-forget `progress_update` is surfaced but never joins the pending map, and its request
/// file is deleted on adoption (`native-supervisor-channel.ts:669-671`). Without that arm every
/// progress update would sit forever in `pending`, poisoning the single-pending `reply` shorthand.
#[tokio::test]
async fn a_progress_update_is_surfaced_but_never_becomes_pending() {
    let root_dir = channel_root();

    let services = Arc::new(RecordingServices {
        session_id: "session-parent-1".to_string(),
        ..Default::default()
    });
    let channel = Arc::new(NativeSupervisorChannel::with_root(root_dir.path().to_path_buf()));
    channel.bind_services(services.clone());

    let channel_dir = native_supervisor::resolve_supervisor_channel_dir_in(root_dir.path(), "run-A", "worker", 0);
    native_supervisor::ensure_supervisor_channel_dir(&channel_dir).expect("channel dirs");
    let metadata = child_metadata(&channel_dir, "session-parent-1");
    let (request, reply) = native_supervisor::send_supervisor_request(
        &metadata,
        SupervisorReason::ProgressUpdate,
        Some("UPDATE: halfway"),
        None,
        &CancelToken::new(),
    )
    .await
    .expect("a progress update never blocks");
    assert!(reply.is_none(), "a progress update must not wait for a reply");
    assert!(!request.expects_reply);

    let adopted = channel.poll_once();
    assert_eq!(adopted.len(), 1, "the update is still shown to the supervisor");
    assert!(adopted[0].contains("UPDATE: halfway"));
    assert!(
        !adopted[0].contains("Reply with:"),
        "a non-blocking update must carry no reply recipe: {}",
        adopted[0]
    );
    assert!(
        channel.pending().is_empty(),
        "a progress update must never enter the pending map"
    );

    let tool = SubagentSupervisorTool::new(channel.clone());
    let listed = call_tool(&tool, serde_json::json!({ "action": "list" })).await;
    assert_eq!(listed, "No pending supervisor requests.");
    channel.dispose();
}

// =================================================================================================
// (c) session scoping, and the two refusal arms
// =================================================================================================

/// `requestMatchesContext` (`native-supervisor-channel.ts:445-448`): a request keyed to a DIFFERENT
/// orchestrator session is never adopted. Without it, two cyrup sessions sharing one machine would
/// each surface — and be able to answer — the other's children.
#[tokio::test]
async fn a_request_from_another_orchestrator_session_is_never_adopted() {
    let root_dir = channel_root();

    let services = Arc::new(RecordingServices {
        session_id: "session-parent-1".to_string(),
        ..Default::default()
    });
    let channel = Arc::new(NativeSupervisorChannel::with_root(root_dir.path().to_path_buf()));
    channel.bind_services(services.clone());

    let channel_dir = native_supervisor::resolve_supervisor_channel_dir_in(root_dir.path(), "run-B", "worker", 0);
    native_supervisor::ensure_supervisor_channel_dir(&channel_dir).expect("channel dirs");
    // Keyed to a session this orchestrator is not.
    let metadata = child_metadata(&channel_dir, "session-SOMEONE-ELSE");
    let child = tokio::spawn({
        let metadata = metadata.clone();
        async move {
            native_supervisor::send_supervisor_request(
                &metadata,
                SupervisorReason::NeedDecision,
                Some("not yours"),
                None,
                &CancelToken::new(),
            )
            .await
        }
    });

    // Give the child time to land its file, then poll several times.
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    for _ in 0..3 {
        assert!(
            channel.poll_once().is_empty(),
            "another session's request must never be adopted"
        );
    }
    assert!(channel.pending().is_empty());
    assert!(services.injected.lock().unwrap().is_empty());

    child.abort();
    channel.dispose();
}

/// The two arms upstream deliberately REFUSES (`native-supervisor-channel.ts:617-619`), plus the
/// unknown-action arm. The `action` enum advertises six values and every one has a real dispatch
/// arm — `send`/`ask` return upstream's verbatim refusal rather than falling through to a generic
/// "unsupported", which is what tells the model to use `contact_supervisor` from the child instead.
#[tokio::test]
async fn send_and_ask_are_refused_with_the_upstream_text_and_unknown_actions_are_rejected() {
    let root_dir = channel_root();

    let channel = Arc::new(NativeSupervisorChannel::with_root(root_dir.path().to_path_buf()));
    channel.bind_services(Arc::new(RecordingServices {
        session_id: "session-parent-1".to_string(),
        ..Default::default()
    }));
    let tool = SubagentSupervisorTool::new(channel.clone());

    for action in ["send", "ask"] {
        let err = tool
            .execute(
                ToolCallId::from("c"),
                serde_json::json!({ "action": action, "message": "x" }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await
            .expect_err("send/ask are refused on the parent side");
        assert!(
            err.to_string().contains("Child agents initiate asks with contact_supervisor"),
            "`{action}` must point the model at the child-side tool: {err}"
        );
    }

    let err = tool
        .execute(
            ToolCallId::from("c"),
            serde_json::json!({ "action": "frobnicate" }),
            CancelToken::new(),
            Box::new(|_| {}),
        )
        .await
        .expect_err("an unadvertised action is rejected");
    assert!(err.to_string().contains("Unsupported intercom action: frobnicate"));

    // Every value the schema advertises must have a dispatch arm (the crate's own invariant).
    let advertised: Vec<String> = tool.parameters()["properties"]["action"]["enum"]
        .as_array()
        .expect("the action enum is advertised")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect();
    assert_eq!(advertised, ["list", "send", "ask", "reply", "pending", "status"]);
    for action in &advertised {
        let outcome = tool
            .execute(
                ToolCallId::from("c"),
                serde_json::json!({ "action": action, "message": "x" }),
                CancelToken::new(),
                Box::new(|_| {}),
            )
            .await;
        match outcome {
            Ok(_) => {}
            Err(e) => assert!(
                !e.to_string().starts_with("Unsupported intercom action"),
                "advertised action `{action}` has no dispatch arm"
            ),
        }
    }
    channel.dispose();
}

/// `resolvePendingRequest`'s ambiguity guards (`native-supervisor-channel.ts:578-594`): with two
/// blocking requests outstanding a bare `reply` must REFUSE rather than pick one, and a `to` that
/// matches both must refuse too — answering the wrong blocked child is worse than answering none.
#[tokio::test]
async fn an_ambiguous_reply_is_refused_rather_than_guessed() {
    let root_dir = channel_root();

    let services = Arc::new(RecordingServices {
        session_id: "session-parent-1".to_string(),
        ..Default::default()
    });
    let channel = Arc::new(NativeSupervisorChannel::with_root(root_dir.path().to_path_buf()));
    channel.bind_services(services);

    let mut children = Vec::new();
    for index in 0..2usize {
        let dir = native_supervisor::resolve_supervisor_channel_dir_in(root_dir.path(), "run-C", "worker", index);
        native_supervisor::ensure_supervisor_channel_dir(&dir).expect("channel dirs");
        let mut metadata = child_metadata(&dir, "session-parent-1");
        metadata.run_id = "run-C".to_string();
        metadata.agent = "worker".to_string();
        metadata.child_index = index;
        metadata.child_target = Some(format!("subagent-worker-run-c-{}", index + 1));
        children.push(tokio::spawn(async move {
            native_supervisor::send_supervisor_request(
                &metadata,
                SupervisorReason::NeedDecision,
                Some("which?"),
                None,
                &CancelToken::new(),
            )
            .await
        }));
    }

    for _ in 0..80 {
        drop(channel.poll_once());
        if channel.pending().len() == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(channel.pending().len(), 2, "both children must be pending");

    let err = channel
        .reply(None, None, "pick one")
        .await
        .expect_err("a bare reply with two outstanding requests must refuse");
    assert!(err.contains("Multiple pending supervisor requests need replies. Use replyTo."));

    // `to` naming the shared agent name matches BOTH -> also refused.
    let err = channel
        .reply(None, Some("worker"), "pick one")
        .await
        .expect_err("an ambiguous `to` must refuse");
    assert!(err.contains("Multiple pending supervisor requests match 'worker'. Use replyTo."));

    // An unknown replyTo names nothing.
    let err = channel
        .reply(Some("no-such-id"), None, "x")
        .await
        .expect_err("an unknown replyTo must refuse");
    assert!(err.contains("No pending supervisor request found for replyTo 'no-such-id'."));

    // A `to` naming ONE child's own presence label resolves unambiguously and unblocks that child.
    channel
        .reply(None, Some("subagent-worker-run-c-1"), "answer for the first")
        .await
        .expect("a unique `to` resolves");
    assert_eq!(channel.pending().len(), 1, "only the answered one leaves the map");

    for child in children {
        child.abort();
    }
    channel.dispose();
}

// =================================================================================================
// (d) REGISTRATION — the half every test above leaves unexercised.
//
// The tests above construct `SubagentSupervisorTool::new(channel)` directly, so deleting
// `api.register_tool(Arc::new(SubagentSupervisorTool::new(...)))` from `NativeExtension::init`'s
// `Full` arm leaves every one of them green — while the orchestrator's model has no way to answer a
// blocked child at all, which is the whole point of the channel. G106 was therefore unexercised at
// BOTH ends: the parent's tool was never proved reachable, and the child's `contact_supervisor` is
// registered by a different extension entirely.
//
// This drives a REAL `SessionBuilder`-assembled session (scripted faux LLM responses only, no
// subprocess) whose scripted response is a `subagent_supervisor` call, and asserts the session's own
// event stream shows that call dispatching and returning the tool's OWN text — the same shape the
// sibling `wait_tool_registration_integration.rs` uses for `wait`.
// =================================================================================================

/// The load-bearing G106 parent-end assertion: a model in the orchestrator session can call
/// `subagent_supervisor`, and the call reaches THIS extension's registered tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_supervisor_tool_is_registered_and_dispatches_on_a_real_session() {
    use cyrup_ext_subagents::extension::SubagentsExtension;
    use cyrup_ext_subagents::registration::SubagentExtensionConfig;
    use cyrup_test_support::harness::{create_harness_with_extensions, HarnessOptions};
    use cyrup_test_support::response::FauxResponse;

    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");
    // Pin EVERY env var the alias gate reads, because this test asserts on a precondition it must
    // therefore actually control (see the `FauxResponse::tool_call("intercom", …)` comment below).
    // `intercom_supervisor_channel_available` (`src/native_supervisor.rs:1692-1712`) is
    // `env_opt_in || config exists`, and its `env_opt_in` term reads `CYRUP_INTERCOM` — the
    // documented product opt-in, exported on developer machines and CI runners alike. Left
    // unscrubbed it made the gate true, the alias unregistered, and this test fail with
    // `Tool 'intercom' not found` on any box that sets it. `CYRUP_CODING_AGENT_DIR` is scrubbed
    // too because `intercom_agent_dir_from` (`src/native_supervisor.rs:1770-1785`) reads it BEFORE
    // `CYRUP_HOME`, so an ambient value would silently defeat the tempdir isolation below.
    // The guards must be installed BEFORE the extension is constructed: `SubagentsExtension::init`
    // is what reads the env.
    // Pinned on the CONFIG, not on the process. Both gates read through the crate's injectable
    // `&dyn Fn(&str) -> Option<String>` resolver, so `None` ("unset") scrubs an ambient value just
    // as effectively as `remove_var` did — and nothing global moves, so no lock and no `unsafe`.
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            env_overrides: [
                ("CYRUP_INTERCOM".to_string(), None),
                ("CYRUP_CODING_AGENT_DIR".to_string(), None),
                (
                    "CYRUP_HOME".to_string(),
                    Some(home.path().display().to_string()),
                ),
            ]
            .into_iter()
            .collect(),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            FauxResponse::tool_call(
                "subagent_supervisor",
                serde_json::json!({ "action": "status" }),
            ),
            // ...and upstream's SECOND parent registration, under the bare name. On an
            // orchestrator that never installed intercom (this one: no `intercom/config.json`,
            // no `CYRUP_INTERCOM`) nothing else owns the name, so the alias must take it.
            FauxResponse::tool_call("intercom", serde_json::json!({ "action": "status" })),
            FauxResponse::text("observed the supervisor status"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds a real session with the subagents extension loaded");

    let events = harness.run("check the supervisor channel").await;

    // No teardown: nothing process-global was ever changed, so there is nothing to restore and
    // no way for this test to leak state into a sibling.
    let events = events.expect("the turn completes without a transport/session-level error");

    let starts: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionStart { tool_name, .. } => {
                Some(tool_name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        starts,
        vec!["subagent_supervisor", "intercom"],
        "BOTH parent registrations must be live on the orchestrator session and actually dispatch \
         (`native-supervisor-channel.ts:636-637`) — without the first a blocked child has nobody \
         who can reply, and without the second the bare name every pre-native-channel prompt \
         reaches for resolves to no tool at all; got: {events:#?}"
    );

    let ends: Vec<(&str, String, bool)> = events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } => Some((tool_name.as_str(), result.to_string(), *is_error)),
            _ => None,
        })
        .collect();
    assert_eq!(ends.len(), 2, "expected one tool_execution_end per call; got: {events:#?}");
    for (tool_name, result, is_error) in &ends {
        assert!(
            !is_error,
            "an idle channel is not an error condition ({tool_name}); result: {result}"
        );
        assert!(
            result.contains("Native supervisor channel active."),
            "the result must be the supervisor channel's OWN text — proving this extension's tool \
             serviced the call, not a same-named stand-in ({tool_name}); got: {result}"
        );
    }
}

/// The other half of the same gate: a fanout child must NOT get `subagent_supervisor`. Upstream
/// registers the parent tools from `createNativeSupervisorChannel(...).start()`, which only the
/// orchestrator extension reaches (`extension/index.ts:373,757` @v0.43.0); `fanout-child.ts` registers one
/// tool and starts no channel. A child that could answer supervisor requests would be answering its
/// PARENT's, which is exactly the confusion the channel's session scoping exists to prevent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fanout_child_registers_no_supervisor_tool() {
    use cyrup_ext::native::{InitApi, NativeExtension};
    use cyrup_ext_subagents::extension::{RegistrationMode, SubagentsExtension};
    use cyrup_ext_subagents::registration::SubagentExtensionConfig;

    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");
    let sandboxed = || SubagentExtensionConfig {
        roots: Roots::sandboxed(home.path()),
        ..SubagentExtensionConfig::default()
    };

    let child = SubagentsExtension::with_mode(
        sandboxed(),
        work_dir.path().to_path_buf(),
        RegistrationMode::ChildSafe,
    );
    let mut child_api = InitApi::new();
    let child_init = child.init(&mut child_api).await;

    let full = SubagentsExtension::with_config_and_cwd(sandboxed(), work_dir.path().to_path_buf());
    let mut full_api = InitApi::new();
    let full_init = full.init(&mut full_api).await;

    child_init.expect("child-safe init succeeds");
    full_init.expect("full init succeeds");

    // `InitApi` exposes no tool list, so the two arms are distinguished by the one subscription
    // only `Full` declares — the same arm the supervisor-tool registration and
    // `supervisorChannel.start()` live in (`extension/index.ts:757` fires from session start).
    assert!(
        !child_api
            .subscriptions()
            .contains(cyrup_ext::EventKind::SessionStart),
        "sanity: the ChildSafe arm installs no lifecycle subscriptions, so it never reaches the \
         supervisor channel's `start()` either"
    );
    assert!(
        full_api
            .subscriptions()
            .contains(cyrup_ext::EventKind::SessionStart),
        "the Full arm MUST subscribe to SessionStart — that is where `supervisor_channel.start()` \
         and its live capability binding happen"
    );
}

// =================================================================================================
// (e) The SECOND tool at each end — G106's other half.
//
// Upstream registers TWO tools per side from one builder each: `subagent_supervisor` + an
// `intercom` alias on the parent (`native-supervisor-channel.ts:636-637`), and
// `contact_supervisor` + an `intercom` fallback on the child (`:294-321`, layered by
// `subagent-prompt-runtime.ts:271-277,324` @v0.34.0). cyrup had one per side, so the bare name `intercom` —
// the name pi-intercom uses, the name the child bridge instruction names, and the name every
// pre-native-channel prompt reaches for — resolved to nothing on exactly the orchestrator the
// native channel exists for.
// =================================================================================================

/// The child end, driven through the REAL env-based resolver a spawned child runs.
#[tokio::test]
async fn a_child_that_declared_intercom_gets_both_native_supervisor_tools() {
    use cyrup_ext::native::InitApi;

    let root_dir = channel_root();
    let root = root_dir.path().to_path_buf();
    // An agent dir with NO intercom config and no `CYRUP_INTERCOM`: the orchestrator never
    // installed intercom, so the native channel is this child's only supervisor route.
    let agent_dir = tempfile::tempdir().expect("agent dir");
    let channel_dir = root.join("supervisor-channels").join("run-e-reviewer-0");
    native_supervisor::ensure_supervisor_channel_dir(&channel_dir).expect("channel dirs");

    let env = |declared: &'static str| {
        let agent_dir = agent_dir.path().display().to_string();
        let channel_dir = channel_dir.display().to_string();
        move |key: &str| match key {
            "CYRUP_CODING_AGENT_DIR" => Some(agent_dir.clone()),
            "CYRUP_SUBAGENT_SUPERVISOR_CHANNEL_DIR" => Some(channel_dir.clone()),
            "CYRUP_SUBAGENT_RUN_ID" => Some("run-e".to_string()),
            "CYRUP_SUBAGENT_CHILD_AGENT" => Some("reviewer".to_string()),
            "CYRUP_SUBAGENT_CHILD_INDEX" => Some("0".to_string()),
            "CYRUP_SUBAGENT_ORCHESTRATOR_SESSION_ID" => Some("sess-e".to_string()),
            "CYRUP_SUBAGENT_REQUIRED_TOOLS" => Some(declared.to_string()),
            _ => None,
        }
    };

    // Declaring `intercom` gets BOTH tools.
    let runtime = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from(&env(
        r#"["read","intercom"]"#,
    ))
    .expect("a metadata-carrying child with no installed intercom gets a runtime");
    let mut api = InitApi::new();
    runtime.init(&mut api).await.expect("init");

    // Declaring no `intercom` gets `contact_supervisor` only — the plain-child case, and the
    // reason the fallback cannot simply always register.
    let plain = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from(&env(
        r#"["read","bash"]"#,
    ))
    .expect("still gets contact_supervisor");
    let mut plain_api = InitApi::new();
    plain.init(&mut plain_api).await.expect("init");

    // `InitApi` exposes no tool list, so assert on the tools the resolver actually built — the
    // same objects `init` hands to `register_tool`.
    let contact = native_supervisor::NativeContactSupervisorTool::new(child_metadata(
        &channel_dir,
        "sess-e",
    ));
    let fallback =
        native_supervisor::NativeChildIntercomTool::new(child_metadata(&channel_dir, "sess-e"));
    assert_eq!(cyrup_core::Tool::name(&contact), "contact_supervisor");
    assert_eq!(cyrup_core::Tool::name(&fallback), "intercom");
    assert_eq!(
        cyrup_core::Tool::description(&fallback),
        "Native supervisor-channel intercom fallback for subagents. Prefer contact_supervisor when \
         available.",
        "upstream `native-supervisor-channel.ts:306`, verbatim"
    );

    assert!(
        native_supervisor::native_child_intercom_fallback_should_register(
            &env(r#"["read","intercom"]"#),
            agent_dir.path()
        ),
        "a child whose agent declared `intercom` must be given a tool by that name — otherwise it \
         was launched with a declared tool it does not have"
    );
    assert!(
        !native_supervisor::native_child_intercom_fallback_should_register(
            &env(r#"["read","bash"]"#),
            agent_dir.path()
        ),
        "a plain child must NOT claim the `intercom` name"
    );
}

/// The child `intercom` fallback's own dispatch is upstream's, not a re-skin of
/// `contact_supervisor`: four serviced actions and one verbatim refusal.
#[tokio::test]
async fn the_child_intercom_fallback_services_pis_four_actions_and_refuses_the_rest() {
    let root_dir = channel_root();
    let root = root_dir.path().to_path_buf();
    let channel_dir = root.join("supervisor-channels").join("run-f-reviewer-0");
    native_supervisor::ensure_supervisor_channel_dir(&channel_dir).expect("channel dirs");
    let tool =
        native_supervisor::NativeChildIntercomTool::new(child_metadata(&channel_dir, "sess-f"));

    let call = |args: serde_json::Value| {
        let tool = &tool;
        async move {
            cyrup_core::Tool::execute(
                tool,
                ToolCallId::from("fallback"),
                args,
                CancelToken::new(),
                Box::new(|_u: cyrup_core::ToolUpdate| {}),
            )
            .await
        }
    };

    let status = call(serde_json::json!({ "action": "status" }))
        .await
        .expect("status is local and never blocks");
    assert!(
        format!("{:?}", status.content).contains("Native supervisor channel is active."),
        "upstream `:311`, verbatim; got {:?}",
        status.content
    );

    let list = call(serde_json::json!({ "action": "list" }))
        .await
        .expect("list is local and never blocks");
    assert!(
        format!("{:?}", list.content)
            .contains("Supervisor session available through contact_supervisor."),
        "upstream `:312`, verbatim; got {:?}",
        list.content
    );

    // `send` is the fire-and-forget progress update, so it returns without a supervisor.
    let sent = call(serde_json::json!({ "action": "send", "message": "halfway" }))
        .await
        .expect("send is fire-and-forget");
    assert!(
        format!("{:?}", sent.content).contains("Supervisor progress update queued."),
        "got {:?}",
        sent.content
    );
    // ...and it really wrote a request into the channel, so the parent's poller can see it.
    let written: Vec<_> = std::fs::read_dir(channel_dir.join("requests"))
        .expect("the requests directory must exist")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(written.len(), 1, "`send` must reach the real file channel");

    let refused = call(serde_json::json!({ "action": "reply", "replyTo": "x" }))
        .await
        .expect_err("`reply` belongs to the PARENT tool");
    assert!(
        refused
            .to_string()
            .contains("Native child intercom supports status, list, send, and ask."),
        "upstream `:315`, verbatim; got {refused}"
    );
}
