//! Tool exposure: how the model reaches a tool, and which tools a request declares to it.
//!
//! Port of pi's `ToolExposure` / `ToolNamespace` / `ToolLoadout` / `ToolLoadoutChanges` model —
//! `packages/coding-agent/src/core/extensions/types.ts:495-557` @v1.0.1 — and of the one function
//! that applies it, `AgentSession._applyToolLoadout` (`core/agent-session.ts:1528-1572` @v1.0.1).
//!
//! # The two sets, and why they are different types
//!
//! A session knows three sets of tools, and conflating them is the failure this module exists to
//! make impossible:
//!
//! * **registered** — every tool the session was given (`&[Arc<dyn Tool>]`). Never shown to the
//!   model as such.
//! * **executable** — the tools the agent loop will run when the model calls them: the *active*
//!   set, minus `hidden` tools. [`ToolLoadout::executable`].
//! * **advertised** — the tools a request *declares* to the model. [`ToolLoadout::advertised`],
//!   an [`AdvertisedTools`]. It is the executable set minus the tools a `prepare_loadout` hook asked
//!   to leave out of requests (pi's `hiddenDeclarations`).
//!
//! The request builder takes an [`AdvertisedTools`], and the only way to obtain one is from a
//! [`ToolLoadout`], which is itself only built by [`ToolLoadout::resolve`]. A caller that has "all
//! the tools" in hand therefore cannot put them in a request: the type has no conversion from a
//! tool slice, so there is no filter to forget.
//!
//! # The guarantee, exactly
//!
//! For a registry `R` and a requested active-name list `N`, [`ToolLoadout::resolve`] returns a
//! loadout `L` such that, as a pure function of `(R, N)` and of what each executable tool's
//! [`Tool::prepare_loadout`] returns:
//!
//! 1. `L.executable()` is `N` deduplicated in order, with every name that is not in `R` and every
//!    tool whose [`Tool::exposure`] is [`ToolExposure::Hidden`] removed. A `hidden` tool is never
//!    executable and never advertised, whatever `N` says (upstream: "Activating it has no effect").
//! 2. `L.advertised()` is `L.executable()` minus `L.hidden_declarations()`. Nothing outside
//!    `L.executable()` is ever advertised.
//! 3. `codemode` and `deferred` tools are advertised **only if `N` names them** — they are not
//!    activated on registration ([`ToolExposure::activated_on_registration`]) — and `model-only`
//!    tools are advertised when active but are never in the *callable* view
//!    ([`ToolExposure::callable`]). Naming a `codemode` or `deferred` tool in `N` is an explicit
//!    activation, which is what `tool_search` does to load one; pi declares such a tool exactly as
//!    it would a `direct` one.
//! 4. A description a hook returns replaces the tool's description in the advertised declaration
//!    *and* in the executable tool the hooks and the dispatcher see, as upstream's
//!    `{ ...tool, description }` does. It never changes `name` or `parameters`.
//! 5. A hook that returns `Err` contributes nothing and is recorded in
//!    [`ToolLoadout::hook_failures`]; the rest of the loadout is unaffected (upstream catches and
//!    emits an extension error for `prepare_loadout`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::ToolCallId;
use crate::cancel::CancelToken;
use crate::tool::{ExecMode, Tool, ToolError, ToolRenderKind, ToolResult, ToolUpdateSink};
use crate::tool_def::ToolDef;

/// How the model reaches a tool (pi `ToolExposure`, `types.ts:509` @v1.0.1).
///
/// An explicit domain enum, not a pair of booleans: the five states have different rules for being
/// declared, being activated on registration and being callable, and those rules live in the
/// exhaustive `match`es below so that adding a sixth exposure is a compile error everywhere a
/// decision is made.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum ToolExposure {
    /// Declared to the model while active, and callable while active. The default.
    #[default]
    Direct,
    /// Declared to the model while active, never callable. For orchestrating or interactive tools.
    ModelOnly,
    /// Callable whenever registered. Not declared to the model unless explicitly activated.
    /// Codemode tools list it in their description.
    Codemode,
    /// Like [`Self::Codemode`], but codemode tools do not list it; tool search can find it.
    Deferred,
    /// Registered but unreachable. Activating it has no effect.
    Hidden,
}

/// A string that is not one of the five exposures.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("unknown tool exposure `{0}` (expected direct, model-only, codemode, deferred or hidden)")]
pub struct UnknownExposure(pub String);

impl ToolExposure {
    /// The wire spelling, which is pi's string literal.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::ModelOnly => "model-only",
            Self::Codemode => "codemode",
            Self::Deferred => "deferred",
            Self::Hidden => "hidden",
        }
    }

    /// Whether activating the tool declares it to the model by default — pi
    /// `_isDeclarable` (`agent-session.ts:3548` @v1.0.1): `direct` or `model-only`.
    pub const fn declarable(self) -> bool {
        match self {
            Self::Direct | Self::ModelOnly => true,
            Self::Codemode | Self::Deferred | Self::Hidden => false,
        }
    }

    /// Whether registering the tool activates it — pi `_isActivatedOnRegistration`
    /// (`agent-session.ts:3554` @v1.0.1): declarable, and not registered with
    /// `defaultActive: false`.
    pub const fn activated_on_registration(self, default_active: bool) -> bool {
        self.declarable() && default_active
    }

    /// Whether naming the tool in an active set can put it in a request. Everything but `hidden`:
    /// `_applyToolLoadout` drops only `hidden` tools (`agent-session.ts:1529-1532` @v1.0.1).
    pub const fn can_be_activated(self) -> bool {
        match self {
            Self::Direct | Self::ModelOnly | Self::Codemode | Self::Deferred => true,
            Self::Hidden => false,
        }
    }

    /// Whether another tool can call it through `ctx.executeTool()`, given whether it is in the
    /// active set — pi `_getCallableTools` (`agent-session.ts:1515-1520` @v1.0.1):
    /// `codemode || deferred || (direct && active)`.
    pub const fn callable(self, active: bool) -> bool {
        match self {
            Self::Codemode | Self::Deferred => true,
            Self::Direct => active,
            Self::ModelOnly | Self::Hidden => false,
        }
    }
}

impl std::str::FromStr for ToolExposure {
    type Err = UnknownExposure;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "direct" => Ok(Self::Direct),
            "model-only" => Ok(Self::ModelOnly),
            "codemode" => Ok(Self::Codemode),
            "deferred" => Ok(Self::Deferred),
            "hidden" => Ok(Self::Hidden),
            other => Err(UnknownExposure(other.to_string())),
        }
    }
}

/// A group of related tools, such as the tools of one MCP server (pi `ToolNamespace`,
/// `types.ts:527` @v1.0.1).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolNamespace {
    /// For example `mcp__docs`.
    pub name: String,
    /// Short summary shown once with the group in model-facing tool listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Longer usage guidance, such as MCP server instructions. Not part of tool listings; a tool
    /// that describes the namespace on request returns it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

/// What a [`Tool::prepare_loadout`] hook may change (pi `ToolLoadoutChanges`, `types.ts:552`
/// @v1.0.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolLoadoutChanges {
    /// Model-facing descriptions of declared tools, by tool name.
    pub descriptions: BTreeMap<String, String>,
    /// Declared tools whose declarations requests leave out. They stay active and executable.
    pub hidden_declarations: Vec<String>,
}

/// The tools of a session as a [`Tool::prepare_loadout`] hook sees them (pi `ToolLoadout`,
/// `types.ts:540` @v1.0.1). Three ordered views and two accessors.
pub struct LoadoutView<'a> {
    declared: &'a [Arc<dyn Tool>],
    callable: Vec<Arc<dyn Tool>>,
    registered: &'a [Arc<dyn Tool>],
}

impl<'a> LoadoutView<'a> {
    /// Tools declared to the model (the active tools), in order, with their original descriptions.
    pub fn declared(&self) -> &[Arc<dyn Tool>] {
        self.declared
    }

    /// Tools callable through `ctx.executeTool()`.
    pub fn callable(&self) -> &[Arc<dyn Tool>] {
        &self.callable
    }

    /// Every registered tool.
    pub fn registered(&self) -> &[Arc<dyn Tool>] {
        self.registered
    }

    /// The exposure of the registered tool `name`. An unknown name reads as `direct`, as upstream's
    /// `_getToolExposure` does (`?? "direct"`, `agent-session.ts:1507` @v1.0.1).
    pub fn exposure(&self, name: &str) -> ToolExposure {
        self.registered
            .iter()
            .rev()
            .find(|t| t.name() == name)
            .map(|t| t.exposure())
            .unwrap_or_default()
    }

    /// The namespace of the registered tool `name`, if it has one.
    pub fn namespace(&self, name: &str) -> Option<ToolNamespace> {
        self.registered
            .iter()
            .rev()
            .find(|t| t.name() == name)
            .and_then(|t| t.namespace().cloned())
    }
}

/// A `prepare_loadout` hook that failed. The hook's tool is named; the message is the error's own
/// text. Upstream reports the same pair through `emitError({ event: "prepare_loadout", … })`
/// (`agent-session.ts:1556-1561` @v1.0.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadoutHookFailure {
    pub tool: String,
    pub message: String,
}

/// A resolved tool loadout: what the loop can run and what a request may declare. Built only by
/// [`ToolLoadout::resolve`] (or [`ToolLoadout::from_tools`], which resolves over a registry that
/// is exactly the tools given). See the module docs for the guarantee.
#[derive(Clone, Default)]
pub struct ToolLoadout {
    executable: Vec<Arc<dyn Tool>>,
    hidden: BTreeSet<String>,
    failures: Vec<LoadoutHookFailure>,
}

impl ToolLoadout {
    /// No tools.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Resolve the loadout for the requested active names against the registry — pi
    /// `_applyToolLoadout` (`agent-session.ts:1528-1572` @v1.0.1). A later registry entry with the
    /// same name replaces an earlier one, as a `Map.set` does.
    pub fn resolve(requested: &[String], registry: &[Arc<dyn Tool>]) -> Self {
        let by_name: BTreeMap<&str, &Arc<dyn Tool>> =
            registry.iter().map(|t| (t.name(), t)).collect();
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let tools: Vec<Arc<dyn Tool>> = requested
            .iter()
            .filter(|name| seen.insert(name.as_str()))
            .filter_map(|name| by_name.get(name.as_str()).copied())
            .filter(|tool| tool.exposure().can_be_activated())
            .cloned()
            .collect();

        // A `Map` keeps the first slot of a re-set key and the last value.
        let mut slots: BTreeSet<&str> = BTreeSet::new();
        let deduped_registry: Vec<Arc<dyn Tool>> = registry
            .iter()
            .filter(|t| slots.insert(t.name()))
            .filter_map(|t| by_name.get(t.name()).map(|t| (*t).clone()))
            .collect();
        let active_names: BTreeSet<&str> = tools.iter().map(|t| t.name()).collect();
        let view = LoadoutView {
            declared: &tools,
            callable: callable_tools(&deduped_registry, &active_names),
            registered: &deduped_registry,
        };

        let mut descriptions: BTreeMap<String, String> = BTreeMap::new();
        let mut hidden: BTreeSet<String> = BTreeSet::new();
        let mut failures: Vec<LoadoutHookFailure> = Vec::new();
        for tool in &tools {
            match tool.prepare_loadout(&view) {
                Ok(changes) => {
                    descriptions.extend(changes.descriptions);
                    hidden.extend(changes.hidden_declarations);
                }
                Err(error) => failures.push(LoadoutHookFailure {
                    tool: tool.name().to_string(),
                    message: error.message,
                }),
            }
        }

        let executable = tools
            .iter()
            .map(|tool| match descriptions.get(tool.name()) {
                Some(description) => Arc::new(DescribedTool {
                    inner: Arc::clone(tool),
                    description: description.clone(),
                }) as Arc<dyn Tool>,
                None => Arc::clone(tool),
            })
            .collect();
        Self {
            executable,
            hidden,
            failures,
        }
    }

    /// A loadout over exactly `tools`: each is requested, and each is in the registry. What a
    /// standalone agent (one with no session registry behind it) uses.
    pub fn from_tools(tools: Vec<Arc<dyn Tool>>) -> Self {
        let names: Vec<String> = tools.iter().map(|t| t.name().to_string()).collect();
        Self::resolve(&names, &tools)
    }

    /// The tools the agent loop runs when the model calls them, in order. **Not** what a request
    /// declares — use [`Self::advertised`] for that.
    pub fn executable(&self) -> &[Arc<dyn Tool>] {
        &self.executable
    }

    /// The tools a request declares to the model.
    pub fn advertised(&self) -> AdvertisedTools<'_> {
        AdvertisedTools {
            tools: self
                .executable
                .iter()
                .filter(|t| !self.hidden.contains(t.name()))
                .collect(),
        }
    }

    /// The names whose declarations requests leave out (pi `_hiddenDeclarations`).
    pub fn hidden_declarations(&self) -> &BTreeSet<String> {
        &self.hidden
    }

    /// The declarations the TRANSCRIPT records: every executable tool, hidden declarations
    /// included — pi's `context.tools.map(toToolDeclaration)` in `declareToolChanges`
    /// (`packages/agent/src/agent-loop.ts:349-352` @v1.0.1), where `context.tools` is the executable
    /// set. A hidden declaration is recorded and then projected out of every request
    /// (`_installHiddenDeclarationsProjection`, `agent-session.ts:1721-1738` @v1.0.1), which is what
    /// lets the loadout survive a resume.
    ///
    /// Not for requests: a request declares [`Self::advertised`], and only that type yields a
    /// request's `ToolDef`s from a loadout.
    pub fn transcript_declarations(&self) -> Vec<ToolDef> {
        self.executable.iter().map(declaration_of).collect()
    }

    /// The hooks that failed while this loadout was resolved.
    pub fn hook_failures(&self) -> &[LoadoutHookFailure] {
        &self.failures
    }
}

impl From<Vec<Arc<dyn Tool>>> for ToolLoadout {
    fn from(tools: Vec<Arc<dyn Tool>>) -> Self {
        Self::from_tools(tools)
    }
}

/// The tools a request declares to the model. Obtainable only from [`ToolLoadout::advertised`].
pub struct AdvertisedTools<'a> {
    tools: Vec<&'a Arc<dyn Tool>>,
}

impl AdvertisedTools<'_> {
    /// The declarations a provider request carries, in order.
    pub fn declarations(&self) -> Vec<ToolDef> {
        self.tools.iter().map(|t| declaration_of(t)).collect()
    }

    pub fn names(&self) -> Vec<&str> {
        self.tools.iter().map(|t| t.name()).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.iter().any(|t| t.name() == name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

/// The declaration a tool presents to the model (pi `toToolDeclaration`,
/// `packages/ai/src/utils/transcript.ts:129-137` @v1.0.1): name, description, schema and the
/// PROV-011 `constrainedSampling` opt-in, copied off the tool verbatim (pi `wrapToolDefinition`,
/// `tool-definition-wrapper.ts:14`).
fn declaration_of(tool: &Arc<dyn Tool>) -> ToolDef {
    ToolDef {
        name: tool.name().to_string(),
        description: tool.description().to_string(),
        parameters: tool.parameters().clone(),
        constrained_sampling: tool.constrained_sampling().cloned(),
    }
}

/// The registered tools another tool can call, given the active names — pi `_getCallableTools`
/// (`agent-session.ts:1515-1520` @v1.0.1). `registry` order is preserved.
pub fn callable_tools(registry: &[Arc<dyn Tool>], active: &BTreeSet<&str>) -> Vec<Arc<dyn Tool>> {
    registry
        .iter()
        .filter(|t| t.exposure().callable(active.contains(t.name())))
        .cloned()
        .collect()
}

/// A tool whose model-facing description a `prepare_loadout` hook replaced. Everything else —
/// name, schema, execution, every defaulted accessor — is the wrapped tool's.
struct DescribedTool {
    inner: Arc<dyn Tool>,
    description: String,
}

#[async_trait::async_trait]
impl Tool for DescribedTool {
    fn name(&self) -> &str {
        self.inner.name()
    }
    fn parameters(&self) -> &serde_json::Value {
        self.inner.parameters()
    }
    fn execution_mode(&self) -> ExecMode {
        self.inner.execution_mode()
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn label(&self) -> Option<&str> {
        self.inner.label()
    }
    fn output_schema(&self) -> Option<&serde_json::Value> {
        self.inner.output_schema()
    }
    fn prompt_snippet(&self) -> Option<&str> {
        self.inner.prompt_snippet()
    }
    fn prompt_guidelines(&self) -> Vec<&str> {
        self.inner.prompt_guidelines()
    }
    fn render_kind(&self) -> ToolRenderKind {
        self.inner.render_kind()
    }
    fn constrained_sampling(&self) -> Option<&crate::ConstrainedSampling> {
        self.inner.constrained_sampling()
    }
    fn exposure(&self) -> ToolExposure {
        self.inner.exposure()
    }
    fn namespace(&self) -> Option<&ToolNamespace> {
        self.inner.namespace()
    }
    fn default_active(&self) -> bool {
        self.inner.default_active()
    }
    fn prepare_loadout(&self, view: &LoadoutView<'_>) -> Result<ToolLoadoutChanges, ToolError> {
        self.inner.prepare_loadout(view)
    }
    async fn prepare_arguments(&self, args: serde_json::Value) -> serde_json::Value {
        self.inner.prepare_arguments(args).await
    }
    fn render_call(&self, args: &serde_json::Value) -> Option<String> {
        self.inner.render_call(args)
    }
    fn render_result(&self, result: &serde_json::Value) -> Option<String> {
        self.inner.render_result(result)
    }
    async fn execute(
        &self,
        call_id: ToolCallId,
        params: serde_json::Value,
        cancel: CancelToken,
        on_update: ToolUpdateSink,
    ) -> Result<ToolResult, ToolError> {
        self.inner.execute(call_id, params, cancel, on_update).await
    }
}

#[cfg(test)]
mod tests;
