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
//! **This is its own `[[test]]` target on purpose, and that is load-bearing.** Proving the
//! INHERITED branch requires the secret to be in this process's environment: the verify command
//! declares no env of its own, and `effective_verify_env` feeds only the memo key — the child
//! actually receives the value through ordinary `Command` inheritance. Injecting a map would prove
//! the declared-env branch instead, which is a different (already covered) path, so neither R2
//! tier 1 nor tier 2 reaches this one.
//!
//! `std::env::set_var` is `unsafe` in Rust 2024 precisely because it races any concurrent reader,
//! and a single-test binary is what makes "there is no concurrent reader" TRUE rather than merely
//! asserted. It briefly was not: consolidation folded this file into `tests/subagents` alongside
//! ~195 other tests, silently voiding that argument under `cargo test` (nextest's process-per-test
//! still isolated it). Restoring the dedicated target restores the property the comment claims.
//! Do not fold it back in.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
// The ONE place in the workspace that may mutate the process environment, and the module doc above
// is the justification: this test's subject IS inheritance into a real child, so injecting the
// value would prove a different branch. The exemption is scoped to this single-test target — which
// is also what makes `unsafe { set_var }` sound here, since there is no concurrent reader.
#![allow(clippy::disallowed_methods)]

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

    // SAFETY: the same condition the mutation above states, and it still holds here — one test in
    // this binary on a `current_thread` runtime, so no other thread exists to race the write. The
    // criterion is the THREAD and it covers ANY key, not just this one.
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
