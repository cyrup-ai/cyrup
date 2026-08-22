//! The RPC `bash` surface: a genuine backend failure that must not be fabricated into a success,
//! `abort_bash` interrupting an in-flight command (the G1 concurrency property, whose only
//! deterministic observable is the interrupted command's `cancelled:true`), and the DRIFT-004
//! `user_bash` extension event with its full and partial result overrides.

use std::io::Cursor;
use std::sync::Arc;

use super::support::{build_runtime, build_runtime_with_ext, fixture, parse_lines, type_of};
use crate::run_rpc;
use cyrup_provider::faux::FauxProvider;
use serde_json::Value;

/// A genuine immediate-bash backend failure (not a cancellation) must be reported as a real RPC
/// `error(...)` response, NEVER fabricated into a "successful" `bash` response — and must NEVER be
/// recorded into transcript history. Pi's `executeBashWithOperations` only catches the abort case in
/// its `catch` block (`bash-executor.ts:130-155`); every other error `throw`s (line 154),
/// propagating through `AgentSession.executeBash` uncaught (`agent-session.ts:2628-2643`:
/// `recordBashResult` is only reached on the success path inside `try`) to the RPC dispatcher's
/// `catch` (`rpc-mode.ts:756-772`), which converts it into an `error(...)` response with no history
/// entry ever recorded.
#[tokio::test]
async fn rpc_bash_backend_failure_is_not_fabricated_into_a_success() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;
    let session = runtime.session().await;

    // Doom every real spawn attempt: `LocalProc::exec` checks the session cwd exists BEFORE ever
    // spawning (mirrors Pi's `fsAccess(cwd, F_OK)`, bash.ts:70-74) — remove it out from under the
    // already-built session so the bash command below hits a genuine backend error, not a
    // cancellation and not a real process failure racy across platforms.
    std::fs::remove_dir_all(&fx.cwd).expect("remove the session cwd out from under the session");

    let input = concat!(r#"{"type":"bash","id":"b1","command":"echo hi"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let bash_resp = lines.iter().find(|l| l["id"] == "b1").expect("bash response");
    assert_eq!(
        bash_resp["success"], false,
        "a genuine backend failure must not report success: {bash_resp}"
    );
    assert!(
        bash_resp["error"].as_str().unwrap_or_default().contains("Working directory does not exist"),
        "the real backend error message must surface verbatim: {bash_resp}"
    );
    assert!(bash_resp["data"].is_null(), "a failed bash call carries no data payload: {bash_resp}");

    // `cyrup_agent::AgentMessage` isn't a direct dependency of this crate; serialize the live agent
    // state generically (its `Custom{kind:"bashExecution",..}` variant always serializes with that
    // literal string, `session.rs`'s `record_bash_result`) rather than naming the foreign type.
    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        !msgs_json.to_string().contains("bashExecution"),
        "a genuine backend failure must NEVER be recorded into transcript history: {msgs_json}"
    );
}

// ----------------------------------------------------------------------------------------------
// G1 (CRITICAL) — the command loop must not be serialized: `abort_bash`/`abort` sent WHILE a
// long-running command is in flight must interrupt it (Pi `void handleInputLine(line)`,
// rpc-mode.ts:782, dispatches each line concurrently, so `abort_bash` reaches `session.abortBash()`
// (rpc-mode.ts:557-560) while the in-flight `bash`'s `await session.executeBash(...)`
// (rpc-mode.ts:550-555) is still running).
// ----------------------------------------------------------------------------------------------

/// Drives a real `sleep`-backed `bash` over the in-memory transport, sends `abort_bash` immediately
/// after, and asserts the whole exchange finishes far faster than the sleep would take on its own —
/// only possible if the loop serviced `abort_bash` concurrently with the blocking `bash`, cancelling
/// it. On the pre-fix (fully-serialized) loop the `abort_bash` line stays buffered until the `bash`
/// `dispatch().await` returns naturally (~the full sleep), so `bash` runs to completion
/// (`cancelled:false`) and the exchange takes ~the whole sleep.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rpc_abort_bash_interrupts_a_running_bash_command() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let runtime = build_runtime(&fx, faux).await;

    // A bash that would run for 6s if never interrupted, immediately followed by `abort_bash`.
    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"sleep 6"}"#,
        "\n",
        r#"{"type":"abort_bash","id":"ab"}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();

    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let lines = parse_lines(&out);
    let bash = lines.iter().find(|l| l["id"] == "b1").expect("bash response");
    let abort = lines.iter().find(|l| l["id"] == "ab").expect("abort_bash response");
    assert_eq!(abort["command"], "abort_bash");
    assert_eq!(abort["success"], true, "abort_bash must be acknowledged: {abort}");

    // SEAM-030 — the wall-clock assertion that used to sit here (`elapsed < 3s`, "proving the
    // command loop is serialized") is DELETED. It asserted a scheduling outcome the test cannot
    // control, so under CI load or a debug build it failed for reasons unrelated to the behaviour
    // under test, while the deterministic assertion five lines below proves the same property: a
    // `cancelled:true` bash result can ONLY arise if `abort_bash` was serviced while `bash` was
    // still running (G1).
    assert_eq!(bash["command"], "bash");
    assert_eq!(
        bash["data"]["cancelled"], true,
        "the interrupted bash must report cancelled:true, not a full completion: {bash}"
    );
}

// ----------------------------------------------------------------------------------------------
// DRIFT-004 — the JSON-RPC `bash` command must fire the `user_bash` extension event.
//
// Pi `rpc-mode.ts:558-579`'s `case "bash"` (added by pi 5d548ae9, 2026-07-28, "fix: rpc bash no
// longer bypass user_bash", #7214) emits `emitUserBash({type:"user_bash", command,
// excludeFromContext: command.excludeFromContext ?? false, cwd: sessionManager.getCwd()})` BEFORE
// touching the executor; a handler returning a full `UserBashEventResult.result`
// (`extensions/types.ts:1078-1083`) short-circuits execution entirely — Pi records that override
// via `recordBashResult` and answers `success(id, "bash", eventResult.result)`.
//
// The emission lives at the CALLER, not inside `AgentSession.executeBash` — Pi's `executeBash` still
// has no `emitUserBash` at HEAD, and both front-ends (`rpc-mode.ts:559`, `interactive-mode.ts:6014`)
// emit for themselves. cyrup mirrors that with the shared
// `AgentSession::execute_bash_with_user_event` wrapper over the bare `execute_bash`.
// ----------------------------------------------------------------------------------------------

/// A native extension that records every `user_bash` event payload it is delivered, and optionally
/// answers with a full `UserBashEventResult.result` override that must short-circuit execution.
struct RpcUserBashProbe {
    /// `(command, exclude_from_context, cwd)` for each delivered `user_bash` event.
    seen: Arc<std::sync::Mutex<Vec<(String, bool, String)>>>,
    /// When set, returned as the `result` override (Pi `UserBashEventResult.result`).
    override_result: Option<Value>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for RpcUserBashProbe {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("rpc-user-bash-probe")
    }
    async fn init(&self, api: &mut cyrup_ext::InitApi) -> Result<(), cyrup_ext::ExtError> {
        api.subscribe(&[cyrup_ext::EventKind::UserBash]);
        Ok(())
    }
    async fn on_event(
        &self,
        ev: &cyrup_ext::HostEvent,
        _ctx: &cyrup_ext::HostCtx,
    ) -> cyrup_ext::HookOutcome {
        if let cyrup_ext::HostEvent::UserBash { command, exclude_from_context, cwd } = ev {
            self.seen.lock().unwrap().push((
                command.clone(),
                *exclude_from_context,
                cwd.clone(),
            ));
            if let Some(result) = &self.override_result {
                return cyrup_ext::HookOutcome::Handled(cyrup_ext::HandledValue(
                    serde_json::json!({ "result": result }),
                ));
            }
        }
        cyrup_ext::HookOutcome::Noop
    }
}

/// DRIFT-004 (a): an extension subscribed to `user_bash` actually RECEIVES the event — with the live
/// `{command, excludeFromContext, cwd}` — when the command arrives over JSON-RPC, and the command
/// still really executes when no handler overrides it (Pi `rpc-mode.ts:559-564,573-578`). Pre-fix,
/// `SessionCommand::Bash` called the bare `execute_bash`, so the probe stayed empty and every
/// RPC-issued command was invisible to extensions.
#[tokio::test]
async fn rpc_bash_delivers_user_bash_to_an_extension() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe { seen: seen.clone(), override_result: None });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo rpc-hello","excludeFromContext":true}"#,
        "\n",
    );
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    let delivered = seen.lock().unwrap().clone();
    assert_eq!(
        delivered.len(),
        1,
        "the extension must RECEIVE exactly one user_bash event from the RPC bash command: \
         {delivered:?}"
    );
    assert_eq!(delivered[0].0, "echo rpc-hello", "the live command crosses the seam");
    assert!(delivered[0].1, "the RPC excludeFromContext flag crosses the seam");
    assert_eq!(delivered[0].2, fx.cwd.display().to_string(), "the session cwd crosses the seam");

    // With no override, the command still really ran. Select the RESPONSE, not the
    // `bash_execution_update` event that shares the request id (DRIFT-006).
    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["success"], true, "an un-overridden RPC bash still executes: {resp}");
    assert!(
        resp["data"]["output"].as_str().unwrap_or_default().contains("rpc-hello"),
        "the real command output is returned: {resp}"
    );
}

/// DRIFT-004 (b): a `user_bash` handler that returns a full `result` override short-circuits local
/// execution — the RPC response carries the extension's `BashResult` verbatim and the transcript
/// records that override (Pi `rpc-mode.ts:566-571`: `recordBashResult(...)` then
/// `success(id, "bash", eventResult.result)`). The command chosen here would produce completely
/// different output if it were actually executed, so the assertion cannot pass by accident.
#[tokio::test]
async fn rpc_bash_honors_a_user_bash_result_override() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe {
        seen: seen.clone(),
        override_result: Some(serde_json::json!({
            "output": "handled-by-extension",
            "exitCode": 0,
            "cancelled": false,
            "truncated": false,
        })),
    });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;
    let session = runtime.session().await;

    let input =
        concat!(r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    assert_eq!(seen.lock().unwrap().len(), 1, "the handler was consulted");

    let lines = parse_lines(&out);
    assert!(
        !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
        "a short-circuited bash streams no execution deltas — nothing ran: {lines:?}"
    );
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["success"], true, "an overridden bash is a success response: {resp}");
    assert_eq!(
        resp["data"]["output"], "handled-by-extension",
        "the extension's result override is returned verbatim: {resp}"
    );
    assert!(
        !resp["data"]["output"].as_str().unwrap_or_default().contains("locally-executed"),
        "a result override must short-circuit local execution entirely: {resp}"
    );

    // Pi records the override through the same `recordBashResult` path a real execution uses.
    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        msgs_json.to_string().contains("handled-by-extension"),
        "the overridden result is recorded into the transcript: {msgs_json}"
    );
}

/// DRIFT-004 (c): a PARTIAL `result` override must short-circuit too. Pi is TypeScript with no
/// runtime type enforcement, so `emitUserBash` short-circuits on ANY truthy `result`
/// (`runner.ts:955-981`) and `rpc-mode.ts:566-571` takes it unconditionally — an extension may
/// legally return just `{output, exitCode}`.
///
/// A strict deserializer here would be a FAIL-OPEN, not a nicety: a sandbox or remote-exec
/// extension returning that shape would yield `None`, the override would be dropped, and the
/// command would fall through and run raw on the local shell — exactly what the extension existed
/// to prevent. `BashResult` is therefore `#[serde(default)]` on every field.
///
/// The sibling test above supplies all four fields and so cannot catch this.
#[tokio::test]
async fn rpc_bash_honors_a_partial_user_bash_result_override() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe {
        seen: seen.clone(),
        // Only the two fields a sandbox extension would realistically know.
        override_result: Some(serde_json::json!({
            "output": "sandboxed-elsewhere",
            "exitCode": 0,
        })),
    });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;
    let session = runtime.session().await;

    let input = concat!(r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#, "\n");
    let reader = Cursor::new(input.as_bytes().to_vec());
    let mut out: Vec<u8> = Vec::new();
    run_rpc(&runtime, reader, &mut out).await.expect("rpc mode runs");

    assert_eq!(seen.lock().unwrap().len(), 1, "the handler was consulted");

    let lines = parse_lines(&out);
    assert!(
        !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
        "a partial override must short-circuit local execution just like a full one: {lines:?}"
    );
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["data"]["output"], "sandboxed-elsewhere", "partial override honored: {resp}");
    assert!(
        !resp["data"]["output"].as_str().unwrap_or_default().contains("locally-executed"),
        "the command must NOT have reached the local shell: {resp}"
    );
    // Omitted fields fall back to their defaults rather than voiding the whole override.
    assert_eq!(resp["data"]["cancelled"], false, "omitted `cancelled` defaults: {resp}");
    assert_eq!(resp["data"]["truncated"], false, "omitted `truncated` defaults: {resp}");

    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        msgs_json.to_string().contains("sandboxed-elsewhere"),
        "the partial override is recorded into the transcript: {msgs_json}"
    );
}
