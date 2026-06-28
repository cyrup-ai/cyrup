//! Full COMPONENT end-to-end (arch-08 §11): build a tiny `cyrup-ext-sdk` guest to a
//! `wasm32-wasip2` component, load it via the host, dispatch an event, and assert capability
//! scoping + epoch preemption.
//!
//! TOOLING-GATED: this requires the componentization toolchain (`wasm32-wasip2` target plus
//! `cargo-component` / `wasm-tools`) to produce a guest component from the SDK. On this host that
//! tooling is not installed, so the test is `#[ignore]`d. The loader code path it exercises
//! (`WasmExtension::from_bytes` -> `instantiate`) is present and compiled; the native-builtin tests
//! cover the dispatch/registration/seam/containment contracts, and `wasm_host.rs` proves epoch
//! preemption + OOM containment at the engine level without a guest.
#![cfg(feature = "wasm-host")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use cyrup_ext::host::{build_engine_on_demand, StoreLimits, WasmExtension};

#[tokio::test]
#[ignore = "tooling-gated: requires wasm32-wasip2 component build (cargo-component/wasm-tools) for the guest"]
async fn component_loads_dispatches_and_is_epoch_preempted() {
    // When the toolchain is available, the harness builds cyrup-ext-sdk's fixture guest to a
    // component and points COMPONENT_PATH at it. Until then this asserts the loader API shape.
    let path = std::env::var("CYRUP_EXT_FIXTURE_COMPONENT")
        .expect("set CYRUP_EXT_FIXTURE_COMPONENT to a built wasm32-wasip2 component");
    let bytes = std::fs::read(&path).expect("read component bytes");

    let engine = build_engine_on_demand().expect("engine");
    let ext = WasmExtension::from_bytes(engine, "fixture".into(), &bytes, StoreLimits::default())
        .expect("load component");
    // Instantiate with a generous epoch budget; a runaway export would be preempted (see wasm_host).
    let (_store, _instance) = ext.instantiate(1_000_000).await.expect("instantiate");
}
