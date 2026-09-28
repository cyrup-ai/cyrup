//! EXT-077 — the `user_bash` seam must fail CLOSED **on the production path**.
//!
//! coding-agent 0.86.0, *Breaking Changes* (#9068): "`user_bash` now fails closed: errors or invalid
//! defined results abort the command without invoking later handlers or executing locally."
//! `extensions/runner.ts::emitUserBash` @v0.87.1 reports each fault via `emitError` and then
//! **re-throws** (`throw err;`, `:1177`), and throws
//! `"Invalid user_bash handler result: …"` (`:1163-1167`) for a defined value that fails
//! `isUserBashEventResult` (`:136-158`). Neither caller falls back: `interactive-mode.ts`'s
//! `handleBashCommand` wraps the emit in `try { … } catch { return; }` with the comment "The
//! extension runner already reported the error. Do not fall back to local execution."
//! (`:6735-6743`), and `rpc-mode.ts`'s `case "bash":` has no catch at all, so the throw becomes an
//! error response.
//!
//! Every test here drives [`crate::AgentSession::execute_bash_with_user_event`] — the shared wrapper
//! that BOTH the interactive `!`/`!!` handler (`cyrup-tui/src/app/bash_spawn.rs`) and the JSON-RPC
//! `bash` command (`cyrup-modes/src/rpc/mod.rs`) go through — and each one proves the abort by the
//! only thing that actually matters: a **side effect on the host that must not happen**. The command
//! under test writes a sentinel file, so "the command did not execute locally" is `!sentinel.exists()`
//! rather than a claim about a return value.
//!
//! `cyrup-ext`'s own `ext_fail_closed.rs` / `payload_and_seam_parity.rs` cover the reducer in
//! isolation; these are the proofs that the reducer is the one production reaches.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use cyrup_core::{ExtensionId, StopReason};
use cyrup_ext::{
    EventKind, ExtError, HandledValue, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};
use cyrup_provider::Provider;
use cyrup_provider::faux::{FauxProvider, faux_assistant_message, faux_text};
use serde_json::{Value, json};
use tempfile::TempDir;

use crate::{BashOptions, SessionBuilder, SessionConfig};

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
    Fixture {
        _tmp: tmp,
        cwd,
        agent_dir,
    }
}

fn base_config(fx: &Fixture) -> SessionConfig {
    let mut cfg = SessionConfig::new(fx.cwd.clone(), fx.agent_dir.clone());
    cfg.trust_override = Some(true);
    cfg
}

fn faux_ok() -> Arc<FauxProvider> {
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("ok")],
        StopReason::Stop,
    )]);
    faux
}

/// The command every test runs: it writes `sentinel` if — and only if — it reaches a real shell.
fn sentinel_command(sentinel: &Path) -> String {
    format!("echo ran > {}", sentinel.display())
}

/// What a `user_bash` handler does when it is invoked.
enum Behaviour {
    /// A contained fault: pi's handler `throw`, which `emitUserBash` reports and re-throws.
    Panic,
    /// A defined result handed back verbatim.
    Handled(Value),
    /// pi's `undefined`: not this handler's command.
    Undefined,
}

struct UserBash(Behaviour);

#[async_trait::async_trait]
impl NativeExtension for UserBash {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("user-bash-guard")
    }
    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.subscribe(&[EventKind::UserBash]);
        Ok(())
    }
    async fn on_event(&self, ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        if !matches!(ev, HostEvent::UserBash { .. }) {
            return HookOutcome::Noop;
        }
        match &self.0 {
            Behaviour::Panic => panic!("the routing backend is unreachable"),
            Behaviour::Handled(v) => HookOutcome::Handled(HandledValue(v.clone())),
            Behaviour::Undefined => HookOutcome::Noop,
        }
    }
    // NOTE: no `user_bash_operations` override — this extension declares NO bash-operations
    // backend, which is the tier-resolution half of pi's `typeof operations.exec === "function"`.
}

/// Run the sentinel command through the production wrapper with one `user_bash` extension loaded.
/// Returns `Ok(())` when the wrapper reported success, `Err(message)` with the rendered error
/// otherwise, plus whether the command actually touched the host.
async fn drive(behaviour: Behaviour) -> (Result<(), String>, bool) {
    let fx = fixture();
    let sentinel = fx.cwd.join("ran.txt");
    let session = SessionBuilder::new(faux_ok() as Arc<dyn Provider>, base_config(&fx))
        .with_native_extension(Arc::new(UserBash(behaviour)))
        .build()
        .await
        .expect("build");

    let outcome = session
        .execute_bash_with_user_event(
            &sentinel_command(&sentinel),
            BashOptions {
                exclude_from_context: false,
                id: None,
                operations: None,
            },
            None,
        )
        .await;

    let executed = sentinel.exists();
    (outcome.map(|_| ()).map_err(|e| e.to_string()), executed)
}

/// A FAULTING `user_bash` handler aborts the user's command — it does NOT run on the host.
///
/// `EventKind::fails_closed()` now covers `user_bash`, so the dispatcher answers a contained fault
/// with `Reduced::Blocked { reason: "Extension failed, blocking execution: …" }`. Before EXT-077's
/// reconciliation that `Blocked` reached `emit_user_bash_event`'s "not handled" arm, became
/// `UserBashOutcome::None`, and `execute_bash_with_user_event` ran the command locally anyway: an
/// extension that exists to route `!` commands into a container, onto a remote box or through a
/// policy gate had the command re-run on the host the moment it faulted. This is the proof that
/// stops.
#[tokio::test]
async fn a_faulting_user_bash_handler_aborts_the_command_instead_of_running_it_on_the_host() {
    let (outcome, executed) = drive(Behaviour::Panic).await;

    assert!(
        !executed,
        "EXT-077: a faulting user_bash handler must abort the command — pi runs NOTHING locally \
         (`interactive-mode.ts:6740-6743`), but the sentinel file exists, so the command ran on the \
         host behind the extension's back"
    );
    let err = outcome.expect_err("the aborted command must not report success");
    assert!(
        err.contains("Extension failed, blocking execution"),
        "the abort carries the dispatcher's fail-closed reason (pi `agent-session.ts:475-487`): {err}"
    );
}

/// A DEFINED-but-invalid `UserBashEventResult` aborts too, carrying pi's own sentence.
///
/// `{operations, result}` is the `hasOperations === hasResult` rejection (`runner.ts:140`). pi throws
/// `Invalid user_bash handler result: …`; cyrup's production reducer validates with the SAME
/// predicate (`cyrup_ext::is_user_bash_event_result`) and aborts. Before the reconciliation this
/// value was accepted: `serde_json::from_value::<BashResult>` on the empty `result` object
/// succeeded — every field is `#[serde(default)]` — so a handler that returned nonsense SERVICED
/// the command with an empty success.
#[tokio::test]
async fn a_defined_but_invalid_user_bash_result_aborts_the_command() {
    let (outcome, executed) = drive(Behaviour::Handled(
        json!({"operations": {}, "result": {"output": "", "exitCode": 0, "cancelled": false, "truncated": false}}),
    ))
    .await;

    assert!(
        !executed,
        "EXT-077: an invalid defined result aborts without executing locally (#9068)"
    );
    let err = outcome.expect_err("an invalid defined result must not report success");
    assert!(
        err.contains(cyrup_ext::INVALID_USER_BASH_RESULT_MESSAGE),
        "the abort carries pi's own `Invalid user_bash handler result: …` sentence verbatim \
         (`runner.ts:1163-1167` @v0.87.1): {err}"
    );
}

/// A `{operations}` result whose owner declared NO bash-operations backend aborts.
///
/// This is the other half of pi's `typeof operations.exec === "function"` (`runner.ts:145`), which
/// `is_user_bash_event_result` cannot check on the JSON side because a callable does not cross
/// cyrup's guest boundary (ADR-0002) — its CYRUP-DELTA says the check "fails at that later seam
/// rather than here", and this test is what makes that sentence true. The value is pi-valid on the
/// wire, so nothing earlier rejects it; the extension simply has no `exec`, and pi's predicate is
/// therefore false and `emitUserBash` throws. Running the command on the local shell — which is what
/// `user_bash_operations` returning `None` used to mean here — is the opposite of what a handler
/// asking to supply its OWN backend wanted.
#[tokio::test]
async fn an_operations_result_without_a_declared_backend_aborts_rather_than_running_locally() {
    let (outcome, executed) =
        drive(Behaviour::Handled(json!({"operations": {"kind": "ssh"}}))).await;

    assert!(
        !executed,
        "EXT-077: an `operations` half with no callable `exec` is an invalid result upstream, so \
         the command must not fall back to the local shell"
    );
    let err = outcome.expect_err("a backendless operations result must not report success");
    assert!(
        err.contains(cyrup_ext::INVALID_USER_BASH_RESULT_MESSAGE),
        "it is the same refusal pi's predicate produces: {err}"
    );
}

/// The fail-closed is not a blanket refusal: pi's `undefined` still falls through to local execution.
///
/// Presence-before-absence for the three tests above — if the reducer aborted everything they would
/// all pass vacuously.
#[tokio::test]
async fn an_undefined_user_bash_result_still_executes_the_command_locally() {
    let (outcome, executed) = drive(Behaviour::Undefined).await;

    assert!(
        outcome.is_ok(),
        "a handler that declined must leave the command alone: {outcome:?}"
    );
    assert!(
        executed,
        "pi's `undefined` means local execution (`emitUserBash` returns `undefined` and the caller \
         proceeds); the sentinel must exist"
    );
}
