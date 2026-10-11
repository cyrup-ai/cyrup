//! The shared Wasmtime `Engine` (arch-08 §5, engine.rs). Configured with the Component Model,
//! async support (so a guest awaiting a host capability yields the host thread), epoch interruption
//! (preemption, R-ARCH-EXT-012), and the pooling allocator (linear-memory slab reuse across tool
//! calls, §10). Also the containment mapper that turns any Wasmtime fault into a typed `ExtError`.

use crate::error::ExtError;
use crate::host::limits::OOM_SENTINEL;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// The engine the host runs on: the pooling engine, or the on-demand one when the pool cannot be
/// built.
///
/// [CYRUP-DELTA] The pooling allocator reserves its slabs of address space up front (`total_memories
/// (100)` and its siblings above), and a process whose address space is limited (`ulimit -v`, a
/// container's `--ulimit as`, a CI job wrapper) cannot map them: `failed to create memory pool
/// mapping`, at every limit measured below 512 GiB (2, 4, 8, 16, 32, 64, 128, 256 and 384 GiB
/// failed; 512 GiB, 768 GiB and 1 TiB did not). With no fallback that error ended session start
/// for every extension, the native ones (`codemode`, `mcp`, `flux`) that never touch wasm among
/// them, and `--no-extensions` did not help. The on-demand allocator maps memory as a guest asks
/// for it, so the host starts and a wasm guest that needs more than the limit allows fails to load
/// by itself, as a contained error. pi has no wasm runtime to size; its extensions are TypeScript.
///
/// The fallback is logged once per process (the host builds an engine for the pre-trust pass and
/// again for the session).
pub fn build_engine_or_on_demand() -> Result<Engine, ExtError> {
    build_engine_with_fallback(build_engine, build_engine_on_demand)
}

/// [`build_engine_or_on_demand`] with the two builders given, so a test can make the first fail.
pub(crate) fn build_engine_with_fallback(
    pooling: impl FnOnce() -> Result<Engine, ExtError>,
    on_demand: impl FnOnce() -> Result<Engine, ExtError>,
) -> Result<Engine, ExtError> {
    static WARNED: AtomicBool = AtomicBool::new(false);
    match pooling() {
        Ok(engine) => Ok(engine),
        Err(pooling_error) => {
            if !WARNED.swap(true, Ordering::Relaxed) {
                tracing::warn!(
                    error = %pooling_error,
                    "wasm pooling allocator unavailable (is the address space limited, e.g. by \
                     `ulimit -v`?); using the on-demand allocator"
                );
            }
            on_demand().map_err(|on_demand_error| {
                ExtError::Engine(format!(
                    "{pooling_error}; the on-demand engine failed too: {on_demand_error}"
                ))
            })
        }
    }
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
