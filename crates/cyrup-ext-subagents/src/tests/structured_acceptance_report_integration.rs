//! SUBA-105 — `acceptance.report` end to end through `run_sync`, the chokepoint every launch path
//! (single, chain/parallel step, background hop-2) reaches, with a REAL child process: a shell
//! script standing in for the child's `structured_output` tool, which writes exactly the two files
//! the real tool writes (`prompt_runtime::StructuredOutputTool`) to the paths the parent hands it
//! in its environment.
//!
//! Upstream: `resolveAcceptanceReportMode` / `validateAcceptanceReportMode`
//! (`runs/shared/acceptance.ts:192-197,423-430` @v0.68.0), the structured runtime's
//! `acceptanceReportPath` (`structured-output.ts:132-148`), `readStructuredOutputAcceptanceReport`
//! (`:185-196`), and `evaluateAcceptance`'s `report`/`reportError` inputs (`acceptance.ts:1428-1464`);
//! the cases mirror `test/integration/single-execution.part-2.test.ts:1945-2060` @v0.68.0.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::exec::acceptance::{AcceptanceStatus, lower_acceptance_input};
use crate::exec::structured::MISSING_STRUCTURED_ACCEPTANCE_REPORT_ERROR;
use crate::exec::testsupport::{base_opts, sample_agent_config};

/// The criterion every fixture gates on (upstream's own `proof`).
fn proof_report() -> serde_json::Value {
    json!({
        "criteriaSatisfied": [{ "id": "proof", "status": "satisfied", "evidence": "ok is true" }],
        "residualRisks": ["none"],
    })
}

/// The shell child. It records the report mode it was handed and the task text it received (an
/// `@<file>` task argument is expanded), writes the structured value to the capture path, writes
/// `report` to the acceptance-report path ONLY when the parent handed one, and ends with `prose`
/// as its final assistant text.
fn child(dir: &Path, report: Option<&serde_json::Value>, prose: &str) -> PathBuf {
    let script = dir.join("structured-child.sh");
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let line = |json: &serde_json::Value| format!("printf '%s\\n' {}\n", quote(&json.to_string()));
    let mut body = String::from("#!/bin/sh\n");
    body.push_str(&format!(
        "printf '%s' \"$CYRUP_SUBAGENT_STRUCTURED_OUTPUT_ACCEPTANCE_REPORT_MODE\" > {}\n",
        quote(&dir.join("mode.txt").display().to_string())
    ));
    body.push_str(&format!(
        "for a in \"$@\"; do case \"$a\" in @*) f=\"${{a#@}}\"; [ -f \"$f\" ] && cat \"$f\" >> {argv};; \
         *) printf '%s\\n' \"$a\" >> {argv};; esac; done\n",
        argv = quote(&dir.join("argv.txt").display().to_string())
    ));
    body.push_str(
        "[ -n \"$CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE\" ] && printf '%s' '{\"ok\":true}' > \"$CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE\"\n",
    );
    if let Some(report) = report {
        body.push_str(&format!(
            "[ -n \"$CYRUP_SUBAGENT_STRUCTURED_OUTPUT_ACCEPTANCE_REPORT\" ] && printf '%s' {} > \"$CYRUP_SUBAGENT_STRUCTURED_OUTPUT_ACCEPTANCE_REPORT\"\n",
            quote(&report.to_string())
        ));
    }
    body.push_str(&line(&json!({ "type": "agent_start" })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": { "role": "assistant", "content": [{ "type": "text", "text": prose }] },
    })));
    body.push_str(&line(&json!({ "type": "agent_settled" })));
    body.push_str("exit 0\n");
    std::fs::write(&script, body).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

async fn run(
    dir: &Path,
    policy: serde_json::Value,
    schema: Option<serde_json::Value>,
    script: PathBuf,
) -> crate::exec::run_result::SingleResult {
    let agent = sample_agent_config("m1", &[]);
    let mut opts = base_opts(dir, &["m1"]);
    opts.acceptance = lower_acceptance_input(&policy).expect("a valid policy");
    opts.structured_output_schema = schema;
    opts.spawn_command = Some(crate::spawn::SpawnCommand {
        binary: script,
        base_args: Vec::new(),
    });
    crate::exec::run_sync(&agent, "Return structured data", &opts).await
}

fn schema() -> serde_json::Value {
    json!({ "type": "object", "required": ["ok"], "properties": { "ok": { "type": "boolean" } } })
}

fn read(dir: &Path, name: &str) -> String {
    std::fs::read_to_string(dir.join(name)).unwrap_or_default()
}

/// pi "rejects workflow outputSchema children that omit checked acceptance sidecars"
/// (`single-execution.part-2.test.ts:1995-2030` @v0.68.0): with `report: "on"` the child is
/// handed the `required` channel, and a `structured_output` value that arrives with no report is
/// rejected with `MISSING_STRUCTURED_ACCEPTANCE_REPORT_ERROR` — the explicit policy then fails the
/// run — even though the structured value itself is kept.
#[tokio::test]
async fn a_required_structured_acceptance_report_that_never_arrives_rejects_the_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(dir.path(), None, "done");
    let result = run(
        dir.path(),
        json!({ "level": "checked", "report": "on", "criteria": [{ "id": "proof", "must": "Return required proof" }] }),
        Some(schema()),
        script,
    )
    .await;

    assert_eq!(read(dir.path(), "mode.txt"), "required");
    assert_eq!(
        result.structured_output,
        Some(json!({ "ok": true })),
        "{result:?}"
    );
    let ledger = result.acceptance.as_ref().expect("a ledger");
    assert_eq!(ledger.status, AcceptanceStatus::Rejected, "{ledger:?}");
    assert_eq!(
        ledger.detail.as_deref(),
        Some(MISSING_STRUCTURED_ACCEPTANCE_REPORT_ERROR)
    );
    assert_eq!(result.exit_code, 1, "{result:?}");
}

/// pi "accepts workflow outputSchema children with checked acceptance sidecars"
/// (`single-execution.part-2.test.ts:1945-1993`): the report handed in the `structured_output`
/// call satisfies the criterion although the child's prose carries no report at all — the
/// structured report is the report. The default channel (`optional`, no `report` key) is used, and
/// the child is told to put the report in its `structured_output` call, not in a fenced block.
#[tokio::test]
async fn a_structured_acceptance_report_satisfies_the_gate_without_any_prose_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(dir.path(), Some(&proof_report()), "done");
    let result = run(
        dir.path(),
        json!({ "level": "checked", "criteria": [{ "id": "proof", "must": "Return required proof" }] }),
        Some(schema()),
        script,
    )
    .await;

    assert_eq!(read(dir.path(), "mode.txt"), "optional");
    let task = read(dir.path(), "argv.txt");
    assert!(
        task.contains(
            "Include an `acceptanceReport` object in your final `structured_output` tool call in this shape:"
        ),
        "{task}"
    );
    assert!(!task.contains("```acceptance-report"), "{task}");
    let ledger = result.acceptance.as_ref().expect("a ledger");
    assert_eq!(ledger.status, AcceptanceStatus::Checked, "{ledger:?}");
    assert_eq!(result.exit_code, 0, "{result:?}");
}

/// pi "uses fenced acceptance reports when outputSchema acceptance report capture is off"
/// (`single-execution.part-2.test.ts:2032-2060`): `report: "off"` hands the child no channel —
/// even a child that would write one cannot — so the fenced prompt is sent and the fenced prose
/// report is what the gate reads.
#[tokio::test]
async fn report_off_keeps_the_fenced_prompt_and_the_prose_report() {
    let dir = tempfile::tempdir().expect("tempdir");
    let prose = format!("done\n```acceptance-report\n{}\n```", proof_report());
    let script = child(
        dir.path(),
        Some(&json!({ "criteriaSatisfied": [] })),
        &prose,
    );
    let result = run(
        dir.path(),
        json!({ "level": "checked", "report": "off", "criteria": [{ "id": "proof", "must": "Return required proof" }] }),
        Some(schema()),
        script,
    )
    .await;

    assert_eq!(
        read(dir.path(), "mode.txt"),
        "",
        "no report channel is handed over"
    );
    let task = read(dir.path(), "argv.txt");
    assert!(
        task.contains("Finish with a fenced JSON block tagged `acceptance-report` in this shape:"),
        "{task}"
    );
    let ledger = result.acceptance.as_ref().expect("a ledger");
    assert_eq!(ledger.status, AcceptanceStatus::Checked, "{ledger:?}");
    assert_eq!(result.exit_code, 0, "{result:?}");
}

/// pi `validateAcceptanceReportMode` (`acceptance.ts:423-430`): `report` without an
/// `outputSchema` is refused before the child is spawned — the script never runs.
#[tokio::test]
async fn a_declared_report_without_an_output_schema_is_refused_before_spawning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(dir.path(), None, "done");
    let result = run(
        dir.path(),
        json!({ "level": "checked", "report": "on" }),
        None,
        script,
    )
    .await;

    assert_eq!(result.exit_code, 1, "{result:?}");
    assert_eq!(
        result.error.as_deref(),
        Some("acceptance.report requires outputSchema.")
    );
    assert!(
        !dir.path().join("mode.txt").exists(),
        "the child must not have been spawned"
    );
}

/// A policy whose ONLY key is `report` is still a policy (`explicitAcceptanceRequestsPolicy`,
/// `acceptance.ts:238-240` @v0.68.0): its level is inferred, and its `required` channel reaches the
/// child — it must not collapse to "no acceptance policy" (which would be `optional`).
#[tokio::test]
async fn a_report_only_policy_still_arms_the_required_channel() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(dir.path(), None, "done");
    let result = run(
        dir.path(),
        json!({ "report": "on" }),
        Some(schema()),
        script,
    )
    .await;

    assert_eq!(read(dir.path(), "mode.txt"), "required");
    let ledger = result.acceptance.as_ref().expect("a ledger");
    assert_eq!(
        ledger.detail.as_deref(),
        Some(MISSING_STRUCTURED_ACCEPTANCE_REPORT_ERROR),
        "{ledger:?}"
    );
}
