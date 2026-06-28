//! Per-`Store` host state (arch-08 §5.1, store_state.rs): the WASI p2 context (capability-scoped —
//! NO ambient fs/net unless granted, R-ARCH-EXT-011), the component resource table, and the
//! `ResourceLimiter`. One `HostState` lives in each per-extension `Store<HostState>`.

use crate::host::limits::StoreLimits;
use wasmtime_wasi::{ResourceTable, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

/// State carried by each per-extension `Store`.
pub struct HostState {
    ctx: WasiCtx,
    table: ResourceTable,
    pub limits: StoreLimits,
}

impl HostState {
    /// Build a capability-scoped WASI context. By default there is NO ambient fs/net authority;
    /// only explicitly preopened dirs (granted via the manifest, arch-12) are visible.
    pub fn new(limits: StoreLimits) -> Self {
        let ctx = WasiCtxBuilder::new().build();
        Self { ctx, table: ResourceTable::new(), limits }
    }

    /// Build with stdout/stderr inherited (used by test fixtures that print diagnostics).
    pub fn with_inherited_stdio(limits: StoreLimits) -> Self {
        let ctx = WasiCtxBuilder::new().inherit_stdout().inherit_stderr().build();
        Self { ctx, table: ResourceTable::new(), limits }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.ctx, table: &mut self.table }
    }
}
