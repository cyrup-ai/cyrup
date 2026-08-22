//! PRINT mode (R-11-005): one prompt run to completion, the FINAL assistant message on stdout, a
//! failed or aborted turn's reason on stderr, and pi's zero-or-one exit code — asserted on the
//! bytes `run_print` writes into an in-memory sink.

use std::sync::Arc;

use super::support::{build_runtime, fixture};
use crate::{run_print, PrintOptions};
use cyrup_core::StopReason;
use cyrup_provider::faux::{
    faux_assistant_message, faux_assistant_message_with, faux_text, FauxMessageOptions,
    FauxProvider,
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
    assert!(text.contains("the final answer"), "final assistant text missing:\n{text}");
    assert!(String::from_utf8(err).unwrap().is_empty(), "a clean turn writes nothing to stderr");
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
    assert_eq!(text, "second answer\n", "only the FINAL message prints, exactly once (G3): {text:?}");
    assert!(!text.contains("first answer"), "an intermediate message must NOT print (G3): {text:?}");
    assert!(String::from_utf8(err).unwrap().is_empty(), "a clean turn writes nothing to stderr");
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
    assert!(stdout.is_empty(), "a failed turn suppresses assistant stdout (G4): {stdout:?}");
    assert_eq!(stderr, "the model exploded\n", "the error message goes to stderr (G4): {stderr:?}");
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

    assert!(String::from_utf8(out).unwrap().is_empty(), "aborted turn suppresses stdout (G4)");
    assert_eq!(
        String::from_utf8(err).unwrap(),
        "Request aborted\n",
        "an aborted turn without an error_message falls back to `Request aborted` (G4)"
    );
}
