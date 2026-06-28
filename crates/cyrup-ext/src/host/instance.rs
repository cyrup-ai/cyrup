//! Per-extension `Store` instances and the instance pool (arch-08 §5.1, R-ARCH-EXT-013). A
//! Wasmtime `Store` is driven by one host task at a time; concurrency comes from MANY instances
//! across the tokio pool. Each extension gets a slot behind an async `Mutex` (single-thread-per-Store
//! invariant); different extensions run in parallel. Tool/event calls arm the epoch deadline from
//! the `CancelToken` and race against cancellation; faults are mapped to `ExtError` (R-08-036).

use crate::error::ExtError;
use crate::host::engine::map_wasm_error;
use crate::host::limits::StoreLimits;
use crate::host::store_state::HostState;
use cyrup_core::{CancelToken, ExtensionId};
use dashmap::DashMap;
use std::sync::Arc;
use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

/// A loaded WASM extension: a precompiled component plus the engine/limits to instantiate it. A
/// fresh `Store` (and fresh linear memory, via the pooling allocator) is preferred per call.
pub struct WasmExtension {
    id: ExtensionId,
    engine: Engine,
    component: Component,
    limits: StoreLimits,
}

impl WasmExtension {
    /// Load a component from its (precompiled or binary) bytes (arch-08 §6.4). The bytes come from
    /// the artifact cache on a hit, or a fresh Tier-1 build on a miss.
    pub fn from_bytes(
        engine: Engine,
        id: ExtensionId,
        bytes: &[u8],
        limits: StoreLimits,
    ) -> Result<Self, ExtError> {
        let component = Component::from_binary(&engine, bytes)
            .map_err(|e| ExtError::Component(e.to_string()))?;
        Ok(Self { id, engine, component, limits })
    }

    pub fn id(&self) -> &ExtensionId {
        &self.id
    }

    /// Build a per-call `Store` with the `ResourceLimiter` wired and an epoch deadline armed. The
    /// engine's epoch driver preempts the guest if it exceeds `epoch_deadline_ticks`.
    pub fn new_store(&self, epoch_deadline_ticks: u64) -> Store<HostState> {
        let mut store = Store::new(&self.engine, HostState::new(self.limits.clone()));
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(epoch_deadline_ticks);
        store
    }

    /// A linker with WASI p2 capability-scoped imports added (no ambient authority unless granted,
    /// R-ARCH-EXT-011). Host capability imports (ui/session/models/exec/bus/control) are added on
    /// top of this in the gated component-call path.
    pub fn linker(&self) -> Result<Linker<HostState>, ExtError> {
        let mut linker = Linker::<HostState>::new(&self.engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)
            .map_err(|e| ExtError::Engine(e.to_string()))?;
        Ok(linker)
    }

    /// Instantiate the component into a fresh store (gated component-call path). Returns the store +
    /// instance; typed export calls (`init`, the §5 event exports) are driven by the bindgen glue.
    pub async fn instantiate(
        &self,
        epoch_deadline_ticks: u64,
    ) -> Result<(Store<HostState>, wasmtime::component::Instance), ExtError> {
        let linker = self.linker()?;
        let mut store = self.new_store(epoch_deadline_ticks);
        let instance = linker
            .instantiate_async(&mut store, &self.component)
            .await
            .map_err(|e| map_wasm_error(&e))?;
        Ok((store, instance))
    }
}

/// The per-extension instance pool (arch-08 §5.1). Each extension's `WasmExtension` lives behind an
/// async `Mutex` so its `Store` is touched by one task at a time; N extensions run concurrently.
#[derive(Default)]
pub struct InstancePool {
    slots: DashMap<ExtensionId, Arc<tokio::sync::Mutex<WasmExtension>>>,
}

impl InstancePool {
    pub fn new() -> Self {
        Self { slots: DashMap::new() }
    }

    /// Register a loaded extension's component in the pool.
    pub fn insert(&self, ext: WasmExtension) {
        let id = ext.id().clone();
        self.slots.insert(id, Arc::new(tokio::sync::Mutex::new(ext)));
    }

    pub fn contains(&self, id: &ExtensionId) -> bool {
        self.slots.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    fn slot(
        &self,
        id: &ExtensionId,
    ) -> Result<Arc<tokio::sync::Mutex<WasmExtension>>, ExtError> {
        self.slots
            .get(id)
            .map(|s| s.value().clone())
            .ok_or_else(|| ExtError::Component(format!("no instance for extension {id}")))
    }

    /// Run `f` against the extension's instance, holding the single-thread-per-Store guard and
    /// racing against `cancel` (arch-08 §5.1). A cancelled call surfaces `ExtError::Cancelled`.
    pub async fn with_instance<R, F>(
        &self,
        id: &ExtensionId,
        cancel: &CancelToken,
        f: F,
    ) -> Result<R, ExtError>
    where
        F: AsyncFnOnce(&mut WasmExtension) -> Result<R, ExtError>,
    {
        let slot = self.slot(id)?;
        let mut guard = slot.lock().await;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(ExtError::Cancelled),
            r = f(&mut guard) => r,
        }
    }
}
