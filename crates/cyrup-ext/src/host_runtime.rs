//! The Wasmtime runtime bundle owned by [`crate::facade::ExtensionHost`] when the `wasm-host`
//! feature is on (arch-08 §3.1): the shared engine, the per-extension instance pool, and the
//! background epoch driver. Feature-gated so the native foundation builds without Wasmtime.

use crate::error::ExtError;
use crate::host::{build_engine, EpochDriver, InstancePool};
use crate::host::epoch::DEFAULT_TICK;
use cyrup_core::RunCancel;
use std::sync::Arc;
use wasmtime::Engine;

/// Engine + instance pool + epoch driver (arch-08 §3.1).
pub struct WasmRuntime {
    engine: Engine,
    pool: Arc<InstancePool>,
    cancel: RunCancel,
    _epoch: EpochDriver,
}

impl WasmRuntime {
    /// Build the engine and spawn the epoch driver. Must be called from within a tokio runtime
    /// (the epoch driver is a background task).
    pub fn new() -> Result<Self, ExtError> {
        let engine = build_engine()?;
        let cancel = RunCancel::new();
        let epoch = EpochDriver::spawn(engine.clone(), DEFAULT_TICK, cancel.clone());
        Ok(Self { engine, pool: Arc::new(InstancePool::new()), cancel, _epoch: epoch })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    pub fn pool(&self) -> &Arc<InstancePool> {
        &self.pool
    }

    /// Cancel-and-preempt: cancels the run token (which the epoch driver also watches) and bumps the
    /// epoch immediately so any running guest is preempted (arch-08 §5.3).
    pub fn preempt_all(&self) {
        self.cancel.cancel();
        self.engine.increment_epoch();
    }
}
