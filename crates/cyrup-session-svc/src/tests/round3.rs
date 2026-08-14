//! Round-3 facade parity tests (vs Pi `agent-session.ts`): the retry subsystem, auto-compaction
//! toggles, the immediate-bash seam, dynamic tools + custom tools, `setModel(Model)` + the typed
//! `cycleModel`/scoped models, the `prompt` ordering fix + skill/template expansion, `clone_at`, and
//! the runtime `modelFallbackMessage` getter.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::path::PathBuf;
use std::sync::Arc;

use cyrup_core::{
    AssistantMessage, Content, StopReason, Tool, ToolError, ToolResult, ToolUpdateSink,
};
use cyrup_provider::faux::{
    faux_assistant_message, faux_text, faux_tool_call, FauxConfig, FauxModelDefinition, FauxProvider,
};
use cyrup_provider::Provider;
use crate::{
    BashOptions, ScopedModel, SessionBuilder, SessionConfig, SessionFactory, SessionTarget,
};
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    cwd: PathBuf,
    agent_dir: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let cwd = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    Fixture { _tmp: tmp, cwd, agent_dir }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

/// Stands in for the binary's `select_provider` seam: hands back an offline faux provider for any
/// id, so a cross-provider model change can actually install its owning provider.
struct AnyFauxResolver;

impl crate::ProviderResolver for AnyFauxResolver {
    fn resolve(&self, _provider_id: &str) -> Result<Arc<dyn Provider>, String> {
        Ok(Arc::new(FauxProvider::new()))
    }
}

fn two_model_provider() -> Arc<FauxProvider> {
    let mut reasoning = FauxModelDefinition::new("faux-2");
    reasoning.reasoning = true;
    let cfg = FauxConfig {
        models: vec![FauxModelDefinition::new("faux-1"), reasoning],
        ..FauxConfig::default()
    };
    Arc::new(FauxProvider::with_config(cfg))
}

// A trivial custom tool (Pi `customTools`).
struct EchoTool {
    params: serde_json::Value,
}
impl EchoTool {
    fn new() -> Self {
        Self { params: serde_json::json!({"type": "object", "properties": {}}) }
    }
}
#[async_trait::async_trait]
impl Tool for EchoTool {
    fn name(&self) -> &str {
        "echo"
    }
    fn parameters(&self) -> &serde_json::Value {
        &self.params
    }
    fn description(&self) -> &str {
        "Echo a message"
    }
    fn prompt_snippet(&self) -> Option<&str> {
        Some("Echo a message back")
    }
    async fn execute(
        &self,
        _call_id: cyrup_core::ToolCallId,
        _args: serde_json::Value,
        _cancel: cyrup_core::CancelToken,
        _on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult { content: vec![Content::text("echo")], details: None, terminate: false, ..Default::default() })
    }
}

// ------------------------------------------------------------------------------ retry subsystem ----

#[tokio::test]
async fn retry_toggles_classification_and_backoff() {
    let fx = fixture();
    // Fast backoff so the success path completes quickly.
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("retry", serde_json::json!({"enabled": true, "maxRetries": 2, "baseDelayMs": 3}))
        .unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).cli_settings(cli).build().await.unwrap();

    // Toggle mirrors the settings default, then the override.
    assert!(session.auto_retry_enabled(), "settings default retry.enabled = true");
    session.set_auto_retry_enabled(false);
    assert!(!session.auto_retry_enabled());
    session.set_auto_retry_enabled(true);

    // Classification: a transient error is retryable; a clean stop is not.
    let transient = AssistantMessage::errored(
        "faux".into(),
        "faux-1",
        None,
        StopReason::Error,
        "overloaded: please retry",
    );
    assert!(session.is_retryable_error(&transient), "overloaded is retryable");
    let clean = faux_assistant_message(vec![faux_text("done")], StopReason::Stop);
    assert!(!session.is_retryable_error(&clean), "a clean stop is never retryable");

    // will_retry_after_agent_end scans the last assistant message.
    assert!(session
        .will_retry_after_agent_end(&[cyrup_agent::AgentMessage::Assistant(transient.clone())]));
    assert!(!session
        .will_retry_after_agent_end(&[cyrup_agent::AgentMessage::Assistant(clean.clone())]));

    // prepare_retry: first attempt waits the backoff and signals continue; the budget then exhausts.
    assert_eq!(session.retry_attempt(), 0);
    assert!(session.prepare_retry(&transient).await, "attempt 1 continues");
    assert_eq!(session.retry_attempt(), 1);
    assert!(session.prepare_retry(&transient).await, "attempt 2 continues");
    assert_eq!(session.retry_attempt(), 2);
    assert!(!session.prepare_retry(&transient).await, "budget exhausted at maxRetries");
    assert_eq!(session.retry_attempt(), 2, "attempt count is preserved on exhaustion");
    assert!(!session.is_retrying(), "no backoff is in flight after prepare returns");
}

// -------------------------------------------------------------------------- auto-compaction ----

#[tokio::test]
async fn auto_compaction_toggle_and_is_compacting() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    assert!(session.auto_compaction_enabled(), "settings default compaction.enabled = true");
    assert!(!session.is_compacting(), "nothing compacting at rest");
    session.set_auto_compaction_enabled(false);
    assert!(!session.auto_compaction_enabled());

    // With auto-compaction disabled, check_compaction is a no-op.
    let small = faux_assistant_message(vec![faux_text("hi")], StopReason::Stop);
    assert!(!session.check_compaction(&small, false).await.unwrap(), "disabled = never compacts");
    session.set_auto_compaction_enabled(true);
    // A tiny session is well under threshold → still no compaction.
    assert!(!session.check_compaction(&small, false).await.unwrap(), "small session under threshold");
}

// ------------------------------------------------------------------------------ bash seam ----

#[tokio::test]
async fn execute_bash_records_result_and_persists() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    assert!(!session.is_bash_running());
    assert!(!session.has_pending_bash_messages());
    let result = session
        .execute_bash("echo hello-bash", BashOptions::default(), None)
        .await
        .expect("a well-formed local echo command succeeds");
    assert_eq!(result.exit_code, Some(0), "echo exits 0");
    assert!(result.output.contains("hello-bash"), "captured stdout: {:?}", result.output);
    assert!(!result.cancelled);
    assert!(!session.is_bash_running(), "bash slot cleared after completion");

    // The bash result landed in the agent transcript (not streaming) as a bashExecution message.
    let msgs = session.agent_messages().await;
    assert!(
        msgs.iter().any(|m| matches!(m, cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "bashExecution")),
        "bash result recorded in transcript"
    );
    // abort_bash is idempotent when nothing runs.
    session.abort_bash();
}

/// Poll until `path` exists — the "poll until observed" barrier, NOT a fixed sleep: the thing being
/// waited on is a side effect of a real child PROCESS, which no in-process level-triggered primitive
/// can observe. The bound converts a hang into a named failure; it is never the assertion.
async fn await_marker(path: &std::path::Path) {
    for _ in 0..5_000 {
        if path.exists() {
            return;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }
    panic!("marker {path:?} never appeared — the child never started");
}

/// A shell snippet that announces itself by creating `started` and then blocks until `go` appears,
/// so the test can synchronize on the child's real state instead of on the clock.
fn blocking_command(started: &std::path::Path, go: &std::path::Path) -> String {
    format!(
        "touch {}; while [ ! -f {} ]; do sleep 0.01; done; echo done",
        started.display(),
        go.display()
    )
}

/// DRIFT-029 — `abort_bash` must cancel EVERY in-flight command, not just the most recent.
///
/// Pi holds `_bashAbortControllers = new Set<AbortController>()` (agent-session.ts:337 @v0.83.0),
/// adds one handle per `executeBash` call (`:2771`), and aborts a spread COPY of the whole set
/// (`:2833-2835`). cyrup held a single `Option<CancelToken>` slot that each call overwrote, so with
/// two commands in flight the first was orphaned and ran on after the user asked to stop.
///
/// RED before the fix: the first command is never cancelled, so its join handle never resolves and
/// the bounded await below reports the orphan.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drift029_abort_bash_cancels_every_in_flight_command() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session =
        Arc::new(SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap());

    let started_a = fx.cwd.join("started-a");
    let started_b = fx.cwd.join("started-b");
    let never = fx.cwd.join("never-created");

    let cmd_a = blocking_command(&started_a, &never);
    let cmd_b = blocking_command(&started_b, &never);
    let (sa, sb) = (Arc::clone(&session), Arc::clone(&session));
    let task_a =
        tokio::spawn(async move { sa.execute_bash(&cmd_a, BashOptions::default(), None).await });
    let task_b =
        tokio::spawn(async move { sb.execute_bash(&cmd_b, BashOptions::default(), None).await });

    // Both children are genuinely running before the abort — no clock involved.
    await_marker(&started_a).await;
    await_marker(&started_b).await;
    assert!(session.is_bash_running(), "two commands are in flight");

    session.abort_bash();

    let a = task_a.await.unwrap().expect("the cancelled command still returns a result");
    let b = task_b.await.unwrap().expect("the cancelled command still returns a result");
    assert!(a.cancelled, "the FIRST command must be cancelled too (it was orphaned): {a:?}");
    assert!(b.cancelled, "the second command must be cancelled: {b:?}");
    assert!(!session.is_bash_running(), "the set drains as each call's guard drops");
}

/// DRIFT-029, second half — `is_bash_running` must answer on the whole set (pi's
/// `this._bashAbortControllers.size > 0`, agent-session.ts:2840 @v0.83.0), so the FIRST command to
/// finish may not report the session idle while another is still executing.
///
/// RED before the fix: `execute_bash`'s completion path cleared the single slot unconditionally, so
/// the assertion right after `task_a` finishes read `false`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drift029_is_bash_running_answers_on_the_whole_set() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session =
        Arc::new(SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap());

    let started_a = fx.cwd.join("started-a");
    let started_b = fx.cwd.join("started-b");
    let go_a = fx.cwd.join("go-a");
    let go_b = fx.cwd.join("go-b");

    let cmd_a = blocking_command(&started_a, &go_a);
    let cmd_b = blocking_command(&started_b, &go_b);
    let (sa, sb) = (Arc::clone(&session), Arc::clone(&session));
    let task_a =
        tokio::spawn(async move { sa.execute_bash(&cmd_a, BashOptions::default(), None).await });
    let task_b =
        tokio::spawn(async move { sb.execute_bash(&cmd_b, BashOptions::default(), None).await });

    await_marker(&started_a).await;
    await_marker(&started_b).await;
    assert!(session.is_bash_running());

    // Release ONLY the first command and wait for it to actually return.
    std::fs::write(&go_a, b"").unwrap();
    let a = task_a.await.unwrap().expect("the released command completes");
    assert_eq!(a.exit_code, Some(0));
    assert!(
        session.is_bash_running(),
        "the second command is still executing — a finished sibling may not report the session idle"
    );

    std::fs::write(&go_b, b"").unwrap();
    let b = task_b.await.unwrap().expect("the released command completes");
    assert_eq!(b.exit_code, Some(0));
    assert!(!session.is_bash_running(), "both handles removed once both calls returned");
}

/// The immediate-bash (`!!`/RPC) seam must prepend the managed `agent_dir/bin` onto the child
/// `PATH` exactly like the agent-loop `bash` tool does (Pi `getShellEnv()`'s unconditional
/// `getBinDir()` prefix, `utils/shell.ts:122-128`, reached via `createLocalBashOperations`'s
/// `env: env ?? getShellEnv()`, `core/tools/bash.ts:100`) — mirrors
/// `cyrup-tools/tests/tools.rs::bash_bin_dir_prepended_to_path` for the `bash` tool itself.
#[tokio::test]
async fn execute_bash_prepends_agent_bin_dir_to_path() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    let result = session
        .execute_bash(r#"printf '%s' "$PATH""#, BashOptions::default(), None)
        .await
        .expect("printf runs");
    assert_eq!(result.exit_code, Some(0));
    let bin_dir = fx.agent_dir.join("bin");
    assert!(
        result.output.starts_with(&bin_dir.to_string_lossy().into_owned()),
        "expected PATH to start with the managed bin dir {bin_dir:?}, got: {:?}",
        result.output
    );
}

/// The immediate-bash seam sanitizes output exactly like Pi's `bash-executor.ts`'s `onData`
/// (`sanitizeBinaryOutput(stripAnsi(decoder.decode(data,{stream:true}))).replace(/\r/g,"")`,
/// line 82): raw ANSI SGR codes and carriage returns from a REAL child process must never reach the
/// recorded transcript, only the plain text they decorate.
#[tokio::test]
async fn execute_bash_sanitizes_real_ansi_and_cr_from_a_live_child() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    // `printf` with a real ESC byte (\033) driving an SGR color code, plus a bare CR — exactly the
    // kind of raw terminal control bytes a real command can legitimately emit.
    let result = session
        .execute_bash(
            r#"printf '\033[31mred\033[0m\rtext\n'"#,
            BashOptions::default(),
            None,
        )
        .await
        .expect("printf runs");
    assert_eq!(result.exit_code, Some(0));
    assert!(
        !result.output.contains('\u{1B}'),
        "no raw ESC byte may reach the recorded output: {:?}",
        result.output
    );
    assert!(!result.output.contains('\r'), "no raw CR may reach the recorded output: {:?}", result.output);
    assert_eq!(result.output, "redtext\n", "the plain text survives, decorations stripped");

    // The SAME sanitized text is what actually lands in the persisted transcript entry.
    let msgs = session.agent_messages().await;
    let bash_msg = msgs
        .iter()
        .find_map(|m| match m {
            cyrup_agent::AgentMessage::Custom { kind, payload, .. } if kind == "bashExecution" => {
                Some(payload.clone())
            }
            _ => None,
        })
        .expect("a bashExecution message was recorded");
    assert_eq!(bash_msg["output"], "redtext\n");
}

/// `shellCommandPrefix` (Pi `getShellCommandPrefix`, settings-manager.ts:895-896) must be prepended
/// before the command on the immediate-bash (`!!`/RPC) seam, exactly like Pi's `executeBash`
/// (agent-session.ts:2624-2627: `prefix ? \`${prefix}\n${command}\` : command`).
#[tokio::test]
async fn execute_bash_applies_shell_command_prefix_setting() {
    let fx = fixture();
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("shellCommandPrefix", serde_json::json!("export L4_PREFIX_TEST=from-prefix"))
        .unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).cli_settings(cli).build().await.unwrap();

    let result = session
        .execute_bash("echo $L4_PREFIX_TEST", BashOptions::default(), None)
        .await
        .expect("the prefixed command runs");
    assert_eq!(result.exit_code, Some(0));
    assert!(
        result.output.contains("from-prefix"),
        "shellCommandPrefix must be prepended before the command: {:?}",
        result.output
    );

    // The prefix is applied to the RESOLVED command only — the ORIGINAL command (unprefixed) is
    // what gets recorded into history (Pi `recordBashResult(command, ...)`, agent-session.ts:2628).
    let msgs = session.agent_messages().await;
    let bash_msg = msgs
        .iter()
        .find_map(|m| match m {
            cyrup_agent::AgentMessage::Custom { kind, payload, .. } if kind == "bashExecution" => {
                Some(payload.clone())
            }
            _ => None,
        })
        .expect("a bashExecution message was recorded");
    assert_eq!(bash_msg["command"], "echo $L4_PREFIX_TEST");
}

/// `shellPath` (Pi `getShellPath`, settings-manager.ts:864-865) must be honored on the immediate-bash
/// seam, resolved fresh on each call (Pi `createLocalBashOperations({ shellPath })` → `getShellConfig`
/// inside `exec`, bash.ts:69/89); a missing custom path surfaces the exact `Custom shell path not
/// found` error `getShellConfig` throws (shell.ts:73), matching the agent-loop `bash` tool
/// (`cyrup-tools/src/ops/shell.rs::ShellConfig::resolve`).
#[tokio::test]
async fn execute_bash_missing_custom_shell_path_setting_errors() {
    let fx = fixture();
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("shellPath", serde_json::json!("/no/such/shell/l4-round13-finding1-test"))
        .unwrap();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).cli_settings(cli).build().await.unwrap();

    let err = session
        .execute_bash("echo hi", BashOptions::default(), None)
        .await
        .expect_err("a nonexistent custom shellPath must surface as a real error, not a fabricated success");
    assert!(
        err.to_string().contains("Custom shell path not found"),
        "expected Pi's exact error text, got: {err}"
    );
    // The abort-controller-equivalent bash slot is cleared even on this error path (Pi's `finally`,
    // agent-session.ts:2643).
    assert!(!session.is_bash_running());
}

/// `shellCommandPrefix` must ALSO reach the agent-loop `bash` TOOL (not just the immediate-bash
/// seam) — Pi's `_buildRuntime` passes the SAME `{commandPrefix, shellPath}` into
/// `createAllToolDefinitions` (agent-session.ts:2436-2448), so a real LLM-issued `bash` tool call
/// gets the identical prefix as the `!!`/RPC path. Drives a REAL scripted tool_use through the agent
/// loop end-to-end (not a direct `BashTool::execute` unit call) to observe the wiring live.
#[tokio::test]
async fn agent_loop_bash_tool_applies_shell_command_prefix_setting() {
    let fx = fixture();
    let mut cli = cyrup_config::Settings::new();
    cli.set_field("shellCommandPrefix", serde_json::json!("export L4_TOOL_PREFIX_TEST=from-tool-prefix"))
        .unwrap();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("bash", serde_json::json!({"command": "echo $L4_TOOL_PREFIX_TEST"}))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx)).cli_settings(cli).build().await.unwrap();

    let _ = session.prompt("run the bash tool").await.unwrap();
    session.wait_for_idle().await;

    let msgs = session.agent_messages().await;
    let tool_output: String = msgs
        .iter()
        .filter_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(tr) if tr.tool_name == "bash" => Some(
                tr.content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect();
    assert!(
        tool_output.contains("from-tool-prefix"),
        "the agent-loop bash tool must honor the shellCommandPrefix setting: {tool_output:?}"
    );
}

/// Output exceeding `DEFAULT_MAX_BYTES` (50KB) from a REAL child process is tail-truncated in the
/// returned/recorded `output`, `truncated` is set, and the FULL sanitized output is spilled to a
/// real temp file at `fullOutputPath` — Pi's `truncateTail(fullOutput)` +
/// `ensureTempFile`/`BashResult.{truncated,fullOutputPath}` (`bash-executor.ts:57-124`).
#[tokio::test]
async fn execute_bash_truncates_and_spills_large_real_output() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let session = SessionBuilder::new(faux, base_config(&fx)).build().await.unwrap();

    // ~3000 lines x ~40 bytes ≈ 120KB, comfortably over both the 50KB byte cap and the 2000-line cap.
    let result = session
        .execute_bash(
            "for i in $(seq 1 3000); do echo \"line-number-$i-padding-xxxxxxxxxx\"; done",
            BashOptions::default(),
            None,
        )
        .await
        .expect("the loop runs");
    assert_eq!(result.exit_code, Some(0));
    assert!(result.truncated, "120KB of output must be reported as truncated");
    assert!(
        result.output.len() <= 2 * 50 * 1024,
        "the returned preview must be tail-truncated, not the full 120KB: {} bytes",
        result.output.len()
    );

    let full_path = result.full_output_path.clone().expect("a temp file path must be recorded");
    let full_contents = std::fs::read_to_string(&full_path)
        .unwrap_or_else(|e| panic!("full output temp file must be readable at {full_path}: {e}"));
    assert!(
        full_contents.contains("line-number-3000-padding-xxxxxxxxxx"),
        "the FULL untruncated output (all 3000 lines) must be on disk, got {} bytes",
        full_contents.len()
    );
    assert!(
        full_contents.contains("line-number-1-padding-xxxxxxxxxx"),
        "the first line must also be on disk (nothing dropped from the front)"
    );
    let _ = std::fs::remove_file(&full_path);

    // The recorded transcript entry carries the SAME truncated/fullOutputPath fields.
    let msgs = session.agent_messages().await;
    let bash_msg = msgs
        .iter()
        .find_map(|m| match m {
            cyrup_agent::AgentMessage::Custom { kind, payload, .. } if kind == "bashExecution" => {
                Some(payload.clone())
            }
            _ => None,
        })
        .expect("a bashExecution message was recorded");
    assert_eq!(bash_msg["truncated"], true);
    assert!(bash_msg["fullOutputPath"].is_string());
}

// ---------------------------------------------------------------------------- dynamic tools ----

#[tokio::test]
async fn dynamic_tools_toggle_active_set_and_register_custom() {
    let fx = fixture();
    let faux: Arc<dyn Provider> = Arc::new(FauxProvider::new());
    let mut cfg = base_config(&fx);
    cfg.custom_tools = vec![Arc::new(EchoTool::new())];
    let session = SessionBuilder::new(faux, cfg).build().await.unwrap();

    // The default active set is the built-in selection; the custom tool is enable-able but inactive.
    let active = session.active_tool_names();
    assert!(active.contains(&"read".to_string()), "read active by default: {active:?}");
    let all: Vec<String> = session.all_tools().into_iter().map(|t| t.name).collect();
    assert!(all.contains(&"echo".to_string()), "custom tool registered: {all:?}");
    assert!(session.tool_definition("echo").is_some());
    assert!(
        !session.active_tool_names().contains(&"echo".to_string()),
        "custom tool not auto-activated"
    );

    // Toggle the active set down to just read + echo; the agent's tool array follows.
    session.set_active_tools_by_name(&["read".to_string(), "echo".to_string()]).await;
    let active = session.active_tool_names();
    assert_eq!(active, vec!["read".to_string(), "echo".to_string()]);
    let snap = session.agent_messages().await; // force a snapshot to ensure no panic
    let _ = snap;
    // The agent's tool set now reflects the toggle.
    assert!(session.tool_definition("echo").unwrap().active, "echo is active after toggle");
    assert!(!session.tool_definition("write").map(|t| t.active).unwrap_or(false), "write toggled off");

    // Unknown names are ignored.
    session.set_active_tools_by_name(&["read".to_string(), "nope".to_string()]).await;
    assert_eq!(session.active_tool_names(), vec!["read".to_string()]);
}

// ------------------------------------------------------------------- model: set + cycle typed ----

#[tokio::test]
async fn set_model_resolved_auth_precheck_and_typed_cycle() {
    let fx = fixture();
    let faux = two_model_provider();
    let provider: Arc<dyn Provider> = faux.clone();
    let mut cfg = base_config(&fx);
    cfg.model_pattern = Some("faux-1".to_string());
    // The real host always carries a resolver (`main.rs` hands every factory a
    // `BuiltinProviderResolver`), and it is load-bearing for the cycle below: `cycle_model`'s
    // available arm walks `getAvailable()` across EVERY configured provider (Pi
    // `_modelRuntime.getAvailable()`, agent-session.ts:1644), and this fixture is not hermetic
    // against the ambient environment — a `TOGETHER_API_KEY` in the shell makes `together`
    // configured and puts its catalog in the cycle set, which then has to be installable.
    let session = SessionBuilder::new(provider.clone(), cfg)
        .provider_resolver(Arc::new(AnyFauxResolver) as Arc<dyn crate::ProviderResolver>)
        .build()
        .await
        .unwrap();

    // set_model_resolved on an in-catalog model succeeds.
    let faux2 = provider.models().iter().find(|m| m.id.as_str() == "faux-2").unwrap().clone();
    assert!(session.has_configured_auth(&faux2));
    session.set_model_resolved(faux2.clone()).await.unwrap();
    assert_eq!(session.model().expect("session must have a resolved model").model.as_str(), "faux-2");

    // A fabricated model not in the catalog fails the auth-proxy precheck.
    let mut bogus = faux2.clone();
    bogus.id = "ghost".into();
    assert!(!session.has_configured_auth(&bogus));
    assert!(session.set_model_resolved(bogus).await.is_err(), "out-of-catalog model rejected");

    // Scoped set with a per-model thinking level reports is_scoped = true. Asserted BEFORE the
    // available arm because that arm now legitimately leaves the session on ANOTHER provider (it
    // walks every configured provider, Pi `_modelRuntime.getAvailable()`), which would put this
    // fixture's two scoped models out of the newly installed provider's catalog.
    session.set_scoped_models(vec![
        ScopedModel { model: provider.models()[0].clone(), thinking_level: None },
        ScopedModel { model: faux2.clone(), thinking_level: Some(cyrup_core::ModelThinkingLevel::High) },
    ]);
    let r = session.cycle_model(true).await.unwrap().expect("scoped cycle");
    assert!(r.is_scoped, "scoped set configured → scoped path");

    // Typed cycle over the available (auth-filtered) registry reports is_scoped = false.
    session.set_scoped_models(Vec::new());
    let r = session.cycle_model(true).await.unwrap().expect("two models cycle");
    assert!(!r.is_scoped, "no scoped set configured → available path");
}

// --------------------------------------------------------------- prompt ordering + expansion ----

#[tokio::test]
async fn prompt_injects_next_turn_after_user_and_expands_skill() {
    let fx = fixture();
    // A discoverable skill so `/skill:demo` expands.
    let dir = fx.agent_dir.join("skills").join("demo");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "---\nname: demo\ndescription: a demo skill\n---\n\nSKILL_BODY_MARKER\n",
    )
    .unwrap();

    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    // Stage a next-turn custom message; it must be injected AFTER the user message (Pi ordering).
    session
        .send_custom_message(
            "note",
            serde_json::json!({"text": "ctx"}),
            false,
            None,
            Some(crate::DeliverAs::NextTurn),
        )
        .await
        .unwrap();

    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let _ = session.prompt("/skill:demo extra args").await.unwrap();
    session.wait_for_idle().await;

    let msgs = session.agent_messages().await;
    let user_idx = msgs.iter().position(|m| matches!(m, cyrup_agent::AgentMessage::User { .. }));
    let custom_idx = msgs
        .iter()
        .position(|m| matches!(m, cyrup_agent::AgentMessage::Custom { kind, .. } if kind == "note"));
    let (u, c) = (user_idx.expect("user message present"), custom_idx.expect("next-turn custom present"));
    assert!(u < c, "user message must precede the injected next-turn message (Pi ordering): {u} < {c}");

    // The skill command expanded into the user message body.
    if let cyrup_agent::AgentMessage::User { content, .. } = &msgs[u] {
        let text: String = content
            .iter()
            .filter_map(|x| match x {
                Content::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert!(text.contains("SKILL_BODY_MARKER"), "skill body expanded: {text}");
        assert!(text.contains("<skill name=\"demo\""), "skill block wrapper present");
        assert!(text.contains("extra args"), "trailing args preserved");
    } else {
        panic!("expected a user message at index {u}");
    }
}

// ------------------------------------------------------- clone_at + runtime fallback getter ----

#[tokio::test]
async fn clone_at_creates_new_file_and_runtime_surfaces_fallback() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = SessionBuilder::new(provider.clone(), base_config(&fx)).build().await.unwrap();

    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    let _ = session.prompt("hi").await.unwrap();
    session.wait_for_idle().await;

    let original = session.session_id().clone();
    let cloned = session.clone_at(None).await.unwrap();
    assert_ne!(cloned, original, "clone_at branches into a distinct session id");

    // Runtime re-surfaces the (absent) model-fallback message of its active session.
    let factory = Arc::new(SessionFactory::new(provider, base_config(&fx)));
    let runtime = crate::AgentSessionRuntime::create(factory, SessionTarget::New)
        .await
        .unwrap();
    assert!(runtime.model_fallback_message().await.is_none(), "clean model resolve = no fallback");
}

/// The bash tool the MODEL calls must get the same PATH as the user-facing `/bash` seam.
///
/// pi's `getShellEnv()` (`utils/shell.ts:122-134`) unconditionally prepends `getBinDir()` for every
/// bash child (`tools/bash.ts:100,165`) — there is no pi path where the bash tool spawns without it.
/// cyrup set `bin_dir` only on `execute_bash` (asserted by
/// `execute_bash_prepends_agent_bin_dir_to_path` above), so the agent-loop tool inherited the parent
/// PATH unchanged and a binary managed into `<agent_dir>/bin` was `command not found` for the model
/// while the identical command succeeded through `/bash`.
///
/// Deliberately end-to-end through a real tool call rather than inspecting `BashOpts`: the defect
/// was that two seams DISAGREED, so the test has to exercise the seam that was wrong.
#[tokio::test]
async fn the_agent_loop_bash_tool_also_prepends_the_agent_bin_dir_to_path() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call(
                "bash",
                serde_json::json!({ "command": r#"printf '%s' "$PATH""# }),
            )],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let provider: Arc<dyn Provider> = faux;
    let session = SessionBuilder::new(provider, base_config(&fx)).build().await.unwrap();

    let _ = session.prompt("print the path").await.expect("prompt");
    session.wait_for_idle().await;

    let bin_dir = fx.agent_dir.join("bin").to_string_lossy().into_owned();
    let saw_bin_dir = session.messages().await.iter().any(|m| {
        format!("{m:?}").contains(&bin_dir)
    });
    assert!(
        saw_bin_dir,
        "the agent-loop bash tool's PATH must contain the managed bin dir {bin_dir:?}"
    );
}
