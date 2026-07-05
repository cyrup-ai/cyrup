//! Integration test: closing C19 — a FOREGROUND single run streams live progress through the host
//! [`cyrup_core::ToolUpdateSink`] as its child's NDJSON stdout arrives, instead of surfacing zero
//! progress until completion (`extension.rs::run_foreground_streaming` -> the C19 live sink; pi
//! `runs/foreground/execution.ts:478-499` `onUpdate`/`fireUpdate`).
//!
//! No mocking (this crate's standing convention): the run spawns the REAL `cyrup-subagent-fixture`
//! binary as a genuine OS subprocess via `CYRUP_SUBAGENT_BINARY`, the fixture emits a real scripted
//! NDJSON stream (a tool call + an assistant message), and the test asserts on the REAL typed
//! [`SubagentUpdatePayload`] details the `ToolUpdateSink` actually received DURING the run.
//!
//! Gated on `test-fixtures` (the `cyrup-subagent-fixture` `[[bin]]`'s own `required-features` gate);
//! without it this file compiles to an empty, passing test list.

#![cfg(feature = "test-fixtures")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Mutex as AsyncMutex;

use cyrup_core::{ToolUpdate, ToolUpdateSink};
use cyrup_ext_subagents::background::RunMode;
use cyrup_ext_subagents::extension::{ForegroundRunRequest, SubagentExecutor};
use cyrup_ext_subagents::tui::events::{LiveProgressStatus, SubagentUpdatePayload};

/// Serializes every test in this file that mutates process-global env (`CYRUP_SUBAGENT_BINARY`,
/// `CYRUP_SUBAGENT_FIXTURE_SCRIPT`, `CYRUP_HOME`) so `cargo test`'s concurrent execution never lets
/// two tests clobber each other's overrides mid-run.
static ENV_MUTATION_LOCK: AsyncMutex<()> = AsyncMutex::const_new(());

/// Path to the real, already-built `cyrup-subagent-fixture` binary (Cargo sets
/// `CARGO_BIN_EXE_<name>` when built with `--features test-fixtures`).
fn fixture_binary_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_cyrup-subagent-fixture"))
}

fn emit(line: String) -> serde_json::Value {
    serde_json::json!({ "kind": "emit", "line": line })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_run_streams_live_progress_through_on_update() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home tempdir");

    // A discoverable PROJECT persona under `cwd/.cyrup/agents` (so `find_nearest_project_root(cwd)`
    // resolves to `cwd` and this agent is discovered). `model: fixture-model` gives a non-empty
    // fallback ladder; the fixture binary ignores `--model` and just replays its script. `tools:
    // read` keeps the run entirely read-only (the completion-mutation guard short-circuits).
    let agents_dir = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
    std::fs::write(
        agents_dir.join("streamtest.md"),
        "---\nname: streamtest\ndescription: fixture streaming persona\nmodel: fixture-model\n\
         systemPromptMode: replace\ntools: read\n---\nYou are a fixture agent.\n",
    )
    .expect("write persona");

    // A real scripted NDJSON stream: a tool call (start+end) then an assistant message carrying
    // usage, then agent_end — exactly the events pi fires `fireUpdate` on.
    let tool_start = serde_json::json!({
        "type": "tool_execution_start", "toolCallId": "c1", "toolName": "read"
    })
    .to_string();
    let tool_end = serde_json::json!({
        "type": "tool_execution_end", "toolCallId": "c1", "toolName": "read",
        "result": "file contents", "isError": false
    })
    .to_string();
    let msg_end = serde_json::json!({
        "type": "message_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "Researched the topic."}],
            "usage": {
                "input": 30, "output": 12, "cacheRead": 0, "cacheWrite": 0, "totalTokens": 42,
                "cost": {"input": 0.0, "output": 0.0, "cacheRead": 0.0, "cacheWrite": 0.0, "total": 0.0}
            },
            "stopReason": "stop"
        }
    })
    .to_string();
    let script = serde_json::json!({
        "steps": [
            emit(r#"{"type":"agent_start"}"#.to_string()),
            emit(tool_start),
            emit(tool_end),
            emit(msg_end),
            emit(r#"{"type":"agent_end"}"#.to_string()),
        ],
        "exit_code": 0
    });
    let script_path = dir.path().join("script.json");
    std::fs::write(&script_path, script.to_string()).expect("write script");

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation — see this file's `ENV_MUTATION_LOCK` doc.
    unsafe {
        std::env::set_var("CYRUP_SUBAGENT_BINARY", &fixture);
        std::env::set_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT", &script_path);
        // Isolate user-scope discovery from the developer's real `~/.cyrup` (`dirs_home` honors
        // `CYRUP_HOME` ahead of `HOME`) so this test never depends on machine-local settings.
        std::env::set_var("CYRUP_HOME", home.path());
    }

    // The host sink: capture every `ToolUpdate` the run streams.
    let updates: Arc<Mutex<Vec<ToolUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_updates = Arc::clone(&updates);
    let on_update: ToolUpdateSink = Box::new(move |u: ToolUpdate| {
        if let Ok(mut guard) = sink_updates.lock() {
            guard.push(u);
        }
    });

    let executor = SubagentExecutor::new();
    let result = tokio::time::timeout(
        Duration::from_secs(15),
        executor.run_foreground_streaming(
            ForegroundRunRequest {
                cwd: dir.path(),
                agent_name: "streamtest",
                task: "Research the topic",
                context: None,
                model_override: None,
                timeout_ms: None,
            },
            on_update,
        ),
    )
    .await
    .expect("streaming foreground run must not hang against a fast fixture child")
    .expect("run_foreground_streaming resolves the persona and completes");

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_SUBAGENT_BINARY");
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
        std::env::remove_var("CYRUP_HOME");
    }

    assert_eq!(result.exit_code, 0, "the scripted fixture run must exit 0: {result:?}");

    let captured = updates.lock().expect("updates lock").clone();
    assert!(
        !captured.is_empty(),
        "C19: on_update MUST receive progress events DURING the foreground run, but got none"
    );

    // Every streamed update must carry the typed `SubagentUpdatePayload` under `details` (the wire
    // shape cyrup-tui deserializes for the inline surface, C20).
    let payloads: Vec<SubagentUpdatePayload> = captured
        .iter()
        .filter_map(|u| {
            u.details
                .as_ref()
                .and_then(|d| serde_json::from_value::<SubagentUpdatePayload>(d.clone()).ok())
        })
        .collect();
    assert!(
        !payloads.is_empty(),
        "at least one streamed update must carry a decodable SubagentUpdatePayload: {captured:?}"
    );

    // A live update (fired on tool_execution_start) must report the current tool + a tool count —
    // proof the child's NDJSON was folded live, not just summarized at completion.
    assert!(
        payloads.iter().any(|p| p
            .progress
            .iter()
            .any(|pr| pr.current_tool.as_deref() == Some("read") && pr.tool_count >= 1)),
        "a live progress update must report current_tool=read with tool_count>=1: {payloads:?}"
    );

    // A later update (after the assistant message_end) must reflect the turn's tokens (input+output).
    assert!(
        payloads
            .iter()
            .any(|p| p.progress.iter().any(|pr| pr.turn_count >= 1 && pr.tokens >= 42)),
        "a progress update must reflect the assistant turn (turns>=1, tokens>=42): {payloads:?}"
    );

    // Every payload is SINGLE mode and names the running persona.
    assert!(
        payloads.iter().all(|p| p.mode == RunMode::Single),
        "SINGLE-mode run must tag every payload mode=single: {payloads:?}"
    );
    assert!(
        payloads
            .iter()
            .any(|p| p.progress.iter().any(|pr| pr.agent.as_deref() == Some("streamtest"))),
        "progress entries must name the running persona: {payloads:?}"
    );

    // The final settle update flips status to Complete (pi's terminal snapshot) and carries the
    // settled SingleResult in `results`.
    assert!(
        payloads.iter().any(|p| {
            p.progress.iter().any(|pr| pr.status == LiveProgressStatus::Complete) && !p.results.is_empty()
        }),
        "a terminal settle update must carry status=complete + the settled result: {payloads:?}"
    );
}
