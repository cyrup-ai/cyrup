//! Integration test (T6): a REAL foreground subagent run leaves the full artifact quadruple on
//! disk — `<runId>_<agent>_input.md`, `_output.md`, `.jsonl`, `_meta.json` (pi
//! `runs/foreground/execution.ts:960-1074` + `shared/artifacts.ts:186-196`).
//!
//! No mocking (this crate's standing convention): the run spawns the REAL `cyrup-subagent-fixture`
//! binary as a genuine OS subprocess via `CYRUP_SUBAGENT_BINARY` (R-SA-045 tier 1), discovers a REAL
//! persona `.md` through the REAL discovery pipeline, and drives the REAL
//! `SubagentExecutor::run_foreground` -> `exec::run_sync` path the `subagent` tool itself uses. The
//! artifacts directory is isolated to a tempdir by pointing `CYRUP_HOME` at it, so the assertion
//! observes exactly the files this run produced.
//!
//! Gated on the `test-fixtures` Cargo feature (matching every other fixture-based integration test
//! in this crate) — without it the fixture binary is never built and this file compiles to an empty,
//! passing test list.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::PathBuf;

use cyrup_ext_subagents::artifacts::project_artifacts_dir;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

/// Write a trivial agent persona `.md` to `<cwd>/.cyrup/agents/<name>.md` — the exact project-scope
/// discovery root `SubagentExecutor::discovery_config` scans (mirrors `extension_end_to_end_smoke.rs`).
fn write_fixture_persona(cwd: &std::path::Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the T6 artifacts test\n\
             model: fixture/model\n---\n\nYou are a trivial test persona.\n"
        ),
    )
    .expect("write fixture persona");
}

/// One `message_end` NDJSON line the fixture writes to stdout (mirrors the sibling integration tests).
fn message_end_line(text: &str) -> String {
    serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": text}],
            "usage": {
                "input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 5,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

/// Every artifact file directly in `dir` (non-recursive), as file-name strings.
fn artifact_file_names(dir: &std::path::Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_real_foreground_run_writes_the_four_artifact_files() {
    let work_dir = tempfile::tempdir().expect("real tempdir for the fixture persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate CYRUP_HOME artifacts");
    write_fixture_persona(work_dir.path(), "worker");

    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": message_end_line("ARTIFACT_TEST_OUTPUT: the real child ran") },
        ],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();

    // Resolve the exact artifacts directory this run will write into.
    //
    // `project_artifacts_dir`, NOT `temp_artifacts_dir`. `ArtifactDirPreference::default()` is
    // `Project` — pi's own `DEFAULT_ARTIFACT_CONFIG.dir = "project"`
    // (`src/shared/types.ts:1796-1798` @v0.43.0) — so an unconfigured run writes
    // `<cwd>/.cyrup-subagents/artifacts`, not the scoped temp root. This test predates SUBA-048,
    // which moved the default onto pi's, and kept looking in the temp location; it found an empty
    // directory and blamed the run for writing nothing. `CYRUP_HOME` is still pointed at a tempdir
    // above because the background/results roots derive from it, but it no longer decides where
    // THESE files land — `work_dir` does.
    let art_dir = project_artifacts_dir(work_dir.path());

    // This run names its own binary, script and home root rather than moving three process-global
    // vars. `run_foreground` is the path `spawn_command` reaches.
    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            spawn_command: Some(SpawnCommand {
                binary: fixture,
                base_args: vec![
                    "--fixture-script".to_string(),
                    script_path.display().to_string(),
                ],
            }),
            roots: Roots::sandboxed(home_dir.path()),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    );
    let executor = extension.executor().clone();

    let result = executor
        .run_foreground(
            work_dir.path(),
            "worker",
            "do the trivial thing",
            None,
            None,
            None,
        )
        .await;

    let result = result.expect("the foreground run completes without an orchestration-level error");
    assert_eq!(
        result.exit_code, 0,
        "the fixture child exits cleanly; error: {:?}",
        result.error
    );
    assert!(
        result
            .final_output
            .as_deref()
            .unwrap_or("")
            .contains("ARTIFACT_TEST_OUTPUT"),
        "the run's final output must carry the fixture child's emitted text"
    );

    let names = artifact_file_names(&art_dir);
    assert!(
        !names.is_empty(),
        "the run must have written artifacts under {}; found none",
        art_dir.display()
    );

    // All four artifact files (the `<runId>_worker` base is random; assert one of each suffix).
    let has_suffix = |suffix: &str| names.iter().any(|n| n.ends_with(suffix));
    assert!(
        has_suffix("_input.md"),
        "missing _input.md artifact; got: {names:?}"
    );
    assert!(
        has_suffix("_output.md"),
        "missing _output.md artifact; got: {names:?}"
    );
    assert!(
        has_suffix(".jsonl"),
        "missing .jsonl artifact; got: {names:?}"
    );
    assert!(
        has_suffix("_meta.json"),
        "missing _meta.json artifact; got: {names:?}"
    );

    // The output artifact carries the delivered answer; the input artifact carries the task.
    let output_file = names.iter().find(|n| n.ends_with("_output.md")).unwrap();
    let output_body = std::fs::read_to_string(art_dir.join(output_file)).unwrap();
    assert!(
        output_body.contains("ARTIFACT_TEST_OUTPUT"),
        "the _output.md artifact must contain the child's delivered output; got: {output_body:?}"
    );
    let input_file = names.iter().find(|n| n.ends_with("_input.md")).unwrap();
    let input_body = std::fs::read_to_string(art_dir.join(input_file)).unwrap();
    assert!(
        input_body.contains("# Task for worker") && input_body.contains("do the trivial thing"),
        "the _input.md artifact must contain the task the child was given; got: {input_body:?}"
    );

    // R-SA-058: the per-attempt raw-stdout tee `run_sync` writes under `attempt_scratch_dir(cwd)`
    // (SUBA-072: `<temp_root_dir>/scratch/<cwd_key>`, never under the project tree)
    // is this run's persisted, observable child record and survives the orchestrator — exactly as
    // it does on every other spawn path in this crate (the tool single/parallel/chain fan-outs and
    // the background hop-2 runner all leave it in place; it is the observation channel
    // `tool_parallel_chain_integration`/`companions_wiring_proof` read back). It is NOT swept by the
    // foreground orchestrator: mirroring pi, which never deletes its persisted child NDJSON stream
    // and only cleans the transient `os.tmpdir()` prompt/task-overflow dir (`runs/shared/pi-args.ts:1052-1059`
    // `cleanupTempDir` @v0.64.0, `execution.ts:491`) that lives outside the working tree.
    let tee = std::fs::read_to_string(
        cyrup_ext_subagents::background::attempt_scratch_dir(work_dir.path())
            .join("attempt-0.jsonl"),
    )
    .unwrap_or_default();
    assert!(
        !tee.is_empty(),
        "the per-attempt raw-stdout tee is this run's persisted child record and must survive the \
         foreground orchestrator (it must not be swept away with the scratch dir)"
    );
    // SUBA-072: and that record lives under the crate's run-scratch root — the project working
    // tree gets NO `.cyrup-subagent-scratch/` (the pre-fix location, pi's `os.tmpdir()`-rooted
    // per-spawn scratch never touches the project: `runs/shared/pi-args.ts:787` @v0.64.0).
    assert!(
        !work_dir.path().join(".cyrup-subagent-scratch").exists(),
        "a real subagent run must leave no scratch directory in the project working tree"
    );
}
