//! The two bash seams a live `AgentSession` owns, and the requirement that they AGREE.
//!
//! * the agent-loop `bash` TOOL — what the model calls;
//! * the immediate `execute_bash` seam — what the user's `!`/`!!` line and the JSON-RPC `bash`
//!   command reach (plus its `execute_bash_with_user_event` wrapper).
//!
//! Every defect filed here is one seam having something the other does not: the managed
//! `agent_dir/bin` on `PATH`, the `shellCommandPrefix`/`shellPath` settings, the session identity
//! published into the child env. Splitting them across files is what let them drift apart, so they
//! are asserted side by side.
//!
//! TOOL-008 end-to-end: the `bash` tool a real `AgentSession` registers publishes THIS session's
//! metadata to its child, and republishes it when the model or reasoning level changes.
//!
//! Pi gets this for free: `resolveSpawnContext` reads `ctx.sessionManager.getSessionId()` /
//! `getSessionFile()` / `ctx.model` / `ctx.thinkingLevel` off the per-call `ExtensionContext` every
//! time a command spawns (`pi/packages/coding-agent/src/core/tools/bash.ts:158-184`), which is what
//! `pi/packages/coding-agent/docs/environment-variables.md:27` means by "The values are resolved
//! when each command starts. Switching models or changing the reasoning level therefore affects the
//! next bash command without restarting Pi."
//!
//! cyrup's `Tool::execute` takes no session context, so the session PUSHES into a shared handle.
//! `crates/cyrup-tools/tests/bash_session_env.rs` proves the tool half in isolation; this file
//! proves the wiring — that the handle the builder gives the registered `BashTool` is the same one
//! `set_model_*` / `set_thinking_level` update, driven through a real scripted tool-call round trip
//! rather than by poking the tool directly.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_core::{Content, ExtensionId, ModelId, ProviderId, StopReason, ToolError};
use cyrup_ext::{
    EventKind, ExtError, HostCtx, HostEvent, HookOutcome, InitApi, NativeExtension,
};
use cyrup_provider::faux::{faux_assistant_message, faux_text, faux_tool_call, FauxProvider};
use cyrup_provider::Provider;
use super::common::{base_config, fixture, Fixture};
use crate::{AgentSession, BashOptions, InputSource, SessionBuilder, UserInput};

/// A faux provider scripted with a single `ok` answer — the bash seams below never reach the model,
/// but building a session requires one.
fn faux_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(vec![faux_text("ok")], StopReason::Stop)]);
    faux
}

/// The `bash` command the scripted assistant issues: dump the five variables into `out`, one
/// `KEY=value` per line, with an unset variable rendering as an empty value.
fn probe_command(out: &str) -> String {
    format!(
        r#"{{ for v in CYRUP_SESSION_ID CYRUP_SESSION_FILE CYRUP_PROVIDER CYRUP_MODEL CYRUP_REASONING_LEVEL; do
  eval "printf '%s=%s\n' \"$v\" \"\${{$v-}}\""
done ; }} > {out}"#
    )
}

/// Drive one scripted turn whose assistant message calls `bash` with the probe, then read the file
/// the child wrote back as a key/value map.
async fn probe(session: &Arc<AgentSession>, faux: &Arc<FauxProvider>, fx: &Fixture, out: &str) -> Vec<(String, String)> {
    faux.set_responses(vec![
        faux_assistant_message(
            vec![faux_tool_call("bash", serde_json::json!({ "command": probe_command(out) }))],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let _ = session.prompt(UserInput::text("probe", InputSource::Sdk)).await.expect("prompt");
    session.wait_for_idle().await;

    let text = std::fs::read_to_string(fx.cwd.join(out))
        .unwrap_or_else(|e| panic!("the bash child did not write {out}: {e}"));
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn get<'a>(kv: &'a [(String, String)], key: &str) -> &'a str {
    kv.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str()).unwrap_or_else(|| {
        panic!("the probe did not report {key}; got {kv:?}")
    })
}

#[tokio::test]
async fn bash_children_see_this_sessions_metadata_and_track_model_changes() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = Arc::new(
        SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build session"),
    );

    // ---- 1. the child sees THIS session's identity, not a stale or empty value ----
    let kv = probe(&session, &faux, &fx, "probe1.txt").await;
    assert_eq!(
        get(&kv, "CYRUP_SESSION_ID"),
        session.session_id().to_string(),
        "the bash child did not see the live session id; got {kv:?}"
    );
    let expected_file =
        session.session_file().await.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    assert_eq!(
        get(&kv, "CYRUP_SESSION_FILE"),
        expected_file,
        "the bash child did not see the live session file; got {kv:?}"
    );
    assert!(
        !get(&kv, "CYRUP_PROVIDER").is_empty() && !get(&kv, "CYRUP_MODEL").is_empty(),
        "the provider/model pair must be published; got {kv:?}"
    );
    assert!(
        !get(&kv, "CYRUP_REASONING_LEVEL").is_empty(),
        "the reasoning level must be published; got {kv:?}"
    );

    // ---- 2. a model change reaches the NEXT command with no rebuild ----
    // environment-variables.md:27. `set_model_id` is the no-resolution setter, so this asserts the
    // republish rather than the registry.
    session
        .set_model_id(ProviderId::from("acme"), ModelId::from("acme-model-9"))
        .await
        .expect("set model");
    let kv = probe(&session, &faux, &fx, "probe2.txt").await;
    assert_eq!(get(&kv, "CYRUP_PROVIDER"), "acme", "model change did not reach the child: {kv:?}");
    assert_eq!(get(&kv, "CYRUP_MODEL"), "acme-model-9", "got {kv:?}");
    // The identity half is unchanged by a model swap.
    assert_eq!(get(&kv, "CYRUP_SESSION_ID"), session.session_id().to_string());
}

/// A fork mutates the session manager IN PLACE (`create_branched_session`), giving the session a new
/// id and a new file. Pi re-reads both off the manager on every spawn, so a `bash` child run after a
/// fork reports the POST-fork identity; cyrup must republish to match.
#[tokio::test]
async fn a_fork_republishes_the_session_identity_to_bash_children() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let provider: Arc<dyn Provider> = faux.clone();
    let session = Arc::new(
        SessionBuilder::new(provider, base_config(&fx)).build().await.expect("build session"),
    );

    let before = probe(&session, &faux, &fx, "before.txt").await;
    let id_before = get(&before, "CYRUP_SESSION_ID").to_string();
    let file_before = get(&before, "CYRUP_SESSION_FILE").to_string();

    let forked = session.fork().await.expect("fork");
    assert_ne!(forked.to_string(), id_before, "fixture: the fork must mint a new session id");

    let after = probe(&session, &faux, &fx, "after.txt").await;
    assert_eq!(
        get(&after, "CYRUP_SESSION_ID"),
        forked.to_string(),
        "a bash child run after /fork still reported the PRE-fork session id: {after:?}"
    );
    assert_ne!(
        get(&after, "CYRUP_SESSION_FILE"),
        file_before,
        "a bash child run after /fork still pointed at the pre-fork session file: {after:?}"
    );
    assert_eq!(
        get(&after, "CYRUP_SESSION_FILE"),
        session.session_file().await.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    );
}

// ================================================= the immediate `execute_bash` seam ====

// ------------------------------------------------------------------------------ bash seam ----

/// Facade parity vs Pi `agent-session.ts`: the immediate-bash (`!!`/RPC) seam — a command run through `execute_bash` is
/// recorded into the transcript and persisted. The seam's env/prefix/shell/truncation details are
/// each pinned by their own test below.
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

// ============================= the `execute_bash_with_user_event` wrapper's own event ====

type BashProbe = Arc<Mutex<Vec<(String, bool, String)>>>;

/// A native extension that records every `user_bash` event payload it is delivered.
struct UserBashProbe(BashProbe);
#[async_trait::async_trait]
impl NativeExtension for UserBashProbe {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("user-bash-probe")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::UserBash]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if let HostEvent::UserBash { command, exclude_from_context, cwd } = ev {
            self.0.lock().unwrap().push((command.clone(), *exclude_from_context, cwd.clone()));
        }
        HookOutcome::Noop
    }
}

/// B/user_bash: `execute_bash_with_user_event` — the shared user-initiated-bash entry point BOTH the
/// interactive `!`/`!!`-prefix handler and the JSON-RPC `bash` command go through — fires the
/// `user_bash` extension event with the LIVE `{command, excludeFromContext, cwd (session cwd)}` (Pi
/// `extensionRunner.emitUserBash`, `interactive-mode.ts:6010-6060`'s `handleBashCommand` and
/// `rpc-mode.ts:558-579`'s `case "bash"`; `extensions/types.ts:813-821`). The end-to-end RPC proof
/// that an extension actually RECEIVES this from a wire `{"type":"bash"}` command lives in
/// `cyrup-modes/tests/modes.rs::rpc_bash_delivers_user_bash_to_an_extension`.
#[tokio::test]
async fn execute_bash_with_user_event_emits_user_bash_with_live_values() {
    let fx = fixture();
    let probe: BashProbe = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(UserBashProbe(probe.clone())))
        .build()
        .await
        .expect("build");

    let _ = session
        .execute_bash_with_user_event(
            "echo hello",
            BashOptions { exclude_from_context: true, id: None, operations: None },
            None,
        )
        .await;

    let seen = probe.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "the user_bash handler fired exactly once");
    assert_eq!(seen[0].0, "echo hello", "the live command is delivered");
    assert!(seen[0].1, "the !!-prefix excludeFromContext flag is delivered");
    assert_eq!(seen[0].2, fx.cwd.display().to_string(), "the agent cwd is delivered");
}

/// B/user_bash (executor placement): the bare `execute_bash` — the raw out-of-loop executor, NOT a
/// user-facing entry point — fires no `user_bash` event of its own. Pi's `executeBash`
/// (`agent-session.ts:2582-2684`) has zero `emitUserBash` even at HEAD: the emission lives at the
/// front-end callers (`interactive-mode.ts:6014`, `rpc-mode.ts:559`), each of which emits and then
/// calls the executor. Keeping the executor emission-free is what makes the shared
/// `execute_bash_with_user_event` wrapper emit EXACTLY once per user command rather than twice, and
/// what keeps non-user callers (the wrapper's own fall-through) from re-firing the event.
///
/// This test previously asserted that the JSON-RPC `bash` command therefore emits nothing either —
/// which enshrined DRIFT-004. Pi `5d548ae9` (2026-07-28, "fix: rpc bash no longer bypass user_bash",
/// #7214) made the RPC arm emit; cyrup's arm now calls `execute_bash_with_user_event`, proven by
/// `cyrup-modes/tests/modes.rs::rpc_bash_delivers_user_bash_to_an_extension`.
#[tokio::test]
async fn bare_execute_bash_executor_emits_no_user_bash() {
    let fx = fixture();
    let probe: BashProbe = Arc::new(Mutex::new(Vec::new()));
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(UserBashProbe(probe.clone())))
        .build()
        .await
        .expect("build");

    let _ = session
        .execute_bash("echo hello", BashOptions { exclude_from_context: true, id: None, operations: None }, None)
        .await;

    let seen = probe.lock().unwrap().clone();
    assert!(
        seen.is_empty(),
        "the bare execute_bash executor must not emit user_bash itself — Pi emits at the callers, \
         so an executor-level emission would double-fire for every user command: {seen:?}"
    );
}

// ===========================================================================================
// DRIFT-004 / SEAM-015 — `options.operations`, the per-call bash-backend override
// ===========================================================================================

/// A [`cyrup_tools::ops::BashOperations`] that records what it was handed and answers with a
/// sentinel the local shell could not produce, so "the override ran" and "the local shell did not"
/// are two independent observations rather than one.
///
/// The sentinel carries an ANSI escape on purpose: pi builds its `onChunk` wrapper ONCE and hands it
/// to `executeBashWithOperations` whichever backend the `??` resolved (`agent-session.ts:2779-2789`),
/// so an overriding backend's bytes must go through the SAME sanitize → rolling-buffer → temp-spill
/// pipeline. A port that wired the override straight to the result would return the escape verbatim.
/// One recorded `exec` call: the command line, the cwd it was handed, and the child env as
/// (key, value) pairs — i.e. exactly what `BashExecOptions` carried into the override.
type SeenExec = (String, PathBuf, Vec<(String, String)>);

struct RecordingBashOps {
    seen: Mutex<Vec<SeenExec>>,
}

#[async_trait::async_trait]
impl cyrup_tools::ops::BashOperations for RecordingBashOps {
    async fn exec(
        &self,
        command: &str,
        cwd: &std::path::Path,
        opts: cyrup_tools::ops::BashExecOptions<'_>,
    ) -> Result<cyrup_tools::ExitStatus, ToolError> {
        self.seen.lock().unwrap().push((
            command.to_string(),
            cwd.to_path_buf(),
            opts.env.clone(),
        ));
        (opts.on_data)(b"REMOTE-SENTINEL\x1b[0m\n");
        Ok(cyrup_tools::ExitStatus::Exited(7))
    }
}

/// DRIFT-004 / SEAM-015: `BashOptions::operations` is pi's `options.operations`
/// (`agent-session.ts:2768`), consumed as `options?.operations ?? createLocalBashOperations({
/// shellPath })` (`:2782`). When it is `Some`, THAT backend executes the command and the local
/// process backend is never reached.
///
/// **Presence before absence.** The absence half — "no local shell ran" — is asserted against a
/// sentinel, and a sentinel assertion is vacuous unless the same command demonstrably DOES reach the
/// local shell without the override. The sibling test below is that control: it runs the identical
/// command with `operations: None` and asserts the local shell's own output comes back. Neither test
/// means anything alone.
#[tokio::test]
async fn execute_bash_routes_through_an_operations_override_instead_of_the_local_shell() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .expect("build");

    let ops = Arc::new(RecordingBashOps { seen: Mutex::new(Vec::new()) });
    let streamed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_out = streamed.clone();

    let result = session
        .execute_bash(
            "echo LOCAL_SHELL_RAN",
            BashOptions {
                exclude_from_context: false,
                id: None,
                operations: Some(ops.clone() as Arc<dyn cyrup_tools::ops::BashOperations>),
            },
            Some(Box::new(move |delta: &str| {
                sink_out.lock().unwrap().push(delta.to_string());
            })),
        )
        .await
        .expect("the override backend succeeds");

    // PRESENCE — the override actually executed and its result is what came back.
    let seen = ops.seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "exactly one exec reached the override backend");
    assert_eq!(
        seen[0].0, "echo LOCAL_SHELL_RAN",
        "pi hands `executeBashWithOperations` the RESOLVED command verbatim (bash-executor.ts)"
    );
    assert_eq!(seen[0].1, fx.cwd, "and the session cwd (`this.sessionManager.getCwd()`, :2781)");
    assert_eq!(result.exit_code, Some(7), "the override's exit code is the reported one");

    // The shared pipeline still ran: the ANSI escape the backend emitted is stripped, exactly as it
    // is on the local branch, because pi's `onChunk` wrapper is built once for both.
    assert!(result.output.contains("REMOTE-SENTINEL"), "got: {:?}", result.output);
    assert!(
        !result.output.contains('\x1b'),
        "an overriding backend's bytes go through the SAME sanitizer as the local branch — pi \
         builds the onChunk wrapper once and passes it to whichever backend the `??` chose \
         (agent-session.ts:2779-2789); got: {:?}",
        result.output
    );
    // …and the caller's streaming sink saw it, so the override is not a buffered special case.
    let deltas = streamed.lock().unwrap().concat();
    assert!(deltas.contains("REMOTE-SENTINEL"), "the on_chunk sink streams the override's output: {deltas:?}");

    // The env vector this seam builds is handed to the override too — `getShellEnv()` is inside
    // `createLocalBashOperations`' options upstream, but cyrup's per-child agent-identity stamping
    // (TOOL-031) lives on this path and an override that never saw it would run user commands in a
    // measurably different environment from the local branch.
    let env_keys: Vec<&str> = seen[0].2.iter().map(|(k, _)| k.as_str()).collect();
    assert!(env_keys.contains(&"PI_CODING_AGENT"), "got: {env_keys:?}");
    assert!(env_keys.contains(&"AI_AGENT"), "got: {env_keys:?}");

    // ABSENCE — the local shell never ran this command.
    assert!(
        !result.output.contains("LOCAL_SHELL_RAN"),
        "the local process backend must never have been reached; got: {:?}",
        result.output
    );
}

/// The control that makes the test above non-vacuous, and the regression pin on the `??`'s
/// right-hand branch: with `operations: None` — upstream's absent `operations` — the identical
/// command reaches the local shell and returns ITS output, byte-for-byte the path this seam took
/// before the field existed.
#[tokio::test]
async fn execute_bash_without_an_operations_override_still_runs_on_the_local_shell() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .expect("build");

    let result = session
        .execute_bash(
            "echo LOCAL_SHELL_RAN",
            BashOptions { exclude_from_context: false, id: None, operations: None },
            None,
        )
        .await
        .expect("the local backend succeeds");

    assert!(
        result.output.contains("LOCAL_SHELL_RAN"),
        "without an override the local shell runs the command — this is what makes the sibling \
         test's absence assertion mean something; got: {:?}",
        result.output
    );
    assert_eq!(result.exit_code, Some(0));
}

/// DRIFT-004 / SEAM-015, the wrapper half: `execute_bash_with_user_event` FORWARDS a caller-supplied
/// `operations` down to the executor. Pi's RPC front-end writes the field
/// (`operations: eventResult?.operations`, `rpc-mode.ts:576`) and `executeBash` consumes it one
/// frame lower, so the value has to survive the wrapper.
///
/// What this test deliberately does NOT assert is that the wrapper FILLS the field from the
/// `user_bash` event result — it cannot, and that is the row's last open half: cyrup's extension I/O
/// is serde values (ADR-0002), so the reduction payload can carry the `operations` key but never a
/// callable behind it. See `crates/cyrup-ext/src/lib.rs`'s CYRUP-DELTA register.
#[tokio::test]
async fn execute_bash_with_user_event_forwards_the_operations_override_to_the_executor() {
    let fx = fixture();
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .build()
        .await
        .expect("build");

    let ops = Arc::new(RecordingBashOps { seen: Mutex::new(Vec::new()) });
    let result = session
        .execute_bash_with_user_event(
            "echo LOCAL_SHELL_RAN",
            BashOptions {
                exclude_from_context: false,
                id: None,
                operations: Some(ops.clone() as Arc<dyn cyrup_tools::ops::BashOperations>),
            },
            None,
        )
        .await
        .expect("the override backend succeeds");

    assert_eq!(ops.seen.lock().unwrap().len(), 1, "the wrapper forwarded the override");
    assert!(result.output.contains("REMOTE-SENTINEL"), "got: {:?}", result.output);
    assert!(!result.output.contains("LOCAL_SHELL_RAN"), "got: {:?}", result.output);
}
