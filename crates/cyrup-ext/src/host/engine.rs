//! The shared Wasmtime `Engine` (arch-08 §5, engine.rs). Configured with the Component Model,
//! async support (so a guest awaiting a host capability yields the host thread), epoch interruption
//! (preemption, R-ARCH-EXT-012), and the pooling allocator (linear-memory slab reuse across tool
//! calls, §10). Also the containment mapper that turns any Wasmtime fault into a typed `ExtError`.

use crate::error::ExtError;
use crate::host::limits::OOM_SENTINEL;
use wasmtime::{Config, Engine, InstanceAllocationStrategy, PoolingAllocationConfig};

/// Build the shared engine (arch-08 §5). The host binary does not need the wasm toolchain to build
/// this — only the Tier-1 build loop needs `wasm32-wasip2` (arch-00 Appendix B).
pub fn build_engine() -> Result<Engine, ExtError> {
    let mut config = Config::new();
    // Async support is always enabled when wasmtime's `async` feature is on (wasmtime 46:
    // `Config::async_support` is a deprecated no-op); `instantiate_async`/`call_async` work directly.
    config.epoch_interruption(true);
    config.wasm_component_model(true);

    // Pooling allocator: reuse linear-memory slabs across instantiations (§10). Conservative caps.
    let mut pool = PoolingAllocationConfig::default();
    pool.total_memories(100);
    pool.total_tables(100);
    pool.total_core_instances(100);
    config.allocation_strategy(InstanceAllocationStrategy::Pooling(pool));

    Engine::new(&config).map_err(|e| ExtError::Engine(e.to_string()))
}

/// Build an engine WITHOUT the pooling allocator (the on-demand allocator). Used where the pooling
/// allocator's fixed reservations are undesirable (e.g. constrained test environments).
pub fn build_engine_on_demand() -> Result<Engine, ExtError> {
    let mut config = Config::new();
    // Async support is always enabled when wasmtime's `async` feature is on (wasmtime 46).
    config.epoch_interruption(true);
    config.wasm_component_model(true);
    Engine::new(&config).map_err(|e| ExtError::Engine(e.to_string()))
}

/// Map any Wasmtime fault into a typed, surfaced `ExtError` (R-08-036). Epoch preemption becomes
/// `EpochTimeout`; a `ResourceLimiter` denial (carrying [`OOM_SENTINEL`]) becomes `OutOfMemory`;
/// everything else is a contained `Trap`. The host NEVER crashes.
pub fn map_wasm_error(err: &wasmtime::Error) -> ExtError {
    let text = format!("{err:?}");
    if text.contains(OOM_SENTINEL) {
        return ExtError::OutOfMemory;
    }
    if let Some(trap) = err.downcast_ref::<wasmtime::Trap>() {
        return match trap {
            wasmtime::Trap::Interrupt => ExtError::EpochTimeout,
            other => ExtError::Trap(format!("{other:?}")),
        };
    }
    ExtError::Trap(text)
}
