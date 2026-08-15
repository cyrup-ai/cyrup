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

    /// Record (or replace) a tool's prompt contribution so a tool registered AFTER the build
    /// contributes its snippet/guidelines to the rebuilt prompt (EXT-004; Pi rebuilds
    /// `_toolPromptSnippets`/`_toolPromptGuidelines` from the refreshed definition registry,
    /// agent-session.ts:2487-2506). Without this a late tool would reach the model's tool array
    /// with no prompt guidance at all.
    fn upsert_contribution(&mut self, tool: &Arc<dyn Tool>) {
        self.contributions
            .insert(tool.name().to_string(), crate::builder::tool_contribution(tool));
    }

    /// Rebuild the base system prompt for `active` tools, pulling each tool's contribution from the
    /// precomputed map (Pi `_rebuildSystemPrompt`, agent-session.ts:2304-2396).
    fn rebuild(&self, active: &[String]) -> String {
        let mut inputs = self.base.clone();
        inputs.selected_tools = Some(active.iter().map(|n| Arc::from(n.as_str())).collect());
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
    ) -> Option<(Vec<Arc<dyn Tool>>, String)> {
        let mut newly_registered: Vec<String> = Vec::new();
        let mut redefined = false;
        for t in tools {
            let name = t.name().to_string();
            self.rebuilder.upsert_contribution(&t);
            match self.registry.insert(name.clone(), t) {
                None => newly_registered.push(name),
                Some(previous) => {
                    if let Some(current) = self.registry.get(&name)
                        && definition_changed(&previous, current)
                    {
                        redefined = true;
                    }
                }
            }
        }
        // Pi filters the auto-activation through `new Set(...)`; a name already active stays once.
        newly_registered.retain(|n| !self.active.contains(n));
        if newly_registered.is_empty() && !redefined {
            return None;
        }
        let mut names = self.active.clone();
        names.extend(newly_registered);
        Some(self.set_active(&names))
    }
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
        || previous.parameters() != current.parameters()
        || previous.prompt_snippet() != current.prompt_snippet()
        || previous.prompt_guidelines() != current.prompt_guidelines()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use cyrup_core::{CancelToken, ToolCallId, ToolError, ToolResult, ToolUpdateSink};
    use serde_json::{json, Value};

    /// A tool double whose whole model-visible definition is settable, so a test can register a
    /// SECOND definition under the SAME name and prove the replacement reaches the agent.
    struct Fake {
        name: &'static str,
        description: String,
        params: Value,
        snippet: Option<String>,
    }

    impl Fake {
        fn new(name: &'static str, description: &str) -> Self {
            Self {
                name,
                description: description.to_string(),
                params: json!({"type": "object", "properties": {}}),
                snippet: None,
            }
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
        let mut st = state_with(vec![Fake::new("deploy", "v1").with_snippet("deploy: v1").arc()]);
        assert_eq!(st.active_names(), vec!["deploy".to_string()]);

        let push = st
            .merge_registered(vec![Fake::new("deploy", "v2").with_snippet("deploy: v2").arc()])
            .expect("a CHANGED definition for an existing name must still push");
        let (tools, prompt) = push;

        assert_eq!(tools.len(), 1, "the rebuilt array still holds exactly the active set");
        assert_eq!(
            tools[0].description(),
            "v2",
            "the agent must receive the NEW definition, not the one it was already running"
        );
        assert!(
            prompt.contains("deploy: v2"),
            "the rebuilt system prompt carries the new snippet: {prompt}"
        );
        assert!(!prompt.contains("deploy: v1"), "…and not the stale one: {prompt}");
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
        let mut st = state_with(vec![Fake::new("deploy", "v1").with_snippet("deploy: v1").arc()]);
        assert!(
            st.merge_registered(vec![Fake::new("deploy", "v1").with_snippet("deploy: v1").arc()])
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
        let (tools, _) = st
            .merge_registered(vec![Fake::new("audit", "new").arc()])
            .expect("a new name pushes");
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"audit"), "the late tool is active: {names:?}");
        assert!(names.contains(&"deploy"), "…without disturbing what was already active: {names:?}");
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
        let mut st = state_with(vec![Fake::new("read", "builtin").with_snippet("read: read files").arc()]);
        st.register_custom(vec![Fake::new("deploy", "custom")
            .with_snippet("deploy: ships the build")
            .arc()]);

        // Registered INERT — pi's build-time `customTools` are activated by selection, never
        // auto-activated. That half must not change.
        assert_eq!(st.active_names(), vec!["read".to_string()], "custom tools register inert");

        let (tools, prompt) =
            st.set_active(&["read".to_string(), "deploy".to_string()]);
        let names: Vec<&str> = tools.iter().map(|t| t.name()).collect();
        assert_eq!(names, ["read", "deploy"], "the custom tool is enable-able: {names:?}");
        assert!(
            prompt.contains("deploy: ships the build"),
            "the custom tool's snippet reaches the model's system prompt: {prompt}"
        );
    }
}
