//! SUBA-192 end to end: on a real, fully wired session, a steer the operator types while the model
//! is blocked in `bg_wait` ends the wait at once, as a NON-error `user_input` result naming the
//! work still running — instead of holding the message until the 30-minute window closes.
//!
//! Every hop is the production one. The session is a real `SessionBuilder`-assembled
//! `AgentSession` (via `cyrup-test-support`'s harness; the model's responses are scripted) with the
//! real `SubagentsExtension` loaded. The model calls `bg_wait` on a background run that is
//! `running` on disk and never finishes. The steer goes through `AgentSession::steer` — the call
//! the TUI makes for Enter while the agent streams — whose `input` event the host dispatches to
//! the extension; the extension's handler ends the open wait; the wait returns; the tool result is
//! what the session reports.
//!
//! Nothing is timed. The steer is sent only once the wait is OPEN — the call has registered with
//! the executor's set of open waits, which is where the extension's handler looks — because pi
//! drops a message that precedes the call (`wait-tool.ts:37` @ad11b7ab), and so does this port.
//! The run is seeded where the TOOL looks — the roots of the extension's own cwd, which
//! `WaitTool` resolves them from — and settled once the wait has returned, so nothing is left
//! running behind the test.
//!
//! Fixture-free, like `wait_tool_registration_integration`: nothing is spawned.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cyrup_ext::native::NativeExtension;
use cyrup_ext_subagents::background::{
    ResultFile, RunId, RunMode, RunPaths, RunState, RunStatus, run_artifact_roots_in,
};
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::identity::SessionId;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_session_svc::{AgentSessionEvent, InputSource, UserInput};
use cyrup_test_support::harness::{HarnessOptions, create_harness_with_extensions};
use cyrup_test_support::response::FauxResponse;
use futures::StreamExt;

/// A `running` background run owned by `session` — the on-disk shape `bg_wait` lists.
fn seed_running(async_root: &Path, results_dir: &Path, run_id: &RunId, session: &str) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    std::fs::create_dir_all(&paths.run_dir).expect("run dir");
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(std::process::id()));
    status.state = RunState::Running;
    status.session_id = SessionId::parse_opt(Some(session));
    std::fs::write(&paths.status, serde_json::to_string(&status).unwrap()).expect("status.json");
}

/// The run's terminal `ResultFile`, which flips it terminal for every reader.
fn settle(async_root: &Path, results_dir: &Path, run_id: &RunId) {
    let paths = RunPaths::for_run(async_root, results_dir, run_id);
    if let Some(parent) = paths.legacy_result_root.parent() {
        std::fs::create_dir_all(parent).expect("results dir");
    }
    let result = ResultFile {
        schedule_origin: None,
        id: run_id.clone(),
        run_id: run_id.clone(),
        agent: "worker".to_string(),
        mode: RunMode::Single,
        state: RunState::Complete,
        success: true,
        cwd: PathBuf::from("/tmp"),
        session_file: None,
        session_id: None,
        completion_owner_id: None,
        results: Vec::new(),
        workflow_children: None,
        workflow_receipt: None,
    };
    std::fs::write(
        &paths.legacy_result_root,
        serde_json::to_string(&result).unwrap(),
    )
    .expect("result file");
}

/// **Proven red** with `EventKind::Input` dropped from the extension's subscriptions: the steer
/// then ends nothing, the wait sleeps on toward its 30-minute window, and the 60 s guard fires.
/// The other links — the extension's `Input` arm, the wait's user-input check, the verdict's
/// non-error arm — are each proven by a single mutation against the crate's own tests
/// (`background::wait`, `tests::mid_run_input_wiring`). Opening the call's user-input entry after
/// its first `.await` instead of before is NOT caught here (the test waits for the entry), which
/// is why that ordering is argued in `execute`'s comment rather than pinned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steer_typed_during_a_blocking_bg_wait_ends_it_with_a_non_error_user_input_result() {
    let home = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let roots = Roots::sandboxed(home.path());
    let extension = Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: roots.clone(),
            ..SubagentExtensionConfig::default()
        },
        work_dir.path().to_path_buf(),
    ));
    let harness = create_harness_with_extensions(HarnessOptions {
        native_extensions: vec![extension.clone() as Arc<dyn NativeExtension>],
        responses: vec![
            FauxResponse::tool_call("bg_wait", serde_json::json!({})),
            FauxResponse::text("read the steer; the run keeps going"),
            FauxResponse::text("done"),
        ],
        ..HarnessOptions::default()
    })
    .await
    .expect("a real session with the real SubagentsExtension");
    let session_id = extension
        .executor()
        .current_session_id()
        .expect("the harness binds a live session id, which scopes the wait");
    // The tool resolves its run roots from the EXTENSION's cwd (`WaitTool::new(…, cwd, …)`), which
    // is `work_dir` here; the session's own cwd is the harness temp dir.
    let artifact_roots = run_artifact_roots_in(&roots, work_dir.path());
    let run_id = RunId::from_token("steerwait00001");
    seed_running(
        &artifact_roots.async_root,
        &artifact_roots.results_dir,
        &run_id,
        &session_id,
    );

    let waits = extension.executor().user_input_waits();
    let started = Instant::now();
    let mut stream = harness
        .session()
        .prompt("wait for the background run")
        .await
        .expect("the prompt is accepted");
    let mut events = Vec::new();
    let mut steered = false;
    let mut settled = false;
    let drive = async {
        while let Some(event) = stream.next().await {
            if !steered
                && matches!(
                    &event,
                    AgentSessionEvent::ToolExecutionStart { tool_name, .. } if tool_name == "bg_wait"
                )
            {
                // The wait registers before its first `.await`. Bounded on its own: a wait that
                // never opens — or opens and returns at once because it found nothing to wait
                // for — must fail here, not behind the 60 s guard with the stream unread.
                tokio::time::timeout(Duration::from_secs(10), async {
                    while waits.open_count() == 0 {
                        tokio::time::sleep(Duration::from_millis(5)).await;
                    }
                })
                .await
                .expect("bg_wait never held a wait open: is the seeded run where it looks?");
                harness
                    .session()
                    .steer(UserInput::text(
                        "also check the docs while it runs",
                        InputSource::Tui,
                    ))
                    .await
                    .expect("the steer is accepted mid-run");
                steered = true;
            }
            if !settled
                && matches!(
                    &event,
                    AgentSessionEvent::ToolExecutionEnd { tool_name, .. } if tool_name == "bg_wait"
                )
            {
                settle(
                    &artifact_roots.async_root,
                    &artifact_roots.results_dir,
                    &run_id,
                );
                settled = true;
            }
            events.push(event);
        }
    };
    tokio::time::timeout(Duration::from_secs(60), drive)
        .await
        .expect("the steer must end the wait; it may not sit out the 30-minute window");
    harness.session().wait_for_idle().await;

    assert!(steered, "bg_wait never started: {events:#?}");
    let (result, is_error) = events
        .iter()
        .find_map(|event| match event {
            AgentSessionEvent::ToolExecutionEnd {
                tool_name,
                result,
                is_error,
                ..
            } if tool_name == "bg_wait" => Some((result.clone(), *is_error)),
            _ => None,
        })
        .expect("bg_wait returned");
    assert!(
        !is_error,
        "a user_input yield is not an error result: {result:#}"
    );
    assert_eq!(
        result["details"]["wait"],
        serde_json::json!({
            "reason": "user_input",
            "timedOut": false,
            "activeRunIds": [run_id.as_str()],
            "activeProviderItems": [],
        }),
        "{result:#}"
    );
    assert!(
        result.to_string().contains(
            "Wait yielded because the user sent a message. Background work remains active; read \
             the message before waiting again."
        ),
        "{result:#}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(waits.open_count(), 0, "the call closed its entry on return");
}
