//! Self-contained demonstrations of epoch preemption + `ResourceLimiter` containment at the
//! Wasmtime engine level (arch-08 §5.3, R-08-036 / R-ARCH-EXT-012). These use bare core-wasm
//! modules (no guest component / no `wasm32-wasip2` toolchain needed) so the engine config,
//! the epoch driver, and the fault-mapping path are exercised end-to-end. The full COMPONENT
//! E2E (loading a `cyrup-ext-sdk` guest) is tooling-gated — see `tests/wasm_component.rs`.

use crate::error::ExtError;
use crate::host::engine::{build_engine_on_demand, map_wasm_error};
use crate::host::epoch::EpochDriver;
use crate::host::limits::StoreLimits;
use cyrup_core::RunCancel;
use std::time::Duration;
use wasmtime::{Instance, Module, Store};

/// Instantiate a core module with an infinite loop, arm a 1-tick epoch deadline, and drive the
/// epoch. The guest is preempted and the trap is mapped — returns the CONTAINED `ExtError`
/// (expected: `EpochTimeout`). The host stays alive (this function returns normally).
pub async fn epoch_preemption_demo() -> Result<ExtError, ExtError> {
    let engine = build_engine_on_demand()?;
    let module = Module::new(&engine, r#"(module (func (export "run") (loop br 0)))"#)
        .map_err(|e| ExtError::Component(e.to_string()))?;

    let cancel = RunCancel::new();
    let _driver = EpochDriver::spawn(engine.clone(), Duration::from_millis(1), cancel.clone());

    let mut store = Store::new(&engine, StoreLimits::default());
    store.limiter(|s| s);
    store.set_epoch_deadline(1);

    let instance = Instance::new_async(&mut store, &module, &[])
        .await
        .map_err(|e| map_wasm_error(&e))?;
    let run = instance
        .get_typed_func::<(), ()>(&mut store, "run")
        .map_err(|e| ExtError::Component(e.to_string()))?;

    match run.call_async(&mut store, ()).await {
        Ok(()) => Err(ExtError::Trap(
            "guest unexpectedly completed (no preemption)".into(),
        )),
        Err(e) => Ok(map_wasm_error(&e)),
    }
}

/// Instantiate a core module that grows memory far past the per-instance cap. The
/// `ResourceLimiter` denies the growth; the fault is mapped — returns the CONTAINED `ExtError`
/// (expected: `OutOfMemory`). The host stays alive.
pub async fn oom_demo() -> Result<ExtError, ExtError> {
    let engine = build_engine_on_demand()?;
    let module = Module::new(
        &engine,
        r#"(module
             (memory (export "mem") 1)
             (func (export "grow") (result i32) (memory.grow (i32.const 4000))))"#,
    )
    .map_err(|e| ExtError::Component(e.to_string()))?;

    // 2 MiB cap; growing by 4000 pages (~256 MiB) is denied.
    let mut store = Store::new(
        &engine,
        StoreLimits::default().with_max_memory(2 * 1024 * 1024),
    );
    store.limiter(|s| s);
    // Epoch interruption is enabled on the engine; with no driver and no deadline the default
    // deadline (0) would trap immediately. Set a far deadline so ONLY the limiter denies here.
    store.set_epoch_deadline(u64::MAX);

    let instance = Instance::new_async(&mut store, &module, &[])
        .await
        .map_err(|e| map_wasm_error(&e))?;
    let grow = instance
        .get_typed_func::<(), i32>(&mut store, "grow")
        .map_err(|e| ExtError::Component(e.to_string()))?;

    match grow.call_async(&mut store, ()).await {
        Ok(pages) => Err(ExtError::Trap(format!(
            "grow unexpectedly succeeded: {pages}"
        ))),
        Err(e) => Ok(map_wasm_error(&e)),
    }
}

/// Build the production engine (pooling allocator + epoch + component model + async) and confirm it
/// constructs successfully.
pub fn engine_builds() -> Result<(), ExtError> {
    crate::host::engine::build_engine().map(|_| ())
}
