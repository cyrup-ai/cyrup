//! TOOL-054 / TOOL-058 on the production path: the model calls the REAL built-in `bash` and `read`
//! through the session's agent loop, and the settled results carry pi v1.1.0's structured half.
//!
//! The model never sees `structuredContent`, so the observation points are the persisted
//! `ToolResultMessage` (what the model and the session file get) and the `tool_execution_end`
//! event's `result`, which is `finalized.result` verbatim (`emitToolExecutionEnd`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::{AgentSessionEvent, SessionBuilder, SessionConfig};
use cyrup_core::{Content, StopReason};
use cyrup_provider::Provider;
use cyrup_provider::faux::{
    FauxProvider, FauxResponseStep, faux_assistant_message, faux_text, faux_tool_call,
};
use serde_json::{Value, json};
use tempfile::TempDir;

/// Drive one model-issued call of the built-in `tool` with `args` in a fresh session whose cwd
/// holds `files`; return the persisted tool result and the `result` of its `tool_execution_end`.
async fn run(
    tool: &'static str,
    args: Value,
    files: &[(&str, &[u8])],
) -> (cyrup_agent::ToolResultMessage, Value) {
    let tmp = TempDir::new().unwrap();
    let cwd: PathBuf = tmp.path().join("project");
    let agent_dir = tmp.path().join("agent");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&agent_dir).unwrap();
    for (name, bytes) in files {
        std::fs::write(cwd.join(name), bytes).unwrap();
    }

    let faux = Arc::new(FauxProvider::new());
    faux.set_response_steps(vec![
        FauxResponseStep::factory(move |_ctx, _o, _s, _m| {
            faux_assistant_message(
                vec![faux_tool_call(tool.to_string(), args.clone())],
                StopReason::ToolUse,
            )
        }),
        FauxResponseStep::factory(|_ctx, _o, _s, _m| {
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop)
        }),
    ]);
    let mut cfg = SessionConfig::new(cwd, agent_dir);
    cfg.trust_override = Some(true);
    cfg.no_extensions = true;
    let session = SessionBuilder::new(faux as Arc<dyn Provider>, cfg)
        .build()
        .await
        .unwrap()
        .into_shared();
    session.set_active_tools_by_name(&[tool.to_owned()]).await;

    let ends: Arc<Mutex<Vec<Value>>> = Arc::default();
    let mut stream = session.subscribe();
    let sink = Arc::clone(&ends);
    tokio::spawn(async move {
        use futures::StreamExt as _;
        while let Some(event) = stream.next().await {
            if let AgentSessionEvent::ToolExecutionEnd { result, .. } = event {
                sink.lock().unwrap().push(result);
            }
        }
    });

    let _ = session.prompt("go").await.unwrap();
    session.wait_for_idle().await;
    let started = std::time::Instant::now();
    while ends.lock().unwrap().is_empty() && started.elapsed() < std::time::Duration::from_secs(10)
    {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let result = session
        .agent_messages()
        .await
        .into_iter()
        .find_map(|m| match m {
            cyrup_agent::AgentMessage::ToolResult(t) if t.tool_name == tool => Some(t),
            _ => None,
        })
        .expect("a tool result");
    let end = ends.lock().unwrap().first().cloned().expect("an end event");
    (result, end)
}

fn text_of(message: &cyrup_agent::ToolResultMessage) -> String {
    message
        .content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// A non-zero exit reaches the model as the same error result it always did — `is_error`, the
/// output and `Command exited with code 3`, `details.exitCode` (ACP-141) — and the event now also
/// carries the tool's own `isError` and pi's `structuredContent` (bash.ts:389-407 @v1.1.0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failing_bash_call_settles_as_an_error_result_with_structured_content() {
    let (result, end) = run("bash", json!({ "command": "echo out; exit 3" }), &[]).await;
    assert!(result.is_error);
    assert_eq!(text_of(&result), "out\n\n\nCommand exited with code 3");
    assert_eq!(result.details.as_ref().unwrap()["exitCode"], 3);

    assert_eq!(end["isError"], true);
    assert_eq!(end["details"]["exitCode"], 3);
    let structured = &end["structuredContent"];
    assert_eq!(structured["output"], "out\n");
    assert_eq!(structured["truncated"], false);
    assert_eq!(structured["exit_code"], 3);
    assert!(structured["wall_time_seconds"].is_number(), "{structured}");
    assert!(structured.get("full_output_path").is_none(), "{structured}");
}

/// A clean exit carries the structured half and no `isError`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_clean_bash_call_carries_structured_content() {
    let (result, end) = run("bash", json!({ "command": "echo fine" }), &[]).await;
    assert!(!result.is_error);
    assert_eq!(text_of(&result), "fine\n");
    assert!(end.get("isError").is_none(), "{end}");
    assert_eq!(end["structuredContent"]["output"], "fine\n");
    assert_eq!(end["structuredContent"]["exit_code"], 0);
}

/// A text `read` settles with its model text as the structured content (read.ts:216 @v1.1.0).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_call_carries_its_text_as_structured_content() {
    let (result, end) = run(
        "read",
        json!({ "path": "notes.txt" }),
        &[("notes.txt", b"hello\n")],
    )
    .await;
    assert!(!result.is_error);
    assert_eq!(text_of(&result), "hello\n");
    assert_eq!(end["structuredContent"], "hello\n");
}
