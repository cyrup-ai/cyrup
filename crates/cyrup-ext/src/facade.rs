//! The `ExtensionHost` facade (arch-08 §3.1): the single entry point the session service wires in.
//! Holds the registry + dispatcher + native registry (+ the Wasmtime engine/pool when the
//! `wasm-host` feature is on). Exposes the two agent seams — [`ExtSubscriber`] (notify) and
//! [`ExtHooks`] (mutating) — plus the merged active tool set.

use crate::contract::{HandledValue, Reduced};
use crate::dispatch::Dispatcher;
use crate::error::ExtError;
use crate::event::{HostEvent, InputEventSource, InputStreamingBehavior};
use crate::hooks::ExtHooks;
use crate::loader::{DiscoveredExtension, DiscoveryRoots, LoadError, LoadExtensionsResult};
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
        ext.set_host_services(services);
        self.load_native_inner(ext).await
    }

    /// The shared native-load body (register tools/commands, build subscriptions, wire into the
    /// dispatcher). [`Self::load_native`] and [`Self::load_native_with_services`] differ only in
    /// whether they first late-bind the host-services slot (P-1).
    async fn load_native_inner(&self, ext: Arc<dyn NativeExtension>) -> Result<(), ExtError> {
        let id = ext.id();
        self.reserve_id(&id)?;

        let mut api = InitApi::new();
        ext.init(&mut api).await?;
        let (subs, tools, commands) = api.into_parts();

        for tool in tools {
            self.registry.register_tool(id.clone(), tool)?;
        }
        for (name, desc) in commands {
            self.registry.register_command(id.clone(), name, desc)?;
        }

        // Keep the native handle for command-tier slash execution (R-08-016) before it is wrapped
        // for event dispatch.
        if let Ok(mut g) = self.native.write() {
            g.insert(id.clone(), ext.clone());
        }
        let ctx = HostCtx::event(self.config.mode, self.config.has_ui, self.config.cwd.clone());
        let handle = Arc::new(NativeHandle::new(ext, subs, ctx));
        self.dispatcher.add(handle)?;
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
    pub fn active_tools(&self, base: &[Arc<dyn Tool>]) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        self.registry.active_tools(base)
    }

    /// Aggregate the `project_trust` decision across extensions (Pi runner.ts:1046; gap-08 #4). The
    /// FIRST extension that returns a parseable `{trusted, remember}` wins; `None` = no decision (the
    /// host falls back to its own trust prompt).
    pub async fn aggregate_project_trust(
        &self,
        cancel: &CancelToken,
    ) -> Option<crate::ProjectTrustDecision> {
        use crate::event::HostEvent;
        let handled =
            self.dispatcher.dispatch_collect_handled(&HostEvent::ProjectTrust, cancel).await;
        crate::fold_project_trust(&handled)
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

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    pub fn dispatcher(&self) -> &Dispatcher {
        &self.dispatcher
    }

    /// Apply CLI-captured extension-flag overrides (Pi `applyExtensionFlagValues`,
    /// agent-session-services.ts:84-113). Called AFTER extensions load (so their `registerFlag`
    /// specs are known). For each captured `(name, value)`:
    /// - no loaded extension registered `name` ⇒ ignored (Pi records an "Unknown option" diagnostic
    ///   and skips — cyrup drops it silently, the same no-effect outcome);
    /// - the registered flag's `type` is `boolean` ⇒ the stored value is `true` regardless of the
    ///   token's value (Pi agent-session-services.ts:109);
    /// - the registered flag's `type` is `string` and the token carried a value ⇒ that string (Pi
    ///   :113); a bare `--flag` on a string flag is skipped (Pi's "requires a value" diagnostic).
    ///
    /// The resolved value lands in the shared flag store so a guest's `getFlag(name)` reads the CLI
    /// value AHEAD of its registered default (gap-08 §5.6 — the step that was missing, so the
    /// 1:1-ported CLI capture used to be dropped one call short of `getFlag`).
    pub fn apply_extension_flag_values(
        &self,
        flags: &[(String, ExtensionFlagOverride)],
    ) -> Result<(), ExtError> {
        for (name, value) in flags {
            let spec = match self.registry.get_flag(name)? {
                Some(s) => s,
                // Unregistered flag: no extension owns it — ignored (Pi unknownFlags diagnostic).
                None => continue,
            };
            let is_boolean = spec.get("type").and_then(|t| t.as_str()) == Some("boolean");
            let resolved = match (is_boolean, value) {
                (true, _) => Value::Bool(true),
                (false, ExtensionFlagOverride::Str(s)) => Value::String(s.clone()),
                // A string-typed flag passed with no value: Pi emits "requires a value" and does not
                // set it — skip so the registered default stands.
                (false, ExtensionFlagOverride::Bool(_)) => continue,
            };
            self.registry.set_flag_value(name.clone(), resolved)?;
        }
        Ok(())
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
                .with_bus(self.bus.clone()),
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
        // Surface the guest's registered tools as executable handles in the active set: each runs
        // by dispatching `execute-tool` back into this live instance (R-08-012/014/015).
        for desc in self.registry.guest_tool_descriptors()? {
            let tool: Arc<dyn Tool> = Arc::new(crate::host::WasmTool::new(ext.clone(), desc));
            self.registry.register_tool(id.clone(), tool)?;
        }
        self.dispatcher.add(ext.clone())?;
        if let Ok(mut g) = self.live.write() {
            g.insert(id, ext.clone());
        }
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
                Err(e) => result.errors.push(LoadError { path: disc.dir.clone(), error: e.to_string() }),
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
