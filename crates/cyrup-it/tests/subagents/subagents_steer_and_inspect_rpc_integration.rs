//! VL-S11's two closing registrations, driven through the REAL
//! `SubagentsExtension::execute_command`:
//!
//! * **`/subagents-steer`** (pi `slash/slash-commands.ts:1057-1119` @v0.68.0). The `steer` ACTION
//!   was already ported, advertised and dispatched (`extension/tool/routing.rs:2543`); what was
//!   missing was the slash surface over it. So the thing worth asserting is not that the command
//!   parses — it is that the message it carries lands in a LIVE run's own control inbox on disk.
//! * **`/subagents-inspect-rpc`** (pi `:928-943`). The parser (`background/inspect_rpc/request.rs`)
//!   and the reply encoder were already in-tree with no registered command. Upstream's handler is
//!   fifteen lines and every one of them is observable only from outside: a `tui` invocation
//!   REFUSES with a notice, and any other surface emits the reply into the widget slot and then
//!   RETRACTS it in the same handler.
//!
//! Both are integration tests rather than in-crate ones for the same reason: the behaviour is the
//! command's effect on something outside the function — a directory under the run dir, and the
//! host's widget channel.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cyrup_ext::host::{HostServices, NotifyKind, WidgetPlacement};
use cyrup_ext::native::{ExtMode, HostCtx, NativeExtension};
use cyrup_ext_subagents::background::active_async_capacity::{
    read_owner, session_pool_dir, slot_dir,
};
use cyrup_ext_subagents::background::atomic::write_atomic_json;
use cyrup_ext_subagents::background::process_terminal::{ProcessTerminal, ProcessTerminalBase};
use cyrup_ext_subagents::background::{
    RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus,
    active_async_capacity_root_in, run_artifact_roots_in,
};
use cyrup_ext_subagents::extension::{BackgroundStepsSpec, SubagentsExtension};
use cyrup_ext_subagents::identity::SessionId;
use cyrup_ext_subagents::paths::Roots;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;
use cyrup_ext_subagents::spawn::SpawnCommand;
use cyrup_ext_subagents::spawn::chain_graph::{RunnerStep, SingleStepSpec};

/// The session every record here is attributed to. `control_steer`'s S4 gate
/// (`foreground_actions/steer.rs:166-176`, pi `async-steering-action.ts:48`) compares the caller's
/// session against the run's, so both sides must name the same one or the steer is refused as
/// foreign — which is a different failure from the one under test.
const SESSION: &str = "steer-session";

struct RecordingHost {
    widgets: Mutex<Vec<(String, Option<Vec<String>>)>>,
    notices: Mutex<Vec<String>>,
}

impl RecordingHost {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            widgets: Mutex::new(Vec::new()),
            notices: Mutex::new(Vec::new()),
        })
    }

    fn widgets(&self) -> Vec<(String, Option<Vec<String>>)> {
        self.widgets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn notices(&self) -> Vec<String> {
        self.notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl HostServices for RecordingHost {
    fn session_id(&self) -> Option<String> {
        Some(SESSION.to_string())
    }

    fn set_widget(&self, key: &str, lines: Option<&[String]>, _placement: WidgetPlacement) {
        self.widgets
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((key.to_string(), lines.map(<[String]>::to_vec)));
    }

    fn notify(&self, message: &str, _kind: NotifyKind) {
        self.notices
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(message.to_string());
    }
}

fn worker_step() -> RunnerStep {
    RunnerStep::SingleStep(SingleStepSpec {
        machine: None,
        skills: None,
        session_dir: None,
        agent: "worker".to_string(),
        task: "keep working".to_string(),
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
        fast: None,
        reads: None,
        acceptance: None,
        context: None,
        agent_scope: None,
    })
}

fn background_spec(run_id: RunId) -> BackgroundStepsSpec {
    BackgroundStepsSpec {
        steps: vec![worker_step()],
        mode: RunMode::Single,
        session_file: None,
        resolved_agents: BTreeMap::new(),
        original_task: "keep working".to_string(),
        chain_dir: None,
        control: None,
        include_progress: None,
        run_id,
        timeout_ms: None,
        share: None,
        artifacts_dir: None,
        artifact_config: cyrup_ext_subagents::artifacts::ArtifactConfig::default(),
        turn_budget: None,
        permission_rules: None,
        usage_budget: None,
        transfer_from: None,
        revival_lease: None,
        thinking_ceiling: None,
        capability_ceiling: None,
        model_origin: None,
    }
}

/// A hop-1 runner that stays alive for the whole test: a run whose runner process has already
/// exited reconciles to a terminal state and is refused by `control_steer`'s own state guard
/// (`steer.rs:177-181`), which is not the refusal under test.
fn sleeping_runner_script(dir: &Path) -> PathBuf {
    let script = serde_json::json!({
        "steps": [{ "kind": "sleep_ms", "ms": 120_000 }],
        "exit_code": 0
    });
    let path = dir.join("sleeping-runner.json");
    std::fs::write(&path, script.to_string()).expect("write fixture script");
    path
}

/// Every `*.json` under `dir`, recursively, as text. Used instead of naming
/// `steer_requests_dir(run_dir)` so the assertion is "the message reached this run's control
/// tree", not "the message reached the directory this test happens to know the name of".
fn json_files_under(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(json_files_under(&path));
        } else if path.extension().is_some_and(|e| e == "json")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            out.push(text);
        }
    }
    out
}

// =================================================================================================
// /subagents-steer
// =================================================================================================

/// `/subagents-steer <run-id> <message>` reaches the LIVE `steer` action, not merely the parser.
///
/// The run is real in every respect the steer path consults: `spawn_background_steps` — the
/// production claim path — creates the run directory and spawns a real hop-1 process that is still
/// alive when the command runs, and the status record names that process, this session and one
/// RUNNING child. `control_steer` then walks its whole ladder (workflow lookup, foreground
/// classification, id resolution, reconcile, session gate, state guard, index guard) before it
/// writes anything.
///
/// Two assertions, and they fail differently:
///
/// * the RETURNED sentence is `Steering <state> for async run <id> (request <rid>).`
///   (`steer.rs:232`), which is produced only after the request has been written and the
///   acknowledgment budget has elapsed — a command that parsed its arguments and stopped cannot
///   produce it;
/// * the MESSAGE TEXT is on disk under the run's own control tree. That is the one that
///   distinguishes "the action ran" from "the action ran against some other run".
///
/// Gutted: register the command but route it to `control_status`, or to a stub, and the first
/// assertion sees a different sentence. Route it to `control_steer` with the wrong id (the usage
/// line's `<run-id>` dropped, say) and the first assertion sees
/// `No async run found for '…'.` Parse the message but never pass it through, and the second
/// assertion fires while the first still passes — which is why both are here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn subagents_steer_delivers_its_message_into_a_live_async_runs_control_inbox() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let roots = Roots::sandboxed(home.path());
    let script = sleeping_runner_script(cwd.path());

    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            spawn_command: Some(SpawnCommand {
                binary: crate::support::bins::subagent_fixture(),
                base_args: vec!["--fixture-script".to_string(), script.display().to_string()],
            }),
            max_active_async_runs_per_session: Some(1),
            roots: roots.clone(),
            ..SubagentExtensionConfig::default()
        },
        cwd.path().to_path_buf(),
    );
    let host = RecordingHost::new();
    ext.executor()
        .set_host_services(Arc::clone(&host) as Arc<dyn HostServices>);

    let run_id = RunId::new();
    let spawned = ext
        .executor()
        .spawn_background_steps(cwd.path(), background_spec(run_id.clone()))
        .await
        .expect("spawn_background_steps confirms the detached hop-1 spawn");
    assert_eq!(spawned, run_id);

    // The runner pid the spawn bound onto this session's capacity slot — the same pid the status
    // record must name, so `reconcile_by_id` sees a run whose runner is genuinely alive.
    let session = SessionId::parse(SESSION).expect("a valid session id");
    let pool = session_pool_dir(&active_async_capacity_root_in(&roots), &session);
    let owner = read_owner(&slot_dir(&pool, 0))
        .await
        .expect("the spawn claimed slot 0 of this session's pool");
    let pid = owner
        .runner_pid
        .expect("mark_started bound the runner pid onto the slot");
    let instance = owner
        .runner_process_instance_id
        .clone()
        .expect("mark_started bound the minted runner instance onto the slot");

    // The RUNNING record the scripted fixture-as-runner does not write for itself.
    let artifact_roots = run_artifact_roots_in(&roots, cwd.path());
    let paths = RunPaths::for_run(
        &artifact_roots.async_root,
        &artifact_roots.results_dir,
        &run_id,
    );
    let mut status = RunStatus::queued(run_id.clone(), RunMode::Single, Some(pid));
    // The `pending` process-terminal OVERLAY `publish_initial_status` writes from
    // `RunnerConfig::runner_process_instance_id` (pi `subagent-runner.ts:2102`), built from the
    // SAME minted instance the launch bound onto the capacity slot. Written here for the reason
    // `debug_run_lifecycle_integration.rs` states: the scripted fixture standing in for the runner
    // writes no `status.json` of its own, and reconciliation reads this pair.
    status.process_terminal = Some(ProcessTerminal::Pending {
        base: ProcessTerminalBase::new(run_id.clone(), instance),
    });
    status
        .advance_state(RunState::Running)
        .expect("Queued -> Running");
    let mut step = StepStatus::pending("worker");
    step.status = StepState::Running;
    status.steps = vec![step];
    status.session_id = SessionId::parse(SESSION);
    write_atomic_json(&paths.status, &status)
        .await
        .expect("write the running status.json");

    const MESSAGE: &str = "STEER_PAYLOAD_MARKER prefer the narrower fix";
    let ctx = HostCtx::command(ExtMode::Tui, true, cwd.path().to_path_buf());
    let output = ext
        .execute_command("subagents-steer", &format!("{run_id} {MESSAGE}"), &ctx)
        .await
        .expect(
            "`/subagents-steer` must be a DISPATCHED command: an ExtError here means no \
             `SlashCommandName::SubagentsSteer` arm exists in `dispatch_slash`",
        )
        .expect("the `/subagents-steer` handler renders text");

    assert!(
        output.contains(&format!("for async run {run_id}")),
        "the reply must be `control_steer`'s own `Steering <state> for async run <id> \
         (request <rid>).` (`steer.rs:232`) naming THIS run — a command that only parsed its \
         arguments cannot produce it; got: {output}\nnotices: {:?}",
        host.notices()
    );

    let control_json = json_files_under(&paths.run_dir);
    assert!(
        control_json.iter().any(|text| text.contains(MESSAGE)),
        "THE ASSERTION THAT MATTERS: the steer message must be on disk under {} — that is what \
         makes this a delivery rather than a parse. Files found: {control_json:#?}",
        paths.run_dir.display()
    );

    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status();
}

// =================================================================================================
// /subagents-inspect-rpc
// =================================================================================================

/// pi `:928-943`, the NON-tui arm: the handler writes the encoded reply into
/// `INSPECT_WIDGET_KEY` and then writes `undefined` into the SAME key, in one handler.
///
/// Upstream's own comment says why the pair is the behaviour and not an implementation detail:
/// *"stdio delivers the two widget updates in order, the host buffers the payload by requestId,
/// and the dedicated key never accumulates visible state."* A handler that emits and does not
/// retract leaves a JSON blob pinned in the user's chrome forever.
///
/// The run id names nothing on purpose. `handleInspectRpcArgs` answers a not-found request with an
/// ERROR REPLY, not with silence (`inspect_rpc::respond`'s `InspectErrorCode`), so the
/// emit-then-retract pair happens either way — and asserting it without standing up a real async
/// run keeps this test about the fifteen lines that were missing.
///
/// Gutted: drop the second `set_widget` and the retract assertion fires with one recorded write.
/// Drop the emit and the first fires with none. Register the command but route it to
/// `ctx.ui.notify` instead of the widget channel and BOTH fire.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagents_inspect_rpc_emits_its_reply_and_retracts_it_in_the_same_handler() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        cwd.path().to_path_buf(),
    );
    let host = RecordingHost::new();
    ext.executor()
        .set_host_services(Arc::clone(&host) as Arc<dyn HostServices>);

    // Both of upstream's guards are exercised deliberately: `has_ui: true` gets past `:935`'s
    // silent return, and the MODE is what `:931` keys the refusal on. The two are independent
    // fields on `HostCtx` for exactly that reason, and the port reads the mode through a latch
    // (`slash_inspect_rpc::record_attached_mode`) that `NativeExtension::execute_command` writes
    // from THIS invocation's ctx immediately before dispatching — so going through
    // `execute_command`, rather than calling the handler directly, is what makes the mode real.
    let ctx = HostCtx::command(ExtMode::Rpc, true, cwd.path().to_path_buf());
    let _ = ext
        .execute_command("subagents-inspect-rpc", "req-1 no-such-run", &ctx)
        .await
        .expect(
            "`/subagents-inspect-rpc` must be a DISPATCHED command: an ExtError here means no \
             `SlashCommandName::SubagentsInspectRpc` arm exists in `dispatch_slash`",
        );

    let key = cyrup_ext_subagents::background::inspect_rpc::INSPECT_WIDGET_KEY;
    let writes: Vec<(String, Option<Vec<String>>)> = host
        .widgets()
        .into_iter()
        .filter(|(k, _)| k == key)
        .collect();

    assert_eq!(
        writes.len(),
        2,
        "the handler must write the widget key EXACTLY twice — the emit and its retraction (pi \
         `:936-940`). Recorded widget traffic: {:#?}; notices: {:?}",
        host.widgets(),
        host.notices()
    );
    let payload = writes[0]
        .1
        .as_ref()
        .expect("the FIRST write is the reply payload, never a removal");
    let prefix = cyrup_ext_subagents::background::inspect_rpc::INSPECT_WIDGET_PREFIX;
    assert!(
        payload.iter().any(|line| line.contains(prefix)),
        "the emitted payload is `encodeInspectReply`'s own envelope, carrying the \
         `PI_SUBAGENT_INSPECT_JSON:` prefix the host demultiplexes on; got: {payload:?}"
    );
    assert!(
        payload.iter().any(|line| line.contains("req-1")),
        "and it echoes the caller's correlation token, which is the whole point of an RPC reply; \
         got: {payload:?}"
    );
    assert!(
        writes[1].1.is_none(),
        "the SECOND write must REMOVE the key (`None` is cyrup's spelling of pi's `undefined`), \
         so the dedicated slot never accumulates visible state; got: {:?}",
        writes[1].1
    );
}

/// pi `:931-934`: in `tui` mode the reply is NOT emitted at all — the user gets a notice pointing
/// at the interactive surfaces instead.
///
/// Gutted: drop the mode guard and the widget assertion fires, because the RPC payload is written
/// into an interactive user's chrome.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagents_inspect_rpc_refuses_in_tui_mode_and_emits_no_widget() {
    let home = tempfile::tempdir().expect("home tempdir");
    let cwd = tempfile::tempdir().expect("cwd tempdir");
    let ext = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig {
            roots: Roots::sandboxed(home.path()),
            ..SubagentExtensionConfig::default()
        },
        cwd.path().to_path_buf(),
    );
    let host = RecordingHost::new();
    ext.executor()
        .set_host_services(Arc::clone(&host) as Arc<dyn HostServices>);

    // `has_ui: true` again, so a pass here cannot be the `!has_ui` silent return wearing the
    // refusal's clothes: the ONLY thing different from the test above is the mode.
    let ctx = HostCtx::command(ExtMode::Tui, true, cwd.path().to_path_buf());
    let rendered = ext
        .execute_command("subagents-inspect-rpc", "req-1 no-such-run", &ctx)
        .await
        .expect("the command is dispatched in tui mode too — it refuses, it does not 404")
        .unwrap_or_default();

    // The refusal may reach the user as the command's own rendered text or as a `notify`; both are
    // the same observable ("the sentence was delivered and the reply was not"), and which one the
    // port picks is not this test's business.
    let delivered = format!("{rendered}\n{}", host.notices().join("\n"));
    assert!(
        delivered.contains("Inspection replies are emitted only on RPC surfaces."),
        "pi `:932`'s notice, verbatim; delivered was: {delivered:?}"
    );

    let key = cyrup_ext_subagents::background::inspect_rpc::INSPECT_WIDGET_KEY;
    assert!(
        !host.widgets().iter().any(|(k, _)| k == key),
        "and NOTHING may be written to the inspect widget slot in tui mode; got: {:#?}",
        host.widgets()
    );
}
