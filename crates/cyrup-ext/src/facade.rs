//! The `ExtensionHost` facade (arch-08 §3.1): the single entry point the session service wires in.
//! Holds the registry + dispatcher + native registry (+ the Wasmtime engine/pool when the
//! `wasm-host` feature is on). Exposes the two agent seams — [`ExtSubscriber`] (notify) and
//! [`ExtHooks`] (mutating) — plus the merged active tool set.

use crate::contract::{HandledValue, Reduced};
use crate::dispatch::Dispatcher;
use crate::error::ExtError;
use crate::event::{HostEvent, InputEventSource, InputStreamingBehavior};
use crate::hooks::ExtHooks;
use crate::loader::{DiscoveredExtension, DiscoveryRoots};
// Only the wasm-host guest-loading path constructs these; `discover()` (ungated) needs just the
// two above, so gate the rest to keep `--no-default-features` warning-free.
#[cfg(feature = "wasm-host")]
use crate::loader::{LoadError, LoadExtensionsResult};
#[cfg(feature = "wasm-host")]
use crate::manifest::HOST_WORLD;
use crate::native::{ExtMode, HostCtx, InitApi, NativeExtension, NativeHandle};
use crate::registry::ExtensionRegistry;
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
    Blocked { reason: Option<String>, by: ExtensionId },
}

/// The reduced result of [`ExtensionHost::emit_user_bash`] (Pi `UserBashEventResult`, types.ts:1043):
/// proceed, the extension fully serviced it (`operations`/`result`), or a block.
#[derive(Clone, Debug)]
pub enum UserBashReduction {
    /// No handler intercepted the `!`/`!!` command (proceed with the default bash execution).
    Continue,
    /// A handler fully serviced it (Pi `{operations}`/`{result}`) — carried as the open-shaped value.
    Handled(Value),
    /// A handler blocked the command (first block wins).
    Blocked { reason: Option<String>, by: ExtensionId },
}

/// The reduced result of [`ExtensionHost::emit_session_before_compact`] (Pi
/// `SessionBeforeCompactResult`, types.ts:1077-1080): proceed with the default compaction, a veto
/// (first block wins), or an extension-supplied compaction override (Pi `compaction: CompactionResult`).
#[derive(Clone, Debug)]
pub enum CompactionReduction {
    /// No handler intervened — run the default (model) compaction.
    Proceed,
    /// A handler vetoed the compaction (Pi `{cancel:true}`); first block wins.
    Blocked { reason: Option<String>, by: ExtensionId },
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
    Blocked { reason: Option<String>, by: ExtensionId },
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

/// The extension host facade (arch-08 §3.1).
pub struct ExtensionHost {
    dispatcher: Arc<Dispatcher>,
    registry: Arc<ExtensionRegistry>,
    config: HostConfig,
    loaded: RwLock<Vec<ExtensionId>>,
    /// Loaded native built-ins keyed by id — the native slash-command routing table (R-08-016). A
    /// native command runs in-process (no wasm), so the handle is kept here to reach its
    /// [`NativeExtension::execute_command`] when a `/<cmd>` it owns is invoked.
    native: RwLock<std::collections::HashMap<ExtensionId, Arc<dyn NativeExtension>>>,
    #[cfg(feature = "wasm-host")]
    wasm: Option<crate::host_runtime::WasmRuntime>,
    /// Loaded live wasm instances keyed by id — the slash-command/reload routing table (R-08-016/005).
    #[cfg(feature = "wasm-host")]
    live: RwLock<std::collections::HashMap<ExtensionId, Arc<crate::host::LiveExtension>>>,
    /// The single host-owned inter-extension event bus (Pi's one `createEventBus()` threaded to every
    /// extension, event-bus.ts:12-32 / loader.ts:492,499). Shared into every loaded guest so a
    /// `bus.emit` from one reaches another's subscribed `bus-deliver` handler (gap-08 §5.3).
    #[cfg(feature = "wasm-host")]
    bus: Arc<crate::host::SharedBus>,
    /// The live capability backend the session injected (via [`Self::load_native_with_services`]).
    /// Threaded into every native dispatch/command ctx so a built-in's `ctx.rich()` reports the
    /// LIVE idle/trust/usage/model/system-prompt instead of `HostCtxRich::default()` (EXT-005).
    #[cfg(feature = "wasm-host")]
    services: RwLock<Option<Arc<dyn crate::host::HostServices>>>,
    /// The live `getActiveTools` source the registered-tool wrapper diffs against (Pi
    /// `ExtensionRunner.getActiveTools`, runner.ts:664-667). `None` until a session attaches one
    /// via [`Self::set_active_tool_source`], in which case [`Self::active_tools`] hands tools back
    /// UNWRAPPED — there is no live agent whose tool set could change, so the diff has no meaning.
    active_tool_source: RwLock<Option<Arc<dyn crate::wrapper::ActiveToolNames>>>,
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
        Self {
            dispatcher: Arc::new(Dispatcher::new()),
            registry: Arc::new(ExtensionRegistry::new()),
            config,
            loaded: RwLock::new(Vec::new()),
            native: RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "wasm-host")]
            wasm: None,
            #[cfg(feature = "wasm-host")]
            live: RwLock::new(std::collections::HashMap::new()),
            #[cfg(feature = "wasm-host")]
            bus: Arc::new(crate::host::SharedBus::new()),
            #[cfg(feature = "wasm-host")]
            services: RwLock::new(None),
            active_tool_source: RwLock::new(None),
        }
    }

    /// Attach the live `getActiveTools` source (Pi binds `runtime.getActiveTools` from the session's
    /// actions, runner.ts:330). Every tool [`Self::active_tools`] returns from then on is wrapped
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
        // (EXT-005), not just the built-ins that stash the Arc themselves.
        if let Ok(mut g) = self.services.write() {
            *g = Some(services);
        }
        self.load_native_inner(ext).await
    }

    /// The injected capability backend, if one was supplied (EXT-005).
    #[cfg(feature = "wasm-host")]
    fn host_services(&self) -> Option<Arc<dyn crate::host::HostServices>> {
        self.services.read().ok().and_then(|g| g.clone())
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
        let mut api = InitApi::new();
        ext.init(&mut api).await?;
        let (subs, tools, commands, tool_renderers, message_renderers) = api.into_parts();

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
        for (name, desc) in commands {
            self.registry.register_command(id.clone(), name, desc)?;
        }
        // EXT-006: a native built-in's renderer declarations land in the SAME registry tables the
        // guest path writes, so `render_tool_call`/`render_message_call` route by name/type without
        // caring which runtime supplies the renderer.
        for tool_name in tool_renderers {
            self.registry.register_tool_renderer(id.clone(), tool_name)?;
        }
        for custom_type in message_renderers {
            self.registry.register_message_renderer(id.clone(), custom_type)?;
        }

        // Keep the native handle for command-tier slash execution (R-08-016) before it is wrapped
        // for event dispatch.
        if let Ok(mut g) = self.native.write() {
            g.insert(id.clone(), ext.clone());
        }
        let ctx = HostCtx::event(self.config.mode, self.config.has_ui, self.config.cwd.clone());
        let handle = NativeHandle::new(ext, subs, ctx);
        #[cfg(feature = "wasm-host")]
        let handle = handle.with_services(self.host_services());
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
        let owner = match self.registry.command_owner(name)? {
            Some(o) => o,
            None => return Ok(None),
        };
        let ext = match self.native.read().ok().and_then(|g| g.get(&owner).cloned()) {
            Some(e) => e,
            // The command is owned by a non-native (wasm) extension: not our route.
            None => return Ok(None),
        };
        let ctx = HostCtx::command(self.config.mode, self.config.has_ui, self.config.cwd.clone());
        // Live rich fields for the command ctx too (EXT-005) — a command handler reading
        // `ctx.is_idle()`/`ctx.is_project_trusted()` used to get `HostCtxRich::default()`.
        #[cfg(feature = "wasm-host")]
        let ctx = match self.host_services() {
            Some(svc) => ctx.with_rich(crate::native::rich_from_services(svc.as_ref())),
            None => ctx,
        };
        let fut = std::panic::AssertUnwindSafe(ext.execute_command(name, args, &ctx));
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
    pub fn subscriber(&self, cancel: CancelToken) -> Arc<dyn EventSubscriber> {
        Arc::new(ExtSubscriber::new(self.dispatcher.clone(), cancel))
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
    /// agent-session.ts:2452-2546; EXT-004). Returns whether the executable tool set changed, so a
    /// caller can skip an expensive downstream rebuild (system prompt / active-set push).
    ///
    /// Cheap when nothing changed: a relaxed atomic load, no lock. Sync on purpose — wrapping a
    /// descriptor is pure bookkeeping, so this is callable from `active_tools` and from a
    /// non-async drain alike.
    pub fn refresh_tools(&self) -> Result<bool, ExtError> {
        if !self.registry.take_tools_dirty() {
            return Ok(false);
        }
        self.materialize_guest_tools()
    }

    /// The `wasm-host` half of [`Self::refresh_tools`]: wrap every guest descriptor that has no
    /// executable counterpart yet into a [`crate::host::WasmTool`] bound to its OWNING live instance.
    #[cfg(feature = "wasm-host")]
    fn materialize_guest_tools(&self) -> Result<bool, ExtError> {
        let mut changed = false;
        for (owner, desc) in self.registry.guest_tool_entries()? {
            if self.registry.tool(&desc.name)?.is_some() {
                continue;
            }
            let Some(ext) = self.live.read().ok().and_then(|g| g.get(&owner).cloned()) else {
                // A descriptor whose owner is not (yet) live: skip it rather than fabricating a
                // tool that cannot execute. Re-arm so the next refresh retries once it is live.
                self.registry.mark_tools_dirty();
                continue;
            };
            let tool: Arc<dyn Tool> = Arc::new(crate::host::WasmTool::new(ext, desc));
            self.registry.register_tool(owner, tool)?;
            changed = true;
        }
        // `register_tool` re-dirties the flag by design (it is the same signal a host-tool
        // registration raises); clear the marks THIS pass produced so a caller does not loop.
        if changed {
            self.registry.take_tools_dirty();
        }
        Ok(changed)
    }

    /// Native-only build: a native extension's tools are already executable `Arc<dyn Tool>`s
    /// registered directly into the registry, so "re-materialize" is a no-op — but the dirty flag
    /// still reports that the set CHANGED, which is what the caller acts on.
    #[cfg(not(feature = "wasm-host"))]
    fn materialize_guest_tools(&self) -> Result<bool, ExtError> {
        Ok(true)
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
            .dispatch_first_handled(&HostEvent::ProjectTrust, cancel, |HandledValue(v)| {
                crate::aggregate::parse_trust_decision(v).is_some()
            })
            .await?;
        crate::fold_project_trust(std::slice::from_ref(&hit))
    }

    /// Aggregate the skill/prompt/theme paths every extension provides (Pi `resources_discover`,
    /// runner.ts:197; gap-08 #4) into a typed, attributed [`crate::ResourcesAggregate`].
    pub async fn aggregate_resources(&self, cancel: &CancelToken) -> crate::ResourcesAggregate {
        use crate::event::HostEvent;
        let handled =
            self.dispatcher.dispatch_collect_handled(&HostEvent::ResourcesDiscover, cancel).await;
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
                let HostEvent::BeforeAgentStart { system_prompt: sp, injected, .. } = *ev else {
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
        let ev = HostEvent::Input { text: orig_text.clone(), images, source, streaming_behavior };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by } => InputReduction::Blocked { reason, by },
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
    pub async fn emit_user_bash(
        &self,
        command: &str,
        cancel: &CancelToken,
    ) -> UserBashReduction {
        // `exclude_from_context` (the `!!` prefix) is decided by the submission parser at the caller
        // (cross-crate), so it defaults to `false` here; `cwd` is the process working directory (Pi
        // `UserBashEvent.cwd`, types.ts:789). The richer caller-supplied values flow once the
        // submission pipeline threads them into this entry point.
        let cwd =
            std::env::current_dir().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let ev = HostEvent::UserBash {
            command: command.to_string(),
            exclude_from_context: false,
            cwd,
        };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by } => UserBashReduction::Blocked { reason, by },
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
            Reduced::Blocked { reason, by } => CompactionReduction::Blocked { reason, by },
            Reduced::Pass(ev) => match *ev {
                HostEvent::SessionBeforeCompact { override_result: Some(v), .. } => {
                    CompactionReduction::Override(v)
                }
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
        let ev = HostEvent::SessionBeforeTree { preparation, override_result: None };
        match self.dispatcher.dispatch_block_mutate(ev, cancel).await {
            Reduced::Blocked { reason, by } => TreeReduction::Blocked { reason, by },
            Reduced::Pass(ev) => match *ev {
                HostEvent::SessionBeforeTree { override_result: Some(v), .. } => {
                    TreeReduction::Override(v)
                }
                _ => TreeReduction::Proceed,
            },
            Reduced::Handled(_) => TreeReduction::Proceed,
        }
    }

    /// Render a TOOL CALL through the extension that registered a renderer for that tool (Pi's
    /// per-tool `ToolDefinition.renderCall`, extensions/types.ts:472-473, resolved by
    /// `modes/interactive/components/tool-execution.ts:81-112` — the extension's definition is
    /// preferred over the built-in). `None` = no extension renders this tool (draw the standard
    /// shell), which is also what a faulting renderer degrades to.
    ///
    /// This is the missing link EXT-006 was about: `ToolDescriptor.has_renderer` was recorded and
    /// `LiveExtension::render_call` existed, but nothing could get from a tool NAME to the guest
    /// that renders it, so both were dead outside a unit test.
    pub async fn render_tool_call(&self, tool_name: &str, call: &Value) -> Option<Value> {
        let owner = self.registry.tool_renderer_owner(tool_name).ok().flatten()?;
        self.render_via(&owner, tool_name, call, true).await
    }

    /// Render a TOOL RESULT through the tool's registered renderer (Pi `renderResult`,
    /// extensions/types.ts:475-481). See [`Self::render_tool_call`].
    pub async fn render_tool_result(&self, tool_name: &str, result: &Value) -> Option<Value> {
        let owner = self.registry.tool_renderer_owner(tool_name).ok().flatten()?;
        self.render_via(&owner, tool_name, result, false).await
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
        let owner = self.registry.message_renderer_owner(custom_type).ok().flatten()?;
        self.render_via(&owner, custom_type, message, true).await
    }

    /// The result-side companion of [`Self::render_message_call`].
    pub async fn render_message_result(&self, custom_type: &str, message: &Value) -> Option<Value> {
        let owner = self.registry.message_renderer_owner(custom_type).ok().flatten()?;
        self.render_via(&owner, custom_type, message, false).await
    }

    /// Invoke `render-call`/`render-result` on a specific extension, containing faults LOCALLY
    /// (`warn!` + `None`) the way [`Self::deliver_bus_events`] does. Deliberately NOT routed through
    /// `dispatch_block_mutate`: a renderer is a presentation concern and a faulting one must never
    /// be able to block the tool call it was asked to draw (R-08-036).
    ///
    /// NATIVE owners are tried first and are available in EVERY build (a native renderer needs no
    /// wasm host); a guest owner resolves against the live instance map only under `wasm-host`.
    async fn render_via(
        &self,
        owner: &ExtensionId,
        key: &str,
        payload: &Value,
        is_call: bool,
    ) -> Option<Value> {
        if let Some(native) = self.native.read().ok().and_then(|g| g.get(owner).cloned()) {
            // A panicking native renderer must degrade to the default framing, never take the
            // frame down with it (R-08-036) — the same containment the guest arm gets below.
            let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if is_call {
                    native.render_call(key, payload)
                } else {
                    native.render_result(key, payload)
                }
            }));
            return match rendered {
                Ok(v) => v,
                Err(panic) => {
                    tracing::warn!(
                        extension = %owner, key = %key,
                        error = %native_panic_msg(panic),
                        "native renderer panicked (falling back to the default renderer)"
                    );
                    None
                }
            };
        }
        self.render_via_guest(owner, key, payload, is_call).await
    }

    #[cfg(feature = "wasm-host")]
    async fn render_via_guest(
        &self,
        owner: &ExtensionId,
        key: &str,
        payload: &Value,
        is_call: bool,
    ) -> Option<Value> {
        let ext = self.live.read().ok().and_then(|g| g.get(owner).cloned())?;
        let out = if is_call {
            ext.render_call(key, payload).await
        } else {
            ext.render_result(key, payload).await
        };
        match out {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(
                    extension = %owner, key = %key, error = %e,
                    "extension renderer fault contained (falling back to the default renderer)"
                );
                None
            }
        }
    }

    /// Native-only build: no live guest can hold a renderer, so a guest-owned key draws with the
    /// host's own framing. (A NATIVE-owned key is still rendered — see [`Self::render_via`].)
    #[cfg(not(feature = "wasm-host"))]
    async fn render_via_guest(
        &self,
        _owner: &ExtensionId,
        _key: &str,
        _payload: &Value,
        _is_call: bool,
    ) -> Option<Value> {
        None
    }

    /// Whether ANY extension registered a renderer for this tool name (Pi
    /// `hasRendererDefinition`, tool-execution.ts:81-112) — the cheap check a UI makes before
    /// paying for a guest round trip.
    pub fn has_tool_renderer(&self, tool_name: &str) -> bool {
        self.registry.tool_renderer_owner(tool_name).ok().flatten().is_some()
    }

    /// Whether ANY extension registered a custom-message renderer for `custom_type` (Pi
    /// `getMessageRenderer(...) !== undefined`, runner.ts:579-587).
    pub fn has_message_renderer(&self, custom_type: &str) -> bool {
        self.registry.message_renderer_owner(custom_type).ok().flatten().is_some()
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

    /// Fan out every queued inter-extension bus event to its subscribers (gap-08 §5.3). Drains the
    /// shared bus (Pi's `createEventBus()` fan-out) and invokes each subscribed guest's `bus-deliver`
    /// export. Loops so a handler that emits during delivery is itself delivered (bounded to guard a
    /// pathological emit cycle — never hangs). A faulting delivery is contained + skipped (R-08-036),
    /// never crashing the host. Called at the tail of a guest call that may have emitted (e.g.
    /// [`Self::run_command`]); also public so other guest-call seams can drain after their dispatch.
    #[cfg(feature = "wasm-host")]
    pub async fn deliver_bus_events(&self, cancel: &CancelToken) {
        // Bound on delivery rounds: each round drains the whole queue, then re-checks for events a
        // just-delivered handler emitted. A cycle (A→B→A→…) stops after the bound rather than hanging.
        const MAX_ROUNDS: usize = 64;
        for _ in 0..MAX_ROUNDS {
            let batch = self.bus.take_pending();
            if batch.is_empty() {
                return;
            }
            for (topic, payload) in batch {
                for id in self.bus.subscribers_for(&topic) {
                    let ext = self.live.read().ok().and_then(|g| g.get(&id).cloned());
                    // Contained (R-08-036): a faulting listener is logged + skipped; the rest of the
                    // fan-out proceeds and the host never crashes.
                    if let Some(ext) = ext
                        && let Err(e) = ext.bus_deliver(&topic, &payload, cancel).await
                    {
                        tracing::warn!(
                            extension = %id, topic = %topic, error = %e,
                            "inter-extension bus delivery contained (skipped)"
                        );
                    }
                }
            }
        }
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
    #[cfg(feature = "wasm-host")]
    pub async fn load_wasm(
        &self,
        id: ExtensionId,
        bytes: &[u8],
        services: Arc<dyn crate::host::HostServices>,
    ) -> Result<Arc<crate::host::LiveExtension>, ExtError> {
        let wasm = self.wasm.as_ref().ok_or(ExtError::WasmHostDisabled)?;
        self.reserve_id(&id)?;
        let guest = Arc::new(
            crate::host::GuestState::with_services(id.clone(), self.registry.clone(), services)
                // Wire the guest onto the HOST-OWNED shared bus (not a fresh per-guest one) so its
                // `bus.subscribe`/`bus.emit` reach other guests (Pi's single shared EventBus,
                // gap-08 §5.3).
                .with_bus(self.bus.clone())
                // Pi `ctx.mode` / `ctx.hasUI` (extensions/types.ts:311,313) are host configuration,
                // not session state: copy them in from the SAME [`HostConfig`] the native path
                // hands to `HostCtx::event`/`::command` above, so a WASM guest's `ctx.mode()` and a
                // built-in's `ctx.mode` cannot disagree about the mode the host is running in.
                .with_host_mode(self.config.mode, self.config.has_ui),
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

    /// Discover extensions across the three roots (Pi `discoverAndLoadExtensions`). Pure filesystem
    /// scan; no wasm runtime required. See [`crate::loader::discover`].
    pub fn discover(&self, roots: &DiscoveryRoots) -> Vec<DiscoveredExtension> {
        crate::loader::discover(roots)
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
        for disc in self.discover(roots) {
            match self.load_discovered(&disc, project_trusted, services.clone()).await {
                Ok(id) => result.loaded.push(id),
                Err(e) => result.errors.push(LoadError {
                    path: disc.dir.clone(),
                    // The trust-gate skip is NOT a load failure (see `LoadError::fatal`).
                    fatal: !matches!(e, ExtError::Untrusted),
                    error: e.to_string(),
                }),
            }
        }
        result
    }

    /// Load one discovered extension, applying the world-version check + trust gate (R-08-002).
    #[cfg(feature = "wasm-host")]
    pub async fn load_discovered(
        &self,
        disc: &DiscoveredExtension,
        project_trusted: bool,
        services: Arc<dyn crate::host::HostServices>,
    ) -> Result<ExtensionId, ExtError> {
        disc.manifest.check_world(HOST_WORLD)?;
        if !disc.is_trusted(project_trusted) {
            // Project-local extension in an untrusted project: not loaded (R-ARCH-EXT-017).
            return Err(ExtError::Untrusted);
        }
        let bytes = crate::loader::resolve_component_bytes(disc)?;
        let id = disc.id();
        self.load_wasm(id.clone(), &bytes, services).await?;
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
        let ext = self.live_for_command(name)?;
        let out = ext.execute_command(name, args, cancel).await;
        // Fan out any inter-extension bus events this command emitted (Pi's EventEmitter dispatch
        // runs the listeners after the emit call, event-bus.ts; gap-08 §5.3) — deferred to here
        // because wasm reentrancy forbids delivering inside the guest's `bus.emit` import. Delivery
        // runs even if the command errored (an emit before the error still fires, Pi-faithfully).
        self.deliver_bus_events(cancel).await;
        out
    }

    /// Dynamic argument completions for a guest command (Pi `getArgumentCompletions`).
    #[cfg(feature = "wasm-host")]
    pub async fn command_completions(
        &self,
        name: &str,
        prefix: &str,
    ) -> Result<Vec<String>, ExtError> {
        let ext = self.live_for_command(name)?;
        ext.argument_completions(name, prefix).await
    }

    #[cfg(feature = "wasm-host")]
    fn live_for_command(&self, name: &str) -> Result<Arc<crate::host::LiveExtension>, ExtError> {
        let owner = self
            .registry
            .command_owner(name)?
            .ok_or_else(|| ExtError::Component(format!("no such command: {name}")))?;
        self.live
            .read()
            .ok()
            .and_then(|g| g.get(&owner).cloned())
            .ok_or_else(|| ExtError::Component(format!("command `{name}` has no live owner")))
    }

    /// Every key-id an extension has registered a keyboard shortcut for (R-08-017; Pi
    /// `registerShortcut`). The L6 TUI reads this at boot / on rebind so a matching key press routes
    /// to [`ExtensionHost::run_shortcut`] instead of the editor. Registry-backed, so it is available
    /// with or without the `wasm-host` feature (an empty list when nothing is registered).
    pub fn shortcut_keys(&self) -> Vec<String> {
        self.registry.shortcut_keys().unwrap_or_default()
    }

    /// Execute the extension-registered keyboard shortcut bound to `key` (R-08-017; Pi
    /// `registerShortcut` handler). Resolves the owning live extension from the registry and runs its
    /// [`crate::host::LiveExtension::execute_shortcut`] at command tier. An unregistered key or a
    /// shortcut with no live owner is a typed `ExtError`.
    #[cfg(feature = "wasm-host")]
    pub async fn run_shortcut(&self, key: &str, cancel: &CancelToken) -> Result<(), ExtError> {
        let owner = self
            .registry
            .shortcut_owner(key)?
            .ok_or_else(|| ExtError::Component(format!("no such shortcut: {key}")))?;
        let ext = self
            .live
            .read()
            .ok()
            .and_then(|g| g.get(&owner).cloned())
            .ok_or_else(|| ExtError::Component(format!("shortcut `{key}` has no live owner")))?;
        let out = ext.execute_shortcut(key, cancel).await;
        // Fan out any inter-extension bus events the shortcut handler emitted (gap-08 §5.3).
        self.deliver_bus_events(cancel).await;
        out
    }

    /// Native-host fallback for [`ExtensionHost::run_shortcut`] (no `wasm-host` feature): no live
    /// guest can own a shortcut, so a fired key is a typed error rather than a silent success.
    #[cfg(not(feature = "wasm-host"))]
    pub async fn run_shortcut(&self, key: &str, _cancel: &CancelToken) -> Result<(), ExtError> {
        Err(ExtError::Component(format!("shortcut `{key}` has no live owner (wasm-host disabled)")))
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
            .dispatch_notify(&HostEvent::SessionShutdown { reason: "reload".into() }, cancel)
            .await;
        // 2) cache-bust: drop dispatcher entries, registry tables, live instances, loaded ids.
        self.dispatcher.clear()?;
        self.registry.clear()?;
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
        let result = self.discover_and_load(roots, project_trusted, services).await;
        // 4) signal start to the fresh set (reason = "reload").
        self.dispatcher
            .dispatch_notify(&HostEvent::SessionStart { reason: "reload".into() }, cancel)
            .await;
        Ok(result)
    }

    /// Per-call epoch budget for a loaded wasm extension. The epoch driver ticks every
    /// [`crate::host::epoch::DEFAULT_TICK`] (5ms); 1000 ticks ≈ 5s before a runaway guest is
    /// preempted (R-ARCH-EXT-012). The dispatcher's invocation budget is a coarser backstop.
    #[cfg(feature = "wasm-host")]
    const WASM_EPOCH_BUDGET_TICKS: u64 = 1000;

    /// Names of every registered native command (diagnostics / completion). A subset of
    /// [`ExtensionRegistry::command_names`] limited to native-owned commands.
    pub fn native_command_names(&self) -> Vec<String> {
        let native = match self.native.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        self.registry
            .command_names()
            .unwrap_or_default()
            .into_iter()
            .filter(|n| {
                self.registry
                    .command_owner(n)
                    .ok()
                    .flatten()
                    .is_some_and(|o| native.contains_key(&o))
            })
            .collect()
    }

    fn reserve_id(&self, id: &ExtensionId) -> Result<(), ExtError> {
        let mut g = self.loaded.write().map_err(|_| ExtError::Io("host lock poisoned".into()))?;
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
fn native_panic_msg(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "panic".to_string()
    }
}
