//! Wasmtime host isolation contracts (arch-08 §5, R-08-036 / R-ARCH-EXT-012). Gated behind the
//! `wasm-host` feature. These exercise the engine config, the epoch driver, and the fault-mapping
//! path end-to-end against bare core-wasm modules — proving a runaway/over-allocating guest is
//! PREEMPTED and SURFACED, never crashing the host, WITHOUT needing the `wasm32-wasip2` guest
//! toolchain. The full component E2E (loading a `cyrup-ext-sdk` guest) is tooling-gated.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

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

use crate::host::live::ToolCancelBinding;
use crate::host::GuestState;
use crate::registry::ExtensionRegistry;
use cyrup_core::{CancelToken, ExtensionId};
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
        let _bound = ToolCancelBinding(&guest);
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
            let _bound = ToolCancelBinding(&guest);
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
