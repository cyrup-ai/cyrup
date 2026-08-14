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
//! ## CYRUP-DELTA register — what `interface ctx-state` deliberately does NOT mirror
//!
//! `world.wit`'s `ctx-state` header claims to mirror pi's base `ExtensionContext`
//! (`pi/packages/coding-agent/src/core/extensions/types.ts:305-347` @v0.83.0). It very nearly does,
//! and EXT-044/EXT-045 closed `cwd` (:315), `scopedModels` (:326) and the poll form of `signal`
//! (:334). Two members remain deliberately unported. They are recorded HERE, in the source, because
//! EXT-005 was closed on the promise that they would be and they were not — which is precisely how
//! a gap becomes invisible to the next reader (README structural blind spot 1).
//!
//! * **`signal: AbortSignal | undefined`** (`types.ts:334`, "The current abort signal, or undefined
//!   when the agent is not streaming") is ported as a POLL, not as the object.
//!   `ctx-state.is-run-cancelled` answers `signal?.aborted`; nothing answers
//!   `signal.addEventListener`. **Reason:** an `AbortSignal` is an event target, and a Component
//!   Model value cannot be a callback target — a guest cannot be handed a host object it can
//!   subscribe to, and the host cannot re-enter a suspended guest's single-instance store to wake
//!   it. A guest therefore checks between units of work instead of being interrupted. A guest TOOL
//!   is unaffected: `host-tool.is-cancelled` is the exact analog of upstream's `execute(…, signal,
//!   …)` parameter.
//! * **`sessionManager: ReadonlySessionManager` / `modelRegistry: ModelRegistry`**
//!   (`types.ts:317,319`) are live object handles, not values. cyrup exposes the DATA they carry as
//!   the `session` / `models` import interfaces instead (arch-08 §5.6); there is no verb-for-verb
//!   mirror of either object and there will not be one, because ADR-0002 makes extension I/O values
//!   rather than references.
//!
//! Anything else `types.ts:305-347` declares should either be reachable from `ctx-state` /
//! `session` / `models` / `control`, or filed. If you find a third omission, it is a gap, not a
//! delta — file it rather than adding it here.
//!
//! No-panic policy (arch-00 §8) is enforced crate-wide via `[workspace.lints]`; tests may
//! `#[allow(...)]` where unwrap/expect is acceptable.
#![forbid(unsafe_code)]

pub mod aggregate;
pub mod build;
pub mod bus;
pub mod contract;
pub mod dispatch;
pub mod error;
pub mod event;
pub mod extension;
pub mod facade;
pub mod hooks;
pub mod loader;
pub mod manifest;
pub mod native;
pub mod provider;
pub mod registry;
pub mod subscriber;
pub mod wrapper;

#[cfg(test)]
mod tests;

#[cfg(feature = "wasm-host")]
pub mod caps;
#[cfg(feature = "wasm-host")]
pub mod host;
#[cfg(feature = "wasm-host")]
pub mod host_runtime;

// --- Re-exports: the load-bearing surface (arch-08 §3). ---
pub use aggregate::{
    fold_project_trust, fold_resources, AttributedPath, ProjectTrustDecision, ResourcesAggregate,
};
// The inter-extension bus is NOT `wasm-host`-gated (EXT-018): pi hangs `events` on the one base
// `ExtensionAPI` every extension receives (extensions/loader.ts:389 @v0.83.0), so which tier an
// extension runs in cannot decide whether it has a coordination channel.
pub use bus::SharedBus;
pub use contract::{EventPatch, HandledValue, HookOutcome, Reduced};
pub use dispatch::{Dispatcher, ErrorListener, ExtensionError};
pub use error::ExtError;
pub use event::{EventKind, HostEvent, InputEventSource, InputStreamingBehavior, Subscriptions};
pub use extension::{ExtKind, Extension};
pub use facade::{
    BeforeAgentStartReduction, CompactionReduction, ExtensionFlagOverride, ExtensionHost,
    HostConfig, InputReduction, RenderOutcome, TreeReduction, UserBashReduction,
};
pub use build::build_component;
pub use hooks::ExtHooks;
pub use loader::{
    discover, discover_with_diagnostics, resolve_component_bytes, DiscoveredExtension,
    DiscoveryRoots, ExtOrigin, LoadError, LoadExtensionsResult,
};
pub use manifest::{Capabilities, ExtensionManifest, FsGrant, HOST_WORLD, MANIFEST_FILE};
pub use native::{
    CtxTier, ExtMode, HostCtx, HostCtxRich, HumanWaitGate, HumanWaitGuard, InitApi,
    NativeExtension, NativeHandle,
};
pub use provider::{
    resolve_api_key, ModelCost, ModelCostTier, ModelRegistrySink, ProviderConfig, ProviderHub,
    ProviderModelConfig, ProviderRegistration,
};
pub use registry::{
    CommandDescriptor, ExecModeWire, ExtensionConflict, ExtensionRegistry, ResolvedCommand,
    ToolDescriptor,
};
pub use subscriber::ExtSubscriber;
pub use wrapper::{wrap_registered_tool, ActiveToolNames, RegisteredTool};

#[cfg(feature = "wasm-host")]
pub use host::{
    CannedResponses, ControlOp, DenyServices, DialogOptions, EpochDriver, ExecOutput, FsCaps,
    DENIED_EXEC, DENIED_NET, DENIED_UI,
    GuestState, HostServices, HttpRequest, HttpResponse, HttpStreamResponse, HumanInteractionGuard,
    HumanInteractionLock, InstancePool, InteractiveOverlay, LiveExtension, NotifyKind, OAuthEvent,
    OverlayColor, OverlayKey, OverlayKeyCode, OverlayLine, OverlayOutcome, OverlaySpan,
    ProcSpawnSpec, RecordingServices, StoreLimits, UiChrome, WasmExtension, WasmTool,
};
#[cfg(feature = "wasm-host")]
pub use host_runtime::WasmRuntime;
