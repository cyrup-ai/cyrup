//! Per-`Store` host state (arch-08 §5.1, store_state.rs): the WASI p2 context (capability-scoped —
//! NO ambient fs/net unless granted, R-ARCH-EXT-011), the component resource table, and the
//! `ResourceLimiter`. One `HostState` lives in each per-extension `Store<HostState>`.

use crate::host::limits::StoreLimits;
use crate::host::services::GuestState;
use std::sync::Arc;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// State carried by each per-extension `Store`. The `guest` field backs the WIT capability imports
/// (registration/ui/session/models/exec/ext-fs/bus/control) for the loaded extension; it is `None`
/// for engine-level test fixtures that only exercise core-wasm modules.
pub struct HostState {
    ctx: WasiCtx,
    table: ResourceTable,
    pub limits: StoreLimits,
    pub guest: Option<Arc<GuestState>>,
}

impl HostState {
    /// Build a capability-scoped WASI context. By default there is NO ambient fs/net authority;
    /// only explicitly preopened dirs (granted via the manifest, arch-12) are visible.
    pub fn new(limits: StoreLimits) -> Self {
        let ctx = WasiCtxBuilder::new().build();
        Self { ctx, table: ResourceTable::new(), limits, guest: None }
    }

    /// Build with the guest import backing wired in (the loaded-extension path).
    pub fn with_guest(limits: StoreLimits, guest: Arc<GuestState>) -> Self {
        let ctx = WasiCtxBuilder::new().build();
        Self { ctx, table: ResourceTable::new(), limits, guest: Some(guest) }
    }

    /// Build with stdout/stderr inherited (used by test fixtures that print diagnostics).
    pub fn with_inherited_stdio(limits: StoreLimits) -> Self {
        let ctx = WasiCtxBuilder::new().inherit_stdout().inherit_stderr().build();
        Self { ctx, table: ResourceTable::new(), limits, guest: None }
    }

    /// The guest import backing, or an error if this store was not built for a loaded extension.
    pub fn guest(&self) -> Result<&Arc<GuestState>, crate::error::ExtError> {
        self.guest
            .as_ref()
            .ok_or_else(|| crate::error::ExtError::Component("store has no guest state".into()))
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.ctx, table: &mut self.table }
    }
}
