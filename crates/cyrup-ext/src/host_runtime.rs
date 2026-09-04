//! The Wasmtime runtime bundle owned by [`crate::facade::ExtensionHost`] when the `wasm-host`
//! feature is on (arch-08 §3.1): the shared engine and the background epoch driver. Feature-gated
//! so the native foundation builds without Wasmtime.
//!
//! The per-extension instance pool arch-08 §3.1 also lists is NOT a separate structure here: it is
//! `ExtensionHost::live` (the id -> `Arc<LiveExtension>` map) plus each
//! [`crate::host::LiveExtension`]'s own `Mutex<LiveInner>`, which together give exactly
//! R-ARCH-EXT-013's single-thread-per-`Store` confinement with host parallelism ACROSS extensions.
//! A second, standalone `InstancePool`/`WasmExtension` pair used to sit here duplicating that; it
//! was deleted after a workspace-wide grep found it had never had a single caller (EXT-M08).

use crate::error::ExtError;
use crate::host::epoch::DEFAULT_TICK;
use crate::host::{EpochDriver, build_engine};
use cyrup_core::RunCancel;
use wasmtime::Engine;

/// Engine + epoch driver (arch-08 §3.1); the instance pool is the host's live map (see module doc).
pub struct WasmRuntime {
    engine: Engine,
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
        Ok(Self {
            engine,
            cancel,
            _epoch: epoch,
        })
    }

    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Cancel-and-preempt: cancels the run token (which the epoch driver also watches) and bumps the
    /// epoch immediately so any running guest is preempted (arch-08 §5.3).
    ///
    /// **EXT-M09 — this has NO caller anywhere in the workspace**, so the "bridge" [`crate::host::epoch`]'s
    /// module doc describes ("On `RunCancel.cancel()` the bridge calls `engine.increment_epoch()`
    /// immediately to preempt any running guest") does not run in production. Cancellation reaches a
    /// running guest only through the `cancel.cancelled()` arm of the `tokio::select!`s in
    /// [`crate::host::LiveExtension`], which DROPS the in-flight call future rather than trapping the
    /// guest — a different mechanism with a different aftermath for the instance. Deliberately kept
    /// rather than deleted with the dead instance pool (EXT-M08): the pool was a superseded
    /// duplicate, this is a specified mechanism that was never wired. Note the cancel is one-way
    /// (`RunCancel` has no re-arm) and would also stop the epoch ticker, so wiring it needs a
    /// decision, not just a call site.
    pub fn preempt_all(&self) {
        self.cancel.cancel();
        self.engine.increment_epoch();
    }
}
