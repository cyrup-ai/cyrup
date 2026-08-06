//! SUBA-041 integration test: the SINGLE-mode `output` / `outputMode` / `skill` / `acceptance` /
//! `share` / `sessionDir` / `artifacts` overrides the `subagent` tool advertises are actually
//! HONORED by a real foreground run, instead of being rejected at dispatch.
//!
//! Upstream reference: `pi-subagents/src/runs/foreground/subagent-executor.ts` @v0.34.0
//! `runSinglePath` — `:2788-2791` (skill/output/outputMode), `:2882-2896` (output path resolution +
//! `file-only` validation + skill lowering), `:2962` (acceptance), `:3354/:3387-3401`
//! (share/artifacts/sessionDir) — plus `runs/shared/single-output.ts:11-34,73-83` for the
//! normalization and saved-output-reference shapes.
//!
//! No mocking (this crate's standing convention): the run spawns the REAL `cyrup-subagent-fixture`
//! binary as a genuine OS subprocess through `CYRUP_SUBAGENT_BINARY`, resolves a REAL persona `.md`
//! through the REAL discovery pipeline, and drives the REAL `SubagentTool::execute` ->
//! `route_single` -> `run_foreground_streaming` -> `exec::run_sync` path.
//!
//! Gated on the `test-fixtures` Cargo feature, matching every sibling integration test here.

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;

use tokio::sync::Mutex;

use cyrup_core::{CancelToken, Content, Tool, ToolCallId};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Serializes every test that mutates process-global env — mirrors every sibling integration test.
static ENV_MUTATION_LOCK: Mutex<()> = Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

fn write_fixture_persona(cwd: &std::path::Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the SUBA-041 test\n\
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
                "input": 3, "output": 2, "cacheRead": 0, "cacheWrite": 0,
                "totalTokens": 5,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string()
}

fn tool_result_text(result: &cyrup_core::ToolResult) -> String {
    result
        .content
        .iter()
        .find_map(|c| match c {
            Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// Every file under `dir`, recursively, as absolute paths.
fn walk(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// SUBA-041's stated acceptance criterion, verbatim: `{agent, task, output: "report.md",
/// outputMode: "file-only"}` must COMPLETE, with the output written to a file and only a concise
/// file reference returned inline — pi's `finalizeSingleOutput` + `formatSavedOutputReference`
/// (`single-output.ts:73-83`) behavior for `outputMode: "file-only"`.
///
/// Against pre-SUBA-041 code this call never spawned anything at all: `route_single` refused it
/// with `subagent SINGLE mode does not yet support the following param(s): output, outputMode`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_mode_output_and_output_mode_write_a_file_and_return_a_concise_reference() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir for the fixture persona + cwd");
    let home_dir = tempfile::tempdir().expect("real tempdir to isolate CYRUP_HOME artifacts");
    write_fixture_persona(work_dir.path(), "worker");

    const CHILD_OUTPUT: &str = "SUBA041_FILE_ONLY_PAYLOAD: the real child produced this";
    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": message_end_line(CHILD_OUTPUT) }],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation for the duration of this one test (Rust 2024
    // requires `unsafe` for `set_var`; this integration file is a separate compilation unit from
    // the crate's `#![forbid(unsafe_code)]` lib, exactly like every sibling test).
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
        std::env::set_var("CYRUP_HOME", home_dir.path());
    }

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let result = extension
        .subagent_tool()
        .execute(
            ToolCallId::from("suba041"),
            serde_json::json!({
                "agent": "worker",
                "task": "do the trivial thing",
                "output": "report.md",
                "outputMode": "file-only"
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        std::env::remove_var("CYRUP_HOME");
    }

    let result = result.expect("the run must COMPLETE, not be refused at dispatch");
    let text = tool_result_text(&result);

    // (1) The file was actually written. `output: "report.md"` is relative, so pi resolves it
    // against the run's own scoped output base dir (`<artifactsDir>/outputs/<runId>`,
    // `subagent-executor.ts:2203-2207`) — under the isolated CYRUP_HOME, never the user's cwd.
    let mut files = Vec::new();
    walk(home_dir.path(), &mut files);
    let report = files
        .iter()
        .find(|p| p.file_name().and_then(|n| n.to_str()) == Some("report.md"))
        .unwrap_or_else(|| panic!("report.md must exist somewhere under the run's output base dir; found: {files:?}"));
    let written = std::fs::read_to_string(report).expect("read report.md");
    assert!(
        written.contains(CHILD_OUTPUT),
        "the child's output must be persisted to the file, got: {written:?}"
    );
    assert!(
        !report.starts_with(work_dir.path()),
        "a relative `output` must NOT land in the run cwd (pi resolves it against the scoped \
         output base dir): {report:?}"
    );

    // (2) `outputMode: "file-only"` returns ONLY the concise reference inline — pi's
    // `formatSavedOutputReference` message, with the full payload deliberately absent.
    assert!(
        text.contains("Output saved to:") && text.contains("Read this file if needed."),
        "file-only must return pi's saved-output reference, got: {text:?}"
    );
    assert!(
        text.contains("report.md"),
        "the reference must name the file, got: {text:?}"
    );
    assert!(
        !text.contains(CHILD_OUTPUT),
        "file-only must NOT also inline the full output, got: {text:?}"
    );
}

/// SUBA-041, the `inline` half of the same wiring: with `outputMode` omitted (pi's `params.outputMode
/// ?? "inline"`, `subagent-executor.ts:2791`) the file is still written AND the full output is still
/// delivered inline alongside the reference — proving the two modes genuinely differ rather than the
/// param being accepted and ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn single_mode_output_without_file_only_still_inlines_the_full_output() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let work_dir = tempfile::tempdir().expect("real tempdir");
    let home_dir = tempfile::tempdir().expect("real tempdir for CYRUP_HOME");
    write_fixture_persona(work_dir.path(), "worker");

    const CHILD_OUTPUT: &str = "SUBA041_INLINE_PAYLOAD: still delivered inline";
    let script = serde_json::json!({
        "steps": [{ "kind": "emit", "line": message_end_line(CHILD_OUTPUT) }],
        "exit_code": 0
    });
    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, script.to_string()).expect("write fixture script");

    let fixture = fixture_binary_path();
    // SAFETY: see the sibling test above.
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
        std::env::set_var("CYRUP_HOME", home_dir.path());
    }

    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    let result = extension
        .subagent_tool()
        .execute(
            ToolCallId::from("suba041-inline"),
            serde_json::json!({
                "agent": "worker",
                "task": "do the trivial thing",
                "output": "notes.md",
                // SUBA-041's other wired overrides ride along to prove they are accepted too:
                // `artifacts: false` (pi `enabled: params.artifacts !== false`), an explicit
                // `skill: false` (pi's "no skills at all" form), and an explicit acceptance level.
                "artifacts": false,
                "skill": false,
                "acceptance": "none"
            }),
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        std::env::remove_var("CYRUP_HOME");
    }

    let result = result.expect("the run must COMPLETE, not be refused at dispatch");
    let text = tool_result_text(&result);
    assert!(
        text.contains(CHILD_OUTPUT),
        "inline mode must still deliver the full output, got: {text:?}"
    );
    assert!(
        text.contains("Output saved to:"),
        "inline mode appends the saved-output reference, got: {text:?}"
    );

    let mut files = Vec::new();
    walk(home_dir.path(), &mut files);
    assert!(
        files
            .iter()
            .any(|p| p.file_name().and_then(|n| n.to_str()) == Some("notes.md")),
        "the output file must still be written in inline mode; found: {files:?}"
    );
    // `artifacts: false` (pi `subagent-executor.ts:3387-3390`) suppresses the T6 quadruple, so no
    // `*_input.md`/`*_output.md`/`*_meta.json` companion is written for this run.
    assert!(
        !files
            .iter()
            .any(|p| p.to_string_lossy().ends_with("_input.md")),
        "artifacts: false must suppress the artifact quadruple; found: {files:?}"
    );
}
