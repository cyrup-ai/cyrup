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
//!    `runs/shared/pi-args.ts:251-252`), covered by `exec`'s own unit tests;
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

use crate::background::control;
use crate::prompt_runtime::STEER_INBOX_ENV;

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
        _details: Option<&serde_json::Value>,
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
    crate::prompt_runtime::prompt_runtime_extension_from(&move |key: &str| {
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
    control::request_async_steer(
        &run_dir,
        "  focus on the failing test only  ",
        None,
        Some("steer-action"),
    )
    .await
    .expect("the parent writes a steer request");

    // --- hop 2: the RUNNER drains the run-level queue and routes to child 0 --------------------
    // The exact pair upstream's `onSteer` handler calls.
    let drained = control::consume_steer_requests(&run_dir).await;
    assert_eq!(
        drained.len(),
        1,
        "the runner must see exactly the one queued request"
    );
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
    child
        .on_event(
            &HostEvent::SessionStart {
                reason: "start".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;

    // Nothing may be delivered before the session has a turn to steer (pi's `canSteer` gate).
    assert!(
        services.injected.lock().unwrap().is_empty(),
        "a request must NOT be injected before the first turn-lifecycle event"
    );

    // ...and the first turn-lifecycle event flushes it.
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;

    let injected = services.injected.lock().unwrap().clone();
    assert_eq!(
        injected.len(),
        1,
        "the queued guidance must reach the child's model exactly once; got {injected:?}"
    );
    let Injection {
        content,
        custom_type,
        display,
        trigger_turn,
    } = &injected[0];
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
    assert!(
        *display,
        "the operator watching the child's transcript must see the guidance arrive"
    );
    assert!(
        *trigger_turn,
        "an IDLE child must actually act on the guidance rather than parking it until its next \
         self-initiated turn"
    );

    // The request is consumed: a second flush must not re-deliver it.
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;
    assert_eq!(
        services.injected.lock().unwrap().len(),
        1,
        "a delivered request must never be re-delivered"
    );

    child
        .on_event(
            &HostEvent::SessionShutdown {
                reason: "end".to_string(),
                target_session_file: None,
            },
            &ctx,
        )
        .await;
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
        control::enqueue_step_steer(&run_dir, 0, &request)
            .await
            .expect("routed");
    }
    let inbox = control::step_steer_inbox_dir(&run_dir, 0);

    let child = child_runtime_for(&inbox);
    let services = Arc::new(RecordingServices::default());
    *services.fail.lock().unwrap() = true;
    child.set_host_services(services.clone() as Arc<dyn cyrup_ext::host::HostServices>);

    let ctx = ctx(dir.path());
    child
        .on_event(
            &HostEvent::SessionStart {
                reason: "start".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;

    assert!(
        services.injected.lock().unwrap().is_empty(),
        "the failing backend delivered nothing"
    );

    // SUBA-049 CHANGED THIS ASSERTION, and the change is a parity correction rather than a
    // regression. It used to require BOTH requests to survive and be re-delivered, because with no
    // acknowledgment channel a failed request that was not written back was simply lost — retrying
    // it was the only way to avoid silent loss. Upstream does NOT retry the failed one: it
    // acknowledges it `failed` and writes back only `requests.slice(index + 1)`
    // (`subagent-prompt-runtime.ts:390-391` @v0.43.0). Now that the ack path exists, the failed
    // request is REPORTED rather than silently re-attempted — and re-attempting it would be worse
    // than upstream, since a request the host has already rejected once would be delivered late and
    // out of order behind guidance the parent sent afterwards.
    *services.fail.lock().unwrap() = false;
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;
    let delivered: Vec<String> = services
        .injected
        .lock()
        .unwrap()
        .iter()
        .map(|injection| injection.content.clone())
        .collect();
    assert_eq!(
        delivered.len(),
        1,
        "only the requests AFTER the failed one are written back and re-delivered (pi \
         `requests.slice(index + 1)`); got {delivered:?}"
    );
    assert!(
        delivered[0].contains("second"),
        "the survivor must be the one that never got its turn, not the one that failed: \
         {delivered:?}"
    );
}

// =================================================================================================
// SUBA-049 — the RETURN half: capability, delivery mode, and acknowledgment
//
// Before this, every one of the assertions below was unreachable: `rg 'STEER_ACK|steer_ack|
// STEER_CAPABILITY' crates/cyrup-ext-subagents/src` was zero-hit, `SteerRequest` had no `mode`
// field, and the child consumed a request and wrote nothing back. A steer that landed, one that was
// refused by a host with no injection capability, and one that fell into a full follow-up queue
// were three identical successes at the tool boundary.
// =================================================================================================

/// Build a child runtime with the FULL steering channel from its environment, exactly as a real
/// spawned background child gets it: request inbox, acknowledgment directory, capability file and
/// flat index.
fn child_runtime_with_return_path(
    run_dir: &std::path::Path,
    index: usize,
) -> Arc<dyn NativeExtension> {
    let inbox = control::step_steer_inbox_dir(run_dir, index)
        .display()
        .to_string();
    let acks = control::steer_acks_dir(run_dir, index)
        .display()
        .to_string();
    let capability = control::steer_capability_path(run_dir, index)
        .display()
        .to_string();
    let index = index.to_string();
    crate::prompt_runtime::prompt_runtime_extension_from(&move |key: &str| {
        if key == STEER_INBOX_ENV {
            Some(inbox.clone())
        } else if key == crate::prompt_runtime::STEER_ACK_DIR_ENV {
            Some(acks.clone())
        } else if key == crate::prompt_runtime::STEER_CAPABILITY_ENV {
            Some(capability.clone())
        } else if key == crate::spawn::nested_events::CHILD_INDEX_ENV {
            Some(index.clone())
        } else {
            None
        }
    })
    .expect("a child handed a steer inbox MUST get a prompt runtime")
}

/// THE ITEM'S OWN VERIFY, first half: *"Steer a child whose follow-up queue is full; the tool must
/// answer `failed` with upstream's 'Follow-up queue is full (N messages).' text rather than
/// success."*
///
/// **Pre-fix this test cannot even be written**: `MAX_STEER_QUEUE_SIZE` did not exist, there was no
/// `mode` to request follow-up delivery with, no follow-up queue to fill, and no acknowledgment
/// file to read the refusal out of. Every one of the 21 requests below was simply injected.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_full_follow_up_queue_is_refused_with_pis_exact_text() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-queue");
    std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

    // One more than the cap, all in `follow_up` mode so every one of them parks.
    let over = control::MAX_STEER_QUEUE_SIZE + 1;
    let mut ids = Vec::with_capacity(over);
    for n in 0..over {
        let (_, id) = control::request_async_steer_with_mode(
            &run_dir,
            &format!("guidance {n}"),
            Some(control::SteerDeliveryMode::FollowUp),
            None,
            Some("steer-action"),
        )
        .await
        .expect("queued");
        ids.push(id);
    }
    for request in control::consume_steer_requests(&run_dir).await {
        assert_eq!(
            request.mode,
            Some(control::SteerDeliveryMode::FollowUp),
            "a non-default mode must survive the wire — it is what selects the queue at all"
        );
        control::enqueue_step_steer(&run_dir, 0, &request)
            .await
            .expect("routed");
    }

    let child = child_runtime_with_return_path(&run_dir, 0);
    let services = Arc::new(RecordingServices::default());
    child.set_host_services(services.clone() as Arc<dyn cyrup_ext::host::HostServices>);
    let ctx = ctx(dir.path());
    child
        .on_event(
            &HostEvent::SessionStart {
                reason: "start".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;

    assert!(
        services.injected.lock().unwrap().is_empty(),
        "a `follow_up` steer must NOT be injected on arrival — that is the whole difference \
         between it and `steer`"
    );

    let acks = control::consume_steer_acks(&run_dir).await;
    assert_eq!(
        acks.len(),
        over,
        "every consumed request must be acknowledged exactly once — a consumed-and-unacknowledged \
         request is the fire-and-forget defect this item was filed for; got {acks:?}"
    );
    let queued: Vec<&control::SteerAck> = acks
        .iter()
        .filter(|a| a.state == control::SteerAckState::Queued)
        .collect();
    let failed: Vec<&control::SteerAck> = acks
        .iter()
        .filter(|a| a.state == control::SteerAckState::Failed)
        .collect();
    assert_eq!(
        queued.len(),
        control::MAX_STEER_QUEUE_SIZE,
        "exactly the cap may be parked (pi `MAX_STEER_QUEUE_SIZE`, `control-channel.ts:99`)"
    );
    assert_eq!(
        failed.len(),
        1,
        "exactly the overflowing one is refused; got {failed:?}"
    );
    assert_eq!(
        failed[0].message, "Follow-up queue is full (20 messages).",
        "upstream's refusal sentence, byte for byte (`subagent-prompt-runtime.ts:378` @v0.43.0)"
    );
    assert_eq!(
        failed[0].request_id,
        ids[over - 1],
        "the LAST request is the one that overflows — the queue fills in `(ts, id)` order"
    );
    assert_eq!(failed[0].index, 0, "the ack names the child that answered");
}

/// THE ITEM'S OWN VERIFY, second half: *"Steer with `mode:"follow_up"` and assert the child receives
/// it at the next turn boundary, not mid-turn."*
///
/// **Pre-fix**: `mode` did not exist on the request, in the schema, or as a `control_steer`
/// argument, and the child injected every request the moment it saw one — so a `follow_up` steer
/// was delivered mid-turn, which is the behaviour the mode exists to avoid.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_up_steer_is_held_until_the_next_turn_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-followup");
    std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

    control::request_async_steer_with_mode(
        &run_dir,
        "prefer the smaller refactor",
        Some(control::SteerDeliveryMode::FollowUp),
        None,
        Some("steer-action"),
    )
    .await
    .expect("queued");
    for request in control::consume_steer_requests(&run_dir).await {
        control::enqueue_step_steer(&run_dir, 0, &request)
            .await
            .expect("routed");
    }

    let child = child_runtime_with_return_path(&run_dir, 0);
    let services = Arc::new(RecordingServices::default());
    child.set_host_services(services.clone() as Arc<dyn cyrup_ext::host::HostServices>);
    let ctx = ctx(dir.path());
    child
        .on_event(
            &HostEvent::SessionStart {
                reason: "start".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;

    // A turn is now IN FLIGHT. The request is consumed and parked, not injected — and it is parked
    // NOT-ready, so this very turn cannot deliver it.
    child
        .on_event(
            &HostEvent::TurnStart {
                turn_index: 0,
                timestamp: 0,
            },
            &ctx,
        )
        .await;
    assert!(
        services.injected.lock().unwrap().is_empty(),
        "a `follow_up` steer must not land inside the turn that was already running"
    );
    let acks = control::take_steer_acks(&run_dir, None).await;
    assert_eq!(
        acks.len(),
        1,
        "the park is acknowledged immediately; got {acks:?}"
    );
    assert_eq!(acks[0].state, control::SteerAckState::Queued);
    assert_eq!(
        acks[0].delivery_status,
        Some(control::SteerDeliveryStatus::Queued),
        "`deliveryStatus` is what tells the parent the request is alive but not yet delivered"
    );

    // The turn ends: the parked follow-up becomes eligible.
    child
        .on_event(
            &HostEvent::TurnEnd {
                turn_index: 0,
                message: cyrup_agent::AgentMessage::User {
                    content: Vec::new(),
                    timestamp: None,
                },
                tool_results: Vec::new(),
            },
            &ctx,
        )
        .await;
    assert!(
        services.injected.lock().unwrap().is_empty(),
        "turn_end makes it READY, it does not deliver it — delivery is the next turn's job"
    );

    // ...and the NEXT turn boundary delivers it.
    child
        .on_event(
            &HostEvent::TurnStart {
                turn_index: 0,
                timestamp: 0,
            },
            &ctx,
        )
        .await;
    let injected = services.injected.lock().unwrap().clone();
    assert_eq!(
        injected.len(),
        1,
        "delivered exactly once at the boundary; got {injected:?}"
    );
    assert!(
        injected[0].content.contains("prefer the smaller refactor"),
        "{:?}",
        injected[0]
    );
    assert!(
        !injected[0].trigger_turn,
        "a follow-up released INTO a turn that is starting must not start a second one"
    );
    let acks = control::take_steer_acks(&run_dir, None).await;
    assert_eq!(
        acks.len(),
        1,
        "the delivery is acknowledged in its turn; got {acks:?}"
    );
    assert_eq!(acks[0].state, control::SteerAckState::Delivered);
    assert_eq!(
        acks[0].message, "Cyrup delivered the queued follow-up at a turn boundary.",
        "pi `:452`, with this crate's standing Pi->Cyrup product-noun rebrand"
    );
}

/// The capability record, and the two facts that only exist because of it: a child that has reached
/// its runtime says so with its own pid, and one whose host cannot inject says `supported: false`
/// rather than leaving the parent to time out.
///
/// **Pre-fix**: no capability file was ever written by anything.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_publishes_its_steering_capability_and_a_delivered_steer_is_acknowledged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-capability");
    std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

    control::request_async_steer(&run_dir, "tighten the diff", None, Some("steer-action"))
        .await
        .expect("queued");
    for request in control::consume_steer_requests(&run_dir).await {
        control::enqueue_step_steer(&run_dir, 2, &request)
            .await
            .expect("routed");
    }

    let child = child_runtime_with_return_path(&run_dir, 2);
    let services = Arc::new(RecordingServices::default());
    child.set_host_services(services.clone() as Arc<dyn cyrup_ext::host::HostServices>);
    let ctx = ctx(dir.path());
    child
        .on_event(
            &HostEvent::SessionStart {
                reason: "start".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;

    let capability = control::read_steer_capability(&run_dir, 2)
        .await
        .expect("the child must publish a capability record once it is live");
    assert_eq!(
        capability.index, 2,
        "the record names the child it describes"
    );
    assert_eq!(
        capability.pid,
        std::process::id(),
        "the pid is what makes a stale record from a dead child detectable"
    );
    assert!(
        capability.supported,
        "a child whose host services are bound CAN be steered, and must say so"
    );
    assert!(capability.ready_at > 0);

    let acks = control::take_steer_acks(&run_dir, None).await;
    assert_eq!(
        acks.len(),
        1,
        "one request, one acknowledgment; got {acks:?}"
    );
    assert_eq!(acks[0].state, control::SteerAckState::Delivered);
    assert_eq!(acks[0].index, 2);
    assert_eq!(
        acks[0].message, "Cyrup accepted the correlated steering input.",
        "pi `:413`, rebranded"
    );
}

/// A child whose host never bound an injection backend must REFUSE, not silently drop. This is the
/// difference the whole ack path exists to expose: before it, this request vanished and the tool
/// reported success.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_that_cannot_inject_refuses_the_steer_instead_of_dropping_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-nohost");
    std::fs::create_dir_all(&run_dir).expect("mkdir run dir");

    control::request_async_steer(&run_dir, "anything at all", None, Some("steer-action"))
        .await
        .expect("queued");
    for request in control::consume_steer_requests(&run_dir).await {
        control::enqueue_step_steer(&run_dir, 0, &request)
            .await
            .expect("routed");
    }

    // NO `set_host_services` call — this is the headless / never-bound backend.
    let child = child_runtime_with_return_path(&run_dir, 0);
    let ctx = ctx(dir.path());
    child
        .on_event(
            &HostEvent::SessionStart {
                reason: "start".to_string(),
                previous_session_file: None,
            },
            &ctx,
        )
        .await;
    child
        .on_event(
            &HostEvent::MessageStart {
                message: serde_json::json!({"role": "assistant"}),
            },
            &ctx,
        )
        .await;

    let acks = control::take_steer_acks(&run_dir, None).await;
    assert_eq!(acks.len(), 1, "the refusal must be reported; got {acks:?}");
    assert_eq!(acks[0].state, control::SteerAckState::Failed);
    assert_eq!(
        acks[0].message, "Child Cyrup session does not support sendUserMessage steering.",
        "pi `:371`, rebranded"
    );
    let capability = control::read_steer_capability(&run_dir, 0)
        .await
        .expect("a capability record is published even when steering is unsupported");
    assert!(
        !capability.supported,
        "`supported: false` is the fact that saves the parent from waiting out the ack timeout"
    );
}

/// Two parents steering the same run must not consume each other's acknowledgments. This is the
/// narrowing [`control::take_steer_acks`] exists for, and without it the `pending` answer would be
/// routine rather than exceptional.
#[tokio::test]
async fn an_acknowledgment_wait_never_consumes_another_requests_answer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-two-waiters");
    let acks = control::steer_acks_dir(&run_dir, 0);
    std::fs::create_dir_all(&acks).expect("mkdir");

    for id in ["req-a", "req-b"] {
        control::write_steer_ack_at(
            &acks,
            &control::SteerAck {
                kind: "steer-ack".to_string(),
                protocol_version: 1,
                request_id: id.to_string(),
                index: 0,
                state: control::SteerAckState::Delivered,
                message: "ok".to_string(),
                ts: 1_700_000_000_000,
                delivery_status: Some(control::SteerDeliveryStatus::Delivered),
            },
        )
        .await
        .expect("written");
    }

    let mine = control::take_steer_acks(&run_dir, Some("req-a")).await;
    assert_eq!(mine.len(), 1, "only my own answer comes back; got {mine:?}");
    assert_eq!(mine[0].request_id, "req-a");

    let theirs = control::take_steer_acks(&run_dir, Some("req-b")).await;
    assert_eq!(
        theirs.len(),
        1,
        "the other waiter's answer must still be on disk — an unfiltered drain would have \
         destroyed it and reported `pending` forever"
    );
    assert_eq!(theirs[0].request_id, "req-b");
}

/// The default mode is ABSENT on the wire, not written as `"steer"` (pi
/// `requestAsyncSteer`'s `...(payload.mode && payload.mode !== "steer" ? { mode } : {})`,
/// `control-channel.ts:329` @v0.43.0). A request minted by this build must stay readable by a
/// runner that predates the field.
#[tokio::test]
async fn the_default_delivery_mode_is_omitted_from_the_wire_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run_dir = dir.path().join("run-wire");
    std::fs::create_dir_all(&run_dir).expect("mkdir");

    for mode in [None, Some(control::SteerDeliveryMode::Steer)] {
        let (path, _) =
            control::request_async_steer_with_mode(&run_dir, "m", mode, None, Some("steer-action"))
                .await
                .expect("queued");
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
        assert!(
            raw.get("mode").is_none(),
            "the default mode must not appear on the wire; got {raw}"
        );
        std::fs::remove_file(&path).expect("cleanup");
    }

    let (path, _) = control::request_async_steer_with_mode(
        &run_dir,
        "m",
        Some(control::SteerDeliveryMode::Auto),
        None,
        Some("steer-action"),
    )
    .await
    .expect("queued");
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    assert_eq!(
        raw.get("mode").and_then(serde_json::Value::as_str),
        Some("auto"),
        "a non-default mode IS written, with pi's own wire spelling; got {raw}"
    );
}

/// A child with NO steer inbox must be untouched: no runtime, no subscriptions, no poller. This is
/// every foreground child, and the overwhelming majority of spawns.
#[test]
fn a_child_with_no_steer_inbox_gets_no_steering_surface() {
    assert!(
        crate::prompt_runtime::prompt_runtime_extension_from(&|_key: &str| None).is_none(),
        "a process with none of the child env vars must get no runtime at all"
    );
    assert!(
        crate::prompt_runtime::prompt_runtime_extension_from(&|key: &str| {
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
