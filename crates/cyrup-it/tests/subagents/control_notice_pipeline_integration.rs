//! SUBA-N05 end-to-end proof: a SINGLE `subagent` call carrying `control` reaches the child, moves
//! the thresholds its NDJSON stream is judged against, and makes the control-NOTICE pipeline
//! actually FIRE — all the way to a real `HostServices::inject_message` transcript injection.
//!
//! Upstream reference (`@v0.34.0` throughout):
//! - `runs/shared/subagent-control.ts` — `resolveControlConfig`, `deriveActivityState`,
//!   `formatControlNoticeMessage`.
//! - `runs/foreground/subagent-executor.ts:1179` — `controlConfig: resolveControlConfig(
//!   deps.config.control, input.params.control)` on the SINGLE path.
//! - `runs/foreground/subagent-executor.ts:801-831` — `emitControlNotification`: the
//!   `childIntercomTarget` resolution, the `noticeText` render, and the
//!   `notifyChannels.includes("event")` gate.
//! - `extension/control-notices.ts:23-42` — `deliverControlNotice` →
//!   `pi.sendMessage({ customType: SUBAGENT_CONTROL_MESSAGE_TYPE, ... }, { triggerTurn })`.
//!
//! No mocking of the wired code (this crate's standing convention). Every run below:
//! - spawns the REAL `cyrup-subagent-fixture` binary as a genuine OS subprocess through
//!   `CYRUP_SUBAGENT_BINARY` and reads its REAL piped NDJSON stdout;
//! - resolves a REAL persona `.md` through the REAL discovery pipeline;
//! - drives the REAL `SubagentTool::execute` → `route_single` → `run_foreground_streaming` →
//!   `exec::run_sync` → `ControlMonitor` → `foreground_control_notifier` →
//!   `ControlNoticeState` → `HostServicesControlNoticeSink` path;
//! - and asserts on a recording `HostServices` bound through the SAME P-1 seam
//!   `cyrup-session-svc`'s `load_native_with_services` binds in production — i.e. the production
//!   sink, not a test-only one, and not the stderr `LoggingControlNoticeSink` fallback.
//!
//! The only thing shortened is the debounce window (`set_control_notice_debounce`), so a test does
//! not have to sleep out the full production 1000 ms per scenario. Everything else is production.
//!
//! Gated on the `test-fixtures` Cargo feature, matching every sibling integration test here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::type_complexity
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cyrup_core::{CancelToken, Tool, ToolCallId};
use cyrup_ext::host::HostServices;
use cyrup_ext_subagents::extension::SubagentsExtension;
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Serializes every test in this file — all of them mutate process-global env
/// (`CYRUP_SUBAGENT_BINARY`, `CYRUP_SUBAGENT_FIXTURE_SCRIPT`, `CYRUP_HOME`), exactly like every
/// sibling integration test here. A tokio mutex so the guard can be held across `.await`.
static ENV_MUTATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const FIXTURE_BINARY_ENV_VAR: &str = "CYRUP_SUBAGENT_BINARY";
const FIXTURE_SCRIPT_ENV_VAR: &str = "CYRUP_SUBAGENT_FIXTURE_SCRIPT";

/// pi `SUBAGENT_CONTROL_MESSAGE_TYPE` (`extension/control-notices.ts:5`).
const CONTROL_NOTICE_CUSTOM_TYPE: &str = "subagent_control_notice";

/// How long the child idles before finishing. Must exceed `ACTIVITY_TICK_MS` (1 s) — the control
/// monitor re-evaluates the idle heuristic on that timer, so a run shorter than one tick can never
/// raise `needs_attention` no matter how small `needsAttentionAfterMs` is.
const CHILD_IDLE_MS: u64 = 1_800;

/// The shortened debounce. Long enough that it is a real window (the actionability re-check still
/// runs against live state at fire time), short enough that the notice lands well inside
/// `CHILD_IDLE_MS`.
const TEST_DEBOUNCE: Duration = Duration::from_millis(80);

fn fixture_binary_path() -> PathBuf {
    crate::support::bins::subagent_fixture()
}

fn write_fixture_persona(cwd: &std::path::Path, name: &str) {
    let agents_dir = cwd.join(".cyrup").join("agents");
    std::fs::create_dir_all(&agents_dir).expect("mkdir .cyrup/agents");
    std::fs::write(
        agents_dir.join(format!("{name}.md")),
        format!(
            "---\nname: {name}\ndescription: a trivial fixture persona for the SUBA-N05 control test\n\
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

/// A child that emits NOTHING for `CHILD_IDLE_MS`, then produces its answer and exits cleanly.
/// The silent window is what the `needsAttentionAfterMs` heuristic measures.
fn idle_then_answer_script() -> serde_json::Value {
    serde_json::json!({
        "steps": [
            { "kind": "sleep_ms", "ms": CHILD_IDLE_MS },
            { "kind": "emit", "line": message_end_line("SUBA_N05 child answer") }
        ],
        "exit_code": 0
    })
}

/// A `HostServices` backend that records every `inject_message` and advertises a live session
/// identity — the same P-1 seam `SubagentExecutor::set_host_services` takes in production.
///
/// The session id/name matter beyond identity: `SubagentExecutor::orchestrator_intercom_target`
/// derives this orchestrator's own presence label from them, which is cyrup's equivalent of pi's
/// `intercomBridge.active && intercomBridge.orchestratorTarget` predicate — the gate
/// `emitControlNotification` uses to decide whether to resolve a `childIntercomTarget` at all
/// (`subagent-executor.ts:512-513`).
#[derive(Default)]
struct RecordingHostServices {
    /// `(content, custom_type, display, trigger_turn)` per call.
    calls: Mutex<Vec<(String, Option<String>, bool, bool)>>,
}

impl RecordingHostServices {
    fn control_notices(&self) -> Vec<(String, bool, bool)> {
        self.calls
            .lock()
            .expect("inject lock")
            .iter()
            .filter(|(_, custom_type, _, _)| {
                custom_type.as_deref() == Some(CONTROL_NOTICE_CUSTOM_TYPE)
            })
            .map(|(content, _, display, trigger)| (content.clone(), *display, *trigger))
            .collect()
    }

    /// Poll until at least one control notice has been recorded, or `deadline` elapses.
    ///
    /// A bounded poll rather than a fixed sleep because the production sink hands the injection to
    /// `tokio::task::spawn_blocking` and deliberately does not await it (pi does not await
    /// `sendMessage` either) — so "has it landed yet" is genuinely asynchronous, and asserting on a
    /// fixed sleep would be either flaky or slow.
    async fn await_control_notice(&self, deadline: Duration) -> Vec<(String, bool, bool)> {
        let started = Instant::now();
        loop {
            let notices = self.control_notices();
            if !notices.is_empty() || started.elapsed() >= deadline {
                return notices;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

impl HostServices for RecordingHostServices {
    fn session_id(&self) -> Option<String> {
        Some("suban05sessionid".to_string())
    }

    fn session_name(&self) -> Option<String> {
        Some("orchestrator".to_string())
    }

    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        trigger_turn: bool,
    ) -> Result<(), String> {
        self.calls.lock().expect("inject lock").push((
            content.to_string(),
            custom_type.map(str::to_string),
            display,
            trigger_turn,
        ));
        Ok(())
    }
}

/// One scenario's outcome: the tool's own `details` (so `controlEvents` can be inspected) plus
/// whatever the transcript sink received.
struct Outcome {
    details: serde_json::Value,
    notices: Vec<(String, bool, bool)>,
}

/// Run one real SINGLE `subagent` call with `control_param`, under the shortened debounce.
async fn run_single_with_control(control_param: Option<serde_json::Value>) -> Outcome {
    run_single_with_control_and_debounce(control_param, Some(TEST_DEBOUNCE)).await
}

/// Run one real SINGLE `subagent` call with `control_param`. `debounce: None` leaves the
/// PRODUCTION window (`tui::notices::DEBOUNCE_MS`, 1000 ms) in place; `Some(_)` installs an
/// explicit one through the same `set_control_notice_debounce` seam production never uses.
async fn run_single_with_control_and_debounce(
    control_param: Option<serde_json::Value>,
    debounce: Option<Duration>,
) -> Outcome {
    let work_dir = tempfile::tempdir().expect("cwd tempdir");
    let home_dir = tempfile::tempdir().expect("CYRUP_HOME tempdir");
    write_fixture_persona(work_dir.path(), "worker");

    let script_path = work_dir.path().join("fixture-script.json");
    std::fs::write(&script_path, idle_then_answer_script().to_string()).expect("write script");

    let fixture = fixture_binary_path();
    // SAFETY: scoped, mutex-serialized env mutation for the duration of this one call (Rust 2024
    // requires `unsafe` for `set_var`; this integration file is a separate compilation unit from
    // the crate's `#![forbid(unsafe_code)]` lib, exactly like every sibling test).
    unsafe {
        std::env::set_var(FIXTURE_BINARY_ENV_VAR, &fixture);
        std::env::set_var(FIXTURE_SCRIPT_ENV_VAR, &script_path);
        std::env::set_var("CYRUP_HOME", home_dir.path());
    }

    let services = Arc::new(RecordingHostServices::default());
    let extension = SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        work_dir.path().to_path_buf(),
    );
    // The P-1 binding + the SessionStart anchor capture, in the order production performs them.
    extension.executor().set_host_services(services.clone());
    extension.executor().capture_parent_session_anchor();
    if let Some(debounce) = debounce {
        extension.executor().set_control_notice_debounce(debounce).await;
    }

    let mut params = serde_json::json!({ "agent": "worker", "task": "idle for a while" });
    if let Some(control) = control_param {
        params
            .as_object_mut()
            .expect("object literal")
            .insert("control".to_string(), control);
    }

    let result = extension
        .subagent_tool()
        .execute(
            ToolCallId::from("suba-n05"),
            params,
            CancelToken::new(),
            Box::new(|_u: cyrup_core::ToolUpdate| {}),
        )
        .await;

    // The sink's injection is fire-and-forget through `spawn_blocking`; give it a bounded window.
    let notices = services.await_control_notice(Duration::from_secs(2)).await;

    // SAFETY: scoped cleanup under the same held env lock.
    unsafe {
        std::env::remove_var(FIXTURE_BINARY_ENV_VAR);
        std::env::remove_var(FIXTURE_SCRIPT_ENV_VAR);
        std::env::remove_var("CYRUP_HOME");
    }

    let result = result.expect("the run must COMPLETE, not be refused at dispatch");
    Outcome {
        details: result.details.unwrap_or(serde_json::Value::Null),
        notices,
    }
}

/// The raised control events off a SETTLED single-run `details`.
///
/// pi's `Details` puts them on `results[i].controlEvents` (`shared/types.ts:950-1042`, populated by
/// `snapshotResult` at `runs/foreground/execution.ts:256` from `result.controlEvents` set at
/// `:975`), NOT at the details root: `runSinglePath`'s own details object
/// (`subagent-executor.ts:3811-3823` @v0.43.0) has no `controlEvents` key at all. The root
/// `controlEvents` upstream does emit belongs to the LIVE `onUpdate` snapshot only (`:982-987`).
///
/// This read used to be `details["controlEvents"]`, which worked solely because cyrup emitted the
/// bare `SingleResult` AT the details root — the port bug that also made the `subagent` tool's
/// result unrenderable. With `details` now pi-shaped (`{mode, runId, results:[r], …}`) the correct
/// path is the nested one, and `results[0].controlEvents` is the SAME `SingleResult::control_events`
/// value this test has always been about.
fn control_events(details: &serde_json::Value) -> Vec<serde_json::Value> {
    details
        .get("results")
        .and_then(|v| v.as_array())
        .and_then(|results| results.first())
        .and_then(|result| result.get("controlEvents"))
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
}

/// THE PROOF: `control: { needsAttentionAfterMs: 1 }` on a real SINGLE call makes the child's
/// silent window trip the attention heuristic, and the resulting notice is INJECTED into the
/// orchestrator transcript through the real `HostServices` seam.
///
/// Every assertion is about a value only the full pipeline can produce:
/// - the raised event lands on `SingleResult::control_events` (`run_sync` folded
///   `ControlMonitor::into_events`), proving the config reached `RunOptions::control_config` and
///   the monitor judged the REAL child stream against it;
/// - the notice arrives with pi's `SUBAGENT_CONTROL_MESSAGE_TYPE`, `display: true`, and
///   `triggerTurn: false` (R-SA-118: a foreground notice never forces a new orchestrator turn,
///   pi's `triggerTurn: input.details.source !== "foreground"`);
/// - the body is `formatControlNoticeMessage`'s, including the steer/resume command hints;
/// - and it names a `Direct intercom target:` derived from
///   `resolveSubagentIntercomTarget(runId, agent, index)` — which is only resolved when the bridge
///   predicate holds, so it also proves `childIntercomTarget` is wired rather than hardcoded
///   `None`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_single_call_carrying_control_fires_the_real_notice_pipeline() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let outcome =
        run_single_with_control(Some(serde_json::json!({ "needsAttentionAfterMs": 1 }))).await;

    // (1) The override reached the child's judging thresholds.
    let events = control_events(&outcome.details);
    assert!(
        !events.is_empty(),
        "the run must have RAISED a control event; details were {}",
        outcome.details
    );
    let attention = events
        .iter()
        .find(|e| e.get("type").and_then(|t| t.as_str()) == Some("needs_attention"))
        .unwrap_or_else(|| panic!("no needs_attention event among {events:?}"));
    assert_eq!(attention.get("agent").and_then(|a| a.as_str()), Some("worker"));
    assert_eq!(attention.get("reason").and_then(|r| r.as_str()), Some("idle"));

    // (2) The notice reached the transcript, through the production sink.
    assert_eq!(
        outcome.notices.len(),
        1,
        "exactly one control notice must have been injected; got {:?}",
        outcome.notices
    );
    let (body, display, trigger_turn) = &outcome.notices[0];
    assert!(*display, "a control notice is a visible transcript entry (R-SA-121)");
    assert!(
        !*trigger_turn,
        "a FOREGROUND notice must not force a new orchestrator turn (R-SA-118 / pi \
         `triggerTurn: source !== \"foreground\"`)"
    );

    // (3) The body is `formatControlNoticeMessage`'s, not a placeholder.
    for expected in [
        "Subagent needs attention: worker",
        "Run: ",
        "Signal: ",
        "Top-level live async nudge: subagent({ action: \"steer\"",
        "Routed live nested nudge: subagent({ action: \"resume\"",
        "Status: subagent({ action: \"status\"",
        "Interrupt: subagent({ action: \"interrupt\"",
    ] {
        assert!(body.contains(expected), "notice body missing {expected:?}; got:\n{body}");
    }

    // (4) `childIntercomTarget` is resolved, not hardcoded `None`.
    assert!(
        body.contains("Direct intercom target: subagent-worker-"),
        "the notice must carry the child's resolved intercom target (pi \
         `resolveSubagentIntercomTarget(event.runId, event.agent, event.index)`); got:\n{body}"
    );
    // `index: 0` on the foreground SINGLE path renders pi's 1-based step suffix.
    assert!(
        body.lines().any(|l| l.starts_with("Direct intercom target: ") && l.ends_with("-1")),
        "the target must carry the `-{{index + 1}}` step suffix; got:\n{body}"
    );
}

/// The DISCRIMINATOR for the test above: the identical run WITHOUT `control` raises nothing and
/// injects nothing.
///
/// Without this, "a notice fired" would be consistent with the notice firing unconditionally. The
/// stock `needsAttentionAfterMs` is 60 000 ms (`DEFAULT_CONTROL_CONFIG`,
/// `shared/subagent-control.ts:16`), and this child idles for well under two seconds, so the only
/// thing that can turn the notice on is the per-call override actually taking effect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_same_run_without_control_raises_nothing_and_injects_nothing() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let outcome = run_single_with_control(None).await;

    assert!(
        control_events(&outcome.details).is_empty(),
        "a ~{CHILD_IDLE_MS}ms idle is far inside the stock 60s attention window, so no control \
         event may be raised; details were {}",
        outcome.details
    );
    assert!(
        outcome.notices.is_empty(),
        "and therefore no transcript notice; got {:?}",
        outcome.notices
    );
}

/// The `notifyChannels` gate, end to end (pi `subagent-executor.ts:817`).
///
/// `notifyChannels: ["intercom"]` must still RAISE the event — it lands on
/// `SingleResult::control_events` exactly as before — while delivering NO transcript notice,
/// because the `event` channel is not in the list. This is the half that had zero runtime
/// consumers before SUBA-N05: the notifier had no channel check at all, so this configuration
/// still produced a transcript notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notify_channels_without_event_raises_the_event_but_delivers_no_transcript_notice() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let outcome = run_single_with_control(Some(serde_json::json!({
        "needsAttentionAfterMs": 1,
        "notifyChannels": ["intercom"]
    })))
    .await;

    assert!(
        control_events(&outcome.details)
            .iter()
            .any(|e| e.get("type").and_then(|t| t.as_str()) == Some("needs_attention")),
        "the CHANNEL gate must not suppress the raise — `notifyOn` is what does that; details \
         were {}",
        outcome.details
    );
    assert!(
        outcome.notices.is_empty(),
        "with `event` absent from notifyChannels no transcript notice may be delivered; got {:?}",
        outcome.notices
    );
}

/// The `notifyOn` gate, end to end (pi `shouldNotifyControlEvent`,
/// `shared/subagent-control.ts:137-139`).
///
/// `notifyOn: ["active_long_running"]` suppresses the raise itself, so `needs_attention` never
/// reaches `SingleResult::control_events` and no notice can follow. This is a strictly earlier gate
/// than `notifyChannels` and the two must not be conflated.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn notify_on_without_needs_attention_suppresses_the_raise_itself() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let outcome = run_single_with_control(Some(serde_json::json!({
        "needsAttentionAfterMs": 1,
        "notifyOn": ["active_long_running"]
    })))
    .await;

    assert!(
        !control_events(&outcome.details)
            .iter()
            .any(|e| e.get("type").and_then(|t| t.as_str()) == Some("needs_attention")),
        "`notifyOn` excludes needs_attention, so it must never be raised; details were {}",
        outcome.details
    );
    assert!(outcome.notices.is_empty(), "and no notice; got {:?}", outcome.notices);
}

/// ADVERSARIAL — "can a control notice be LOST if the child exits promptly after emitting?"
///
/// **Yes, and that is upstream's behaviour, not a cyrup defect.** This test pins it so nobody
/// "fixes" it into a divergence, and so the mechanism is written down somewhere other than a
/// comment.
///
/// A foreground notice is held for a debounce window and re-validated against LIVE state when the
/// timer fires (R-SA-116 / pi `handleSubagentControlNotice`'s `setTimeout` +
/// `isForegroundNoticeStillActionable`, `control-notices.ts:67-92` @v0.34.0). When a run settles, pi calls
/// `clearPendingForegroundControlNotices(deps.state, runId)` and then deletes its
/// `foregroundControls` entry (`subagent-executor.ts:3579-3581` @v0.34.0) — so a timer that has not
/// yet fired is CANCELLED, and one that fires later would fail `if (!control) return false` anyway.
/// cyrup's `ControlNoticeState::forget_run` does both, in the same order.
///
/// The concrete shape: this child raises `needs_attention` on the first 1 s activity tick and exits
/// ~800 ms later. Whenever the debounce window outlives that remainder — which the PRODUCTION
/// 1000 ms window does for exactly this run — the notice is raised, recorded on
/// `SingleResult::control_events` (the caller can still see it), and never surfaces in the
/// transcript. The only difference from
/// [`a_single_call_carrying_control_fires_the_real_notice_pipeline`] is the debounce, which is
/// precisely the variable this behaviour turns on.
///
/// The window is set to 30 s rather than left at the production 1000 ms deliberately: with the
/// production value the margin is ~200 ms of wall clock, and "the notice loses a race it is
/// supposed to lose" is not an assertion worth having if a loaded machine can flip it. 30 s makes
/// the outcome structural — the run CANNOT outlive the window — while testing the identical code
/// path, since nothing between raise and fire depends on the window's magnitude.
///
/// This is also the reason the pump exists: with the previous spawn-per-event hand-off the outcome
/// here was not "reliably dropped" but "dropped or delivered depending on whether a task got polled
/// before teardown", and a late hand-off left the finished run permanently resurrected in
/// `live_runs`. Deterministic loss is a behaviour; a race is a bug.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_notice_still_debouncing_when_the_child_exits_is_dropped_exactly_as_pi_drops_it() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let outcome = run_single_with_control_and_debounce(
        Some(serde_json::json!({ "needsAttentionAfterMs": 1 })),
        Some(Duration::from_secs(30)),
    )
    .await;

    assert!(
        control_events(&outcome.details)
            .iter()
            .any(|e| e.get("type").and_then(|t| t.as_str()) == Some("needs_attention")),
        "the event is still RAISED and still reaches the caller on `controlEvents` — only the \
         transcript notice is dropped; details were {}",
        outcome.details
    );
    assert!(
        outcome.notices.is_empty(),
        "a notice whose debounce window outlives the run must be dropped, never delivered about a \
         run that is already over (pi `clearPendingForegroundControlNotices` + \
         `isForegroundNoticeStillActionable`); got {:?}",
        outcome.notices
    );
}

/// `control: { enabled: false }` turns the whole subsystem off for this run (pi
/// `ResolvedControlConfig.enabled`, checked at the top of every raise path).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn control_enabled_false_disables_every_raise_path_for_the_run() {
    let _guard = ENV_MUTATION_LOCK.lock().await;

    let outcome = run_single_with_control(Some(serde_json::json!({
        "enabled": false,
        "needsAttentionAfterMs": 1
    })))
    .await;

    assert!(
        control_events(&outcome.details).is_empty(),
        "`enabled: false` must win over an aggressive threshold; details were {}",
        outcome.details
    );
    assert!(outcome.notices.is_empty(), "and no notice; got {:?}", outcome.notices);
}
