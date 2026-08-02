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
    /// Extension tools keyed by name; last insert wins (override, R-08-012). Insertion order kept.
    tool_order: Vec<String>,
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Which extension owns each tool name (for diagnostics / unload).
    tool_owner: HashMap<String, ExtensionId>,
    commands: HashMap<String, (ExtensionId, CommandDescriptor)>,
    /// Every command registration in LOAD ORDER (Pi preserves load order for invocation-name
    /// disambiguation, runner.ts:559-565). The `commands` map is the fast last-wins lookup; this Vec
    /// retains duplicates + order so [`ExtensionRegistry::resolved_commands`] can assign `name:N`.
    command_order: Vec<(ExtensionId, String, CommandDescriptor)>,
    shortcuts: HashMap<String, ExtensionId>,
    flags: HashMap<String, Value>,
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
    /// Pi's per-tool `renderCall`/`renderResult`, extensions/types.ts:472-481, resolved by
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
}

impl ExtensionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register (or override) an extension tool. A `PoisonError` degrades to a surfaced error,
    /// never a panic (R-00-009).
    pub fn register_tool(&self, owner: ExtensionId, tool: Arc<dyn Tool>) -> Result<(), ExtError> {
        let name = tool.name().to_string();
        let mut g = self.lock_write()?;
        if !g.tools.contains_key(&name) {
            g.tool_order.push(name.clone());
        }
        g.tool_owner.insert(name.clone(), owner);
        g.tools.insert(name, tool);
        drop(g);
        self.mark_tools_dirty();
        Ok(())
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

    /// Register a guest (WASM) tool by descriptor (R-08-012). Last insert wins (override); insertion
    /// order is preserved for stable active-set surfacing.
    pub fn register_guest_tool(
        &self,
        owner: ExtensionId,
        desc: ToolDescriptor,
    ) -> Result<(), ExtError> {
        desc.validate()?;
        let name = desc.name.clone();
        let mut g = self.lock_write()?;
        if !g.guest_tools.contains_key(&name) {
            g.guest_tool_order.push(name.clone());
        }
        // A descriptor that declares `has_renderer` makes its owner the renderer for that TOOL name
        // (Pi's per-tool `renderCall`/`renderResult`, types.ts:472-481) — EXT-006.
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
    /// extensions/types.ts:472-481). LAST registration wins here, matching
    /// [`Self::register_guest_tool`]'s descriptor path (a re-registered descriptor re-points the
    /// owner) rather than the first-wins custom-MESSAGE rule.
    pub fn register_tool_renderer(
        &self,
        owner: ExtensionId,
        tool_name: impl Into<String>,
    ) -> Result<(), ExtError> {
        self.lock_write()?.tool_renderer_owner.insert(tool_name.into(), owner);
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
    pub fn message_renderer_owner(&self, custom_type: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.message_renderer_owner.get(custom_type).cloned())
    }

    /// All registered tool names with Pi's **first-registration-wins** ordering (`getAllRegisteredTools`,
    /// runner.ts:417; gap-08 #7). `tool_order`/`guest_tool_order` are both first-insert order (a later
    /// override updates the value but keeps the original position), so the union — extension tools
    /// then guest-only tools, de-duplicated first-wins — is exactly Pi's getter order. (Execution
    /// still resolves last-wins via the `tools` map; only the *getter* is first-wins.)
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

    /// `ToolInfo[]` for every registered tool (Pi `getAllTools`, types.ts:1260): name + source +
    /// parameter schema. First-registration-wins order (matches [`Self::all_registered_tool_names`]).
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
                    "source": "extension",
                    "description": t.description(),
                    "parameters": t.parameters(),
                }));
            }
        }
        for n in &g.guest_tool_order {
            if !seen.insert(n.clone()) {
                continue;
            }
            if let Some((_, d)) = g.guest_tools.get(n) {
                out.push(serde_json::json!({
                    "name": d.name,
                    "source": "guest",
                    "description": d.description,
                    "parameters": d.parameters,
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
    /// extension (Pi resolves the handler from the `ResolvedCommand`, runner.ts). Falls back to the
    /// last-wins `commands` map for a bare name with no disambiguation.
    pub fn resolved_command_owner(&self, invocation: &str) -> Result<Option<ExtensionId>, ExtError> {
        if let Some(r) = self.resolved_commands()?.into_iter().find(|r| r.invocation_name == invocation)
        {
            return Ok(Some(r.owner));
        }
        self.command_owner(invocation)
    }

    /// Register a keyboard shortcut owned by an extension (R-08-017).
    pub fn register_shortcut(
        &self,
        owner: ExtensionId,
        key: impl Into<String>,
    ) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.shortcuts.insert(key.into(), owner);
        Ok(())
    }

    /// Keys with a registered shortcut.
    pub fn shortcut_keys(&self) -> Result<Vec<String>, ExtError> {
        Ok(self.lock_read()?.shortcuts.keys().cloned().collect())
    }

    /// The extension that registered the shortcut bound to `key`, if any (R-08-017). Used by the host
    /// to route a fired key press to its owning live instance's `execute-shortcut` export.
    pub fn shortcut_owner(&self, key: &str) -> Result<Option<ExtensionId>, ExtError> {
        Ok(self.lock_read()?.shortcuts.get(key).cloned())
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

    pub fn set_flag(&self, name: impl Into<String>, spec: Value) -> Result<(), ExtError> {
        let mut g = self.lock_write()?;
        g.flags.insert(name.into(), spec);
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
