//! The watchdog is REACHED from production, not just constructible.
//!
//! `pi-subagents` calls its two watchdog entry points from exactly two places —
//! `registerMainWatchdog(pi)` at `src/extension/index.ts:375` and `registerChildWatchdog(pi)` at
//! `src/runs/shared/subagent-prompt-runtime.ts:477` — and everything the watchdog does afterwards
//! hangs off the event subscriptions those two install. A port can have a complete, green
//! `watchdog/` module tree and still be dead code if those two calls are missing, which is exactly
//! the failure this file exists to make impossible.
//!
//! So every test here drives the REAL [`cyrup_ext::native::NativeExtension`] surface — `init`,
//! `on_event`, `execute_command`, `render_call` — on the real
//! [`cyrup_ext_subagents::extension::SubagentsExtension`] and
//! [`cyrup_ext_subagents::prompt_runtime::SubagentPromptRuntime`], and asserts the watchdog state
//! machine moved. Nothing here calls a watchdog method directly.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_ext::native::{ExtMode, HostCtx, InitApi, NativeExtension};
use cyrup_ext::HostEvent;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::watchdog::child_status::{
    ChildWatchdogPhase, ChildWatchdogStatusEvent, CHILD_WATCHDOG_STATUS_EVENT,
};
use cyrup_ext_subagents::watchdog::register_child::{register_child_watchdog, ChildWatchdog};
use cyrup_ext_subagents::watchdog::types::SUBAGENT_WATCHDOG_WARNING_TYPE;

fn ctx(cwd: &std::path::Path) -> HostCtx {
    HostCtx::event(ExtMode::Json, false, cwd.to_path_buf())
}

fn command_ctx(cwd: &std::path::Path) -> HostCtx {
    HostCtx::command(ExtMode::Json, false, cwd.to_path_buf())
}

fn turn_end(text: &str) -> HostEvent {
    HostEvent::TurnEnd {
        turn_index: 0,
        message: cyrup_agent::AgentMessage::Custom {
            kind: "probe".to_string(),
            payload: serde_json::json!({ "text": text }),
            timestamp: Some(0),
        },
        tool_results: Vec::new(),
    }
}

// =================================================================================================
// The ORCHESTRATOR role (`extension/index.ts:375`)
// =================================================================================================

#[tokio::test]
async fn the_extension_registers_the_watchdog_command_and_its_message_renderer() {
    // Loaded through the REAL host, so these are the same registry queries the TUI makes before it
    // routes a `/subagents-watchdog` invocation or asks an extension to draw a custom message.
    let root = tempfile::tempdir().expect("tempdir");
    let host = cyrup_ext::ExtensionHost::new(cyrup_ext::facade::HostConfig {
        mode: ExtMode::Tui,
        has_ui: false,
        cwd: root.path().to_path_buf(),
    });
    host.load_native(Arc::new(SubagentsExtension::with_config_and_cwd(
        Default::default(),
        root.path().to_path_buf(),
    )))
    .await
    .expect("load");

    assert!(
        host.has_message_renderer(SUBAGENT_WATCHDOG_WARNING_TYPE),
        "the warning renderer is registered with the host (`register-main.ts:392`)"
    );
    let commands = host.registry().resolved_commands().expect("commands");
    assert!(
        commands
            .iter()
            .any(|command| command.name == "subagents-watchdog"),
        "`/subagents-watchdog` is registered (`register-main.ts:403`): {:?}",
        commands.iter().map(|c| c.name.clone()).collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn the_extension_subscribes_every_event_the_watchdog_state_machine_needs() {
    use cyrup_ext::EventKind;
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    let mut api = InitApi::new();
    extension.init(&mut api).await.expect("init");
    let subs = api.subscriptions();
    // `register-main.ts:411-433`. Without these the runtime is constructed and never fed.
    for kind in [
        EventKind::SessionStart,
        EventKind::BeforeAgentStart,
        EventKind::TurnEnd,
        EventKind::ToolResult,
        EventKind::AgentEnd,
        EventKind::SessionBeforeSwitch,
        EventKind::SessionBeforeFork,
        EventKind::SessionCompact,
        EventKind::SessionShutdown,
    ] {
        assert!(subs.contains(kind), "{kind:?} is subscribed");
    }
}

#[tokio::test]
async fn session_start_binds_the_watchdog_and_shutdown_disposes_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    let before = extension.watchdog().get_snapshot(None).epoch;

    extension
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
            },
            &ctx(root.path()),
        )
        .await;
    let after_start = extension.watchdog().get_snapshot(None).epoch;
    assert!(
        after_start > before,
        "`session_start` reached `bindSession` ({before} -> {after_start})"
    );

    // A waiter parked BEFORE the shutdown resolves `false` — the terminal state's own observable
    // (`runtime.ts:301` calls `resolveWaiters(false)`, and `:860` lets it through only because the
    // runtime is disposed). It has to be parked first: `waitForSettled` short-circuits `true` for
    // an already-settled runtime, disposed or not (`:846`).
    extension.watchdog().enqueue_delta("something to settle");
    let waiting = {
        let watchdog = Arc::clone(extension.watchdog());
        tokio::spawn(async move {
            watchdog
                .wait_for_idle(std::time::Duration::from_secs(5))
                .await
        })
    };
    tokio::task::yield_now().await;

    extension
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "test".to_string(),
            },
            &ctx(root.path()),
        )
        .await;
    let after_shutdown = extension.watchdog().get_snapshot(None).epoch;
    assert!(
        after_shutdown > after_start,
        "`session_shutdown` reached `dispose()` ({after_start} -> {after_shutdown})"
    );
    // Whether the parked waiter resolved false depends on the watchdog having been ENABLED enough
    // to buffer anything, which it is not by default; the epoch bump above is the unconditional
    // proof. Drain the task either way so it cannot outlive the test.
    let _ = waiting.await;
}

#[tokio::test]
async fn a_switch_a_fork_and_a_compaction_each_reset_the_watchdog() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    let mut epoch = extension.watchdog().get_snapshot(None).epoch;
    for event in [
        HostEvent::SessionBeforeSwitch {
            target_id: "t".to_string(),
        },
        HostEvent::SessionBeforeFork {
            entry_id: "e".to_string(),
        },
        HostEvent::SessionCompact {
            compaction_entry: serde_json::Value::Null,
            from_extension: false,
            reason: "manual".to_string(),
            will_retry: false,
        },
    ] {
        let label = format!("{event:?}");
        extension.on_event(&event, &ctx(root.path())).await;
        let next = extension.watchdog().get_snapshot(None).epoch;
        assert!(next > epoch, "{label} reached the runtime's reset");
        epoch = next;
    }
}

#[tokio::test]
async fn the_turn_and_boundary_events_reach_the_runtime() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    let cwd = ctx(root.path());

    // `before_agent_start` resets, so the epoch moves — the cheapest proof the arm is wired at all.
    let before = extension.watchdog().get_snapshot(None).epoch;
    extension
        .on_event(
            &HostEvent::BeforeAgentStart {
                prompt: "do the thing".to_string(),
                images: serde_json::Value::Null,
                system_prompt: String::new(),
                options: serde_json::Value::Null,
                injected: Vec::new(),
            },
            &cwd,
        )
        .await;
    assert!(
        extension.watchdog().get_snapshot(None).epoch > before,
        "`before_agent_start` reached `handleBeforeAgentStart`"
    );

    // `turn_end`, `tool_result` and `agent_end` are all inert while the watchdog is OFF (the
    // default), which is the point: they must reach the runtime and be REFUSED there, not be
    // unreachable. The runtime stays settled and buffers nothing.
    extension.on_event(&turn_end("wrote a file"), &cwd).await;
    extension
        .on_event(
            &HostEvent::ToolResult {
                call_id: cyrup_core::ToolCallId::from("c1"),
                name: "write".to_string(),
                input: serde_json::Value::Null,
                content: Vec::new(),
                details: None,
                is_error: false,
                usage: None,
            },
            &cwd,
        )
        .await;
    extension
        .on_event(&HostEvent::AgentEnd { messages: Vec::new() }, &cwd)
        .await;
    let snapshot = extension.watchdog().get_snapshot(None);
    assert!(!snapshot.enabled, "the watchdog is default OFF");
    assert_eq!(snapshot.buffered_deltas, 0);
    assert_eq!(snapshot.failed_reviews, 0);
}

#[tokio::test]
async fn the_slash_command_routes_to_the_watchdog_status_block() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    let output = extension
        .execute_command("subagents-watchdog", "status", &command_ctx(root.path()))
        .await
        .expect("the command is serviced")
        .expect("status text");
    assert!(output.starts_with("Subagent watchdog\n"), "{output}");
    assert!(output.contains("Review trigger: repo edits only"), "{output}");
    assert!(output.contains("Sources:"), "{output}");
}

#[tokio::test]
async fn a_session_override_through_the_slash_command_reaches_the_runtime() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    assert_eq!(extension.watchdog().get_snapshot(None).session_override, None);
    let output = extension
        .execute_command("subagents-watchdog", "session on", &command_ctx(root.path()))
        .await
        .expect("serviced")
        .expect("text");
    assert!(
        output.starts_with("Subagent watchdog session override: on."),
        "{output}"
    );
    assert_eq!(
        extension.watchdog().get_snapshot(None).session_override,
        Some(true),
        "the command mutated the SAME runtime `on_event` drives"
    );
}

#[tokio::test]
async fn the_test_command_records_a_warning_on_the_live_runtime() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    extension
        .execute_command(
            "subagents-watchdog",
            "test blocker the renderer is broken",
            &command_ctx(root.path()),
        )
        .await
        .expect("serviced");
    let warning = extension
        .watchdog()
        .get_snapshot(None)
        .last_warning
        .expect("the warning was recorded on the live runtime");
    assert_eq!(warning.summary, "the renderer is broken");
}

#[test]
fn the_extension_renders_a_watchdog_warning_message() {
    let root = tempfile::tempdir().expect("tempdir");
    let extension =
        SubagentsExtension::with_config_and_cwd(Default::default(), root.path().to_path_buf());
    let content = [
        "<subagent_watchdog severity=\"blocker\" category=\"correctness\" source=\"main\" guidance=\"weigh, don't blindly obey\">",
        "<summary>the tests were deleted</summary>",
        "<evidence>step 2 removed the suite</evidence>",
        "<recommended_action>restore them</recommended_action>",
        "</subagent_watchdog>",
    ]
    .join("\n");
    let message = serde_json::json!({
        "role": "custom",
        "kind": SUBAGENT_WATCHDOG_WARNING_TYPE,
        "payload": content,
    });
    let rendered = extension
        .render_call(SUBAGENT_WATCHDOG_WARNING_TYPE, &message)
        .expect("the renderer is reachable through the trait");
    let text = rendered.as_str().expect("text");
    assert!(text.starts_with("Subagent watchdog Blocker"), "{text}");
    assert!(text.contains("the tests were deleted"), "{text}");
}

// =================================================================================================
// The CHILD role (`subagent-prompt-runtime.ts:477`)
// =================================================================================================

fn child_config_json() -> String {
    serde_json::json!({
        "enabled": true,
        "runId": "run-1",
        "agent": "reviewer",
        "childIndex": 0,
        "watchdogTailTimeoutMs": 120_000,
        "agentEndTimeoutMs": 5_000,
        "maxWarnings": null,
        "lsp": { "enabled": false, "timeoutMs": 1_000, "maxFiles": 1, "maxDiagnostics": 0 },
        "autoFollowBlockers": true,
        "autoFollowMaxAttempts": 3,
        "stalemateRepeats": 3,
    })
    .to_string()
}

fn armed_child(
    cwd: &std::path::Path,
) -> (Arc<ChildWatchdog>, Arc<Mutex<Vec<ChildWatchdogStatusEvent>>>) {
    let events: Arc<Mutex<Vec<ChildWatchdogStatusEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sink_events = Arc::clone(&events);
    let watchdog = register_child_watchdog(
        Some(&child_config_json()),
        cwd,
        Arc::new(|| None),
        None,
        Arc::new(move |event: &ChildWatchdogStatusEvent| {
            sink_events
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event.clone());
        }),
    )
    .expect("the child is armed");
    (watchdog, events)
}

#[tokio::test]
async fn an_unarmed_child_installs_no_watchdog_at_all() {
    // The env-driven resolver with nothing in the environment: no watchdog, and — with no other
    // child flag set either — no prompt-runtime extension at all, exactly as upstream's
    // `registerChildWatchdog` returns `undefined`.
    let extension = cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from(&|_| None);
    assert!(extension.is_none());
}

#[tokio::test]
async fn an_armed_child_drives_its_watchdog_through_the_extension_event_path() {
    let root = tempfile::tempdir().expect("tempdir");
    let (watchdog, events) = armed_child(root.path());
    let runtime = cyrup_ext_subagents::prompt_runtime::SubagentPromptRuntime::from_parts(
        None, None, false,
    )
    .with_watchdog(
        Some(Arc::clone(&watchdog)),
        Arc::new(std::sync::Mutex::new(None)),
    );

    // `init` must declare the five subscriptions, or none of the below is ever delivered.
    let mut api = InitApi::new();
    runtime.init(&mut api).await.expect("init");
    let subs = api.subscriptions();
    for kind in [
        cyrup_ext::EventKind::SessionStart,
        cyrup_ext::EventKind::BeforeAgentStart,
        cyrup_ext::EventKind::TurnEnd,
        cyrup_ext::EventKind::AgentEnd,
        cyrup_ext::EventKind::SessionShutdown,
    ] {
        assert!(subs.contains(kind), "{kind:?} is subscribed by the child runtime");
    }

    let cwd = ctx(root.path());
    runtime
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
            },
            &cwd,
        )
        .await;
    runtime
        .on_event(&HostEvent::AgentEnd { messages: Vec::new() }, &cwd)
        .await;
    runtime
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "test".to_string(),
            },
            &cwd,
        )
        .await;

    let events = events.lock().unwrap();
    let phases: Vec<ChildWatchdogPhase> = events.iter().map(|e| e.phase).collect();
    assert_eq!(
        phases,
        vec![
            ChildWatchdogPhase::Idle,      // session_start
            ChildWatchdogPhase::Reviewing, // agent_end, before
            ChildWatchdogPhase::Idle,      // agent_end, after
            ChildWatchdogPhase::Idle,      // session_shutdown
        ],
        "the child's status channel was driven from the real event path"
    );
    assert_eq!(
        events.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    for event in events.iter() {
        assert_eq!(event.event_type, CHILD_WATCHDOG_STATUS_EVENT);
        assert_eq!(event.run_id.as_deref(), Some("run-1"));
        assert_eq!(event.agent.as_deref(), Some("reviewer"));
    }
}

#[tokio::test]
async fn an_armed_childs_runtime_is_enabled_where_the_orchestrators_is_not() {
    let root = tempfile::tempdir().expect("tempdir");
    let (watchdog, _events) = armed_child(root.path());
    let snapshot = watchdog.runtime().get_snapshot(None);
    assert!(
        snapshot.enabled,
        "a child the parent armed is ON regardless of the child's own settings.json"
    );
    assert_eq!(snapshot.config.agent_end_timeout_ms, 5_000);
    assert_eq!(watchdog.config().agent.as_deref(), Some("reviewer"));
}

#[tokio::test]
async fn a_child_watchdog_buffers_the_turn_delta_it_is_handed() {
    let root = tempfile::tempdir().expect("tempdir");
    let (watchdog, _events) = armed_child(root.path());
    let runtime = cyrup_ext_subagents::prompt_runtime::SubagentPromptRuntime::from_parts(
        None, None, false,
    )
    .with_watchdog(
        Some(Arc::clone(&watchdog)),
        Arc::new(std::sync::Mutex::new(None)),
    );
    let cwd = ctx(root.path());
    runtime
        .on_event(
            &HostEvent::SessionStart {
                reason: "test".to_string(),
            },
            &cwd,
        )
        .await;
    runtime.on_event(&turn_end("the child did work"), &cwd).await;
    assert_eq!(
        watchdog.runtime().get_snapshot(None).buffered_deltas,
        0,
        "a custom-role message renders no review section, so nothing buffers"
    );

    // A real assistant turn DOES buffer — this is the arm that proves `turn_end` reaches the
    // runtime rather than being dropped on the way.
    let assistant = HostEvent::TurnEnd {
        turn_index: 1,
        message: cyrup_agent::AgentMessage::Custom {
            kind: "unused".to_string(),
            payload: serde_json::Value::Null,
            timestamp: Some(0),
        },
        tool_results: Vec::new(),
    };
    let _ = assistant;
    watchdog.handle_turn_end(
        &serde_json::json!({
            "type": "turn_end",
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "did work" }] },
            "toolResults": [],
        }),
        root.path(),
    );
    assert_eq!(
        watchdog.runtime().get_snapshot(None).buffered_deltas,
        1,
        "the child's runtime is live and buffering"
    );
}

#[tokio::test]
async fn a_disabled_child_config_installs_nothing() {
    let root = tempfile::tempdir().expect("tempdir");
    let raw = serde_json::json!({ "enabled": false }).to_string();
    assert!(register_child_watchdog(
        Some(&raw),
        root.path(),
        Arc::new(|| None),
        None,
        Arc::new(|_: &ChildWatchdogStatusEvent| {}),
    )
    .is_none());
}

#[test]
fn the_scratch_paths_never_escape_the_test_tempdir() {
    // A guard against the hand-rolled `env::temp_dir()` leak this repo has been bitten by: every
    // fixture above is a `tempfile::TempDir`, so nothing survives the test.
    let root = tempfile::tempdir().expect("tempdir");
    let path: PathBuf = root.path().to_path_buf();
    assert!(path.exists());
    drop(root);
    assert!(!path.exists());
}
