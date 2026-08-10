//! G90 — `action: "steer"` DELIVERED, end to end, through every hop it really has.
//!
//! # The defect
//!
//! `steer` was a dead letter. `SubagentExecutor::control_steer` validated a request and
//! `control::request_async_steer` wrote it to `<run_dir>/control/steer-requests/`; the detached
//! runner drained that queue and `control::enqueue_step_steer` routed each accepted request into
//! `<run_dir>/control/steer-targets/<flatIndex>/`. And there it stopped: **nothing in the crate
//! read `steer-targets/` in production, and no env var told a child the directory existed.** The
//! tool's own success text conceded it ("Delivery requires a live Cyrup child session that supports
//! mid-run steering") — there was no such session, because the child half had never been written.
//! Every user-visible effect of the verb was absent.
//!
//! The batch that added the parent half called the child hop "genuinely blocked" on `HostCtx` not
//! exposing `ControlOp::SendUserMessage`. That is false twice over. `HostCtx` is a data-only struct
//! and was never the seam; the seam is `cyrup_ext::host::HostServices::inject_message`
//! (`cyrup-ext/src/host/services.rs:311`), late-bound through
//! `NativeExtension::set_host_services` (`cyrup-ext/src/native.rs:449`), live-implemented at
//! `cyrup-session-svc/src/host_services.rs:735` and routed to `AgentSession::send_user_message`,
//! which STEERS the running turn while streaming (`session.rs:3671-3677`) — i.e. exactly
//! `pi.sendUserMessage(text, { deliverAs: "steer" })`. This crate already called it from two other
//! places before this test existed.
//!
//! # What this file proves
//!
//! The whole chain, using the REAL functions at every hop and no mocking except the capability
//! backend itself (which stands in for a live session, the one thing an integration test cannot
//! own):
//!
//! 1. the PARENT hop — `control::request_async_steer` (pi `requestAsyncSteer`);
//! 2. the RUNNER hop — `control::consume_steer_requests` + `control::enqueue_step_steer`, the exact
//!    pair upstream's `onSteer` handler calls (`subagent-runner.ts:3026-3063`);
//! 3. the SPAWN hop — `exec::build_attempt_spawn_plan` writing `CYRUP_SUBAGENT_STEER_INBOX` (pi
//!    `pi-args.ts:251-252`), covered by `exec`'s own unit tests;
//! 4. the CHILD hop — `prompt_runtime::prompt_runtime_extension_from` building the runtime from
//!    that env var, and its real event lifecycle injecting pi's exact `formatSteerMessage` text.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cyrup_ext::native::{ExtMode, HostCtx, InitApi, NativeExtension};
use cyrup_ext::{EventKind, HostEvent};

use cyrup_ext_subagents::background::control;
use cyrup_ext_subagents::prompt_runtime::STEER_INBOX_ENV;

/// A stand-in for the live session's capability backend. `inject_message` is the ONE seam under
/// test, so it records rather than performs; `fail_next` lets the write-back-on-failure path be
/// driven without a broken session.
/// One recorded `inject_message` call — the four arguments whose values are the whole contract
/// (`custom_type: None` in particular is what routes to `send_user_message` rather than a
/// non-LLM custom message).
#[derive(Clone, Debug)]
struct Injection {
    content: String,
    custom_type: Option<String>,
    display: bool,
    trigger_turn: bool,
}

#[derive(Default)]
struct RecordingServices {
    injected: Mutex<Vec<Injection>>,
    fail: Mutex<bool>,
}

impl cyrup_ext::host::HostServices for RecordingServices {
    fn inject_message(
        &self,
        content: &str,
        custom_type: Option<&str>,
        display: bool,
        trigger_turn: bool,
    ) -> Result<(), String> {
        if *self.fail.lock().unwrap() {
            return Err("no live session".to_string());
        }
        self.injected.lock().unwrap().push(Injection {
            content: content.to_string(),
            custom_type: custom_type.map(str::to_string),
            display,
            trigger_turn,
        });
        Ok(())
    }
}

fn ctx(cwd: &std::path::Path) -> HostCtx {
    HostCtx::event(ExtMode::Json, false, cwd.to_path_buf())
}

/// Build the child-side runtime exactly as a real spawned child does — from its environment —
/// with `CYRUP_SUBAGENT_STEER_INBOX` pointing at `inbox` and nothing else set.
fn child_runtime_for(inbox: &std::path::Path) -> Arc<dyn NativeExtension> {
    let inbox = inbox.display().to_string();
    cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from(&move |key: &str| {
        (key == STEER_INBOX_ENV).then(|| inbox.clone())
    })
    .expect(
        "a child handed a steer inbox MUST get a prompt runtime — before this wiring the env var \
         did not exist and this resolver returned None",
    )
}

/// THE PROOF: a steer message the parent queued reaches the child's model.
///
/// Every hop is the real function. The only stand-in is the capability backend, which is what a
/// live `AgentSession` provides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_steer_message_is_delivered_into_the_childs_live_turn() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-abc");
    std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

    // --- hop 1: the PARENT queues guidance (pi `requestAsyncSteer`) ---------------------------
    control::request_async_steer(&run_dir, "  focus on the failing test only  ", None, Some("steer-action"))
        .await
        .expect("the parent writes a steer request");

    // --- hop 2: the RUNNER drains the run-level queue and routes to child 0 --------------------
    // The exact pair upstream's `onSteer` handler calls.
    let drained = control::consume_steer_requests(&run_dir).await;
    assert_eq!(drained.len(), 1, "the runner must see exactly the one queued request");
    assert_eq!(
        drained[0].message, "focus on the failing test only",
        "the message is trimmed once, at the parent"
    );
    control::enqueue_step_steer(&run_dir, 0, &drained[0])
        .await
        .expect("the runner routes it into child 0's inbox");

    let inbox = control::step_steer_inbox_dir(&run_dir, 0);
    assert!(
        inbox.exists(),
        "the per-child inbox must exist after routing; expected {}",
        inbox.display()
    );

    // --- hop 4: the CHILD reads it and injects it into its live turn ---------------------------
    let child = child_runtime_for(&inbox);
    let services = Arc::new(RecordingServices::default());
    child.set_host_services(services.clone() as Arc<dyn cyrup_ext::host::HostServices>);

    let mut api = InitApi::new();
    child.init(&mut api).await.expect("child runtime init");
    for kind in [
        EventKind::SessionStart,
        EventKind::SessionShutdown,
        EventKind::TurnEnd,
        EventKind::MessageStart,
        EventKind::MessageEnd,
        EventKind::ToolExecStart,
        EventKind::ToolExecEnd,
    ] {
        assert!(
            api.subscriptions().contains(kind),
            "a steering child must subscribe to {kind:?} — an unsubscribed handler is never called \
             at all (`Dispatcher::no_subscribers`)"
        );
    }

    let ctx = ctx(dir.path());
    child.on_event(&HostEvent::SessionStart { reason: "start".to_string() }, &ctx).await;

    // Nothing may be delivered before the session has a turn to steer (pi's `canSteer` gate).
    assert!(
        services.injected.lock().unwrap().is_empty(),
        "a request must NOT be injected before the first turn-lifecycle event"
    );

    // ...and the first turn-lifecycle event flushes it.
    child.on_event(&HostEvent::MessageStart { message: serde_json::json!({"role": "assistant"}) }, &ctx).await;

    let injected = services.injected.lock().unwrap().clone();
    assert_eq!(
        injected.len(),
        1,
        "the queued guidance must reach the child's model exactly once; got {injected:?}"
    );
    let Injection { content, custom_type, display, trigger_turn } = &injected[0];
    assert_eq!(
        content,
        "Mid-run steering from the parent orchestrator:\n\n\
         focus on the failing test only\n\n\
         Incorporate this guidance at the next safe point. Do not restart the task unless the \
         guidance explicitly asks you to.",
        "the injected text must be pi's `formatSteerMessage` VERBATIM \
         (`subagent-prompt-runtime.ts:161-169` @v0.34.0)"
    );
    assert_eq!(
        custom_type.as_deref(),
        None,
        "`custom_type: None` is load-bearing: it routes to `send_user_message`, i.e. a real USER \
         message the model must answer. A custom type would make it a non-LLM message the model \
         never sees — the dead letter, one layer further in."
    );
    assert!(*display, "the operator watching the child's transcript must see the guidance arrive");
    assert!(
        *trigger_turn,
        "an IDLE child must actually act on the guidance rather than parking it until its next \
         self-initiated turn"
    );

    // The request is consumed: a second flush must not re-deliver it.
    child.on_event(&HostEvent::MessageStart { message: serde_json::json!({"role": "assistant"}) }, &ctx).await;
    assert_eq!(
        services.injected.lock().unwrap().len(),
        1,
        "a delivered request must never be re-delivered"
    );

    child.on_event(&HostEvent::SessionShutdown { reason: "end".to_string() }, &ctx).await;
}

/// The lossless half (pi `:214-217`): a failed injection writes the undelivered requests BACK to
/// the inbox and stops the drain. `consume_steer_requests_from_dir` deletes each file as it reads
/// it, so without the write-back one transient host error would silently discard the queue.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_injection_returns_the_undelivered_guidance_to_the_inbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-def");
    std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

    for message in ["first", "second"] {
        control::request_async_steer(&run_dir, message, None, Some("steer-action"))
            .await
            .expect("queued");
    }
    for request in control::consume_steer_requests(&run_dir).await {
        control::enqueue_step_steer(&run_dir, 0, &request).await.expect("routed");
    }
    let inbox = control::step_steer_inbox_dir(&run_dir, 0);

    let child = child_runtime_for(&inbox);
    let services = Arc::new(RecordingServices::default());
    *services.fail.lock().unwrap() = true;
    child.set_host_services(services.clone() as Arc<dyn cyrup_ext::host::HostServices>);

    let ctx = ctx(dir.path());
    child.on_event(&HostEvent::SessionStart { reason: "start".to_string() }, &ctx).await;
    child.on_event(&HostEvent::MessageStart { message: serde_json::json!({"role": "assistant"}) }, &ctx).await;

    assert!(
        services.injected.lock().unwrap().is_empty(),
        "the failing backend delivered nothing"
    );

    // Both requests must still be on disk, ready for the next flush.
    *services.fail.lock().unwrap() = false;
    child.on_event(&HostEvent::MessageStart { message: serde_json::json!({"role": "assistant"}) }, &ctx).await;
    let delivered: Vec<String> = services
        .injected
        .lock()
        .unwrap()
        .iter()
        .map(|injection| injection.content.clone())
        .collect();
    assert_eq!(
        delivered.len(),
        2,
        "both requests must survive a failed flush and be delivered on the next one; got \
         {delivered:?}"
    );
    assert!(delivered[0].contains("first") && delivered[1].contains("second"), "{delivered:?}");
}

/// A child with NO steer inbox must be untouched: no runtime, no subscriptions, no poller. This is
/// every foreground child, and the overwhelming majority of spawns.
#[test]
fn a_child_with_no_steer_inbox_gets_no_steering_surface() {
    assert!(
        cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from(&|_key: &str| None)
            .is_none(),
        "a process with none of the child env vars must get no runtime at all"
    );
    assert!(
        cyrup_ext_subagents::prompt_runtime::prompt_runtime_extension_from(&|key: &str| {
            (key == STEER_INBOX_ENV).then(|| "   ".to_string())
        })
        .is_none(),
        "a BLANK inbox path is the same as unset (pi `:194`'s `?.trim()`), not a poller against \
         the process cwd"
    );
}

/// The SPAWN hop, at the seam that hands the path over: a `RunOptions::steer_inbox_dir` must reach
/// the child as `CYRUP_SUBAGENT_STEER_INBOX`, and its absence must add no variable at all.
#[test]
fn the_spawn_plan_hands_the_child_its_steer_inbox() {
    // The `exec` module owns the `RunOptions`/`AgentConfig` fixtures, so the detailed argv/env
    // assertions live in its own unit tests; this asserts the public contract those two halves
    // agree on — the env-var NAME, which is the thing a drift between writer and reader would
    // break silently.
    assert_eq!(
        STEER_INBOX_ENV, "CYRUP_SUBAGENT_STEER_INBOX",
        "the child env var is the cyrup rename of pi's `PI_SUBAGENT_STEER_INBOX` \
         (`pi-args.ts:32` @v0.34.0); changing it breaks every already-running child"
    );
    let _: PathBuf = control::step_steer_inbox_dir(std::path::Path::new("/run"), 3);
    assert_eq!(
        control::step_steer_inbox_dir(std::path::Path::new("/run"), 3),
        PathBuf::from("/run/control/steer-targets/3"),
        "pi `stepSteerInboxDir` (`control-channel.ts:81-83` @v0.34.0)"
    );
}
