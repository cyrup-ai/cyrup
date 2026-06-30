//! Dynamic tool registry + system-prompt rebuild (Pi `_toolRegistry`/`_toolDefinitions` +
//! `_rebuildSystemPrompt`, agent-session.ts:786-828,2304-2396). The active tool set is mutable
//! mid-session (`setActiveToolsByName`); changing it re-derives the base system prompt from the new
//! tool snippets and re-pushes both the tool array and the prompt to the agent for the next turn.
//!
//! Tool selection was build-time-only before this module: the builder picked the active set once
//! and never re-derived. [`DynamicToolState`] keeps the full registry of enable-able tools and a
//! [`PromptRebuilder`] capturing the stable prompt inputs so the active set can be retoggled.

use std::collections::BTreeMap;
use std::sync::Arc;

use cyrup_core::Tool;
use cyrup_session::prompt::{PromptInputs, SystemPromptBuilder, ToolPromptContribution};

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
    pub(crate) fn new(base: PromptInputs, contributions: BTreeMap<String, ToolPromptContribution>) -> Self {
        Self { base, contributions }
    }

    /// Rebuild the base system prompt for `active` tools, pulling each tool's contribution from the
    /// precomputed map (Pi `_rebuildSystemPrompt`, agent-session.ts:2304-2396).
    fn rebuild(&self, active: &[String]) -> String {
        let mut inputs = self.base.clone();
        inputs.selected_tools = active.iter().map(|n| Arc::from(n.as_str())).collect();
        inputs.tool_contributions = active
            .iter()
            .filter_map(|n| self.contributions.get(n).cloned())
            .collect();
        SystemPromptBuilder::new().build(&inputs)
    }
}

/// The mutable dynamic-tool surface (Pi `_toolRegistry`/`_toolDefinitions`/`_activeToolNames`).
pub(crate) struct DynamicToolState {
    /// All enable-able tools by name (built-ins after selection + extension/custom tools).
    registry: BTreeMap<String, Arc<dyn Tool>>,
    /// The currently-active tool names, in order (Pi `agent.state.tools` names).
    active: Vec<String>,
    rebuilder: PromptRebuilder,
}

impl DynamicToolState {
    pub(crate) fn new(
        registry_tools: Vec<Arc<dyn Tool>>,
        active: Vec<Arc<dyn Tool>>,
        rebuilder: PromptRebuilder,
    ) -> Self {
        let registry: BTreeMap<String, Arc<dyn Tool>> =
            registry_tools.into_iter().map(|t| (t.name().to_string(), t)).collect();
        let active = active.into_iter().map(|t| t.name().to_string()).collect();
        Self { registry, active, rebuilder }
    }

    /// Names of the currently-active tools (Pi `getActiveToolNames`).
    pub(crate) fn active_names(&self) -> Vec<String> {
        self.active.clone()
    }

    /// All enable-able tools as [`ToolInfo`] (Pi `getAllTools`).
    pub(crate) fn all(&self) -> Vec<ToolInfo> {
        self.registry.values().map(|t| self.info_for(t)).collect()
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
            active: self.active.iter().any(|n| n == t.name()),
        }
    }

    /// Set the active set by name (Pi `setActiveToolsByName`): unknown names are ignored, the active
    /// list is replaced, and the new `(tools, system_prompt)` to push to the agent are returned.
    pub(crate) fn set_active(
        &mut self,
        names: &[String],
    ) -> (Vec<Arc<dyn Tool>>, String) {
        let mut active: Vec<String> = Vec::new();
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        for name in names {
            if let Some(t) = self.registry.get(name) {
                active.push(name.clone());
                tools.push(t.clone());
            }
        }
        self.active = active;
        let prompt = self.rebuilder.rebuild(&self.active);
        (tools, prompt)
    }

    /// Register additional custom tools into the enable-able registry (Pi `customTools`, sdk.ts:71).
    /// New tools are added but not auto-activated (parity with build-time custom-tool registration).
    pub(crate) fn register_custom(&mut self, tools: Vec<Arc<dyn Tool>>) {
        for t in tools {
            self.registry.insert(t.name().to_string(), t);
        }
    }
}
