//! SUBA-004 end-to-end proof: the background-wait tool is actually REGISTERED on a real,
//! fully-wired session, under the name the model is told to call, and actually dispatches.
//!
//! VL-S8: that name is **`bg_wait`** — pi `wait-tool.ts:38` @v0.68.0. Upstream registers exactly
//! one tool (`:44`) and no alias, and so does this crate, which is why the three literals below
//! are `bg_wait` and not `wait`.
//!
//! The defect this file guards is not "a function is missing" — it is that an orchestrator had no
//! way at all to block on a background subagent run, because `extension.rs` registered exactly one
//! tool (`subagent`) and nothing else. A unit test on the wait loop cannot catch that: the loop
//! could be perfect and still unreachable by the model. So this test drives a REAL
//! `SessionBuilder`-assembled `AgentSession` (via `cyrup-test-support`'s harness, scripted faux LLM
//! responses only) whose scripted response is a `bg_wait` tool call, and asserts the session's own
//! event stream shows that call being dispatched and returning the wait loop's own text.
//!
//! Deliberately fixture-free (unlike `extension_end_to_end_smoke.rs`): `bg_wait` spawns nothing, so
//! this file needs no `test-fixtures` gate and no `CYRUP_SUBAGENT_BINARY` override — which also
//! means a regression here can never be masked by a missing fixture binary.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_test_support::harness::{HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::FauxResponse;

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

/// The load-bearing SUBA-004 assertion: a model can call `bg_wait`, and the call reaches this
/// extension's real registered tool. Before the fix the session's tool registry had no wait tool
/// at all, so this same scripted call surfaced as an unknown-tool error instead of the wait loop's
/// own summary.
///
/// VL-S8 added a second thing this proves: the tool is registered under pi's v0.68.0 name. A
/// session that still registered the pre-rename `"wait"` emits NO `tool_execution_start` for the
/// `bg_wait` call the faux model makes, so `tool_starts(&events)` comes back EMPTY and the first
/// assertion fires with that exact diagnosis.
///
/// With no background runs in the fixture cwd, `bg_wait` returns its "nothing to wait for" summary
/// immediately — which is exactly what makes this a fast, spawn-free registration proof rather than
/// a duplicate of the blocking-behavior unit tests in `background::wait`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bg_wait_tool_is_registered_under_its_v0_68_0_name_and_dispatches_on_a_real_session() {
    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");

    // This run names its own root rather than moving process-global state every other test in
    // this binary shares: `roots` reaches the wait tool's own root resolution
    // (`WaitDeps::for_cwd`) through the config the extension already carries.
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            FauxResponse::tool_call("bg_wait", serde_json::json!({})),
            FauxResponse::text("observed the bg_wait result"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds a real session with the subagents extension loaded");

    let events = harness.run("bg_wait for the background subagents").await;

    let events = events.expect("the turn completes without a transport/session-level error");

    assert_eq!(
        tool_starts(&events),
        vec!["bg_wait"],
        "the `bg_wait` tool must be registered on the live session under THAT name and actually \
         dispatch. An EMPTY list here means the session registered some other name (the \
         pre-VL-S8 `wait`, or `subagent_wait`) and the model's `bg_wait` call reached no tool at \
         all; got: {events:#?}"
    );

    let ends = tool_ends(&events);
    assert_eq!(
        ends.len(),
        1,
        "expected exactly one tool_execution_end; got: {events:#?}"
    );
    let (tool_name, result, is_error) = ends[0];
    assert_eq!(tool_name, "bg_wait");
    assert!(
        !is_error,
        "an empty async root is not an error condition; result: {result:#?}"
    );
    let text = result.to_string();
    assert!(
        text.contains("No active async runs in this session. Nothing to wait for."),
        "the result must be the wait loop's OWN summary — proving this extension's tool serviced \
         the call, not some same-named stand-in; got: {text}"
    );

    assert!(
        events.iter().any(|e| e.kind() == "agent_end"),
        "the turn must reach agent_end after the bg_wait result is consumed; got: {events:#?}"
    );
}

/// SUBA-232 — a fan-out child gets `bg_wait` exactly ONCE, and it is the CHILD's: owned by the
/// prompt runtime and described for a child (pi `subagent-prompt-runtime.ts:520`, `wait-tool.ts:19`
/// @ad11b7ab) — never the `ChildSafe` subagents arm's, which loads first in the same session and
/// would win the registry's first-wins race with the orchestrator's description and a user-input
/// set nothing feeds. Upstream's fan-out child registers none either (`fanout-child.ts`).
///
/// The pair is loaded into one host in launch order (`crates/cyrup/src/session_launch.rs`: the
/// subagents extension, then the prompt runtime), and the `ChildSafe` arm is checked alone too.
///
/// This replaces `a_fanout_child_does_not_get_the_bg_wait_tool`, which asserted a lifecycle
/// subscription and never looked at the tool at all.
///
/// **Proven red at HEAD** (before SUBA-232): the pair registers no `bg_wait` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fanout_child_gets_one_bg_wait_and_it_is_the_childs() {
    use cyrup_ext::NativeExtension;
    use cyrup_ext_subagents::extension::RegistrationMode;
    use cyrup_ext_subagents::prompt_runtime::{
        INHERIT_PROJECT_CONTEXT_ENV, PROMPT_RUNTIME_EXTENSION_ID, prompt_runtime_extension_from,
    };
    use cyrup_ext_subagents::spawn::nested_events::{CHILD_ENV, FANOUT_CHILD_ENV};

    let home = tempfile::tempdir().expect("home tempdir");
    let work_dir = tempfile::tempdir().expect("work tempdir");
    let child_safe = || -> Arc<dyn NativeExtension> {
        Arc::new(SubagentsExtension::with_mode(
            SubagentExtensionConfig {
                roots: Roots::sandboxed(home.path()),
                ..SubagentExtensionConfig::default()
            },
            work_dir.path().to_path_buf(),
            RegistrationMode::ChildSafe,
        ))
    };
    let host = || {
        cyrup_ext::ExtensionHost::new(cyrup_ext::facade::HostConfig {
            mode: cyrup_ext::native::ExtMode::Tui,
            has_ui: false,
            cwd: work_dir.path().to_path_buf(),
        })
    };

    // The ChildSafe arm alone registers no wait: it is not where a child's wait lives.
    let alone = host();
    alone
        .load_native(child_safe())
        .await
        .expect("the ChildSafe arm loads");
    assert!(
        alone
            .registry()
            .tool("bg_wait")
            .expect("registry")
            .is_none(),
        "the ChildSafe subagents arm must not register bg_wait"
    );

    // The real fan-out child: both extensions, in launch order.
    let runtime = prompt_runtime_extension_from(&|key| match key {
        CHILD_ENV | FANOUT_CHILD_ENV | INHERIT_PROJECT_CONTEXT_ENV => Some("1".to_string()),
        _ => None,
    })
    .expect("no tool budget, so the runtime builds")
    .expect("a child env builds the runtime");
    let pair = host();
    pair.load_native(child_safe())
        .await
        .expect("the ChildSafe arm loads");
    pair.load_native(runtime)
        .await
        .expect("the prompt runtime loads");

    let tool = pair
        .registry()
        .tool("bg_wait")
        .expect("registry")
        .expect("a fan-out child must have bg_wait");
    assert_eq!(
        pair.registry()
            .tool_owner("bg_wait")
            .expect("registry")
            .map(|id| id.to_string())
            .as_deref(),
        Some(PROMPT_RUNTIME_EXTENSION_ID),
        "the child's bg_wait is the prompt runtime's"
    );
    assert!(
        pair.registry().conflicts().expect("registry").is_empty(),
        "exactly one registration: {:?}",
        pair.registry().conflicts()
    );
    assert!(
        tool.description().contains(
            "This child runtime does not install the root session's native completion notifier."
        ),
        "{}",
        tool.description()
    );
}
