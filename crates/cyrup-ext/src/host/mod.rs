//! The Wasmtime Component Model host (arch-08 §5). Feature-gated behind `wasm-host` so the
//! native-builtin dispatch foundation builds without pulling Wasmtime. Provides: an `Engine`
//! configured with component-model + async + epoch interruption + the pooling allocator
//! (R-ARCH-EXT-011/012); a `ResourceLimiter` memory cap; a background epoch driver that preempts a
//! runaway guest; and a per-extension `Store` instance pool (single-thread-confined, host
//! parallelizes by instances, R-ARCH-EXT-013). Every guest call is fault-contained: a trap, epoch
//! timeout, or OOM is mapped to a typed `ExtError` and surfaced — the host never crashes
//! (R-08-036).

pub mod engine;
pub mod epoch;
pub mod instance;
pub mod limits;
pub mod live;
pub mod services;
pub mod store_state;
pub mod testkit;

pub use engine::{build_engine, build_engine_on_demand, map_wasm_error};
pub use epoch::EpochDriver;
pub use instance::{InstancePool, WasmExtension};
pub use limits::StoreLimits;
pub use live::{LiveExtension, WasmTool};
pub use services::{
    CannedResponses, ControlOp, DenyServices, DialogOptions, ExecOutput, FsCaps, GuestState,
    HostServices, HttpRequest, HttpResponse, NotifyKind, OAuthEvent, RecordingServices, UiChrome,
};
pub use store_state::HostState;
