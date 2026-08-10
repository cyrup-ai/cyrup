//! G80 — verify-command workspace MEMOIZATION + secret REDACTION.
//!
//! Ports `pi-subagents/src/runs/shared/acceptance.ts` @v0.43.0:
//!
//! - **Redaction** (`:974-994`): `SENSITIVE_ENV_KEY_PATTERN` selects the environment entries whose
//!   KEY looks like a credential and whose VALUE is at least 4 long; every occurrence of every such
//!   VALUE is replaced with `[REDACTED]` in the captured `stdout`/`stderr` BEFORE they are trimmed
//!   (`trimOutput(redactVerifyEnv(stdout, command.env))`, `:1194-1195`, and the same on the
//!   `child.on("error")` arm, `:1203-1204`). This is a security boundary: a verify command's output
//!   is whatever a build/test/curl invocation printed while holding the orchestrator's full
//!   environment, and it lands verbatim in the acceptance ledger and from there in a transcript.
//!
//! - **Memoization** (`:1032-1132`): `runMemoizedVerifyCommand` keys a command's recorded result on
//!   the command text, its repo-relative cwd, its declared env key names, a hash of the whole
//!   effective environment, its timeout, its `allowFailure` flag, the repository `HEAD` and a hash
//!   of the full working-tree diff (`:1091-1101`), and stores it at
//!   `<artifactsDir>/acceptance/verify/<runId>/<cacheKey>.json` (`:1102`). A hit replays the result
//!   without spawning anything (`:1105-1107`); ANY edit to the tree changes `diffHash` and
//!   invalidates every memo. With no artifacts dir/run id, or outside a git working tree, it falls
//!   straight through to a real execution (`:1085-1087`).
//!
//! Every test here drives a LIVE entry point:
//!
//! - `exec::acceptance::evaluate_acceptance` — the enum-lattice gate `exec::run_sync` calls
//!   (`exec/mod.rs`, the `subagent` tool's single-run path and the background hop-2 runner), now
//!   carrying the memo context `RunOptions::artifacts_dir` + `RunOptions::run_id` produce.
//! - `exec::acceptance::run_verify_commands_memoized` — the runner that gate loops over.
//! - `exec::acceptance::model::run_memoized_verify_command` — the pi-shaped runner
//!   `model::evaluate_acceptance` loops over, which `spawn::chain_graph`'s dynamic-group gate calls.
//!
//! Every verify command below is a REAL `/bin/sh` subprocess; nothing here is mocked.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::Path;

use cyrup_ext_subagents::exec::acceptance::{
    AcceptanceContract, AcceptanceStatus, CleanCompletionGate, VerifyCommand, evaluate_acceptance,
    model, run_verify_commands_memoized,
};
use cyrup_ext_subagents::exec::completion_guard::CompletionMutationGuardResult;

// ------------------------------------------------------------------------------------------------
// Fixtures
// ------------------------------------------------------------------------------------------------

fn clean_gate() -> CleanCompletionGate {
    CleanCompletionGate {
        exit_code: 0,
        detached: false,
        interrupted: false,
        timed_out: false,
    }
}

fn guard_did_not_fire() -> CompletionMutationGuardResult {
    CompletionMutationGuardResult {
        expected_mutation: false,
        attempted_mutation: false,
        triggered: false,
    }
}

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn verify(id: &str, command: &str) -> VerifyCommand {
    VerifyCommand {
        id: id.to_string(),
        command: command.to_string(),
        timeout_ms: None,
        cwd: None,
        env: None,
        allow_failure: None,
    }
}

fn verify_with_env(id: &str, command: &str, env: &[(&str, &str)]) -> VerifyCommand {
    VerifyCommand {
        env: Some(env_of(env)),
        ..verify(id, command)
    }
}

/// A real git repository with one committed file, so `readVerifyWorkspaceState`
/// (`acceptance.ts:1046-1060`) can resolve `HEAD` and a diff.
fn init_repo(dir: &Path) {
    let run = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("git runs");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    run(&["init", "-q", "-b", "main"]);
    std::fs::write(dir.join("tracked.txt"), "one\n").expect("seed file");
    run(&["add", "tracked.txt"]);
    run(&["commit", "-q", "-m", "seed"]);
}

/// A verify command that appends one line to `marker` every time it ACTUALLY executes, then exits 0.
/// Counting the lines counts real subprocess executions, so a memo hit is directly observable.
fn counting_command(marker: &Path) -> String {
    format!("echo ran >> {}", shell_quote(marker))
}

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

fn execution_count(marker: &Path) -> usize {
    std::fs::read_to_string(marker)
        .map(|text| text.lines().count())
        .unwrap_or(0)
}

fn memo<'a>(artifacts_dir: &'a Path, run_id: &'a str) -> model::VerifyMemoContext<'a> {
    model::VerifyMemoContext {
        artifacts_dir,
        run_id,
    }
}

// ------------------------------------------------------------------------------------------------
// Redaction — the key pattern (acceptance.ts:974)
// ------------------------------------------------------------------------------------------------

#[test]
fn the_sensitive_key_pattern_matches_upstreams_words_at_underscore_boundaries_only() {
    // `/(?:^|_)(?:TOKEN|SECRET|PASSWORD|PASS|AUTH|CREDENTIAL|COOKIE|SESSION|PRIVATE|API_KEY
    // |ACCESS_KEY)(?:_|$)/i` (`acceptance.ts:974`).
    let matching = [
        "TOKEN",
        "GITHUB_TOKEN",
        "TOKEN_FILE",
        "token",                  // the `i` flag
        "MY_SECRET",
        "DB_PASSWORD",
        "SSH_PASS",
        "AUTH",
        "SERVICE_CREDENTIAL_X",
        "COOKIE_JAR",
        "SESSION_ID",
        "PRIVATE_KEY_PEM",        // via PRIVATE
        "OPENAI_API_KEY",
        "AWS_SECRET_ACCESS_KEY",
    ];
    for key in matching {
        let redacted = model::redact_verify_env("value=supersecretvalue", Some(&env_of(&[(key, "supersecretvalue")])));
        assert_eq!(
            redacted, "value=[REDACTED]",
            "`{key}` must be treated as sensitive (acceptance.ts:974)"
        );
    }

    let not_matching = [
        "TOKENIZER",     // TOKEN not followed by `_` or end
        "PASSAGE",       // PASS not followed by `_` or end
        "AUTHORITY",
        "SESSIONS",
        "PATH",
        "APIKEY",        // upstream requires the underscore: API_KEY
        "HOME",
        "CARGO_TARGET_DIR",
    ];
    for key in not_matching {
        let redacted = model::redact_verify_env("value=supersecretvalue", Some(&env_of(&[(key, "supersecretvalue")])));
        assert_eq!(
            redacted, "value=supersecretvalue",
            "`{key}` is NOT in upstream's pattern and must not blanket-redact its value"
        );
    }
}

#[test]
fn a_secret_shorter_than_four_is_not_redacted() {
    // `value.length >= 4` (`acceptance.ts:985`) — without the floor, a `DEBUG_TOKEN=on` would
    // redact every "on" in the output.
    let redacted = model::redact_verify_env(
        "turning on the thing",
        Some(&env_of(&[("DEBUG_TOKEN", "on")])),
    );
    assert_eq!(
        redacted, "turning on the thing",
        "a value under 4 long must be left alone"
    );
}

#[test]
fn overlapping_secrets_are_redacted_longest_first() {
    // `.sort((left, right) => right.length - left.length)` (`acceptance.ts:991`): redacting the
    // SHORT secret first would leave the long one's remainder ("-extended") in the output.
    let redacted = model::redact_verify_env(
        "leaked abcd1234-extended here",
        Some(&env_of(&[
            ("A_TOKEN", "abcd1234"),
            ("B_TOKEN", "abcd1234-extended"),
        ])),
    );
    assert_eq!(
        redacted, "leaked [REDACTED] here",
        "the longer secret must be redacted first, leaving no fragment behind"
    );
}

#[test]
fn every_occurrence_is_redacted_not_just_the_first() {
    // `replaceAll` (`acceptance.ts:992`).
    let redacted = model::redact_verify_env(
        "s3cr3tvalue and again s3cr3tvalue",
        Some(&env_of(&[("API_KEY", "s3cr3tvalue")])),
    );
    assert_eq!(redacted, "[REDACTED] and again [REDACTED]");
}

// ------------------------------------------------------------------------------------------------
// Redaction — through the LIVE runners, against real subprocess output
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_lattice_runner_redacts_a_leaked_secret_out_of_the_captured_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A real verify command that echoes its own credential — the exact shape of a `curl -v` or a
    // test harness dumping its environment on failure.
    let command = verify_with_env(
        "leaky",
        "echo \"authorization: Bearer $DEPLOY_TOKEN\"; exit 0",
        &[("DEPLOY_TOKEN", "tok_live_9f3a2b7c")],
    );

    let results = run_verify_commands_memoized(std::slice::from_ref(&command), dir.path(), None).await;

    assert_eq!(results.len(), 1);
    let tail = &results[0].output_tail;
    assert!(
        !tail.contains("tok_live_9f3a2b7c"),
        "the credential reached the acceptance ledger verbatim: {tail:?}"
    );
    assert!(
        tail.contains("[REDACTED]"),
        "the redaction marker must be present where the credential was: {tail:?}"
    );
    assert!(
        tail.contains("authorization: Bearer"),
        "only the secret is masked — the surrounding diagnostic text survives: {tail:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_model_runner_redacts_both_stdout_and_stderr() {
    let dir = tempfile::tempdir().expect("tempdir");
    let command = model::AcceptanceVerifyCommand {
        id: "leaky".to_string(),
        command: "echo \"out $CI_SECRET\"; echo \"err $CI_SECRET\" >&2; exit 0".to_string(),
        timeout_ms: None,
        cwd: None,
        env: Some(env_of(&[("CI_SECRET", "hunter2hunter2")])),
        allow_failure: None,
    };

    let result = model::run_memoized_verify_command(&command, dir.path(), None).await;

    let stdout = result.stdout.clone().unwrap_or_default();
    let stderr = result.stderr.clone().unwrap_or_default();
    assert!(
        !stdout.contains("hunter2hunter2") && !stderr.contains("hunter2hunter2"),
        "stdout={stdout:?} stderr={stderr:?} still carry the credential"
    );
    assert_eq!(stdout, "out [REDACTED]", "stdout redacted (acceptance.ts:1194)");
    assert_eq!(stderr, "err [REDACTED]", "stderr redacted (acceptance.ts:1195)");
    assert!(
        result.memoized.is_none() && result.cache_key.is_none(),
        "with no memo context the result carries no memoization evidence at all: {result:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redaction_runs_before_the_output_is_bounded_so_a_straddling_secret_cannot_leak() {
    // `trimOutput(redactVerifyEnv(stdout, env))` (`acceptance.ts:1194`) — redact the WHOLE capture,
    // then bound it. Bounding first would cut a secret in half and let the surviving half through.
    // The lattice runner keeps a 4096-byte TAIL, so this prints padding sized to land the secret
    // exactly across that cut.
    let dir = tempfile::tempdir().expect("tempdir");
    let secret = "AAAABBBBCCCCDDDDEEEEFFFF"; // 24 bytes
    // Raw output is `50*'x' + secret + 4084*'y'` = 4158 bytes. The 4096-byte tail therefore opens
    // at byte 62 — 12 bytes INTO the secret — so redacting the already-bounded tail would leave
    // `DDDDEEEEFFFF` in the ledger. Redacting first collapses the whole secret to `[REDACTED]`
    // before any cut is made.
    let command = verify_with_env(
        "straddle",
        "printf 'x%.0s' $(seq 1 50); printf '%s' \"$LEAK_TOKEN\"; printf 'y%.0s' $(seq 1 4084)",
        &[("LEAK_TOKEN", secret)],
    );

    let results =
        run_verify_commands_memoized(std::slice::from_ref(&command), dir.path(), None).await;
    let tail = &results[0].output_tail;
    let head = &tail[..tail.len().min(80)];

    assert!(
        !tail.contains("DDDDEEEEFFFF"),
        "the tail of a straddling secret survived the cut — redaction ran AFTER bounding: {head:?}"
    );
    assert!(
        tail.contains("[REDACTED]"),
        "the straddling secret must be masked whole, before bounding: {head:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_spawn_message_is_redacted_as_well() {
    // Upstream redacts the `child.on("error")` message too (`acceptance.ts:1204`). The message
    // echoes the command line, and a verify command may carry a credential in its own argv.
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("no-such-dir");
    // The OS error text for a missing cwd is stable ("No such file or directory"); declaring it AS
    // the credential is what makes the redaction observable on this arm without depending on the
    // error string happening to quote something secret.
    let command = VerifyCommand {
        cwd: Some(missing.display().to_string()),
        ..verify_with_env(
            "boom",
            "echo hi",
            &[("RUN_TOKEN", "No such file or directory")],
        )
    };

    let results =
        run_verify_commands_memoized(std::slice::from_ref(&command), dir.path(), None).await;

    let spawn_error = results[0]
        .spawn_error
        .clone()
        .expect("a command whose cwd does not exist cannot be spawned");
    assert!(
        spawn_error.contains("[REDACTED]"),
        "the spawn-error text goes through `redactVerifyEnv` too (acceptance.ts:1204): \
         {spawn_error}"
    );
    assert!(
        !spawn_error.contains("No such file or directory"),
        "the secret value must not survive in the spawn-error text: {spawn_error}"
    );
}

// ------------------------------------------------------------------------------------------------
// Memoization — through the LIVE gate `exec::run_sync` calls
// ------------------------------------------------------------------------------------------------

/// The acceptance contract the gate runs: `verified`, one real verify command.
fn verified_contract(command: VerifyCommand) -> AcceptanceContract {
    AcceptanceContract::explicit(AcceptanceStatus::Verified, vec![command])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_live_gate_replays_a_memoized_verify_result_instead_of_re_running_the_command() {
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let contract = verified_contract(verify("unit", &counting_command(&marker)));

    let first = evaluate_acceptance(
        &contract,
        clean_gate(),
        None,
        guard_did_not_fire(),
        repo.path(),
        Some(memo(artifacts.path(), "run-A")),
        None,
    )
    .await;
    assert_eq!(
        first.status,
        AcceptanceStatus::Verified,
        "an exit-0 verify command reaches verified: {first:?}"
    );
    assert_eq!(execution_count(&marker), 1, "the first evaluation really ran it");

    // The memo artifact upstream writes (`acceptance.ts:1113-1126`).
    let cache_dir = artifacts.path().join("acceptance").join("verify").join("run-A");
    let entries: Vec<_> = std::fs::read_dir(&cache_dir)
        .expect("the memo directory is created")
        .filter_map(Result::ok)
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "exactly one memo artifact for one verify command: {entries:?}"
    );

    // NB the marker file lives inside the repo but is UNTRACKED, so `git diff HEAD` does not see
    // it — writing it does not perturb the workspace state the cache key is built from.
    let second = evaluate_acceptance(
        &contract,
        clean_gate(),
        None,
        guard_did_not_fire(),
        repo.path(),
        Some(memo(artifacts.path(), "run-A")),
        None,
    )
    .await;

    assert_eq!(second.status, AcceptanceStatus::Verified);
    assert_eq!(
        execution_count(&marker),
        1,
        "the second evaluation must REPLAY the recorded result, not spawn the command again"
    );
    assert_eq!(
        second.verify_results, first.verify_results,
        "a replayed result is the recorded one verbatim (`acceptance.ts:1106`)"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn editing_a_tracked_file_invalidates_the_memo_and_the_command_runs_again() {
    // `diffHash` covers `git diff --binary --full-index HEAD` (`acceptance.ts:1051,1058`), so ANY
    // edit to the working tree changes the cache key. That is the whole reason replaying a
    // `cargo test` result is safe.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let contract = verified_contract(verify("unit", &counting_command(&marker)));

    for _ in 0..2 {
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            guard_did_not_fire(),
            repo.path(),
            Some(memo(artifacts.path(), "run-B")),
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Verified);
    }
    assert_eq!(execution_count(&marker), 1, "premise: the second was a memo hit");

    std::fs::write(repo.path().join("tracked.txt"), "one\ntwo\n").expect("edit tracked file");

    let after_edit = evaluate_acceptance(
        &contract,
        clean_gate(),
        None,
        guard_did_not_fire(),
        repo.path(),
        Some(memo(artifacts.path(), "run-B")),
        None,
    )
    .await;

    assert_eq!(after_edit.status, AcceptanceStatus::Verified);
    assert_eq!(
        execution_count(&marker),
        2,
        "a changed working tree must invalidate the memo and re-run the command"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_different_run_id_does_not_share_a_memo() {
    // The run id is a path segment of the artifact (`acceptance.ts:1102`), so memos are scoped per
    // run — a fresh run never inherits an older run's recorded verification.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let contract = verified_contract(verify("unit", &counting_command(&marker)));

    for run_id in ["run-1", "run-2"] {
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            guard_did_not_fire(),
            repo.path(),
            Some(memo(artifacts.path(), run_id)),
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Verified);
    }

    assert_eq!(
        execution_count(&marker),
        2,
        "two distinct run ids must each execute the command"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_memo_context_the_gate_executes_every_time() {
    // `if (!workspaceState || !options.artifactsDir || !options.runId) return runVerifyCommand(...)`
    // (`acceptance.ts:1085-1087`) — and this is also the pre-G80 behavior, unchanged.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let marker = repo.path().join("runs.log");
    let contract = verified_contract(verify("unit", &counting_command(&marker)));

    for _ in 0..2 {
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            guard_did_not_fire(),
            repo.path(),
            None,
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Verified);
    }

    assert_eq!(
        execution_count(&marker),
        2,
        "no artifacts dir/run id means no memoization at all"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn outside_a_git_working_tree_nothing_is_memoized() {
    // `readVerifyWorkspaceState` returns undefined for a non-repo cwd (`acceptance.ts:1046-1048`),
    // which disables memoization for that command — there is no cheap identity to key a cache on.
    let plain = tempfile::tempdir().expect("tempdir");
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = plain.path().join("runs.log");
    let contract = verified_contract(verify("unit", &counting_command(&marker)));

    for _ in 0..2 {
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            guard_did_not_fire(),
            plain.path(),
            Some(memo(artifacts.path(), "run-C")),
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Verified);
    }

    assert_eq!(execution_count(&marker), 2, "a non-git cwd is never memoized");
    assert!(
        !artifacts.path().join("acceptance").exists(),
        "and no memo artifact is written for it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_verify_result_is_memoized_and_still_rejects_on_replay() {
    // Upstream memoizes the RESULT, not "success" — `isCachedVerifyResult` accepts any of the four
    // statuses (`acceptance.ts:1068`). A replayed failure must still reject the gate.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let contract = verified_contract(verify(
        "unit",
        &format!("{}; exit 3", counting_command(&marker)),
    ));

    let first = evaluate_acceptance(
        &contract,
        clean_gate(),
        None,
        guard_did_not_fire(),
        repo.path(),
        Some(memo(artifacts.path(), "run-D")),
        None,
    )
    .await;
    let second = evaluate_acceptance(
        &contract,
        clean_gate(),
        None,
        guard_did_not_fire(),
        repo.path(),
        Some(memo(artifacts.path(), "run-D")),
        None,
    )
    .await;

    assert_eq!(first.status, AcceptanceStatus::Rejected, "{first:?}");
    assert_eq!(
        second.status,
        AcceptanceStatus::Rejected,
        "a replayed FAILURE must still reject: {second:?}"
    );
    assert_eq!(execution_count(&marker), 1, "the failure was replayed, not re-run");
    assert_eq!(second.verify_results[0].exit_code, Some(3));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_declared_env_value_invalidates_the_memo() {
    // `envHash` covers the whole EFFECTIVE environment (`acceptance.ts:1089`), so rotating a
    // credential re-runs the verification even though the command text and the tree are identical —
    // and it does so without the value ever being written into the artifact.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let command_text = counting_command(&marker);

    for value in ["first_value", "second_value"] {
        let contract = verified_contract(verify_with_env(
            "unit",
            &command_text,
            &[("DEPLOY_TOKEN", value)],
        ));
        let ledger = evaluate_acceptance(
            &contract,
            clean_gate(),
            None,
            guard_did_not_fire(),
            repo.path(),
            Some(memo(artifacts.path(), "run-E")),
            None,
        )
        .await;
        assert_eq!(ledger.status, AcceptanceStatus::Verified);
    }

    assert_eq!(
        execution_count(&marker),
        2,
        "a rotated credential must invalidate the memo"
    );

    // And the credential itself must not be sitting in the artifact directory.
    let cache_dir = artifacts.path().join("acceptance").join("verify").join("run-E");
    for entry in std::fs::read_dir(&cache_dir).expect("memo dir").flatten() {
        let text = std::fs::read_to_string(entry.path()).expect("readable artifact");
        assert!(
            !text.contains("first_value") && !text.contains("second_value"),
            "the memo artifact records env KEY NAMES and a HASH, never values: {text}"
        );
        assert!(
            text.contains("\"envKeys\""),
            "the artifact records the declared key names (acceptance.ts:1119): {text}"
        );
    }
}

// ------------------------------------------------------------------------------------------------
// Memoization — the pi-shaped `model` runner and its evidence fields
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_model_runner_stamps_upstreams_memoization_evidence_onto_the_result() {
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let command = model::AcceptanceVerifyCommand {
        id: "unit".to_string(),
        command: counting_command(&marker),
        timeout_ms: None,
        cwd: None,
        env: Some(env_of(&[("BUILD_PROFILE", "release")])),
        allow_failure: None,
    };
    let ctx = memo(artifacts.path(), "run-F");

    let fresh = model::run_memoized_verify_command(&command, repo.path(), Some(ctx)).await;
    assert_eq!(
        fresh.memoized,
        Some(false),
        "a freshly executed result is stamped `memoized: false` (`acceptance.ts:1112`): {fresh:?}"
    );
    assert!(fresh.cache_key.is_some(), "{fresh:?}");
    assert!(fresh.artifact_path.is_some(), "{fresh:?}");
    assert!(fresh.artifact_error.is_none(), "{fresh:?}");
    assert_eq!(
        fresh.env_keys.as_deref(),
        Some(["BUILD_PROFILE".to_string()].as_slice()),
        "`envKeys` is the command's OWN declared keys, sorted (`acceptance.ts:1088`)"
    );
    assert!(fresh.env_hash.is_some(), "{fresh:?}");
    let workspace = fresh.workspace_state.clone().expect("git workspace state");
    assert_eq!(workspace.kind, model::VerifyWorkspaceKind::GitTracked);
    assert_eq!(workspace.cwd_relative, ".", "cwd IS the repo root here");
    assert!(
        workspace.head.len() >= 40 && workspace.head.chars().all(|c| c.is_ascii_hexdigit()),
        "a full HEAD object id: {}",
        workspace.head
    );

    let replayed = model::run_memoized_verify_command(&command, repo.path(), Some(ctx)).await;
    assert_eq!(
        replayed.memoized,
        Some(true),
        "the second call replays (`acceptance.ts:1106`): {replayed:?}"
    );
    assert_eq!(execution_count(&marker), 1, "nothing was spawned the second time");
    assert_eq!(replayed.exit_code, fresh.exit_code);
    assert_eq!(replayed.status, fresh.status);
    assert_eq!(replayed.cache_key, fresh.cache_key);
    assert_eq!(replayed.workspace_state, fresh.workspace_state);
    assert_eq!(
        replayed.id, command.id,
        "id/command/cwd are re-stamped from the CURRENT command, not replayed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_unwritable_artifact_path_sets_artifact_error_and_clears_artifact_path() {
    // `catch (error) { evidenced.artifactError = …; delete evidenced.artifactPath; }`
    // (`acceptance.ts:1127-1130`) — a memo that cannot be written never fails the verification and
    // never claims an artifact that is not there.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let blocker = tempfile::tempdir().expect("blocker");
    // A regular FILE where the artifacts ROOT should be: `create_dir_all` under it must fail.
    let artifacts_root = blocker.path().join("artifacts");
    std::fs::write(&artifacts_root, "not a directory").expect("write blocker file");

    let command = model::AcceptanceVerifyCommand {
        id: "unit".to_string(),
        command: "exit 0".to_string(),
        timeout_ms: None,
        cwd: None,
        env: None,
        allow_failure: None,
    };

    let result =
        model::run_memoized_verify_command(&command, repo.path(), Some(memo(&artifacts_root, "run-G")))
            .await;

    assert_eq!(
        result.status,
        model::VerifyRunStatus::Passed,
        "the verification itself is unaffected by an artifact-write failure: {result:?}"
    );
    assert!(
        result.artifact_error.is_some(),
        "the write failure is recorded: {result:?}"
    );
    assert!(
        result.artifact_path.is_none(),
        "and the artifact path is cleared, never claimed: {result:?}"
    );
    assert_eq!(result.memoized, Some(false));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_corrupt_memo_artifact_is_a_miss_not_a_failure() {
    // `catch { /* A cache miss or unreadable artifact must not prevent host verification. */ }`
    // (`acceptance.ts:1108-1110`).
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let command = model::AcceptanceVerifyCommand {
        id: "unit".to_string(),
        command: counting_command(&marker),
        timeout_ms: None,
        cwd: None,
        env: None,
        allow_failure: None,
    };
    let ctx = memo(artifacts.path(), "run-H");

    let fresh = model::run_memoized_verify_command(&command, repo.path(), Some(ctx)).await;
    let artifact = fresh.artifact_path.clone().expect("an artifact was written");
    std::fs::write(&artifact, "{ this is not json").expect("corrupt the artifact");

    let after = model::run_memoized_verify_command(&command, repo.path(), Some(ctx)).await;

    assert_eq!(
        after.memoized,
        Some(false),
        "a corrupt artifact must be a MISS, so the command really runs: {after:?}"
    );
    assert_eq!(execution_count(&marker), 2);
    assert_eq!(after.status, model::VerifyRunStatus::Passed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_memoized_result_still_carries_the_redacted_output() {
    // The two halves compose: what is recorded is the ALREADY-redacted capture, so a replay cannot
    // resurrect a credential the live run masked.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let command = model::AcceptanceVerifyCommand {
        id: "leaky".to_string(),
        command: "echo \"token=$DEPLOY_TOKEN\"".to_string(),
        timeout_ms: None,
        cwd: None,
        env: Some(env_of(&[("DEPLOY_TOKEN", "tok_live_9f3a2b7c")])),
        allow_failure: None,
    };
    let ctx = memo(artifacts.path(), "run-I");

    let fresh = model::run_memoized_verify_command(&command, repo.path(), Some(ctx)).await;
    let replayed = model::run_memoized_verify_command(&command, repo.path(), Some(ctx)).await;

    assert_eq!(replayed.memoized, Some(true), "premise: this is a replay");
    assert_eq!(fresh.stdout.as_deref(), Some("token=[REDACTED]"));
    assert_eq!(
        replayed.stdout.as_deref(),
        Some("token=[REDACTED]"),
        "the replayed capture is the redacted one"
    );

    let artifact = std::fs::read_to_string(replayed.artifact_path.clone().expect("artifact path"))
        .expect("readable artifact");
    assert!(
        !artifact.contains("tok_live_9f3a2b7c"),
        "the credential must not be written to disk in the memo artifact either: {artifact}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_verify_workspace_state_reports_the_repo_relative_subdirectory() {
    // `cwdRelative: path.relative(repoRoot, fs.realpathSync(cwd)) || "."` (`acceptance.ts:1056`) —
    // part of the cache key, so the same command run from two directories memoizes separately.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let nested = repo.path().join("crates").join("inner");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let state = model::read_verify_workspace_state(&nested)
        .await
        .expect("a subdirectory of a repo is git-tracked");

    assert_eq!(state.cwd_relative, "crates/inner");
    assert_eq!(state.kind, model::VerifyWorkspaceKind::GitTracked);
    assert!(!state.diff_hash.is_empty());

    // A directory outside any repository has no workspace state — which is exactly what disables
    // memoization there (`acceptance.ts:1085-1087`). Guarded on the (pathological) case of a
    // temp dir that is itself inside a checkout.
    let outside = tempfile::tempdir().expect("outside dir");
    let outside_state = model::read_verify_workspace_state(outside.path()).await;
    assert!(
        outside_state.is_none() || outside_state.is_some_and(|s| s.repo_root != state.repo_root),
        "a plain temp directory must not resolve to the fixture repository"
    );
}
