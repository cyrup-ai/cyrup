//! G80 — the INHERITED half of `effectiveVerifyEnv`
//! (`pi-subagents/src/runs/shared/acceptance.ts:976-981` @v0.43.0).
//!
//! `effectiveVerifyEnv` is `{ ...process.env, ...(env ?? {}) }`, so the redaction set
//! (`verifyRedactionEnv`, `:983-987`) covers credentials this process INHERITED, not only ones the
//! verify command declared for itself. That is the case that matters most in practice: a real
//! orchestrator runs with `GITHUB_TOKEN`/`ANTHROPIC_API_KEY`/`AWS_SECRET_ACCESS_KEY` already in its
//! environment, hands them to the verify command's shell by inheritance, and a `curl -v` or a
//! failing test harness echoes them straight back into the acceptance ledger.
//!
//! **This lives in its own test binary on purpose.** Proving the inherited branch requires mutating
//! this process's environment, and `std::env::set_var` is `unsafe` in Rust 2024 precisely because
//! it races any concurrent reader of the environment — which is exactly what
//! `effective_verify_env` and every `Command::spawn` are. A test file is its own binary, so keeping
//! this the only test in it means there is no concurrent reader to race.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use cyrup_ext_subagents::exec::acceptance::{VerifyCommand, run_verify_commands_memoized};

#[tokio::test(flavor = "current_thread")]
async fn a_secret_inherited_from_the_process_environment_is_redacted_out_of_the_ledger() {
    let dir = tempfile::tempdir().expect("tempdir");

    // SAFETY: this test crate carries no `#![forbid(unsafe_code)]`, this is the only test in this
    // binary, and the runtime is single-threaded (`flavor = "current_thread"`), so no other thread
    // can be reading the environment while it is mutated.
    unsafe {
        std::env::set_var("G80_INHERITED_API_KEY", "inherited_secret_value");
    }

    // The command declares NO env of its own — the credential reaches the child purely by
    // inheritance, which is the branch under test.
    let command = VerifyCommand {
        id: "inherited".to_string(),
        command: "echo \"key=$G80_INHERITED_API_KEY\"".to_string(),
        timeout_ms: None,
        cwd: None,
        env: None,
        allow_failure: None,
    };
    let results =
        run_verify_commands_memoized(std::slice::from_ref(&command), dir.path(), None).await;

    // SAFETY: same critical section, still single-threaded.
    unsafe {
        std::env::remove_var("G80_INHERITED_API_KEY");
    }

    // Upstream keeps the two captured streams separate (`AcceptanceVerifyResult.stdout`/`.stderr`,
    // `shared/types.ts:741-742`); the retired lattice shape's single `output_tail` was cyrup's.
    let tail = format!(
        "{}{}",
        results[0].stdout.as_deref().unwrap_or_default(),
        results[0].stderr.as_deref().unwrap_or_default()
    );
    let tail = &tail;
    assert!(
        !tail.contains("inherited_secret_value"),
        "an INHERITED credential reached the acceptance ledger verbatim: {tail:?}"
    );
    assert!(
        tail.contains("key=[REDACTED]"),
        "the inherited credential must be masked in place: {tail:?}"
    );
}
