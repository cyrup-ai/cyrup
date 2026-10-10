//! SUBA-126 / SUBA-127 / SUBA-132 — how a child's structured output and tool-budget block reach
//! its result (and SUBA-135, how a vanished cwd refuses the run), end to end through `run_sync` (the chokepoint every launch path reaches) with a REAL
//! child process: a shell script that prints the NDJSON a real child prints and writes the capture
//! file the real `structured_output` tool writes.
//!
//! Upstream @v0.71.0: `runs/foreground/execution.ts:1144-1156` (tool-budget block), `:1449-1471`
//! (structured read), `:1515` (structured-only output); `runs/background/subagent-runner.ts:1245-1270`
//! and `:1373-1374`; `runs/shared/structured-output.ts:61-80` (rejection summary).

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::exec::structured::STRUCTURED_OUTPUT_MISSING_ERROR;
use crate::exec::testsupport::{base_opts, sample_agent_config};

/// One step of the shell child.
enum Step {
    /// Print one NDJSON line on stdout.
    Line(serde_json::Value),
    /// Write `value` to the structured-output capture path the parent handed the child.
    Capture(serde_json::Value),
}

fn child(dir: &Path, steps: &[Step], exit_code: i32) -> PathBuf {
    let script = dir.join("child.sh");
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let mut body = String::from("#!/bin/sh\n");
    for step in steps {
        match step {
            Step::Line(line) => {
                body.push_str(&format!("printf '%s\\n' {}\n", quote(&line.to_string())));
            }
            Step::Capture(value) => body.push_str(&format!(
                "printf '%s' {} > \"$CYRUP_SUBAGENT_STRUCTURED_OUTPUT_CAPTURE\"\n",
                quote(&value.to_string())
            )),
        }
    }
    body.push_str(&format!("exit {exit_code}\n"));
    std::fs::write(&script, body).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

fn tool_start(id: &str, name: &str) -> Step {
    Step::Line(
        json!({ "type": "tool_execution_start", "toolCallId": id, "toolName": name, "args": {} }),
    )
}

fn tool_end(id: &str, name: &str, text: &str, is_error: bool) -> Step {
    Step::Line(json!({
        "type": "tool_execution_end",
        "toolCallId": id,
        "toolName": name,
        "result": { "content": [{ "type": "text", "text": text }] },
        "isError": is_error,
    }))
}

fn assistant(text: &str) -> Step {
    Step::Line(json!({
        "type": "message_end",
        "message": { "role": "assistant", "content": [{ "type": "text", "text": text }] },
    }))
}

fn settled() -> Step {
    Step::Line(json!({ "type": "agent_settled" }))
}

fn schema() -> serde_json::Value {
    json!({ "type": "object", "required": ["ok"], "properties": { "ok": { "type": "boolean" } } })
}

async fn run(
    dir: &Path,
    script: PathBuf,
    configure: impl FnOnce(&mut crate::exec::AgentConfig, &mut crate::exec::RunOptions),
) -> crate::exec::run_result::SingleResult {
    let mut agent = sample_agent_config("m1", &[]);
    let mut opts = base_opts(dir, &["m1"]);
    opts.spawn_command = Some(crate::spawn::SpawnCommand {
        binary: script,
        base_args: Vec::new(),
    });
    configure(&mut agent, &mut opts);
    crate::exec::run_sync(&agent, "Return structured data", &opts).await
}

/// SUBA-126 — pi `if (!fullOutput.trim() && result.structuredOutput !== undefined) fullOutput =
/// JSON.stringify(result.structuredOutput, null, 2)`: a child that answers ONLY through
/// `structured_output` saves the pretty JSON to its `output` file and delivers it, instead of an
/// empty file and an empty answer.
#[tokio::test]
async fn a_structured_only_answer_is_saved_and_delivered_as_pretty_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(
        dir.path(),
        &[
            tool_start("so-1", "structured_output"),
            Step::Capture(json!({ "ok": true })),
            tool_end(
                "so-1",
                "structured_output",
                "Structured output captured.",
                false,
            ),
            assistant(""),
            settled(),
        ],
        0,
    );
    let out = dir.path().join("out.json");
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
        opts.output_path = Some(out.clone());
    })
    .await;

    let pretty = serde_json::to_string_pretty(&json!({ "ok": true })).expect("pretty");
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(result.structured_output, Some(json!({ "ok": true })));
    assert_eq!(
        std::fs::read_to_string(&out).expect("the output file is written"),
        pretty
    );
    assert!(
        result
            .final_output
            .as_deref()
            .is_some_and(|text| text.starts_with(&pretty)),
        "{:?}",
        result.final_output
    );
    assert_eq!(result.saved_output_path, Some(out.display().to_string()));
}

/// SUBA-197 Verify — pi `8983754b` / #2672 (`execution.ts:1582` @ad11b7ab): a child that calls
/// `structured_output({ok:true})` and then says "Done." writes the pretty JSON to its bound output
/// file, not the prose. RED at HEAD: the file held `Done.` (the structured value replaced the prose
/// only when the prose was blank, SUBA-126). The reply follows upstream's post-save assignment
/// (`fullOutput = stripAcceptanceReport(resolvedOutput.fullOutput)`, `:1583`), which is the saved
/// JSON; upstream's own tests for the change (`single-execution.part-2`, `async-execution.part-2`
/// / `part-3`) assert only the file, so the reply assertion here is pinned to that code line.
#[tokio::test]
async fn a_bound_output_file_receives_the_structured_result_not_the_closing_prose() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(
        dir.path(),
        &[
            tool_start("so-1", "structured_output"),
            Step::Capture(json!({ "ok": true })),
            tool_end(
                "so-1",
                "structured_output",
                "Structured output captured.",
                false,
            ),
            assistant("Done."),
            settled(),
        ],
        0,
    );
    let out = dir.path().join("r.json");
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
        opts.output_path = Some(out.clone());
    })
    .await;

    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(
        std::fs::read_to_string(&out).expect("the output file is written"),
        "{\n  \"ok\": true\n}"
    );
    let reply = result.final_output.as_deref().unwrap_or_default();
    assert!(reply.starts_with("{\n  \"ok\": true\n}"), "{reply}");
    assert!(!reply.contains("Done."), "{reply}");
}

/// SUBA-197 — with no bound output file nothing is saved, so the reply keeps the child's prose
/// (upstream's `structuredText ?? fullOutput` is only the SAVE input). Passes at HEAD: the
/// NON-REGRESSION GUARD for the `output_path` half of the gate — dropping it turns this red.
#[tokio::test]
async fn without_a_bound_output_file_the_reply_keeps_the_childs_prose() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(
        dir.path(),
        &[
            tool_start("so-1", "structured_output"),
            Step::Capture(json!({ "ok": true })),
            tool_end(
                "so-1",
                "structured_output",
                "Structured output captured.",
                false,
            ),
            assistant("Done."),
            settled(),
        ],
        0,
    );
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
    })
    .await;

    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(result.final_output.as_deref(), Some("Done."));
    assert_eq!(result.structured_output, Some(json!({ "ok": true })));
}

/// SUBA-126 — upstream measures "blank" after `stripAcceptanceReport` (`execution.ts:1514-1515`,
/// `subagent-runner.ts:1373-1374`): prose that is nothing but an acceptance-report block still
/// saves and delivers the structured value, not the machine report.
#[tokio::test]
async fn prose_that_is_only_an_acceptance_report_still_delivers_the_structured_value() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(
        dir.path(),
        &[
            tool_start("so-1", "structured_output"),
            Step::Capture(json!({ "ok": true })),
            tool_end(
                "so-1",
                "structured_output",
                "Structured output captured.",
                false,
            ),
            assistant("```acceptance-report\n{\"criteria\": []}\n```"),
            settled(),
        ],
        0,
    );
    let out = dir.path().join("out.json");
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
        opts.output_path = Some(out.clone());
    })
    .await;

    let pretty = serde_json::to_string_pretty(&json!({ "ok": true })).expect("pretty");
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(
        std::fs::read_to_string(&out).expect("the output file is written"),
        pretty
    );
    assert!(
        result
            .final_output
            .as_deref()
            .is_some_and(|text| text.starts_with(&pretty)),
        "{:?}",
        result.final_output
    );
}

/// SUBA-127 — a child whose `structured_output` call was REJECTED is failed with the bounded,
/// redacted summary of that rejection (pi `formatStructuredOutputRejectionError`), never with the
/// missing-call error that tells it the tool was not called. The echoed submission is dropped.
#[tokio::test]
async fn a_rejected_structured_output_call_fails_with_the_rejection_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(
        dir.path(),
        &[
            tool_start("so-1", "structured_output"),
            tool_end(
                "so-1",
                "structured_output",
                "\u{1b}[31mStructured output validation failed: ok: \"yes\" is not of type \"boolean\"\u{1b}[0m\nsubmitted value: {\"ok\":\"yes\"}\n    at validate (runtime.js:1:1)",
                true,
            ),
            assistant("I could not produce the structured result."),
            settled(),
        ],
        0,
    );
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
    })
    .await;

    assert_eq!(result.exit_code, 1, "{result:?}");
    assert_eq!(result.structured_output, None);
    assert_eq!(
        result.error.as_deref(),
        Some("Structured output validation failed: ok: \"yes\" is not of type \"boolean\"")
    );
}

/// SUBA-127 — a child that never invoked the tool still gets the missing-call error.
#[tokio::test]
async fn a_child_that_never_invokes_structured_output_gets_the_missing_call_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(dir.path(), &[assistant("here is prose"), settled()], 0);
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
    })
    .await;

    assert_eq!(result.exit_code, 1, "{result:?}");
    assert_eq!(
        result.error.as_deref(),
        Some(STRUCTURED_OUTPUT_MISSING_ERROR)
    );
}

/// SUBA-127 — a valid value produced before a later failure is kept as evidence on the failed
/// result, and the run's own failure is not re-labeled by the structured check.
#[tokio::test]
async fn a_valid_structured_value_survives_a_later_failure() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = child(
        dir.path(),
        &[
            tool_start("so-1", "structured_output"),
            Step::Capture(json!({ "ok": true })),
            tool_end(
                "so-1",
                "structured_output",
                "Structured output captured.",
                false,
            ),
            assistant("partial"),
        ],
        3,
    );
    let result = run(dir.path(), script, |_, opts| {
        opts.structured_output_schema = Some(schema());
    })
    .await;

    assert_ne!(result.exit_code, 0, "{result:?}");
    assert_eq!(result.structured_output, Some(json!({ "ok": true })));
    assert!(
        !result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains(STRUCTURED_OUTPUT_MISSING_ERROR),
        "{:?}",
        result.error
    );
}

fn budget(hard: u32) -> crate::discovery::types::ResolvedToolBudget {
    crate::exec::tool_budget::decode_tool_budget_env(
        Some(&format!("{{\"hard\": {hard}}}")),
        crate::exec::tool_budget::HardMinimum::One,
    )
    .expect("valid")
    .expect("some")
}

fn blocked_child(dir: &Path) -> PathBuf {
    let message = crate::exec::tool_budget::tool_budget_blocked_message(&budget(1), "read", 2);
    child(
        dir,
        &[
            tool_start("c1", "read"),
            tool_end("c1", "read", "file contents", false),
            tool_start("c2", "read"),
            tool_end("c2", "read", &message, true),
            assistant("Finalizing from what I have."),
            settled(),
        ],
        0,
    )
}

/// SUBA-132 — pi `result.toolBudgetBlocked`: the child's own budget refused a call, so the result
/// carries the block and a workflow settles the child `budget_exhausted`.
#[tokio::test]
async fn a_tool_budget_block_reaches_the_result_and_settles_budget_exhausted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = blocked_child(dir.path());
    let result = run(dir.path(), script, |agent, _| {
        agent.tool_budget = Some(budget(1));
    })
    .await;

    assert!(result.tool_budget_blocked, "{result:?}");
    assert_eq!(
        crate::workflows::workflow_terminal_outcome_for_result(
            crate::workflows::WorkflowBudgetSignals::from_single_result(&result)
        ),
        Some(crate::workflows::WorkflowTerminalOutcome::Partial {
            reason: crate::workflows::WorkflowTerminalOutcomeReason::BudgetExhausted,
        })
    );
}

/// SUBA-132 — the same text from a child that runs under no budget (or another budget) is ordinary
/// tool output, not a block.
#[tokio::test]
async fn the_blocked_message_counts_only_against_the_childs_own_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = run(dir.path(), blocked_child(dir.path()), |_, _| {}).await;
    assert!(!result.tool_budget_blocked, "{result:?}");

    let other = tempfile::tempdir().expect("tempdir");
    let result = run(other.path(), blocked_child(other.path()), |agent, _| {
        agent.tool_budget = Some(budget(5));
    })
    .await;
    assert!(!result.tool_budget_blocked, "{result:?}");
}

/// SUBA-135 — pi `preflightLaunchCwd` at the head of the run (`execution.ts:1604`; the runner's
/// per-step `subagent-runner.ts:1061`): a run whose cwd vanished is refused by name, and nothing is
/// spawned — the child script would have written `ran` had it been.
#[tokio::test]
async fn a_run_into_a_missing_cwd_is_refused_by_name_before_spawning() {
    let dir = tempfile::tempdir().expect("tempdir");
    let marker = dir.path().join("ran");
    let script = dir.path().join("marker.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\nexit 0\n", marker.display()),
    )
    .expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let missing = dir.path().join("gone");
    let result = run(dir.path(), script, |_, opts| {
        opts.cwd = missing.clone();
    })
    .await;

    assert_eq!(result.exit_code, 1, "{result:?}");
    assert_eq!(
        result.error,
        Some(format!(
            "Subagent launch aborted: cwd does not exist: {}",
            missing.display()
        ))
    );
    assert!(!marker.exists(), "no child was spawned");
}

/// SUBA-134 — pi `deriveChildSessionName` threaded through the launch (`child-launch.ts:251`,
/// `execution.ts` `sessionName`): the REAL spawned child receives the derived name in its env —
/// this child answers with it — and the result carries the same name as `sessionName`.
#[tokio::test]
async fn the_derived_child_session_name_reaches_the_child_env_and_the_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    let script = dir.path().join("child.sh");
    std::fs::write(
        &script,
        concat!(
            "#!/bin/sh\n",
            "printf '{\"type\":\"message_end\",\"message\":{\"role\":\"assistant\",",
            "\"content\":[{\"type\":\"text\",\"text\":\"%s\"}]}}\\n' \"$CYRUP_SUBAGENT_SESSION_NAME\"\n",
            "printf '{\"type\":\"agent_settled\"}\\n'\n",
            "exit 0\n",
        ),
    )
    .expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let result = run(dir.path(), script, |_, _| {}).await;

    let expected = "worker: Return structured data";
    assert_eq!(result.exit_code, 0, "{result:?}");
    assert_eq!(result.session_name.as_deref(), Some(expected));
    assert_eq!(
        result.final_output.as_deref().map(str::trim),
        Some(expected),
        "the child saw the name in its env"
    );
}
