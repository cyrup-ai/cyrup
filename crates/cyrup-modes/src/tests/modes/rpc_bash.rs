//! The RPC `bash` surface: a genuine backend failure that must not be fabricated into a success,
//! `abort_bash` interrupting an in-flight command (the G1 concurrency property, whose only
//! deterministic observable is the interrupted command's `cancelled:true`), and the DRIFT-004
//! `user_bash` extension event with its `{result}` override, its `{operations}` backend and the
//! #9068 fail-closed verdict for every other DEFINED shape.

use std::sync::Arc;

use super::support::{
    build_runtime, build_runtime_with_ext, fixture, has_response, parse_lines, run_rpc_until_bytes,
    type_of,
};
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
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

    let lines = parse_lines(&out);
    let bash_resp = lines
        .iter()
        .find(|l| l["id"] == "b1")
        .expect("bash response");
    assert_eq!(
        bash_resp["success"], false,
        "a genuine backend failure must not report success: {bash_resp}"
    );
    assert!(
        bash_resp["error"]
            .as_str()
            .unwrap_or_default()
            .contains("Working directory does not exist"),
        "the real backend error message must surface verbatim: {bash_resp}"
    );
    assert!(
        bash_resp["data"].is_null(),
        "a failed bash call carries no data payload: {bash_resp}"
    );

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
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

    let lines = parse_lines(&out);
    let bash = lines
        .iter()
        .find(|l| l["id"] == "b1")
        .expect("bash response");
    let abort = lines
        .iter()
        .find(|l| l["id"] == "ab")
        .expect("abort_bash response");
    assert_eq!(abort["command"], "abort_bash");
    assert_eq!(
        abort["success"], true,
        "abort_bash must be acknowledged: {abort}"
    );

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
        if let cyrup_ext::HostEvent::UserBash {
            command,
            exclude_from_context,
            cwd,
        } = ev
        {
            self.seen
                .lock()
                .unwrap()
                .push((command.clone(), *exclude_from_context, cwd.clone()));
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
    let probe = Arc::new(RpcUserBashProbe {
        seen: seen.clone(),
        override_result: None,
    });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo rpc-hello","excludeFromContext":true}"#,
        "\n",
    );
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

    let delivered = seen.lock().unwrap().clone();
    assert_eq!(
        delivered.len(),
        1,
        "the extension must RECEIVE exactly one user_bash event from the RPC bash command: \
         {delivered:?}"
    );
    assert_eq!(
        delivered[0].0, "echo rpc-hello",
        "the live command crosses the seam"
    );
    assert!(
        delivered[0].1,
        "the RPC excludeFromContext flag crosses the seam"
    );
    assert_eq!(
        delivered[0].2,
        fx.cwd.display().to_string(),
        "the session cwd crosses the seam"
    );

    // With no override, the command still really ran. Select the RESPONSE, not the
    // `bash_execution_update` event that shares the request id (DRIFT-006).
    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(
        resp["success"], true,
        "an un-overridden RPC bash still executes: {resp}"
    );
    assert!(
        resp["data"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("rpc-hello"),
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

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#,
        "\n"
    );
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

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
    assert_eq!(
        resp["success"], true,
        "an overridden bash is a success response: {resp}"
    );
    assert_eq!(
        resp["data"]["output"], "handled-by-extension",
        "the extension's result override is returned verbatim: {resp}"
    );
    assert!(
        !resp["data"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("locally-executed"),
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

/// DRIFT-004 (c), RECONCILED AGAINST THE v0.87.1 PIN: a PARTIAL `result` override does not
/// short-circuit — it ABORTS. This test used to assert the opposite, on the strength of the v0.84.4
/// pin, where `emitUserBash` really was `if (handlerResult) { return handlerResult as
/// UserBashEventResult; }` (`core/extensions/runner.ts:1005-1032` @v0.84.4, the return at
/// `:1015-1017`) — no predicate at all, and a `catch` that only reported. That is the code the old
/// doc's "Pi is TypeScript with no runtime type enforcement" reasoning described.
///
/// coding-agent 0.86.0 (*Breaking*, #9068: "`user_bash` now fails closed: errors or invalid defined
/// results abort the command without invoking later handlers or executing locally") replaced it with
/// `isUserBashEventResult` (`runner.ts:136-158` @v0.87.1) plus a `throw` (`runner.ts:1162-1166`),
/// and that predicate requires a `result` to carry a string `output`, a PRESENT `exitCode`, and BOTH
/// `cancelled` and `truncated` as booleans (`runner.ts:150-157`). `{output, exitCode}` satisfies two
/// of those four, so upstream REJECTS it — and pins that very shape itself:
/// `test/extensions-runner.test.ts:656` lists `["an incomplete result", '{ result: { output:
/// "handled" } }']` among the results that must `rejects.toThrow("Invalid user_bash handler
/// result")` (`:650-681`).
///
/// The SAFETY property the old test existed to protect is unchanged and still asserted below: a
/// sandbox or remote-exec extension's command must NEVER fall through onto the local shell. Pi's
/// fail-closed is the stronger of the two ways to get that — the half-built result becomes a visible
/// RPC `error` instead of being silently downgraded into a local run (EXT-077). `BashResult`'s
/// `#[serde(default)]` fields stay as they are; they are no longer what decides the question,
/// `cyrup_ext::is_user_bash_event_result` is, one layer above them.
#[tokio::test]
async fn rpc_bash_fails_closed_on_a_partial_user_bash_result_override() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let probe = Arc::new(RpcUserBashProbe {
        seen: seen.clone(),
        // Only the two fields a sandbox extension would realistically know — and NOT a valid
        // `UserBashEventResult`, because `cancelled`/`truncated` are absent.
        override_result: Some(serde_json::json!({
            "output": "sandboxed-elsewhere",
            "exitCode": 0,
        })),
    });
    let runtime = build_runtime_with_ext(&fx, faux, probe).await;
    let session = runtime.session().await;

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#,
        "\n"
    );
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

    assert_eq!(seen.lock().unwrap().len(), 1, "the handler was consulted");

    let lines = parse_lines(&out);
    assert!(
        !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
        "an aborted bash streams no execution deltas — nothing ran: {lines:?}"
    );
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(
        resp["success"], false,
        "an invalid override is an RPC error, not a success: {resp}"
    );
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or_default()
            .contains(cyrup_ext::INVALID_USER_BASH_RESULT_MESSAGE),
        "the abort carries pi's own `Invalid user_bash handler result: …` sentence: {resp}"
    );
    assert!(
        resp["data"].is_null(),
        "an aborted bash carries no data payload: {resp}"
    );

    // The property the pre-#9068 version of this test was really defending: the command the
    // extension meant to service elsewhere must not have run here instead.
    for line in &lines {
        for field in [&line["data"]["output"], &line["delta"]] {
            assert!(
                !field
                    .as_str()
                    .unwrap_or_default()
                    .contains("locally-executed"),
                "the command must NOT have reached the local shell: {lines:?}"
            );
        }
    }

    let msgs = session.agent_messages().await;
    let msgs_json = serde_json::to_value(&msgs).expect("agent messages serialize");
    assert!(
        !msgs_json.to_string().contains("bashExecution"),
        "an aborted bash is never recorded into transcript history: {msgs_json}"
    );
    assert!(
        !msgs_json.to_string().contains("sandboxed-elsewhere"),
        "a rejected override must not be recorded either: {msgs_json}"
    );
}

// ----------------------------------------------------------------------------------------------
// SEAM-015 — the JSON-RPC `bash` command must honour the `operations` backend the `user_bash`
// handler supplied, not just its `result`.
//
// Pi `rpc-mode.ts:578-582` @v0.87.1: after the `result` short-circuit (`:571-576`) it calls
// `session.executeBash(command.command, undefined, {excludeFromContext, id,
// operations: eventResult?.operations})` — `operations` at `:581`, the sibling half of
// `UserBashEventResult` (`core/extensions/types.ts:1229-1239` @v0.87.1). cyrup fills the same
// field one frame lower, inside the shared `execute_bash_with_user_event` wrapper, so this arm's
// literal `operations: None` is upstream's absent CALLER-supplied backend rather than a dropped
// extension-supplied one.
// ----------------------------------------------------------------------------------------------

/// The extension-supplied remote-exec backend: it never touches a shell, it just answers with a
/// sentinel the local shell could not possibly produce for the command the test sends.
struct SentinelBashOps {
    /// The commands it was handed, so the test can assert it really executed rather than inferring
    /// it from the output alone.
    seen: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl cyrup_tools::ops::BashOperations for SentinelBashOps {
    async fn exec(
        &self,
        command: &str,
        _cwd: &std::path::Path,
        opts: cyrup_tools::ops::BashExecOptions<'_>,
    ) -> Result<cyrup_tools::ExitStatus, cyrup_tools::ToolError> {
        self.seen.lock().unwrap().push(command.to_string());
        (opts.on_data)(b"ran-on-extension-backend\n");
        Ok(cyrup_tools::ExitStatus::Exited(0))
    }
}

/// A native extension that wins the `user_bash` reduction with a payload carrying no `result`, and
/// supplies [`SentinelBashOps`] as Pi's `UserBashEventResult.operations`.
struct RpcBashOpsSupplier {
    ops: Arc<SentinelBashOps>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for RpcBashOpsSupplier {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("rpc-bash-ops-supplier")
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
        if matches!(ev, cyrup_ext::HostEvent::UserBash { .. }) {
            // The FIRST DEFINED `UserBashEventResult` ends the handler loop and becomes THE result
            // (`core/extensions/runner.ts:1159-1168` @v0.87.1); one carrying only `operations` does
            // not short-circuit execution, it redirects it (`rpc-mode.ts:578-582`).
            //
            // The `operations` KEY is load-bearing, not decoration: `isUserBashEventResult` demands
            // EXACTLY ONE of `operations`/`result` and rejects a payload with NEITHER
            // (`runner.ts:140-142`) — upstream pins the bare `{}` as fail-closed itself
            // (`test/extensions-runner.test.ts:652`). Pi puts the callable `exec` behind that key;
            // ADR-0002 forbids a callable crossing cyrup's value-typed seam, so the key carries an
            // empty object and the backend itself arrives through `user_bash_operations` below. That
            // split is the CYRUP-DELTA `cyrup_ext::is_user_bash_event_result` documents, and it is
            // mechanism-only: an owner that declares NO backend is pi's `{ operations: {} }` with no
            // callable `exec` (`test/extensions-runner.test.ts:654`) and aborts just the same,
            // rather than falling through onto the local shell.
            return cyrup_ext::HookOutcome::Handled(cyrup_ext::HandledValue(
                serde_json::json!({ "operations": {} }),
            ));
        }
        cyrup_ext::HookOutcome::Noop
    }
    fn user_bash_operations(
        &self,
        _command: &str,
        _exclude_from_context: bool,
        _cwd: &str,
    ) -> Option<Arc<dyn cyrup_tools::ops::BashOperations>> {
        Some(self.ops.clone() as Arc<dyn cyrup_tools::ops::BashOperations>)
    }
}

/// SEAM-015: a wire `{"type":"bash"}` command runs on the backend the `user_bash` handler supplied.
/// The command sent would print `locally-executed` if it reached the local shell, and the backend
/// answers `ran-on-extension-backend`, so neither the presence nor the absence assertion can pass by
/// accident — and `rpc_bash_delivers_user_bash_to_an_extension` above is the control proving this
/// same wire command DOES reach the local shell when no handler supplies a backend.
#[tokio::test]
async fn rpc_bash_runs_on_an_extension_supplied_operations_backend() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let ops = Arc::new(SentinelBashOps {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let ext = Arc::new(RpcBashOpsSupplier { ops: ops.clone() });
    let runtime = build_runtime_with_ext(&fx, faux, ext).await;

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#,
        "\n"
    );
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

    let executed = ops.seen.lock().unwrap().clone();
    assert_eq!(
        executed,
        vec!["echo locally-executed".to_string()],
        "the extension's backend must have executed the wire command: {executed:?}"
    );

    let lines = parse_lines(&out);
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response");
    assert_eq!(resp["success"], true, "{resp}");
    assert!(
        resp["data"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("ran-on-extension-backend"),
        "the RPC result is the extension backend's output: {resp}"
    );
    assert!(
        !resp["data"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("locally-executed"),
        "the local shell must never have run the command: {resp}"
    );
    // The streaming half is shared with the local branch (pi builds the `onChunk` wrapper once and
    // hands it to whichever backend the `??` chose, `agent-session.ts:2779-2789`), so the
    // redirected command still emits its `bash_execution_update` deltas keyed by the request id.
    assert!(
        lines.iter().any(|l| type_of(l) == "bash_execution_update"
            && l["id"] == "b1"
            && l["delta"]
                .as_str()
                .unwrap_or_default()
                .contains("ran-on-extension-backend")),
        "a redirected bash still streams its output over the same event: {lines:?}"
    );
}

// ----------------------------------------------------------------------------------------------
// The whole `user_bash` result contract, end to end over the wire (EXT-077 / coding-agent #9068).
//
// Pi decides it in two places and nothing else touches it:
//
//   * `emitUserBash` (`core/extensions/runner.ts:1154-1183` @v0.87.1) — `handlerResult ===
//     undefined` CONTINUEs to the next handler (`:1160`) and, if every handler does that, the method
//     returns `undefined` (`:1182`) and the command runs locally. Any other value is checked by
//     `isUserBashEventResult` and, if it fails, becomes
//     `throw new Error("Invalid user_bash handler result: …")` (`:1162-1166`) — reported to the error
//     listeners with `event: "user_bash"` and RE-THROWN (`:1168-1178`), so no later handler runs and
//     nothing executes.
//   * `isUserBashEventResult` (`runner.ts:136-158`) — a non-null object carrying EXACTLY ONE of
//     `operations` / `result` (`:140-142`, so both `{}` and `{operations, result}` are rejected);
//     `operations` must be a non-null object with a CALLABLE `exec` (`:143-146`); a `result` must
//     have a string `output`, a PRESENT `exitCode` that is `undefined`-or-number, and BOTH
//     `cancelled` and `truncated` as booleans, with `fullOutputPath` absent-or-string (`:148-157`).
//
// Upstream pins both halves directly: the fail-closed table at
// `test/extensions-runner.test.ts:650-681` (`{}`, `{operations: null}`, `{operations: {}}`,
// `{result: null}`, `{result: {output}}`, and both keys at once) and the accept case at `:682-704`
// (`{operations: {exec}}` returned as-is, `{result: {output, exitCode, cancelled, truncated}}`
// resolved verbatim).
//
// `cyrup-ext` pins the PREDICATE unit-wise already
// (`tests::ext_fail_closed::is_user_bash_event_result_matches_pis_predicate`). What is pinned HERE is
// that the RPC `bash` front-end — the surface EXT-077's lane never ran, which is how two of these
// rows regressed unnoticed — routes each verdict to the right place: invalid to an RPC `error` with
// nothing executed and nothing recorded, a valid `{result}` to the override verbatim with nothing
// executed, and a valid `{operations}` to the extension's own backend instead of the local shell.
// ----------------------------------------------------------------------------------------------

/// A native extension that answers every `user_bash` event with ONE payload, verbatim, and
/// optionally declares [`SentinelBashOps`] as its backend — cyrup's stand-in for the callable
/// `operations.exec` ADR-0002 keeps off the value seam.
struct RawUserBashProbe {
    payload: Value,
    ops: Option<Arc<SentinelBashOps>>,
}

#[async_trait::async_trait]
impl cyrup_ext::NativeExtension for RawUserBashProbe {
    fn id(&self) -> cyrup_core::ExtensionId {
        cyrup_core::ExtensionId::from("raw-user-bash-probe")
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
        if matches!(ev, cyrup_ext::HostEvent::UserBash { .. }) {
            return cyrup_ext::HookOutcome::Handled(cyrup_ext::HandledValue(self.payload.clone()));
        }
        cyrup_ext::HookOutcome::Noop
    }
    fn user_bash_operations(
        &self,
        _command: &str,
        _exclude_from_context: bool,
        _cwd: &str,
    ) -> Option<Arc<dyn cyrup_tools::ops::BashOperations>> {
        self.ops
            .clone()
            .map(|o| o as Arc<dyn cyrup_tools::ops::BashOperations>)
    }
}

/// One wire `bash` command, answered by a `user_bash` handler returning `payload` verbatim and
/// declaring a backend only when `with_backend`. Returns the RPC lines, the commands the backend was
/// handed, and the serialized transcript.
async fn drive_raw_user_bash(
    payload: Value,
    with_backend: bool,
) -> (Vec<Value>, Vec<String>, String) {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let ops = Arc::new(SentinelBashOps {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    let ext: Arc<dyn cyrup_ext::NativeExtension> = Arc::new(RawUserBashProbe {
        payload,
        ops: if with_backend {
            Some(ops.clone())
        } else {
            None
        },
    });
    let runtime = build_runtime_with_ext(&fx, faux, ext).await;
    let session = runtime.session().await;

    let input = concat!(
        r#"{"type":"bash","id":"b1","command":"echo locally-executed"}"#,
        "\n"
    );
    // SEAM-154: closing the input aborts a running command, so the client waits for its answer.
    let out = run_rpc_until_bytes(&runtime, input, |lines| has_response(lines, "b1")).await;

    let msgs = session.agent_messages().await;
    let transcript = serde_json::to_value(&msgs)
        .expect("agent messages serialize")
        .to_string();
    let executed = ops.seen.lock().unwrap().clone();
    (parse_lines(&out), executed, transcript)
}

/// Every shape pi's fail-closed table rejects becomes an RPC `error` carrying pi's own sentence, with
/// the local shell never reached, the extension's backend never reached, and nothing recorded.
///
/// Two rows of this table are cyrup's ADR-0002 mechanism split rather than a literal JSON copy of
/// upstream's: `{operations: {}}` with no declared backend is pi's `["operations without exec"]`
/// (`test/extensions-runner.test.ts:654`) — the callable is missing, so it must abort — and
/// `{operations: true}` is its `typeof operations !== "object"` reject (`runner.ts:143-144`).
#[tokio::test]
async fn rpc_bash_fails_closed_on_every_invalid_user_bash_result() {
    let cases: Vec<(&str, Value, bool)> = vec![
        // ("an empty object", extensions-runner.test.ts:652) — NEITHER key.
        ("an empty object", serde_json::json!({}), false),
        // ("null operations", :653)
        (
            "null operations",
            serde_json::json!({"operations": null}),
            false,
        ),
        // `typeof operations !== "object"` (runner.ts:143-144).
        (
            "non-object operations",
            serde_json::json!({"operations": true}),
            false,
        ),
        // ("operations without exec", :654) over cyrup's seam: well-formed key, NO backend declared.
        (
            "an operations key whose owner declared no backend",
            serde_json::json!({"operations": {}}),
            false,
        ),
        // ("a null result", :655)
        ("a null result", serde_json::json!({"result": null}), false),
        // ("an incomplete result", :656)
        (
            "an incomplete result",
            serde_json::json!({"result": {"output": "handled"}}),
            false,
        ),
        // The same, one field further along — still missing `cancelled`/`truncated`.
        (
            "a result with only output and exitCode",
            serde_json::json!({"result": {"output": "handled", "exitCode": 0}}),
            false,
        ),
        // ("operations and a result", :657-660) — EXCLUSIVE upstream, even with both halves valid,
        // and even with a backend really declared behind the `operations` key.
        (
            "operations and a result",
            serde_json::json!({
                "operations": {},
                "result": {
                    "output": "handled", "exitCode": 0, "cancelled": false, "truncated": false,
                },
            }),
            true,
        ),
    ];

    for (what, payload, with_backend) in cases {
        let (lines, executed, transcript) = drive_raw_user_bash(payload, with_backend).await;

        let resp = lines
            .iter()
            .find(|l| l["command"] == "bash" && l["id"] == "b1")
            .unwrap_or_else(|| panic!("bash response for {what}: {lines:?}"));
        assert_eq!(
            resp["success"], false,
            "{what} must fail closed, not succeed: {resp}"
        );
        assert!(
            resp["error"]
                .as_str()
                .unwrap_or_default()
                .contains(cyrup_ext::INVALID_USER_BASH_RESULT_MESSAGE),
            "{what} must carry pi's own rejection sentence: {resp}"
        );
        assert!(
            !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
            "{what} must not stream execution deltas: {lines:?}"
        );
        for line in &lines {
            for field in [&line["data"]["output"], &line["delta"]] {
                assert!(
                    !field
                        .as_str()
                        .unwrap_or_default()
                        .contains("locally-executed"),
                    "{what} must not reach the local shell: {lines:?}"
                );
            }
        }
        assert!(
            executed.is_empty(),
            "{what} must not reach the extension's backend either: {executed:?}"
        );
        assert!(
            !transcript.contains("bashExecution"),
            "{what} must record nothing into transcript history: {transcript}"
        );
    }
}

/// The other side of the same predicate: the two shapes pi ACCEPTS
/// (`test/extensions-runner.test.ts:682-704`) are honoured, each to its own destination — so the
/// fail-closed above is a faithful port and not a blanket refusal.
#[tokio::test]
async fn rpc_bash_honors_both_valid_user_bash_result_shapes() {
    // `{ result: { output, exitCode, cancelled, truncated } }` — `rpc-mode.ts:571-576` records it and
    // answers with it; execution never happens.
    let (lines, executed, transcript) = drive_raw_user_bash(
        serde_json::json!({"result": {
            "output": "handled-by-extension", "exitCode": 0, "cancelled": false, "truncated": false,
        }}),
        // A backend is declared and must still be IGNORED: pi tests `eventResult?.result` first and
        // returns before it ever looks at `operations` (`rpc-mode.ts:571-576`).
        true,
    )
    .await;
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response for a valid result override");
    assert_eq!(
        resp["success"], true,
        "a valid result override wins: {resp}"
    );
    assert_eq!(
        resp["data"]["output"], "handled-by-extension",
        "the override is returned verbatim: {resp}"
    );
    assert!(
        executed.is_empty(),
        "a `result` short-circuits before `operations` is ever read: {executed:?}"
    );
    assert!(
        !lines.iter().any(|l| type_of(l) == "bash_execution_update"),
        "a serviced bash streams nothing: {lines:?}"
    );
    assert!(
        transcript.contains("handled-by-extension"),
        "the override is recorded (`recordBashResult`, rpc-mode.ts:572-575): {transcript}"
    );

    // `{ operations }` — `rpc-mode.ts:578-582` runs the command THROUGH it rather than instead of it.
    let (lines, executed, _transcript) =
        drive_raw_user_bash(serde_json::json!({"operations": {}}), true).await;
    let resp = lines
        .iter()
        .find(|l| l["command"] == "bash" && l["id"] == "b1")
        .expect("bash response for an operations override");
    assert_eq!(
        resp["success"], true,
        "an operations override redirects rather than fails: {resp}"
    );
    assert_eq!(
        executed,
        vec!["echo locally-executed".to_string()],
        "the extension's backend ran the command: {executed:?}"
    );
    assert!(
        resp["data"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("ran-on-extension-backend"),
        "the response is the backend's output, not the local shell's: {resp}"
    );
    assert!(
        !resp["data"]["output"]
            .as_str()
            .unwrap_or_default()
            .contains("locally-executed"),
        "the local shell never ran it: {resp}"
    );
}
