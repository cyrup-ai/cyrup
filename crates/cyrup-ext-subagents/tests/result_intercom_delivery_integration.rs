//! Regression coverage for the `result-intercom` divergence pass (pi
//! `pi-subagents/src/intercom/result-intercom.ts`): a REAL foreground `subagent` tool call, wired
//! to a confirming [`DeliveryChannel`], must
//!
//!   1. attempt out-of-band delivery for the SINGLE-mode foreground path too (pi's
//!      `runSinglePath`, `subagent-executor.ts:3515-3873`, gated on `!detached && !interrupted`)
//!      — not parallel-mode alone;
//!   2. render the delivered receipt via pi's `formatSubagentResultReceipt`
//!      (`result-intercom.ts:376-421`) — mode label + `"Run: …"` + `"Children: …"` + the exact
//!      closing line — never a bespoke `"N/M succeeded"` string; and
//!   3. cite the run's OWN real, delivered-payload run id in that receipt — never a second, fresh,
//!      disconnected id minted only for the message.
//!
//! No mocking of the child process itself (this crate's standing convention): the run spawns the
//! REAL `cyrup-subagent-fixture` binary as a genuine OS subprocess, discovers a REAL persona `.md`
//! through the REAL discovery pipeline, and drives the REAL `subagent` tool's `execute` dispatch
//! (`SubagentTool::route_single`/`route_parallel_mode`). Only the out-of-band intercom transport
//! itself (a trait with no real implementation shipped in this workspace yet, per
//! `tui/intercom.rs`'s own module doc) is a test double — a [`DeliveryChannel`] that always
//! confirms and records exactly the payload it received.
//!
//! Gated on the `test-fixtures` Cargo feature, matching every other fixture-based integration test
//! in this crate.

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex as StdMutex;

use tokio::sync::Mutex;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::tui::intercom::{
    DeliveryChannel, IntercomPayload, NoOpClarifyChannel, NoTransportSteerChannel,
};

/// Serializes every test that mutates process-global env, mirroring every other fixture-based
/// integration test in this crate.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

fn write_fixture_persona(cwd: &std::path::Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the result-intercom test\n\
             model: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

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

/// A [`DeliveryChannel`] that always confirms delivery and records every payload it received (so
/// the test can assert the receipt cites the SAME run id the payload actually carried).
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

fn tool_result_text(result: &cyrup_core::ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_mode_delivers_out_of_band_and_renders_pis_receipt_with_the_real_run_id() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir for the fixture persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate CYRUP_HOME artifacts");
    write_fixture_persona(work_dir.path(), "worker");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("RESULT_INTERCOM_TEST_OUTPUT: real child ran") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation for the duration of this one test, matching
    // every sibling fixture-based integration test in this crate.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
        std::env::set_var("CYRUP_HOME", home_dir.path());
    }

    let delivery = std::sync::Arc::new(RecordingDeliveryChannel::default());
    let extension = SubagentsExtension::with_channels(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
        delivery.clone(),
        std::sync::Arc::new(NoOpClarifyChannel),
        std::sync::Arc::new(NoTransportSteerChannel),
    );
    let tool = extension.subagent_tool();

    let result = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "agent": "worker", "task": "do the trivial thing" }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        std::env::remove_var("CYRUP_HOME");
    }

    let result = result.expect("a confirmed out-of-band delivery must not surface as a tool error");
    let text = tool_result_text(&result);

    // (1) Delivery was actually ATTEMPTED for SINGLE mode (pre-fix: `route_single` never called
    // `deliver_group_out_of_band` at all, so this channel would never be invoked).
    let received = delivery.received.lock().expect("lock");
    assert_eq!(
        received.len(),
        1,
        "SINGLE-mode foreground completion must attempt exactly one out-of-band delivery; got: {}",
        received.len()
    );
    let payload = &received[0];

    // (2) The rendered receipt is pi's `formatSubagentResultReceipt` shape — mode label + "Run: …"
    // + "Children: …" + the exact closing line — never the old bespoke "N/M succeeded" text.
    assert!(
        text.starts_with("Delivered single subagent result via intercom."),
        "must render pi's formatSubagentResultReceipt mode-label line, got: {text:?}"
    );
    // G104 — the tally reads `1 failed`, and that is upstream's own verdict for this child, not a
    // regression. pi resolves a SINGLE run's child with `foregroundResultIntercomStatus`
    // (`subagent-executor.ts:1594-1605` @v0.43.0), whose `:1597` pins `success: false` whenever
    // `result.acceptance?.status === "rejected"` — unconditionally, not gated on the contract being
    // explicit. This fixture persona declares no acceptance policy, so the heuristic contract infers
    // `attested`; the child emits plain prose with no `acceptance-report` fence; the foreground gate
    // is not `reportOptional` (upstream gates that on `isAgentContractV1(options.agentContract)`,
    // `execution.ts:1703`, and no agent contract is in play here); so `evaluateAcceptance` rejects
    // for a missing report (`acceptance.ts:1256-1262`) while leaving the exit code at 0 (the
    // post-hoc exit-code correction is itself gated on `result.acceptance.explicit`,
    // `execution.ts:1714`).
    //
    // BEFORE this assertion read `Children: 1 completed`. That expectation could only be produced by
    // the path this change deleted: the payload was built by projecting the real `SingleResult`
    // through a synthetic `chain_graph::StepResult` whose `success` was literally `exit_code == 0`,
    // a shape that carries no acceptance ledger and no `process_signal` and therefore could never
    // reach `result-intercom.ts:32` or `:35` at all.
    // AFTER: the real `SingleResult` is resolved by the real ladder, so the ledger is visible.
    assert!(
        text.contains("Children: 1 failed"),
        "must render pi's countStatuses/formatStatusCounts child tally, got: {text:?}"
    );
    // Stated directly on the payload as well, so the reason above is asserted rather than merely
    // commented: the child's REJECTED acceptance ledger is what produced the verdict.
    assert_eq!(
        payload.child_statuses,
        vec![cyrup_ext_subagents::tui::intercom::SubagentResultStatus::Failed],
        "the per-child status must come from the full ladder over the real SingleResult: {payload:?}"
    );
    assert!(
        text.ends_with("Full grouped output was sent over intercom."),
        "must render pi's exact closing line, got: {text:?}"
    );

    // (3) The receipt cites the SAME run id the delivered payload actually carried — never a
    // second, fresh, disconnected id minted only for the message (pre-fix: `route_parallel_mode`
    // minted `RunId::new()` for the payload; SINGLE mode had no payload/run-id concept at all).
    let expected_run_line = format!("Run: {}", payload.run_id.as_str());
    assert!(
        text.contains(&expected_run_line),
        "receipt must cite the delivered payload's own real run id ({expected_run_line}), got: {text:?}"
    );
}

/// PARALLEL mode's own pre-existing delivery path (`route_parallel_mode`): before this fix it
/// minted a `RunId::new()` just for the out-of-band payload — an id that corresponds to no real
/// run and appeared nowhere else observable. Cross-check the delivered payload's run id against
/// an INDEPENDENT on-disk observation of the SAME run's real id (`{chain_dir}`'s own directory
/// name, `artifacts::chain_runs_dir(cwd).join(run_id)`) — proving it is the run's genuine id, not
/// merely "some id that happens to render".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_mode_receipt_cites_the_same_run_id_the_chain_dir_was_created_under() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir for the fixture persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate CYRUP_HOME artifacts");
    write_fixture_persona(work_dir.path(), "worker");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("RESULT_INTERCOM_PARALLEL_OUTPUT: real child ran") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
        std::env::set_var("CYRUP_HOME", home_dir.path());
    }

    let delivery = std::sync::Arc::new(RecordingDeliveryChannel::default());
    let extension = SubagentsExtension::with_channels(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
        delivery.clone(),
        std::sync::Arc::new(NoOpClarifyChannel),
        std::sync::Arc::new(NoTransportSteerChannel),
    );
    let tool = extension.subagent_tool();

    let result = tool
        .execute(
            ToolCallId::from("t"),
            serde_json::json!({ "tasks": [{ "agent": "worker", "task": "do the trivial thing" }] }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    let result = result.expect("a confirmed out-of-band delivery must not surface as a tool error");
    let text = tool_result_text(&result);

    let received = delivery.received.lock().expect("lock");
    assert_eq!(received.len(), 1, "PARALLEL-mode foreground completion must attempt exactly one delivery");
    let payload = &received[0];

    assert!(
        text.starts_with("Delivered parallel subagent results via intercom."),
        "must render pi's formatSubagentResultReceipt mode-label line, got: {text:?}"
    );
    let expected_run_line = format!("Run: {}", payload.run_id.as_str());
    assert!(
        text.contains(&expected_run_line),
        "receipt must cite the delivered payload's own real run id ({expected_run_line}), got: {text:?}"
    );

    // Independent cross-check: this run's `{chain_dir}` scratch directory (created BEFORE the
    // out-of-band payload is built, under the SAME id per `run_or_background_graph`) must exist
    // under exactly the payload's own run id — proving that id is the run's real, genuine one.
    // Computed BEFORE `CYRUP_HOME` is cleared below (`chain_runs_dir` itself reads that env var).
    let chain_dir = cyrup_ext_subagents::artifacts::chain_runs_dir(work_dir.path())
        .join(payload.run_id.as_str());
    let chain_dir_exists = chain_dir.is_dir();

    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        std::env::remove_var("CYRUP_HOME");
    }

    assert!(
        chain_dir_exists,
        "the payload's run id must be the SAME id this run's real `{{chain_dir}}` was created \
         under ({}), proving it is not a disconnected, freshly-minted id",
        chain_dir.display()
    );
}
