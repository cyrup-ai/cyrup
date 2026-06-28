//! Wasmtime host isolation contracts (arch-08 §5, R-08-036 / R-ARCH-EXT-012). Gated behind the
//! `wasm-host` feature. These exercise the engine config, the epoch driver, and the fault-mapping
//! path end-to-end against bare core-wasm modules — proving a runaway/over-allocating guest is
//! PREEMPTED and SURFACED, never crashing the host, WITHOUT needing the `wasm32-wasip2` guest
//! toolchain. The full component E2E (loading a `cyrup-ext-sdk` guest) is tooling-gated.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use cyrup_ext::host::testkit;
use cyrup_ext::ExtError;

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
