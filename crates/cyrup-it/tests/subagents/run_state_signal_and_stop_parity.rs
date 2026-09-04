//! G104/G77 — the run-state facts that were implemented but never observed.
//!
//! Five behaviours are covered here, each against a REAL OS subprocess (this crate's standing
//! convention — a subagent run is always a genuine re-exec speaking NDJSON over stdout, never an
//! in-process call):
//!
//! 1. **`SingleResult::process_signal` on the live path.** `exec::run_sync` publishes the OS signal
//!    that killed a child (`exec/mod.rs`'s `signal.startup.process_signal.clone()`, pi
//!    `execution.ts:1081` `if (signal) result.processSignal = signal;`). The field exists for exactly
//!    one reason — to make `resolveSubagentResultStatus`'s unexplained-signal → `"stopped"` branch
//!    (`intercom/result-intercom.ts:35` @v0.43.0) reachable — and nothing asserted it, so replacing
//!    it with `None` left the suite green.
//!
//! 2. **The SINGLE foreground run's out-of-band child status.** pi resolves it with
//!    `foregroundResultIntercomStatus` (`runs/foreground/subagent-executor.ts:1594-1605`, applied per
//!    child at `:1626`) — the full ladder over the real `SingleResult`. cyrup projected the result
//!    through a synthetic `StepResult` first, which carries no `process_signal`, so a signal-killed
//!    child was reported `"failed"` where upstream reports `"stopped"`.
//!
//! 3. **`action: "stop"` against a NESTED run.** pi refuses it with its own sentence
//!    (`subagent-executor.ts:4796`), never the generic not-found text.
//!
//! 4. **A REJECTED acceptance ledger on a child that exited `0`.** The second field a
//!    `chain_graph::StepResult` projection cannot see — pi `subagent-executor.ts:1597` pins
//!    `success: false` off it unconditionally, so the child reads `failed` despite the clean exit.
//!
//! 5. **A stop landing together with a timeout.** The terminal record must be `Stopped`, the
//!    hardest and least-resumable of the three verbs (`runs/background/control-channel.ts:653-655`'s drain order
//!    and `subagent-runner.ts:2956`'s mutual-exclusion guard). Both claimed orderings existed in
//!    the runner with nothing exercising them together.
//!
//! The signal-killed child is produced by pointing `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1's
//! documented verbatim override) at a tiny POSIX shell script that emits one real `message_end`
//! NDJSON record and then `kill -KILL $$`s itself. That is a genuine, externally-unexplained signal
//! death — the one thing this crate's own SIGINT→SIGTERM→SIGKILL escalation ladder cannot be used to
//! stage, because a cancel/timeout/interrupt teardown is by definition an EXPLAINED signal and
//! `isUnexplainedProcessSignal` (`runs/shared/process-signal.ts:5-19`) disqualifies it.
//!
//! Gated on the `test-fixtures` Cargo feature, matching every other fixture-based integration test
//! in this crate (`CARGO_BIN_EXE_cyrup-subagent-fixture` only exists in that build graph).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use cyrup_core::{CancelToken, Content, ModelId, Tool, ToolCallId};
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::background::{
    ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, StepState,
};
use cyrup_ext_subagents::discovery::types::{OutputMode, SystemPromptMode};
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::exec::acceptance::{AcceptanceContract, AcceptanceStatus};
use cyrup_ext_subagents::exec::fallback::ModelOverride;
use cyrup_ext_subagents::exec::output::OutputCap;
use cyrup_ext_subagents::exec::{AgentConfig, RunOptions};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::fork_context::ForkContext;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};
use cyrup_ext_subagents::spawn::depth::DepthEnvelope;
use cyrup_ext_subagents::spawn::nested_events::{
    NestedEventInput, NestedRunSummary, create_nested_route_in, write_nested_event_in,
};
use cyrup_ext_subagents::tui::intercom::{
    DeliveryChannel, IntercomPayload, NoOpClarifyChannel, NoTransportSteerChannel,
    SubagentResultStatus, resolve_single_result_status,
};

/// One `message_end` NDJSON record on the real child wire shape (`exec/ndjson.rs`).
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

/// Write an executable POSIX shell script that emits `line` as one NDJSON record on stdout and then
/// kills ITSELF with `SIGKILL` — a real child process that dies of a real, unexplained OS signal.
///
/// `CYRUP_SUBAGENT_BINARY` is honoured verbatim by `spawn::resolve_spawn_command` (R-SA-045 tier 1),
/// so this stands in for the child binary exactly the way `cyrup-subagent-fixture` does; the extra
/// argv the parent appends is simply ignored by the script, and stdin is `null` either way
/// (R-SA-046).
fn write_sigkill_child(dir: &Path, name: &str, line: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    // Single-quoted heredoc-free form: the NDJSON payload contains no single quotes.
    assert!(
        !line.contains('\''),
        "the NDJSON line must contain no single quote for this shell quoting to be safe"
    );
    std::fs::write(
        &path,
        format!("#!/bin/sh\nprintf '%s\\n' '{line}'\nkill -KILL $$\n"),
    )
    .expect("write the sigkill child script");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
        .expect("chmod +x the sigkill child script");
    path
}

fn base_agent_config(model: &str) -> AgentConfig {
    AgentConfig {
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
        name: "worker".to_string(),
        model: Some(ModelId::from(model)),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        exclude_tools: Vec::new(),
        allow_nested_subagents: None,
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_output: OutputCap::default(),
        max_subagent_depth: None,
        memory: None,
        tool_budget: None,
        runner: None, // SUBA-074: the native child, as before
        depth: DepthEnvelope {
            current_depth: 0,
            max_depth: 5,
        },
    }
}

fn base_run_options(cwd: &Path, model: &str) -> RunOptions {
    RunOptions {
        spawn_command: None,
        child_env: std::collections::HashMap::new(),
        turn_budget: None,
        permission_rules: None, // SUBA-073: no policy — the pre-field behaviour
        // SUBA-078: this fixture exercises no reasoning ceiling — `None` is "no ceiling
        // configured, so the bound is off", matching `runner_main.rs`'s own hop-2 default.
        thinking_ceiling: None,
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        enforce_hard_turn_limit: false,
        cwd: cwd.to_path_buf(),
        deadline_at: None,
        timeout_ms: None,
        output_path: None,
        output_mode: OutputMode::Inline,
        reads: None,
        structured_output_schema: None,
        model_override: ModelOverride::Inherit,
        preferred_provider: None,
        available_models: vec![ModelId::from(model)],
        cancel: CancelToken::new(),
        interrupt: CancelToken::new(),
        share: None,
        session_dir: None,
        skills: None,
        runtime_cwd: None,
        include_progress: None,
        agent_scope: None,
        acceptance: Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            vec![],
        )),
        fork_context: ForkContext::fresh(),
        live_events: None,
        parent_session_id: None,
        clarify: None,
        orchestrator_intercom_target: None,
        run_id: None,
        child_index: None,
        steer_inbox_dir: None,
        // SUBA-049: the RETURN half of G90's steer channel. Both paths exist only under a background
        // run directory; a foreground fixture like this one has none. Load-bearing:
        // `build_attempt_spawn_plan` gates both env keys on presence (exec/mod.rs:2227-2250), so
        // `None` keeps the child's env overlay byte-identical to a real foreground child's.
        steer_ack_dir: None,
        steer_capability_path: None,
        control_config: None,
        on_control_event: None,
        artifacts_dir: None,
        model_scope: None,
    }
}

// =================================================================================================
// 1. `SingleResult::process_signal` — published by the LIVE `run_sync` path
// =================================================================================================

/// G104 — `exec::run_sync` must publish the real OS signal name onto the terminal `SingleResult`
/// (pi `execution.ts:1081`), and that field must be what carries a signal-killed child into
/// `resolveSubagentResultStatus`'s `"stopped"` branch rather than the `"failed"` bucket.
///
/// Pre-coverage, `signal.startup.process_signal.clone()` could be replaced with a literal `None` and
/// the whole suite stayed green: nothing anywhere read the field off a REAL run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_signal_killed_child_publishes_its_real_process_signal_and_resolves_stopped() {
    let dir = tempfile::tempdir().expect("real tempdir");

    let child = write_sigkill_child(
        dir.path(),
        "sigkill-child.sh",
        &message_end_line("SIGKILL_TEST: the child spoke before it died"),
    );

    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    // This run names its own child instead of moving `CYRUP_SUBAGENT_BINARY` on a process every
    // other test in this binary shares. `run_sync` is a foreground path, which is what
    // `spawn_command` reaches.
    opts.spawn_command = Some(SpawnCommand {
        binary: child,
        base_args: Vec::new(),
    });
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        cyrup_ext_subagents::exec::run_sync(&agent, "die by signal", &opts),
    )
    .await
    .expect("run_sync must not hang on a child that kills itself");

    assert_eq!(
        result.process_signal.as_deref(),
        Some("SIGKILL"),
        "the terminal SingleResult must carry the OS signal that actually killed the child \
         (pi `execution.ts:1081`); got {result:?}"
    );
    // The four lifecycle verdicts that would EXPLAIN the signal are all false, which is precisely
    // what makes it "unexplained" (`runs/shared/process-signal.ts:5-19`).
    assert!(
        !result.interrupted,
        "no interrupt was requested: {result:?}"
    );
    assert!(!result.timed_out, "no deadline was set: {result:?}");
    assert!(
        !result.stopped,
        "the foreground executor never sets `stopped`: {result:?}"
    );
    assert!(
        !result.detached,
        "the child never asked to detach: {result:?}"
    );
    assert_ne!(
        result.exit_code, 0,
        "a signal death has no numeric exit code and is attributed exit 1 \
         (pi `execution.ts:689`): {result:?}"
    );

    // The whole point of the field: this is the ONLY input that reaches
    // `resolveSubagentResultStatus`'s unexplained-signal branch (`result-intercom.ts:35`).
    assert_eq!(
        resolve_single_result_status(&result),
        SubagentResultStatus::Stopped,
        "an unexplained signal death is a STOP, not a failure (pi `result-intercom.ts:35` sits \
         ABOVE the `success === false` branch): {result:?}"
    );
}

// =================================================================================================
// 2. The SINGLE foreground run's out-of-band payload — the wiring for
//    `resolve_single_result_status`
// =================================================================================================

/// A `DeliveryChannel` that always confirms and records every payload it was handed.
#[derive(Default)]
struct RecordingDeliveryChannel {
    received: StdMutex<Vec<IntercomPayload>>,
}

impl DeliveryChannel for RecordingDeliveryChannel {
    fn send(
        &self,
        payload: IntercomPayload,
    ) -> Pin<Box<dyn Future<Output = Result<bool, String>> + Send + '_>> {
        self.received.lock().expect("lock").push(payload);
        Box::pin(async { Ok(true) })
    }
}

fn write_fixture_persona(cwd: &Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the run-state parity test\n\
             model: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

fn tool_error_text(err: &cyrup_core::ToolError) -> String {
    err.to_string()
}

fn tool_result_text(result: &cyrup_core::ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// G104 — the LIVE single-mode foreground path must resolve its one child through
/// `foregroundResultIntercomStatus` (`subagent-executor.ts:1594-1605`), i.e. the full
/// `resolveSubagentResultStatus` ladder over the REAL `SingleResult`.
///
/// Before the fix this path built a synthetic `chain_graph::StepResult` (`success: exit_code == 0`,
/// `interrupted`) and pushed it through the GROUPED constructor. A `StepResult` has no
/// `process_signal` field at all, so `result-intercom.ts:35` was structurally unreachable for a
/// single run and this same child was delivered as `"1 failed"`.
///
/// The whole chain is real: a real persona discovered off disk, a real `subagent` tool dispatch, a
/// real OS subprocess that dies of a real signal, and a real payload handed to a real
/// `DeliveryChannel`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_mode_out_of_band_delivery_reports_a_signal_killed_child_as_stopped() {
    let work_dir = tempfile::tempdir().expect("real tempdir for the persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate CYRUP_HOME artifacts");
    write_fixture_persona(work_dir.path(), "worker");

    let child = write_sigkill_child(
        work_dir.path(),
        "sigkill-child.sh",
        &message_end_line("SIGKILL_TEST: single-mode child spoke before it died"),
    );

    let delivery = std::sync::Arc::new(RecordingDeliveryChannel::default());
    let extension = SubagentsExtension::with_channels(
        // SUBA-083: asserts out-of-band delivery reports a signal-killed child as stopped, which
        // requires the run to execute and settle in the foreground (pi `config.ts:222-224`).
        // NOTE: the `action='stop'` site at :490 is a management verb and is deliberately
        // left alone — it never reaches the launch-mode decision.
        SubagentExtensionConfig {
            async_by_default: false,
            spawn_command: Some(SpawnCommand {
                binary: child,
                base_args: Vec::new(),
            }),
            roots: Roots::sandboxed(home_dir.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
        delivery.clone(),
        std::sync::Arc::new(NoOpClarifyChannel),
        std::sync::Arc::new(NoTransportSteerChannel),
    );
    let tool = extension.subagent_tool();

    let outcome = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "agent": "worker", "task": "die by signal" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    let received = delivery.received.lock().expect("lock");
    assert_eq!(
        received.len(),
        1,
        "single-mode completion must attempt exactly one out-of-band delivery"
    );
    let payload = &received[0];

    assert_eq!(
        payload.child_statuses,
        vec![SubagentResultStatus::Stopped],
        "the delivered per-child status must come from the FULL ladder over the real SingleResult \
         (pi `foregroundResultIntercomStatus`), which reads `processSignal`; a `StepResult` \
         projection has no such field and reports `failed`: {payload:?}"
    );
    assert_eq!(
        payload.status,
        SubagentResultStatus::Stopped,
        "`resolveGroupedStatus` over one stopped child is `stopped`: {payload:?}"
    );
    assert_eq!(payload.summary, "1 stopped", "{payload:?}");

    // The rendered receipt the model actually sees carries the same tally. A non-zero exit is
    // surfaced as `Err(ToolError)` carrying that receipt text (this crate's documented analogue of
    // pi returning the receipt for a failed run too).
    let text = match &outcome {
        Ok(result) => tool_result_text(result),
        Err(err) => tool_error_text(err),
    };
    assert!(
        text.contains("Children: 1 stopped"),
        "pi's `formatSubagentResultReceipt` child tally must read `1 stopped`, got: {text:?}"
    );
}

// =================================================================================================
// 3. `action: "stop"` against a NESTED run — pi's own sentence, not the not-found text
// =================================================================================================

/// G77 — `subagent-executor.ts:4791,4796`: `resolveSubagentRunId` classifies the selector BEFORE the
/// async store is touched, and a `nested` kind is refused with its own sentence
/// (`"action='stop' supports current-session top-level async runs only."`).
///
/// A nested run is one spawned inside another run's subtree: it publishes a
/// `subagent.nested.started` record into its root's nested route and lives under that root's
/// `nested-subagent-runs` tree, never in this session's own async root. Reporting it as "no
/// stoppable async run" would tell the caller their id was wrong when it was merely out of scope.
#[tokio::test]
async fn stopping_a_nested_run_gets_pis_own_scope_refusal_not_the_not_found_text() {
    let dir = tempfile::tempdir().expect("real tempdir");
    let temp_root = dir.path().join("subagents-temp");
    std::fs::create_dir_all(&temp_root).expect("mkdir temp root");

    // The route carries absolute `event_sink`/`control_inbox` paths, so naming the root here
    // scopes the whole projection to this test's own tree — no `CYRUP_SUBAGENTS_TEMP_ROOT`.
    //
    // Derived FROM the roots the extension is given, rather than assembled alongside them: the
    // whole point of handing the executor a resolved `Roots` is that the tree the test writes into
    // and the tree the executor scans cannot be two different arithmetic expressions that happen
    // to agree today.
    let roots = Roots::sandboxed(&temp_root);
    let events_root = roots.nested_events();
    std::fs::create_dir_all(&events_root).expect("mkdir nested events root");
    // The tool's own nested lookup must scan the SAME tree this route is minted in, or the stop
    // path cannot see the run it is meant to refuse — which is exactly how this test caught the
    // gap when only the route side was scoped.
    std::fs::create_dir_all(&events_root).expect("mkdir events root");
    let route = create_nested_route_in(&events_root, "nestroot0001").expect("mint a nested route");
    let mut child = NestedRunSummary {
        id: "nestedkid001".to_string(),
        parent_run_id: route.root_run_id.clone(),
        parent_step_index: Some(0),
        parent_agent: Some("worker".to_string()),
        depth: 1,
        path: Vec::new(),
        async_dir: Some(dir.path().join("nested-async").display().to_string()),
        pid: None,
        session_id: None,
        session_file: None,
        intercom_target: None,
        owner_intercom_target: None,
        leaf_intercom_target: None,
        owner_state: None,
        control_inbox: None,
        capability_token: None,
        mode: Some("single".to_string()),
        state: "running".to_string(),
        agent: Some("worker".to_string()),
        agents: None,
        current_step: None,
        chain_step_count: None,
        activity_state: None,
        last_activity_at: None,
        current_tool: None,
        current_tool_started_at: None,
        current_path: None,
        turn_count: None,
        tool_count: None,
        total_tokens: None,
        total_cost: None,
        started_at: None,
        ended_at: None,
        last_update: None,
        error: None,
        steps: None,
        children: None,
    };
    child.pid = None;
    write_nested_event_in(
        &events_root,
        &route,
        &NestedEventInput {
            event_type: "subagent.nested.started".to_string(),
            ts: 1,
            parent_run_id: route.root_run_id.clone(),
            parent_step_index: Some(0),
            child,
        },
    )
    .expect("publish the nested-started record");

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: roots.clone(),
            ..SubagentExtensionConfig::default()
        },
        dir.path().to_path_buf(),
    );
    let tool = extension.subagent_tool();

    let nested_err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "stop", "id": "nestedkid001" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("a nested run is not stoppable through this verb");

    // A control: an id that names nothing at all still gets the OTHER text, so this test cannot
    // pass by the two refusals having silently collapsed back into one string.
    let unknown_err = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "action": "stop", "id": "notarunatall" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await
        .expect_err("an unknown id is not stoppable either");

    assert!(
        nested_err
            .to_string()
            .contains("action='stop' supports current-session top-level async runs only."),
        "pi `:4796`'s verbatim nested refusal, got: {nested_err}"
    );
    assert!(
        unknown_err
            .to_string()
            .contains("No stoppable async run found in this session."),
        "pi `:4812`'s verbatim no-target fallback, got: {unknown_err}"
    );
    assert_ne!(
        nested_err.to_string(),
        unknown_err.to_string(),
        "the two refusals are DISTINCT upstream strings and must stay distinct here"
    );
}

// =================================================================================================
// 4. A stop lands together with a timeout — the stop wins
// =================================================================================================

/// A minimal resolved persona for the fixture-driven runner test (mirrors
/// `background_runner_main_integration.rs`'s own `fixture_persona`).
fn fixture_persona(name: &str) -> ResolvedAgentPersona {
    ResolvedAgentPersona {
        acceptance_role: None, // SUBA-082: no declared role, the name decides
        default_acceptance: None,
        name: name.to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        exclude_tools: Vec::new(),
        allow_nested_subagents: None,
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
        runner: None,
    }
}

fn single_step(agent: &str, task: &str) -> SingleStepSpec {
    SingleStepSpec {
        skills: None,
        session_dir: None,
        agent: agent.to_string(),
        task: task.to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    }
}

/// G77 — a stop and a timeout landing together must end the run `Stopped`, never `Failed`.
///
/// Upstream fixes this order in two places and cyrup mirrors both: the inbox drain order
/// (`runs/background/control-channel.ts:653-655` @v0.43.0 — `consumeStopRequest` → `consumeTimeoutRequest` →
/// `consumeInterruptRequest`) and `stopRunner`'s mutual-exclusion guard
/// (`subagent-runner.ts:2955-2986`: `if (stopped || timedOut || interrupted || …) return`). The terminal
/// record must always be the HARDEST, least-resumable verdict — and a timeout is `Failed`, which
/// would lose the fact that a human explicitly stopped this run.
///
/// Both requests are planted through the REAL parent-side primitives before `run()` starts, so this
/// covers both writers as well as the runner's drain order, and the child is a real subprocess with
/// a long sleep so the control watcher is guaranteed to observe both files while a step is live.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_landing_with_a_timeout_ends_the_run_stopped_not_failed() {
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": serde_json::Value::String(r#"{"type":"agent_start"}"#.to_string())},
            {"kind": "sleep_ms", "ms": 6000},
            {"kind": "emit", "line": message_end_line("SHOULD-NOT-REACH")}
        ],
        "exit_code": 0
    });
    let script_path = dir.path().join("stop-vs-timeout-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let async_root = dir.path().join("async");
    let results_dir = dir.path().join("results");
    tokio::fs::create_dir_all(&async_root)
        .await
        .expect("mkdir async root");
    tokio::fs::create_dir_all(&results_dir)
        .await
        .expect("mkdir results dir");

    let run_id = RunId::from_token("stopvstime01");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir)
        .await
        .expect("mkdir run dir");

    let config = RunnerConfig {
        turn_budget: None,
        permission_rules: None,
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        run_id: run_id.clone(),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(single_step(
            "only",
            "sleep a long time",
        ))],
        cwd: dir.path().to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: async_root.clone(),
        results_dir: results_dir.clone(),
        resolved_agents: BTreeMap::from([("only".to_string(), fixture_persona("only"))]),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    };
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config)
        .await
        .expect("write runner config");

    // BOTH verbs, planted through the real parent-side writers, before the runner starts.
    cyrup_ext_subagents::background::control::deliver_timeout_request(
        &run_paths.run_dir,
        "ancestor-timeout",
        None,
    )
    .await
    .expect("plant the timeout request");
    cyrup_ext_subagents::background::control::deliver_stop_request(
        &run_paths.run_dir,
        "stop-action",
        None,
    )
    .await
    .expect("plant the stop request");

    let fixture = crate::support::bins::subagent_fixture();

    let outcome = run_with(
        &cfg_path,
        &run_paths,
        RunnerOverrides {
            spawn_command: Some(SpawnCommand {
                binary: fixture,
                base_args: vec![
                    "--fixture-script".to_string(),
                    script_path.display().to_string(),
                ],
            }),
            ..Default::default()
        },
    )
    .await;
    outcome.expect("run() itself never returns Err");

    let status: RunStatus = serde_json::from_slice(
        &tokio::fs::read(&run_paths.status)
            .await
            .expect("status.json exists"),
    )
    .expect("parse status.json");
    let result: ResultFile = serde_json::from_slice(
        &tokio::fs::read(&run_paths.result)
            .await
            .expect("ResultFile exists"),
    )
    .expect("parse ResultFile");

    assert_eq!(
        status.state,
        RunState::Stopped,
        "a stop outranks a timeout: the terminal record must be the hardest verdict, not `Failed` \
         (pi `control-channel.ts:653-655` drain order + `subagent-runner.ts:2956` guard): {status:?}"
    );
    assert_eq!(result.state, RunState::Stopped);
    assert_eq!(
        status.steps[0].status,
        StepState::Stopped,
        "the swept step must read `stopped`, not the timeout sweep's `failed`: {:?}",
        status.steps[0]
    );
    assert_eq!(
        status.steps[0].error.as_deref(),
        Some(cyrup_ext_subagents::background::control::STOP_MESSAGE),
        "the step carries the STOP message, never a `Subagent timed out…` one"
    );
    // Whichever of the two stop probes wins the race — the loop-top drain (`runner_main`'s
    // `stopped` branch, ahead of the timeout branch) if the watcher tick beats the first spawn, or
    // the mid-flight teardown probe (which checks the stop inbox before the timeout inbox) if the
    // child was already running — the child record is `stopped`. In the first case it is
    // `finish_run`'s synthesized placeholder; in the second it is a real child promoted by
    // `promote_interrupted_results_to_stopped`. Both must agree, and neither may be `interrupted`
    // (which would read as resumable) or `timed_out` (the losing verb).
    assert!(
        !result.results.is_empty(),
        "a terminal stopped run always explains itself with at least one child record: {result:?}"
    );
    assert!(
        result
            .results
            .iter()
            .all(|r| r.stopped && !r.interrupted && !r.timed_out),
        "every child record of a stopped run reads `stopped`, and neither `interrupted` \
         (resumable) nor `timed_out` may survive alongside it: {:?}",
        result.results
    );

    let events = tokio::fs::read_to_string(&run_paths.events)
        .await
        .expect("events.jsonl exists");
    let types: Vec<String> = events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_string))
        .collect();
    assert!(
        types.iter().any(|t| t == "subagent.run.stopped"),
        "events.jsonl must carry `subagent.run.stopped`: {types:?}"
    );
    assert!(
        !types.iter().any(|t| t == "subagent.run.timed_out"),
        "the losing verb must not ALSO write its own terminal lifecycle event: {types:?}"
    );
}

// =================================================================================================
// 5. The acceptance-ledger half of the same divergence
// =================================================================================================

/// G104 — the SECOND field a `chain_graph::StepResult` projection cannot see: the acceptance ledger.
///
/// pi's `foregroundResultIntercomStatus` pins `success: false` whenever
/// `result.acceptance?.status === "rejected"` (`subagent-executor.ts:1597` @v0.43.0) —
/// UNCONDITIONALLY, not gated on the contract being explicit — and `resolveSubagentResultStatus`'s
/// `success === false` branch (`result-intercom.ts:36`) then reports `failed` even though the child
/// process exited `0`.
///
/// That combination is not exotic; it is the DEFAULT for a persona with no declared acceptance
/// policy. The heuristic contract infers `attested` (`acceptance.ts`'s `inferLevel` only ever yields
/// `attested`/`checked`), the foreground gate is not `reportOptional` (upstream gates that on
/// `isAgentContractV1(options.agentContract)`, `execution.ts:1703`), so a child that emits plain
/// prose with no `acceptance-report` fence is REJECTED (`acceptance.ts:1256-1262`) — while the exit
/// code stays `0`, because the post-hoc exit-code correction is itself gated on
/// `result.acceptance.explicit` (`execution.ts:1714`).
///
/// The old `StepResult` projection set `success: exit_code == 0`, which reported this child
/// `completed`. This asserts each link of the chain on one real run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rejected_acceptance_ledger_outranks_a_clean_exit_code_in_the_single_result_status() {
    let dir = tempfile::tempdir().expect("real tempdir");

    let script = serde_json::json!({
        "steps": [
            {"kind": "emit", "line": message_end_line("I did the thing. No acceptance report here.")}
        ],
        "exit_code": 0
    });
    let script_path = dir.path().join("plain-prose-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let agent = base_agent_config("fixture-model");
    let mut opts = base_run_options(dir.path(), "fixture-model");
    opts.spawn_command = Some(SpawnCommand {
        binary: crate::support::bins::subagent_fixture(),
        base_args: vec![
            "--fixture-script".to_string(),
            script_path.display().to_string(),
        ],
    });
    // The point of the test: NO explicit contract, so the heuristic one applies exactly as it does
    // for any ordinary persona that declares no acceptance policy.
    opts.acceptance = None;

    let result = tokio::time::timeout(
        Duration::from_secs(20),
        cyrup_ext_subagents::exec::run_sync(&agent, "do the trivial thing", &opts),
    )
    .await
    .expect("run_sync must not hang against a fast, well-behaved fixture child");

    assert_eq!(
        result.exit_code, 0,
        "the child process itself succeeded — an inferred (non-explicit) contract must not flip the \
         exit code (pi `execution.ts:1714` gates that correction on `result.acceptance.explicit`): {result:?}"
    );
    let ledger = result
        .acceptance
        .as_ref()
        .expect("a non-detached, non-timed-out run always carries a ledger");
    assert_eq!(
        ledger.status,
        AcceptanceStatus::Rejected,
        "a missing acceptance-report rejects when the caller is not `reportOptional`: {ledger:?}"
    );
    // The contract in play is the INFERRED one — `opts.acceptance` was left `None` above — which is
    // why the exit code survived the post-hoc correction while the ledger still rejected.
    assert_eq!(
        resolve_single_result_status(&result),
        SubagentResultStatus::Failed,
        "pi `subagent-executor.ts:1597` pins `success: false` off the REJECTED ledger, and \
         `result-intercom.ts:36` reports `failed` — a clean exit code does not override it. A \
         `chain_graph::StepResult` projection carries no ledger and reported `completed`: {result:?}"
    );
}
