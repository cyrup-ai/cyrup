//! cyrup-ext — the WASM extension host (arch-08; conformance: func-08; binds ADR-0002).
//!
//! A Wasmtime Component Model host + WIT world, capability scoping + epoch/fuel preemption +
//! memory limits, subscription-gated event dispatch, a native built-in extension registry, the
//! extension manifest, and the Tier-1 build/artifact-cache loop. Bridges to the agent via the two
//! seams: `cyrup_agent::Hooks` (mutating) and `cyrup_agent::EventSubscriber` (notify).
//!
//! ## Layering
//! - The **native foundation** (dispatch, registry, the two seams, manifest, build cache,
//!   containment) builds and is fully tested WITHOUT any wasm — native built-in extensions exercise
//!   every dispatch/registration/seam/containment contract (arch-08 §11).
//! - The **Wasmtime host** (`host` / `host_runtime`) is behind the `wasm-host` cargo feature
//!   (arch-08 §2): a shared `Engine` (component-model + async + epoch interruption + pooling
//!   allocator), a `ResourceLimiter` memory cap, a per-extension `Store` instance pool, and the
//!   epoch driver. A guest fault (trap / OOM / epoch timeout) is caught and surfaced — the host
//!   NEVER crashes (R-00-009 / R-08-036).
//!
//! No-panic policy (arch-00 §8) is enforced crate-wide via `[workspace.lints]`; tests may
//! `#[allow(...)]` where unwrap/expect is acceptable.
#![forbid(unsafe_code)]

pub mod build;
pub mod contract;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod extension;
pub mod facade;
pub mod hooks;
pub mod manifest;
pub mod native;
pub mod registry;
pub mod subscriber;

#[cfg(feature = "wasm-host")]
pub mod host;
#[cfg(feature = "wasm-host")]
pub mod host_runtime;

// --- Re-exports: the load-bearing surface (arch-08 §3). ---
pub use contract::{EventPatch, HandledValue, HookOutcome, Reduced};
pub use dispatch::Dispatcher;
pub use error::ExtError;
pub use event::{EventKind, HostEvent, Subscriptions};
pub use extension::{ExtKind, Extension};
pub use facade::{ExtensionHost, HostConfig};
pub use hooks::ExtHooks;
pub use manifest::{Capabilities, ExtensionManifest, HOST_WORLD};
pub use native::{CtxTier, ExtMode, HostCtx, InitApi, NativeExtension, NativeHandle};
pub use registry::{
    CommandDescriptor, ExecModeWire, ExtensionRegistry, ToolDescriptor,
};
pub use subscriber::ExtSubscriber;

#[cfg(feature = "wasm-host")]
pub use host::{EpochDriver, InstancePool, StoreLimits, WasmExtension};
#[cfg(feature = "wasm-host")]
pub use host_runtime::WasmRuntime;
