//! SUBA-016 part B, end to end: the nine `schedule.*` verbs through the REAL `subagent` tool, and
//! a real fired run through the real headless workflow launch.
//!
//! Every row here drives `Tool::execute` or the installed [`ScheduledRunManager`] — never a module
//! function directly — because the defect this task exists to close is a well-tested surface that
//! nothing could reach. A `schedule.create` that works only when called from Rust is a database,
//! not a feature.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::Path;
use std::sync::{Arc, Mutex};

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolError, ToolResult};

use crate::background::scheduled_runs::{
    SCHEDULED_RUN_ACTIONS, ScheduleRunState, ScheduledRunManager, schedule_timestamp,
};
use crate::extension::executor::SubagentExecutor;
use crate::extension::testsupport::{arm_scoped_missions, tool_text};
use crate::extension::tool::SubagentTool;

/// A [`cyrup_ext::host::HostServices`] whose reported session id/file can be SWAPPED mid-test.
///
/// That mutability is the whole point of two rows below: `current_session_id` is documented to
/// read straight off the bound backend on every call, so a trigger that read it live would observe
/// a different identity after the swap. The pinned snapshot must not.
struct SwappableSessionHost {
    session: Mutex<(Option<String>, Option<std::path::PathBuf>)>,
}

impl SwappableSessionHost {
    fn new(id: &str, file: &Path) -> Arc<Self> {
        Arc::new(Self {
            session: Mutex::new((Some(id.to_string()), Some(file.to_path_buf()))),
        })
    }

    fn swap_to(&self, id: &str, file: &Path) {
        *self.session.lock().expect("session slot") =
            (Some(id.to_string()), Some(file.to_path_buf()));
    }
}

impl cyrup_ext::host::HostServices for SwappableSessionHost {
    fn session_id(&self) -> Option<String> {
        self.session.lock().expect("session slot").0.clone()
    }

    fn session_file(&self) -> Option<std::path::PathBuf> {
        self.session.lock().expect("session slot").1.clone()
    }
}

/// An executor with a bound session, a scoped mission index, and scheduled runs installed — i.e.
/// the exact state `HostEvent::SessionStart` leaves the process in.
async fn armed(
    dir: &Path,
    session: &str,
) -> (
    Arc<SubagentExecutor>,
    Arc<SwappableSessionHost>,
    Arc<ScheduledRunManager>,
) {
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir).await;
    let host = SwappableSessionHost::new(session, &dir.join(format!("{session}.jsonl")));
    executor.set_host_services(Arc::clone(&host) as Arc<dyn cyrup_ext::host::HostServices>);
    executor.capture_parent_session_anchor();
    let manager = executor
        .install_scheduled_runs(dir)
        .await
        .expect("scheduled runs are enabled by default");
    (executor, host, manager)
}

async fn dispatch(tool: &SubagentTool, params: serde_json::Value) -> Result<ToolResult, ToolError> {
    tool.execute(
        ToolCallId::from("t"),
        params,
        CancelToken::new(),
        Box::new(|_u: cyrup_core::ToolUpdate| {}),
    )
    .await
}

/// A workflow script that returns immediately and spawns no child.
const TRIVIAL_SCRIPT: &str = "return 1;";

/// THE advertise-vs-dispatch invariant, for this family: every one of the nine reaches a real arm.
///
/// `unknown_subagent_action_message` is the exact failure this guards — a verb in the schema's
/// `action` enum that `route_action` answers with "unknown subagent action" is a worse defect than
/// the missing verb was.
#[tokio::test]
async fn every_scheduled_run_action_dispatches() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    for action in SCHEDULED_RUN_ACTIONS {
        let reply = dispatch(&tool, serde_json::json!({ "action": action })).await;
        let text = match &reply {
            Ok(result) => tool_text(result),
            Err(error) => error.to_string(),
        };
        assert!(
            !text.contains("Unknown subagent action") && !text.contains("unknown subagent action"),
            "action '{action}' is advertised but lands on the unknown-action arm: {text}"
        );
    }
}

/// The single most likely wrong implementation of this task: accepting `{agent, task}` as a
/// schedule target. Upstream refuses it BY NAME, and tells the caller the script to write instead.
#[tokio::test]
async fn schedule_create_refuses_a_non_workflow_script_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create",
            "every": "6h",
            "agent": "delegate",
            "task": "sweep",
        }),
    )
    .await
    .expect_err("an agent/task target must be refused");
    assert_eq!(
        error.to_string(),
        "schedule.create requires workflowScript. Use workflowScript: \"return runs.run('main', { agent, task })\".",
        "upstream's verbatim sentence, which NAMES the replacement"
    );

    // …and the legacy fan-out shapes get their own sentence.
    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create",
            "every": "6h",
            "tasks": [{ "agent": "delegate", "task": "sweep" }],
        }),
    )
    .await
    .expect_err("a tasks[] target must be refused");
    assert_eq!(
        error.to_string(),
        "Recurring schedules require workflowScript; legacy tasks and chain inputs are unsupported."
    );
}

/// The two reserved calendar parameters are DECLARED in the schema and REFUSED at dispatch, which
/// is the only way a model receives an actionable sentence instead of a schema rejection.
#[tokio::test]
async fn schedule_create_refuses_a_calendar_trigger_with_pis_own_sentence() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    const SENTENCE: &str = "Calendar schedules are deferred from this first safe slice. Use a fixed interval such as every:'24h' or every:'7d'.";

    for params in [
        serde_json::json!({ "action": "schedule.create", "every": "6h", "workflowScript": TRIVIAL_SCRIPT, "on": "monday" }),
        serde_json::json!({ "action": "schedule.create", "every": "6h", "workflowScript": TRIVIAL_SCRIPT, "on": 1 }),
        serde_json::json!({ "action": "schedule.create", "every": "6h", "workflowScript": TRIVIAL_SCRIPT, "timezone": "Europe/London" }),
        serde_json::json!({ "action": "schedule.create", "every": "day", "workflowScript": TRIVIAL_SCRIPT }),
    ] {
        let error = dispatch(&tool, params.clone())
            .await
            .expect_err("a calendar schedule must be refused");
        assert_eq!(error.to_string(), SENTENCE, "for {params}");
    }
}

/// `baseRef` is PERSISTED by the record and REFUSED at create — never silently dropped, because a
/// schedule that runs against the wrong tree is discovered from the diff, not from the reply.
#[tokio::test]
async fn schedule_create_refuses_a_base_ref_rather_than_running_against_the_wrong_tree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create",
            "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
            "baseRef": "refs/heads/main",
        }),
    )
    .await
    .expect_err("a baseRef must be refused, not dropped");
    assert_eq!(
        error.to_string(),
        crate::background::scheduled_runs::tool::BASE_REF_UNSUPPORTED
    );

    // A MALFORMED ref is refused first, with the ref validator's own sentence — so a caller who
    // typed it wrong learns that before learning the field is unsupported.
    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create",
            "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
            "baseRef": "bad ref",
        }),
    )
    .await
    .expect_err("a malformed baseRef must be refused");
    assert_eq!(
        error.to_string(),
        crate::workflows::scripted::BASE_REF_VALIDATION_ERROR
    );
}

/// `maxPending` is counted over the WHOLE store, and its refusal names the limit.
#[tokio::test]
async fn the_max_pending_limit_refuses_the_twenty_first_schedule() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    for index in 0..20 {
        dispatch(
            &tool,
            serde_json::json!({
                "action": "schedule.create",
                "id": format!("s{index}"),
                "every": "6h",
                "workflowScript": TRIVIAL_SCRIPT,
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("schedule {index} must be accepted: {error}"));
    }

    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create",
            "id": "s20",
            "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect_err("the twenty-first must be refused");
    assert_eq!(
        error.to_string(),
        "Schedule limit reached (20).",
        "upstream's verbatim sentence, carrying DEFAULT_MAX_PENDING"
    );
}

/// SCOPE_15 pins the predicate; this pins the WIRING — the refusal reached through the real tool.
#[tokio::test]
async fn a_capability_ceiling_refuses_schedule_create_through_the_tool() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    // The gate resolves through the PROCESS registry, which the ceiling below registers into.
    let session = manager
        .fire()
        .session
        .session_id()
        .expect("the bound session")
        .as_str()
        .to_string();
    let _guard = crate::exec::capability_ceiling::register_capability_ceiling(
        &session,
        "org-policy",
        &serde_json::json!({ "allowedTools": ["read"] }),
    )
    .expect("registers");

    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create",
            "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect_err("a ceiling-bound session may not persist a schedule");
    assert_eq!(
        error.to_string(),
        crate::background::scheduled_runs::SCHEDULE_CEILING_REFUSAL
    );
}

/// `schedule.list` is NOT filtered by session — the session dimension is INFORMATION, in the
/// fourth column, not a filter. A filtered list would hide project schedules the same user can
/// still reach with `schedule.show` and `schedule.delete`, and would make `maxPending`'s
/// whole-store count unexplainable from the UI.
#[tokio::test]
async fn listing_reports_session_only_and_project_schedules_distinctly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    // The project one is armed LATER, so the sort by `nextRunAt` is observable.
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "mine", "every": "1h",
            "workflowScript": TRIVIAL_SCRIPT, "sessionOnly": true,
        }),
    )
    .await
    .expect("a session-only schedule");
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "ours", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("a project schedule");

    let listing = dispatch(&tool, serde_json::json!({ "action": "schedule.list" }))
        .await
        .expect("listing");
    let text = tool_text(&listing);

    assert!(text.starts_with("Project schedules: 2"), "{text}");
    let mine = text
        .lines()
        .find(|line| line.contains(" mine") || line.contains("- mine"))
        .unwrap_or_else(|| panic!("the session-only schedule must be listed: {text}"));
    let ours = text
        .lines()
        .find(|line| line.starts_with("- ours"))
        .unwrap_or_else(|| panic!("the project schedule must be listed: {text}"));
    assert!(
        mine.contains("| session-only |"),
        "the fourth column marks a session-only schedule: {mine}"
    );
    assert!(
        ours.contains("| project |"),
        "and marks a project one: {ours}"
    );
    assert!(
        text.find("- mine").expect("mine") < text.find("- ours").expect("ours"),
        "sorted by nextRunAt, so the 1h schedule precedes the 6h one: {text}"
    );

    // The `details` payload strips the owner's session FILE — a filesystem path a model has no
    // business seeing.
    let details = listing.details.expect("details");
    let records = details["schedules"]["records"]
        .as_array()
        .expect("records array");
    assert_eq!(records.len(), 2, "both records, unfiltered");
    for record in records {
        assert!(
            record.get("ownerSessionFile").is_none(),
            "ownerSessionFile must be stripped: {record}"
        );
    }
}

/// The full lifecycle, through the tool: create → pause → resume → delete.
#[tokio::test]
async fn cancelling_a_schedule_removes_it_from_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");
    assert_eq!(manager.store().ids().await.expect("ids").len(), 1);

    let paused = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.pause", "id": "nightly" }),
    )
    .await
    .expect("pause");
    assert_eq!(tool_text(&paused), "Paused schedule nightly.");
    // The idempotent reply is NOT an error — asking a paused schedule to pause is a no-op the
    // caller should be told about, not a failure.
    let again = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.pause", "id": "nightly" }),
    )
    .await
    .expect("a second pause is not an error");
    assert_eq!(tool_text(&again), "Schedule nightly is already paused.");
    let resumed = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.resume", "id": "nightly" }),
    )
    .await
    .expect("resume");
    assert_eq!(tool_text(&resumed), "Resumed schedule nightly.");

    let deleted = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.delete", "id": "nightly" }),
    )
    .await
    .expect("delete");
    assert_eq!(tool_text(&deleted), "Deleted schedule nightly.");
    assert!(
        manager.store().ids().await.expect("ids").is_empty(),
        "the directory is gone, not merely unlisted"
    );
}

/// The guard a one-line "cancel" hides: deleting a schedule whose run is still going would orphan
/// the run — the lock file goes with the directory and nothing would ever settle its record.
#[tokio::test]
async fn deleting_a_schedule_with_a_live_run_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    // A claim that never settled — exactly what a process killed mid-run leaves behind.
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let mut record = manager.store().get(&id).await.expect("read");
    let run_id = crate::background::scheduled_runs::ScheduleRunId::mint();
    record.active_run_id = Some(run_id.clone());
    manager.store().write(&record).await.expect("claim");

    let error = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.delete", "id": "nightly" }),
    )
    .await
    .expect_err("a live run blocks the delete");
    assert_eq!(
        error.to_string(),
        format!(
            "Schedule nightly has active run {run_id}; stop that run before deleting the schedule."
        ),
        "upstream's verbatim sentence, naming the run that has to stop first"
    );
    assert_eq!(
        manager.store().ids().await.expect("ids").len(),
        1,
        "and the schedule really is still there"
    );
}

/// Wait for a schedule's run record to leave `Running`, so a test that fired a REAL workflow can
/// assert on its settled state without sleeping on a wall clock.
async fn await_settled(
    manager: &ScheduledRunManager,
    id: &crate::background::scheduled_runs::ScheduleId,
) -> crate::background::scheduled_runs::ScheduleRunRecord {
    for _ in 0..600 {
        let history = manager.store().history(id).await.expect("history");
        if let Some(run) = history
            .iter()
            .find(|run| run.state != ScheduleRunState::Running)
        {
            return run.clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    panic!("a fired run never settled");
}

/// §5 — a fired schedule really charges the live session's spawn budget. Before this, the schedule
/// path was the ONE route into execution that spent a child the session never paid for:
/// `Tool::execute` dispatches `action` and returns ABOVE the budget charge every other mode pays.
#[tokio::test]
async fn a_fired_schedule_claims_a_spawn_slot_for_the_live_session() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    let cap = executor
        .config_snapshot()
        .await
        .max_subagent_spawns_per_session;
    assert_eq!(
        executor.spawn_budget_snapshot(cap).used,
        0,
        "precondition: nothing spent yet"
    );

    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "at": "+1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");
    let run = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.run", "id": "nightly" }),
    )
    .await
    .expect("a manual fire");
    assert!(
        tool_text(&run).contains("running"),
        "the fire really started: {}",
        tool_text(&run)
    );

    assert_eq!(
        executor.spawn_budget_snapshot(cap).used,
        1,
        "the fired run is billed to this session, exactly once"
    );
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let settled = await_settled(&manager, &id).await;
    assert_eq!(
        settled.state,
        ScheduleRunState::Completed,
        "the run really ran: {settled:?}"
    );
}

/// The cap BINDS, and a refusal costs no lock and does not hot-retry.
#[tokio::test]
async fn a_fired_schedule_is_refused_when_the_session_is_at_its_spawn_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    // A cap of one, already spent.
    executor
        .config_cell()
        .lock()
        .await
        .max_subagent_spawns_per_session = 1;
    let host = SwappableSessionHost::new("session-a", &dir.path().join("session-a.jsonl"));
    executor.set_host_services(Arc::clone(&host) as Arc<dyn cyrup_ext::host::HostServices>);
    executor.capture_parent_session_anchor();
    let manager = executor
        .install_scheduled_runs(dir.path())
        .await
        .expect("installed");
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    executor
        .reserve_subagent_spawns(1, 1)
        .expect("precondition: the one slot is taken");

    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");
    let error = dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.run", "id": "nightly" }),
    )
    .await
    .expect_err("a fire at the cap reports a failed launch");

    assert!(
        error.to_string().contains("failed_launch"),
        "the reply names the failed launch: {error}"
    );
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let run = manager
        .store()
        .history(&id)
        .await
        .expect("history")
        .into_iter()
        .next()
        .expect("one record");
    assert_eq!(run.state, ScheduleRunState::FailedLaunch);
    let message = run.error.expect("the budget's own sentence");
    assert_eq!(
        message,
        crate::exec::spawn_budget::preflight_spawn_budget(&executor.spawn_budget_snapshot(1), 1)
            .expect_err("the cap really is spent"),
        "the run record carries `preflight_spawn_budget`' own sentence verbatim — an `or` over \
         the substrings \"spawn\"/\"limit\" would also pass for a message that lost the numbers"
    );
    assert_eq!(
        manager
            .store()
            .active_lock_holder(&id)
            .await
            .expect("lock read"),
        None,
        "a refusal costs NO lock: the gate is ahead of the claim"
    );
}

/// **THE ACCEPTANCE TEST** — schedule → launch → budget → delivery.
///
/// A fired run is a REAL `RunMode::Workflow` run attributed to the session that fired it, and
/// `OwnershipSnapshot` admits it for that session and no other. Attributing it to a dead session
/// would strand the result behind `readable_sessions()`; attributing it to everyone would deliver
/// one project's schedule into another instance's turn.
#[tokio::test]
async fn a_fired_runs_result_is_delivered_to_the_live_session_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "at": "+1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");
    dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.run", "id": "nightly" }),
    )
    .await
    .expect("fire");

    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let settled = await_settled(&manager, &id).await;
    assert_eq!(settled.state, ScheduleRunState::Completed);
    let async_dir = settled.async_dir.expect("the fired run's directory");
    let status = crate::background::control::read_status_file(&async_dir.join("status.json"))
        .await
        .expect("status read")
        .expect("a fired run really wrote a status");

    assert_eq!(status.mode, crate::background::RunMode::Workflow);
    let owner_session = status
        .session_id
        .clone()
        .expect("the run names its session");
    assert_eq!(
        owner_session.as_str(),
        "session-a",
        "the LIVE session at fire time, never the one that created the schedule if they differ"
    );

    // The delivery gate: session A may read it, session B may not.
    let a = crate::background::delivery::OwnershipSnapshot::new(
        Some(owner_session.clone()),
        status.completion_owner_id.clone(),
        Vec::new(),
    );
    assert!(
        a.readable_sessions().contains(&owner_session),
        "the firing session can reach its own scheduled run"
    );
    let other = crate::identity::SessionId::parse("session-b").expect("id");
    let b = crate::background::delivery::OwnershipSnapshot::new(
        Some(other.clone()),
        status.completion_owner_id.clone(),
        Vec::new(),
    );
    assert!(
        !b.readable_sessions().contains(&owner_session),
        "a DIFFERENT session must not be able to consume this run's completion"
    );
}

/// SUBTASK2 — the pinned identity. `current_session_id` reads live off the P-1 backend on every
/// call, so a trigger that consulted it per step could stamp one identity on the run and judge the
/// ownership gate against another. The snapshot is taken once per BINDING.
#[tokio::test]
async fn the_session_identity_is_pinned_for_the_duration_of_a_fire() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "at": "+1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    // The live backend moves underneath us, exactly as a session switch inside one process does.
    host.swap_to("session-b", &dir.path().join("session-b.jsonl"));
    assert_eq!(
        executor.current_session_id().as_deref(),
        Some("session-b"),
        "precondition: the LIVE read really did move"
    );

    dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.run", "id": "nightly" }),
    )
    .await
    .expect("fire");

    assert_eq!(
        manager
            .fire()
            .session
            .session_id()
            .map(|id| id.as_str().to_string()),
        Some("session-a".to_string()),
        "the manager's pinned identity is the one captured at its binding"
    );
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let settled = await_settled(&manager, &id).await;
    let async_dir = settled.async_dir.expect("run dir");
    let status = crate::background::control::read_status_file(&async_dir.join("status.json"))
        .await
        .expect("status read")
        .expect("status");
    assert_eq!(
        status
            .session_id
            .as_ref()
            .map(crate::identity::SessionId::as_str),
        Some("session-a"),
        "the fired run carries the PINNED identity, not whatever the live read says now"
    );
}

/// D7's half that a partial port would miss: the session FILE is pinned too, and it is the value
/// the `sessionOnly` gate actually reads. Pinning only the id passes the row above and still
/// leaves the ownership gate reading live state.
#[tokio::test]
async fn the_session_file_is_pinned_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "mine", "every": "1h",
            "workflowScript": TRIVIAL_SCRIPT, "sessionOnly": true,
        }),
    )
    .await
    .expect("create");

    // Only the FILE moves; the id is untouched. An id-only pin would see no change at all here.
    host.swap_to("session-a", &dir.path().join("elsewhere.jsonl"));
    assert_eq!(
        executor
            .host_services()
            .and_then(|services| services.session_file()),
        Some(dir.path().join("elsewhere.jsonl")),
        "precondition: the LIVE session file really did move"
    );

    let id = crate::background::scheduled_runs::ScheduleId::parse("mine").expect("id");
    let record = manager.store().get(&id).await.expect("read");
    assert!(
        crate::background::scheduled_runs::schedule_belongs_to_session(
            &record,
            &manager.fire().session
        ),
        "the ownership gate must still answer yes: it reads the PINNED file, which is the one \
         recorded on the schedule, not the live one that just moved"
    );
    assert_eq!(
        manager.fire().session.session_file(),
        crate::background::scheduled_runs::normalized_session_file(Some(
            &dir.path().join("session-a.jsonl")
        ))
        .as_deref(),
        "and the pin is the file captured at the binding"
    );
}

/// The cwd-keyed store's reason to exist: a project schedule outlives the session that created it
/// and fires in a LATER one, attributed to that later session.
#[tokio::test]
async fn a_non_session_only_schedule_created_in_one_session_fires_in_a_later_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    // The session ends, and a new one starts in the same project — the real SessionShutdown /
    // SessionStart pair, not a simulation of one.
    executor.dispose_scheduled_runs();
    host.swap_to("session-b", &dir.path().join("session-b.jsonl"));
    executor.capture_parent_session_anchor();
    let manager = executor
        .install_scheduled_runs(dir.path())
        .await
        .expect("re-installed for the new session");

    // Make it due, then drive the tick the timer would have driven.
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let mut record = manager.store().get(&id).await.expect("read");
    let now = crate::time::now_epoch_millis();
    record
        .trigger
        .set_next_run_at(Some(schedule_timestamp(now - 1)));
    manager.store().write(&record).await.expect("arm");

    let runs =
        crate::background::scheduled_runs::tick_due_schedules(manager.store(), manager.fire(), now)
            .await
            .expect("tick");
    assert_eq!(runs.len(), 1, "the schedule survived the session change");

    let settled = await_settled(&manager, &id).await;
    let async_dir = settled.async_dir.expect("run dir");
    let status = crate::background::control::read_status_file(&async_dir.join("status.json"))
        .await
        .expect("status read")
        .expect("status");
    assert_eq!(
        status
            .session_id
            .as_ref()
            .map(crate::identity::SessionId::as_str),
        Some("session-b"),
        "attributed to the session that is LIVE when it fires — attributing it to the dead \
         creating session would strand its result"
    );
}

/// Every published [`crate::background::ResultFile`] under `cwd`'s results tree.
///
/// A scheduled fire's result is written by `publish_scheduled_result` BEFORE its completion future
/// resolves, so `await_settled` is already a sufficient barrier for reading it back.
async fn published_results(
    executor: &SubagentExecutor,
    cwd: &Path,
) -> Vec<crate::background::ResultFile> {
    let roots = executor.config_snapshot().await.roots.clone();
    let dir = crate::extension::executor::paths::default_results_dir_in(&roots, cwd);
    let mut found = Vec::new();
    let mut stack = vec![dir];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path)
                && let Ok(result) = serde_json::from_slice::<crate::background::ResultFile>(&bytes)
            {
                found.push(result);
            }
        }
    }
    found
}

/// `quiet` is not decorative: it rides onto the published result as `scheduleOrigin.quiet`, which
/// is what `scheduled_completion_triggers_turn` reads. A quiet SUCCESS does not wake a turn; a
/// quiet FAILURE still does, and both are still delivered and displayed.
///
/// The predicates are pinned first, and then the WIRING they depend on: a real quiet schedule is
/// fired through the real tick path and the flag is read back off the result file it published.
/// Without that second half this row asserted a classifier over a hand-built
/// [`crate::background::ScheduleOrigin`] and would have passed unchanged if the record's `quiet`
/// had never reached a fired run at all.
#[tokio::test]
async fn a_quiet_schedules_successful_completion_does_not_wake_a_turn() {
    use crate::background::ScheduleOrigin;
    use crate::background::watch::{
        ClassifiedOutcome, completion_notice_display, scheduled_completion_triggers_turn,
    };

    let quiet = ScheduleOrigin {
        id: "nightly".to_string(),
        name: Some("nightly sweep".to_string()),
        quiet: Some(true),
    };
    let loud = ScheduleOrigin {
        id: "nightly".to_string(),
        name: None,
        quiet: None,
    };

    assert!(
        !scheduled_completion_triggers_turn(Some(&quiet), ClassifiedOutcome::Completed),
        "a quiet schedule's success must not interrupt"
    );
    assert!(
        scheduled_completion_triggers_turn(Some(&quiet), ClassifiedOutcome::Failed),
        "…but its FAILURE still does — quiet is 'do not interrupt me', never 'do not tell me'"
    );
    assert!(
        scheduled_completion_triggers_turn(Some(&loud), ClassifiedOutcome::Completed),
        "a loud schedule wakes the turn like any other completion"
    );
    assert!(
        completion_notice_display(ClassifiedOutcome::Completed, Some(&quiet)),
        "a scheduled run is DISPLAYED even on success: nobody was watching when it fired, and a \
         silent success is indistinguishable from a schedule that never ran"
    );
    assert!(
        !completion_notice_display(ClassifiedOutcome::Completed, None),
        "…while an ordinary successful background completion stays invisible, as before"
    );

    // …and now the wiring those predicates are useless without: a REAL fire of a schedule whose
    // record carries `quiet`, through the real `run-due` path (a MANUAL fire takes its quiet from
    // the call, not from the record — `:877` — so it would not exercise this at all).
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    for (id, quiet_flag) in [("hushed", true), ("loud", false)] {
        dispatch(
            &tool,
            serde_json::json!({
                "action": "schedule.create", "id": id, "every": "1h",
                "workflowScript": TRIVIAL_SCRIPT, "quiet": quiet_flag,
            }),
        )
        .await
        .unwrap_or_else(|error| panic!("create {id}: {error}"));
        let schedule_id = crate::background::scheduled_runs::ScheduleId::parse(id).expect("id");
        let mut record = manager.store().get(&schedule_id).await.expect("read");
        assert_eq!(
            record.quiet,
            quiet_flag.then_some(true),
            "precondition: the RECORD carries the flag"
        );
        record.trigger.set_next_run_at(Some(schedule_timestamp(
            crate::time::now_epoch_millis() - 1,
        )));
        manager.store().write(&record).await.expect("arm");
    }

    dispatch(&tool, serde_json::json!({ "action": "schedule.run-due" }))
        .await
        .expect("run-due fires both");
    for id in ["hushed", "loud"] {
        let schedule_id = crate::background::scheduled_runs::ScheduleId::parse(id).expect("id");
        assert_eq!(
            await_settled(&manager, &schedule_id).await.state,
            ScheduleRunState::Completed,
            "{id} must really have run"
        );
    }

    let results = published_results(&executor, dir.path()).await;
    let origin_for = |id: &str| {
        results
            .iter()
            .filter_map(|result| result.schedule_origin.as_ref())
            .find(|origin| origin.id == id)
            .unwrap_or_else(|| panic!("{id} published no result carrying a scheduleOrigin"))
            .clone()
    };
    let hushed = origin_for("hushed");
    assert_eq!(
        hushed.quiet,
        Some(true),
        "the record's `quiet` must reach the published result, or every predicate above is dead \
         code"
    );
    assert!(
        !scheduled_completion_triggers_turn(Some(&hushed), ClassifiedOutcome::Completed),
        "and the classifier must then answer 'do not wake the turn' for the REAL origin"
    );
    let loud_origin = origin_for("loud");
    assert_eq!(
        loud_origin.quiet, None,
        "a loud schedule must not acquire the flag"
    );
    assert!(scheduled_completion_triggers_turn(
        Some(&loud_origin),
        ClassifiedOutcome::Completed
    ));
}

/// `schedule.create`'s reply reports the two flags a user cannot otherwise see, and a `quiet` on a
/// ONE-SHOT is refused: a one-shot the user asked for is exactly the fire they want to hear about.
#[tokio::test]
async fn schedule_create_reports_its_flags_and_refuses_quiet_on_a_one_shot() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    let created = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT, "sessionOnly": true, "quiet": true,
        }),
    )
    .await
    .expect("create");
    let text = tool_text(&created);
    assert!(text.starts_with("Created schedule nightly."), "{text}");
    assert!(text.contains("Session only: yes"), "{text}");
    assert!(text.contains("Quiet: yes"), "{text}");
    assert!(text.contains("Trigger: every 6h"), "{text}");

    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "once", "at": "+10m",
            "workflowScript": TRIVIAL_SCRIPT, "quiet": true,
        }),
    )
    .await
    .expect_err("quiet on a one-shot is refused");
    assert_eq!(
        error.to_string(),
        "quiet is only supported for recurring schedules."
    );

    let error = dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "bad", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT, "quiet": "yes",
        }),
    )
    .await
    .expect_err("a non-boolean quiet is refused by name, not by serde");
    assert_eq!(error.to_string(), "quiet must be a boolean.");
}

/// A fanout child may READ what the project has scheduled and may not change it.
#[tokio::test]
async fn a_child_safe_tool_refuses_the_six_mutating_schedule_verbs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let child = SubagentTool::new_child_safe(Arc::clone(&executor), dir.path().to_path_buf());

    for action in [
        "schedule.create",
        "schedule.pause",
        "schedule.resume",
        "schedule.run",
        "schedule.run-due",
        "schedule.delete",
    ] {
        let error = dispatch(&child, serde_json::json!({ "action": action }))
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            format!("Action '{action}' is not available from child-safe subagent fanout mode."),
            "the same sentence the mutating mission verbs already get"
        );
    }
    for action in ["schedule.list", "schedule.show", "schedule.history"] {
        let reply = dispatch(&child, serde_json::json!({ "action": action })).await;
        let text = match &reply {
            Ok(result) => tool_text(result),
            Err(error) => error.to_string(),
        };
        assert!(
            !text.contains("child-safe subagent fanout mode"),
            "reading is allowed: {action} -> {text}"
        );
    }
}

/// `schedule.run-due` fires what is due and says so — and says the opposite when nothing is.
#[tokio::test]
async fn run_due_reports_what_it_processed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());

    let nothing = dispatch(&tool, serde_json::json!({ "action": "schedule.run-due" }))
        .await
        .expect("run-due with an empty store");
    assert_eq!(tool_text(&nothing), "No schedules are due.");

    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");
    let not_yet = dispatch(&tool, serde_json::json!({ "action": "schedule.run-due" }))
        .await
        .expect("run-due before it is due");
    assert_eq!(
        tool_text(&not_yet),
        "No schedules are due.",
        "a schedule armed an hour out is not due now"
    );

    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let mut record = manager.store().get(&id).await.expect("read");
    record.trigger.set_next_run_at(Some(schedule_timestamp(
        crate::time::now_epoch_millis() - 1,
    )));
    manager.store().write(&record).await.expect("arm");

    let fired = dispatch(&tool, serde_json::json!({ "action": "schedule.run-due" }))
        .await
        .expect("run-due when it is due");
    assert_eq!(tool_text(&fired), "Processed 1 due schedule(s).");
    await_settled(&manager, &id).await;
}

/// `schedule.show` and `schedule.history` render what a user asks for, and `history` says so
/// plainly when there is nothing yet.
#[tokio::test]
async fn show_and_history_render_a_schedule_and_its_runs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT, "catchUp": "none",
        }),
    )
    .await
    .expect("create");

    let shown = tool_text(
        &dispatch(
            &tool,
            serde_json::json!({ "action": "schedule.show", "id": "nightly" }),
        )
        .await
        .expect("show"),
    );
    assert!(shown.starts_with("Schedule: nightly"), "{shown}");
    assert!(shown.contains("State: scheduled"), "{shown}");
    assert!(shown.contains("Catch up: none"), "{shown}");
    assert!(
        !shown.contains("Active run:"),
        "no active run line before anything fires: {shown}"
    );

    let empty = tool_text(
        &dispatch(
            &tool,
            serde_json::json!({ "action": "schedule.history", "id": "nightly" }),
        )
        .await
        .expect("history"),
    );
    assert_eq!(empty, "No runs recorded for schedule nightly.");

    dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.run", "id": "nightly" }),
    )
    .await
    .expect("fire");
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    await_settled(&manager, &id).await;

    let history = tool_text(
        &dispatch(
            &tool,
            serde_json::json!({ "action": "schedule.history", "id": "nightly" }),
        )
        .await
        .expect("history"),
    );
    assert!(
        history.starts_with("Schedule history: nightly"),
        "{history}"
    );
    assert!(
        history.contains("| async "),
        "a settled run names the run it attached to: {history}"
    );
}

/// An armed schedule is a STOP TARGET: `/subagents-stop` with no id must offer to pause it, or a
/// user is shown the running work and not the work that is about to start.
#[tokio::test]
async fn an_armed_schedule_is_offered_as_a_stop_target() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, _manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    let targets = executor
        .format_stop_targets(dir.path())
        .await
        .expect("stop targets");
    assert!(targets.contains("- nightly"), "{targets}");
    assert!(
        targets
            .contains("pause schedule: subagent({ action: \"schedule.pause\", id: \"nightly\" })"),
        "a schedule is stopped by PAUSING it — `action: \"stop\"` addresses a RUN and would find \
         nothing: {targets}"
    );
}

/// The lifecycle edges, as `HostEvent::SessionStart`/`SessionShutdown` drive them.
///
/// Install is RE-ENTRANT (the previous manager is disposed and replaced, so an old tick cannot
/// keep firing against a store this session no longer uses) and dispose leaves the RECORDS ALONE —
/// that is the entire point of a cwd-keyed store, and the next session's restore is what picks
/// them up.
#[tokio::test]
async fn installing_and_disposing_the_manager_never_touches_the_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, first) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "6h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    let second = executor
        .install_scheduled_runs(dir.path())
        .await
        .expect("a second SessionStart re-installs");
    assert!(
        first.is_disposed(),
        "the previous manager is disposed, or two ticks would race over one store"
    );
    assert!(!second.is_disposed());
    assert_eq!(
        second.store().ids().await.expect("ids").len(),
        1,
        "the record survived the re-install"
    );

    executor.dispose_scheduled_runs();
    assert!(second.is_disposed(), "shutdown stops the tick");
    assert!(
        executor.scheduled_runs().is_none(),
        "and clears the slot, so a shut-down session cannot create a schedule through it"
    );
    assert_eq!(
        second.store().ids().await.expect("ids").len(),
        1,
        "the RECORDS stay on disk — that is the entire point of the durable half"
    );

    // …and a schedule verb now reaches the disabled reply rather than a panic or a stale manager.
    let error = dispatch(&tool, serde_json::json!({ "action": "schedule.list" }))
        .await
        .expect_err("no manager, no dispatch");
    assert_eq!(
        error.to_string(),
        crate::background::scheduled_runs::SCHEDULED_RUNS_DISABLED
    );
}

/// `scheduledRuns.enabled` is a TRI-STATE and its polarity is the opposite of the obvious one:
/// absent and `true` both ENABLE, only the literal `false` disables. A plain `bool` with
/// `#[serde(default)]` would have disabled scheduled runs for every config already on disk.
#[tokio::test]
async fn the_enabled_key_is_a_tri_state_read_through_one_accessor() {
    use crate::registration::{ScheduledRunsConfig, SubagentExtensionConfig};

    let absent = SubagentExtensionConfig::default();
    assert!(
        absent.scheduled_runs_enabled(),
        "an omitted block enables — this is the case every existing config on disk is in"
    );
    let present_empty = SubagentExtensionConfig {
        scheduled_runs: Some(ScheduledRunsConfig::default()),
        ..SubagentExtensionConfig::default()
    };
    assert!(
        present_empty.scheduled_runs_enabled(),
        "a block with no `enabled` key enables"
    );
    let on = SubagentExtensionConfig {
        scheduled_runs: Some(ScheduledRunsConfig {
            enabled: Some(true),
            ..ScheduledRunsConfig::default()
        }),
        ..SubagentExtensionConfig::default()
    };
    assert!(on.scheduled_runs_enabled());
    let off = SubagentExtensionConfig {
        scheduled_runs: Some(ScheduledRunsConfig {
            enabled: Some(false),
            ..ScheduledRunsConfig::default()
        }),
        ..SubagentExtensionConfig::default()
    };
    assert!(
        !off.scheduled_runs_enabled(),
        "only the literal false disables"
    );

    // …and `maxPending` falls back rather than accepting a nonsense value.
    assert_eq!(absent.scheduled_runs_max_pending(), 20);
    let zero = SubagentExtensionConfig {
        scheduled_runs: Some(ScheduledRunsConfig {
            max_pending: Some(0),
            ..ScheduledRunsConfig::default()
        }),
        ..SubagentExtensionConfig::default()
    };
    assert_eq!(
        zero.scheduled_runs_max_pending(),
        20,
        "pi's `>= 1` guard: a zero cap would refuse the FIRST schedule of every project"
    );

    // The polarity really reaches the install edge, not just the accessor.
    let dir = tempfile::tempdir().expect("tempdir");
    let executor = Arc::new(SubagentExecutor::new());
    arm_scoped_missions(&executor, dir.path()).await;
    executor.config_cell().lock().await.scheduled_runs = Some(ScheduledRunsConfig {
        enabled: Some(false),
        ..ScheduledRunsConfig::default()
    });
    assert!(
        executor.install_scheduled_runs(dir.path()).await.is_none(),
        "a disabled config installs no manager at all"
    );
}

/// The TIMER really fires a due schedule with nobody asking — which is the difference between a
/// feature and a database. Driven with paused time so the row costs no wall clock.
#[tokio::test(start_paused = true)]
async fn the_armed_tick_fires_a_due_schedule_with_nobody_asking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "every": "1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    // Arm it in the past, exactly as a schedule that came due while the process was busy is.
    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    let mut record = manager.store().get(&id).await.expect("read");
    record.trigger.set_next_run_at(Some(schedule_timestamp(
        crate::time::now_epoch_millis() - 1,
    )));
    manager.store().write(&record).await.expect("arm");
    assert!(
        manager
            .store()
            .history(&id)
            .await
            .expect("history")
            .is_empty(),
        "precondition: nothing has fired"
    );

    // NO tool call, NO `run-due` — just the tick the manager armed at install. With paused time
    // the whole wait costs no wall clock: `advance` is what releases the tick's own `sleep`.
    tokio::time::advance(crate::background::scheduled_runs::SCHEDULE_TICK).await;
    // …and then the clock is RESUMED before polling. A paused tokio clock auto-advances whenever
    // the scheduler has nothing ready, so a poll loop built out of `sleep` on a paused clock does
    // not wait for the launch's real filesystem I/O at all — it burns its whole budget of virtual
    // time in a few microseconds and the row fails under load while passing in isolation. The
    // advance above is what this test needs paused time FOR; the wait below wants real time.
    tokio::time::resume();
    let mut history = Vec::new();
    for _ in 0..2_000 {
        history = manager.store().history(&id).await.expect("history");
        // Wait for the ATTACH, not merely for the claim: a record still in `schedule.run.started`
        // proves the tick decided to fire, and the `async_id` proves a run really started.
        if history.iter().any(|run| run.async_id.is_some()) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    assert_eq!(
        history.len(),
        1,
        "the armed tick fired the due schedule on its own, exactly once"
    );
    assert!(
        history[0].async_id.is_some(),
        "and it really launched a run: {:?}",
        history[0]
    );
}

/// The §SUBTASK2 invariant, made mechanical rather than left to a reviewer's grep: nothing under
/// `background/scheduled_runs/` may CALL `current_session_id`, because the whole point of the
/// pinned snapshot is that the live read happens exactly once, at the binding.
#[test]
fn nothing_in_the_scheduled_runs_module_reads_the_live_session() {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/background/scheduled_runs");
    let mut offenders = Vec::new();
    let mut stack = vec![root.clone()];
    let mut scanned = 0usize;
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("the module tree must be readable") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                let body = std::fs::read_to_string(&path).expect("readable");
                scanned += 1;
                // A CALL, not a mention: the module doc names the function deliberately.
                if body.contains("current_session_id(") {
                    offenders.push(path.display().to_string());
                }
            }
        }
    }
    assert!(
        scanned >= 5,
        "precondition: the scan found the module ({scanned} files)"
    );
    assert!(
        offenders.is_empty(),
        "these read the LIVE session instead of the pinned snapshot, so one fire could observe \
         three different identities: {offenders:?}"
    );
}

/// The coupling `async_retention`'s own module doc used to record as having "no cyrup analogue":
/// a run a schedule still references must not be reaped.
///
/// It is not cosmetic. `schedule.delete`'s active-run guard opens `<async_dir>/status.json` to
/// confirm the run ended; if retention deleted that directory, a schedule holding a claim could
/// never be deleted again.
#[tokio::test]
async fn a_run_a_schedule_references_is_protected_from_retention() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (executor, _host, manager) = armed(dir.path(), "session-a").await;
    let tool = SubagentTool::new(Arc::clone(&executor), dir.path().to_path_buf());
    dispatch(
        &tool,
        serde_json::json!({
            "action": "schedule.create", "id": "nightly", "at": "+1h",
            "workflowScript": TRIVIAL_SCRIPT,
        }),
    )
    .await
    .expect("create");

    let id = crate::background::scheduled_runs::ScheduleId::parse("nightly").expect("id");
    assert!(
        manager.referenced_async_run_ids().await.is_empty(),
        "precondition: nothing referenced before anything fires"
    );

    dispatch(
        &tool,
        serde_json::json!({ "action": "schedule.run", "id": "nightly" }),
    )
    .await
    .expect("fire");
    let settled = await_settled(&manager, &id).await;
    let async_id = settled.async_id.expect("the fired run's id");

    let referenced = manager.referenced_async_run_ids().await;
    assert!(
        referenced.contains(&crate::background::RunId::from_token(async_id.clone())),
        "the schedule's history still points at {async_id}, so the sweep must protect it: \
         {referenced:?}"
    );
}
