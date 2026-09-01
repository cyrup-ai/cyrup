//! G80 — the parts of verify-result MEMOIZATION that `tests/verify_memo_and_redaction.rs` leaves
//! unproven:
//!
//! 1. **The cache-key TERMS.** `runMemoizedVerifyCommand` keys on nine values
//!    (`pi-subagents/src/runs/shared/acceptance.ts:1091-1101` @v0.43.0). That file proves `command`,
//!    `envHash`, `head`/`diffHash` and the `runId` path segment; it never varies `cwdRelative`,
//!    `timeoutMs` or `allowFailure`, so each of those three could be deleted from the key material
//!    with the whole suite green. `allowFailure` is not cosmetic: with it missing, a command
//!    re-declared `allowFailure: true` REPLAYS the earlier `failed` result and rejects a run its
//!    author explicitly allowed to fail.
//!
//! 2. **The memo-hit re-stamp** of `id`/`command`/`cwd` (`acceptance.ts:1106`:
//!    `{ ...cached.result, id: command.id, command: command.command, cwd, … }`). The recorded
//!    result's own copies of those three fields must never be replayed, or a renamed criterion
//!    reports under the name it had when the memo was written.
//!
//! 3. **The LEFT `(?:^|_)` boundary of `SENSITIVE_ENV_KEY_PATTERN`** (`acceptance.ts:974`). Every
//!    negative probe in the sibling file (`TOKENIZER`, `PASSAGE`, `AUTHORITY`, …) fails on the RIGHT
//!    boundary, so removing the left one leaves that test green. This is a secret-redaction
//!    boundary: without the left check, `MYTOKEN=<value>` starts blanket-replacing that value
//!    everywhere in captured verify output.
//!
//! 4. **The LIVE WIRING.** Every memo test in the sibling file hand-builds a `VerifyMemoContext`.
//!    The three places that build one in production are untested:
//!    `exec/mod.rs`'s `(opts.artifacts_dir, opts.run_id)` pair, `extension.rs`'s
//!    `artifacts_dir: art_cfg.enabled.then(|| art_dir.clone())` (so SUBA-041's `artifacts: false`
//!    disarms memoization with the quadruple), and `background/runner_main.rs`'s
//!    `self.artifacts_dir.clone().filter(|_| self.artifact_config.enabled)` on the hop-2 runner.
//!    The last five tests here drive those seams end to end — including BOTH terms of the hop-2
//!    runner's gate, which need three cases between them because `artifacts_dir: None` alone can
//!    never reach the `.filter(…)` that the `artifact_config.enabled` term lives in.
//!
//! No mocking: every verify command below is a REAL `/bin/sh` subprocess, every workspace is a REAL
//! `git` repository, and the two wiring tests spawn the REAL `cyrup-subagent-fixture` binary as a
//! genuine OS subprocess (`CYRUP_SUBAGENT_BINARY`), exactly like every sibling integration test.
//!
//! Gated on `test-fixtures` for the fixture binary, matching every sibling integration test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};


use cyrup_core::{CancelToken, Content, ModelId, Tool, ToolCallId};
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::artifacts::project_artifacts_dir;
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::runner_main::{RunnerConfig, RunnerOverrides, run_with};
use cyrup_ext_subagents::background::{RunId, RunMode, RunPaths};
use cyrup_ext_subagents::discovery::types::SystemPromptMode;
use cyrup_ext_subagents::exec::ResolvedAgentPersona;
use cyrup_ext_subagents::exec::acceptance::model;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

// ------------------------------------------------------------------------------------------------
// Fixtures
// ------------------------------------------------------------------------------------------------



fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

fn env_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// A real git repository with one committed file, so `readVerifyWorkspaceState`
/// (`acceptance.ts:1046-1060`) can resolve `HEAD` and a diff — memoization is disabled outright
/// outside a working tree (`:1085-1087`).
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

fn shell_quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

/// A verify command that appends one line to `marker` every time it ACTUALLY executes. The path is
/// ABSOLUTE, so the identical command text runs identically from any cwd — which is what lets the
/// `cwdRelative` test vary the cwd and nothing else.
fn counting_command(marker: &Path) -> String {
    format!("echo ran >> {}", shell_quote(marker))
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

fn command(id: &str, text: &str) -> model::AcceptanceVerifyCommand {
    model::AcceptanceVerifyCommand {
        id: id.to_string(),
        command: text.to_string(),
        timeout_ms: None,
        cwd: None,
        env: None,
        allow_failure: None,
    }
}

/// Every `*.json` memo artifact under `<artifacts>/acceptance/verify/`, recursively — the tree
/// `acceptance.ts:1102` writes into.
fn memo_artifacts(artifacts_dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().and_then(|e| e.to_str()) == Some("json") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&artifacts_dir.join("acceptance").join("verify"), &mut out);
    out.sort();
    out
}

// ------------------------------------------------------------------------------------------------
// 1. The cache-key terms (acceptance.ts:1091-1101)
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cache_key_covers_the_repo_relative_cwd() {
    // The whole file is serialized on this: `envHash` makes every cache key a function of the
    // `cwdRelative: workspaceState.cwdRelative` (`acceptance.ts:1093`). The same command text run
    // from two directories of the SAME repository is two different verifications — `cargo test` in
    // `crates/a` proves nothing about `crates/b` — so they must not share a memo.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let nested = repo.path().join("sub");
    std::fs::create_dir_all(&nested).expect("nested dir");
    let marker = repo.path().join("runs.log");
    let ctx = memo(artifacts.path(), "run-CWD");

    let at_root = command("unit", &counting_command(&marker));
    let at_sub = model::AcceptanceVerifyCommand {
        cwd: Some("sub".to_string()),
        ..at_root.clone()
    };

    let first = model::run_memoized_verify_command(&at_root, repo.path(), Some(ctx)).await;
    let second = model::run_memoized_verify_command(&at_sub, repo.path(), Some(ctx)).await;

    assert_eq!(first.memoized, Some(false), "premise: the first executed");
    assert_eq!(
        second.memoized,
        Some(false),
        "a different repo-relative cwd is a different cache key, so this must MISS: {second:?}"
    );
    assert_ne!(
        first.cache_key, second.cache_key,
        "`cwdRelative` must be part of the key material"
    );
    assert_eq!(
        execution_count(&marker),
        2,
        "both directories really ran the command"
    );
    // Everything else about the two is identical, which is what makes `cwdRelative` the only
    // possible cause of the difference.
    assert_eq!(first.command, second.command);
    assert_eq!(first.env_hash, second.env_hash);
    assert_eq!(
        first.workspace_state.as_ref().map(|w| w.cwd_relative.clone()),
        Some(".".to_string())
    );
    assert_eq!(
        second.workspace_state.as_ref().map(|w| w.cwd_relative.clone()),
        Some("sub".to_string())
    );
    assert_eq!(
        first.workspace_state.as_ref().map(|w| w.head.clone()),
        second.workspace_state.as_ref().map(|w| w.head.clone()),
        "same repository, same HEAD — the tree is NOT what differs here"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cache_key_covers_the_declared_timeout() {
    // The whole file is serialized on this: `envHash` makes every cache key a function of the
    // `timeoutMs: command.timeoutMs ?? 120_000` (`acceptance.ts:1090,1097`). A command re-declared
    // with a tighter bound is a different verification: replaying a result recorded under a
    // 120 s budget would report `passed` for a command the author has since said must finish in 5 s.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let ctx = memo(artifacts.path(), "run-TMO");

    let defaulted = command("unit", &counting_command(&marker));
    let bounded = model::AcceptanceVerifyCommand {
        timeout_ms: Some(60_000),
        ..defaulted.clone()
    };

    let first = model::run_memoized_verify_command(&defaulted, repo.path(), Some(ctx)).await;
    let second = model::run_memoized_verify_command(&bounded, repo.path(), Some(ctx)).await;

    assert_eq!(first.memoized, Some(false), "premise: the first executed");
    assert_eq!(
        second.memoized,
        Some(false),
        "a declared `timeoutMs` must not hit a memo recorded under the default: {second:?}"
    );
    assert_ne!(
        first.cache_key, second.cache_key,
        "`timeoutMs` must be part of the key material"
    );
    assert_eq!(execution_count(&marker), 2);

    // And re-declaring the SAME timeout does hit, so the term is the timeout value, not merely the
    // presence of the field.
    let again = model::run_memoized_verify_command(&bounded, repo.path(), Some(ctx)).await;
    assert_eq!(again.memoized, Some(true), "{again:?}");
    assert_eq!(again.cache_key, second.cache_key);
    assert_eq!(execution_count(&marker), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_cache_key_covers_allow_failure_so_a_replay_cannot_change_the_verdict() {
    // The whole file is serialized on this: `envHash` makes every cache key a function of the
    // `allowFailure: command.allowFailure === true` (`acceptance.ts:1098`). This term is
    // BEHAVIOUR-CHANGING, not bookkeeping: `allowFailure` is exactly what turns a nonzero exit from
    // `failed` (which rejects the whole run, `acceptance.ts:1297`) into `allowed-failure` (which
    // does not). Drop the term from the key and the second call below replays the recorded
    // `failed`, rejecting a run whose author explicitly allowed that command to fail.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let ctx = memo(artifacts.path(), "run-ALW");

    let strict = command("lint", &format!("{}; exit 4", counting_command(&marker)));
    let permissive = model::AcceptanceVerifyCommand {
        allow_failure: Some(true),
        ..strict.clone()
    };

    let first = model::run_memoized_verify_command(&strict, repo.path(), Some(ctx)).await;
    assert_eq!(first.status, model::VerifyRunStatus::Failed);
    assert!(first.rejects(), "premise: this one rejects the run");

    let second = model::run_memoized_verify_command(&permissive, repo.path(), Some(ctx)).await;

    assert_eq!(
        second.memoized,
        Some(false),
        "flipping `allowFailure` must MISS, not replay the strict verdict: {second:?}"
    );
    assert_ne!(
        first.cache_key, second.cache_key,
        "`allowFailure` must be part of the key material"
    );
    assert_eq!(
        second.status,
        model::VerifyRunStatus::AllowedFailure,
        "the same nonzero exit is an ALLOWED failure once the author says so: {second:?}"
    );
    assert!(
        !second.rejects(),
        "and it must not reject the run — the whole point of the flag"
    );
    assert_eq!(second.exit_code, Some(4), "same real exit code either way");
    assert_eq!(execution_count(&marker), 2, "both really ran");

    // The reverse direction too: an `allowFailure` memo must not be replayed for a strict re-run.
    let strict_again = model::run_memoized_verify_command(&strict, repo.path(), Some(ctx)).await;
    assert_eq!(
        strict_again.memoized,
        Some(true),
        "the STRICT key was recorded first and is still valid: {strict_again:?}"
    );
    assert_eq!(strict_again.status, model::VerifyRunStatus::Failed);
    assert!(strict_again.rejects());
    assert_eq!(execution_count(&marker), 2, "that one was a replay");
}

// ------------------------------------------------------------------------------------------------
// 2. The memo-hit re-stamp (acceptance.ts:1106)
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_memo_hit_reports_the_current_criterion_id_not_the_recorded_one() {
    // The whole file is serialized on this: `envHash` makes every cache key a function of the
    // `id` is NOT part of the cache key (`acceptance.ts:1091-1101` lists nine terms and `id` is not
    // among them), so renaming a verify command's id and leaving its text alone is a memo HIT — and
    // the replay must announce itself under the CURRENT id (`id: command.id`, `:1106`). Replaying
    // the recorded id makes a ledger name a criterion the policy no longer declares, and
    // `acceptanceFailureMessage`'s `Acceptance verification '<id>' failed.` (`:1362`) then points
    // the reader at a command that does not exist.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let ctx = memo(artifacts.path(), "run-ID");
    let text = counting_command(&marker);

    let old_name = command("unit-tests", &text);
    let new_name = command("cargo-test", &text);

    let recorded = model::run_memoized_verify_command(&old_name, repo.path(), Some(ctx)).await;
    assert_eq!(recorded.memoized, Some(false));
    assert_eq!(recorded.id, "unit-tests");

    let replayed = model::run_memoized_verify_command(&new_name, repo.path(), Some(ctx)).await;

    assert_eq!(
        replayed.memoized,
        Some(true),
        "the id is not in the key, so this must be a HIT: {replayed:?}"
    );
    assert_eq!(execution_count(&marker), 1, "nothing was spawned the second time");
    assert_eq!(
        replayed.id, "cargo-test",
        "a renamed criterion must report under its NEW name, not the recorded one"
    );
    assert_eq!(replayed.cache_key, recorded.cache_key);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_memo_hit_re_stamps_command_and_cwd_over_whatever_the_artifact_recorded() {
    // The whole file is serialized on this: `envHash` makes every cache key a function of the
    // The other two thirds of `{ ...cached.result, id, command: command.command, cwd, … }`
    // (`acceptance.ts:1106`). `command` and `cwd` ARE key terms, so the only way to observe the
    // re-stamp is to make the ARTIFACT disagree with the key it is filed under — which is exactly
    // what a hand-edited or partially-written artifact looks like. The spread must not let the
    // recorded copies through: a ledger that reports a command the run never issued, in a directory
    // it never entered, is worse than a cache miss.
    let repo = tempfile::tempdir().expect("tempdir");
    init_repo(repo.path());
    let artifacts = tempfile::tempdir().expect("artifacts");
    let marker = repo.path().join("runs.log");
    let ctx = memo(artifacts.path(), "run-STAMP");
    let cmd = command("unit", &counting_command(&marker));

    let recorded = model::run_memoized_verify_command(&cmd, repo.path(), Some(ctx)).await;
    let artifact = PathBuf::from(recorded.artifact_path.clone().expect("an artifact was written"));

    // Rewrite ONLY the nested `result`'s identity fields; `cacheKey`/`resultShape` stay valid so the
    // artifact is still a HIT.
    let mut stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&artifact).expect("read artifact"))
            .expect("the artifact is JSON");
    let result = stored
        .get_mut("result")
        .and_then(serde_json::Value::as_object_mut)
        .expect("the artifact carries a `result` object");
    result.insert("id".into(), serde_json::json!("stale-id"));
    result.insert("command".into(), serde_json::json!("rm -rf /stale"));
    result.insert("cwd".into(), serde_json::json!("/stale/directory"));
    std::fs::write(&artifact, stored.to_string()).expect("rewrite artifact");

    let replayed = model::run_memoized_verify_command(&cmd, repo.path(), Some(ctx)).await;

    assert_eq!(replayed.memoized, Some(true), "premise: still a hit: {replayed:?}");
    assert_eq!(execution_count(&marker), 1, "premise: nothing re-ran");
    assert_eq!(replayed.id, "unit", "`id` is re-stamped from the CURRENT command");
    assert_eq!(
        replayed.command, cmd.command,
        "`command` is re-stamped from the CURRENT command, never replayed"
    );
    assert_eq!(
        replayed.cwd.as_deref(),
        Some(repo.path().display().to_string().as_str()),
        "`cwd` is the CURRENT resolved directory, never the recorded one"
    );
    // The recorded halves that ARE replayed still come through, so this is a real hit and not a
    // silently-degraded miss.
    assert_eq!(replayed.status, recorded.status);
    assert_eq!(replayed.exit_code, recorded.exit_code);
    assert_eq!(replayed.duration_ms, recorded.duration_ms);
}

// ------------------------------------------------------------------------------------------------
// 3. SENSITIVE_ENV_KEY_PATTERN's LEFT boundary (acceptance.ts:974)
// ------------------------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_sensitive_key_pattern_requires_the_left_boundary_too() {
    // The whole file is serialized on this: `envHash` makes every cache key a function of the
    // `/(?:^|_)(?:TOKEN|SECRET|…)(?:_|$)/i` — the LEFT `(?:^|_)` is a separate condition from the
    // right one, and the sibling file's negatives (`TOKENIZER`, `PASSAGE`, `AUTHORITY`, `SESSIONS`,
    // `APIKEY`) every one of them fails on the RIGHT side, so deleting the left check leaves that
    // test green. Each key below ends a sensitive word at a real right boundary and is only
    // excluded because the character BEFORE the word is neither `_` nor the start of the string.
    //
    // Direction of the risk: this pattern decides which environment VALUES get blanket-replaced in
    // every captured stdout/stderr. Over-matching is not "safe by default" — a
    // `MYTOKEN=production` would start rewriting the word "production" out of every build log, and
    // a maintainer reading a mangled log has no way to tell that happened.
    let value = "supersecretvalue";

    let left_boundary_only = [
        "MYTOKEN",        // TOKEN at index 2, right boundary is end-of-string
        "XSECRET",        // SECRET at index 1
        "MYPASSWORD",     // PASSWORD at index 2
        "NOPASS",         // PASS at index 2
        "UNAUTH",         // AUTH at index 2
        "XCREDENTIAL",
        "ACOOKIE",
        "MYSESSION",
        "SUBPRIVATE",
        "DEVAPI_KEY",     // API_KEY at index 3
        "PREACCESS_KEY",  // ACCESS_KEY at index 3
        "NOTAUTH_MODE",   // AUTH at index 3, right boundary is the `_` — left is `T`
        "MYTOKEN_FILE",   // TOKEN at index 2, right boundary is the `_`
    ];
    for key in left_boundary_only {
        assert_eq!(
            model::redact_verify_env("value=supersecretvalue", Some(&env_of(&[(key, value)]))),
            "value=supersecretvalue",
            "`{key}` has no LEFT `(?:^|_)` boundary and must NOT be treated as sensitive \
             (acceptance.ts:974)"
        );
    }

    // Positive control on the SAME value, so the test cannot pass by the redaction machinery being
    // switched off: prefix each of those words with a real `_` boundary and every one matches.
    for key in [
        "MY_TOKEN",
        "X_SECRET",
        "MY_PASSWORD",
        "NO_PASS",
        "UN_AUTH",
        "X_CREDENTIAL",
        "A_COOKIE",
        "MY_SESSION",
        "SUB_PRIVATE",
        "DEV_API_KEY",
        "PRE_ACCESS_KEY",
        "NOT_AUTH_MODE",
        "MY_TOKEN_FILE",
    ] {
        assert_eq!(
            model::redact_verify_env("value=supersecretvalue", Some(&env_of(&[(key, value)]))),
            "value=[REDACTED]",
            "`{key}` DOES have both boundaries and must be redacted (acceptance.ts:974)"
        );
    }

    // Through the live runner, against real subprocess output — the boundary rule is only worth
    // anything where the capture actually happens.
    let dir = tempfile::tempdir().expect("tempdir");
    let leaky = model::AcceptanceVerifyCommand {
        env: Some(env_of(&[("MYTOKEN", "production"), ("MY_TOKEN", "tok_live_a1b2")])),
        ..command(
            "boundary",
            "echo \"built for $MYTOKEN with $MY_TOKEN\"; exit 0",
        )
    };
    let result = model::run_memoized_verify_command(&leaky, dir.path(), None).await;
    let stdout = result.stdout.clone().unwrap_or_default();
    assert!(
        !stdout.contains("tok_live_a1b2"),
        "the genuinely-sensitive value must be masked: {stdout:?}"
    );
    assert!(
        stdout.contains("production"),
        "and the non-sensitive one must survive verbatim — over-matching silently corrupts build \
         output: {stdout:?}"
    );
}

// ------------------------------------------------------------------------------------------------
// 4. LIVE WIRING — the three production builders of a `VerifyMemoContext`
// ------------------------------------------------------------------------------------------------

fn write_fixture_persona(cwd: &Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the G80 memo-wiring test\n\
             model: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 2,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// A `verify[]` policy declaring the SAME command text twice under two different ids. `id` is not a
/// cache-key term, so within ONE run the second entry is a memo HIT — which makes the whole memo
/// loop (derive key -> miss -> execute -> write artifact -> derive key -> hit -> replay) observable
/// at a live seam whose `runId` is generated internally and cannot be reused across calls.
fn duplicate_verify_policy(marker: &Path) -> serde_json::Value {
    serde_json::json!({
        "level": "verified",
        "verify": [
            {"id": "first", "command": counting_command(marker)},
            {"id": "second", "command": counting_command(marker)},
        ]
    })
}

fn tool_result_text(result: &cyrup_core::ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

/// G80 LIVE WIRING (`exec/mod.rs`'s `(opts.artifacts_dir, opts.run_id)` -> `VerifyMemoContext`, and
/// `extension.rs`'s `artifacts_dir: art_cfg.enabled.then(|| art_dir.clone())`).
///
/// Drives the REAL `SubagentTool::execute` -> `route_single` -> `run_foreground_streaming` ->
/// `exec::run_sync` -> `exec::acceptance::evaluate_acceptance` path with a real `verified`
/// acceptance policy, and asserts the memo artifact upstream writes
/// (`<artifactsDir>/acceptance/verify/<runId>/<cacheKey>.json`, `acceptance.ts:1102`) really lands
/// there. Without the wiring the gate receives `memo: None`, every command executes, and nothing is
/// written — which is exactly what the `artifacts: false` half of this pair asserts.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_subagent_tools_single_run_memoizes_its_verify_commands_under_the_run_artifacts_dir() {

    let work_dir = tempfile::tempdir().expect("tempdir for the fixture persona + cwd");
    let home_dir = tempfile::tempdir().expect("tempdir to isolate CYRUP_HOME artifacts");
    init_repo(work_dir.path());
    write_fixture_persona(work_dir.path(), "worker");
    // The marker lives OUTSIDE the repo so writing it cannot perturb the working-tree diff the
    // cache key is built from.
    let marker_dir = tempfile::tempdir().expect("marker dir");
    let marker = marker_dir.path().join("runs.log");

    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": message_end_line("G80_MEMO_WIRING: done") }],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();

    // `project_artifacts_dir`, NOT `temp_artifacts_dir`: `ArtifactDirPreference::default()` is
    // `Project` — pi's `DEFAULT_ARTIFACT_CONFIG.dir = "project"` (`src/shared/types.ts:1796-1798`
    // @v0.43.0) — so a subagent-tool run with the default config writes
    // `<cwd>/.cyrup-subagents/artifacts`. SUBA-048 moved that default onto pi's and these two
    // sites were never updated: the presence assertion below went red, and the sibling
    // `artifacts_false_...` ABSENCE assertion went vacuous — it was looking in a directory the
    // run would not have written to even with artifacts enabled.
    let art_dir = project_artifacts_dir(work_dir.path());
    let extension = SubagentsExtension::with_config_and_cwd(
        // SUBA-083: asserts verify-command execution counts under the run artifacts dir — only a
        // completed foreground run memoizes (pi `config.ts:222-224`).
        SubagentExtensionConfig {
            // Named here rather than exported; these runs settle in the FOREGROUND, which is the
            // path `spawn_command` reaches.
            spawn_command: Some(SpawnCommand {
                binary: fixture,
                base_args: vec![
                    "--fixture-script".to_string(),
                    script_path.display().to_string(),
                ],
            }),
            roots: Roots::sandboxed(home_dir.path()),
            async_by_default: false,
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    );
    let result = extension
        .subagent_tool()
        .execute(
            ToolCallId::from("g80memo"),
            serde_json::json!({
                "agent": "worker",
                "task": "summarize the repository layout",
                "acceptance": duplicate_verify_policy(&marker),
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    let result = result.expect("the run must COMPLETE, not be refused at dispatch");
    let text = tool_result_text(&result);
    assert!(
        text.contains("G80_MEMO_WIRING"),
        "premise: the real child ran and its output was delivered: {text:?}"
    );

    // The wiring under test: the gate really received a memo context, so the artifact exists.
    let artifacts = memo_artifacts(&art_dir);
    assert_eq!(
        artifacts.len(),
        1,
        "one memo artifact for two identically-keyed verify commands, under \
         <artifactsDir>/acceptance/verify/<runId>/ (acceptance.ts:1102); found: {artifacts:?} \
         under {}",
        art_dir.display()
    );
    // ... and the memo really SERVED: the second declared command replayed rather than spawning.
    assert_eq!(
        execution_count(&marker),
        1,
        "the second verify command shares the first's cache key and must have been replayed"
    );

    let stored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&artifacts[0]).expect("read artifact"))
            .expect("the artifact is JSON");
    assert_eq!(
        stored.get("cacheKey").and_then(serde_json::Value::as_str),
        artifacts[0]
            .file_stem()
            .and_then(|s| s.to_str()),
        "the artifact is filed under its own cache key (acceptance.ts:1102,1117)"
    );
    assert_eq!(
        stored
            .get("result")
            .and_then(|r| r.get("status"))
            .and_then(serde_json::Value::as_str),
        Some("passed")
    );
}

/// The `artifacts: false` half — SUBA-041's flag reaching `extension.rs`'s
/// `artifacts_dir: art_cfg.enabled.then(|| art_dir.clone())`. Turning the quadruple off must turn
/// verify memoization off with it (pi's `artifactsDir: artifactsEnabled ? getArtifactsDir(...) :
/// undefined`, `api/preflight.ts:288`), so the two commands each execute for real and nothing is
/// recorded anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn artifacts_false_disarms_verify_memoization_along_with_the_quadruple() {

    let work_dir = tempfile::tempdir().expect("tempdir");
    let home_dir = tempfile::tempdir().expect("tempdir for CYRUP_HOME");
    init_repo(work_dir.path());
    write_fixture_persona(work_dir.path(), "worker");
    let marker_dir = tempfile::tempdir().expect("marker dir");
    let marker = marker_dir.path().join("runs.log");

    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": message_end_line("G80_NO_ARTIFACTS: done") }],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();

    // `project_artifacts_dir`, NOT `temp_artifacts_dir`: `ArtifactDirPreference::default()` is
    // `Project` — pi's `DEFAULT_ARTIFACT_CONFIG.dir = "project"` (`src/shared/types.ts:1796-1798`
    // @v0.43.0) — so a subagent-tool run with the default config writes
    // `<cwd>/.cyrup-subagents/artifacts`. SUBA-048 moved that default onto pi's and these two
    // sites were never updated: the presence assertion below went red, and the sibling
    // `artifacts_false_...` ABSENCE assertion went vacuous — it was looking in a directory the
    // run would not have written to even with artifacts enabled.
    let art_dir = project_artifacts_dir(work_dir.path());
    let extension = SubagentsExtension::with_config_and_cwd(
        // SUBA-083: asserts `artifacts: false` disarms verify memoization on a completed run.
        SubagentExtensionConfig {
            // Named here rather than exported; these runs settle in the FOREGROUND, which is the
            // path `spawn_command` reaches.
            spawn_command: Some(SpawnCommand {
                binary: fixture,
                base_args: vec![
                    "--fixture-script".to_string(),
                    script_path.display().to_string(),
                ],
            }),
            roots: Roots::sandboxed(home_dir.path()),
            async_by_default: false,
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    );
    let result = extension
        .subagent_tool()
        .execute(
            ToolCallId::from("g80noart"),
            serde_json::json!({
                "agent": "worker",
                "task": "summarize the repository layout",
                "artifacts": false,
                "acceptance": duplicate_verify_policy(&marker),
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    result.expect("the run must COMPLETE");

    assert!(
        memo_artifacts(&art_dir).is_empty(),
        "`artifacts: false` must write no memo artifact at all; found: {:?}",
        memo_artifacts(&art_dir)
    );
    assert_eq!(
        execution_count(&marker),
        2,
        "with memoization disarmed BOTH verify commands really execute"
    );
}

fn fixture_persona(name: &str) -> ResolvedAgentPersona {
    ResolvedAgentPersona {
        name: name.to_string(),
        model: Some(ModelId::from("fixture-model")),
        fallback_models: Vec::new(),
        thinking: None,
        system_prompt_mode: SystemPromptMode::Replace,
        system_prompt_body: String::new(),
        tools: None,
        extensions: None,
        subagent_only_extensions: Vec::new(),
        output: None,
        inherit_project_context: false,
        inherit_skills: true,
        skills: Vec::new(),
        completion_guard: Some(false),
        max_subagent_depth: None,
        default_context: None,
        memory: None,
        tool_budget: None,
        runner: None, // SUBA-074: the native child, as before
    }
}

fn single_step(agent: &str, task: &str, acceptance: serde_json::Value) -> SingleStepSpec {
    SingleStepSpec {
        skills: Some(Vec::new()),
        session_dir: None,
        agent: agent.to_string(),
        task: task.to_string(),
        cwd: None,
        model: None,
        tools: None,
        extensions: None,
        session_file: None,
        max_depth_override: None,
        structured_output_schema: None,
        output: None,
        output_path: None,
        output_mode: None,
        reads: None,
        acceptance: Some(acceptance),
        context: None,
        agent_scope: None,
    }
}

fn runner_config(
    dir: &Path,
    run_id: &str,
    artifacts_dir: Option<PathBuf>,
    step: SingleStepSpec,
) -> RunnerConfig {
    RunnerConfig {
        turn_budget: None,
        permission_rules: None, // SUBA-073: no policy — the pre-field behaviour
        // SUBA-021: pi's `usageBudget` is an OPTIONAL param — upstream has no default budget, so a
        // call that does not ask for one runs unbudgeted. This fixture asks for none.
        usage_budget: None,
        timeout_ms: None,
        deadline_at_ms: None,
        share: None,
        artifacts_dir,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::foreground(),
        run_id: RunId::from_token(run_id.to_string()),
        mode: RunMode::Single,
        steps: vec![RunnerStep::SingleStep(step)],
        cwd: dir.to_path_buf(),
        session_file: None,
        session_id: None,
        global_concurrency_limit: 20,
        worktree_base_dir: None,
        max_subagent_depth: 2,
        async_root: dir.join("async"),
        results_dir: dir.join("results"),
        resolved_agents: [("worker".to_string(), fixture_persona("worker"))]
            .into_iter()
            .collect(),
        original_task: String::new(),
        chain_dir: None,
        orchestrator_intercom_target: None,
        inherited_session_model: None,
        nested_route: None,
        nested_self: None,
        dynamic_fanout_max_items: None,
        model_scope: None,
        control: None,
        include_progress: None,
    }
}

async fn run_hop2(dir: &Path, config: RunnerConfig) {
    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": message_end_line("G80_HOP2: done") }],
        "exit_code": 0
    });
    let script_path = dir.join("script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");
    let fixture = fixture_binary_path();

    let async_root = dir.join("async");
    let results_dir = dir.join("results");
    tokio::fs::create_dir_all(&async_root).await.expect("mkdir async_root");
    tokio::fs::create_dir_all(&results_dir).await.expect("mkdir results_dir");
    let run_paths = RunPaths::for_run(&async_root, &results_dir, &config.run_id);
    tokio::fs::create_dir_all(&run_paths.run_dir).await.expect("mkdir run_dir");
    let cfg_path = run_paths.run_dir.join("runner-config.json");
    write_atomic_json(&cfg_path, &config).await.expect("write runner config");

    // Driving the runner IN-PROCESS, so the fixture is handed down directly.
    let outcome = run_with(
        &cfg_path,
        &run_paths,
        RunnerOverrides {
            spawn_command: Some(SpawnCommand {
                binary: fixture,
                base_args: vec!["--fixture-script".to_string(), script_path.display().to_string()],
            }),
            ..Default::default()
            },
    )
    .await;
    outcome.expect("run() itself never returns Err");
}

/// G80 LIVE WIRING on the BACKGROUND hop — `background/runner_main.rs`'s
/// `artifacts_dir: self.artifacts_dir.clone().filter(|_| self.artifact_config.enabled)`, pi's
/// `artifactsDir: ctx.artifactsDir` on the async runner's own `evaluateAcceptance` call
/// (`runs/background/subagent-runner.ts:1638-1639` @v0.43.0).
///
/// A background step's `verify[]` results are memoized under the SAME
/// `<artifactsDir>/acceptance/verify/<runId>/` tree the foreground path uses, and an absent
/// `artifacts_dir` disarms it — pi's own two-term gate (`subagent-runner.ts:1192`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_background_hop_memoizes_its_steps_verify_commands() {

    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let artifacts_dir = dir.path().join("arts");
    let marker_dir = tempfile::tempdir().expect("marker dir");
    let marker = marker_dir.path().join("runs.log");

    run_hop2(
        dir.path(),
        runner_config(
            dir.path(),
            "memohop2run1",
            Some(artifacts_dir.clone()),
            single_step("worker", "do the thing", duplicate_verify_policy(&marker)),
        ),
    )
    .await;

    let artifacts = memo_artifacts(&artifacts_dir);
    assert_eq!(
        artifacts.len(),
        1,
        "the hop-2 runner must thread its `artifacts_dir`/`run_id` into the acceptance gate; \
         found: {artifacts:?} under {}",
        artifacts_dir.display()
    );
    assert!(
        artifacts[0].starts_with(artifacts_dir.join("acceptance/verify/memohop2run1")),
        "the memo is scoped by THIS run's id (acceptance.ts:1102): {artifacts:?}"
    );
    assert_eq!(
        execution_count(&marker),
        1,
        "the second, identically-keyed verify command was replayed, not re-run"
    );
}

/// The disarming half on the same hop: `artifacts: false` reaches hop 2 as `artifacts_dir: None`
/// (`RunnerConfig::artifacts_dir`'s own doc), which must leave verify memoization off.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_background_hop_writes_no_memo_when_the_run_disabled_artifacts() {

    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let artifacts_dir = dir.path().join("arts");
    let marker_dir = tempfile::tempdir().expect("marker dir");
    let marker = marker_dir.path().join("runs.log");

    run_hop2(
        dir.path(),
        runner_config(
            dir.path(),
            "memohop2run2",
            None,
            single_step("worker", "do the thing", duplicate_verify_policy(&marker)),
        ),
    )
    .await;

    assert!(
        !artifacts_dir.exists(),
        "an `artifacts_dir: None` run must write nothing at all under {}",
        artifacts_dir.display()
    );
    assert_eq!(
        execution_count(&marker),
        2,
        "with no memo context BOTH verify commands really execute"
    );
}

/// The SECOND term of the same gate, which the test directly above cannot reach.
///
/// `background/runner_main.rs`'s `self.artifacts_dir.clone().filter(|_| self.artifact_config
/// .enabled)` is a two-term gate, and an `artifacts_dir: None` run short-circuits on the FIRST
/// term: the `.filter(…)` never runs, so deleting it entirely leaves that test green. Only a run
/// carrying a REAL `artifacts_dir` alongside `artifact_config.enabled == false` distinguishes the
/// two, and this is that run — same fixture, same duplicate verify policy, only the config pair
/// changed.
///
/// The pairing is upstream's, at the boundary that BUILDS the runner's ctx rather than at the
/// runner: `artifactsDir: artifactConfig.enabled ? artifactsDir : undefined`
/// (`runs/background/async-execution.ts:1037`, and again at `:1454` for the parallel result mode)
/// — the async runner itself then passes `artifactsDir: ctx.artifactsDir` to `evaluateAcceptance`
/// unconditionally (`runs/background/subagent-runner.ts:1638-1639` @v0.43.0), because by then a
/// disabled config has already erased the directory. cyrup carries both fields all the way to
/// hop 2 in `RunnerConfig`, so it re-applies that same `enabled ? dir : undefined` here; drop it
/// and `artifacts: false` would silently keep writing memo artifacts under a directory the run was
/// told not to use.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_background_hop_writes_no_memo_when_the_config_is_disabled_despite_a_real_dir() {

    let dir = tempfile::tempdir().expect("tempdir");
    init_repo(dir.path());
    let artifacts_dir = dir.path().join("arts");
    let marker_dir = tempfile::tempdir().expect("marker dir");
    let marker = marker_dir.path().join("runs.log");

    // A REAL directory on the first term — so the run reaches the second one — with the config
    // that `artifacts: false` produces. `foreground()` (what `runner_config` installs) has every
    // include flag on, so nothing but `enabled` is holding the write back.
    let mut config = runner_config(
        dir.path(),
        "memohop2run3",
        Some(artifacts_dir.clone()),
        single_step("worker", "do the thing", duplicate_verify_policy(&marker)),
    );
    config.artifact_config.enabled = false;

    run_hop2(dir.path(), config).await;

    let artifacts = memo_artifacts(&artifacts_dir);
    assert!(
        artifacts.is_empty(),
        "`artifact_config.enabled == false` must disarm verify memoization even though \
         `artifacts_dir` is a real path; found: {artifacts:?} under {}",
        artifacts_dir.display()
    );
    assert_eq!(
        execution_count(&marker),
        2,
        "with memoization disarmed BOTH verify commands really execute — a count of 1 means the \
         second was replayed from a memo this run was never allowed to write"
    );
    // The quadruple shares the gate, so the disabled run leaves the directory untouched entirely.
    assert!(
        !artifacts_dir.exists(),
        "a disabled artifact config must create nothing under {}",
        artifacts_dir.display()
    );
}
