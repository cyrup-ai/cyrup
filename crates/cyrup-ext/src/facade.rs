//! The `ExtensionHost` facade (arch-08 §3.1): the single entry point the session service wires in.
//! Holds the registry + dispatcher + native registry (+ the Wasmtime engine/pool when the
//! `wasm-host` feature is on). Exposes the two agent seams — [`ExtSubscriber`] (notify) and
//! [`ExtHooks`] (mutating) — plus the merged active tool set.

use crate::contract::{HandledValue, Reduced, TerminalInputDecision, TerminalInputResult};
use crate::dispatch::Dispatcher;
use crate::error::ExtError;
use crate::event::{HostEvent, InputEventSource, InputStreamingBehavior};
use crate::hooks::ExtHooks;
use crate::loader::{DiscoveredExtension, DiscoveryRoots, LoadError};
// EXT-026: `LoadError` is NOT wasm-host-gated — `discover_with_diagnostics` returns it in every
// build (a manifest that will not parse is a diagnostic whether or not a guest could be
// instantiated). `LoadExtensionsResult` genuinely is guest-loading only.
#[cfg(feature = "wasm-host")]
use crate::loader::LoadExtensionsResult;
#[cfg(feature = "wasm-host")]
use crate::manifest::{Capabilities, HOST_WORLD};
use crate::native::{ExtMode, HostCtx, InitApi, NativeExtension, NativeHandle};
use crate::registry::{CommandDescriptor, ExtensionRegistry};
use crate::subscriber::ExtSubscriber;
use cyrup_agent::{EventSubscriber, Hooks};
use cyrup_core::{CancelToken, Content, ExtensionId, Message, Tool};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// The reduced result of [`ExtensionHost::emit_before_agent_start`] (Pi `BeforeAgentStartCombinedResult`,
/// runner.ts:1036-1042). `None` from the emit method = no handler modified anything (Pi returns
/// `undefined`); a `Some` carries only what changed: the (optionally) replaced system prompt and the
/// messages injected across the handler chain (accumulated, Pi `messages.push`, runner.ts:1014).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BeforeAgentStartReduction {
    /// The replaced system prompt, iff at least one handler changed it (Pi `systemPromptModified`).
    pub system_prompt: Option<String>,
    /// Messages injected by handlers, in chain order (Pi `messages`).
    pub injected: Vec<Message>,
}

/// The reduced result of [`ExtensionHost::emit_input`] (Pi `InputEventResult`, types.ts:805-808):
/// continue unchanged, the transformed text/images, a full short-circuit (`handled`), or a block.
#[derive(Clone, Debug)]
pub enum InputReduction {
    /// No handler changed the submission (Pi `{action:"continue"}`).
    Continue,
    /// A handler rewrote the text and/or images (Pi `{action:"transform", text, images}`).
    Transform { text: String, images: Vec<Content> },
    /// A handler fully serviced the input (Pi `{action:"handled"}`) — do not submit it.
    Handled,
    /// A handler blocked the submission (first block wins).
    Blocked {
        reason: Option<String>,
        by: ExtensionId,
    },
}

/// The reduced result of [`ExtensionHost::emit_user_bash`] (Pi `UserBashEventResult`, `extensions/types.ts:1078-1083` @v0.83.0; EXT-036 corrected `:1043`, a member of the `ExtensionEvent` union):
/// proceed, the extension fully serviced it (`operations`/`result`), or a block.
#[derive(Clone, Debug)]
pub enum UserBashReduction {
    /// No handler intercepted the `!`/`!!` command (proceed with the default bash execution).
    Continue,
    /// A handler fully serviced it (Pi `{operations}`/`{result}`) — carried as the open-shaped value.
    Handled(Value),
    /// A handler blocked the command (first block wins).
    Blocked {
        reason: Option<String>,
        by: ExtensionId,
    },
}

/// The reduced result of [`ExtensionHost::emit_session_before_compact`] (Pi
/// `SessionBeforeCompactResult`, types.ts:1077-1080): proceed with the default compaction, a veto
/// (first block wins), or an extension-supplied compaction override (Pi `compaction: CompactionResult`).
#[derive(Clone, Debug)]
pub enum CompactionReduction {
    /// No handler intervened — run the default (model) compaction.
    Proceed,
    /// A handler vetoed the compaction (Pi `{cancel:true}`); first block wins.
    Blocked {
        reason: Option<String>,
        by: ExtensionId,
    },
    /// A handler supplied a compaction override (Pi `SessionBeforeCompactResult.compaction`) — the
    /// producer threads its summary/details into the appended compaction entry (`fromExtension`).
    Override(Value),
}

/// The reduced result of [`ExtensionHost::emit_session_before_tree`] (Pi `SessionBeforeTreeResult`,
/// types.ts:1082-1094): proceed, a veto, or a summary/customInstructions/label override.
#[derive(Clone, Debug)]
pub enum TreeReduction {
    /// No handler intervened — run the default branch summarization / navigation.
    Proceed,
    /// A handler vetoed the navigation (Pi `{cancel:true}`); first block wins.
    Blocked {
        reason: Option<String>,
        by: ExtensionId,
    },
    /// A handler supplied a summary/customInstructions/label override (Pi `SessionBeforeTreeResult`).
    Override(Value),
}

/// A captured CLI extension-flag value (Pi `unknownFlags` map entry, args.ts:52-53). `Bool(true)` is
/// a bare `--flag`; `Str` is `--flag=value`. Mirrors the bin/session-svc `ExtensionFlagValue` so
/// [`ExtensionHost::apply_extension_flag_values`] can apply Pi's `applyExtensionFlagValues` type
/// rules (agent-session-services.ts:102-114) without depending on the downstream crates' own type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExtensionFlagOverride {
    Bool(bool),
    Str(String),
}

/// Configuration for the host (mode + cwd + UI availability drive the dispatch `HostCtx`).
#[derive(Clone, Debug)]
pub struct HostConfig {
    pub mode: ExtMode,
    pub has_ui: bool,
    pub cwd: PathBuf,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self {
            mode: ExtMode::default(),
            has_ui: true,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }
}

/// The host's [`crate::native::LateRegistrar`] implementation — one per loaded native, handed to it
/// before `init` (HA-1 / MCP-037).
///
/// Holds the `Arc<ExtensionRegistry>` rather than the host, which is what makes this a narrow
/// capability instead of a back-door: the registry does not own the host, so there is no cycle to
/// break with a `Weak`, and the handle can reach registration and nothing else.
///
/// `owner` is bound HERE, at construction, not passed by the caller. An extension holding this
/// cannot register under another extension's id — which the alternative shape (handing out a
/// `Weak<ExtensionHost>` and letting the extension call `register_late_tool(owner, tool)`) would
/// have allowed, since that `owner` is just a parameter.
struct HostLateRegistrar {
    registry: Arc<ExtensionRegistry>,
    owner: ExtensionId,
    /// Fires after a command registration so a snapshot consumer can rebuild. Held as a callback
    /// rather than a host handle for the same reason as `registry` above.
    on_commands_changed: Arc<dyn Fn() + Send + Sync>,
}

impl crate::native::LateRegistrar for HostLateRegistrar {
    fn register_tool(&self, tool: Arc<dyn Tool>) -> Result<(), ExtError> {
        // The same call `register_late_tool` makes. `register_tool_inner` raises the tools-dirty
        // flag, `refresh_tools` reports it (MCP-037a), `AgentSession::refresh_extension_tools`
        // merges and `push_active_tools` rewrites the agent's tool array and the system prompt at
        // the next turn boundary. Every step of that already existed; this call is what was missing.
        self.registry.register_tool(self.owner.clone(), tool)
    }

    fn register_command(&self, name: String, desc: CommandDescriptor) -> Result<(), ExtError> {
        self.registry
            .register_command(self.owner.clone(), name, desc)?;
        (self.on_commands_changed)();
        Ok(())
    }

    fn register_tool_renderer(&self, tool_name: String) -> Result<(), ExtError> {
        self.registry
            .register_tool_renderer(self.owner.clone(), tool_name)
    }

    fn owner(&self) -> ExtensionId {
        self.owner.clone()
    }
}

/// The extension host facade (arch-08 §3.1).
pub struct ExtensionHost {
    dispatcher: Arc<Dispatcher>,
    registry: Arc<ExtensionRegistry>,
    config: HostConfig,
    loaded: RwLock<Vec<ExtensionId>>,
    /// Loaded native built-ins keyed by id — the native slash-command routing table (R-08-016). A
    /// native command runs in-process (no wasm), so the handle is kept here to reach its
    /// [`NativeExtension::execute_command`] when a `/<cmd>` it owns is invoked.
    ///
    /// `Arc`-shared with [`BusFanout`] (EXT-034) so the bus drain can resolve a subscriber without
    /// borrowing the host — the dispatcher holds the fan-out, not the facade.
    native: Arc<NativeMap>,
    #[cfg(feature = "wasm-host")]
    wasm: Option<crate::host_runtime::WasmRuntime>,
    /// Loaded live wasm instances keyed by id — the slash-command/reload routing table (R-08-016/005).
    /// `Arc`-shared with [`BusFanout`] for the same reason as [`Self::native`].
    #[cfg(feature = "wasm-host")]
    live: Arc<LiveMap>,
    /// The single host-owned inter-extension event bus (pi's one `createEventBus()` threaded to
    /// every extension, `core/event-bus.ts:12-32` / `extensions/loader.ts:389` @v0.83.0). Shared
    /// into every loaded guest AND consulted for every loaded native, so a `bus.emit` reaches any
    /// subscriber whichever tier it lives in.
    ///
    /// NOT `wasm-host`-gated (EXT-018). It used to be, which meant the three extensions cyrup
    /// ships — permission-system, intercom, subagents, all natives — had no `pi.events` at all.
    /// Upstream hangs `events` on the ONE base `ExtensionAPI` every extension receives; which tier
    /// an extension runs in is not something the coordination channel is allowed to know.
    bus: Arc<crate::bus::SharedBus>,
    /// The live rich-ctx source every native dispatch/command/shortcut ctx is enriched from
    /// (EXT-060). Feature-INDEPENDENT, unlike the `wasm-host`-gated capability backend:
    /// `HostCtxSource` is the read-only five-getter slice of that backend, so a
    /// `--no-default-features` host can attach one and its native built-ins read the same live
    /// `is_idle`/`is_project_trusted` the shipped arm does. Set by [`Self::set_ctx_source`], which
    /// [`Self::load_native_with_services`] calls for the `wasm-host` path.
    ctx_source: RwLock<Option<Arc<dyn crate::native::HostCtxSource>>>,
    /// Subscribers notified after a command registration lands from a LIVE handler (HA-1's command
    /// leg, MCP-039/MCP-395). Tools need no such list — they have the tools-dirty flag and
    /// `AgentSession`'s turn-boundary poll — but commands have neither: `resolved_commands()` is
    /// read live by the RPC catalog, while the TUI `/` menu is a SNAPSHOT rebuilt at exactly three
    /// points (session start, session swap, the `enableSkillCommands` toggle), none of them
    /// extension-driven. Without this a late command is invocable by typing it in full and
    /// invisible in the menu.
    ///
    /// Shaped after [`Self::add_error_listener`], which is already feature-independent and already
    /// consumed by the TUI through an `UnboundedSender`.
    commands_listeners: Arc<RwLock<Vec<crate::CommandsListener>>>,
    /// The live `getActiveTools` source the registered-tool wrapper diffs against (Pi
    /// `ExtensionRunner.getActiveTools`, runner.ts:664-667). `None` until a session attaches one
    /// via [`Self::set_active_tool_source`], in which case [`Self::active_tools`] hands tools back
    /// UNWRAPPED — there is no live agent whose tool set could change, so the diff has no meaning.
    active_tool_source: RwLock<Option<Arc<dyn crate::wrapper::ActiveToolNames>>>,
    /// The bus fan-out (EXT-034). Owned here (strong) and handed to the dispatcher as a `Weak`, so
    /// every dispatch entry point drains after its subscriber loop — not just the two command-tier
    /// call sites that used to be the only drain.
    fanout: Arc<BusFanout>,
    /// Tools that carry their OWN `render_call`/`render_result`, keyed by tool name — the SDK half
    /// of upstream's one renderer map, consulted by [`Self::render_tool_call_outcome`].
    ///
    /// Upstream keeps a SINGLE map: `_toolDefinitions` is the built-in table overlaid by a loop
    /// over `allCustomTools` = `extensionRunner.getAllRegisteredTools()` **followed by** the SDK
    /// `_customTools` (`core/agent-session.ts:2471-2495` @v0.84.2), and the resolver reads
    /// `session.getToolDefinition(name)?.renderCall` out of it
    /// (`modes/interactive/components/tool-execution.ts:84-91`, via `interactive-mode.ts:1996`).
    /// cyrup's extension tier is keyed in [`ExtensionRegistry`] by the OWNING extension, which a
    /// plain `Arc<dyn Tool>` has none of, so the two halves cannot share one table here.
    ///
    /// **Precedence follows upstream's write order, not the table split.** `_customTools` is spread
    /// LAST into `allCustomTools` and the loop is a plain `definitionRegistry.set(name, …)`, so on
    /// a name collision the SDK tool overwrites the extension-registered one — which is why
    /// [`Self::render_tool_call_outcome`] consults THIS table first and the extension registry
    /// second.
    native_tool_renderers: RwLock<std::collections::HashMap<String, Arc<dyn Tool>>>,
    /// Counts [`Self::invalidate_live`] calls — see [`Self::live_invalidations`].
    live_invalidations: std::sync::atomic::AtomicU64,
}

/// The native routing table, shared between the facade and the bus fan-out.
type NativeMap = RwLock<std::collections::HashMap<ExtensionId, Arc<dyn NativeExtension>>>;
/// The live-guest routing table, shared between the facade and the bus fan-out.
#[cfg(feature = "wasm-host")]
type LiveMap = RwLock<std::collections::HashMap<ExtensionId, Arc<crate::host::LiveExtension>>>;

/// The inter-extension bus fan-out, extracted from the facade so it can be reached from the
/// dispatcher (EXT-034).
///
/// pi needs no such object: `createEventBus()` delivers synchronously at the emit call
/// (`pi/packages/coding-agent/src/core/event-bus.ts:12-32` @v0.83.0), so there is no drain point
/// and nothing to give a handle to. cyrup's delivery is deferred (a guest's `bus.emit` import runs
/// while that guest holds its own single-instance store), so the drain has to be invoked at every
/// seam that can have re-entered a guest — and [`Dispatcher`], which owns the event seams, holds no
/// reference to the facade.
pub(crate) struct BusFanout {
    bus: Arc<crate::bus::SharedBus>,
    config: HostConfig,
    native: Arc<NativeMap>,
    #[cfg(feature = "wasm-host")]
    live: Arc<LiveMap>,
    /// For [`Dispatcher::report_external`] — the `onError` channel EXT-057b routes bus faults onto.
    dispatcher: Arc<Dispatcher>,
    /// Set while a drain is in progress, so a nested seam (the facade's own explicit
    /// [`ExtensionHost::deliver_bus_events`], or a handler reached from a delivery) does not start a
    /// second concurrent fan-out over the same queue.
    draining: std::sync::atomic::AtomicBool,
}

impl Default for ExtensionHost {
    fn default() -> Self {
        Self::new(HostConfig::default())
    }
}

impl ExtensionHost {
    /// A native-only host foundation (no Wasmtime engine spun up). Sufficient for the full
    /// dispatch/registration/seam/containment surface (tested without wasm).
    pub fn new(config: HostConfig) -> Self {
        let dispatcher = Arc::new(Dispatcher::new());
        let registry = Arc::new(ExtensionRegistry::new());
        let bus = Arc::new(crate::bus::SharedBus::new());
        let native: Arc<NativeMap> = Arc::new(RwLock::new(std::collections::HashMap::new()));
        #[cfg(feature = "wasm-host")]
        let live: Arc<LiveMap> = Arc::new(RwLock::new(std::collections::HashMap::new()));
        let fanout = Arc::new(BusFanout {
            bus: bus.clone(),
            config: config.clone(),
            native: native.clone(),
            #[cfg(feature = "wasm-host")]
            live: live.clone(),
            dispatcher: dispatcher.clone(),
            draining: std::sync::atomic::AtomicBool::new(false),
        });
        // EXT-034: the dispatcher holds the drain WEAKLY. `fanout` already holds the dispatcher
        // strongly (it needs `report_external`), so a strong edge back would be a reference cycle
        // that leaks the whole host.
        dispatcher
            .set_bus_drain(Arc::downgrade(&fanout) as std::sync::Weak<dyn crate::bus::BusDrain>);
        Self {
            dispatcher,
            registry,
            config,
            loaded: RwLock::new(Vec::new()),
            native,
            #[cfg(feature = "wasm-host")]
            wasm: None,
            #[cfg(feature = "wasm-host")]
            live,
            native_tool_renderers: RwLock::new(std::collections::HashMap::new()),
            live_invalidations: std::sync::atomic::AtomicU64::new(0),
            bus,
            ctx_source: RwLock::new(None),
            commands_listeners: Arc::new(RwLock::new(Vec::new())),
            active_tool_source: RwLock::new(None),
            fanout,
        }
    }

    /// Attach the live `getActiveTools` source (Pi binds `runtime.getActiveTools` from the session's
    /// actions, runner.ts:329 — `:330` is `getAllTools`). Every tool [`Self::active_tools`] returns from then on is wrapped
    /// for `addedToolNames` derivation (Pi `wrapRegisteredTool`, extensions/wrapper.ts:17-36).
    /// Idempotent; the last source wins.
    pub fn set_active_tool_source(&self, source: Arc<dyn crate::wrapper::ActiveToolNames>) {
        if let Ok(mut g) = self.active_tool_source.write() {
            *g = Some(source);
        }
    }

    /// Wrap one tool for `addedToolNames` derivation with the attached source, or return it as-is
    /// when no live agent is attached. Public so the session builder can put the SDK-supplied
    /// custom tools through the same wrapper Pi applies to its `_baseToolDefinitions`
    /// (agent-session.ts:2507-2515) without reaching into the registry.
    pub fn wrap_tool(&self, tool: Arc<dyn Tool>) -> Arc<dyn Tool> {
        match self.active_tool_source.read().ok().and_then(|g| g.clone()) {
            Some(src) => crate::wrapper::wrap_registered_tool(tool, src),
            None => tool,
        }
    }

    /// Load a compiled-in native extension (R-ARCH-EXT-003). Awaits `init` (R-08-001), registers its
    /// tools/commands, builds its subscription bitset, and wires it into the dispatcher in load order.
    pub async fn load_native(&self, ext: Arc<dyn NativeExtension>) -> Result<(), ExtError> {
        self.load_native_inner(ext).await
    }

    /// As [`Self::load_native`], but late-binds the live `Arc<dyn HostServices>` into the native
    /// extension BEFORE `init` (reconciliation §2 item 1 / P-1). The session builder threads its own
    /// `LiveHostServices` here so a native built-in captures the SAME backend the WASM path already
    /// gets via `discover_and_load` — letting a background tokio task the built-in spawns reach the
    /// real session id/file, dialogs, and message-injection OUTSIDE any live `HostCtx`. Binding before
    /// `init` means an `init`-spawned background task already holds the backend; the session attaches
    /// the manager / ui sink / inject sink LATER (builder steps 6/10 + the mode entry point), and the
    /// captured `Arc` observes those through interior mutability, so early capture is correct.
    #[cfg(feature = "wasm-host")]
    pub async fn load_native_with_services(
        &self,
        ext: Arc<dyn NativeExtension>,
        services: Arc<dyn crate::host::HostServices>,
    ) -> Result<(), ExtError> {
        ext.set_host_services(services.clone());
        // Keep the backend so EVERY native dispatch/command ctx can carry live rich fields
        // (EXT-005), not just the built-ins that stash the Arc themselves. It is held as the
        // feature-independent [`crate::native::HostCtxSource`] slice (EXT-060) — the ONLY thing the
        // facade ever read the backend for — so the enrichment exists on both arms of `wasm-host`.
        self.set_ctx_source(Arc::new(crate::native::ServicesCtxSource(services)));
        self.load_native_inner(ext).await
    }

    /// Attach the live source of the rich `HostCtx` fields (EXT-005/EXT-060). Idempotent; the last
    /// source wins. [`Self::load_native_with_services`] calls this with the injected
    /// `HostServices`; a host built WITHOUT `wasm-host` calls it directly, which is the whole point
    /// of the seam being feature-independent — see [`crate::native::HostCtxSource`].
    pub fn set_ctx_source(&self, source: Arc<dyn crate::native::HostCtxSource>) {
        if let Ok(mut g) = self.ctx_source.write() {
            *g = Some(source);
        }
    }

    /// The attached rich-ctx source, if any (EXT-060).
    fn ctx_source(&self) -> Option<Arc<dyn crate::native::HostCtxSource>> {
        self.ctx_source.read().ok().and_then(|g| g.clone())
    }

    /// The shared native-load body (register tools/commands, build subscriptions, wire into the
    /// dispatcher). [`Self::load_native`] and [`Self::load_native_with_services`] differ only in
    /// whether they first late-bind the host-services slot (P-1).
    async fn load_native_inner(&self, ext: Arc<dyn NativeExtension>) -> Result<(), ExtError> {
        let id = ext.id();
        // A DUPLICATE id is rejected here and must NOT release: the reservation belongs to the
        // extension that is already loaded.
        self.reserve_id(&id)?;
        // EXT-S01: every failure PAST the reservation releases it again. Otherwise a native whose
        // `init()` returned `Err` stays in `loaded_ids()` — the startup listing reports it as
        // loaded, and a later legitimate load of the same id fails with a spurious `DuplicateId`.
        // Pi's `LoadExtensionsResult.extensions` only ever holds extensions that loaded; failures
        // live in the sibling `errors` array. (Registrations already written to the registry before
        // the failing step are left in place — a native `init` builds its whole `InitApi` before any
        // of them run, so in practice `init` is the only failing step this can reach.)
        let result = self.load_native_body(ext, id.clone()).await;
        if result.is_err() {
            self.release_id(&id);
        }
        result
    }

    /// The body of [`Self::load_native_inner`], run under its id reservation.
    async fn load_native_body(
        &self,
        ext: Arc<dyn NativeExtension>,
        id: ExtensionId,
    ) -> Result<(), ExtError> {
        // HA-1: bind the late-registration handle BEFORE `init`, for the same reason
        // `set_host_services` binds before it (P-1) — an `init`-spawned background task must
        // already hold it when it runs. Bound HERE, in the shared body, rather than in
        // `load_native_with_services`: that method is `cfg(feature = "wasm-host")`, and the whole
        // point of `LateRegistrar` being feature-independent is that both build arms get one.
        ext.set_late_registrar(self.late_registrar_for(id.clone()));

        let mut api = InitApi::new();
        ext.init(&mut api).await?;
        let (
            subs,
            tools,
            commands,
            tool_renderers,
            message_renderers,
            entry_renderers,
            shortcuts,
            flags,
            providers,
            autocomplete,
            autocomplete_providers,
            bus_topics,
            markdown_transformer,
            terminal_input,
        ) = api.into_parts();

        // EXT-003 footgun guard: a native that subscribes to `project_trust` but did NOT override
        // `decides_project_trust` is skipped by the pre-trust bootstrap pass, so its vote arrives
        // after trust is already decided and is silently ignored. Say so once, at load, rather than
        // letting the author debug a handler that "runs but changes nothing".
        if subs.contains(crate::EventKind::ProjectTrust) && !ext.decides_project_trust() {
            tracing::warn!(
                extension = %id,
                "extension subscribes to `project_trust` but does not override \
                 `NativeExtension::decides_project_trust`; its verdict cannot affect the session's \
                 trust decision, which is made before this (post-trust) load"
            );
        }

        for tool in tools {
            self.registry.register_tool(id.clone(), tool)?;
        }
        // SEAM-084 — a compiled-in native has no path and no directory, which is exactly upstream's
        // `loadExtensionFromFactory` case: its default `extensionPath` is the literal `"<inline>"`
        // (`core/extensions/loader.ts:490` @v0.83.0), so `createExtension`'s `<…>` split yields
        // `source: "inline"` and `baseDir: undefined`. Recorded alongside the commands it describes,
        // and after `init` has succeeded, so a native whose `init` failed leaves no orphan row.
        self.registry
            .record_extension_provenance(id.clone(), crate::ExtensionProvenance::inline())?;
        for (name, desc) in commands {
            self.registry.register_command(id.clone(), name, desc)?;
        }
        // EXT-006: a native built-in's renderer declarations land in the SAME registry tables the
        // guest path writes, so `render_tool_call`/`render_message_call` route by name/type without
        // caring which runtime supplies the renderer.
        for tool_name in tool_renderers {
            self.registry
                .register_tool_renderer(id.clone(), tool_name)?;
        }
        for custom_type in message_renderers {
            self.registry
                .register_message_renderer(id.clone(), custom_type)?;
        }
        // X15: the custom-ENTRY renderer table (Pi `extension.entryRenderers`, loader.ts:314-318) —
        // separate from the message table above, exactly as upstream keeps them.
        for custom_type in entry_renderers {
            self.registry
                .register_entry_renderer(id.clone(), custom_type)?;
        }

        // EXT-035: the six registration surfaces `interface registration` offered a WASM guest and
        // `InitApi` did not, so a native reached 5 of 11. pi has one api object for one extension
        // kind (`extensions/loader.ts:274-410` @v0.83.0) and no notion of an extension that can
        // register tools but not shortcuts, flags or providers.
        for (key, desc) in shortcuts {
            self.registry.register_shortcut(id.clone(), key, desc)?;
        }
        for (name, spec) in flags {
            self.registry.register_flag(id.clone(), name, spec)?;
        }
        for (provider_id, config) in providers {
            self.registry
                .register_provider(id.clone(), provider_id, config)?;
        }
        for command in autocomplete {
            self.registry
                .add_command_autocomplete(id.clone(), command)?;
        }
        for _ in 0..autocomplete_providers {
            self.registry.add_autocomplete_provider(id.clone())?;
        }
        // EXT-019: at most one markdown transformer per extension, recorded in LOAD ORDER (pi
        // assigns `extension.markdownTransformer`, `extensions/loader.ts:309-312` @v0.84.1, and
        // folds them in `this.extensions` order, `runner.ts:589-591`).
        if markdown_transformer {
            self.registry.register_markdown_transformer(id.clone())?;
        }
        // EXT-021: terminal-input subscription, also in LOAD ORDER — pi folds its listeners in the
        // insertion order of the `Set` `addInputListener` writes to
        // (`packages/tui/src/tui.ts:651-655`, folded `:773-788`).
        if terminal_input {
            self.registry.subscribe_terminal_input(id.clone())?;
        }
        // EXT-018: a native's bus subscriptions land in the SAME host-owned bus a guest's
        // `bus.subscribe` import writes to (pi's single `createEventBus()`, `loader.ts:389`).
        for topic in bus_topics {
            self.bus.subscribe(id.clone(), topic);
        }

        // Keep the native handle for command-tier slash execution (R-08-016) before it is wrapped
        // for event dispatch.
        if let Ok(mut g) = self.native.write() {
            g.insert(id.clone(), ext.clone());
        }
        let ctx = HostCtx::event(
            self.config.mode,
            self.config.has_ui,
            self.config.cwd.clone(),
        );
        let handle = NativeHandle::new(ext, subs, ctx).with_ctx_source(self.ctx_source());
        self.dispatcher.add(Arc::new(handle))?;
        Ok(())
    }

    /// Execute a NATIVE built-in's registered slash command (Pi `_tryExecuteExtensionCommand` →
    /// `command.handler(args, ctx)`, agent-session.ts:1148-1172; R-08-016). Routes the command name
    /// to its owning native extension and runs [`NativeExtension::execute_command`] with a
    /// **command-tier** [`HostCtx`] (session mutation allowed). Returns `Ok(None)` when no native
    /// extension owns the command (the caller falls through, e.g. to the wasm path or normal prompt
    /// handling); `Ok(Some(_))`/`Ok(Some(None))` when a native owner serviced it. Panic-contained
    /// like event dispatch (R-08-036): a panicking handler is surfaced as `ExtError::Panicked`.
    pub async fn execute_native_command(
        &self,
        name: &str,
        args: &str,
        cancel: &CancelToken,
    ) -> Result<Option<Result<Option<String>, ExtError>>, ExtError> {
        // SEAM-048 — route by INVOCATION name, not by the raw last-wins map. pi disambiguates two
        // extensions registering `deploy` into `deploy:1` / `deploy:2` in load order
        // (`resolveRegisteredCommands`, `extensions/runner.ts:598-631` @v0.83.0) and dispatches from
        // the `ResolvedCommand`; `command_owner` is a last-wins `HashMap` lookup on the raw name, so
        // `deploy:2` resolved to nothing and the second registrant was silently unexecutable.
        //
        // [`Self::command_route`] carries the REGISTERED name back alongside the owner, and that is
        // what goes to the handler below — see its doc comment for why passing `name` through was
        // still a dead end for every suffixed invocation.
        let (owner, registered) = match self.command_route(name)? {
            Some(route) => route,
            None => return Ok(None),
        };
        let ext = match self.native.read().ok().and_then(|g| g.get(&owner).cloned()) {
            Some(e) => e,
            // The command is owned by a non-native (wasm) extension: not our route.
            None => return Ok(None),
        };
        let ctx = HostCtx::command(
            self.config.mode,
            self.config.has_ui,
            self.config.cwd.clone(),
        );
        // Live rich fields for the command ctx too (EXT-005) — a command handler reading
        // `ctx.is_idle()`/`ctx.is_project_trusted()` used to get `HostCtxRich::default()`.
        let ctx = match self.ctx_source() {
            Some(src) => ctx.with_rich(src.rich()),
            None => ctx,
        };
        let fut = std::panic::AssertUnwindSafe(ext.execute_command(&registered, args, &ctx));
        use futures::FutureExt;
        let raced = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(Some(Err(ExtError::Cancelled))),
            r = fut.catch_unwind() => r,
        };
        match raced {
            Ok(out) => Ok(Some(out)),
            Err(panic) => Ok(Some(Err(ExtError::Panicked(native_panic_msg(panic))))),
        }
    }

    /// The ordered-awaited, notify-only subscriber handed to the agent (R-02-012/048).
    ///
    /// EXT-061: no cancel token is taken. pi passes the run's signal to each listener at the emit
    /// (`await listener(event, signal)`, `packages/agent/src/agent.ts:574` @v0.83.0) and keeps no
    /// subscriber-lifetime token; `ExtSubscriber` correspondingly holds none, and the per-event
    /// token `EventSubscriber::on_event` receives is what every dispatched handler races against.
    pub fn subscriber(&self) -> Arc<dyn EventSubscriber> {
        Arc::new(ExtSubscriber::new(self.dispatcher.clone()))
    }

    /// The mutating hooks adapter handed to the agent (arch-02 §3.3).
    pub fn hooks(&self) -> Arc<dyn Hooks> {
        Arc::new(ExtHooks::new(self.dispatcher.clone()))
    }

    /// The merged active tool set: built-ins overridden by extension tools (R-08-012/014).
    ///
    /// Re-materializes any tool registered SINCE the last call first ([`Self::refresh_tools`]) —
    /// Pi's `registerTool()` ends with `runtime.refreshTools()` on every registration
    /// (extensions/loader.ts:249-256), so a tool a guest registers from inside a `session_start`
    /// handler is model-visible on the next turn (`examples/extensions/dynamic-tools.ts`). Before
    /// EXT-004 the wrapping happened exactly once, immediately after `init`, so a post-`init`
    /// registration produced a descriptor no caller could ever execute.
    ///
    /// Every tool that comes back is put through the registered-tool wrapper
    /// ([`Self::wrap_tool`]) — the built-ins in `base` as well as the extension-contributed ones,
    /// exactly as Pi wraps BOTH halves of `_toolRegistry` (`wrapRegisteredTools(allCustomTools,
    /// runner)` + `wrapRegisteredTools(baseToolDefinitions…)`, agent-session.ts:2506-2515). That
    /// wrapper is the only producer of `ToolResult::added_tool_names`: a tool never sets the field
    /// itself upstream, the host derives it from the active-set diff around `execute`.
    pub fn active_tools(&self, base: &[Arc<dyn Tool>]) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        self.refresh_tools()?;
        let merged = self.registry.active_tools(base)?;
        let Some(src) = self.active_tool_source.read().ok().and_then(|g| g.clone()) else {
            return Ok(merged);
        };
        Ok(merged
            .into_iter()
            .map(|t| crate::wrapper::wrap_registered_tool(t, src.clone()))
            .collect())
    }

    /// Re-materialize guest tool descriptors registered after their extension's `init` into
    /// executable `Arc<dyn Tool>` handles (Pi `refreshTools` → `_refreshToolRegistry`,
    /// agent-session.ts:2452-2546; EXT-004). Returns whether any tool registration has landed since
    /// the last call, so a caller can skip an expensive downstream rebuild (system prompt /
    /// active-set push) when nothing has.
    ///
    /// Cheap when nothing changed: a relaxed atomic load, no lock. Sync on purpose — wrapping a
    /// descriptor is pure bookkeeping, so this is callable from `active_tools` and from a
    /// non-async drain alike.
    pub fn refresh_tools(&self) -> Result<bool, ExtError> {
        if !self.registry.take_tools_dirty() {
            return Ok(false);
        }
        self.materialize_guest_tools()?;
        // The FLAG is the answer, not the materializer's own bookkeeping. It is raised by exactly
        // one thing — a tool registration landing in the registry — and this call has just
        // consumed it, so the tool set demonstrably changed. Reporting the materializer's
        // `changed` instead made every registration the materializer does not itself re-wrap read
        // as "nothing happened": a NATIVE late registration (`register_late_tool` ->
        // `register_tool`, which lands in the executable `tools` map, not `guest_tools`) always
        // came back `Ok(false)`, and so did a guest descriptor whose owner was not yet live —
        // while `take_tools_dirty` is a `swap(false)`, so the signal was consumed and destroyed
        // with nothing to re-read on a later turn. `AgentSession::refresh_extension_tools`
        // hard-gates on this bool with no diagnostic (MCP-037a). Upstream cannot express the bug:
        // `registerTool` ends with an unconditional `runtime.refreshTools()`
        // (`extensions/loader.ts:245-252` @v0.83.0) and `_refreshToolRegistry` rebuilds the whole
        // registry every time (`core/agent-session.ts:2452-2546`).
        Ok(true)
    }

    /// The `wasm-host` half of [`Self::refresh_tools`]: wrap every guest descriptor whose owner is
    /// live into a [`crate::host::WasmTool`] bound to that instance.
    ///
    /// EXT-059: every descriptor is re-wrapped, not just the ones with no executable counterpart
    /// yet. [`crate::ExtensionRegistry::register_guest_tool`] REPLACES the descriptor when its own
    /// owner re-registers the same name (the documented `dynamic-tools.ts` pattern: same tool,
    /// changed schema / description / guidelines / `prepare_arguments` / `render_shell` /
    /// `constrained_sampling`), but [`crate::host::WasmTool`] captured the descriptor BY VALUE at
    /// first materialization. Skipping on `registry.tool(&name).is_some()` therefore answered
    /// "already materialized" from a subset of the question — a tool of that name exists — and
    /// left the model looking at the ORIGINAL parameters for the rest of the session, while
    /// `registry.tool_info()` (which reads `guest_tools` directly) showed the guest the NEW ones.
    /// pi holds no such handle: `_refreshToolRegistry` rebuilds `_toolDefinitions`,
    /// `_toolPromptSnippets`, `_toolPromptGuidelines` and `_toolRegistry` from scratch out of
    /// `getAllRegisteredTools()` on every refresh, so a stale definition is not representable.
    /// This is that full rebuild, scoped to the guest half (native tools are already executable
    /// `Arc<dyn Tool>`s and are never re-derived from a descriptor).
    #[cfg(feature = "wasm-host")]
    fn materialize_guest_tools(&self) -> Result<(), ExtError> {
        for (owner, desc) in self.registry.guest_tool_entries()? {
            let Some(ext) = self.live.read().ok().and_then(|g| g.get(&owner).cloned()) else {
                // A descriptor whose owner is not (yet) live: skip it rather than fabricating a
                // tool that cannot execute. Re-arm so the next refresh retries once it is live.
                self.registry.mark_tools_dirty();
                continue;
            };
            let tool: Arc<dyn Tool> = Arc::new(crate::host::WasmTool::new(ext, desc));
            // EXT-030: register QUIETLY. This pass is already the consumer of the dirty flag
            // (`refresh_tools` took it at entry), so its own re-registrations are not new signal.
            // The previous shape raised the flag here and then cleared it wholesale with
            // `take_tools_dirty()`, which also swallowed the deliberate `mark_tools_dirty()`
            // re-arm three lines above — a descriptor whose owner was not yet live was dropped
            // for the rest of the session — plus any mark another extension raised concurrently.
            self.registry.register_materialized_tool(owner, tool)?;
        }
        Ok(())
    }

    /// Native-only build: a native extension's tools are already executable `Arc<dyn Tool>`s
    /// registered directly into the registry, so "re-materialize" is genuinely a no-op. Both arms
    /// return the SAME thing now (nothing): the two used to disagree about the caller-visible
    /// `bool` — `Ok(true)` here, `Ok(false)` from the `wasm-host` arm for anything it did not
    /// itself re-wrap — which is how MCP-037a stayed invisible on the shipped arm (`default =
    /// ["wasm-host"]`) while the other arm looked correct. The refresh answer now comes from the
    /// dirty flag in [`Self::refresh_tools`], which is tier-independent.
    #[cfg(not(feature = "wasm-host"))]
    fn materialize_guest_tools(&self) -> Result<(), ExtError> {
        Ok(())
    }

    /// Register a tool AFTER its extension's `init` (Pi `api.registerTool()` called from a live
    /// handler, extensions/loader.ts:249-256 — the `examples/extensions/dynamic-tools.ts` pattern).
    /// Marks the tool set dirty so the next [`Self::refresh_tools`]/[`Self::active_tools`] surfaces
    /// it. This is the NATIVE analog of the guest's `registration.register-tool` import, which a
    /// native built-in cannot reach (it has no wasm boundary to call across).
    pub fn register_late_tool(
        &self,
        owner: ExtensionId,
        tool: Arc<dyn Tool>,
    ) -> Result<(), ExtError> {
        self.registry.register_tool(owner, tool)
    }

    // NOTE on callers: this is the EMBEDDER-facing door — a host that already holds the
    // `ExtensionHost` and is registering on some extension's behalf. An EXTENSION reaches the same
    // `ExtensionRegistry::register_tool` through [`crate::native::LateRegistrar`], which is handed
    // to it at load with its own id already bound, so it cannot pass an `owner` it does not own.
    // Two doors, one call; neither is dead.

    /// The command sibling of [`Self::register_late_tool`] (MCP-039 / MCP-395). The registry half
    /// already existed and already handled re-registration in place; this is the facade verb that
    /// was missing, plus the notification a snapshot consumer needs.
    pub fn register_late_command(
        &self,
        owner: ExtensionId,
        name: impl Into<String>,
        desc: CommandDescriptor,
    ) -> Result<(), ExtError> {
        self.registry.register_command(owner, name, desc)?;
        self.notify_commands_changed();
        Ok(())
    }

    /// Build the per-extension [`crate::native::LateRegistrar`] handed to a native before `init`.
    fn late_registrar_for(&self, owner: ExtensionId) -> Arc<dyn crate::native::LateRegistrar> {
        // The notifier closes over the SHARED listener list, not over the host: a registrar
        // outlives nothing it should keep alive, and holds no path back to the facade.
        let listeners = Arc::clone(&self.commands_listeners);
        Arc::new(HostLateRegistrar {
            registry: Arc::clone(&self.registry),
            owner,
            on_commands_changed: Arc::new(move || {
                let subscribers = match listeners.read() {
                    Ok(g) => g.clone(),
                    Err(_) => return,
                };
                for l in subscribers {
                    l();
                }
            }),
        })
    }

    /// Subscribe to late COMMAND registrations (HA-1's command leg). Modelled on
    /// [`Self::add_error_listener`]: the TUI forwards into an `UnboundedSender` it already pumps
    /// and rebuilds its `/` menu from `slash_command_catalog()`, which is already live.
    pub fn add_commands_listener(&self, listener: crate::CommandsListener) {
        if let Ok(mut g) = self.commands_listeners.write() {
            g.push(listener);
        }
    }

    /// Fire every [`Self::add_commands_listener`] subscriber. A poisoned lock drops the
    /// notification rather than panicking a live handler: the command IS registered either way,
    /// and the menu re-syncs at the next session swap.
    fn notify_commands_changed(&self) {
        let listeners = match self.commands_listeners.read() {
            Ok(g) => g.clone(),
            Err(_) => return,
        };
        for l in listeners {
            l();
        }
    }

    /// Aggregate the `project_trust` decision across extensions (Pi `emitProjectTrustEvent`,
    /// extensions/runner.ts:203-232). The FIRST extension that returns a DECIDED tri-state
    /// (`"yes"`/`"no"`) wins and the chain stops there — an `"undecided"` handler falls through.
    /// `None` = nobody decided (the host falls back to its saved/default/prompt tiers).
    ///
    /// Short-circuiting matters beyond efficiency: Pi returns the instant a handler decides, so a
    /// later extension's `project_trust` handler must not run at all — it would otherwise execute
    /// (and side-effect) with its verdict silently discarded.
    pub async fn aggregate_project_trust(
        &self,
        cancel: &CancelToken,
    ) -> Option<crate::ProjectTrustDecision> {
        use crate::event::HostEvent;
        let hit = self
            .dispatcher
            // EXT-043: the facade has held the cwd at `HostConfig.cwd` all along; pi's
            // `ProjectTrustEvent` is `{type, cwd}` (extensions/types.ts:519-522 @v0.83.0) and the
            // whole verdict is per-directory (`options.trustStore.set(options.cwd, trusted)`,
            // core/project-trust.ts:63-65), so a handler without it cannot key an allowlist.
            .dispatch_first_handled(
                &HostEvent::ProjectTrust {
                    cwd: self.config.cwd.to_string_lossy().into_owned(),
                },
                cancel,
                |HandledValue(v)| crate::aggregate::parse_trust_decision(v).is_some(),
            )
            .await?;
        crate::fold_project_trust(std::slice::from_ref(&hit))
    }

    /// Aggregate the skill/prompt/theme paths every extension provides (Pi `resources_discover`,
    /// runner.ts:197; gap-08 #4) into a typed, attributed [`crate::ResourcesAggregate`].
    pub async fn aggregate_resources(&self, cancel: &CancelToken) -> crate::ResourcesAggregate {
        use crate::event::HostEvent;
        // EXT-016: pi `ResourcesDiscoverEvent {type, cwd, reason: "startup" | "reload"}`
        // (extensions/types.ts:544-548 @v0.83.0). This entry point is the STARTUP discovery; the
        // reload path goes through `ExtensionHost::reload`, which passes "reload".
        let handled = self
            .dispatcher
            .dispatch_collect_handled(
                &HostEvent::ResourcesDiscover {
                    cwd: self.config.cwd.to_string_lossy().into_owned(),
                    reason: "startup".into(),
                },
                cancel,
            )
            .await;
        crate::fold_resources(&handled)
    }

    /// Dispatch `before_agent_start` (Pi `ExtensionRunner.emitBeforeAgentStart`, runner.ts:980-1044;
    /// gap-08 #1). Folds every subscribed handler's system-prompt replacement + message injection
    /// across the chain (later handlers observe the running system prompt; injected messages
    /// ACCUMULATE), returning what changed — or `None` when nothing did (Pi returns `undefined`).
    /// This is the production seam the prior doc claimed existed: it drives the live `on-before-agent-start`
    /// export and flows the guest's reduction back to the agent loop.
    pub async fn emit_before_agent_start(
        &self,
        prompt: &str,
        images: Value,
        system_prompt: &str,
        options: Value,
        cancel: &CancelToken,
    ) -> Option<BeforeAgentStartReduction> {
        let ev = HostEvent::BeforeAgentStart {
            prompt: prompt.to_string(),
            images,
            system_prompt: system_prompt.to_string(),
            options,
            injected: Vec::new(),
        };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Pass(ev) => {
                let HostEvent::BeforeAgentStart {
                    system_prompt: sp,
                    injected,
                    ..
                } = *ev
                else {
                    return None;
                };
                let changed = sp != system_prompt;
                if changed || !injected.is_empty() {
                    Some(BeforeAgentStartReduction {
                        system_prompt: if changed { Some(sp) } else { None },
                        injected,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Dispatch `input` (Pi `ExtensionRunner.emitInput`, runner.ts:1094-1134; gap-08 #2). A handler may
    /// block, fully service (`handled`), or transform the submission text/images; transforms FOLD across
    /// handlers. Returns the reduced [`InputReduction`] for the submission pipeline to apply.
    pub async fn emit_input(
        &self,
        text: &str,
        images: Vec<Content>,
        source: InputEventSource,
        streaming_behavior: Option<InputStreamingBehavior>,
        cancel: &CancelToken,
    ) -> InputReduction {
        let orig_text = text.to_string();
        let orig_images = images.clone();
        let ev = HostEvent::Input {
            text: orig_text.clone(),
            images,
            source,
            streaming_behavior,
        };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by, .. } => InputReduction::Blocked { reason, by },
            Reduced::Handled(_) => InputReduction::Handled,
            Reduced::Pass(ev) => {
                if let HostEvent::Input { text, images, .. } = *ev
                    && (text != orig_text || images != orig_images)
                {
                    return InputReduction::Transform { text, images };
                }
                InputReduction::Continue
            }
        }
    }

    /// Dispatch `message_end` (Pi `ExtensionRunner.emitMessageEnd`, runner.ts:770-810; gap-08 #3). A
    /// handler may return a same-role replacement message (a mismatched role is rejected — the
    /// original is kept — inside `apply_patch`, never a panic). Returns `Some(replacement)` iff a
    /// handler changed the message (Pi returns `undefined` when unmodified). This is the mutating seam
    /// the prior doc wrongly listed DONE: `message_end` was notify-only.
    pub async fn emit_message_end(
        &self,
        message: Message,
        cancel: &CancelToken,
    ) -> Option<Message> {
        let orig = message.clone();
        let ev = HostEvent::MessageEnd { message };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Pass(ev) => match *ev {
                // Same-role enforcement (Pi `runner.ts:796-803`): a genuine change is accepted only
                // when the replacement preserves the original role; a mismatched-role replacement is
                // rejected and the original kept (never a panic). `Message`'s variant discriminant is
                // exactly its role (User|Assistant|ToolResult), so a discriminant match is the role
                // guard.
                HostEvent::MessageEnd { message }
                    if message != orig
                        && std::mem::discriminant(&message) == std::mem::discriminant(&orig) =>
                {
                    Some(message)
                }
                _ => None,
            },
            _ => None,
        }
    }

    /// Dispatch `before_provider_request` (Pi `ExtensionRunner.emitBeforeProviderRequest`,
    /// runner.ts:946-978; gap-08 #4). Each handler's return value REPLACES the outbound payload
    /// wholesale; later handlers observe the replacement. Returns the final (possibly replaced)
    /// payload to send to the provider.
    pub async fn emit_before_provider_request(
        &self,
        payload: Value,
        cancel: &CancelToken,
    ) -> Value {
        let orig = payload.clone();
        let ev = HostEvent::BeforeProviderRequest { payload };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Pass(ev) => match *ev {
                HostEvent::BeforeProviderRequest { payload } => payload,
                _ => orig,
            },
            _ => orig,
        }
    }

    /// Dispatch `user_bash` (Pi `ExtensionRunner.emitUserBash`, runner.ts:885-912; gap-08 #5). The
    /// FIRST handler that returns a result wins (Pi short-circuits): a block stops the command, a
    /// `handled` result (operations/result) supplies the execution. Returns the reduced
    /// [`UserBashReduction`].
    pub async fn emit_user_bash(&self, command: &str, cancel: &CancelToken) -> UserBashReduction {
        // `exclude_from_context` (the `!!` prefix) is decided by the submission parser at the caller
        // (cross-crate), so it defaults to `false` here; `cwd` is the process working directory (Pi
        // `UserBashEvent.cwd`, types.ts:789). The richer caller-supplied values flow once the
        // submission pipeline threads them into this entry point.
        let cwd = std::env::current_dir()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let ev = HostEvent::UserBash {
            command: command.to_string(),
            exclude_from_context: false,
            cwd,
        };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by, .. } => UserBashReduction::Blocked { reason, by },
            Reduced::Handled(HandledValue(v)) => UserBashReduction::Handled(v),
            Reduced::Pass(_) => UserBashReduction::Continue,
        }
    }

    /// Dispatch `session_before_compact` (Pi `emit("session_before_compact")`,
    /// agent-session.ts:1672-1693; L4 gap #5). The guest sees the computed `preparation` + branch
    /// entries + reason/willRetry and may veto (`block`) or return a compaction override (`mutate`,
    /// Pi `SessionBeforeCompactResult.compaction`). Returns the reduced [`CompactionReduction`] for the
    /// compaction producer to apply (proceed / cancel / thread the override into the entry).
    #[allow(clippy::too_many_arguments)]
    pub async fn emit_session_before_compact(
        &self,
        preparation: Value,
        branch_entries: Value,
        custom_instructions: Option<String>,
        reason: &str,
        will_retry: bool,
        cancel: &CancelToken,
    ) -> CompactionReduction {
        let ev = HostEvent::SessionBeforeCompact {
            preparation,
            branch_entries,
            custom_instructions,
            reason: reason.to_string(),
            will_retry,
            override_result: None,
        };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by, .. } => CompactionReduction::Blocked { reason, by },
            Reduced::Pass(ev) => match *ev {
                HostEvent::SessionBeforeCompact {
                    override_result: Some(v),
                    ..
                } => CompactionReduction::Override(v),
                _ => CompactionReduction::Proceed,
            },
            // `session_before_compact` has no `handled` channel (Pi returns only cancel/compaction).
            Reduced::Handled(_) => CompactionReduction::Proceed,
        }
    }

    /// Dispatch `session_before_tree` (Pi `emit("session_before_tree")`, agent-session.ts:2752-2783;
    /// L4 gap #5). The guest sees the computed `preparation` (`TreePreparation`) and may veto or return
    /// a summary/customInstructions/label override. Returns the reduced [`TreeReduction`].
    pub async fn emit_session_before_tree(
        &self,
        preparation: Value,
        cancel: &CancelToken,
    ) -> TreeReduction {
        let ev = HostEvent::SessionBeforeTree {
            preparation,
            override_result: None,
        };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by, .. } => TreeReduction::Blocked { reason, by },
            Reduced::Pass(ev) => match *ev {
                HostEvent::SessionBeforeTree {
                    override_result: Some(v),
                    ..
                } => TreeReduction::Override(v),
                _ => TreeReduction::Proceed,
            },
            Reduced::Handled(_) => TreeReduction::Proceed,
        }
    }

    /// Render a TOOL CALL through the extension that registered a renderer for that tool (Pi's
    /// per-tool `ToolDefinition.renderCall`, extensions/types.ts:489, resolved by
    /// `modes/interactive/components/tool-execution.ts:81-112` — the extension's definition is
    /// preferred over the built-in). `None` = no extension renders this tool (draw the standard
    /// shell), which is also what a faulting renderer degrades to.
    ///
    /// This is the missing link EXT-006 was about: `ToolDescriptor.has_renderer` was recorded and
    /// `LiveExtension::render_call` existed, but nothing could get from a tool NAME to the guest
    /// that renders it, so both were dead outside a unit test.
    pub async fn render_tool_call(&self, tool_name: &str, call: &Value) -> Option<Value> {
        self.render_tool_call_outcome(tool_name, call)
            .await
            .into_option()
    }

    /// Register a tool that supplies its own `render_call`/`render_result` — the SDK half of
    /// upstream's renderer map. See [`Self::native_tool_renderers`].
    ///
    /// Called by `SessionBuilder` for every configured custom tool, mirroring upstream spreading
    /// `this._customTools` into `allCustomTools` (`core/agent-session.ts:2474-2477` @v0.84.2).
    /// Registering is unconditional and cheap: `Tool::render_call`/`render_result` default to
    /// `None`, and a tool that takes both defaults resolves to [`RenderOutcome::None`], i.e. the
    /// built-in shell — exactly as a definition without a `renderCall` does upstream.
    pub fn register_native_tool_renderer(&self, tool: Arc<dyn Tool>) {
        if let Ok(mut g) = self.native_tool_renderers.write() {
            g.insert(tool.name().to_string(), tool);
        }
    }

    /// The registered native tool for `tool_name`, if any.
    fn native_tool_renderer(&self, tool_name: &str) -> Option<Arc<dyn Tool>> {
        self.native_tool_renderers
            .read()
            .ok()
            .and_then(|g| g.get(tool_name).cloned())
    }

    /// [`Self::render_tool_call`] keeping the FAULT distinct from "no renderer" — see
    /// [`RenderOutcome`].
    ///
    /// Tiers, in upstream's order (`tool-execution.ts:84-91`; see
    /// [`Self::native_tool_renderers`] for why the SDK tier is consulted FIRST): the tool's own
    /// `render_call`, then the extension that registered a renderer for this name. `None` from
    /// both leaves the caller to draw the built-in shell.
    pub async fn render_tool_call_outcome(&self, tool_name: &str, call: &Value) -> RenderOutcome {
        if let Some(tool) = self.native_tool_renderer(tool_name)
            && let Some(text) = tool.render_call(call)
        {
            return RenderOutcome::Rendered(Value::String(text));
        }
        let Some(owner) = self.registry.tool_renderer_owner(tool_name).ok().flatten() else {
            return RenderOutcome::None;
        };
        self.render_via(&owner, tool_name, call, RenderKind::Call)
            .await
    }

    /// Render a TOOL RESULT through the tool's registered renderer (Pi `renderResult`,
    /// extensions/types.ts:492-497). See [`Self::render_tool_call`].
    pub async fn render_tool_result(&self, tool_name: &str, result: &Value) -> Option<Value> {
        self.render_tool_result_outcome(tool_name, result)
            .await
            .into_option()
    }

    /// [`Self::render_tool_result`] keeping the FAULT distinct from "no renderer" — see
    /// [`RenderOutcome`].
    pub async fn render_tool_result_outcome(
        &self,
        tool_name: &str,
        result: &Value,
    ) -> RenderOutcome {
        if let Some(tool) = self.native_tool_renderer(tool_name)
            && let Some(text) = tool.render_result(result)
        {
            return RenderOutcome::Rendered(Value::String(text));
        }
        let Some(owner) = self.registry.tool_renderer_owner(tool_name).ok().flatten() else {
            return RenderOutcome::None;
        };
        self.render_via(&owner, tool_name, result, RenderKind::Result)
            .await
    }

    /// Render a CUSTOM MESSAGE through the extension that registered a renderer for `custom_type`
    /// (Pi `registerMessageRenderer` + `getMessageRenderer`, extensions/types.ts:1284 /
    /// runner.ts:579-587; first extension in load order wins). `None` = no renderer, so the host
    /// falls back to its own labeled-message framing.
    ///
    /// CYRUP-DELTA: Pi keeps two distinct renderer surfaces — the per-tool `renderCall`/
    /// `renderResult` above and this per-custom-type `MessageRenderer`. cyrup's WIT exposes ONE
    /// pair of guest exports (`render-call`/`render-result`, keyed by an opaque `custom-type`), so
    /// both surfaces route through them; the two are kept apart by their REGISTRY tables
    /// (`tool_renderer_owner` vs `message_renderer_owner`), not by the wire shape.
    pub async fn render_message_call(&self, custom_type: &str, message: &Value) -> Option<Value> {
        self.render_message_call_outcome(custom_type, message)
            .await
            .into_option()
    }

    /// [`Self::render_message_call`] keeping the FAULT distinct from "no renderer" — see
    /// [`RenderOutcome`].
    ///
    /// The MESSAGE surface's own consumer collapses the two again on purpose
    /// (`custom-message.ts:82-84` catches and falls through to the default box); the distinction is
    /// preserved HERE so it survives to the ENTRY surface, which does not
    /// ([`Self::render_entry`]).
    pub async fn render_message_call_outcome(
        &self,
        custom_type: &str,
        message: &Value,
    ) -> RenderOutcome {
        let Some(owner) = self
            .registry
            .message_renderer_owner(custom_type)
            .ok()
            .flatten()
        else {
            return RenderOutcome::None;
        };
        self.render_via(&owner, custom_type, message, RenderKind::Call)
            .await
    }

    /// The result-side companion of [`Self::render_message_call`].
    pub async fn render_message_result(&self, custom_type: &str, message: &Value) -> Option<Value> {
        self.render_message_result_outcome(custom_type, message)
            .await
            .into_option()
    }

    /// [`Self::render_message_result`] keeping the FAULT distinct from "no renderer" — see
    /// [`RenderOutcome`].
    pub async fn render_message_result_outcome(
        &self,
        custom_type: &str,
        message: &Value,
    ) -> RenderOutcome {
        let Some(owner) = self
            .registry
            .message_renderer_owner(custom_type)
            .ok()
            .flatten()
        else {
            return RenderOutcome::None;
        };
        self.render_via(&owner, custom_type, message, RenderKind::Result)
            .await
    }

    /// Render a custom ENTRY through the extension that registered an ENTRY renderer for
    /// `custom_type` (Pi `getEntryRenderer(entry.customType)`, `extensions/runner.ts:593-600`,
    /// resolved by `interactive-mode.ts:3431-3436 addCustomEntryToChat`).
    ///
    /// X15 — this is the surface whose FAULT is user-visible, and the reason [`RenderOutcome`]
    /// exists. Upstream `CustomEntryComponent.rebuild` (`custom-entry.ts:40-52`) has three distinct
    /// outcomes and they draw three different things:
    ///
    /// | upstream                                    | here                     | drawn |
    /// |---------------------------------------------|--------------------------|-------|
    /// | `getEntryRenderer(...) === undefined` (:3433) | [`RenderOutcome::None`]  | nothing |
    /// | renderer returned `undefined` (:54-56 / :3438) | [`RenderOutcome::None`] | nothing |
    /// | renderer returned a `Component` (:58-60)     | [`RenderOutcome::Rendered`] | `Spacer(1)` + the component |
    /// | renderer THREW (:47-52)                      | [`RenderOutcome::Failed`] | `Spacer(1)` + a `customMessageBg` box holding `[type] renderer failed: {message}` |
    ///
    /// The first two collapse because upstream draws nothing for both; the THIRD must not, and
    /// before this existed it did — `render_via` reported a faulting renderer as `None` and the
    /// failure box had no producer anywhere in `crates/`.
    ///
    /// CYRUP-DELTA (wire): there is no `render-entry` guest export. cyrup's WIT deliberately keeps
    /// ONE renderer pair (`render-call`/`render-result`) keyed by an opaque `custom-type` and tells
    /// the surfaces apart by their REGISTRY table (see [`Self::render_message_call`]); an entry
    /// therefore travels over `render-call`. Adding a fourth export would break every already-built
    /// guest component for no behavioural gain. A NATIVE owner has no such constraint and gets its
    /// own [`crate::NativeExtension::render_entry`] hook.
    pub async fn render_entry(&self, custom_type: &str, entry: &Value) -> RenderOutcome {
        let Some(owner) = self
            .registry
            .entry_renderer_owner(custom_type)
            .ok()
            .flatten()
        else {
            return RenderOutcome::None;
        };
        self.render_via(&owner, custom_type, entry, RenderKind::Entry)
            .await
    }

    /// Fold every registered markdown transformer over `markdown`, in extension LOAD ORDER
    /// (EXT-019).
    ///
    /// pi: `getMarkdownTransformers(): this.extensions.flatMap(ext => ext.markdownTransformer ? [..]
    /// : [])` (`pi/packages/coding-agent/src/core/extensions/runner.ts:589-591` @v0.84.1) — a
    /// POST-BASELINE addition, absent at the ported v0.83.0 — with the transformer typed
    /// `(markdown: string, context: MarkdownTransformContext) => string` (`types.ts:1153`). Each
    /// transformer's output is the next one's input, which is why this is a fold and not a
    /// first-wins lookup like the renderer tables.
    ///
    /// `message_type` is one of `"user"` / `"assistant"` / `"assistant-thinking"`; `is_streaming`
    /// and `available_width` are the other two `MarkdownTransformContext` fields
    /// (`types.ts:1147-1151`). A faulting transformer is CONTAINED and SKIPPED — its input passes
    /// through unchanged, so a broken extension can never blank a line of transcript.
    pub async fn transform_markdown(
        &self,
        markdown: &str,
        message_type: &str,
        is_streaming: bool,
        available_width: u32,
    ) -> String {
        let owners = self
            .registry
            .markdown_transformer_owners()
            .unwrap_or_default();
        if owners.is_empty() {
            return markdown.to_string();
        }
        let ctx = serde_json::json!({
            "messageType": message_type,
            "isStreaming": is_streaming,
            "availableWidth": available_width,
        });
        let mut current = markdown.to_string();
        for owner in owners {
            match self.transform_markdown_via(&owner, &current, &ctx).await {
                Ok(next) => current = next,
                Err(e) => {
                    tracing::warn!(
                        extension = %owner, error = %e,
                        "markdown transformer contained (skipped; text passes through unchanged)"
                    );
                }
            }
        }
        current
    }

    /// Offer one raw terminal-input chunk to every subscribed extension, in LOAD order, and say
    /// what the caller should do with it (EXT-021).
    ///
    /// 1:1 with pi's `TUI.handleInput` listener fold (`packages/tui/src/tui.ts:773-788` @v0.83.0),
    /// clause for clause:
    /// * each listener sees the CURRENT — possibly already rewritten — data, not the original;
    /// * `result?.consume` truthy STOPS the fold and drops the keystroke (`:777-779`);
    /// * `result?.data !== undefined` replaces the buffer for the listeners after it (`:780-782`);
    /// * a fold that ends with an EMPTY string also drops the keystroke (`:784-786`).
    ///
    /// With no subscribers this is the identity — upstream guards the whole block on
    /// `inputListeners.size > 0` (`:773`) — so the ordinary keystroke path costs one `Vec` read.
    ///
    /// A faulting or panicking extension is CONTAINED and treated as `undefined`. That direction
    /// is load-bearing rather than merely tidy: the alternative (fail closed) would let one broken
    /// extension swallow the user's keyboard with no way to type the command that unloads it.
    pub async fn terminal_input(&self, data: &str) -> TerminalInputDecision {
        let owners = self
            .registry
            .terminal_input_subscribers()
            .unwrap_or_default();
        if owners.is_empty() {
            return TerminalInputDecision::Deliver(data.to_string());
        }
        let mut current = data.to_string();
        for owner in owners {
            let result = self.terminal_input_via(&owner, &current).await;
            let Some(result) = result else { continue };
            if result.consume.unwrap_or(false) {
                return TerminalInputDecision::Consume;
            }
            if let Some(next) = result.data {
                current = next;
            }
        }
        if current.is_empty() {
            return TerminalInputDecision::Consume;
        }
        TerminalInputDecision::Deliver(current)
    }

    /// One terminal-input handler invocation, whichever tier its owner lives in. A fault is
    /// contained as `None` — upstream's `undefined` — for the reason stated on
    /// [`Self::terminal_input`].
    async fn terminal_input_via(
        &self,
        owner: &ExtensionId,
        data: &str,
    ) -> Option<TerminalInputResult> {
        if let Some(native) = self.native.read().ok().and_then(|g| g.get(owner).cloned()) {
            return match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                native.on_terminal_input(data)
            })) {
                Ok(v) => v,
                Err(panic) => {
                    tracing::warn!(
                        extension = %owner, error = %native_panic_msg(panic),
                        "native terminal-input handler panicked (input passes through untouched)"
                    );
                    None
                }
            };
        }
        #[cfg(feature = "wasm-host")]
        {
            let ext = self.live.read().ok().and_then(|g| g.get(owner).cloned())?;
            return match ext.on_terminal_input(data).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(
                        extension = %owner, error = %e,
                        "terminal-input handler contained (input passes through untouched)"
                    );
                    None
                }
            };
        }
        #[cfg(not(feature = "wasm-host"))]
        None
    }

    /// One transformer invocation, whichever tier its owner lives in. A native's panic is caught
    /// for the same reason a native renderer's is (R-08-036): a presentation hook must never take
    /// the frame down.
    async fn transform_markdown_via(
        &self,
        owner: &ExtensionId,
        markdown: &str,
        ctx: &Value,
    ) -> Result<String, ExtError> {
        if let Some(native) = self.native.read().ok().and_then(|g| g.get(owner).cloned()) {
            return std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                native.transform_markdown(markdown, ctx)
            }))
            .map_err(|panic| ExtError::Panicked(native_panic_msg(panic)));
        }
        #[cfg(feature = "wasm-host")]
        {
            let Some(ext) = self.live.read().ok().and_then(|g| g.get(owner).cloned()) else {
                // Recorded owner with no live instance: nothing threw, nothing to call.
                return Ok(markdown.to_string());
            };
            return ext.transform_markdown(markdown, ctx).await;
        }
        #[cfg(not(feature = "wasm-host"))]
        Ok(markdown.to_string())
    }

    /// Invoke the owner's renderer, containing faults LOCALLY (`warn!` + [`RenderOutcome::Failed`])
    /// the way [`Self::deliver_bus_events`] does. Deliberately NOT routed through
    /// `dispatch_block_mutate`: a renderer is a presentation concern and a faulting one must never
    /// be able to block the tool call it was asked to draw (R-08-036).
    ///
    /// X15 — containment used to mean `warn!` + `None`, and `None` is ALSO what "no extension
    /// registered a renderer for this key" returns, so "the renderer threw" and "there is no
    /// renderer" were indistinguishable to every caller. Upstream never conflates them: the throw
    /// is caught at the COMPONENT (`custom-entry.ts:47-52` / `custom-message.ts:82-84`), which
    /// still knows which of the two it is looking at. The fault now travels as its own variant and
    /// each surface decides what to draw for it; the `warn!` is unchanged, since a faulting
    /// renderer is still an extension bug worth logging once per render.
    ///
    /// NATIVE owners are tried first and are available in EVERY build (a native renderer needs no
    /// wasm host); a guest owner resolves against the live instance map only under `wasm-host`.
    async fn render_via(
        &self,
        owner: &ExtensionId,
        key: &str,
        payload: &Value,
        kind: RenderKind,
    ) -> RenderOutcome {
        if let Some(native) = self.native.read().ok().and_then(|g| g.get(owner).cloned()) {
            // A panicking native renderer must degrade gracefully, never take the frame down with
            // it (R-08-036) — the same containment the guest arm gets below. `catch_unwind` IS the
            // native analog of upstream's `try`/`catch`, so its `Err` is the `throw` of
            // `custom-entry.ts:47` and carries the same payload: the panic message.
            // The LIVE-component tier is consulted first, exactly as the native tool-renderer fast
            // tier is consulted before this dispatch. `None` falls through to the string hooks
            // below — upstream's `return undefined` for a payload it cannot draw from.
            let live = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                native.render_live(key, payload)
            }));
            match live {
                Ok(Some(component)) => return RenderOutcome::Live(component),
                Ok(None) => {}
                Err(panic) => {
                    let message = native_panic_msg(panic);
                    tracing::warn!(
                        extension = %owner, key = %key, error = %message,
                        "native live renderer panicked (contained; the surface decides how to degrade)"
                    );
                    return RenderOutcome::Failed(message);
                }
            }
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match kind {
                RenderKind::Call => native.render_call(key, payload),
                RenderKind::Result => native.render_result(key, payload),
                RenderKind::Entry => native.render_entry(key, payload),
            }));
            return match rendered {
                Ok(v) => RenderOutcome::from_option(v),
                Err(panic) => {
                    let message = native_panic_msg(panic);
                    tracing::warn!(
                        extension = %owner, key = %key, error = %message,
                        "native renderer panicked (the surface decides: default framing for a \
                         message/tool row, a failure box for a custom entry)"
                    );
                    RenderOutcome::Failed(message)
                }
            };
        }
        self.render_via_guest(owner, key, payload, kind).await
    }

    #[cfg(feature = "wasm-host")]
    async fn render_via_guest(
        &self,
        owner: &ExtensionId,
        key: &str,
        payload: &Value,
        kind: RenderKind,
    ) -> RenderOutcome {
        let Some(ext) = self.live.read().ok().and_then(|g| g.get(owner).cloned()) else {
            // The owner is recorded but has no live instance (unloaded mid-render, or a native-only
            // build). Nothing threw — there is simply nothing to call.
            return RenderOutcome::None;
        };
        // CYRUP-DELTA: an ENTRY rides the `render-call` export — see [`Self::render_entry`] for why
        // the world deliberately has no fourth renderer export.
        let out = match kind {
            RenderKind::Call | RenderKind::Entry => ext.render_call(key, payload).await,
            RenderKind::Result => ext.render_result(key, payload).await,
        };
        match out {
            Ok(v) => RenderOutcome::from_option(v),
            Err(e) => {
                let message = e.to_string();
                tracing::warn!(
                    extension = %owner, key = %key, error = %message,
                    "extension renderer fault contained (the surface decides: default framing for \
                     a message/tool row, a failure box for a custom entry)"
                );
                RenderOutcome::Failed(message)
            }
        }
    }

    /// Native-only build: no live guest can hold a renderer, so a guest-owned key draws with the
    /// host's own framing. (A NATIVE-owned key is still rendered — see [`Self::render_via`].)
    /// [`RenderOutcome::None`], not `Failed`: nothing threw, the runtime simply is not there.
    #[cfg(not(feature = "wasm-host"))]
    async fn render_via_guest(
        &self,
        _owner: &ExtensionId,
        _key: &str,
        _payload: &Value,
        _kind: RenderKind,
    ) -> RenderOutcome {
        RenderOutcome::None
    }

    /// Whether anything outside the built-in table can render this tool name (Pi
    /// `hasRendererDefinition`, tool-execution.ts:104-106) — the cheap check a UI makes before
    /// paying for a guest round trip.
    ///
    /// Covers BOTH non-built-in tiers, because upstream's single `getToolDefinition(name)` lookup
    /// does: an extension-registered renderer, or a tool that carries its own
    /// (see [`Self::native_tool_renderers`]). Answering only for the extension tier is what kept
    /// `Tool::render_call` unreachable — `cyrup_tui::app::extension_render` early-returns on a
    /// `false` here, so a custom tool's renderer was never invoked even after the resolver knew
    /// how to call it.
    ///
    /// COARSE on the native tier, and deliberately: whether `Tool::render_call` will return
    /// `Some` cannot be known without calling it, so any registered custom tool answers `true` and
    /// a tool that overrides neither method costs one resolution returning
    /// [`RenderOutcome::None`]. Upstream's gate is coarser still — `hasRendererDefinition()` is
    /// `builtInToolDefinition !== undefined || toolDefinition !== undefined`
    /// (`tool-execution.ts:104-106`), true for every tool that has a definition at all.
    pub fn has_tool_renderer(&self, tool_name: &str) -> bool {
        if self
            .native_tool_renderers
            .read()
            .is_ok_and(|g| g.contains_key(tool_name))
        {
            return true;
        }
        self.registry
            .tool_renderer_owner(tool_name)
            .ok()
            .flatten()
            .is_some()
    }

    /// Whether ANY extension registered a custom-message renderer for `custom_type` (Pi
    /// `getMessageRenderer(...) !== undefined`, runner.ts:579-587).
    pub fn has_message_renderer(&self, custom_type: &str) -> bool {
        self.registry
            .message_renderer_owner(custom_type)
            .ok()
            .flatten()
            .is_some()
    }

    /// Whether ANY extension registered a custom-ENTRY renderer for `custom_type` (Pi
    /// `getEntryRenderer(...) !== undefined`, runner.ts:593-600 — the `if (!renderer) return;`
    /// early-out of `addCustomEntryToChat`, interactive-mode.ts:3432-3435).
    pub fn has_entry_renderer(&self, custom_type: &str) -> bool {
        self.registry
            .entry_renderer_owner(custom_type)
            .ok()
            .flatten()
            .is_some()
    }

    /// Whether ANY extension registered a markdown transformer — the sync pre-check twin of the
    /// `has_*_renderer` trio above, for [`Self::transform_markdown`].
    ///
    /// [`Self::transform_markdown`] already early-returns on an empty owner list, so this answers
    /// nothing the fold could not; what it buys is the *shape* of the call. The consumer
    /// (`cyrup_tui::app::App::apply_markdown_transformers`) runs on the streaming path, once per
    /// delta, and reaching the fold means an `async` hop plus a cloned owner list per chunk. Gating
    /// on this makes the no-extension path one rwlock read and keeps the rendered lines
    /// byte-identical to a build with no host at all.
    pub fn has_markdown_transformers(&self) -> bool {
        self.registry.has_markdown_transformers().unwrap_or(false)
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    pub fn dispatcher(&self) -> &Dispatcher {
        &self.dispatcher
    }

    /// Apply CLI-captured extension-flag overrides (Pi `applyExtensionFlagValues`,
    /// agent-session-services.ts:84-125). Called AFTER extensions load (so their `registerFlag`
    /// specs are known). For each captured `(name, value)`:
    /// - no loaded extension registered `name` ⇒ collected into ONE `Unknown option(s): --a, --b`
    ///   error (Pi :100-103,:118-124);
    /// - the registered flag's `type` is `boolean` ⇒ the stored value is `true` regardless of the
    ///   token's value (Pi :105-108);
    /// - the registered flag's `type` is `string` and the token carried a value ⇒ that string (Pi
    ///   :109-112);
    /// - a bare `--flag` on a string-typed flag ⇒ an `Extension flag "--foo" requires a value` error
    ///   and the registered default stands (Pi :113-116).
    ///
    /// The resolved value lands in the shared flag store so a guest's `getFlag(name)` reads the CLI
    /// value AHEAD of its registered default (gap-08 §5.6 — the step that was missing, so the
    /// 1:1-ported CLI capture used to be dropped one call short of `getFlag`).
    ///
    /// SEAM-S01: the two error classes used to be a bare `continue`, so a mistyped `--flag` was
    /// swallowed with no message and no exit code. They are returned here in Pi's exact order —
    /// every per-flag "requires a value" in iteration order, then the single aggregated "Unknown
    /// option(s)" last — for the caller to surface (the bin reports them and exits 1, Pi
    /// main.ts:843-848).
    pub fn apply_extension_flag_values(
        &self,
        flags: &[(String, ExtensionFlagOverride)],
    ) -> Result<Vec<String>, ExtError> {
        let mut diagnostics: Vec<String> = Vec::new();
        let mut unknown: Vec<String> = Vec::new();
        for (name, value) in flags {
            let spec = match self.registry.get_flag(name)? {
                Some(s) => s,
                // Unregistered flag: no extension owns it (Pi `unknownFlags.push(name)`, :101-102).
                None => {
                    unknown.push(name.clone());
                    continue;
                }
            };
            let is_boolean = spec.get("type").and_then(|t| t.as_str()) == Some("boolean");
            let resolved = match (is_boolean, value) {
                (true, _) => Value::Bool(true),
                (false, ExtensionFlagOverride::Str(s)) => Value::String(s.clone()),
                // A string-typed flag passed with no value: Pi emits "requires a value" and does not
                // set it, so the registered default stands (:113-116).
                (false, ExtensionFlagOverride::Bool(_)) => {
                    diagnostics.push(format!("Extension flag \"--{name}\" requires a value"));
                    continue;
                }
            };
            self.registry.set_flag_value(name.clone(), resolved)?;
        }
        if !unknown.is_empty() {
            // Pi pluralizes on the COUNT and joins with ", " (:120-123).
            let plural = if unknown.len() == 1 { "" } else { "s" };
            let names: Vec<String> = unknown.iter().map(|n| format!("--{n}")).collect();
            diagnostics.push(format!("Unknown option{plural}: {}", names.join(", ")));
        }
        Ok(diagnostics)
    }

    /// Fan out every queued inter-extension bus event to its subscribers.
    ///
    /// Upstream this is not a function at all: `createEventBus()` returns `emit: (channel, data)
    /// => { emitter.emit(channel, data); }` over a node `EventEmitter`, so every listener runs
    /// synchronously at the emit call (`pi/packages/coding-agent/src/core/event-bus.ts:12-32`
    /// @v0.83.0) — no queue, no drain point, nothing that can go undelivered. cyrup MUST defer,
    /// because a guest emitting from inside its own `bus.emit` import already holds its
    /// single-instance store and delivering would re-enter it; this is the drain.
    ///
    /// Three defects closed here:
    ///
    /// * **EXT-018** — delivery now reaches NATIVE extensions as well as wasm guests. pi hangs the
    ///   one bus on the one `ExtensionAPI` it builds for every extension it loads (`events:
    ///   eventBus,`, `extensions/loader.ts:389` @v0.83.0); cyrup had it inside the `wasm-host`
    ///   gate, resolving subscribers out of `self.live` only, so the three extensions that
    ///   actually ship — permission-system, intercom, subagents, all natives — could not use the
    ///   channel built for exactly their coordination.
    /// * **EXT-057a** — reaching `MAX_ROUNDS` with work still queued used to fall out of the loop
    ///   with events sitting in `SharedBus.pending`: no diagnostic, no error, no record. It now
    ///   drops the remainder EXPLICITLY and reports one [`crate::ExtensionError`] through the same
    ///   `add_error_listener` channel `App::show_extension_error` drains.
    /// * **EXT-057b** — a faulting `bus-deliver` was `tracing::warn!` only, so it never reached the
    ///   `[Extension issues]` surface EXT-S03 exists to make faults visible in. pi's own `on`
    ///   wrapper surfaces handler faults (`catch (err) { console.error(...) }`); this now does too,
    ///   keeping the `tracing::warn!` as well.
    /// * **EXT-034** — the drain used to be wired only into the command tier (`run_command` /
    ///   `run_shortcut`), so `pi.events` silently worked from a slash-command handler and silently
    ///   did NOT work from an event handler, which is where cross-extension coordination actually
    ///   happens. The body now lives on [`BusFanout`], which [`Dispatcher`] holds, and every
    ///   dispatch entry point drains after its subscriber loop. This method stays as the explicit
    ///   host-tier drain and is a no-op when a drain is already running.
    pub async fn deliver_bus_events(&self, cancel: &CancelToken) {
        use crate::bus::BusDrain;
        self.fanout.drain_bus(cancel).await;
    }

    /// The host-owned inter-extension bus (pi's single `createEventBus()`,
    /// `core/event-bus.ts:12-32` @v0.83.0). Exposed so a NATIVE built-in can emit on it without a
    /// wasm boundary — the guest tier reaches the same object through the `bus.emit`/`subscribe`
    /// imports (EXT-018).
    pub fn bus(&self) -> &Arc<crate::bus::SharedBus> {
        &self.bus
    }

    /// Register a listener for contained extension faults (Pi `onError`, R-08-036). The listener
    /// receives a typed [`crate::ExtensionError`] (`{extension, event, error}`) each time a guest
    /// handler fault is contained and skipped.
    pub fn add_error_listener(&self, listener: crate::ErrorListener) {
        self.dispatcher.add_error_listener(listener);
    }

    /// Ids of loaded extensions in load order.
    pub fn loaded_ids(&self) -> Vec<ExtensionId> {
        self.loaded.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Build a host with the Wasmtime runtime spun up (engine + instance pool + epoch driver).
    /// Must be called from within a tokio runtime. Behind the `wasm-host` feature (arch-08 §2).
    #[cfg(feature = "wasm-host")]
    pub fn with_wasm(config: HostConfig) -> Result<Self, ExtError> {
        let mut host = Self::new(config);
        host.wasm = Some(crate::host_runtime::WasmRuntime::new()?);
        Ok(host)
    }

    /// The Wasmtime runtime bundle, if this host was built with [`Self::with_wasm`].
    #[cfg(feature = "wasm-host")]
    pub fn wasm(&self) -> Option<&crate::host_runtime::WasmRuntime> {
        self.wasm.as_ref()
    }

    /// Load a real `.wasm` extension COMPONENT end-to-end (arch-08b, R-08-001): instantiate it,
    /// link the host imports, run its `init` export (which registers tools/commands and declares
    /// subscriptions), and wire the live instance into the dispatcher in load order. The `services`
    /// backend supplies interactive capabilities (default: [`crate::host::DenyServices`] = deny-all).
    /// Returns the live handle so callers can observe its host-side effects.
    ///
    /// # Capabilities (EXT-054)
    ///
    /// This is the MANIFEST-LESS entry point: the caller is the host itself and has already made
    /// the decision an `extension.json` would express, so the grant applied is
    /// [`Capabilities::host_granted`] — `exec`/`net`/`ui` on, `fs` still empty (`ext-fs` has no
    /// root to resolve against without a declared grant). A DISCOVERED extension never comes
    /// through here: [`Self::load_discovered`] resolves its manifest and calls
    /// [`Self::load_wasm_with_caps`], so `capabilities.{fs,exec,net,ui}` really do narrow it.
    ///
    /// The full grant is what PARITY requires of an embedder-supplied extension, not merely what
    /// cyrup happens to do. Pi's embedder seam is `loadExtensionFromFactory`
    /// (`packages/coding-agent/src/core/extensions/loader.ts:485-498` @v0.83.0) and
    /// `DefaultResourceLoader`'s `extensionFactories` / `additionalExtensionPaths`
    /// (`examples/sdk/06-extensions.ts` @v0.83.0): both build the extension straight from the
    /// caller's own code and hand it the complete `ExtensionAPI` — Pi has no capability model at
    /// all, so an embedder-supplied extension is unconditionally total. An embedder that wants LESS
    /// than that is asking for something Pi cannot express, and [`Self::load_wasm_with_caps`] is
    /// the seam for it; narrowing THIS function would diverge from Pi and silently break callers.
    #[cfg(feature = "wasm-host")]
    pub async fn load_wasm(
        &self,
        id: ExtensionId,
        bytes: &[u8],
        services: Arc<dyn crate::host::HostServices>,
    ) -> Result<Arc<crate::host::LiveExtension>, ExtError> {
        self.load_wasm_with_caps(id, bytes, services, &Capabilities::host_granted())
            .await
    }

    /// [`Self::load_wasm`] under an explicit capability grant — the seam EXT-054 was missing.
    ///
    /// The grant crosses into instantiation as **data** (`GuestState::with_capabilities`) and is
    /// enforced **host-side** at the import boundary in `crates/cyrup-ext/src/host/live.rs`; the
    /// guest is handed no reference to it and no import that could change it. That split is
    /// ADR-0002's batch-17 instruction, and it is why a session that injects a fully-capable
    /// `LiveHostServices` still cannot let a `{"exec": false}` guest run a process.
    ///
    /// `capabilities.fs` roots resolve against [`HostConfig::cwd`] — the project the host is
    /// running in — so the manifest's own example `["read:.", "write:.cyrup/todo"]` means what it
    /// reads as. A malformed grant fails the load ([`ExtError::Capability`]) instead of being
    /// dropped.
    #[cfg(feature = "wasm-host")]
    pub async fn load_wasm_with_caps(
        &self,
        id: ExtensionId,
        bytes: &[u8],
        services: Arc<dyn crate::host::HostServices>,
        caps: &Capabilities,
    ) -> Result<Arc<crate::host::LiveExtension>, ExtError> {
        let wasm = self.wasm.as_ref().ok_or(ExtError::WasmHostDisabled)?;
        let fs_grants = caps.parse_fs_grants()?;
        self.reserve_id(&id)?;
        let guest = Arc::new(
            crate::host::GuestState::with_services(id.clone(), self.registry.clone(), services)
                // Wire the guest onto the HOST-OWNED shared bus (not a fresh per-guest one) so its
                // `bus.subscribe`/`bus.emit` reach other guests (Pi's single shared EventBus,
                // gap-08 §5.3).
                .with_bus(self.bus.clone())
                // EXT-054: the declared grant, seeded BEFORE `init` so the guest's very first call
                // — `init` itself registers tools and can already reach `ui`/`exec` — runs under
                // the restriction its manifest declared.
                .with_capabilities(caps.clone(), &fs_grants, &self.config.cwd)
                // Pi `ctx.mode` / `ctx.hasUI` (extensions/types.ts:311,313) are host configuration,
                // not session state: copy them in from the SAME [`HostConfig`] the native path
                // hands to `HostCtx::event`/`::command` above, so a WASM guest's `ctx.mode()` and a
                // built-in's `ctx.mode` cannot disagree about the mode the host is running in.
                .with_host_mode(
                    self.config.mode,
                    self.config.has_ui,
                    self.config.cwd.clone(),
                )
                // EXT-052: the `before_provider_request`/`after_provider_response` reductions a
                // guest provider's `streamSimple` MUST invoke (pi extensions/types.ts:1452-1457
                // @v0.84.1). Installed before `init` so a provider registered during `init` is
                // never the one route whose requests are invisible.
                .with_provider_reduction(Arc::new(DispatcherProviderReduction {
                    dispatcher: self.dispatcher.clone(),
                })),
        );
        let ext = crate::host::LiveExtension::load(
            wasm.engine(),
            id.clone(),
            bytes,
            crate::host::StoreLimits::default(),
            guest,
            Self::WASM_EPOCH_BUDGET_TICKS,
        )
        .await?;
        let ext = Arc::new(ext);
        self.dispatcher.add(ext.clone())?;
        if let Ok(mut g) = self.live.write() {
            g.insert(id, ext.clone());
        }
        // Surface the guest's registered tools as executable handles in the active set: each runs
        // by dispatching `execute-tool` back into ITS OWN live instance (R-08-012/014/015). Done
        // through the shared re-materializer (EXT-004) so an `init`-time registration and a later
        // one take exactly the same path — and so a descriptor is bound to its OWNING instance
        // rather than to whichever extension happened to load last.
        self.materialize_guest_tools()?;
        Ok(ext)
    }

    /// Every tool/flag name collision between two DIFFERENT loaded extensions, in load order (Pi
    /// `ResourceLoader.detectExtensionConflicts`, resource-loader.ts:1059-1094). The FIRST extension
    /// to claim a name keeps it; each rejected claim is one record here.
    ///
    /// [`Self::discover_and_load`] folds these into its returned
    /// [`LoadExtensionsResult::errors`](crate::LoadExtensionsResult) — Pi's
    /// `addExtensionConflictDiagnostics` does exactly that (`resource-loader.ts:625-632`) — so a
    /// collision reaches the session's startup diagnostics without any caller opting in. Call this
    /// directly when conflicts are needed outside that path (e.g. a native-only host).
    pub fn extension_conflicts(&self) -> Vec<crate::ExtensionConflict> {
        self.registry.conflicts().unwrap_or_default()
    }

    /// Discover extensions across the three roots (Pi `discoverAndLoadExtensions`). Pure filesystem
    /// scan; no wasm runtime required. See [`crate::loader::discover`].
    pub fn discover(&self, roots: &DiscoveryRoots) -> Vec<DiscoveredExtension> {
        crate::loader::discover(roots)
    }

    /// [`Self::discover`], additionally returning the non-fatal diagnostics the scan produced (an
    /// `extension.json` that exists but does not parse). [`Self::discover_and_load`] folds these
    /// into its `errors` for you; call this directly only when discovering without loading.
    pub fn discover_with_diagnostics(
        &self,
        roots: &DiscoveryRoots,
    ) -> (Vec<DiscoveredExtension>, Vec<LoadError>) {
        crate::loader::discover_with_diagnostics(roots)
    }

    /// Discover + load every eligible extension across the three roots, returning a
    /// [`LoadExtensionsResult`] of loaded ids + per-path errors (Pi `LoadExtensionsResult`). The
    /// trust split (R-08-002) is applied: global + configured (CLI) extensions are pre-trust;
    /// project-local extensions load only when `project_trusted`. A world-version mismatch, a load
    /// fault, or an untrusted project-local extension is recorded in `errors` (never a panic), and
    /// the loop continues with the next extension. `services` backs the interactive capabilities.
    #[cfg(feature = "wasm-host")]
    pub async fn discover_and_load(
        &self,
        roots: &DiscoveryRoots,
        project_trusted: bool,
        services: Arc<dyn crate::host::HostServices>,
    ) -> LoadExtensionsResult {
        let mut result = LoadExtensionsResult::default();
        // A directory whose `extension.json` exists but does not parse still loads under the
        // manifest-less rule (Pi's `readPiManifest` -> `null` -> `index.ts` fall-through,
        // `loader.ts:568-579,594-624` @v0.83.0), but at a DIFFERENT id and with
        // `Capabilities::none()`. Pi's manifest carries neither, so it can afford to say nothing;
        // cyrup's does, so the operator is told — non-fatally, since Pi does not abort the startup.
        // Reported FIRST: it is the cause of whatever the load loop then reports about that id.
        let (discovered, manifest_diags) = self.discover_with_diagnostics(roots);
        result.errors.extend(manifest_diags);
        for disc in discovered {
            match self
                .load_discovered(&disc, project_trusted, services.clone())
                .await
            {
                Ok(id) => result.loaded.push(id),
                Err(e) => result.errors.push(LoadError {
                    path: disc.dir.clone(),
                    // The trust-gate skip is NOT a load failure (see `LoadError::fatal`).
                    fatal: !matches!(e, ExtError::Untrusted),
                    error: e.to_string(),
                }),
            }
        }
        // Pi `addExtensionConflictDiagnostics` (resource-loader.ts:625-632): AFTER every extension
        // has loaded, the tool/flag name collisions are appended to the SAME `errors` array the load
        // faults use — "Keep all extensions loaded. Conflicts are reported as diagnostics, and
        // precedence is handled by load order." `main.ts:735-738` then renders each as
        // `Failed to load extension "<path>": <message>` and `main.ts:843-848` exits 1, so a
        // collision is FATAL upstream — hence `fatal: true` (only the project-trust skip is not).
        // Native built-ins are loaded before this call by the session builder, so their names are in
        // scope here too, matching Pi's sweep over the whole loaded set (inline extensions included).
        result
            .errors
            .extend(self.extension_conflicts().into_iter().map(|c| LoadError {
                path: PathBuf::from(c.path.as_str()),
                fatal: true,
                error: c.message,
            }));
        result
    }

    /// Load one discovered extension, applying the trust gate then the world-version check
    /// (R-08-002).
    #[cfg(feature = "wasm-host")]
    pub async fn load_discovered(
        &self,
        disc: &DiscoveredExtension,
        project_trusted: bool,
        services: Arc<dyn crate::host::HostServices>,
    ) -> Result<ExtensionId, ExtError> {
        // The trust gate runs FIRST, before anything about the extension is judged.
        //
        // This order is load-bearing, not stylistic. `check_world` used to run above it, so an
        // untrusted project-local extension declaring a stale world returned
        // `ExtError::WorldVersion` instead of `ExtError::Untrusted` — and `discover_and_load`
        // classifies everything but `Untrusted` as `fatal: true` (`:1433`), which
        // `cyrup-session-svc/src/runtime.rs:128-138` turns into a `runtime.diagnostics` error and
        // the bin exits 1. Merely OPENING an untrusted project that happens to contain an
        // out-of-date extension therefore aborted startup — the exact failure `LoadError::fatal`'s
        // own doc says the trust skip exists to avoid, and one pi cannot have: pi filters untrusted
        // project resources out of the enabled set BEFORE `loadExtensions` runs
        // (`resource-loader.ts:379-384`, `setProjectTrusted(false)` + `reload()` @v0.83.0), so an
        // untrusted project-local extension is never inspected at all, let alone diagnosed.
        //
        // The general rule this encodes: an untrusted extension must be examined as little as
        // possible. Every check placed above this line is one more thing an untrusted project's
        // files get to influence.
        if !disc.is_trusted(project_trusted) {
            // Project-local extension in an untrusted project: not loaded (R-ARCH-EXT-017).
            return Err(ExtError::Untrusted);
        }
        disc.manifest.check_world(HOST_WORLD)?;
        let bytes = crate::loader::resolve_component_bytes(disc)?;
        let id = disc.id();
        // EXT-054: the manifest this function has been holding all along now reaches instantiation.
        // Before this line the call was `self.load_wasm(id.clone(), &bytes, services)` — a signature
        // with no manifest parameter — so `disc.manifest.capabilities` was parsed and dropped, and a
        // guest declaring `{"fs": [], "exec": false, "net": false, "ui": false}` received the full
        // host surface (reproduced live: `REPRO-LOG.md`, "EXT-054 — CONFIRMED").
        // SEAM-084 — record the extension's provenance BEFORE its factory runs, because the guest's
        // `register-command` imports fire inside `load_wasm_with_caps` and `get_commands` reads the
        // two back together. This is pi's `createExtension` deriving `{source, baseDir}` from the
        // extension path up front (`core/extensions/loader.ts:433-444` @v0.83.0) — a discovered
        // extension is upstream's `else "local"` branch and `disc.dir` is its `path.dirname(
        // resolvedPath)`, since cyrup discovers a DIRECTORY and resolves the component inside it.
        self.registry.record_extension_provenance(
            id.clone(),
            crate::ExtensionProvenance::local(disc.dir.to_string_lossy().into_owned()),
        )?;
        self.load_wasm_with_caps(id.clone(), &bytes, services, &disc.manifest.capabilities)
            .await?;
        Ok(id)
    }

    /// Execute a guest slash command by name (R-08-016; Pi `ResolvedCommand.handler`). Routes to the
    /// owning live extension and runs its `execute-command` export at command tier. Returns the
    /// command's optional text output.
    #[cfg(feature = "wasm-host")]
    pub async fn run_command(
        &self,
        name: &str,
        args: &str,
        cancel: &CancelToken,
    ) -> Result<Option<String>, ExtError> {
        let (ext, registered) = self.live_for_command(name)?;
        let out = ext.execute_command(&registered, args, cancel).await;
        // Fan out any inter-extension bus events this command emitted (Pi's EventEmitter dispatch
        // runs the listeners after the emit call, event-bus.ts; gap-08 §5.3) — deferred to here
        // because wasm reentrancy forbids delivering inside the guest's `bus.emit` import. Delivery
        // runs even if the command errored (an emit before the error still fires, Pi-faithfully).
        self.deliver_bus_events(cancel).await;
        out
    }

    /// Dynamic argument completions for a registered command (Pi `getArgumentCompletions`,
    /// `core/extensions/types.ts:1166` @v0.83.0), by INVOCATION name.
    ///
    /// Routed native-tier-first and then live-wasm, the same split
    /// [`Self::execute_native_command`] / [`Self::run_command`] make for the handler itself:
    /// upstream a command's completer is a field on the same object as its handler, so the two
    /// must resolve to the same extension or a native command that opts in through
    /// [`crate::native::InitApi::add_autocomplete`] would silently answer "no live owner".
    pub async fn command_completions(
        &self,
        name: &str,
        prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        // SEAM-048 — the registered name goes to the handler, the invocation name is what the user
        // typed; see [`Self::command_route`].
        let native = self.command_route(name)?.and_then(|(owner, registered)| {
            self.native
                .read()
                .ok()
                .and_then(|g| g.get(&owner).cloned())
                .map(|ext| (ext, registered))
        });
        if let Some((ext, registered)) = native {
            return ext.argument_completions(&registered, prefix).await;
        }
        self.wasm_command_completions(name, prefix).await
    }

    /// The live-WASM half of [`Self::command_completions`].
    #[cfg(feature = "wasm-host")]
    async fn wasm_command_completions(
        &self,
        name: &str,
        prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        let (ext, registered) = self.live_for_command(name)?;
        ext.argument_completions(&registered, prefix).await
    }

    /// Native-host fallback (no `wasm-host` feature): nothing but a native can own the command, and
    /// [`Self::command_completions`] has already tried that tier.
    #[cfg(not(feature = "wasm-host"))]
    async fn wasm_command_completions(
        &self,
        name: &str,
        _prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        Err(ExtError::Component(format!("no such command: {name}")))
    }

    /// SEAM-048 / EXT-017 — resolve an INVOCATION name to `(owner, registered name)`.
    ///
    /// `registry.command_owner` is a last-wins `HashMap` lookup on the RAW name, so when two
    /// extensions both register `deploy` only the last registrant is reachable and the other is
    /// silently unexecutable. pi disambiguates instead: `resolveRegisteredCommands` assigns
    /// `name:N` in LOAD ORDER with a `takenInvocationNames` bump loop
    /// (`extensions/runner.ts:598-631` @v0.83.0), and `name: cmd.invocationName` is what reaches
    /// autocomplete (`modes/interactive/interactive-mode.ts:605`).
    ///
    /// The SECOND half of the port is the one this function exists to carry: upstream looks the
    /// invocation name up ONCE — `getCommand(name)` matches `command.invocationName`
    /// (`extensions/runner.ts:647-649`) — and then calls the BOUND closure, `command.handler(args,
    /// ctx)` (`agent-session.ts:1283`) / `getArgumentCompletions: cmd.getArgumentCompletions`
    /// (`interactive-mode.ts:607`). The registered `name` is never used for a second lookup, so a
    /// suffix upstream never leaves the resolver.
    ///
    /// cyrup cannot bind a closure across the WIT boundary: the handler is reached by NAME again,
    /// inside the extension (`SdkApi::execute_command` matches `n == name`,
    /// `cyrup-ext-sdk/src/api.rs:1033`; `NativeExtension::execute_command`'s default arm errors on
    /// an unknown name, `native.rs:545`). Resolving the owner alone therefore left `deploy:2`
    /// routed correctly and then failing INSIDE its own owner with `no such command: deploy:2` —
    /// the disambiguation tier looked live while every suffixed command remained unexecutable.
    /// Returning the registered name and handing THAT to the extension is what makes the tier real.
    ///
    /// An uncollided command keeps its bare name unsuffixed, exactly as `resolveRegisteredCommands`
    /// leaves it, so on that path invocation and registered name are equal and the extra field
    /// costs nothing. Matching is on `invocation_name` ALONE — see
    /// [`ExtensionRegistry::resolved_command_owner`] for why the old raw-name fallback was the
    /// last-registration-wins defect rather than a safety net.
    fn command_route(&self, invocation: &str) -> Result<Option<(ExtensionId, String)>, ExtError> {
        Ok(self
            .registry
            .resolved_commands()?
            .into_iter()
            .find(|r| r.invocation_name == invocation)
            .map(|r| (r.owner, r.name)))
    }

    /// [`Self::command_route`] narrowed to the live-WASM tier, with pi's not-found errors.
    #[cfg(feature = "wasm-host")]
    fn live_for_command(
        &self,
        name: &str,
    ) -> Result<(Arc<crate::host::LiveExtension>, String), ExtError> {
        let (owner, registered) = self
            .command_route(name)?
            .ok_or_else(|| ExtError::Component(format!("no such command: {name}")))?;
        let ext = self
            .live
            .read()
            .ok()
            .and_then(|g| g.get(&owner).cloned())
            .ok_or_else(|| ExtError::Component(format!("command `{name}` has no live owner")))?;
        Ok((ext, registered))
    }

    /// Every key-id an extension has registered a keyboard shortcut for (R-08-017; Pi
    /// `registerShortcut`). The L6 TUI reads this at boot / on rebind so a matching key press routes
    /// to [`ExtensionHost::run_shortcut`] instead of the editor. Registry-backed, so it is available
    /// with or without the `wasm-host` feature (an empty list when nothing is registered).
    pub fn shortcut_keys(&self) -> Vec<String> {
        self.registry.shortcut_keys().unwrap_or_default()
    }

    /// Every registered shortcut as `(key, description)` (EXT-040). pi's `/hotkeys` Extensions
    /// table renders `shortcut.description ?? shortcut.extensionPath`
    /// (`modes/interactive/interactive-mode.ts:5856` @v0.83.0) — never the key id as its own
    /// label, which is what cyrup printed while `register_shortcut` discarded the description one
    /// line inside the host.
    pub fn shortcut_specs(&self) -> Vec<(String, Option<String>)> {
        self.registry.shortcut_specs().unwrap_or_default()
    }

    /// Resolve extension shortcuts against the host's keybinding config, refusing reserved keys and
    /// recording pi's warnings (EXT-039) — see [`crate::ExtensionRegistry::resolve_shortcuts`],
    /// which is the direct port of `ExtensionRunner.getShortcuts`
    /// (`extensions/runner.ts:492-534` @v0.83.0). The TUI passes its resolved `action -> keys` map
    /// and installs only the returned keys; [`Self::shortcut_diagnostics`] carries the warnings to
    /// the same `[Extension issues]` panel the load diagnostics use.
    pub fn resolve_shortcuts(
        &self,
        resolved_keybindings: &[(String, Vec<String>)],
    ) -> Vec<(String, ExtensionId)> {
        self.registry
            .resolve_shortcuts(resolved_keybindings)
            .unwrap_or_default()
    }

    /// [`Self::resolve_shortcuts`] in the shape the TUI installs — `(key, description ??
    /// extension id)` for every shortcut that survived pi's rules, see
    /// [`crate::ExtensionRegistry::resolve_shortcut_specs`].
    ///
    /// This is [`Self::shortcut_specs`]'s gated twin, and the one production callers want: pi
    /// never hands the raw per-extension map to its editor or to `/hotkeys`, only
    /// `getShortcuts(this.keybindings.getEffectiveConfig())`
    /// (`modes/interactive/interactive-mode.ts:2079`, `:6364` @v0.84.4).
    pub fn resolve_shortcut_specs(
        &self,
        resolved_keybindings: &[(String, Vec<String>)],
    ) -> Vec<(String, Option<String>)> {
        self.registry
            .resolve_shortcut_specs(resolved_keybindings)
            .unwrap_or_default()
    }

    /// Warnings from the last [`Self::resolve_shortcuts`] (pi `getShortcutDiagnostics()`,
    /// `extensions/runner.ts:538-540` @v0.83.0).
    pub fn shortcut_diagnostics(&self) -> Vec<crate::ExtensionConflict> {
        self.registry.shortcut_diagnostics().unwrap_or_default()
    }

    /// Execute the extension-registered keyboard shortcut bound to `key` (R-08-017; Pi
    /// `registerShortcut` handler, `extensions/types.ts:1249-1255` @v0.83.0). Resolves the owning
    /// extension from the registry and runs its handler at COMMAND tier — a live guest through
    /// [`crate::host::LiveExtension::execute_shortcut`], a native through
    /// [`NativeExtension::execute_shortcut`]. An unregistered key, or a key whose owner is in
    /// neither map, is a typed `ExtError`.
    ///
    /// EXT-035: this used to be `#[cfg(feature = "wasm-host")]` and resolve owners out of
    /// `self.live` only, with a `#[cfg(not(...))]` twin that failed unconditionally. A native's
    /// shortcut therefore registered, was advertised by `shortcut_keys()`, listed by `/hotkeys` —
    /// and could never fire. pi has one extension kind and one `ExtensionAPI`
    /// (`extensions/loader.ts:274-410`), so which tier the owner runs in cannot be allowed to
    /// decide whether its keybinding works.
    pub async fn run_shortcut(&self, key: &str, cancel: &CancelToken) -> Result<(), ExtError> {
        let owner = self
            .registry
            .shortcut_owner(key)?
            .ok_or_else(|| ExtError::Component(format!("no such shortcut: {key}")))?;
        // Native first: a native handle is in-process and cannot be mid-instantiation, and the
        // two maps are disjoint by construction (an id loads through exactly one path).
        let native = self.native.read().ok().and_then(|g| g.get(&owner).cloned());
        let out = if let Some(ext) = native {
            let ctx = HostCtx::command(
                self.config.mode,
                self.config.has_ui,
                self.config.cwd.clone(),
            );
            // Live rich fields, exactly as the native COMMAND route does (EXT-005) — a shortcut
            // handler reading `ctx.is_idle()`/`ctx.is_project_trusted()` must not get
            // `HostCtxRich::default()`.
            let ctx = match self.ctx_source() {
                Some(src) => ctx.with_rich(src.rich()),
                None => ctx,
            };
            ext.execute_shortcut(key, &ctx).await
        } else {
            #[cfg(feature = "wasm-host")]
            {
                let ext = self
                    .live
                    .read()
                    .ok()
                    .and_then(|g| g.get(&owner).cloned())
                    .ok_or_else(|| {
                        ExtError::Component(format!("shortcut `{key}` has no live owner"))
                    })?;
                ext.execute_shortcut(key, cancel).await
            }
            #[cfg(not(feature = "wasm-host"))]
            {
                Err(ExtError::Component(format!(
                    "shortcut `{key}` has no live owner"
                )))
            }
        };
        // Fan out any inter-extension bus events the shortcut handler emitted (gap-08 §5.3).
        self.deliver_bus_events(cancel).await;
        out
    }

    /// Mark every LIVE guest's context stale — the host-level port of pi's
    /// `_extensionRunner.invalidate(message?)` (`extensions/loader.ts:208-214` @v0.84.1, called from
    /// `AgentSession.dispose()` at `core/agent-session.ts:848`).
    ///
    /// Two effects, both upstream's: each instance's bus subscriptions are torn down (pi runs every
    /// tracked unsubscribe), and any later `bus.emit`/`bus.subscribe` from a call still in flight on
    /// an outgoing instance is refused rather than landing on the bus the REPLACEMENT set is
    /// listening on (pi's `assertActive`). See [`crate::host::GuestState::invalidate`].
    ///
    /// [`Self::reload`] calls this itself. It is `pub` because pi invalidates at the OTHER
    /// replacement points too — `dispose`/`teardownCurrent` for new/resume/fork/switch
    /// (`agent-session-runtime.ts:167-177`) — and those live in `cyrup-session-svc`
    /// (`AgentSession::dispose_with`, which already documents itself as sitting at exactly pi's
    /// `invalidate` position). This is the one call that seam needs.
    ///
    /// Callable on BOTH arms of `wasm-host` (EXT-060): it used to exist only with the Wasmtime host
    /// compiled in, which would have forced its cross-crate caller to carry a `#[cfg]` on a feature
    /// of a DEPENDENCY — a seam nobody reaches for is a seam that stays uncalled. Without live
    /// guests there is nothing to invalidate, so the native-only arm is a genuine no-op.
    #[cfg(feature = "wasm-host")]
    pub fn invalidate_live(&self, reason: Option<String>) {
        self.live_invalidations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if let Ok(g) = self.live.read() {
            for ext in g.values() {
                ext.guest().invalidate(reason.clone());
            }
        }
    }

    /// Native-only build: no live guest instances exist, so there is nothing to mark stale. See the
    /// `wasm-host` twin above for the contract.
    ///
    /// It still COUNTS — [`Self::live_invalidations`] records that the teardown contract was
    /// honoured, which is a property of the caller, not of whether a guest happened to be loaded.
    /// Counting on one arm only would make the seam's own test pass or fail on a feature flag.
    #[cfg(not(feature = "wasm-host"))]
    pub fn invalidate_live(&self, _reason: Option<String>) {
        self.live_invalidations
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// How many times [`Self::invalidate_live`] has run on this host.
    ///
    /// The observable for the invalidation seam. Marking a guest stale is otherwise only visible
    /// THROUGH a live guest (`GuestState::stale_reason`), so on a host with no wasm instances —
    /// every unit test, and the whole `--no-default-features` build — a caller that forgets to
    /// invalidate is indistinguishable from one that does. That is exactly how
    /// `AgentSession::dispose_with` went a long time without invalidating at all. This counts the
    /// CALL, so the contract can be asserted without standing up a guest.
    ///
    /// Monotonic and saturating in practice (it is a `u64` bumped once per teardown); the ordering
    /// is `Relaxed` because nothing is published through it.
    pub fn live_invalidations(&self) -> u64 {
        self.live_invalidations
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Hot reload (`/reload`, R-08-005): emit `session_shutdown{reload}` to the live set, cache-bust
    /// (drop the dispatcher + registry + live table + loaded ids), re-discover + re-load across the
    /// three roots, then emit `session_start{reload}`. Returns the fresh [`LoadExtensionsResult`].
    /// Stale instances are dropped (their `Arc`s released), so no invalidated instance is reachable.
    #[cfg(feature = "wasm-host")]
    pub async fn reload(
        &self,
        roots: &DiscoveryRoots,
        project_trusted: bool,
        services: Arc<dyn crate::host::HostServices>,
        cancel: &CancelToken,
    ) -> Result<LoadExtensionsResult, ExtError> {
        use crate::event::HostEvent;
        // 1) signal shutdown to the current set (reason = "reload").
        self.dispatcher
            .dispatch_notify(
                // EXT-015: a reload is not a session REPLACEMENT, so pi's optional
                // `targetSessionFile` ("Destination session file when shutting down due to session
                // replacement", extensions/types.ts:619-620 @v0.83.0) is genuinely absent here.
                &HostEvent::SessionShutdown {
                    reason: "reload".into(),
                    target_session_file: None,
                },
                cancel,
            )
            .await;
        // 2) cache-bust: drop dispatcher entries, registry tables, live instances, loaded ids.
        self.dispatcher.clear()?;
        self.registry.clear()?;
        // EXT-050 — invalidate each outgoing instance BEFORE the map is cleared, the port of pi's
        // `runtime.invalidate()` (`extensions/loader.ts:208-214` @v0.84.1). Two things follow from
        // it: the instance's own subscriptions are torn down (upstream runs every tracked
        // unsubscribe), and any later `bus.emit` from a call still in flight on the OLD instance is
        // refused instead of being queued for the FRESH set that replaced it — upstream's
        // `assertActive`. Without this the only teardown was the whole-bus `clear()` below, which is
        // all-or-nothing and says nothing about who is still allowed to publish.
        self.invalidate_live(None);
        // Drop stale bus subscriptions + any undelivered queued events; the fresh load re-declares
        // its subscriptions during `init` (gap-08 §5.3).
        self.bus.clear();
        if let Ok(mut g) = self.native.write() {
            g.clear();
        }
        if let Ok(mut g) = self.live.write() {
            g.clear();
        }
        if let Ok(mut g) = self.loaded.write() {
            g.clear();
        }
        // 3) re-discover + re-load.
        let result = self
            .discover_and_load(roots, project_trusted, services)
            .await;
        // 4) signal start to the fresh set (reason = "reload").
        self.dispatcher
            .dispatch_notify(
                // EXT-015: pi documents `previousSessionFile` as "Present for \"new\",
                // \"resume\", and \"fork\"" (extensions/types.ts:568 @v0.83.0) — a reload keeps
                // the SAME session file, so it is absent.
                &HostEvent::SessionStart {
                    reason: "reload".into(),
                    previous_session_file: None,
                },
                cancel,
            )
            .await;
        Ok(result)
    }

    /// Per-call epoch budget for a loaded wasm extension. The epoch driver ticks every
    /// [`crate::host::epoch::DEFAULT_TICK`] (5ms); 1000 ticks ≈ 5s before a runaway guest is
    /// preempted (R-ARCH-EXT-012). The dispatcher's invocation budget is a coarser backstop.
    #[cfg(feature = "wasm-host")]
    const WASM_EPOCH_BUDGET_TICKS: u64 = 1000;

    /// INVOCATION names of every registered native command (diagnostics / completion), in load
    /// order. A subset of [`ExtensionRegistry::resolved_commands`] limited to native-owned commands.
    ///
    /// SEAM-048's last reader. This walked [`ExtensionRegistry::command_names`] — the LAST-WINS
    /// `HashMap` — and resolved each through `command_owner`, so when two natives registered the same
    /// name the first one's command was invisible here and the surviving entry was attributed to
    /// whichever extension registered last. Upstream offers completion the resolved `invocationName`
    /// (`modes/interactive/interactive-mode.ts:605` @v0.83.0, `name: cmd.invocationName`, over
    /// `getRegisteredCommands()` → `resolveRegisteredCommands()`), which is load-ordered and keeps
    /// BOTH duplicates as `name:1` / `name:2`. The two dispatch sites already route through
    /// [`Self::command_route`]; this was the one enumerator left on the old map, so a `deploy:2` was
    /// executable but unlistable.
    pub fn native_command_names(&self) -> Vec<String> {
        let native = match self.native.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        self.registry
            .resolved_commands()
            .unwrap_or_default()
            .into_iter()
            .filter(|c| native.contains_key(&c.owner))
            .map(|c| c.invocation_name)
            .collect()
    }

    fn reserve_id(&self, id: &ExtensionId) -> Result<(), ExtError> {
        let mut g = self
            .loaded
            .write()
            .map_err(|_| ExtError::Io("host lock poisoned".into()))?;
        if g.iter().any(|e| e == id) {
            return Err(ExtError::DuplicateId(id.to_string()));
        }
        g.push(id.clone());
        Ok(())
    }

    /// Undo a [`Self::reserve_id`] after the load that claimed it failed (EXT-S01). Silent on a
    /// poisoned lock — the load is already reporting its own error.
    fn release_id(&self, id: &ExtensionId) {
        if let Ok(mut g) = self.loaded.write() {
            g.retain(|e| e != id);
        }
    }
}

/// Extract a panic payload message (mirrors `native::panic_msg`, kept local so the facade does not
/// reach into the native module's private helper).
/// Which of the three renderer hooks [`ExtensionHost::render_via`] is dispatching.
///
/// `Call`/`Result` are the tool + custom-message pair; `Entry` is the custom-ENTRY renderer
/// (`registerEntryRenderer`). On the GUEST wire `Entry` reuses `render-call` — see
/// [`ExtensionHost::render_entry`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RenderKind {
    Call,
    Result,
    Entry,
}

/// What an extension's registered renderer produced — the three outcomes Pi's renderer components
/// distinguish (`custom-entry.ts:40-60`, `custom-message.ts:60-88`).
///
/// X15 — cyrup previously modelled this as `Option<Value>` and so could not tell a renderer that
/// THREW from one that was never registered: both were `None`, and `cyrup-tui` mapped `None` to
/// "draw the default framing". That made `crate::` incapable of ever producing Pi's
/// `[type] renderer failed: …` box, which is the ONLY thing upstream draws for a throw
/// (`custom-entry.ts:50` is the sole occurrence of that string in `packages/`).
///
/// Note the two "nothing came back" cases are deliberately ONE variant: upstream's `!component`
/// check (`custom-entry.ts:54-56`) treats "no renderer registered" and "the renderer returned
/// `undefined`" identically on both surfaces, so no consumer needs to tell them apart. Callers that
/// want the cheap pre-check ask [`ExtensionHost::has_entry_renderer`] /
/// [`ExtensionHost::has_message_renderer`] instead, exactly as upstream does before constructing a
/// component.
///
/// `Eq` is deliberately absent: [`Self::Live`] holds an `Arc<dyn RenderedComponent>`, which has no
/// meaningful total equality. `PartialEq` is hand-written below and compares `Live` by pointer.
#[derive(Clone, Debug, Default)]
pub enum RenderOutcome {
    /// No renderer is registered for this key, or the registered renderer chose to draw nothing
    /// (`Component | undefined` returning `undefined`). The host draws its own framing.
    #[default]
    None,
    /// The renderer's output — a serialized widget tree, the wire analog of the `pi-tui`
    /// `Component` an upstream renderer returns.
    Rendered(Value),
    /// The renderer FAULTED: a native renderer panicked, or a guest renderer trapped/errored. The
    /// payload is the message, upstream's
    /// `error instanceof Error ? error.message : String(error)` (`custom-entry.ts:48`).
    Failed(String),
    /// The renderer handed back a LIVE component, to be re-rendered by the host on every frame at
    /// the current width, theme and expansion. Native-only — see [`crate::RenderedComponent`].
    Live(std::sync::Arc<dyn crate::RenderedComponent>),
}

impl PartialEq for RenderOutcome {
    /// Structural for the value arms; pointer identity for [`Self::Live`], which is the only
    /// equality a trait object can honestly offer.
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::None, Self::None) => true,
            (Self::Rendered(a), Self::Rendered(b)) => a == b,
            (Self::Failed(a), Self::Failed(b)) => a == b,
            (Self::Live(a), Self::Live(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

impl RenderOutcome {
    fn from_option(v: Option<Value>) -> Self {
        match v {
            Some(v) => Self::Rendered(v),
            None => Self::None,
        }
    }

    /// Collapse back to the pre-X15 shape, which is what the MESSAGE and TOOL surfaces genuinely
    /// want: `custom-message.ts:82-84` catches a throw and falls through to the default box, i.e.
    /// upstream itself treats a faulting message renderer as "no rendered component".
    pub fn into_option(self) -> Option<Value> {
        match self {
            Self::Rendered(v) => Some(v),
            // A live component is not a `Value` and cannot be collapsed into one; a caller that
            // wants JSON genuinely has none.
            Self::None | Self::Failed(_) | Self::Live(_) => None,
        }
    }

    /// The fault message, if this is a [`Self::Failed`].
    pub fn failure(&self) -> Option<&str> {
        match self {
            Self::Failed(m) => Some(m.as_str()),
            _ => None,
        }
    }
}

fn native_panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}

/// The [`crate::host::ProviderReduction`] the host installs into every loaded guest (EXT-052).
///
/// Backs `provider-stream.on-payload` / `on-response`, the two callbacks pi's
/// `ProviderConfig.streamSimple` doc makes a MUST-INVOKE contract
/// (`pi/packages/coding-agent/src/core/extensions/types.ts:1452-1457` @v0.84.1). Both route into
/// the SAME reductions the built-in provider path already uses, so an extension-supplied provider's
/// requests stop being invisible to every other extension — and, critically, so
/// `before_provider_request`'s payload REPLACEMENT applies on that route, which is what keeps a
/// redaction extension from leaking the moment the user switches model.
#[cfg(feature = "wasm-host")]
struct DispatcherProviderReduction {
    dispatcher: Arc<Dispatcher>,
}

#[cfg(feature = "wasm-host")]
#[async_trait::async_trait]
impl crate::host::ProviderReduction for DispatcherProviderReduction {
    async fn before_provider_request(&self, from: &ExtensionId, payload: Value) -> Option<Value> {
        let original = payload.clone();
        // The calling guest is suspended inside its own store; excluding it is the forced
        // divergence documented on `Dispatcher::dispatch_block_mutate_excluding`.
        let reduced = self
            .dispatcher
            .dispatch_block_mutate_excluding(
                crate::event::HostEvent::BeforeProviderRequest { payload },
                &CancelToken::new(),
                Some(from),
            )
            .await;
        match reduced {
            Reduced::Pass(ev) => match *ev {
                // pi's "use any returned replacement payload": only an ACTUAL change is a
                // replacement, so an unchanged payload reports `None` and the guest sends its own.
                crate::event::HostEvent::BeforeProviderRequest { payload }
                    if payload != original =>
                {
                    Some(payload)
                }
                _ => None,
            },
            // A blocked/handled `before_provider_request` has no meaning on this seam upstream
            // (the event is `[mutate]`, `extensions/types.ts:676-679`); keep the guest's payload.
            _ => None,
        }
    }

    async fn after_provider_response(&self, from: &ExtensionId, status: u32, headers: Value) {
        self.dispatcher
            .dispatch_notify_excluding(
                &crate::event::HostEvent::AfterProviderResponse { status, headers },
                &CancelToken::new(),
                Some(from),
            )
            .await;
    }
}

#[async_trait::async_trait]
impl crate::bus::BusDrain for BusFanout {
    /// The fan-out proper (EXT-018 / EXT-034 / EXT-057). See
    /// [`ExtensionHost::deliver_bus_events`] for the full provenance.
    async fn drain_bus(&self, cancel: &CancelToken) {
        // Re-entrancy: a nested seam (the host's explicit drain called from `run_command`, or a
        // dispatch reached from a delivered handler) must not start a second fan-out over the same
        // queue — the outer one already owns it and will pick up whatever the inner seam enqueued
        // on its next round. RAII, because a dropped future must not leave the latch stuck.
        let Some(_latch) = crate::bus::DrainLatch::acquire(&self.draining) else {
            return;
        };
        // Bound on delivery rounds: each round drains the whole queue, then re-checks for events a
        // just-delivered handler emitted. A cycle (A→B→A→…) stops after the bound rather than
        // hanging.
        const MAX_ROUNDS: usize = 64;
        for _ in 0..MAX_ROUNDS {
            let batch = self.bus.take_pending();
            if batch.is_empty() {
                return;
            }
            for (topic, payload) in batch {
                for id in self.bus.subscribers_for(&topic) {
                    if let Err(e) = self.deliver_one(&id, &topic, &payload, cancel).await {
                        tracing::warn!(
                            extension = %id, topic = %topic, error = %e,
                            "inter-extension bus delivery contained (skipped)"
                        );
                        // EXT-057b: also onto the onError channel, so a trapping bus listener is
                        // as visible as a trapping event handler.
                        self.report_bus_error(&id, format!("bus `{topic}`: {e}"));
                    }
                }
            }
        }
        // EXT-057a: the bound was reached. Anything still queued is dropped — say so.
        let dropped = self.bus.drop_pending();
        if dropped > 0 {
            let msg = format!(
                "inter-extension bus gave up after {MAX_ROUNDS} delivery rounds; {dropped} queued \
                 event(s) dropped (a handler is emitting on every round)"
            );
            tracing::warn!(rounds = MAX_ROUNDS, dropped, "{msg}");
            self.report_bus_error(&ExtensionId::from("bus"), msg);
        }
    }
}

impl BusFanout {
    /// Deliver one bus event to one subscriber, whichever tier it lives in (EXT-018). A subscriber
    /// that is in NEITHER map has gone stale — its instance left the host — so its subscription is
    /// torn down here rather than left to accumulate (EXT-050; pi's `invalidate()` runs every
    /// tracked unsubscribe, `extensions/loader.ts:206-214` @v0.84.1).
    async fn deliver_one(
        &self,
        id: &ExtensionId,
        topic: &str,
        payload: &Value,
        #[cfg_attr(not(feature = "wasm-host"), allow(unused_variables))] cancel: &CancelToken,
    ) -> Result<(), ExtError> {
        #[cfg(feature = "wasm-host")]
        if let Some(ext) = self.live.read().ok().and_then(|g| g.get(id).cloned()) {
            return ext.bus_deliver(topic, payload, cancel).await;
        }
        if let Some(ext) = self.native.read().ok().and_then(|g| g.get(id).cloned()) {
            let ctx = HostCtx::event(
                self.config.mode,
                self.config.has_ui,
                self.config.cwd.clone(),
            );
            return ext.on_bus_event(topic, payload, &ctx).await;
        }
        self.bus.unsubscribe(id, topic);
        Ok(())
    }

    /// Surface a bus-layer fault on the same `onError` channel a contained handler fault uses.
    fn report_bus_error(&self, id: &ExtensionId, error: String) {
        self.dispatcher.report_external(crate::ExtensionError {
            extension: id.clone(),
            // pi's bus faults are logged with the CHANNEL, not an event name; `"bus"` is the
            // closest honest label in `ExtensionError.event`'s `&'static str` vocabulary.
            event: "bus",
            error,
        });
    }
}
