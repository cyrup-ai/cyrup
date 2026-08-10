//! Integration test: closing C19 — a FOREGROUND single run streams live progress through the host
//! [`cyrup_core::ToolUpdateSink`] as its child's NDJSON stdout arrives, instead of surfacing zero
//! progress until completion (`extension.rs::run_foreground_streaming` -> the C19 live sink; pi
//! `runs/foreground/execution.ts:805-826` `onUpdate`/`fireUpdate`).
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

use cyrup_core::{CancelToken, ToolUpdate, ToolUpdateSink};
use cyrup_ext_subagents::background::RunMode;
use cyrup_ext_subagents::discovery::types::AgentReadScope;
use cyrup_ext_subagents::extension::{
    ForegroundRunRequest, SingleRunOverrides, SubagentExecutor,
};
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
    let (result, _run_id) = tokio::time::timeout(
        Duration::from_secs(15),
        executor.run_foreground_streaming(
            ForegroundRunRequest {
                // SUBA-041: this test exercises live progress, not the per-call override surface.
                overrides: SingleRunOverrides::default(),
                cwd: dir.path(),
                agent_name: "streamtest",
                task: "Research the topic",
                agent_scope: AgentReadScope::Both,
                context: None,
                model_override: None,
                timeout_ms: None,
                cancel: CancelToken::new(),
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

/// Regression proof for the pi-parity fix threading [`ForegroundRunRequest::cancel`] into
/// [`cyrup_ext_subagents::exec::RunOptions::cancel`] (pi `execute(id, params, signal, ...)`,
/// `extension/index.ts:498-500` -> `executeSubagentCollapsed:378-381` -> `executor.execute`):
/// aborting the host tool call must drive the running child through the REAL cancellation race
/// (`drive_attempt`'s `cancel.cancelled()` arm), not be silently dropped in favor of a fresh,
/// never-cancelled token.
///
/// Before the fix, `run_foreground_impl` minted `cancel: CancelToken::new()` itself and had no
/// field on [`ForegroundRunRequest`] to receive a caller-supplied token at all — an ALREADY
/// CANCELLED token passed in here would have been silently ignored and the fixture's scripted
/// 10-second sleep would run to completion, so this test's wall-clock assertion below would fail
/// (or the 15s outer timeout would trip) against the pre-fix code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn foreground_run_honors_an_already_cancelled_host_token() {
    let _guard = ENV_MUTATION_LOCK.lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("home tempdir");

    let agents_dir = dir.path().join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir agents dir");
    std::fs::write(
        agents_dir.join("canceltest.md"),
        "---\nname: canceltest\ndescription: fixture cancellation persona\nmodel: fixture-model\n\
         systemPromptMode: replace\ntools: read\n---\nYou are a fixture agent.\n",
    )
    .expect("write persona");

    // A script that sleeps far longer than any sane cancellation reaction time before ever
    // emitting `agent_end` — if the host cancel token is honored, the run settles in well under a
    // second; if it is dropped (pre-fix), the run instead blocks for the full 10 seconds.
    let script = serde_json::json!({
        "steps": [
            { "kind": "emit", "line": "{\"type\":\"agent_start\"}" },
            { "kind": "sleep_ms", "ms": 10_000 },
            { "kind": "emit", "line": "{\"type\":\"agent_end\"}" },
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
        std::env::set_var("CYRUP_HOME", home.path());
    }

    let on_update: ToolUpdateSink = Box::new(|_u: ToolUpdate| {});

    // The host's own cancellation token, ALREADY cancelled before the run even starts — modeling a
    // turn-abort that raced ahead of the tool call reaching the executor.
    let cancel = CancelToken::new();
    cancel.cancel();

    let executor = SubagentExecutor::new();
    let started = std::time::Instant::now();
    let (result, _run_id) = tokio::time::timeout(
        Duration::from_secs(15),
        executor.run_foreground_streaming(
            ForegroundRunRequest {
                // SUBA-041: this test exercises live progress, not the per-call override surface.
                overrides: SingleRunOverrides::default(),
                cwd: dir.path(),
                agent_name: "canceltest",
                task: "Research the topic",
                agent_scope: AgentReadScope::Both,
                context: None,
                model_override: None,
                timeout_ms: None,
                cancel,
            },
            on_update,
        ),
    )
    .await
    .expect("an honored cancel must settle the run long before the 15s outer timeout")
    .expect("a cancelled run still resolves to a terminal SingleResult, not an Err");
    let elapsed = started.elapsed();

    // SAFETY: scoped cleanup under the same mutex-held critical section.
    unsafe {
        std::env::remove_var("CYRUP_SUBAGENT_BINARY");
        std::env::remove_var("CYRUP_SUBAGENT_FIXTURE_SCRIPT");
        std::env::remove_var("CYRUP_HOME");
    }

    assert!(
        elapsed < Duration::from_secs(5),
        "an already-cancelled host token must abort the child almost immediately, well before \
         the fixture's scripted 10s sleep would otherwise elapse; took {elapsed:?} instead \
         (result: {result:?})"
    );
    assert_ne!(
        result.exit_code, 0,
        "a cancelled run must NOT report the fixture's scripted clean exit_code=0, since the \
         child was terminated mid-sleep rather than allowed to run its `agent_end` step: {result:?}"
    );
}
