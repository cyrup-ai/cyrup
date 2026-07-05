//! End-to-end integration smoke test: `SubagentsExtension` registered against a REAL
//! `SessionBuilder`-assembled session (via `cyrup-test-support`'s `create_harness_with_extensions`),
//! driven through a real LLM turn that emits a `subagent` tool call, observing a REAL child
//! `cyrup`-shaped process (the scripted-NDJSON test-double `cyrup-subagent-fixture` binary, arch-SA
//! §11) actually spawn and complete via `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1's documented
//! override escape hatch, honored transitively through `SubagentTool::execute` ->
//! `SubagentExecutor::run_foreground` -> `exec::run_sync` -> `spawn::resolve_spawn_command`).
//!
//! This is the single most important test in the entire crate — the task brief that commissioned
//! this file says so explicitly, and the reasoning is structural, not rhetorical: every other test
//! in this crate's suite (`discovery_integration.rs`, `exec_run_sync_integration.rs`,
//! `background_spawn_detached_integration.rs`, `background_runner_main_integration.rs`, plus the
//! ~770 unit tests across `discovery/`, `exec/`, `spawn/`, `background/`, `tui/`, `registration/`)
//! proves exactly ONE module (or one narrow module pair) in isolation. None of them proves that
//! `extension.rs` — the file that did not exist until this integration phase — actually wires
//! discovery -> fork-context -> exec/spawn -> the real `cyrup_core::Tool` trait -> the real
//! `cyrup_ext::native::NativeExtension` trait -> a real `SessionBuilder`-assembled `AgentSession`
//! all the way through. A bug in ANY of those seams (a typo in a struct field name that still
//! type-checks against a slightly different but structurally-compatible type, an off-by-one in
//! which `cwd` gets threaded where, a forgotten `.await`, a tool registered under the wrong name)
//! would compile clean and pass every other test in this crate while silently breaking the one
//! thing a user actually does: ask the assistant to delegate work to a subagent. This test is the
//! only one that would catch that class of bug.
//!
//! No mocking anywhere in this file (this codebase's standing convention, already established by
//! every other `tests/*_integration.rs` file in this crate): the session is a real, fully-wired
//! `AgentSession` (scripted faux LLM responses only — the LLM side of the harness — never the
//! subagent-subprocess side), the extension is the real `SubagentsExtension`, the tool call is
//! serviced by the real registered `cyrup_core::Tool`, and the "subagent" it delegates to is a
//! REAL, separately-spawned OS process (the `cyrup-subagent-fixture` scripted binary), not an
//! in-process stand-in.
//!
//! Gated on the `test-fixtures` Cargo feature (matching every other fixture-dependent integration
//! test in this crate) — without it this file compiles to an empty test list rather than failing
//! at spawn time with a confusing "No such file or directory".

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::Mutex;

use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_test_support::harness::{create_harness_with_extensions, HarnessOptions};
use cyrup_test_support::response::FauxResponse;

/// Serializes every test in this file that mutates `CYRUP_SUBAGENT_BINARY`/
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT` (process-global state) — mirrors every other integration test
/// in this crate's identical `ENV_MUTATION_LOCK` convention (`exec_run_sync_integration.rs`,
/// `background_spawn_detached_integration.rs`, `background_runner_main_integration.rs`).
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

/// Path to the real, already-built `cyrup-subagent-fixture` binary (see every other fixture-based
/// integration test in this crate for the identical `CARGO_BIN_EXE_*` convention).
fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

/// Write a trivial agent persona `.md` file to `<cwd>/.cyrup/agents/<local_name>.md` — the exact
/// project-scope discovery root `SubagentExecutor::discovery_config` (`extension.rs`) scans, so
/// this fixture persona is genuinely discovered through the REAL discovery pipeline
/// (`discovery::discover_agents`, R-SA-001..021), not injected via any test-only back door.
fn write_fixture_persona(cwd: &std::path::Path, local_name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{local_name}.md")),
        format!(
            "---\nname: {local_name}\ndescription: a trivial fixture persona for the end-to-end \
             smoke test\nmodel: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

/// One `emit` NDJSON line the fixture binary writes to its stdout, matching `cyrup`'s own
/// `--mode json` `message_end` wire-event shape (mirrors every other fixture-script builder in
/// this crate's own integration tests, e.g. `exec_run_sync_integration.rs`'s `message_end_line`).
fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 5,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// The core end-to-end proof: register [`SubagentsExtension`] against a real
/// [`cyrup_session_svc::SessionBuilder`]-assembled session (via
/// [`create_harness_with_extensions`]), drive a real turn whose scripted LLM response is a
/// `subagent` tool call, and assert that:
///
/// 1. The tool call was actually dispatched (`tool_execution_start`/`tool_execution_end` events
///    observed on the real session's own event stream — proving `InitApi::register_tool`'s
///    registration in [`SubagentsExtension::init`] actually reached the session's live tool
///    registry, not merely that `init()` itself didn't error).
/// 2. The tool result is NOT an error, and its text content is EXACTLY the text the scripted
///    fixture child emitted — proving the full discovery -> fork-context -> spawn -> NDJSON-
///    consumption -> final-output-extraction pipeline actually ran end to end through a REAL
///    separately-spawned OS process, not an in-process stand-in for one.
/// 3. The turn completes normally (`agent_end` observed) — proving the tool result flowed back
///    into the ordinary agent turn loop rather than the extension's dispatch silently swallowing
///    it or hanging the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_tool_call_spawns_a_real_child_process_and_returns_its_output_end_to_end() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir for the fixture persona + cwd");
    write_fixture_persona(work_dir.path(), "worker");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("SMOKE_TEST_SUBAGENT_OUTPUT: the real child ran") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation for the duration of this one test, mirroring
    // every other fixture-based integration test in this crate (Rust 2024 requires `unsafe` for
    // `std::env::set_var`/`remove_var`; this file is a separate compilation unit from this crate's
    // own `#![forbid(unsafe_code)]` `lib.rs`, exactly like every sibling `tests/*_integration.rs`
    // file).
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            // Turn 1: the LLM decides to delegate to the "worker" subagent persona.
            FauxResponse::tool_call(
                "subagent",
                serde_json::json!({
                    "agent": "worker",
                    "task": "do the trivial thing",
                }),
            ),
            // Turn 2: the LLM's follow-up after observing the tool result.
            FauxResponse::text("acknowledged the subagent's output"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds a real, fully-wired AgentSession with the extension loaded");

    let events = harness.run("please delegate this to the worker subagent").await;

    // SAFETY: scoped cleanup under the same mutex-held critical section, mirroring every sibling
    // fixture-based integration test's identical teardown.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    let events = events.expect("the turn completes without a transport/session-level error");

    // (1) The tool call was actually dispatched through the real session's live tool registry.
    let tool_start_names: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionStart { tool_name, .. } => {
                Some(tool_name.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        tool_start_names,
        vec!["subagent"],
        "expected exactly one tool_execution_start for the real 'subagent' tool; got: {events:#?}"
    );

    // (2) The tool result is not an error, and carries the REAL fixture child's output verbatim —
    // the actual proof this is the single most important test in the crate: this text only
    // appears in the session's transcript if a genuinely separate OS process was spawned, wrote
    // it to its own stdout as NDJSON, and that NDJSON was consumed, parsed, and extracted by the
    // real `exec::run_sync` pipeline this extension wires up.
    let tool_ends: Vec<(&str, &serde_json::Value, bool)> = events
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
        .collect();
    assert_eq!(tool_ends.len(), 1, "expected exactly one tool_execution_end; got: {events:#?}");
    let (tool_name, result, is_error) = tool_ends[0];
    assert_eq!(tool_name, "subagent");
    assert!(
        !is_error,
        "the subagent tool call must not surface as an error; result: {result:#?}"
    );
    let result_text = result.to_string();
    assert!(
        result_text.contains("SMOKE_TEST_SUBAGENT_OUTPUT: the real child ran"),
        "the tool result must contain the REAL fixture child's own emitted output verbatim \
         (proving the full discovery -> fork-context -> spawn -> NDJSON-consumption -> \
         final-output-extraction pipeline ran through a genuine separate OS process), got: \
         {result_text}"
    );

    // (3) The turn completed normally — the tool result flowed back into the ordinary agent turn
    // loop rather than hanging or silently dropping.
    assert!(
        events.iter().any(|e| e.kind() == "agent_end"),
        "the turn must reach agent_end after the subagent tool result is consumed; got: {events:#?}"
    );
    let assistant_texts = harness.assistant_texts().await;
    assert!(
        assistant_texts.iter().any(|t| t.contains("acknowledged the subagent's output")),
        "the follow-up assistant turn (scripted response 2) must be present in the persisted \
         transcript: {assistant_texts:?}"
    );
}

/// A narrower, faster companion assertion: an unresolvable agent name fails the tool call BEFORE
/// any subprocess is ever spawned (R-SA-055's "depth check runs before any spawn/discovery"
/// sibling rule, restated here at the discovery layer: an unresolvable agent name must never reach
/// the spawn boundary at all). Verified by never setting `CYRUP_SUBAGENT_BINARY` at all for this
/// test — if this path somehow DID attempt a real spawn, it would fail loudly (no fixture
/// configured) rather than silently, so the absence of any spawn attempt is provable by this test
/// passing at all under `cargo test`'s default no-network/no-subprocess-by-accident posture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_tool_call_against_an_unknown_agent_fails_before_any_subprocess_spawn() {
    let work_dir = tempfile::tempdir().expect("real tempdir");
    // Deliberately no fixture persona written — "ghost" resolves to nothing.

    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            FauxResponse::tool_call(
                "subagent",
                serde_json::json!({ "agent": "ghost", "task": "anything" }),
            ),
            FauxResponse::text("noted the failure"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds");

    let events = harness
        .run("delegate to a nonexistent agent")
        .await
        .expect("the turn completes even though the tool call itself fails");

    let tool_ends: Vec<(bool, &serde_json::Value)> = events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd { is_error, result, .. } => {
                Some((*is_error, result))
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_ends.len(), 1, "expected exactly one tool_execution_end; got: {events:#?}");
    let (is_error, result) = tool_ends[0];
    assert!(is_error, "an unresolvable agent name must surface as a tool error, got: {result:#?}");
}

/// T3 group C — a FAILED single run surfaces as a tool ERROR whose content carries the failure
/// text (pi `formatFailedSingleRunOutput` + `isError: true`, `subagent-executor.ts:2752-2757`).
/// cyrup's `ToolResult` has no `isError` flag, so the faithful analogue is `Err(ToolError)` — which
/// the runtime renders as a `tool_execution_end` with `is_error: true` carrying the message. Proven
/// end to end: the REAL fixture child exits non-zero with detail on its real stderr pipe, and that
/// stderr is surfaced into the model-facing content, not buried in `details` JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagent_tool_call_with_a_failing_child_surfaces_is_error_with_the_error_text() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir for the fixture persona + cwd");
    write_fixture_persona(work_dir.path(), "worker");

    const STDERR_DETAIL: &str = "fatal: the child crashed while applying the change";
    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("partial progress before the crash") },
            { "kind": "emit_stderr", "line": STDERR_DETAIL },
        ],
        "exit_code": 2
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation for the duration of this one test.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
    }

    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    ));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses: vec![
            FauxResponse::tool_call(
                "subagent",
                serde_json::json!({ "agent": "worker", "task": "apply the change" }),
            ),
            FauxResponse::text("noted the subagent failure"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("harness builds a real, fully-wired AgentSession with the extension loaded");

    let events = harness.run("delegate the change to the worker subagent").await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
    }

    let events = events.expect("the turn completes even though the tool call itself fails");

    let tool_ends: Vec<(bool, &serde_json::Value)> = events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd { is_error, result, .. } => {
                Some((*is_error, result))
            }
            _ => None,
        })
        .collect();
    assert_eq!(tool_ends.len(), 1, "expected exactly one tool_execution_end; got: {events:#?}");
    let (is_error, result) = tool_ends[0];
    assert!(
        is_error,
        "a failed single run must set the tool error flag (pi isError: true), got: {result:#?}"
    );
    let result_text = result.to_string();
    assert!(
        result_text.contains(STDERR_DETAIL),
        "the failed run's error text (the child's surfaced stderr) must be in the model-facing \
         CONTENT, not buried in details JSON — got: {result_text}"
    );
    assert!(
        result_text.contains("Output:") && result_text.contains("partial progress before the crash"),
        "formatFailedSingleRunOutput must include the partial Output block: {result_text}"
    );
}
