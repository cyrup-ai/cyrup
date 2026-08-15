//! Wasmtime host isolation contracts (arch-08 §5, R-08-036 / R-ARCH-EXT-012). Gated behind the
//! `wasm-host` feature. These exercise the engine config, the epoch driver, and the fault-mapping
//! path end-to-end against bare core-wasm modules — proving a runaway/over-allocating guest is
//! PREEMPTED and SURFACED, never crashing the host, WITHOUT needing the `wasm32-wasip2` guest
//! toolchain. The full component E2E (loading a `cyrup-ext-sdk` guest) is tooling-gated.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::host::testkit;
use crate::ExtError;

#[test]
fn engine_builds_with_epoch_and_pooling() {
    testkit::engine_builds().expect("production engine builds (component-model+async+epoch+pooling)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn epoch_preemption_surfaces_timeout_host_alive() {
    let contained = testkit::epoch_preemption_demo().await.expect("host returned (alive)");
    assert!(
        matches!(contained, ExtError::EpochTimeout),
        "runaway guest preempted -> EpochTimeout, got {contained:?}"
    );
    // Host is still usable after containing the fault.
    testkit::engine_builds().expect("host healthy after preemption");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oom_denied_surfaces_out_of_memory_host_alive() {
    let contained = testkit::oom_demo().await.expect("host returned (alive)");
    assert!(
        matches!(contained, ExtError::OutOfMemory),
        "over-allocating guest denied -> OutOfMemory, got {contained:?}"
    );
    testkit::engine_builds().expect("host healthy after OOM denial");
}

// ---------------------------------------------------------------------------
// The tool `signal` binding is unwound on EVERY exit path, including a dropped future.
//
// pi passes `signal` as a PARAMETER of `ToolDefinition.execute` (`packages/coding-agent/src/core/
// extensions/types.ts:483` @v0.83.0), so it is scoped to the call by the language and a started
// `async function` always settles. cyrup cannot send a `CancelToken` through the Component Model,
// so `LiveExtension::execute_tool` binds it on `GuestState` for the duration of the call and the
// guest polls it via the `host-tool.is-cancelled` import — turning a call-scoped parameter into
// INSTANCE-scoped mutable state, which a Rust future can abandon at an await point.
// ---------------------------------------------------------------------------

use crate::host::live::ToolCallBinding;
use crate::host::GuestState;
use crate::registry::ExtensionRegistry;
use cyrup_core::{CancelToken, ExtensionId};
use serde_json::json;
use std::sync::Arc;

fn guest_state() -> Arc<GuestState> {
    Arc::new(GuestState::new(ExtensionId::from("drop-guard-probe"), Arc::new(ExtensionRegistry::new())))
}

/// PRESENCE first, so the absence assertions below cannot pass vacuously: a bound, cancelled token
/// really does make `is-cancelled` answer `true` for an arbitrary `call-id`.
#[test]
fn a_bound_cancelled_token_is_what_the_guest_poll_reads() {
    let guest = guest_state();
    assert!(!guest.tool_is_cancelled("call-1"), "nothing bound -> not cancelled");

    let cancel = CancelToken::new();
    guest.set_tool_cancel(Some(cancel.clone()));
    assert!(!guest.tool_is_cancelled("call-1"), "bound but not fired -> not cancelled");

    cancel.cancel();
    assert!(guest.tool_is_cancelled("call-1"), "bound AND fired -> the guest poll reads true");
    // Any call-id, not just the executing one — which is why a leaked binding is observable from
    // handlers that are not tool calls at all.
    assert!(guest.tool_is_cancelled("some-other-call"), "the binding is not keyed by call-id");
}

/// The guard clears the binding on the normal path.
#[test]
fn the_binding_is_cleared_when_the_guard_leaves_scope() {
    let guest = guest_state();
    let cancel = CancelToken::new();
    cancel.cancel();
    {
        guest.set_tool_cancel(Some(cancel));
        let _bound = ToolCallBinding(&guest);
        assert!(guest.tool_is_cancelled("call-1"), "bound inside the scope");
    }
    assert!(!guest.tool_is_cancelled("call-1"), "cleared when the guard leaves scope");
}

/// The regression this guard exists for: the `execute_tool` future is DROPPED at its await point
/// rather than completing or losing the `select!` race.
///
/// Before the guard, the clear lived on the two `select!` arms only, so this path left the
/// cancelled token bound forever and every later `host-tool.is-cancelled` poll — from ANY guest
/// entry point, since the import is not gated to tool calls — answered `true`. A guest that checks
/// `is-cancelled` to decide whether to keep working would silently stop working.
#[tokio::test]
async fn the_binding_is_cleared_when_the_call_future_is_dropped_mid_await() {
    use std::future::pending;

    let guest = guest_state();
    let cancel = CancelToken::new();
    cancel.cancel();

    // Proves the future really reached the guarded await before being dropped — without this the
    // assertion below passes vacuously if `call` is never polled at all.
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let call = {
        let guest = Arc::clone(&guest);
        let reached = Arc::clone(&reached);
        async move {
            guest.set_tool_cancel(Some(cancel));
            let _bound = ToolCallBinding(&guest);
            assert!(guest.tool_is_cancelled("call-1"), "bound while the call is in flight");
            reached.store(true, std::sync::atomic::Ordering::SeqCst);
            // Stands in for `api.call_execute_tool(...)`: an await that never resolves, so the only
            // way out is for the caller to drop us.
            pending::<()>().await;
        }
    };

    // A caller that races the call away — an outer `select!`, a `timeout`, an aborted task. The
    // `pending` branch never completes, so `call` is dropped part-polled.
    tokio::select! {
        biased;
        () = tokio::task::yield_now() => {}
        () = call => unreachable!("the call future never completes"),
    }

    assert!(
        reached.load(std::sync::atomic::Ordering::SeqCst),
        "the call future must have been polled past the binding, or the assertion below is vacuous"
    );
    assert!(
        !guest.tool_is_cancelled("call-1"),
        "a dropped tool call must NOT leave its CancelToken bound — `host-tool.is-cancelled` would \
         answer true for every later poll from every handler until the next execute_tool rebound it"
    );
}

// ---------------------------------------------------------------------------
// EXT-M06 — the streamed `onUpdate` chunks are call-scoped, and unwound on EVERY exit path.
//
// pi's `onUpdate` is a CLOSURE field of `ToolDefinition.execute`'s second argument
// (`packages/coding-agent/src/core/extensions/types.ts:484` @v0.83.0). A chunk therefore reaches
// that call's sink and NOTHING else, by construction — there is no shared buffer for it to sit in
// and no next call for it to surface in. cyrup cannot send a closure through the Component Model,
// so `host-tool.emit-update(call-id, chunk)` writes into an INSTANCE-scoped queue on `GuestState`
// which `LiveExtension::execute_tool` replays after the call settles. That re-introduces exactly
// the cross-call channel the language ruled out upstream, and the queue has the same three exits
// the `signal` binding has — including the one a JS port cannot think of, the future being dropped
// at its await point.
// ---------------------------------------------------------------------------

/// PRESENCE first: chunks really are queued, and the queue really is shared across `call-id`s, so
/// the absence assertions below cannot pass vacuously.
#[test]
fn emitted_update_chunks_land_in_one_instance_wide_queue() {
    let guest = guest_state();
    assert_eq!(guest.queued_tool_update_count(), 0, "nothing emitted -> nothing queued");

    guest.push_tool_update("call-1".into(), json!({"content": [], "details": "a"}));
    guest.push_tool_update("call-2".into(), json!({"content": [], "details": "b"}));
    assert_eq!(
        guest.queued_tool_update_count(),
        2,
        "both calls' chunks sit in the SAME instance-scoped queue — the fact that makes a leak \
         reachable from another call at all"
    );
}

/// The replay hands a call only its OWN chunks. Upstream this is true by construction; here it has
/// to be enforced, because the queue is instance-scoped.
#[test]
fn the_replay_takes_only_this_calls_chunks() {
    let guest = guest_state();
    guest.push_tool_update("call-1".into(), json!({"details": "mine"}));
    guest.push_tool_update("call-2".into(), json!({"details": "someone-elses"}));

    let mine = guest.take_tool_updates_for("call-1");
    assert_eq!(mine.len(), 1, "exactly this call's chunk");
    assert_eq!(mine[0].get("details").and_then(|v| v.as_str()), Some("mine"));
    assert_eq!(
        guest.queued_tool_update_count(),
        0,
        "the foreign chunk is discarded, not left to grow the queue for the life of the instance"
    );
}

/// The guard clears the queue on the normal path (the replay has already emptied it), and — the
/// regression — on the paths that never reach a replay.
#[test]
fn the_queue_is_cleared_when_the_guard_leaves_scope() {
    let guest = guest_state();
    {
        let _bound = ToolCallBinding(&guest);
        guest.push_tool_update("call-1".into(), json!({"details": "partial"}));
        assert_eq!(guest.queued_tool_update_count(), 1, "queued inside the scope");
    }
    assert_eq!(
        guest.queued_tool_update_count(),
        0,
        "a tool call that ended without replaying must not leave its chunks queued"
    );
}

/// The regression, on the exit a Rust future has and a JS `async function` does not: the
/// `execute_tool` future is DROPPED at its await point after the guest has already emitted partial
/// output.
///
/// Before the guard, those chunks stayed queued and the NEXT `execute_tool` on the same instance
/// drained them unconditionally (`take_tool_updates()`, no `call-id` filter) into its own
/// `ToolUpdateSink` — so one tool's partial output surfaced as another tool's streamed result.
#[tokio::test]
async fn a_dropped_call_does_not_leave_its_chunks_for_the_next_call() {
    use std::future::pending;

    let guest = guest_state();
    let reached = Arc::new(std::sync::atomic::AtomicBool::new(false));

    let call = {
        let guest = Arc::clone(&guest);
        let reached = Arc::clone(&reached);
        async move {
            let _bound = ToolCallBinding(&guest);
            guest.push_tool_update("abandoned-call".into(), json!({"details": "partial"}));
            assert_eq!(guest.queued_tool_update_count(), 1, "queued while the call is in flight");
            reached.store(true, std::sync::atomic::Ordering::SeqCst);
            pending::<()>().await;
        }
    };

    tokio::select! {
        biased;
        () = tokio::task::yield_now() => {}
        () = call => unreachable!("the call future never completes"),
    }

    assert!(
        reached.load(std::sync::atomic::Ordering::SeqCst),
        "the call future must have been polled past the emit, or the assertion below is vacuous"
    );
    assert_eq!(
        guest.queued_tool_update_count(),
        0,
        "a dropped tool call must NOT leave its streamed chunks queued — the next execute_tool \
         would replay another call's partial output into its own sink"
    );
    // And the next call, whatever its id, sees nothing of the abandoned one.
    assert!(
        guest.take_tool_updates_for("next-call").is_empty(),
        "the next call's replay is empty"
    );
}
