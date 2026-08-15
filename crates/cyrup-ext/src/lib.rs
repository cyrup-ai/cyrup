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
//! ## CYRUP-DELTA register — what `interface ui` deliberately does NOT mirror
//!
//! `interface ui` mirrors pi's `ExtensionUI` (`types.ts:130-290` @v0.83.0). EXT-021 closed the
//! import-shaped half (`set-working-message`, `set-working-visible`, `set-working-indicator`,
//! `set-hidden-thinking-label`, `theme-get-by-name`, and `theme-list`'s widening to `{name, path}`
//! rows), and its residual `onTerminalInput` (`types.ts:145`) is now closed too — see the second
//! bullet. Exactly **ONE** member remains deliberately unported.
//!
//! * **`setEditorComponent(factory: EditorFactory | undefined)` / `getEditorComponent()`**
//!   (`types.ts:260`, `:263`) are deliberately unported. `EditorFactory` is
//!   `(tui: TUI, theme: EditorTheme, keybindings: KeybindingsManager) => EditorComponent`
//!   (`:125`): the extension hands back a live component the host then drives through a
//!   draw/handle-input/dispose protocol, holding a reference for the editor's whole lifetime.
//!   **Reason:** the same one that keeps `sessionManager` out of `ctx-state` — ADR-0002 makes
//!   extension I/O values rather than references, and the Component Model has no way to hand a
//!   guest three live host objects and take back an object the host can re-enter on every keystroke
//!   (each re-entry would re-enter the guest's single-instance store from inside the host's own draw
//!   pass). A guest that wants editor behaviour uses the value-shaped surface — `ui.get-editor-text`
//!   / `set-editor-text` / `paste-editor-text` — which cyrup already exports. `getEditorComponent`
//!   is the reader of the same handle and goes with it.
//! * **`onTerminalInput(handler: TerminalInputHandler): () => void`** (`types.ts:145`, handler type
//!   `:113`) was **NOT a delta — it was an open gap** (EXT-021's residual), and it is now
//!   **CLOSED**, exactly as this bullet prescribed: a guest EXPORT (`on-terminal-input`) plus the
//!   `ui.subscribe-terminal-input` / `ui.unsubscribe-terminal-input` import pair, with the fold
//!   ([`ExtensionHost::terminal_input`]) a clause-for-clause port of pi's `TUI.handleInput`
//!   listener loop (`packages/tui/src/tui.ts:773-788`). The `HOST_WORLD` bump it was waiting on is
//!   0.6 → 0.7. The bullet is kept, struck through in prose rather than deleted, because the
//!   register's job is to be a record of what was decided and why — a silently vanished bullet
//!   reads as "there was never a gap here".
//!
//! ## CYRUP-DELTA register — `UserBashEventResult.operations` has no guest-supplied form yet
//!
//! * **`operations?: BashOperations`** on `UserBashEventResult`
//!   (`pi/packages/coding-agent/src/core/extensions/types.ts:1078-1080` @v0.83.0, the interface
//!   itself at `core/tools/bash.ts:52-73`) is the ONE member of the `user_bash` result a WASM guest
//!   cannot supply. pi's `rpc-mode.ts:566-579` short-circuits on `result` and otherwise threads
//!   `operations` into `executeBash` (`agent-session.ts:2782`:
//!   `options?.operations ?? createLocalBashOperations({ shellPath })`), so an `ssh` / sandbox /
//!   VM extension redirects that one command's execution without re-implementing the bash seam
//!   All three shipped examples do exactly that, and each does it from a `pi.on("user_bash", …)`
//!   handler returning `{ operations }` and nothing else: `examples/extensions/ssh.ts:203-206`
//!   (factory at `:81`), `examples/extensions/sandbox/index.ts:229-231` (factory at `:132`),
//!   `examples/extensions/gondolin/index.ts:517-520` (factory at `:324`). Note what that means for
//!   the `result` half cyrup already honours: those three extensions never set it, so today an
//!   `ssh`-style extension's `!` command runs on the LOCAL shell — the redirection silently does
//!   not happen, which is the failure mode ADR-0002 rejected-alternative D names.
//!
//!   **Reason:** `BashOperations` is an object with an `exec` METHOD, and ADR-0002
//!   (`docs/adr/ADR-0002-extension-io-is-serde.md`) makes extension I/O values rather than
//!   references. The rule-4 shape is already designed and is the `register-message-renderer` +
//!   `render-call` pattern applied one more time: a `registration.register-bash-operations()`
//!   import declaring that this guest HAS one (argument-less, exactly like
//!   `register-markdown-transformer`, because upstream keeps at most one per handler result), a
//!   keyed `bash-operations-exec(call-id, command, cwd, env-json)` EXPORT, and — because pi's
//!   `exec` streams through `onData` and observes an `AbortSignal` while it runs — a matching
//!   `emit-bash-output(call-id, chunk)` import plus an `is-bash-cancelled(call-id)` poll (rule 6,
//!   the same substitution `ctx-state.is-run-cancelled` already makes). It is an EXPORT, so it
//!   costs a `HOST_WORLD` MINOR bump (rule 9), and it needs the guest half in
//!   `crates/cyrup-ext-sdk/src/{api,guest,macros}.rs` in the same change or every Tier-1 guest
//!   fails to build.
//!
//!   **What is NOT missing.** The reduction carries the field: a guest's `handled` payload reaches
//!   [`UserBashReduction::Handled`] verbatim (`decode_outcome` parses the whole JSON; the per-kind
//!   `decode_patch` shaping applies to `mutate` only), pinned by
//!   `tests::payload_and_seam_parity::user_bash_reduction_carries_the_operations_half_not_only_the_result_half`.
//!   The host-side seam an override would be expressed AS also exists now:
//!   [`cyrup_tools::ops::BashOperations`] with [`cyrup_tools::ops::LocalBashOperations`], the port
//!   of `createLocalBashOperations`. **The consumption half is now built too**: `BashOptions` has an
//!   `operations` field, `AgentSession::execute_bash_with_user_event` forwards it and
//!   `execute_bash` resolves pi's `options?.operations ?? createLocalBashOperations({ shellPath })`
//!   (`agent-session.ts:2782`), pinned by the three `..._operations_override_...` tests in
//!   `cyrup-session-svc/src/tests/round9_l5res.rs`. **What is open is (a) the WIT round-trip above
//!   ALONE** — `emit_user_bash_event` can read the `"operations"` key out of the reduction payload
//!   but there is nothing callable behind it until a guest can register one. **Owning item:
//!   DRIFT-004 / SEAM-015, `docs/gap-analysis/06-cyrup-ext.md`.**
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
pub use contract::{
    EventPatch, HandledValue, HookOutcome, Reduced, TerminalInputDecision, TerminalInputResult,
};
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
    CtxTier, ExtMode, HostCtx, HostCtxRich, HostCtxSource, HumanWaitGate, HumanWaitGuard, InitApi,
    NativeExtension, NativeHandle,
};
/// EXT-060: the `HostServices` -> [`native::HostCtxSource`] adapter is only meaningful when the
/// capability backend exists, but the TRAIT it feeds is unconditional.
#[cfg(feature = "wasm-host")]
pub use native::ServicesCtxSource;
pub use provider::{
    resolve_api_key, ModelCost, ModelCostTier, ModelRegistrySink, ProviderConfig, ProviderHub,
    ProviderModelConfig, ProviderRegistration,
};
pub use registry::{
    CommandDescriptor, ExecModeWire, ExtensionConflict, ExtensionProvenance, ExtensionRegistry,
    ResolvedCommand, ToolDescriptor,
};
pub use subscriber::ExtSubscriber;
pub use wrapper::{wrap_registered_tool, ActiveToolNames, RegisteredTool};

#[cfg(feature = "wasm-host")]
pub use host::{
    CannedResponses, ControlOp, DenyServices, DialogOptions, EpochDriver, ExecOutput, FsCaps,
    DENIED_EXEC, DENIED_NET, DENIED_UI,
    GuestState, HostServices, HttpRequest, HttpResponse, HttpStreamResponse, HumanInteractionGuard,
    CustomOption, CustomSpec, SpecOverlay,
    HumanInteractionLock, InteractiveOverlay, LiveExtension, NotifyKind, OAuthEvent,
    OverlayColor, OverlayKey, OverlayKeyCode, OverlayLine, OverlayOutcome, OverlaySpan,
    ProcSpawnSpec, RecordingServices, StoreLimits, UiChrome, WasmTool,
};
#[cfg(feature = "wasm-host")]
pub use host_runtime::WasmRuntime;
