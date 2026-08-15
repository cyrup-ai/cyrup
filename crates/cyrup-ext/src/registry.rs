//! The extension registry (arch-08 §3.5, registry/*): the tables of tools / commands / shortcuts /
//! flags / providers / renderers an extension contributes. Tool registration overrides a built-in
//! of the same name (R-08-012); the merged active set is handed to the agent each run (R-08-014).

use crate::error::ExtError;
use crate::provider::{ModelRegistrySink, ProviderHub, ProviderRegistration};
use cyrup_core::{ExecMode, ExtensionId, Tool};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// What a guest sends to register a tool (arch-08 §3.5). `parameters` stays JSON-Schema (Pi-interop,
/// R-ARCH-EXT-008). camelCase per arch-00 §4.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDescriptor {
    pub name: String,
    pub label: String,
    pub description: String,
    /// JSON-Schema for the tool parameters.
    pub parameters: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_mode: Option<ExecModeWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    #[serde(default)]
    pub prompt_guidelines: Vec<String>,
    #[serde(default)]
    pub has_renderer: bool,
    /// pi `ToolDefinition.prepareArguments?: (args: unknown) => Static<TParams>`
    /// (`extensions/types.ts:468` @v0.83.0), run BEFORE `validateToolArguments` in
    /// `packages/agent/src/agent-loop.ts`. A function cannot cross the component boundary, so the
    /// descriptor carries the flag and the host calls the guest's `prepare-arguments` export when
    /// it is set (EXT-023). Before this the whole field stopped at the SDK struct: the SDK accepted
    /// `prepare_arguments`, documented it as "the host coerces args before validation when set",
    /// and `lower_tool_descriptor` copied 8 of 10 fields — struct-literal construction of a
    /// different type, so no compile error and no warning.
    #[serde(default)]
    pub prepare_arguments: bool,
    /// pi `ToolDefinition.renderShell?: "default" | "self"` (`extensions/types.ts:465` @v0.83.0):
    /// "Controls whether ToolExecutionComponent renders the standard colored shell or the tool
    /// renders its own framing." `None` is upstream's omitted field, i.e. `"default"` (EXT-024).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render_shell: Option<String>,
    /// pi `ToolDefinition.constrainedSampling?: false | ConstrainedSamplingConfig`
    /// (`extensions/types.ts:463` @v0.83.0). Copied verbatim onto the runtime tool upstream by
    /// `wrapToolDefinition` (`core/tools/tool-definition-wrapper.ts:14`) and read back off
    /// `Context.tools` by the provider adapters; here it is surfaced by
    /// `<WasmTool as Tool>::constrained_sampling` so the agent loop can forward it. Stored PARSED
    /// rather than as the raw JSON string so a malformed declaration is rejected once, at
    /// registration, instead of on every turn (PROV-011 / EXT-024). `None` = the omitted field,
    /// which upstream is indistinguishable from `false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constrained_sampling: Option<cyrup_core::ConstrainedSampling>,
}

impl ToolDescriptor {
    /// Validate the descriptor at registration (R-ARCH-EXT-008): `name` non-empty and `parameters`
    /// is a JSON object (a minimal JSON-Schema validity check; full schema validation is a follow-on).
    pub fn validate(&self) -> Result<(), ExtError> {
        if self.name.trim().is_empty() {
            return Err(ExtError::Schema("tool name is empty".into()));
        }
        if !self.parameters.is_object() {
            return Err(ExtError::Schema(format!(
                "tool `{}` parameters must be a JSON-Schema object",
                self.name
            )));
        }
        Ok(())
    }
}

/// Wire form of `ExecMode` (serde camelCase).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExecModeWire {
    Parallel,
    Sequential,
}

impl From<ExecModeWire> for ExecMode {
    fn from(w: ExecModeWire) -> Self {
        match w {
            ExecModeWire::Parallel => ExecMode::Parallel,
            ExecModeWire::Sequential => ExecMode::Sequential,
        }
    }
}

/// A registered command descriptor (R-08-016). The handler runs with a command-tier ctx.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandDescriptor {
    pub description: String,
    #[serde(default)]
    pub completions: Vec<String>,
}

/// A command after invocation-name disambiguation (Pi `ResolvedCommand`, runner.ts:556-595). When two
/// extensions register the same `name`, Pi assigns `name:1`/`name:2` suffixes in LOAD ORDER (the
/// `invocation_name`) while keeping the original `name`; a unique name keeps its bare `name`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedCommand {
    /// The disambiguated name the user invokes (`name` or `name:N`).
    pub invocation_name: String,
    /// The original registered command name.
    pub name: String,
    /// The extension that registered it.
    pub owner: ExtensionId,
    pub descriptor: CommandDescriptor,
}

/// One extension-name collision, in Pi's `detectExtensionConflicts` shape
/// (`coding-agent/src/core/resource-loader.ts:1059-1094`): `{path, message}`, where `path` is the
/// extension whose registration LOST (the later one in load order) and `message` names the
/// extension that already owns the name.
///
/// Pi walks the loaded-extension list AFTER loading and emits one record per losing registration;
/// cyrup detects the same collisions at registration time (registrations stream in through
/// [`ExtensionRegistry::register_tool`] / [`ExtensionRegistry::register_guest_tool`] /
/// [`ExtensionRegistry::register_flag`] rather than landing in per-extension maps), which yields the
/// same set in the same order. `path` is an [`ExtensionId`] because a cyrup native built-in has no
/// filesystem path — Pi's own inline extensions likewise carry a synthetic `<inline:…>` path.
///
/// These records are folded into [`crate::LoadExtensionsResult::errors`] by
/// [`crate::ExtensionHost::discover_and_load`], exactly as Pi's `addExtensionConflictDiagnostics`
/// pushes them onto `extensionsResult.errors` (`resource-loader.ts:625-632`) — which `main.ts:735-738`
/// renders as `Failed to load extension "<path>": <message>` and exits 1 on (`main.ts:843-848`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtensionConflict {
    /// The extension whose registration was rejected (Pi's `conflicts[].path`).
    pub path: ExtensionId,
    /// Pi's verbatim message: `Tool "<name>" conflicts with <owner>` /
    /// `Flag "--<name>" conflicts with <owner>`.
    pub message: String,
}

/// The registry of everything extensions contribute. `Send + Sync` (interior `RwLock`), shared via
/// `Arc` across the host (arch-08 §3.1).
#[derive(Default)]
pub struct ExtensionRegistry {
    inner: RwLock<RegistryInner>,
    /// Set whenever a tool registration lands (host tool OR guest descriptor) — Pi's
    /// `registerTool()` ends with `runtime.refreshTools()` on EVERY registration
    /// (extensions/loader.ts:249-256). cyrup cannot mint the executable `Arc<dyn Tool>` inside the
    /// guest's `register-tool` import (the `LiveExtension` does not exist yet during `init`, and the
    /// store is borrowed during a later call), so the host instead marks the tool set DIRTY here and
    /// re-materializes at [`crate::ExtensionHost::refresh_tools`]. Outside the `RwLock` so the check
    /// is a relaxed atomic load, not a lock acquisition (EXT-004).
    tools_dirty: std::sync::atomic::AtomicBool,
}

#[derive(Default)]
struct RegistryInner {
    /// Extension tools keyed by name. FIRST registering extension wins (Pi `getAllRegisteredTools` /
    /// `getToolDefinition`, runner.ts:450-471 — both loop `this.extensions` in load order and take
    /// the first hit); a re-registration by the SAME extension replaces, matching Pi's per-extension
    /// `extension.tools.set(tool.name, …)` (loader.ts:245-252). Insertion order kept.
    tool_order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Which extension owns each tool name (for diagnostics / unload).
    tool_owner: HashMap<String, ExtensionId>,
    commands: HashMap<String, (ExtensionId, CommandDescriptor)>,
    /// Every command registration in LOAD ORDER (Pi preserves load order for invocation-name
    /// disambiguation, runner.ts:559-565). The `commands` map is the fast last-wins lookup; this Vec
    /// retains duplicates + order so [`ExtensionRegistry::resolved_commands`] can assign `name:N`.
    command_order: Vec<(ExtensionId, String, CommandDescriptor)>,
    /// Registered shortcuts, keyed by the RAW key id the extension declared, carrying the owner and
    /// pi's optional `description` (EXT-040). Kept raw and unfiltered on purpose: this is the analog
    /// of pi's per-extension `ext.shortcuts` map, which `getShortcuts` reads and normalizes at
    /// RESOLUTION time (`extensions/runner.ts:492-534` @v0.83.0) rather than at registration.
    shortcuts: HashMap<String, (ExtensionId, Option<String>)>,
    /// Registration ORDER of `shortcuts`, so [`ExtensionRegistry::resolve_shortcuts`] can walk
    /// them the way pi walks `this.extensions` — load order, last-wins with a warning.
    shortcut_order: Vec<String>,
    /// Commands opted into argument autocomplete, in registration order (pi
    /// `addAutocompleteProvider`'s per-command sibling; the WASM tier records the same thing on
    /// `GuestState`). Registry-backed so a NATIVE can reach it too (EXT-035).
    command_autocomplete: Vec<(ExtensionId, String)>,
    /// Count of stacked GLOBAL autocomplete providers (pi `addAutocompleteProvider`,
    /// `extensions/types.ts:218`) contributed by natives (EXT-035).
    autocomplete_providers: Vec<ExtensionId>,
    /// Warnings produced by the last [`ExtensionRegistry::resolve_shortcuts`] call (pi
    /// `getShortcutDiagnostics()`, `extensions/runner.ts:538-540` @v0.83.0), which upstream folds
    /// into the `[Extension issues]` startup panel (`interactive-mode.ts:1612-1618`).
    shortcut_diagnostics: Vec<ExtensionConflict>,
    flags: HashMap<String, Value>,
    /// Which extension owns each flag name. FIRST registration wins (Pi `getFlags`,
    /// runner.ts:473-483 — `if (!allFlags.has(name))` over `this.extensions` in load order).
    flag_owner: HashMap<String, ExtensionId>,
    /// Name collisions between DIFFERENT extensions, in load order (Pi `detectExtensionConflicts`,
    /// resource-loader.ts:1059-1094). Accumulated as registrations arrive; read back by
    /// [`ExtensionRegistry::conflicts`].
    conflicts: Vec<ExtensionConflict>,
    /// CLI-supplied flag VALUE overrides (Pi `runtime.flagValues` entries set by
    /// `applyExtensionFlagValues`, agent-session-services.ts:102-114). ONE shared map keyed by flag
    /// name (Pi's single `runtime.flagValues`), consulted by `GuestState::get_flag` AHEAD of the
    /// registered default so a `--flag=value` the CLI captured is what a guest's `getFlag` reads
    /// (gap-08 §5.6). Empty until [`crate::ExtensionHost::apply_extension_flag_values`] runs.
    flag_values: HashMap<String, Value>,
    /// Custom-provider registrations: typed, api-key-resolved, deferred→bind→flush (A-08-7).
    provider_hub: ProviderHub,
    /// Which extension owns each provider id (for diagnostics / unload).
    provider_owner: HashMap<String, ExtensionId>,
    /// Guest (WASM) tool descriptors keyed by name. A guest tool executes back across the boundary
    /// (gap-08 #28), so it is held as a descriptor here rather than as an `Arc<dyn Tool>`.
    guest_tool_order: Vec<String>,
    guest_tools: HashMap<String, (ExtensionId, ToolDescriptor)>,
    /// Which extension owns the RENDERER for a tool name (`ToolDescriptor.has_renderer` =>
    /// Pi's per-tool `renderCall`/`renderResult`, extensions/types.ts:489-497, resolved by
    /// `modes/interactive/components/tool-execution.ts:81-112`). Populated by
    /// [`ExtensionRegistry::register_guest_tool`]; read by
    /// [`crate::ExtensionHost::render_tool_call`]/[`crate::ExtensionHost::render_tool_result`] to
    /// route a tool NAME back to the guest that can render it (EXT-006). Without this table
    /// `has_renderer` was a field nothing read.
    tool_renderer_owner: HashMap<String, ExtensionId>,
    /// Which extension owns the CUSTOM-MESSAGE renderer for a custom type (Pi
    /// `registerMessageRenderer(customType, renderer)`, extensions/types.ts:1284, resolved by
    /// `extensions/runner.ts:579-587 getMessageRenderer` — FIRST extension in load order wins).
    /// Populated by [`ExtensionRegistry::register_message_renderer`]; read by
    /// [`crate::ExtensionHost::render_message_call`]/[`crate::ExtensionHost::render_message_result`].
    message_renderer_owner: HashMap<String, ExtensionId>,
    /// Which extension owns the custom-ENTRY renderer for a custom type (Pi
    /// `registerEntryRenderer(customType, renderer)`, extensions/types.ts:1295, implemented at
    /// `loader.ts:314-318`, resolved by `extensions/runner.ts:593-600 getEntryRenderer` — FIRST
    /// extension in load order wins, exactly like `getMessageRenderer`).
    ///
    /// A SEPARATE table from [`Self::message_renderer_owner`] because upstream keeps two disjoint
    /// maps (`extension.messageRenderers` vs `extension.entryRenderers`, types.ts:1703-1704) and
    /// their consumers render DIFFERENT things: a custom MESSAGE draws `CustomMessageComponent`
    /// (which swallows a renderer throw, `custom-message.ts:82-84`), a custom ENTRY draws
    /// `CustomEntryComponent` (which draws a failure box on a throw, `custom-entry.ts:47-52`). Same
    /// custom type may legitimately be claimed by different extensions on the two surfaces.
    entry_renderer_owner: HashMap<String, ExtensionId>,
    /// Extensions that registered a markdown transformer, in LOAD ORDER (EXT-019).
    ///
    /// pi stores AT MOST ONE per extension — `extension.markdownTransformer = transformer`
    /// (`extensions/loader.ts:309-312` @v0.84.1, field at `types.ts:1703`) — and collects them with
    /// `getMarkdownTransformers(): this.extensions.flatMap(ext => ext.markdownTransformer ? [..] :
    /// [])` (`runner.ts:589-591`), so the fold order IS extension load order. A `Vec` (not a map)
    /// for exactly that reason; re-registration by the same owner is idempotent rather than a
    /// second fold step, because upstream's field ASSIGNMENT replaces rather than appends.
    markdown_transformers: Vec<ExtensionId>,
    /// Extensions subscribed to raw terminal input, in LOAD order (EXT-021; pi
    /// `ExtensionUIContext.onTerminalInput`, `extensions/types.ts:145` @v0.83.0).
    ///
    /// pi keeps the handlers in an insertion-ordered `Set` (`packages/tui/src/tui.ts:651-655`) and
    /// folds them in that order (`:773-788`), so a `Vec` reproduces both the ordering and the
    /// `Set.add` idempotence. `unsubscribe` is `Set.delete` and is likewise idempotent.
    terminal_input_subscribers: Vec<ExtensionId>,
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an extension tool. A `PoisonError` degrades to a surfaced error, never a panic
    /// (R-00-009).
    ///
    /// **First registering extension wins.** Pi resolves an extension tool by looping the loaded
    /// extensions in LOAD ORDER and returning the first hit — `getAllRegisteredTools`
    /// (`extensions/runner.ts:450-460`, `if (!toolsByName.has(...))`) supplies the executable
    /// definitions that `agent-session.ts:2463-2487 _refreshToolRegistry` puts in `_toolRegistry`,
    /// and `getToolDefinition` (`runner.ts:463-471`) resolves by the same rule. So the extension that
    /// loaded FIRST both appears in the tool list and is the one that RUNS. A later extension
    /// claiming the same name is rejected here and recorded as an [`ExtensionConflict`] (Pi
    /// `detectExtensionConflicts`, resource-loader.ts:1059-1094).
    ///
    /// A re-registration by the SAME owner still replaces: Pi's per-extension map is a plain
    /// `extension.tools.set(tool.name, …)` (`extensions/loader.ts:245-252`), so an extension that
    /// re-registers its own tool (hot-reload, or the descriptor→`WasmTool` materialization pass)
    /// overwrites its previous entry.
    pub fn register_tool(&self, owner: ExtensionId, tool: Arc<dyn Tool>) -> Result<(), ExtError> {
        self.register_tool_inner(owner, tool, true)
    }

    /// [`Self::register_tool`] WITHOUT raising the tools-dirty flag (EXT-030).
    ///
    /// The one legitimate caller is the guest-descriptor materializer
    /// ([`crate::ExtensionHost::refresh_tools`]), which is running *because* the flag was already
    /// taken: its own re-registrations are not new signal, and letting them raise the flag forced
    /// the materializer to clear it again on the way out — a wholesale
    /// `take_tools_dirty()` swap that also discarded its own deliberate re-arm for a
    /// descriptor whose owner was not yet live, and any mark raised concurrently by another
    /// extension. pi has no dirty flag to lose a signal in: `registerTool` ends with
    /// `runtime.refreshTools()` on every registration
    /// (pi/packages/coding-agent/src/core/extensions/loader.ts:245-252 @v0.83.0) and
    /// `_refreshToolRegistry` rebuilds the whole registry each time
    /// (core/agent-session.ts:2452-2546), so this quiet variant is what keeps cyrup's cheaper
    /// flag-gated equivalent from dropping a registration that pi could not drop.
    pub fn register_materialized_tool(
        &self,
        owner: ExtensionId,
        tool: Arc<dyn Tool>,
    ) -> Result<(), ExtError> {
        self.register_tool_inner(owner, tool, false)
    }

    fn register_tool_inner(
        &self,
        owner: ExtensionId,
        tool: Arc<dyn Tool>,
        mark_dirty: bool,
    ) -> Result<(), ExtError> {
        let name = tool.name().to_string();
        let mut g = self.lock_write()?;
        if let Some(existing) = Self::tool_owner_in(&g, &name)
            && existing != owner
        {
            Self::record_conflict(&mut g, owner, format!("Tool \"{name}\" conflicts with {existing}"));
            return Ok(());
        }
        if !g.tools.contains_key(&name) {
            g.tool_order.push(name.clone());
        }
        g.tool_owner.insert(name.clone(), owner);
        g.tools.insert(name, tool);
        drop(g);
        if mark_dirty {
            self.mark_tools_dirty();
        }
        Ok(())
    }

    /// The extension that already owns tool `name`, across BOTH tool tables: the executable
    /// `Arc<dyn Tool>` map and the not-yet-materialized guest descriptor map. Pi has ONE
    /// `extension.tools` map per extension holding both kinds, so the first-wins rule must see them
    /// as one namespace — otherwise a guest descriptor and a native tool of the same name would each
    /// "win" in their own table.
    fn tool_owner_in(g: &RegistryInner, name: &str) -> Option<ExtensionId> {
        g.tool_owner
            .get(name)
            .or_else(|| g.guest_tools.get(name).map(|(o, _)| o))
            .cloned()
    }

    /// Append a conflict record, de-duplicated. Pi emits one record per losing registration from a
    /// single post-load sweep; cyrup sees registrations streaming in and a retryable path (the guest
    /// descriptor re-materializer) can re-offer the same losing registration, so identical records
    /// collapse to keep the diagnostic list Pi-shaped.
    fn record_conflict(g: &mut RegistryInner, path: ExtensionId, message: String) {
        let record = ExtensionConflict { path, message };
        if !g.conflicts.contains(&record) {
            g.conflicts.push(record);
        }
    }

    /// Every name collision between two DIFFERENT extensions, in load order (Pi
    /// `detectExtensionConflicts`, resource-loader.ts:1059-1094).
    pub fn conflicts(&self) -> Result<Vec<ExtensionConflict>, ExtError> {
        Ok(self.lock_read()?.conflicts.clone())
    }

    /// Mark the tool set as changed (Pi's `runtime.refreshTools()` trigger, loader.ts:249-256).
    /// Consumed by [`Self::take_tools_dirty`].
    pub fn mark_tools_dirty(&self) {
        self.tools_dirty.store(true, std::sync::atomic::Ordering::Release);
    }

    /// Take-and-clear the "tools changed since the last refresh" flag (EXT-004). `true` = at least
    /// one tool registration landed since the previous call.
    pub fn take_tools_dirty(&self) -> bool {
        self.tools_dirty.swap(false, std::sync::atomic::Ordering::AcqRel)
    }

    /// Register a guest (WASM) tool by descriptor (R-08-012). Same precedence rule as
    /// [`Self::register_tool`] — the FIRST extension to claim a tool name wins (Pi
    /// `getAllRegisteredTools`/`getToolDefinition`, runner.ts:450-471), a later claimant is rejected
    /// and recorded as an [`ExtensionConflict`], and the SAME owner re-registering replaces.
    /// Insertion order is preserved for stable active-set surfacing.
    pub fn register_guest_tool(
        &self,
        owner: ExtensionId,
        desc: ToolDescriptor,
    ) -> Result<(), ExtError> {
        desc.validate()?;
        let name = desc.name.clone();
        let mut g = self.lock_write()?;
        if let Some(existing) = Self::tool_owner_in(&g, &name)
            && existing != owner
        {
            Self::record_conflict(&mut g, owner, format!("Tool \"{name}\" conflicts with {existing}"));
            return Ok(());
        }
        if !g.guest_tools.contains_key(&name) {
            g.guest_tool_order.push(name.clone());
        }
        // A descriptor that declares `has_renderer` makes its owner the renderer for that TOOL name
        // (Pi's per-tool `renderCall`/`renderResult`, types.ts:489-497) — EXT-006.
        if desc.has_renderer {
            g.tool_renderer_owner.insert(name.clone(), owner.clone());
        } else {
            g.tool_renderer_owner.remove(&name);
        }
        g.guest_tools.insert(name, (owner, desc));
        drop(g);
        self.mark_tools_dirty();
        Ok(())
    }

    /// Record a per-TOOL renderer registration for a tool that is NOT described by a guest
    /// [`ToolDescriptor`] — i.e. a NATIVE extension's tool, which arrives as an already-executable
    /// `Arc<dyn Tool>` and therefore carries no `has_renderer` flag (Pi does not distinguish:
    /// `ToolDefinition.renderCall` is declared the same way whichever runtime supplies the tool,
    /// extensions/types.ts:489-497).
    ///
    /// **FIRST registration wins** (EXT-056), like every sibling table. Upstream has no separate
    /// tool-renderer table at all: `renderCall`/`renderResult` ride on the tool's own
    /// `ToolDefinition` and are resolved by `getToolDefinition`, which returns the FIRST extension
    /// in load order whose `ext.tools` map has the name
    /// (pi/packages/coding-agent/src/core/extensions/runner.ts:463-471 @v0.83.0) — whoever wins the
    /// TOOL wins its renderer. The previous last-wins rule let a later extension re-point rendering
    /// to an extension that had lost (or never made) the tool registration, so the tool executed as
    /// one extension's and drew as another's. Its stated justification — "matching
    /// `register_guest_tool`'s descriptor path" — stopped holding when EXT-008 made that path
    /// early-return on a foreign owner before it touches `tool_renderer_owner`. A second owner
    /// claiming the same tool name is recorded as an [`ExtensionConflict`] so the drop is
    /// diagnosable.
    pub fn register_tool_renderer(
        &self,
        owner: ExtensionId,
        tool_name: impl Into<String>,
    ) -> Result<(), ExtError> {
        let tool_name = tool_name.into();
        let mut g = self.lock_write()?;
        if let Some(existing) = g.tool_renderer_owner.get(&tool_name)
            && *existing != owner
        {
            let existing = existing.clone();
            Self::record_conflict(
                &mut g,
                owner,
                format!("Tool renderer for \"{tool_name}\" conflicts with {existing}"),
            );
            return Ok(());
        }
        g.tool_renderer_owner.insert(tool_name, owner);
        Ok(())
    }

    /// The extension that renders the tool named `name` (`ToolDescriptor.has_renderer`), if any.
    pub fn tool_renderer_owner(&self, name: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.tool_renderer_owner.get(name).cloned())
    }

    /// Record a custom-MESSAGE renderer registration (Pi `registerMessageRenderer(customType, …)`,
    /// types.ts:1284). FIRST registration wins, matching Pi's load-order `getMessageRenderer` loop
    /// (runner.ts:579-587) which returns the first extension that declares the type.
    pub fn register_message_renderer(
        &self,
        owner: ExtensionId,
        custom_type: impl Into<String>,
    ) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.message_renderer_owner.entry(custom_type.into()).or_insert(owner);
        Ok(())
    }

    /// The extension that renders custom messages of `custom_type` (first-wins), if any.
    /// Record that `owner` registered a markdown transformer (EXT-019; pi
    /// `registerMarkdownTransformer`, `extensions/loader.ts:309-312` @v0.84.1). Idempotent per
    /// owner: upstream ASSIGNS `extension.markdownTransformer`, so a second registration by the
    /// same extension replaces its single transformer rather than adding a fold step.
    pub fn register_markdown_transformer(&self, owner: ExtensionId) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        if !g.markdown_transformers.contains(&owner) {
            g.markdown_transformers.push(owner);
        }
        Ok(())
    }

    /// Extensions with a markdown transformer, in load order (pi
    /// `ExtensionRunner.getMarkdownTransformers()`, `extensions/runner.ts:589-591` @v0.84.1).
    pub fn markdown_transformer_owners(&self) -> Result<Vec<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.markdown_transformers.clone())
    }

    /// Record that `owner` subscribed to raw terminal input (EXT-021; pi
    /// `onTerminalInput(handler)`, `extensions/types.ts:145` @v0.83.0). Idempotent, matching
    /// upstream's `Set.add`.
    pub fn subscribe_terminal_input(&self, owner: ExtensionId) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        if !g.terminal_input_subscribers.contains(&owner) {
            g.terminal_input_subscribers.push(owner);
        }
        Ok(())
    }

    /// The unsubscribe function upstream's `onTerminalInput` returns (`Set.delete`,
    /// `packages/tui/src/tui.ts:652-654`). Idempotent; unsubscribing a non-subscriber is a no-op.
    pub fn unsubscribe_terminal_input(&self, owner: &ExtensionId) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.terminal_input_subscribers.retain(|o| o != owner);
        Ok(())
    }

    /// Terminal-input subscribers in LOAD order — the fold order of pi's `TUI.handleInput`
    /// (`packages/tui/src/tui.ts:773-788`).
    pub fn terminal_input_subscribers(&self) -> Result<Vec<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.terminal_input_subscribers.clone())
    }

    pub fn message_renderer_owner(&self, custom_type: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.message_renderer_owner.get(custom_type).cloned())
    }

    /// Record a custom-ENTRY renderer registration (Pi `registerEntryRenderer(customType, …)`,
    /// types.ts:1295 / loader.ts:314-318). FIRST registration wins, matching Pi's load-order
    /// `getEntryRenderer` loop (runner.ts:593-600).
    pub fn register_entry_renderer(
        &self,
        owner: ExtensionId,
        custom_type: impl Into<String>,
    ) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.entry_renderer_owner.entry(custom_type.into()).or_insert(owner);
        Ok(())
    }

    /// The extension that renders custom ENTRIES of `custom_type` (first-wins), if any.
    pub fn entry_renderer_owner(&self, custom_type: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.entry_renderer_owner.get(custom_type).cloned())
    }

    /// All registered tool names with Pi's **first-registration-wins** ordering (`getAllRegisteredTools`,
    /// runner.ts:417; gap-08 #7). `tool_order`/`guest_tool_order` are both first-insert order (a later
    /// override updates the value but keeps the original position), so the union — extension tools
    /// then guest-only tools, de-duplicated first-wins — is exactly Pi's getter order. Execution
    /// resolves first-wins too ([`Self::register_tool`] rejects a second extension's claim on the
    /// name), so the getter and the executed tool always agree, as they do in Pi where both read the
    /// same load-ordered `this.extensions` loop.
    pub fn all_registered_tool_names(&self) -> Result<Vec<String>, ExtError> {
        let g = self.lock_read()?;
        let mut out: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for n in g.tool_order.iter().chain(g.guest_tool_order.iter()) {
            if seen.insert(n.as_str()) {
                out.push(n.clone());
            }
        }
        Ok(out)
    }

    /// `ToolInfo[]` for every registered tool (Pi `getAllTools`, `extensions/types.ts:1323`,
    /// implemented at `core/agent-session.ts:906-914` @v0.83.0). First-registration-wins order
    /// (matches [`Self::all_registered_tool_names`]).
    ///
    /// EXT-060 — the emitted object is EXACTLY pi's five keys:
    /// `ToolInfo = Pick<ToolDefinition, "name"|"description"|"parameters"|"promptGuidelines"> &
    /// {sourceInfo}` (`extensions/types.ts:1552-1554` @v0.83.0). It previously also carried a
    /// cyrup-invented `source: "extension"|"guest"` discriminator, which leaked cyrup's
    /// native-vs-WASM TIER onto a guest-facing parity surface — a distinction pi's one-extension-kind
    /// model has no word for, and one a guest could read and branch on. Note that pi DOES have a
    /// `source` on the sibling command info (`SlashCommandSource = "extension"|"prompt"|"skill"`,
    /// `core/slash-commands.ts:4`), which is what makes the key look plausible here; it is not the
    /// same field and does not license one. The tier a tool runs in is already recoverable from
    /// `sourceInfo` if a guest genuinely needs provenance.
    pub fn tool_info(&self) -> Result<Vec<Value>, ExtError> {
        let g = self.lock_read()?;
        let mut out: Vec<Value> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for n in &g.tool_order {
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some(t) = g.tools.get(n) {
                out.push(serde_json::json!({
                    "name": t.name(),
                    "description": t.description(),
                    "parameters": t.parameters(),
                    // EXT-038: pi's `ToolInfo` is
                    // `Pick<ToolDefinition, "name"|"description"|"parameters"|"promptGuidelines"> &
                    // {sourceInfo}` (`extensions/types.ts:1552-1554` @v0.83.0 — re-verified this
                    // pass; `:1551` is the doc comment and the type body runs `:1552-1554`, so the
                    // `:1551-1553` this line carried disagreed with the EXT-060 doc directly above
                    // it on the same type), produced by
                    // `getAllTools()` at `core/agent-session.ts:906-914`. Both trailing fields were
                    // simply absent, so an extension reading this API could not see the guidelines
                    // a tool contributes to the system prompt, nor where a tool came from.
                    "promptGuidelines": t.prompt_guidelines(),
                    "sourceInfo": tool_source_info(g.tool_owner.get(n)),
                }));
            }
        }
        for n in &g.guest_tool_order {
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some((owner, d)) = g.guest_tools.get(n) {
                out.push(serde_json::json!({
                    "name": d.name,
                    "description": d.description,
                    "parameters": d.parameters,
                    "promptGuidelines": d.prompt_guidelines,
                    "sourceInfo": tool_source_info(Some(owner)),
                }));
            }
        }
        Ok(out)
    }

    /// All guest tool descriptors in registration order.
    pub fn guest_tool_descriptors(&self) -> Result<Vec<ToolDescriptor>, ExtError> {
        let g = self.lock_read()?;
        Ok(g.guest_tool_order.iter().filter_map(|n| g.guest_tools.get(n).map(|(_, d)| d.clone())).collect())
    }

    /// All guest tool descriptors in registration order, each with its OWNING extension id — what
    /// [`crate::ExtensionHost::refresh_tools`] needs to wrap a descriptor into an executable
    /// `WasmTool` bound to the right live instance (EXT-004).
    pub fn guest_tool_entries(&self) -> Result<Vec<(ExtensionId, ToolDescriptor)>, ExtError> {
        let g = self.lock_read()?;
        Ok(g.guest_tool_order
            .iter()
            .filter_map(|n| g.guest_tools.get(n).map(|(o, d)| (o.clone(), d.clone())))
            .collect())
    }

    /// Whether a guest tool with this name is registered.
    pub fn has_guest_tool(&self, name: &str) -> Result<bool, ExtError> {
        Ok(self.lock_read()?.guest_tools.contains_key(name))
    }

    pub fn register_command(
        &self,
        owner: ExtensionId,
        name: impl Into<String>,
        desc: CommandDescriptor,
    ) -> Result<(), ExtError> {
        let name = name.into();
        let mut g = self.lock_write()?;
        g.commands.insert(name.clone(), (owner.clone(), desc.clone()));
        g.command_order.push((owner, name, desc));
        Ok(())
    }

    /// Resolve every registered command with Pi's invocation-name disambiguation (Pi
    /// `resolveRegisteredCommands`, runner.ts:556-595; gap-08 #11). Duplicate `name`s across
    /// extensions get `name:1`/`name:2` suffixes assigned in LOAD ORDER; a unique name keeps its bare
    /// form. A collision with an already-taken invocation name bumps the suffix until free (Pi's
    /// `takenInvocationNames` loop).
    pub fn resolved_commands(&self) -> Result<Vec<ResolvedCommand>, ExtError> {
        let g = self.lock_read()?;
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (_, name, _) in &g.command_order {
            *counts.entry(name.as_str()).or_insert(0) += 1;
        }
        let mut seen: HashMap<String, usize> = HashMap::new();
        let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<ResolvedCommand> = Vec::with_capacity(g.command_order.len());
        for (owner, name, descriptor) in &g.command_order {
            let occurrence = {
                let c = seen.entry(name.clone()).or_insert(0);
                *c += 1;
                *c
            };
            let mut invocation_name = if counts.get(name.as_str()).copied().unwrap_or(0) > 1 {
                format!("{name}:{occurrence}")
            } else {
                name.clone()
            };
            if taken.contains(&invocation_name) {
                let mut suffix = occurrence;
                loop {
                    suffix += 1;
                    invocation_name = format!("{name}:{suffix}");
                    if !taken.contains(&invocation_name) {
                        break;
                    }
                }
            }
            taken.insert(invocation_name.clone());
            out.push(ResolvedCommand {
                invocation_name,
                name: name.clone(),
                owner: owner.clone(),
                descriptor: descriptor.clone(),
            });
        }
        Ok(out)
    }

    /// Route an invocation name (bare `name` OR a disambiguated `name:N`) back to its owning
    /// extension, exactly as far as pi routes one.
    ///
    /// 1:1 with `ExtensionRunner.getCommand` (`core/extensions/runner.ts:647-649` @v0.83.0):
    ///
    /// ```text
    /// return this.resolveRegisteredCommands().find((command) => command.invocationName === name);
    /// ```
    ///
    /// `invocationName` is the ONLY key upstream matches on, and there is no second lookup behind
    /// it. SEAM-048 — this used to end with a `self.command_owner(invocation)` fallback into the
    /// last-wins `commands` map, which is unreachable for every name that HAS a resolution (a
    /// registration lands in `commands` and `command_order` together, so `resolved_commands()`
    /// yields a row for each) and wrong for the one case it did catch: after `a` and `b` both
    /// register `deploy`, `resolveRegisteredCommands` emits `deploy:1`/`deploy:2` and NOTHING named
    /// `deploy`, so pi's `getCommand("deploy")` returns `undefined` and `_tryExecuteExtensionCommand`
    /// returns `false` — the bare `/deploy` falls through to a normal prompt
    /// (`core/agent-session.ts:1276-1277`). The fallback instead handed bare `/deploy` to whichever
    /// extension registered LAST, which is precisely the last-registration-wins behaviour the
    /// disambiguation tier exists to remove. Both dispatch gates that read this — the native one via
    /// [`crate::ExtensionHost::execute_native_command`] and `AgentSession::try_execute_wasm_command`
    /// (`cyrup-session-svc/src/session.rs:1131`) — already treat `None` as pi's `false`, so removing
    /// it yields upstream's fall-through at both.
    pub fn resolved_command_owner(&self, invocation: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self
            .resolved_commands()?
            .into_iter()
            .find(|r| r.invocation_name == invocation)
            .map(|r| r.owner))
    }

    /// Register a keyboard shortcut owned by an extension (R-08-017; pi `registerShortcut(shortcut,
    /// {description?, handler})`, `extensions/types.ts:1250` @v0.83.0, storing an
    /// `ExtensionShortcut {shortcut, description?, handler, extensionPath}` at `:1524-1529`).
    ///
    /// Deliberately UNGATED, matching upstream: pi's `registerShortcut` writes straight into the
    /// per-extension `ext.shortcuts` map and every conflict rule — reserved-key refusal,
    /// non-reserved override warning, extension-vs-extension warning — runs later, in `getShortcuts`
    /// against the RESOLVED keybinding config (`extensions/runner.ts:492-534`), which registration
    /// time does not have. [`Self::resolve_shortcuts`] is that function; putting the gate here
    /// instead would refuse keys before knowing what the user bound them to.
    ///
    /// `description` is EXT-040: it crossed the WIT boundary and was thrown away one line inside the
    /// host, so `/hotkeys` printed the key id as its own label.
    pub fn register_shortcut(
        &self,
        owner: ExtensionId,
        key: impl Into<String>,
        description: Option<String>,
    ) -> Result<(), ExtError> {
        let key = key.into();
        let mut g = self.lock_write()?;
        if !g.shortcuts.contains_key(&key) {
            g.shortcut_order.push(key.clone());
        }
        g.shortcuts.insert(key, (owner, description));
        Ok(())
    }

    /// Keys with a registered shortcut, in REGISTRATION order (pi walks `this.extensions` in load
    /// order, `runner.ts:508`).
    pub fn shortcut_keys(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.shortcut_order.clone())
    }

    /// Every registered shortcut as `(key, description)` in registration order (EXT-040). The
    /// `/hotkeys` table renders the description, falling back the way pi does — `const description
    /// = shortcut.description ?? shortcut.extensionPath;`
    /// (`modes/interactive/interactive-mode.ts:5856` @v0.83.0), i.e. to the extension ID, never to
    /// the key id itself.
    pub fn shortcut_specs(&self) -> Result<Vec<(String, Option<String>)>, ExtError> {
        let g = self.lock_read()?;
        Ok(g.shortcut_order
            .iter()
            .filter_map(|k| {
                g.shortcuts.get(k).map(|(owner, desc)| {
                    (k.clone(), Some(desc.clone().unwrap_or_else(|| owner.to_string())))
                })
            })
            .collect())
    }

    /// The extension that registered the shortcut bound to `key`, if any (R-08-017). Used by the host
    /// to route a fired key press to its owning live instance's `execute-shortcut` export.
    /// Case-insensitive, because pi normalizes with `key.toLowerCase()` before it ever matches
    /// (`runner.ts:510`).
    pub fn shortcut_owner(&self, key: &str) -> Result<Option<ExtensionId>, ExtError> {
        let g = self.lock_read()?;
        if let Some((owner, _)) = g.shortcuts.get(key) {
            return Ok(Some(owner.clone()));
        }
        let lower = key.to_lowercase();
        Ok(g.shortcuts
            .iter()
            .find(|(k, _)| k.to_lowercase() == lower)
            .map(|(_, (owner, _))| owner.clone()))
    }

    /// Resolve the extension shortcut map against the host's resolved keybinding config — the
    /// direct port of pi `ExtensionRunner.getShortcuts(resolvedKeybindings)`
    /// (`pi/packages/coding-agent/src/core/extensions/runner.ts:492-534` @v0.83.0), EXT-039.
    ///
    /// `resolved_keybindings` is upstream's `KeybindingsConfig`: action id → the key(s) bound to it.
    /// Every rule below is upstream's, in upstream's order:
    ///
    /// 1. `buildBuiltinKeybindings` (`runner.ts:92-111`) inverts that map to key → `{keybinding,
    ///    restrictOverride}`, lowercasing each key, where `restrictOverride` is membership of
    ///    [`RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`]. When several actions bind the same key
    ///    the RESERVED one wins regardless of iteration order (`:104-106`).
    /// 2. A shortcut colliding with a RESERVED built-in is SKIPPED with a warning (`:513-520`) —
    ///    it never enters the map, so it cannot be listed by `/hotkeys` as if it were live.
    /// 3. A shortcut colliding with a NON-reserved built-in warns but WINS (`:522-528`).
    /// 4. Two extensions on the same key warn, and the LAST registrant wins — `extensionShortcuts.set`
    ///    runs unconditionally after the warning (`:530-536`). This is deliberately NOT the
    ///    first-wins rule the tool/command/renderer tables use; the warning text says so
    ///    ("Using ${shortcut.extensionPath}").
    ///
    /// Returns the resolved `key → owner` map in insertion order. Diagnostics are read back with
    /// [`Self::shortcut_diagnostics`], mirroring `getShortcutDiagnostics()` (`:538-540`).
    pub fn resolve_shortcuts(
        &self,
        resolved_keybindings: &[(String, Vec<String>)],
    ) -> Result<Vec<(String, ExtensionId)>, ExtError> {
        let builtin = build_builtin_keybindings(resolved_keybindings);
        let mut g = self.lock_write()?;
        g.shortcut_diagnostics.clear();
        let mut out: Vec<(String, ExtensionId)> = Vec::new();
        let order = g.shortcut_order.clone();
        for key in order {
            let Some((owner, _)) = g.shortcuts.get(&key).cloned() else { continue };
            let normalized = key.to_lowercase();
            let warn = |g: &mut RegistryInner, message: String| {
                let record = ExtensionConflict { path: owner.clone(), message };
                if !g.shortcut_diagnostics.contains(&record) {
                    g.shortcut_diagnostics.push(record);
                }
            };
            match builtin.get(&normalized) {
                // Rule 2 — reserved: skip. pi's text, verbatim (`runner.ts:515-518`).
                Some(b) if b.restrict_override => {
                    warn(
                        &mut g,
                        format!(
                            "Extension shortcut '{key}' from {owner} conflicts with built-in \
                             shortcut. Skipping."
                        ),
                    );
                    continue;
                }
                // Rule 3 — non-reserved: warn, extension wins (`runner.ts:523-526`).
                Some(b) => warn(
                    &mut g,
                    format!(
                        "Extension shortcut conflict: '{key}' is built-in shortcut for {} and \
                         {owner}. Using {owner}.",
                        b.keybinding
                    ),
                ),
                None => {}
            }
            // Rule 4 — extension vs extension: warn, LAST wins (`runner.ts:530-536`).
            if let Some(pos) = out.iter().position(|(k, _)| *k == normalized) {
                let existing = out[pos].1.clone();
                warn(
                    &mut g,
                    format!(
                        "Extension shortcut conflict: '{key}' registered by both {existing} and \
                         {owner}. Using {owner}."
                    ),
                );
                out.remove(pos);
            }
            out.push((normalized, owner));
        }
        Ok(out)
    }

    /// Warnings from the last [`Self::resolve_shortcuts`] (pi `getShortcutDiagnostics()`,
    /// `extensions/runner.ts:538-540` @v0.83.0). Upstream folds these into the `[Extension issues]`
    /// startup panel (`modes/interactive/interactive-mode.ts:1612-1618`); cyrup surfaces them
    /// through the same `startup_diagnostics.extensions` channel the load diagnostics use.
    pub fn shortcut_diagnostics(&self) -> Result<Vec<ExtensionConflict>, ExtError> {
        Ok(self.lock_read()?.shortcut_diagnostics.clone())
    }

    /// Register a custom LLM provider (R-08-019; A-08-7). Parses the typed config, resolves the API
    /// key (env / `!command` / literal), and routes it through the [`ProviderHub`] (immediate upsert
    /// if the model registry is bound, else queued for the next bind). A parse/resolution failure is
    /// surfaced as a typed error, never a panic.
    pub fn register_provider(
        &self,
        owner: ExtensionId,
        id: impl Into<String>,
        config: Value,
    ) -> Result<(), ExtError> {
        let id = id.into();
        let mut g = self.lock_write()?;
        g.provider_hub.register(id.clone(), &config).map_err(ExtError::Component)?;
        g.provider_owner.insert(id, owner);
        Ok(())
    }

    /// Opt a registered command into argument autocomplete (EXT-035) — the registry-backed native
    /// analog of the guest's `registration.add-autocomplete` import.
    pub fn add_command_autocomplete(
        &self,
        owner: ExtensionId,
        command: impl Into<String>,
    ) -> Result<(), ExtError> {
        self.lock_write()?.command_autocomplete.push((owner, command.into()));
        Ok(())
    }

    /// Commands opted into argument autocomplete, in registration order (EXT-035).
    pub fn command_autocomplete(&self) -> Result<Vec<(ExtensionId, String)>, ExtError> {
        Ok(self.lock_read()?.command_autocomplete.clone())
    }

    /// Stack one global autocomplete provider (EXT-035; pi `addAutocompleteProvider`,
    /// `extensions/types.ts:218`).
    pub fn add_autocomplete_provider(&self, owner: ExtensionId) -> Result<(), ExtError> {
        self.lock_write()?.autocomplete_providers.push(owner);
        Ok(())
    }

    /// The extensions that stacked a global autocomplete provider, in registration order (EXT-035).
    pub fn autocomplete_providers(&self) -> Result<Vec<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.autocomplete_providers.clone())
    }

    pub fn unregister_provider(&self, id: &str) -> Result<bool, ExtError> {
        let mut g = self.lock_write()?;
        g.provider_owner.remove(id);
        Ok(g.provider_hub.unregister(id))
    }

    /// Bind the model-registry sink and flush queued provider registrations (Pi `bindCore`).
    pub fn bind_model_registry(&self, sink: Arc<dyn ModelRegistrySink>) -> Result<(), ExtError> {
        self.lock_write()?.provider_hub.bind(sink);
        Ok(())
    }

    /// Provider ids still queued for the next [`Self::bind_model_registry`] (not yet flushed).
    pub fn provider_pending_ids(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.provider_hub.pending_ids().to_vec())
    }

    /// A resolved provider registration by id (typed config + resolved api key).
    pub fn provider_registration(&self, id: &str) -> Result<Option<ProviderRegistration>, ExtError> {
        Ok(self.lock_read()?.provider_hub.get(id).cloned())
    }

    /// Register a flag WITHOUT an owning extension — the host/embedder-side setter (and what the
    /// flag-reconciliation tests drive). Unconditional insert: with no owner there is nothing to
    /// compare against, so no conflict can be attributed. Extension-supplied flags must go through
    /// [`Self::register_flag`] so Pi's first-wins rule and its conflict diagnostic apply.
    pub fn set_flag(&self, name: impl Into<String>, spec: Value) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.flags.insert(name.into(), spec);
        Ok(())
    }

    /// Register an extension-owned flag (Pi `pi.registerFlag(name, options)`, loader.ts:274-283).
    ///
    /// **First registering extension wins**, mirroring Pi's `getFlags()` which folds every
    /// extension's `flags` map into one in LOAD ORDER under `if (!allFlags.has(name))`
    /// (`extensions/runner.ts:473-483`) — so the second extension's spec is never the one the CLI
    /// reconciles against. The loser is recorded as an [`ExtensionConflict`] carrying Pi's
    /// `Flag "--<name>" conflicts with <owner>` message (resource-loader.ts:1080-1089). A
    /// re-registration by the SAME owner replaces (Pi's per-extension `extension.flags.set`).
    pub fn register_flag(
        &self,
        owner: ExtensionId,
        name: impl Into<String>,
        spec: Value,
    ) -> Result<(), ExtError> {
        let name = name.into();
        let mut g = self.lock_write()?;
        if let Some(existing) = g.flag_owner.get(&name).cloned()
            && existing != owner
        {
            Self::record_conflict(
                &mut g,
                owner,
                format!("Flag \"--{name}\" conflicts with {existing}"),
            );
            return Ok(());
        }
        g.flag_owner.insert(name.clone(), owner);
        g.flags.insert(name, spec);
        Ok(())
    }

    pub fn get_flag(&self, name: &str) -> Result<Option<Value>, ExtError> {
        Ok(self.lock_read()?.flags.get(name).cloned())
    }

    /// Record a CLI-supplied flag override value (Pi `runtime.flagValues.set(name, value)`,
    /// runner.ts:454-456 / agent-session-services.ts:109,113). Shared across guests; read back by
    /// [`crate::host::GuestState::get_flag`] ahead of the registered default (gap-08 §5.6).
    pub fn set_flag_value(&self, name: impl Into<String>, value: Value) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.flag_values.insert(name.into(), value);
        Ok(())
    }

    /// The CLI override value for `name`, if one was applied (Pi `runtime.flagValues.get(name)` for a
    /// CLI-set entry). `None` = no override; the guest's registered default applies instead.
    pub fn flag_value(&self, name: &str) -> Result<Option<Value>, ExtError> {
        Ok(self.lock_read()?.flag_values.get(name).cloned())
    }

    /// All extension tools in registration order (overrides resolved).
    pub fn extension_tools(&self) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        let g = self.lock_read()?;
        Ok(g.tool_order.iter().filter_map(|n| g.tools.get(n).cloned()).collect())
    }

    /// Look up an extension tool by name.
    pub fn tool(&self, name: &str) -> Result<Option<Arc<dyn Tool>>, ExtError> {
        Ok(self.lock_read()?.tools.get(name).cloned())
    }

    pub fn has_command(&self, name: &str) -> Result<bool, ExtError> {
        Ok(self.lock_read()?.commands.contains_key(name))
    }

    /// The extension that owns a registered command (for slash-command routing, R-08-016).
    pub fn command_owner(&self, name: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.commands.get(name).map(|(owner, _)| owner.clone()))
    }

    /// All registered command names (diagnostics / `getCommands`).
    pub fn command_names(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.commands.keys().cloned().collect())
    }

    /// Every registered command's `(name, descriptor)` (Pi `getRegisteredCommands`, runner.ts) — the
    /// invocable extension commands the RPC `get_commands` / TUI command list enumerate (R-11-014).
    pub fn command_descriptions(&self) -> Result<Vec<(String, CommandDescriptor)>, ExtError> {
        Ok(self
            .lock_read()?
            .commands
            .iter()
            .map(|(name, (_, desc))| (name.clone(), desc.clone()))
            .collect())
    }

    /// Drop every registration (hot-reload cache-bust, R-08-005). The dispatcher is reset separately.
    pub fn clear(&self) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        *g = RegistryInner::default();
        drop(g);
        // A cleared registry has no tools to re-materialize; drop any pending refresh so the next
        // `refresh_tools` after a reload does not walk an empty table (R-08-005).
        self.take_tools_dirty();
        Ok(())
    }

    pub fn provider_ids(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.provider_hub.ids())
    }

    /// Merge a base tool set (built-ins) with extension tools; extension tools override by name
    /// (R-08-012/014). Stable order: base order first, then new extension-only tools.
    pub fn active_tools(&self, base: &[Arc<dyn Tool>]) -> Result<Vec<Arc<dyn Tool>>, ExtError> {
        let g = self.lock_read()?;
        let mut out: Vec<Arc<dyn Tool>> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for t in base {
            let name = t.name().to_string();
            seen.insert(name.clone());
            if let Some(over) = g.tools.get(&name) {
                out.push(over.clone());
            } else {
                out.push(t.clone());
            }
        }
        for n in &g.tool_order {
            if !seen.contains(n)
                && let Some(t) = g.tools.get(n) {
                    out.push(t.clone());
                }
        }
        Ok(out)
    }

    fn lock_read(&self) -> Result<std::sync::RwLockReadGuard<'_, RegistryInner>, ExtError> {
        self.inner.read().map_err(|_| ExtError::Io("registry lock poisoned".into()))
    }

    fn lock_write(&self) -> Result<std::sync::RwLockWriteGuard<'_, RegistryInner>, ExtError> {
        self.inner.write().map_err(|_| ExtError::Io("registry lock poisoned".into()))
    }
}

/// The editor-global keybinding ids an extension shortcut may NOT override, ported verbatim and in
/// order from pi's `RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS`
/// (`pi/packages/coding-agent/src/core/extensions/runner.ts:70-89` @v0.83.0), including its comment:
/// "Extension shortcuts compete with canonical keybinding ids from keybindings.json. Only
/// editor-global shortcuts are reserved here. Picker-specific bindings are not." (EXT-039)
pub const RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS: &[&str] = &[
    "app.interrupt",
    "app.clear",
    "app.exit",
    "app.suspend",
    "app.thinking.cycle",
    "app.model.cycleForward",
    "app.model.cycleBackward",
    "app.model.select",
    "app.tools.expand",
    "app.thinking.toggle",
    "app.editor.external",
    "app.message.copy",
    "app.message.followUp",
    "tui.input.submit",
    "tui.select.confirm",
    "tui.select.cancel",
    "tui.input.copy",
    "tui.editor.deleteToLineEnd",
];

/// One built-in binding, as pi's `BuiltInKeyBindings` value (`runner.ts:91`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltinKeybinding {
    /// The canonical action id the key is bound to (pi `keybinding`).
    pub keybinding: String,
    /// Whether an extension is REFUSED this key (pi `restrictOverride`).
    pub restrict_override: bool,
}

/// Invert `action -> keys` into `lowercased key -> {keybinding, restrict_override}` — pi
/// `buildBuiltinKeybindings` (`extensions/runner.ts:92-111` @v0.83.0).
///
/// The load-bearing detail is upstream's `:104-106`: when several actions bind the same key, the
/// RESERVED action wins regardless of iteration order, "so extensions remain blocked by reserved
/// shortcuts regardless of iteration order".
pub fn build_builtin_keybindings(
    resolved: &[(String, Vec<String>)],
) -> HashMap<String, BuiltinKeybinding> {
    let mut out: HashMap<String, BuiltinKeybinding> = HashMap::new();
    for (keybinding, keys) in resolved {
        let restrict_override =
            RESERVED_KEYBINDINGS_FOR_EXTENSION_CONFLICTS.contains(&keybinding.as_str());
        for key in keys {
            let normalized = key.to_lowercase();
            if let Some(existing) = out.get(&normalized)
                && existing.restrict_override
                && !restrict_override
            {
                continue;
            }
            out.insert(
                normalized,
                BuiltinKeybinding { keybinding: keybinding.clone(), restrict_override },
            );
        }
    }
    out
}

/// pi's `SourceInfo` for a registered tool (`core/source-info.ts:6-12` @v0.83.0:
/// `{path, source, scope, origin, baseDir?}`), in the SYNTHETIC form
/// `createSyntheticSourceInfo` produces (`:24-38`: scope "temporary", origin "top-level").
///
/// EXT-038: cyrup's registry knows the owning extension id and nothing else — a discovered
/// extension's on-disk path is held by the loader, not here — so the id fills both `path` and
/// `source`. That is still strictly more than the field being absent, which is what a guest saw.
fn tool_source_info(owner: Option<&ExtensionId>) -> Value {
    let name = owner.map(ToString::to_string).unwrap_or_default();
    serde_json::json!({
        "path": name,
        "source": name,
        "scope": "temporary",
        "origin": "top-level",
    })
}
