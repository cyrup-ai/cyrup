//! PRINT mode (R-11-005): one prompt run to completion, the FINAL assistant message on stdout, a
//! failed or aborted turn's reason on stderr, and pi's zero-or-one exit code — asserted on the
//! bytes `run_print` writes into an in-memory sink.

use std::sync::Arc;

use super::support::{build_runtime, fixture};
use crate::{PrintOptions, run_print};
use cyrup_core::StopReason;
use cyrup_provider::faux::{
    FauxMessageOptions, FauxProvider, faux_assistant_message, faux_assistant_message_with,
    faux_text,
};
use cyrup_session_svc::{InputSource, UserInput};

#[tokio::test]
async fn print_mode_emits_final_assistant_text() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("the final answer")],
        StopReason::Stop,
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [UserInput::text("what is the answer?", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let text = String::from_utf8(out).unwrap();
    assert!(
        text.contains("the final answer"),
        "final assistant text missing:\n{text}"
    );
    assert!(
        String::from_utf8(err).unwrap().is_empty(),
        "a clean turn writes nothing to stderr"
    );
}

/// G3 — PRINT mode prints ONLY the final assistant message of a multi-message turn, exactly once,
/// never one line per intermediate message. Pi's send loop produces no output and the terminal
/// output block reads `state.messages[state.messages.length - 1]` outside the loop
/// (print-mode.ts:121-146). Pre-fix cyrup wrote the accumulated text on every call, so a two-message
/// turn produced BOTH `"first answer"` and `"second answer"`.
#[tokio::test]
async fn print_mode_prints_only_the_final_message_of_a_turn() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![
        faux_assistant_message(vec![faux_text("first answer")], StopReason::Stop),
        faux_assistant_message(vec![faux_text("second answer")], StopReason::Stop),
    ]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [
            UserInput::text("q1", InputSource::Cli),
            UserInput::text("q2", InputSource::Cli),
        ],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let text = String::from_utf8(out).unwrap();
    assert_eq!(
        text, "second answer\n",
        "only the FINAL message prints, exactly once (G3): {text:?}"
    );
    assert!(
        !text.contains("first answer"),
        "an intermediate message must NOT print (G3): {text:?}"
    );
    assert!(
        String::from_utf8(err).unwrap().is_empty(),
        "a clean turn writes nothing to stderr"
    );
}

/// G4 — a failed final turn: Pi writes `errorMessage` to stderr and suppresses the assistant stdout
/// (print-mode.ts:133-137). Pre-fix cyrup wrote the failed turn's partial text to stdout and never
/// touched stderr.
#[tokio::test]
async fn print_mode_routes_a_failed_turn_to_stderr_and_suppresses_stdout() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message_with(
        vec![faux_text("partial garbled output")],
        StopReason::Error,
        FauxMessageOptions {
            error_message: Some("the model exploded".into()),
            ..Default::default()
        },
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [UserInput::text("q", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    let stdout = String::from_utf8(out).unwrap();
    let stderr = String::from_utf8(err).unwrap();
    assert!(
        stdout.is_empty(),
        "a failed turn suppresses assistant stdout (G4): {stdout:?}"
    );
    assert_eq!(
        stderr, "the model exploded\n",
        "the error message goes to stderr (G4): {stderr:?}"
    );
}

/// SEAM-016 — `run_print` returns pi's `exitCode`, decided inside the terminal output block from
/// the SAME `lastMessage` it prints (`print-mode.ts:139-148` @v0.84.1) and returned at `:151`.
///
/// Pins the three arms that used to diverge, all of which were computed elsewhere (`run.rs`'s
/// reverse transcript scan) rather than here:
/// * a clean `stop` keeps the `exitCode = 0` pi initialises at `:35`;
/// * `error` raises it to 1 (`:147`);
/// * **`aborted` raises it to 1 too** — pi's condition is `stopReason === "error" || stopReason ===
///   "aborted"` (`:145`), one branch, one assignment. cyrup answered **130** for this case, a code
///   pi never emits from print mode, so this assertion was RED before the change.
#[tokio::test]
async fn print_mode_exit_code_is_pis_zero_or_one_from_the_final_message() {
    for (reason, expected) in [
        (StopReason::Stop, 0),
        (StopReason::Error, 1),
        (StopReason::Aborted, 1),
    ] {
        let fx = fixture();
        let faux = Arc::new(FauxProvider::new());
        faux.set_responses(vec![faux_assistant_message(vec![faux_text("out")], reason)]);
        let runtime = build_runtime(&fx, faux).await;

        let mut out: Vec<u8> = Vec::new();
        let mut err: Vec<u8> = Vec::new();
        let code = run_print(
            &runtime,
            [UserInput::text("q", InputSource::Cli)],
            &mut out,
            &mut err,
            PrintOptions::default(),
        )
        .await
        .expect("print mode runs");
        assert_eq!(code, expected, "pi's exitCode for stop reason {reason:?}");
    }
}

/// G4 — an aborted final turn with NO `error_message` falls back to Pi's `Request ${stopReason}`
/// string on stderr, still suppressing stdout (print-mode.ts:136, the `|| ` branch).
#[tokio::test]
async fn print_mode_aborted_turn_without_message_uses_the_request_reason_fallback() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("half-written")],
        StopReason::Aborted,
    )]);
    let runtime = build_runtime(&fx, faux).await;

    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    run_print(
        &runtime,
        [UserInput::text("q", InputSource::Cli)],
        &mut out,
        &mut err,
        PrintOptions::default(),
    )
    .await
    .expect("print mode runs");

    assert!(
        String::from_utf8(out).unwrap().is_empty(),
        "aborted turn suppresses stdout (G4)"
    );
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "Request aborted\n",
        "an aborted turn without an error_message falls back to `Request aborted` (G4)"
    );
}

// ---------------------------------------------------------------------------------------------
// SEAM-137 — a prompt an extension command services starts no run, so print mode must not wait
// for one.
// ---------------------------------------------------------------------------------------------

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::support::build_runtime_with_ext;
use cyrup_core::ExtensionId;
use cyrup_ext::{
    CommandDescriptor, ExtError, HookOutcome, HostCtx, HostEvent, InitApi, NativeExtension,
};

/// How long a print run may take before the test calls it a hang. The runs here are in-memory and
/// finish in milliseconds; the pre-fix loop never finished at all.
const HANG_DEADLINE: Duration = Duration::from_secs(20);

/// A native extension whose `/ping` command handles the submission and starts no run (pi
/// `_tryExecuteExtensionCommand`, agent-session.ts:1004-1013, which returns before any prompt is
/// sent). `calls` counts how often the handler ran.
struct PingExt {
    calls: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl NativeExtension for PingExt {
    fn id(&self) -> ExtensionId {
        ExtensionId::from("ping-ext")
    }

    async fn init(&self, api: &mut InitApi) -> Result<(), ExtError> {
        api.register_command(
            "ping",
            CommandDescriptor {
                description: "handled by the extension, starts no run".into(),
                completions: Vec::new(),
            },
        );
        Ok(())
    }

    async fn on_event(&self, _ev: &HostEvent, _ctx: &HostCtx) -> HookOutcome {
        HookOutcome::Noop
    }

    async fn execute_command(
        &self,
        _name: &str,
        _args: &str,
        _ctx: &HostCtx,
    ) -> Result<Option<String>, ExtError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Some(String::new()))
    }
}

/// Drive `run_print` under [`HANG_DEADLINE`], returning its exit code and the bytes it wrote.
async fn run_print_bounded(
    runtime: &cyrup_session_svc::AgentSessionRuntime,
    messages: Vec<UserInput>,
) -> (i32, String, String) {
    let mut out: Vec<u8> = Vec::new();
    let mut err: Vec<u8> = Vec::new();
    let code = tokio::time::timeout(
        HANG_DEADLINE,
        run_print(
            runtime,
            messages,
            &mut out,
            &mut err,
            PrintOptions::default(),
        ),
    )
    .await
    .expect("print mode hung: a prompt an extension command handled never settles a run")
    .expect("print mode runs");
    (
        code,
        String::from_utf8(out).expect("utf8 stdout"),
        String::from_utf8(err).expect("utf8 stderr"),
    )
}

/// `cyrup -p "/ping"`: the command runs, nothing is printed, the exit code is pi's 0, and the mode
/// RETURNS. Pi's `await session.prompt(...)` resolves as soon as the command's handler has
/// (print-mode.ts:121-127); cyrup used to drain the run-scoped stream, which for a handled prompt
/// is never closed — so `cyrup -p "/mcp"` and `cyrup -p "/llama"` ran until killed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_prompt_an_extension_command_handles_does_not_hang_print_mode() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = build_runtime_with_ext(
        &fx,
        faux,
        Arc::new(PingExt {
            calls: Arc::clone(&calls),
        }),
    )
    .await;

    let (code, out, err) =
        run_print_bounded(&runtime, vec![UserInput::text("/ping", InputSource::Cli)]).await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the command handler ran once"
    );
    assert_eq!(
        code, 0,
        "a handled command is not a failed turn (print-mode.ts:35)"
    );
    assert!(out.is_empty(), "a handled command prints nothing:\n{out}");
    assert!(
        err.is_empty(),
        "a handled command writes nothing to stderr:\n{err}"
    );
}

/// The send loop CONTINUES after a handled prompt: a command followed by an ordinary prompt still
/// runs that prompt to completion and prints its final assistant message. This pins that the fix
/// skips only the settle wait, not the rest of the loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_send_loop_continues_after_a_handled_prompt() {
    let fx = fixture();
    let faux = Arc::new(FauxProvider::new());
    faux.set_responses(vec![faux_assistant_message(
        vec![faux_text("answer after the command")],
        StopReason::Stop,
    )]);
    let calls = Arc::new(AtomicUsize::new(0));
    let runtime = build_runtime_with_ext(
        &fx,
        faux,
        Arc::new(PingExt {
            calls: Arc::clone(&calls),
        }),
    )
    .await;

    let (code, out, _err) = run_print_bounded(
        &runtime,
        vec![
            UserInput::text("/ping", InputSource::Cli),
            UserInput::text("what now?", InputSource::Cli),
        ],
    )
    .await;

    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the command handler ran once"
    );
    assert_eq!(code, 0);
    assert!(
        out.contains("answer after the command"),
        "the prompt after the command must still run and print:\n{out}"
    );
}
