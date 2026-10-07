//! Dynamic tool registry + system-prompt rebuild (Pi `_toolRegistry`/`_toolDefinitions` +
//! `_rebuildSystemPrompt`, agent-session.ts:786-828,2304-2396). The active tool set is mutable
//! mid-session (`setActiveToolsByName`); changing it re-derives the base system prompt from the new
//! tool snippets and re-pushes both the tool array and the prompt to the agent for the next turn.
//!
//! Tool selection was build-time-only before this module: the builder picked the active set once
//! and never re-derived. [`DynamicToolState`] keeps the full registry of enable-able tools and a
//! [`PromptRebuilder`] capturing the stable prompt inputs so the active set can be retoggled.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use cyrup_core::{Sections, Tool, ToolExposure, ToolLoadout, ToolNamespace};
use cyrup_session::prompt::{
    PromptInputs, SystemPromptBuilder, ToolPromptContribution, render_sections,
};

/// A serializable tool descriptor for `getAllTools`/`getToolDefinition` (Pi `ToolInfo`,
/// agent-session.ts:790-799). Carries the model-visible name/description/parameter schema plus the
/// per-tool prompt snippet (Pi `promptGuidelines`/`sourceInfo` collapse to the snippet here).
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInfo {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_snippet: Option<String>,
    /// Whether the tool is in the currently-active set (model-visible this turn).
    pub active: bool,
    /// How the model reaches the tool (pi `ToolInfo.exposure`, `core/extensions/types.ts` @v1.0.1,
    /// filled by `getAllTools` at `agent-session.ts:1469` from `_getToolExposure`).
    pub exposure: ToolExposure,
    /// The tool's namespace, when it has one (pi `ToolInfo.namespace`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<ToolNamespace>,
    /// The definition's `renderShell` (pi `ToolDefinition.renderShell?: "default" | "self"`,
    /// `extensions/types.ts:467` @v0.84.4), read off [`Tool::render_kind`] — what
    /// `ToolExecutionComponent.getRenderShell()` resolves from `session.getToolDefinition(name)`
    /// (`modes/interactive/components/tool-execution.ts:108-116`, definition handed in at
    /// `interactive-mode.ts:2067-2069`) to decide whether the row gets the tinted `Box(1, 1)` shell
    /// or the tool's own framing (EXT-024).
    ///
    /// `#[serde(skip)]`: pi's serialized `ToolInfo` is exactly `{name, description, parameters,
    /// promptSnippet?, active?}` (`agent-session.ts:790-799`) and the guest-facing `getAllTools`
    /// shape is pinned separately (EXT-038); this field is for the host's own renderer, not the
    /// wire.
    #[serde(skip)]
    pub render_kind: cyrup_core::ToolRenderKind,
}

/// A built system prompt: the named sections the transcript stores, and the text they render to.
///
/// The two are one value, never built apart. The sections are what a session writes to its file and
/// diffs against the transcript (CODE-014, pi `diffSystemPromptSections`); the text is what
/// `ctx.getSystemPrompt()`, `/export` and a `before_agent_start` handler read, and it is — by
/// construction — the text a provider is sent for those sections. It derefs to that text.
#[derive(Clone, Debug, Default)]
pub(crate) struct BuiltPrompt {
    sections: Sections,
    text: String,
}

impl BuiltPrompt {
    pub(crate) fn new(sections: Sections) -> Self {
        let text = render_sections(&sections);
        Self { sections, text }
    }

    pub(crate) fn sections(&self) -> &Sections {
        &self.sections
    }

    pub(crate) fn text(&self) -> &str {
        &self.text
    }
}

impl std::fmt::Display for BuiltPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl std::ops::Deref for BuiltPrompt {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

/// Captures the stable system-prompt inputs so the base prompt can be rebuilt when the active tool
/// set changes (Pi `_baseSystemPromptOptions` + `_rebuildSystemPrompt`, agent-session.ts:2304).
pub(crate) struct PromptRebuilder {
    /// Everything the [`SystemPromptBuilder`] needs except the per-run tool fields, which are
    /// re-derived from the active set on each rebuild.
    base: PromptInputs,
    /// The per-tool prompt contribution (snippet + guidelines) keyed by tool name — the SAME source
    /// the builder used for the initial prompt, so a rebuild is byte-identical for the same set.
    contributions: BTreeMap<String, ToolPromptContribution>,
}

impl PromptRebuilder {
    pub(crate) fn new(
        base: PromptInputs,
        contributions: BTreeMap<String, ToolPromptContribution>,
    ) -> Self {
        Self {
            base,
            contributions,
        }
    }

    /// Record (or replace) a tool's prompt contribution so a tool registered AFTER the build
    /// contributes its snippet/guidelines to the rebuilt prompt (EXT-004; Pi rebuilds
    /// `_toolPromptSnippets`/`_toolPromptGuidelines` from the refreshed definition registry,
    /// agent-session.ts:2487-2506). Without this a late tool would reach the model's tool array
    /// with no prompt guidance at all.
    fn upsert_contribution(&mut self, tool: &Arc<dyn Tool>) {
        self.contributions.insert(
            tool.name().to_string(),
            crate::builder::tool_contribution(tool),
        );
    }

    /// The base bag itself, serialized as pi's `BuildSystemPromptOptions` — what
    /// `ctx.getSystemPromptOptions()` hands a command handler (EXT-061).
    ///
    /// Upstream this is not derived at all: `_rebuildSystemPrompt` ASSIGNS
    /// `this._baseSystemPromptOptions` on its way to building the string
    /// (`core/agent-session.ts:1044-1053` @v0.83.0) and `getSystemPromptOptions` returns that field
    /// verbatim (`:2436`). cyrup keeps the same two halves in one place — [`Self::base`] plus the
    /// active set — so the bag a guest reads is BY CONSTRUCTION the bag the next
    /// [`Self::rebuild`] would use, rather than a second snapshot that can drift from it.
    ///
    /// Key set and derivation, matching `:1021-1053`:
    /// - `selectedTools` — the ACTIVE names (pi's `validToolNames`), not `base.selected_tools`,
    ///   which is cleared on the rebuild base by design.
    /// - `toolSnippets` — `{name: snippet}` over the active tools that have one.
    /// - `hiddenTools` — the active tools whose declarations requests leave out
    ///   (`BuildSystemPromptOptions.hiddenTools`, `system-prompt.ts:16-20` @v1.0.4).
    /// - `promptGuidelines` — each DECLARED active tool's guidelines in active order, then cyrup's
    ///   free-floating [`PromptInputs::prompt_guidelines`], which upstream has no channel for.
    /// - `customPrompt` / `appendSystemPrompt` — omitted when unset, as pi omits `undefined`.
    /// - `cwd`, `contextFiles` (`{path, content}`), `skills`.
    fn base_options(&self, active: &[String], hidden: &BTreeSet<String>) -> serde_json::Value {
        let mut snippets = serde_json::Map::new();
        let mut guidelines: Vec<String> = Vec::new();
        for name in active {
            if let Some(c) = self.contributions.get(name) {
                if let Some(s) = c.snippet.as_ref() {
                    snippets.insert(name.clone(), serde_json::Value::String(s.to_string()));
                }
                // A hidden declaration is left out of the rules as it is out of the request:
                // `hiddenTools` names it instead (pi `_rebuildSystemPrompt`,
                // `agent-session.ts:1688-1710` @v1.0.4).
                if !hidden.contains(name) {
                    guidelines.extend(c.guidelines.iter().map(|g| g.to_string()));
                }
            }
        }
        guidelines.extend(self.base.prompt_guidelines.iter().map(|g| g.to_string()));

        let mut bag = serde_json::Map::new();
        if let Some(custom) = self.base.custom_prompt.as_ref() {
            bag.insert(
                "customPrompt".into(),
                serde_json::Value::String(custom.to_string()),
            );
        }
        bag.insert("selectedTools".into(), serde_json::json!(active));
        bag.insert("toolSnippets".into(), serde_json::Value::Object(snippets));
        bag.insert(
            "hiddenTools".into(),
            serde_json::json!(
                active
                    .iter()
                    .filter(|name| hidden.contains(*name))
                    .collect::<Vec<_>>()
            ),
        );
        bag.insert("promptGuidelines".into(), serde_json::json!(guidelines));
        if let Some(append) = self.base.append_system_prompt.as_ref() {
            bag.insert(
                "appendSystemPrompt".into(),
                serde_json::Value::String(append.to_string()),
            );
        }
        bag.insert("cwd".into(), serde_json::json!(self.base.cwd));
        bag.insert(
            "contextFiles".into(),
            serde_json::json!(
                self.base
                    .context_files
                    .iter()
                    .map(|f| serde_json::json!({"path": f.path, "content": f.content.to_string()}))
                    .collect::<Vec<_>>()
            ),
        );
        bag.insert(
            "skills".into(),
            serde_json::json!(self.base.skills.as_ref()),
        );
        serde_json::Value::Object(bag)
    }

    /// Rebuild the base system prompt for `active` tools, pulling each tool's contribution from the
    /// precomputed map (Pi `_rebuildSystemPrompt`, agent-session.ts:2304-2396).
    fn rebuild(&self, active: &[String], hidden: &BTreeSet<String>) -> BuiltPrompt {
        let mut inputs = self.base.clone();
        inputs.selected_tools = Some(active.iter().map(|n| Arc::from(n.as_str())).collect());
        // The tool list and the rules must match the declarations the request carries: a tool whose
        // declaration is hidden is reachable only through another tool, so the builder leaves it
        // out of the listing, the rules and the skills hint (pi `_preparePromptAndToolLoadout`,
        // `options.hiddenTools = [...this._hiddenDeclarations]`, `agent-session.ts:1727-1728`
        // @v1.0.4, CODE-020; v1.0.1 blanked the snippet and kept the guidelines).
        inputs.hidden_tools = hidden.iter().map(|n| Arc::from(n.as_str())).collect();
        inputs.tool_contributions = active
            .iter()
            .filter_map(|n| self.contributions.get(n).cloned())
            .collect();
        BuiltPrompt::new(SystemPromptBuilder::new().build_sections(&inputs))
    }
}

/// The mutable dynamic-tool surface (Pi `_toolRegistry`/`_toolDefinitions`/`_activeToolNames`).
pub(crate) struct DynamicToolState {
    /// All enable-able tools by name (built-ins after selection + extension/custom tools).
    registry: BTreeMap<String, Arc<dyn Tool>>,
    /// The resolved loadout of the currently-active names (Pi `agent.state.tools` plus
    /// `_hiddenDeclarations`): what the loop runs, and what a request may declare.
    loadout: ToolLoadout,
    rebuilder: PromptRebuilder,
    /// Tools of the restored loadout that are not registered yet, such as tools of MCP servers that
    /// are still connecting (pi `_pendingToolNames`, `agent-session.ts:431` @v1.0.1). They are
    /// activated when they register, and dropped when [`Self::set_active`] deactivates a tool or
    /// the next run starts ([`Self::clear_pending`]).
    pending: BTreeSet<String>,
}

impl DynamicToolState {
    pub(crate) fn new(
        registry_tools: Vec<Arc<dyn Tool>>,
        active: Vec<Arc<dyn Tool>>,
        rebuilder: PromptRebuilder,
    ) -> Self {
        let registry: BTreeMap<String, Arc<dyn Tool>> = registry_tools
            .into_iter()
            .map(|t| (t.name().to_string(), t))
            .collect();
        let names: Vec<String> = active.iter().map(|t| t.name().to_string()).collect();
        let loadout = ToolLoadout::resolve(&names, &registry.values().cloned().collect::<Vec<_>>());
        Self {
            registry,
            loadout,
            rebuilder,
            pending: BTreeSet::new(),
        }
    }

    /// Restore the loadout the transcript declares (pi `_restoreToolsFromTranscript`,
    /// `agent-session.ts:1762-1769` @v1.0.1): `declared` replaces the active set, and every
    /// declared name stays pending until a tool of that name registers.
    ///
    /// The caller has already dropped the names the session may not expose (pi `_isAllowedTool`,
    /// see [`is_allowed_tool`]). This is `_setActiveTools`, not `setActiveToolsByName`: replacing
    /// the loadout with the restored one must not drop the pending names it just recorded.
    pub(crate) fn restore_declared(&mut self, declared: &[String]) -> (ToolLoadout, BuiltPrompt) {
        self.pending = declared.iter().cloned().collect();
        self.activate(declared)
    }

    /// Drop the restored names that have not registered (pi `_pendingToolNames.clear()`): at the
    /// start of a run, "so a tool that never registers does not stay pending"
    /// (`agent-session.ts:1778-1782`).
    pub(crate) fn clear_pending(&mut self) {
        self.pending.clear();
    }

    /// The names still waiting for a tool to register.
    #[cfg(test)]
    pub(crate) fn pending_names(&self) -> Vec<String> {
        self.pending.iter().cloned().collect()
    }

    /// Pi `_setActiveTools` (`agent-session.ts:1495-1499` @v1.0.1): resolve `names`, retire the
    /// pending names that are now active, and rebuild the prompt.
    fn activate(&mut self, names: &[String]) -> (ToolLoadout, BuiltPrompt) {
        let registry: Vec<Arc<dyn Tool>> = self.registry.values().cloned().collect();
        self.loadout = ToolLoadout::resolve(names, &registry);
        for tool in self.loadout.executable() {
            self.pending.remove(tool.name());
        }
        (self.loadout.clone(), self.prompt())
    }

    /// The rebuilt system prompt for the current loadout.
    pub(crate) fn prompt(&self) -> BuiltPrompt {
        self.rebuilder
            .rebuild(&self.active_names(), self.loadout.hidden_declarations())
    }

    /// The tools another tool can call through `ctx.executeTool()` — the active `direct` tools and
    /// every registered `codemode` or `deferred` one (pi `getCallableToolNames` /
    /// `_getCallableTools`, `agent-session.ts:1457-1520` @v1.0.1).
    pub(crate) fn callable_names(&self) -> Vec<String> {
        self.callable_tools()
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    /// The tools themselves — what a nested call resolves against (pi `_getCallableTools`,
    /// `agent-session.ts:1515-1520` @v1.0.1, which [`Self::callable_names`] names). Never the
    /// declared set: a tool that is hidden, `model-only`, or an inactive `direct` one is not here.
    pub(crate) fn callable_tools(&self) -> Vec<Arc<dyn Tool>> {
        let active_names: BTreeSet<String> = self.active_names().into_iter().collect();
        let active: BTreeSet<&str> = active_names.iter().map(String::as_str).collect();
        let registry: Vec<Arc<dyn Tool>> = self.registry.values().cloned().collect();
        cyrup_core::callable_tools(&registry, &active)
    }

    /// The base system-prompt options bag for the CURRENT active set (EXT-061) — pi
    /// `_baseSystemPromptOptions` (`core/agent-session.ts:1044-1053` @v0.83.0), returned to a
    /// command handler by `ctx.getSystemPromptOptions()` (`:2436`).
    pub(crate) fn base_prompt_options(&self) -> serde_json::Value {
        self.rebuilder
            .base_options(&self.active_names(), self.loadout.hidden_declarations())
    }

    /// Names of the currently-active tools (Pi `getActiveToolNames`).
    pub(crate) fn active_names(&self) -> Vec<String> {
        self.loadout
            .executable()
            .iter()
            .map(|t| t.name().to_string())
            .collect()
    }

    /// All enable-able tools as [`ToolInfo`] (Pi `getAllTools`).
    pub(crate) fn all(&self) -> Vec<ToolInfo> {
        self.registry.values().map(|t| self.info_for(t)).collect()
    }

    /// The enable-able tools themselves (Pi `_toolDefinitions.values()`), name-ordered.
    ///
    /// Distinct from [`Self::all`]: the guest-facing `getAllTools` capability must emit pi's
    /// `ToolInfo` — `{name, description, parameters, promptGuidelines, sourceInfo}`
    /// (`extensions/types.ts:1552-1554` @v0.83.0) — and [`ToolInfo`] carries neither
    /// `promptGuidelines` nor `sourceInfo`, so `LiveHostServices::all_tools` reads the guidelines
    /// off the `Tool` impl directly (EXT-038).
    pub(crate) fn tools(&self) -> Vec<Arc<dyn Tool>> {
        self.registry.values().cloned().collect()
    }

    /// One tool's [`ToolInfo`] by name (Pi `getToolDefinition`).
    pub(crate) fn get(&self, name: &str) -> Option<ToolInfo> {
        self.registry.get(name).map(|t| self.info_for(t))
    }

    fn info_for(&self, t: &Arc<dyn Tool>) -> ToolInfo {
        ToolInfo {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters().clone(),
            prompt_snippet: t.prompt_snippet().map(str::to_string),
            active: self
                .loadout
                .executable()
                .iter()
                .any(|a| a.name() == t.name()),
            exposure: t.exposure(),
            namespace: t.namespace().cloned(),
            render_kind: t.render_kind(),
        }
    }

    /// Set the active set by name (Pi `setActiveToolsByName`): unknown names and `hidden` tools are ignored, the
    /// active list is replaced, and the new `(tools, system_prompt)` to push to the agent are returned.
    ///
    /// A loadout that deactivates a tool replaces the restored one, so its pending names are
    /// dropped; one that only adds tools, like activating `tool_search`, keeps them (pi
    /// `setActiveToolsByName`, `agent-session.ts:1487-1493` @v1.0.1).
    pub(crate) fn set_active(&mut self, names: &[String]) -> (ToolLoadout, BuiltPrompt) {
        let previous = self.active_names();
        let pushed = self.activate(names);
        let active: BTreeSet<String> = self.active_names().into_iter().collect();
        if previous.iter().any(|name| !active.contains(name)) {
            self.pending.clear();
        }
        pushed
    }

    /// Register additional custom tools into the enable-able registry (Pi `customTools`, sdk.ts:71).
    /// New tools are added but not auto-activated (parity with build-time custom-tool registration).
    ///
    /// The contribution upsert is NOT optional and is the half this claimed-parity path was missing:
    /// the build-time route folds every custom tool's snippet/guidelines into the rebuilder's
    /// contribution map (`builder.rs`'s `contributions` collect over `registry_tools`, which
    /// INCLUDES `cfg.custom_tools`), and [`PromptRebuilder::rebuild`] silently drops an active name
    /// with no contribution (`filter_map(|n| self.contributions.get(n))`). Without this a tool
    /// registered here and then activated reached the model's tool array with no prompt guidance at
    /// all — the exact failure [`PromptRebuilder::upsert_contribution`]'s own doc describes.
    pub(crate) fn register_custom(&mut self, tools: Vec<Arc<dyn Tool>>) {
        for t in tools {
            self.rebuilder.upsert_contribution(&t);
            self.registry.insert(t.name().to_string(), t);
        }
    }

    /// Merge the extension-contributed tool set into the registry and AUTO-ACTIVATE anything that
    /// was not registered before (EXT-004; Pi `_refreshToolRegistry`, agent-session.ts:2452-2546 —
    /// `for (const toolName of this._toolRegistry.keys()) { if (!previousRegistryNames.has(toolName))
    /// nextActiveToolNames.push(toolName); }` then `setActiveToolsByName([...new Set(...)])`).
    ///
    /// Returns the rebuilt `(tools, system_prompt)` to push to the agent, or `None` when the
    /// registry did not move at all — neither a new name nor a CHANGED definition for an existing
    /// one. A re-registration of an already-known tool updates the registry entry (a later
    /// definition wins, as it does at build time) but must not disturb the ACTIVE set.
    ///
    /// The changed-definition arm is load-bearing. pi's `_refreshToolRegistry` ends with an
    /// UNCONDITIONAL `this.setActiveToolsByName([...new Set(nextActiveToolNames)])`
    /// (`core/agent-session.ts:2553` @v0.83.0) — the new-name loop at `:2544-2551` only decides
    /// which names are ACTIVE, never whether the push happens — and `setActiveToolsByName` rebuilds
    /// `agent.state.tools` from the freshly-rebuilt registry (`:928-943`), so upstream a replaced
    /// definition ALWAYS reaches the model. Gating the push on "were there new NAMES" meant an
    /// extension re-registering an existing tool mutated this registry and the rebuilder's
    /// contributions while the agent kept running the previously-wrapped `Arc<dyn Tool>` and the
    /// previously-built prompt for the rest of the session, silently on every branch.
    ///
    /// CYRUP-DELTA (`core/agent-session.ts:2553`): the push is still SKIPPED when the incoming set
    /// is definitionally identical to what is already registered, where pi rebuilds anyway. pi
    /// reaches `_refreshToolRegistry` only from real registration events; cyrup's
    /// `AgentSession::next_turn_tools` calls `refresh_extension_tools` on EVERY turn boundary, and
    /// the `#[cfg(not(feature = "wasm-host"))]` arm of `ExtensionHost::refresh_tools` reports
    /// `Ok(true)` unconditionally — so an unconditional rebuild here would re-derive the system
    /// prompt once per turn for a set that never changed. Identical observable behaviour, minus that
    /// per-turn cost.
    ///
    /// This is deliberately NOT `register_custom`: a custom tool is registered *inert* (Pi's
    /// build-time `customTools` are activated by selection), whereas an extension tool registered at
    /// runtime is the extension asking for it to be USABLE — Pi auto-activates exactly this case.
    pub(crate) fn merge_registered(
        &mut self,
        tools: Vec<Arc<dyn Tool>>,
    ) -> Option<(ToolLoadout, BuiltPrompt)> {
        // Pi `previousActivatedOnRegistration` (`agent-session.ts:3446-3449` @v1.0.1): a tool whose
        // exposure changes to `direct` or `model-only` (from `hidden`, say) is activated like a new
        // one, so the comparison is on "activated by registration", not on "was registered".
        let previously_activated: BTreeSet<String> = self
            .registry
            .values()
            .filter(|t| activated_on_registration(t))
            .map(|t| t.name().to_string())
            .collect();
        let mut registry_moved = false;
        for t in tools {
            let name = t.name().to_string();
            self.rebuilder.upsert_contribution(&t);
            match self.registry.insert(name.clone(), t) {
                None => registry_moved = true,
                Some(previous) => {
                    if let Some(current) = self.registry.get(&name)
                        && definition_changed(&previous, current)
                    {
                        registry_moved = true;
                    }
                }
            }
        }
        let mut names = self.active_names();
        let newly_activated: Vec<String> = self
            .registry
            .values()
            .filter(|t| activated_on_registration(t) && !previously_activated.contains(t.name()))
            .map(|t| t.name().to_string())
            .filter(|n| !names.contains(n))
            .collect();
        // A registration that activates nothing still moves the loadout: `callable` and
        // `registered` are what `prepare_loadout` hooks read, and pi's `_refreshToolRegistry` ends
        // with an unconditional `_setActiveTools` (`:3541-3543`).
        if newly_activated.is_empty() && !registry_moved {
            return None;
        }
        names.extend(newly_activated);
        // Restored tools that are registered now become active (pi `nextActiveToolNames.push(
        // ...this._pendingToolNames)`, `agent-session.ts:3541-3542` @v1.0.1) — through
        // `_setActiveTools`, which keeps the names that are still waiting.
        names.extend(self.pending.iter().cloned());
        Some(self.activate(&names))
    }
}

/// pi `_isAllowedTool` (`agent-session.ts:1501-1503` @v1.0.1): inside the session's allowlist, when
/// it has one, and outside its denylist.
pub(crate) fn is_allowed_tool(
    allowed: Option<&std::collections::HashSet<String>>,
    excluded: &std::collections::HashSet<String>,
    name: &str,
) -> bool {
    allowed.is_none_or(|allowed| allowed.contains(name)) && !excluded.contains(name)
}

/// The active tool names a transcript declares (pi `_restoreToolsFromTranscript`'s read,
/// `agent-session.ts:1763-1766` @v1.0.1): the tool set after replaying every system message in
/// `messages`. `None` when the transcript has no system message, which declares no loadout at all.
pub(crate) fn declared_tool_names(messages: &[cyrup_session::AgentMessage]) -> Option<Vec<String>> {
    let system: Vec<cyrup_core::Message> = messages
        .iter()
        .filter_map(|m| match m {
            cyrup_session::AgentMessage::Core(core @ cyrup_core::Message::System(_)) => {
                Some(core.clone())
            }
            _ => None,
        })
        .collect();
    let current = cyrup_provider::get_current_system_message(&system)?;
    Some(current.tools_added.into_iter().map(|t| t.name).collect())
}

/// pi `_isActivatedOnRegistration` (`agent-session.ts:3554` @v1.0.1) for a registered tool.
fn activated_on_registration(tool: &Arc<dyn Tool>) -> bool {
    tool.exposure()
        .activated_on_registration(tool.default_active())
}

/// Whether a re-registration actually replaced the tool the model would run — the model-visible
/// definition pi carries on its `ToolDefinition` (`name`/`description`/`parameters`/
/// `promptGuidelines`, the `ToolInfo` projection at `extensions/types.ts:1552-1554` @v0.83.0), plus
/// the prompt snippet cyrup feeds the system-prompt rebuild.
///
/// `Arc::ptr_eq` short-circuits the common case (the very same handle re-submitted by a turn-boundary
/// refresh). A DIFFERENT handle with an identical definition still counts as unchanged: re-wrapping
/// the same descriptor produces a fresh `Arc` on every `ExtensionHost::active_tools` call, and
/// treating that as a change would rebuild the prompt on every turn.
fn definition_changed(previous: &Arc<dyn Tool>, current: &Arc<dyn Tool>) -> bool {
    if Arc::ptr_eq(previous, current) {
        return false;
    }
    previous.description() != current.description()
        || previous.exposure() != current.exposure()
        || previous.namespace() != current.namespace()
        || previous.default_active() != current.default_active()
        || previous.parameters() != current.parameters()
        || previous.prompt_snippet() != current.prompt_snippet()
        || previous.prompt_guidelines() != current.prompt_guidelines()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_core::{
        CancelToken, ToolCallId, ToolError, ToolExposure, ToolResult, ToolUpdateSink,
    };
    use serde_json::{Value, json};

    /// A tool double whose whole model-visible definition is settable, so a test can register a
    /// SECOND definition under the SAME name and prove the replacement reaches the agent.
    struct Fake {
        name: &'static str,
        description: String,
        params: Value,
        snippet: Option<String>,
        exposure: ToolExposure,
    }

    impl Fake {
        fn new(name: &'static str, description: &str) -> Self {
            Self {
                name,
                description: description.to_string(),
                params: json!({"type": "object", "properties": {}}),
                snippet: None,
                exposure: ToolExposure::Direct,
            }
        }

        /// Registered but not activated by registration (`deferred`).
        fn deferred(mut self) -> Self {
            self.exposure = ToolExposure::Deferred;
            self
        }

        fn with_snippet(mut self, snippet: &str) -> Self {
            self.snippet = Some(snippet.to_string());
            self
        }

        fn arc(self) -> Arc<dyn Tool> {
            Arc::new(self)
        }
    }

    #[async_trait::async_trait]
    impl Tool for Fake {
        fn name(&self) -> &str {
            self.name
        }
        fn parameters(&self) -> &Value {
            &self.params
        }
        fn description(&self) -> &str {
            &self.description
        }
        fn prompt_snippet(&self) -> Option<&str> {
            self.snippet.as_deref()
        }
        fn exposure(&self) -> ToolExposure {
            self.exposure
        }
        async fn execute(
            &self,
            _call_id: ToolCallId,
            _args: Value,
            _cancel: CancelToken,
            _on_update: ToolUpdateSink,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult::default())
        }
    }

    /// A state whose registry+active set is `tools`, with each tool's contribution pre-seeded — the
    /// shape `builder.rs` hands over at build time.
    fn state_with(tools: Vec<Arc<dyn Tool>>) -> DynamicToolState {
        let contributions = tools
            .iter()
            .map(|t| (t.name().to_string(), crate::builder::tool_contribution(t)))
            .collect();
        let rebuilder = PromptRebuilder::new(
            cyrup_session::prompt::PromptInputs::default(),
            contributions,
        );
        DynamicToolState::new(tools.clone(), tools, rebuilder)
    }

    /// A REPLACED tool definition must reach the agent.
    ///
    /// pi's `_refreshToolRegistry` ends with an UNCONDITIONAL
    /// `this.setActiveToolsByName([...new Set(nextActiveToolNames)])`
    /// (`core/agent-session.ts:2553` @v0.83.0) — the new-name loop at `:2544-2551` only decides
    /// which names are ACTIVE — and `setActiveToolsByName` rebuilds `agent.state.tools` from the
    /// freshly rebuilt registry (`:928-943`). cyrup gated the whole push on "were there new NAMES",
    /// so an extension re-registering an existing tool mutated this registry while the agent kept
    /// running the PREVIOUS `Arc<dyn Tool>` and the previous prompt for the rest of the session,
    /// silently on every branch.
    ///
    /// RED before the fix: `merge_registered` returned `None` here, so
    /// `AgentSession::refresh_extension_tools` (which has no `else` and no log) pushed nothing.
    #[test]
    fn merge_registered_pushes_a_replaced_definition() {
        let mut st = state_with(vec![
            Fake::new("deploy", "v1").with_snippet("deploy: v1").arc(),
        ]);
        assert_eq!(st.active_names(), vec!["deploy".to_string()]);

        let push = st
            .merge_registered(vec![
                Fake::new("deploy", "v2").with_snippet("deploy: v2").arc(),
            ])
            .expect("a CHANGED definition for an existing name must still push");
        let (loadout, prompt) = push;
        let tools = loadout.executable();

        assert_eq!(
            tools.len(),
            1,
            "the rebuilt array still holds exactly the active set"
        );
        assert_eq!(
            tools[0].description(),
            "v2",
            "the agent must receive the NEW definition, not the one it was already running"
        );
        assert!(
            prompt.contains("deploy: v2"),
            "the rebuilt system prompt carries the new snippet: {prompt}"
        );
        assert!(
            !prompt.contains("deploy: v1"),
            "…and not the stale one: {prompt}"
        );
        // The active set is untouched by a pure redefinition — only the definition moved.
        assert_eq!(st.active_names(), vec!["deploy".to_string()]);
    }

    /// The complement, and the reason the fix is a CHANGED-definition test rather than an
    /// unconditional push: an IDENTICAL re-registration still returns `None`.
    ///
    /// CYRUP-DELTA (`core/agent-session.ts:2553`) — pi rebuilds unconditionally because it only
    /// reaches `_refreshToolRegistry` from real registration events, whereas cyrup's
    /// `next_turn_tools` calls `refresh_extension_tools` on EVERY turn boundary and the
    /// `#[cfg(not(feature = "wasm-host"))]` arm of `ExtensionHost::refresh_tools` reports `Ok(true)`
    /// unconditionally. Rebuilding the system prompt once per turn for an unchanged set is cost with
    /// no observable difference.
    #[test]
    fn merge_registered_skips_an_unchanged_set() {
        let mut st = state_with(vec![
            Fake::new("deploy", "v1").with_snippet("deploy: v1").arc(),
        ]);
        assert!(
            st.merge_registered(vec![
                Fake::new("deploy", "v1").with_snippet("deploy: v1").arc()
            ])
            .is_none(),
            "a definitionally identical re-registration costs no rebuild"
        );
    }

    /// The auto-activation half is unchanged by the fix: a genuinely NEW name is still added to the
    /// active set (pi `if (!previousRegistryNames.has(toolName)) nextActiveToolNames.push(toolName)`,
    /// `core/agent-session.ts:2549-2551`).
    #[test]
    fn merge_registered_still_auto_activates_a_new_name() {
        let mut st = state_with(vec![Fake::new("deploy", "v1").arc()]);
        let (loadout, _) = st
            .merge_registered(vec![Fake::new("audit", "new").arc()])
            .expect("a new name pushes");
        let names: Vec<&str> = loadout.executable().iter().map(|t| t.name()).collect();
        assert!(
            names.contains(&"audit"),
            "the late tool is active: {names:?}"
        );
        assert!(
            names.contains(&"deploy"),
            "…without disturbing what was already active: {names:?}"
        );
    }

    /// A custom tool registered AFTER build must contribute its prompt guidance, exactly as the
    /// build-time path does (`builder.rs` collects `contributions` over `registry_tools`, which
    /// INCLUDES `cfg.custom_tools`) — the parity `register_custom` claims.
    ///
    /// RED before the fix: `register_custom` was a bare `registry.insert` loop, so
    /// `PromptRebuilder::rebuild`'s `filter_map(|n| self.contributions.get(n))` silently dropped the
    /// key and the model got the tool's schema with none of its guidance — the exact failure
    /// [`PromptRebuilder::upsert_contribution`]'s own doc describes.
    #[test]
    fn register_custom_contributes_prompt_guidance() {
        let mut st = state_with(vec![
            Fake::new("read", "builtin")
                .with_snippet("read: read files")
                .arc(),
        ]);
        st.register_custom(vec![
            Fake::new("deploy", "custom")
                .with_snippet("deploy: ships the build")
                .arc(),
        ]);

        // Registered INERT — pi's build-time `customTools` are activated by selection, never
        // auto-activated. That half must not change.
        assert_eq!(
            st.active_names(),
            vec!["read".to_string()],
            "custom tools register inert"
        );

        let (loadout, prompt) = st.set_active(&["read".to_string(), "deploy".to_string()]);
        let names: Vec<&str> = loadout.executable().iter().map(|t| t.name()).collect();
        assert_eq!(
            names,
            ["read", "deploy"],
            "the custom tool is enable-able: {names:?}"
        );
        assert!(
            prompt.contains("deploy: ships the build"),
            "the custom tool's snippet reaches the model's system prompt: {prompt}"
        );
    }

    fn strings(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    /// A state whose registry holds `read` (active) and `search` (registered, not active).
    fn state_with_inactive_search() -> DynamicToolState {
        let mut st = state_with(vec![Fake::new("read", "builtin").arc()]);
        st.register_custom(vec![Fake::new("search", "deferred").deferred().arc()]);
        st
    }

    /// pi `_restoreToolsFromTranscript` (`agent-session.ts:1762-1769` @v1.0.1): the declared names
    /// REPLACE the active set, a declared name nothing has registered yet stays pending, and the
    /// restore does not drop the names it has just recorded.
    #[test]
    fn restore_declared_replaces_the_active_set_and_keeps_the_unregistered_names_pending() {
        let mut st = state_with_inactive_search();
        let (loadout, _) = st.restore_declared(&strings(&["search", "mcp__docs__find"]));
        let active: Vec<&str> = loadout.executable().iter().map(|t| t.name()).collect();
        assert_eq!(
            active,
            ["search"],
            "read was not declared, so it is not active"
        );
        assert_eq!(st.active_names(), ["search"]);
        assert_eq!(
            st.pending_names(),
            ["mcp__docs__find"],
            "the registered name is retired, the unregistered one waits"
        );
    }

    /// pi `setActiveToolsByName` (`:1487-1493`): a loadout that only ADDS tools — activating
    /// `tool_search` — keeps the pending names; one that deactivates a tool replaces the restored
    /// loadout and drops them.
    #[test]
    fn only_a_deactivating_loadout_drops_the_pending_names() {
        let mut adds = state_with_inactive_search();
        adds.restore_declared(&strings(&["read", "mcp__docs__find"]));
        adds.set_active(&strings(&["read", "search"]));
        assert_eq!(
            adds.pending_names(),
            ["mcp__docs__find"],
            "an addition keeps them"
        );

        let mut drops = state_with_inactive_search();
        drops.restore_declared(&strings(&["read", "mcp__docs__find"]));
        drops.set_active(&strings(&["search"]));
        assert!(
            drops.pending_names().is_empty(),
            "deactivating `read` replaces the restored loadout: {:?}",
            drops.pending_names()
        );
    }

    /// pi `_refreshToolRegistry` (`:3541-3542`): a pending tool that registers is activated even
    /// though its exposure (`deferred`) would not activate it on registration, and is retired.
    #[test]
    fn a_pending_tool_that_registers_is_activated_and_retired() {
        let mut st = state_with(vec![Fake::new("read", "builtin").arc()]);
        st.restore_declared(&strings(&["read", "mcp__docs__find"]));
        assert_eq!(st.pending_names(), ["mcp__docs__find"]);

        let (loadout, _) = st
            .merge_registered(vec![
                Fake::new("mcp__docs__find", "late").deferred().arc(),
                Fake::new("unrelated", "late").deferred().arc(),
            ])
            .expect("a new name pushes");
        let active: Vec<&str> = loadout.executable().iter().map(|t| t.name()).collect();
        assert_eq!(active, ["read", "mcp__docs__find"]);
        assert!(st.pending_names().is_empty());
    }

    /// pi `_runAgentPrompt` (`:1778-1782`): the names still pending when a run starts are dropped,
    /// so a tool registering afterwards is not activated by the restore.
    #[test]
    fn clearing_the_pending_names_stops_a_later_registration_activating() {
        let mut st = state_with(vec![Fake::new("read", "builtin").arc()]);
        st.restore_declared(&strings(&["read", "mcp__docs__find"]));
        st.clear_pending();
        let moved =
            st.merge_registered(vec![Fake::new("mcp__docs__find", "late").deferred().arc()]);
        let (loadout, _) = moved.expect("the registry moved");
        let active: Vec<&str> = loadout.executable().iter().map(|t| t.name()).collect();
        assert_eq!(active, ["read"]);
    }

    /// pi `_isAllowedTool` (`:1501-1503`).
    #[test]
    fn a_tool_is_allowed_inside_the_allowlist_and_outside_the_denylist() {
        use std::collections::HashSet;
        let allowed: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let denied: HashSet<String> = ["b"].iter().map(|s| s.to_string()).collect();
        let none = HashSet::new();
        assert!(is_allowed_tool(None, &none, "x"), "no lists, everything");
        assert!(is_allowed_tool(Some(&allowed), &none, "a"));
        assert!(
            !is_allowed_tool(Some(&allowed), &none, "x"),
            "not on the allowlist"
        );
        assert!(
            !is_allowed_tool(Some(&allowed), &denied, "b"),
            "denied wins"
        );
        assert!(!is_allowed_tool(None, &denied, "b"));
    }

    /// pi `_restoreToolsFromTranscript`'s read (`:1763-1766`): the replayed tool names, or nothing
    /// at all when the transcript holds no system message.
    #[test]
    fn declared_tool_names_replay_the_transcript_and_distinguish_none_from_empty() {
        use cyrup_core::{Message, SystemMessage, ToolDef, ToolReference};
        let tool = |n: &str| ToolDef {
            name: n.to_string(),
            description: String::new(),
            parameters: json!({"type": "object"}),
            constrained_sampling: None,
        };
        let row = |added: &[&str], removed: &[&str]| {
            cyrup_session::AgentMessage::Core(Message::System(SystemMessage {
                tools_added: added.iter().map(|n| tool(n)).collect(),
                tools_removed: removed.iter().map(|n| ToolReference::new(*n)).collect(),
                timestamp: 1,
                ..SystemMessage::default()
            }))
        };
        assert_eq!(
            declared_tool_names(&[]),
            None,
            "no system message declares nothing"
        );
        assert_eq!(
            declared_tool_names(&[row(&["a", "b"], &[]), row(&["c"], &["a"])]),
            Some(strings(&["b", "c"]))
        );
        assert_eq!(
            declared_tool_names(&[row(&[], &[])]),
            Some(Vec::new()),
            "a system message that declares no tool declares an EMPTY loadout"
        );
    }
}
