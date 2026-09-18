//! UW-5 reachability: an `ask`-tier tool inside a subagent child is APPROVED by a real model turn
//! and actually runs — and every way of not reaching a decision still denies.
//!
//! # Why this file exists
//!
//! `SubagentPromptRuntime`'s permission gate is the child-side half of pi's
//! `registerPermissionGate`. Its `ask` tier means *ask a model*, because a subagent child has no
//! human to reach. Until UW-5 the bound `WatchdogPermissionAgent` was `NoDecisionPermissionAgent`,
//! whose `decide` returned `Ok(None)` — so every `ask`-tier tool in every subagent denied as
//! `malformed`. That is a live production defect, not a missing feature, and it got worse when a
//! real merged policy started reaching children.
//!
//! The in-crate `prompt_runtime::permission_gate_tests` already drive the real extension surface
//! (env resolver -> `prompt_runtime_extension_from` -> `init` -> `on_event`), but they cannot reach
//! a provider: nothing calls `set_host_services` there, so the arbiter has no session to resolve a
//! model against. This file is the same production surface WITH a live session behind it.
//!
//! # What is production here, and what is scripted
//!
//! Production: the real `prompt_runtime_extension_from` (which builds the real
//! `ModelTurnPermissionAgent` and the real session-context getter), a real `SessionBuilder`
//! session with the real `LiveHostServices`, the real `write` built-in, the real
//! `request_watchdog_permission` with its audit pair, redaction, `agentEndTimeoutMs` bound and
//! fail-closed mapping. Scripted: only the LLM.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::sync::Arc;

use cyrup_ext::native::NativeExtension;
use cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from;
use cyrup_ext_subagents::watchdog::child_status::{
    CHILD_WATCHDOG_CONFIG_ENV, encode_child_watchdog_config, resolve_child_watchdog_config,
};
use cyrup_ext_subagents::watchdog::permission_arbiter::{
    PERMISSION_AUDIT_PATH_ENV, PERMISSION_POLICY_ENV,
};
use cyrup_ext_subagents::watchdog::settings::default_watchdog_config;
use cyrup_test_support::harness::{Harness, HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::{FauxResponse, FauxToolCall};

/// An ARMED child watchdog config — without it the arbiter takes its "the child watchdog is
/// disabled" arm (`permission-arbiter.ts:74 @v0.68.0`) and denies before any turn.
fn armed_child_config() -> String {
    let mut config = default_watchdog_config();
    config.enabled = true;
    config.children.enabled = true;
    config.agent_end_timeout_ms = 30_000;
    encode_child_watchdog_config(resolve_child_watchdog_config(&config, None, None, None).as_ref())
        .expect("an enabled child watchdog encodes")
}

/// A `watchdog_permission_decision` call as the arbiter model would emit it.
fn decision(verdict: &str, reason: &str) -> FauxResponse {
    FauxResponse {
        tool_calls: vec![FauxToolCall::new(
            "watchdog_permission_decision",
            serde_json::json!({ "decision": verdict, "reason": reason }),
        )],
        ..FauxResponse::default()
    }
}

/// Build a real session carrying the real child prompt-runtime extension, armed with an `ask` rule
/// for `write`, and drive one turn whose scripted response calls `write`.
///
/// Returns `(the harness, the audit lines, the tool_execution_end rows)`. The HARNESS comes back
/// because the session's `write` resolves `arbiter-probe.txt` against `harness.cwd()` — the only
/// directory in which "the denied write touched nothing" is a statement about this session rather
/// than about the cargo target dir the test process happens to run in.
async fn run_ask_tier_turn(
    arbiter_script: Vec<FauxResponse>,
) -> (Harness, Vec<serde_json::Value>, Vec<(String, bool, String)>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let audit = dir.path().join("permission-audit.jsonl");
    let audit_path = audit.display().to_string();
    let child_config = armed_child_config();

    // The REAL env-resolved construction: this is `prompt_runtime_extension_for_env`'s body with an
    // injected lookup, so the gate, the arbiter and the session-context getter are all built by
    // production code.
    let extension: Arc<dyn NativeExtension> =
        prompt_runtime_extension_from(&move |key| match key {
            PERMISSION_POLICY_ENV => Some("{\"write\":\"ask\"}".to_string()),
            PERMISSION_AUDIT_PATH_ENV => Some(audit_path.clone()),
            CHILD_WATCHDOG_CONFIG_ENV => Some(child_config.clone()),
            _ => None,
        })
        .expect("the runtime always builds with no tool budget in this env")
        .expect("a policy alone arms the child runtime");

    let mut responses = vec![FauxResponse::tool_call(
        "write",
        serde_json::json!({
            "path": "arbiter-probe.txt",
            "content": "written only if the arbiter approved\n",
            "apiKey": "sk-abcdefghijkl",
        }),
    )];
    responses.extend(arbiter_script);
    // The outer session's closing turn, plus slack for the child watchdog's own boundary review.
    responses.push(FauxResponse::text("done"));
    responses.push(FauxResponse::text("done"));
    responses.push(FauxResponse::text("done"));

    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension],
        responses,
        queue_responses: true,
        ..HarnessOptions::default()
    })
    .await
    .expect("a real, fully-wired AgentSession with the real child prompt runtime");

    let events = harness
        .run("please write the probe file")
        .await
        .expect("the turn completes");

    let tool_ends = events
        .iter()
        .filter_map(|e| match e {
            cyrup_session_svc::AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                is_error,
                result,
                ..
            } => Some((tool_name.clone(), *is_error, result.to_string())),
            _ => None,
        })
        .collect();

    let lines = std::fs::read_to_string(&audit)
        .expect("the audit file was written")
        .lines()
        .map(|line| serde_json::from_str(line).expect("json"))
        .collect();
    (harness, lines, tool_ends)
}

/// **UW-5, the one that matters.** The arbiter APPROVES and the tool RUNS.
///
/// Over the gutted implementation (`NoDecisionPermissionAgent`'s `Ok(None)`) this is a `Block` with
/// `"Watchdog permission arbiter returned no decision."` and `decision: "malformed"`, so BOTH
/// assertions fail. It also fails if the arbiter is wired but cannot resolve a model — the GAP-1
/// case, where the reason becomes "the current Pi session model is unavailable" and `approved` is
/// still `false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_ask_tier_tool_is_approved_by_a_real_model_turn_and_runs() {
    let (harness, lines, tool_ends) = run_ask_tier_turn(vec![
        decision(
            "approve",
            "ARBITER_SCRIPTED_REASON: inside the worktree, reversible",
        ),
        FauxResponse::text("decision recorded"),
    ])
    .await;

    // (1) The tool actually ran — the gate returned `Noop`, not `Block`.
    let write = tool_ends
        .iter()
        .find(|(name, _, _)| name == "write")
        .unwrap_or_else(|| panic!("the approved tool must execute; rows: {tool_ends:#?}"));
    assert!(
        !write.1,
        "the approved write must not be an error result: {write:#?}"
    );
    assert!(
        !write.2.contains("Blocked by pi-subagents permission rule"),
        "the approved write must not be blocked: {write:#?}"
    );

    // (2) Both audit records, and the decision is the MODEL's own.
    assert_eq!(lines.len(), 2, "the request/decision pair: {lines:#?}");
    assert_eq!(lines[0]["type"], serde_json::json!("permission.request"));
    assert_eq!(lines[1]["decision"], serde_json::json!("approve"));
    assert_eq!(lines[1]["approved"], serde_json::json!(true));
    assert_eq!(
        lines[1]["reason"],
        serde_json::json!("ARBITER_SCRIPTED_REASON: inside the worktree, reversible")
    );

    // (3) The redaction still feeds the audit (and therefore the arbiter's own prompt).
    let preview = lines[0]["preview"].as_str().expect("preview");
    assert!(preview.contains("[redacted]"), "{preview}");
    assert!(!preview.contains("abcdefghijkl"), "{preview}");

    // (4) The file is really there — in the session's cwd, which is where the `write` built-in
    // resolves a relative path. This is the CONTROL that makes the denied test's absence
    // assertion mean something: the same expression, over the same directory, is true here.
    assert!(
        harness.cwd().join("arbiter-probe.txt").exists(),
        "an approved write must land on disk; cwd: {}",
        harness.cwd().display()
    );
}

/// The other half: a real `deny` blocks, and the reason the child sees is the MODEL's own sentence,
/// not a generic one. Gutted, the reason would be the `malformed` sentence instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_denied_ask_tier_tool_blocks_with_the_models_own_reason() {
    let (harness, lines, tool_ends) = run_ask_tier_turn(vec![
        decision(
            "deny",
            "ARBITER_SCRIPTED_DENIAL: writes outside the task scope",
        ),
        FauxResponse::text("decision recorded"),
    ])
    .await;

    assert_eq!(lines[1]["decision"], serde_json::json!("deny"));
    assert_eq!(lines[1]["approved"], serde_json::json!(false));
    let blocked = tool_ends
        .iter()
        .find(|(name, _, result)| {
            name == "write" && result.contains("Blocked by pi-subagents permission rule")
        })
        .unwrap_or_else(|| panic!("a denied tool must be blocked; rows: {tool_ends:#?}"));
    assert!(
        blocked
            .2
            .contains("ARBITER_SCRIPTED_DENIAL: writes outside the task scope"),
        "the block must carry the MODEL's reason: {blocked:#?}"
    );
    // Nothing reached the filesystem — asserted against the SESSION's cwd, the directory the
    // `write` built-in resolves `arbiter-probe.txt` against and the one in which
    // `an_ask_tier_tool_is_approved_by_a_real_model_turn_and_runs` finds the very same file.
    assert!(
        !harness.cwd().join("arbiter-probe.txt").exists(),
        "a denied write must not touch the filesystem; cwd: {}",
        harness.cwd().display()
    );
}

/// **The fail-OPEN guard (definition-of-done item 2).** With the real agent bound and the provider
/// ERRORING, the ask must still deny — `approved: false`, recorded as `error`.
///
/// This single assertion is what stands between UW-5 and a silent fail-open regression: an
/// implementation that treats "the turn did not answer" as "allowed" passes both tests above and
/// fails this one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_provider_failure_during_the_arbiter_turn_still_denies() {
    let (harness, lines, tool_ends) =
        run_ask_tier_turn(vec![FauxResponse::error("arbiter transport exploded")]).await;

    assert_eq!(lines.len(), 2, "both audit records, always: {lines:#?}");
    assert_eq!(
        lines[1]["approved"],
        serde_json::json!(false),
        "a failed arbiter turn must NEVER approve: {lines:#?}"
    );
    assert_eq!(
        lines[1]["decision"],
        serde_json::json!("error"),
        "a transport failure is `error`, not `malformed` — the model never answered: {lines:#?}"
    );
    assert!(
        tool_ends.iter().any(|(name, _, result)| name == "write"
            && result.contains("Blocked by pi-subagents permission rule")),
        "the tool must be blocked; rows: {tool_ends:#?}"
    );
    assert!(
        !harness.cwd().join("arbiter-probe.txt").exists(),
        "a failed arbiter turn must leave the filesystem untouched"
    );
}

/// The model that calls NO tool: upstream's `malformed` arm (`permission-arbiter.ts:130 @v0.68.0`).
/// This is what the previous stub produced for EVERY ask; here it is one arm among several, and it
/// still denies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_arbiter_turn_that_calls_no_tool_denies_as_malformed() {
    let (harness, lines, _) =
        run_ask_tier_turn(vec![FauxResponse::text("I would rather not say.")]).await;
    assert_eq!(lines[1]["approved"], serde_json::json!(false));
    assert_eq!(lines[1]["decision"], serde_json::json!("malformed"));
    assert_eq!(
        lines[1]["reason"],
        serde_json::json!("Watchdog permission arbiter returned no decision.")
    );
    assert!(!harness.cwd().join("arbiter-probe.txt").exists());
}

/// **Fail-closed: MALFORMED MODEL OUTPUT.** The arbiter model does call its one tool, but names a
/// verdict outside `approve`/`deny`. `WatchdogPermissionDecisionTool::record` refuses it — one of
/// the five validation errors at `permission-arbiter.ts:85-86 @v0.68.0` — so the call comes back to
/// the model as a tool ERROR, nothing is latched, `decide` reports `Ok(None)`, and the ask denies.
///
/// This is the arm a fail-OPEN would most plausibly hide in: unlike a transport failure, the turn
/// SUCCEEDED and the model DID answer. It must still deny.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_arbiter_that_names_an_invalid_verdict_denies_and_nothing_runs() {
    let (harness, lines, tool_ends) = run_ask_tier_turn(vec![
        decision("maybe", "ARBITER_SCRIPTED_HEDGE: I could go either way"),
        FauxResponse::text("never mind"),
    ])
    .await;

    assert_eq!(
        lines[1]["approved"],
        serde_json::json!(false),
        "an unparseable verdict must NEVER approve: {lines:#?}"
    );
    assert_eq!(lines[1]["decision"], serde_json::json!("malformed"));
    assert_eq!(
        lines[1]["reason"],
        serde_json::json!("Watchdog permission arbiter returned no decision.")
    );
    assert!(
        tool_ends.iter().any(|(name, _, result)| name == "write"
            && result.contains("Blocked by pi-subagents permission rule")),
        "the tool must be blocked; rows: {tool_ends:#?}"
    );
    assert!(
        !harness.cwd().join("arbiter-probe.txt").exists(),
        "and it must not have run"
    );
}
