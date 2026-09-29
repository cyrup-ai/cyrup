//! SUBA-118 — the abort-recovery DISPATCH end to end through `run_sync`, with a REAL child process.
//!
//! This is the row's own `Verify` clause: *"A fake child that emits `compaction_start`,
//! `agent_settled`, then an empty aborted assistant message after a successful tool call is
//! re-launched once with the recovery prompt and the run succeeds; without the compaction events it
//! settles with no re-launch."*
//!
//! Upstream: `planAbortRecovery` (`runs/shared/abort-recovery.ts:68-116` @pi-subagents v0.71.0) and
//! its foreground call site — the two-attempt loop at `runs/foreground/execution.ts:1855-1903`,
//! which reassigns `recoveryPrompt` to `ABORT_RECOVERY_PROMPT` (`:1880`), pushes the
//! `[abort-recovery]` note (`:1881`) and `continue`s on the SAME candidate. The compaction evidence
//! itself is folded at `:970-999` and stamped onto the attempt at `:1418-1420`.
//!
//! The child is a shell script rather than a mock runner on purpose: the resume is only real if the
//! SECOND OS process is launched with the recovery prompt as its task text, and only a real spawn
//! proves the argv the plan's answer produced.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde_json::json;

use crate::exec::abort_recovery::ABORT_RECOVERY_PROMPT;
use crate::exec::testsupport::{base_opts, sample_agent_config};

const TASK: &str = "Audit the crate and write the report";

/// The scripted child. Every launch appends its received task text to `tasks.txt` (an `@<file>`
/// task argument is expanded, which is how a long task crosses the argv boundary) and bumps
/// `launches`.
///
/// The FIRST launch emits, in order: `agent_start`, an assistant turn that calls a tool, the
/// tool's successful result, `compaction_start`, an EMPTY assistant message that reports zero
/// output tokens and `stopReason: "aborted"`, `agent_settled` — then exits non-zero. That is every
/// positive clause of the ladder at once: compaction settlement (`compaction_start` outstanding
/// when the `agent_settled` starts the drain), a terminal assistant abort, useful prior progress,
/// and no unresolved tool call.
///
/// `with_compaction: false` is the negative control: the identical stream with the
/// `compaction_start` line removed, so `afterCompactionSettlement` is false and the plan settles on
/// `compaction settlement not verified` (`abort-recovery.ts:114`).
///
/// Every later launch emits an ordinary successful turn and exits 0.
fn child(dir: &Path, with_compaction: bool) -> PathBuf {
    let script = dir.join("aborting-child.sh");
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let line = |json: &serde_json::Value| format!("printf '%s\\n' {}\n", quote(&json.to_string()));
    let tasks = quote(&dir.join("tasks.txt").display().to_string());
    let launches = quote(&dir.join("launches").display().to_string());

    let mut body = String::from("#!/bin/sh\n");
    // Record the task text: the LAST positional argument, or the contents of an `@<file>` spill.
    body.push_str(&format!(
        "for a in \"$@\"; do case \"$a\" in @*) f=\"${{a#@}}\"; [ -f \"$f\" ] && t=\"$(cat \"$f\")\";; \
         --*) ;; *) t=\"$a\";; esac; done\n\
         printf '%s\\n' \"$t\" >> {tasks}\n\
         printf 'x' >> {launches}\n\
         n=$(wc -c < {launches} | tr -d ' ')\n"
    ));

    // ---- second and later launches: an ordinary success ----
    body.push_str("if [ \"$n\" != \"1\" ]; then\n");
    body.push_str(&line(&json!({ "type": "agent_start" })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{ "type": "text", "text": "resumed and finished the report" }],
            "usage": { "output": 42 },
            "stopReason": "stop",
        },
    })));
    body.push_str(&line(&json!({ "type": "agent_settled" })));
    body.push_str("exit 0\nfi\n");

    // ---- first launch: the compaction-induced abort ----
    body.push_str(&line(&json!({ "type": "agent_start" })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [
                { "type": "text", "text": "reading the crate" },
                { "type": "toolCall", "id": "call-1", "name": "read" },
            ],
            "usage": { "output": 99 },
            "stopReason": "toolUse",
        },
    })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "toolResult",
            "toolCallId": "call-1",
            "content": [{ "type": "text", "text": "fn main() {}" }],
        },
    })));
    if with_compaction {
        body.push_str(&line(
            &json!({ "type": "compaction_start", "reason": "context" }),
        ));
    }
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [],
            "usage": { "output": 0 },
            "stopReason": "aborted",
        },
    })));
    body.push_str(&line(&json!({ "type": "agent_settled" })));
    body.push_str("exit 1\n");

    std::fs::write(&script, body).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

async fn run(dir: &Path, with_compaction: bool) -> crate::exec::run_result::SingleResult {
    let agent = sample_agent_config("m1", &[]);
    let mut opts = base_opts(dir, &["m1"]);
    // `sessionAvailable: Boolean(options.sessionFile && existsSync(options.sessionFile))`
    // (`execution.ts:1882`) — the retained session the resumed child is relaunched against. It must
    // exist on disk: a resume with nothing to resume is refused on the plan's second rung
    // (`abort-recovery.ts:103`).
    let session = dir.join("child.jsonl");
    std::fs::write(&session, "").expect("write the retained session file");
    opts.fork_context.session_file_path = Some(session);
    opts.spawn_command = Some(crate::spawn::SpawnCommand {
        binary: child(dir, with_compaction),
        base_args: Vec::new(),
    });
    crate::exec::run_sync(&agent, TASK, &opts).await
}

fn tasks(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("tasks.txt"))
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect()
}

/// A child that declares an `outputSchema`, runs a tool, compacts, then ends with the SAME empty
/// aborted assistant message — and exits ZERO, never having written its structured capture. That is
/// the one shape in which upstream's `structuredOutputFailed` is actually `true` at the
/// abort-recovery call site: its structured block runs while the attempt is still
/// `exitCode === 0 && !result.error` and sets both the flag and the error (`execution.ts:1465-1471`),
/// so the plan refuses the resume on `abort-recovery.ts:112`.
///
/// The absence of assistant TEXT is load-bearing: prose would make the attempt a plain success at
/// ladder time and the whole question would never be asked.
fn structured_child(dir: &Path) -> PathBuf {
    let script = dir.join("structured-aborting-child.sh");
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let line = |json: &serde_json::Value| format!("printf '%s\\n' {}\n", quote(&json.to_string()));
    let launches = quote(&dir.join("launches").display().to_string());
    let mut body = String::from("#!/bin/sh\n");
    body.push_str(&format!("printf 'x' >> {launches}\n"));
    body.push_str(&line(&json!({ "type": "agent_start" })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": "call-1", "name": "read" }],
            "usage": { "output": 99 },
            "stopReason": "toolUse",
        },
    })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "toolResult",
            "toolCallId": "call-1",
            "content": [{ "type": "text", "text": "fn main() {}" }],
        },
    })));
    body.push_str(&line(
        &json!({ "type": "compaction_start", "reason": "context" }),
    ));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [],
            "usage": { "output": 0 },
            "stopReason": "aborted",
        },
    })));
    body.push_str(&line(&json!({ "type": "agent_settled" })));
    body.push_str("exit 0\n");
    std::fs::write(&script, body).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

/// A child whose BUDGET stopped it: the same compaction-induced abort, but one of its tool results
/// is the run's own hard-block message (`tool-budget.ts:66-68`), which is `result.toolBudgetBlocked`
/// upstream (`execution.ts:1149-1156`) and refuses the resume on `abort-recovery.ts:111`.
///
/// The budget is enforced CHILD-side in this crate (it crosses as `CYRUP_SUBAGENT_TOOL_BUDGET`), so
/// this fixture plays the child's part: it emits the blocked result the real runtime would.
fn budget_blocked_child(dir: &Path, blocked_result: &str) -> PathBuf {
    let script = dir.join("budget-blocked-child.sh");
    let quote = |text: &str| format!("'{}'", text.replace('\'', r"'\''"));
    let line = |json: &serde_json::Value| format!("printf '%s\\n' {}\n", quote(&json.to_string()));
    let launches = quote(&dir.join("launches").display().to_string());
    let mut body = String::from("#!/bin/sh\n");
    body.push_str(&format!("printf 'x' >> {launches}\n"));
    body.push_str(&line(&json!({ "type": "agent_start" })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{ "type": "toolCall", "id": "call-1", "name": "read" }],
            "usage": { "output": 99 },
            "stopReason": "toolUse",
        },
    })));
    body.push_str(&line(&json!({
        "type": "tool_execution_start",
        "toolCallId": "call-1",
        "toolName": "read",
        "args": {},
    })));
    body.push_str(&line(&json!({
        "type": "tool_execution_end",
        "toolCallId": "call-1",
        "toolName": "read",
        "result": [{ "type": "text", "text": blocked_result }],
        "isError": false,
    })));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "toolResult",
            "toolCallId": "call-1",
            "content": [{ "type": "text", "text": blocked_result }],
        },
    })));
    body.push_str(&line(
        &json!({ "type": "compaction_start", "reason": "context" }),
    ));
    body.push_str(&line(&json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [],
            "usage": { "output": 0 },
            "stopReason": "aborted",
        },
    })));
    body.push_str(&line(&json!({ "type": "agent_settled" })));
    body.push_str("exit 1\n");
    std::fs::write(&script, body).expect("write child");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    script
}

fn launches(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("launches"))
        .unwrap_or_default()
        .len()
}

/// The row's `Verify`, positive half.
#[tokio::test]
async fn a_compaction_induced_abort_is_resumed_once_with_the_recovery_prompt_and_the_run_succeeds()
{
    let dir = tempfile::tempdir().expect("tempdir");
    let result = run(dir.path(), true).await;

    let launched = tasks(dir.path());
    assert_eq!(
        launched.len(),
        2,
        "exactly TWO child processes: the aborted one and its resume. Tasks seen: {launched:?}"
    );
    assert!(
        launched[0].contains(TASK),
        "the FIRST launch carries the real task: {:?}",
        launched[0]
    );
    assert!(
        launched[1].contains(ABORT_RECOVERY_PROMPT),
        "the RESUMED launch carries `ABORT_RECOVERY_PROMPT`, not the task again: {:?}",
        launched[1]
    );
    assert!(
        !launched[1].contains(TASK),
        "the resume tells the child to continue, never to restart the task: {:?}",
        launched[1]
    );

    assert_eq!(
        result.exit_code, 0,
        "the resumed run succeeds: {:?}",
        result.error
    );
    assert_eq!(result.error, None);
    assert_eq!(
        result.attempted_models.len(),
        1,
        "a resume is the SAME rung of the ladder: {:?}",
        result.attempted_models
    );
    assert_eq!(
        result.model_attempts.len(),
        2,
        "...but every LAUNCH keeps its own row: {:?}",
        result.model_attempts
    );
    let delivered = result.final_output.clone().unwrap_or_default();
    assert!(
        delivered.contains("[abort-recovery] compaction abort after useful progress"),
        "the operator-facing note is prepended to the delivered output (pi `:1432-1433`): \
         {delivered:?}"
    );
}

/// The row's `Verify`, negative half: the SAME abort, with no compaction outstanding when the child
/// settled, is not resumed at all — `afterCompactionSettlement` is the gate
/// (`abort-recovery.ts:114`).
#[tokio::test]
async fn the_same_abort_without_the_compaction_evidence_settles_with_no_relaunch() {
    let dir = tempfile::tempdir().expect("tempdir");
    let result = run(dir.path(), false).await;

    assert_eq!(
        tasks(dir.path()).len(),
        1,
        "ONE child process: nothing licensed a resume"
    );
    assert_ne!(result.exit_code, 0, "the run fails, as upstream's does");
    let delivered = result.final_output.clone().unwrap_or_default();
    assert!(
        !delivered.contains("[abort-recovery]"),
        "no resume happened, so no resume note: {delivered:?}"
    );
    // The diagnostic rides only on a COMPACTION-looking settle (`abort-recovery.ts:94-98`); this one
    // is not, so the error must not claim a compaction-induced abort was refused.
    let error = result.error.clone().unwrap_or_default();
    assert!(
        !error.contains("Compaction-induced child abort"),
        "an abort with no compaction evidence is not a compaction-induced abort: {error:?}"
    );
}

/// `structuredOutputFailed` (`abort-recovery.ts:112`) REFUSES a resume that every other clause would
/// have granted — and the settle explains itself, because the abort still looked
/// compaction-induced (`:94-98`).
///
/// This is the rung whose input can only be produced at upstream's own position in the per-attempt
/// diagnosis (`attempt_runner.rs`'s step (d.1)): the attempt is clean when the structured verdict is
/// taken and has an error by the time the ladder sees it. Deriving the flag any later — from the
/// settled attempt, or from `run_sync`'s post-ladder step 5 — yields `false` here, and this child is
/// then resumed where upstream settles.
///
/// RED with the flag computed after the empty-output gate instead of before it: two launches, and no
/// diagnostic.
#[tokio::test]
async fn a_declared_schema_with_no_captured_value_refuses_the_resume_and_says_why() {
    let dir = tempfile::tempdir().expect("tempdir");
    let agent = sample_agent_config("m1", &[]);
    let mut opts = base_opts(dir.path(), &["m1"]);
    let session = dir.path().join("child.jsonl");
    std::fs::write(&session, "").expect("write the retained session file");
    opts.fork_context.session_file_path = Some(session);
    opts.structured_output_schema = Some(json!({
        "type": "object",
        "required": ["ok"],
        "properties": { "ok": { "type": "boolean" } },
    }));
    opts.spawn_command = Some(crate::spawn::SpawnCommand {
        binary: structured_child(dir.path()),
        base_args: Vec::new(),
    });
    let result = crate::exec::run_sync(&agent, TASK, &opts).await;

    assert_eq!(
        launches(dir.path()),
        1,
        "the structured-output rung refuses the resume, so there is no second child"
    );
    assert_ne!(result.exit_code, 0);
    let error = result.error.clone().unwrap_or_default();
    assert!(
        error.contains(
            "Compaction-induced child abort could not be resumed safely: structured output failure."
        ),
        "the settle names the rung that refused it, in upstream's wording: {error:?}"
    );
}

/// `toolBudgetExhausted` (`abort-recovery.ts:111`) likewise refuses a resume every other clause
/// would have granted. The evidence is the child's OWN hard-block tool result, recognized by
/// `is_tool_budget_blocked_message` — and only when it is this run's budget and this run's tool, so
/// the control below (a message rendered for a different hard limit) is resumed as before.
///
/// RED without the recognizer in the drive loop: both cases resume, and the first one's diagnostic
/// is absent.
#[tokio::test]
async fn a_child_its_own_tool_budget_stopped_is_not_resumed() {
    let budget = crate::exec::tool_budget::validate_tool_budget_config(
        Some(&json!({ "hard": 3 })),
        "toolBudget",
    )
    .expect("a valid budget")
    .expect("some");

    for (label, blocked_result, expect_launches) in [
        (
            "this run's own hard-block message",
            crate::exec::tool_budget::tool_budget_blocked_message(&budget, "read", 4),
            1usize,
        ),
        (
            "a hard-block message rendered for a DIFFERENT budget",
            crate::exec::tool_budget::tool_budget_blocked_message(
                &crate::exec::tool_budget::validate_tool_budget_config(
                    Some(&json!({ "hard": 99 })),
                    "toolBudget",
                )
                .expect("valid")
                .expect("some"),
                "read",
                100,
            ),
            2usize,
        ),
    ] {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut agent = sample_agent_config("m1", &[]);
        agent.tool_budget = Some(budget.clone());
        let mut opts = base_opts(dir.path(), &["m1"]);
        let session = dir.path().join("child.jsonl");
        std::fs::write(&session, "").expect("write the retained session file");
        opts.fork_context.session_file_path = Some(session);
        opts.spawn_command = Some(crate::spawn::SpawnCommand {
            binary: budget_blocked_child(dir.path(), &blocked_result),
            base_args: Vec::new(),
        });
        let result = crate::exec::run_sync(&agent, TASK, &opts).await;

        assert_eq!(
            launches(dir.path()),
            expect_launches,
            "{label}: launches. error = {:?}",
            result.error
        );
        let error = result.error.clone().unwrap_or_default();
        assert_eq!(
            error.contains(
                "Compaction-induced child abort could not be resumed safely: budget exhausted."
            ),
            expect_launches == 1,
            "{label}: the diagnostic names the budget rung only when the budget actually blocked \
             this run: {error:?}"
        );
    }
}
