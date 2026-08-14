//! SUBA-004 end-to-end proof: the `wait` tool is actually REGISTERED on a real, fully-wired
//! session and actually dispatches.
//!
//! The defect this file guards is not "a function is missing" — it is that an orchestrator had no
//! way at all to block on a background subagent run, because `extension.rs` registered exactly one
//! tool (`subagent`) and nothing else. A unit test on the wait loop cannot catch that: the loop
//! could be perfect and still unreachable by the model. So this test drives a REAL
//! `SessionBuilder`-assembled `AgentSession` (via `cyrup-test-support`'s harness, scripted faux LLM
//! responses only) whose scripted response is a `wait` tool call, and asserts the session's own
//! event stream shows that call being dispatched and returning wait's own text.
//!
//! Deliberately fixture-free (unlike `extension_end_to_end_smoke.rs`): `wait` spawns nothing, so
//! this file needs no `test-fixtures` gate and no `CYRUP_SUBAGENT_BINARY` override — which also
//! means a regression here can never be masked by a missing fixture binary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use tokio::sync::Mutex;

use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_test_support::harness::{create_harness_with_extensions, HarnessOptions};
use cyrup_test_support::response::FauxResponse;

/// `SubagentsExtension::init` (`RegistrationMode::Full`) runs its T6 startup housekeeping —
/// async/results root creation under `CYRUP_HOME` — at `init()` time, so every test here sandboxes
/// that process-global var and serializes on this lock, mirroring every sibling integration file.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

fn tool_starts(events: &[cyrup_session_svc::AgentSessionEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionStart { tool_name, .. } => {
                Some(tool_name.as_str())
            }
            _ => None,
        })
        .collect()
}

fn tool_ends(
    events: &[cyrup_session_svc::AgentSessionEvent],
) -> Vec<(&str, &serde_json::Value, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } => Some((tool_name.as_str(), result, *is_error)),
            _ => None,
        })
        .collect()
}

/// The load-bearing SUBA-004 assertion: a model can call `wait`, and the call reaches this
/// extension's real registered tool. Before the fix the session's tool registry had no `wait` at
/// all, so this same scripted call surfaced as an unknown-tool error instead of wait's own summary.
///
/// With no background runs in the fixture cwd, `wait` returns its "nothing to wait for" summary
/// immediately — which is exactly what makes this a fast, spawn-free registration proof rather than
/// a duplicate of the blocking-behavior unit tests in `background::wait`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_wait_tool_is_registered_and_dispatches_on_a_real_session() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");
    // SAFETY: scoped, mutex-serialized env mutation for the duration of this one test, mirroring
    // every sibling `tests/*_integration.rs` file in this crate (Rust 2024 requires `unsafe` for
    // `set_var`; this file is a separate compilation unit from the crate's `#![forbid(unsafe_code)]`
    // `lib.rs`).
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            FauxResponse::tool_call("wait", serde_json::json!({})),
            FauxResponse::text("observed the wait result"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds a real session with the subagents extension loaded");

    let events = harness.run("wait for the background subagents").await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_HOME");
    }

    let events = events.expect("the turn completes without a transport/session-level error");

    assert_eq!(
        tool_starts(&events),
        vec!["wait"],
        "the `wait` tool must be registered on the live session and actually dispatch; got: \
         {events:#?}"
    );

    let ends = tool_ends(&events);
    assert_eq!(ends.len(), 1, "expected exactly one tool_execution_end; got: {events:#?}");
    let (tool_name, result, is_error) = ends[0];
    assert_eq!(tool_name, "wait");
    assert!(!is_error, "an empty async root is not an error condition; result: {result:#?}");
    let text = result.to_string();
    assert!(
        text.contains("No active async runs in this session. Nothing to wait for."),
        "the result must be wait's OWN summary — proving this extension's tool serviced the call, \
         not some same-named stand-in; got: {text}"
    );

    assert!(
        events.iter().any(|e| e.kind() == "agent_end"),
        "the turn must reach agent_end after the wait result is consumed; got: {events:#?}"
    );
}

/// A fanout child must NOT get `wait`: it has no business blocking on its parent's whole async
/// root, the same reasoning that makes `control_status`'s no-id listing child-unsafe. The
/// `ChildSafe` registration arm therefore registers only the restricted `subagent` tool.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fanout_child_does_not_get_the_wait_tool() {
    use cyrup_ext::native::{InitApi, NativeExtension};
    use cyrup_ext_subagents::extension::RegistrationMode;

    let _guard = ENV_MUTATION_LOCK.lock().await;
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");
    // SAFETY: as above — scoped, mutex-serialized, restored below.
    unsafe {
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let child = SubagentsExtension::with_mode(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
        RegistrationMode::ChildSafe,
    );
    let mut api = InitApi::new();
    let init = child.init(&mut api).await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_HOME");
    }
    init.expect("child-safe init succeeds");

    assert!(
        !api.subscriptions().contains(cyrup_ext::EventKind::SessionStart),
        "sanity: this really is the restricted ChildSafe surface, which installs no lifecycle \
         subscriptions"
    );
}
