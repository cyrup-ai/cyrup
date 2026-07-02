//! Shared agent/chain definition types (func-SA §4.1/§5.1, R-SA-001..022; arch-SA §3.3).
//!
//! Pure type definitions — no discovery/merge/parsing logic lives here. That logic is owned by
//! this module's siblings: `frontmatter.rs` (parsing), `merge.rs` (four-tier precedence merge and
//! settings-override application), `chains.rs` (chain-file discovery), `management.rs` (CRUD).
//!
//! **Correction #2 (binding):** `AgentDefinition` deliberately does **not** implement
//! `cyrup_resources::discovery::Named`, and this module does not build on top of
//! `cyrup_resources::discovery::ResourceSet<T>`. `ResourceSet::build` (`crates/cyrup-resources/
//! src/discovery.rs`) performs a single stable-sort-by-`ResourceScope::precedence_rank`-then-
//! first-key-wins dedup — a *symmetric* rule appropriate to Pi's skill/prompt/theme precedence,
//! but unable to express R-SA-002's deliberately *asymmetric* rule (Package tier is
//! first-seen-wins; User and Project tiers are last-seen-wins). `merge.rs` implements the
//! four-tier Builtin/Package/User/Project merge as a bespoke, plain algorithm directly per
//! func-SA §6.2 / arch-SA §6.2, keyed on the runtime name (`AgentDefinition::name`) via ordinary
//! `HashMap`/`Vec` operations — never by forcing `AgentDefinition` through `ResourceSet<T>`'s
//! `Named` trait bound. `AgentSource::precedence_rank` below exists for `merge.rs` to consult
//! directly, not to feed a `ResourceScope`-shaped generic primitive.
//!
//! Agent/chain **discovery plumbing** (manifest declarations, package-root enumeration, path
//! resolution) is still shared with `cyrup-resources` per R-SA-020 — only the *precedence
//! semantics* are bespoke (R-SA-021). That plumbing is consumed in `mod.rs`/`chains.rs`, not
//! here.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use cyrup_core::{ModelId, ThinkingLevel};

use crate::fork_context::ContextMode;

// ---------------------------------------------------------------------------------------------
// Provenance / scoping (R-SA-001..004, R-SA-013, R-SA-014)
// ---------------------------------------------------------------------------------------------

/// Provenance/precedence tag for an [`AgentDefinition`] or [`ChainDefinition`] (R-SA-001).
///
/// Fixed precedence order, lowest to highest: `Builtin < Package < User < Project`. On a
/// runtime-name collision, Project wins over User wins over Package wins over Builtin
/// (R-SA-001). Only `User`/`Project` are writable via management actions (R-SA-014) — a
/// create/update/delete/rename targeting a `Builtin`- or `Package`-sourced agent MUST fail with
/// a read-only error (enforced in `management.rs`, not here).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentSource {
    Builtin,
    Package,
    User,
    Project,
}

impl AgentSource {
    /// Precedence rank consulted directly by `merge.rs`'s bespoke four-tier algorithm — **lower
    /// rank wins** on a same-name collision (R-SA-001). Mirrors the *pattern* of
    /// `cyrup_resources::scope::ResourceScope::precedence_rank` (an explicit method rather than a
    /// derived `Ord`) without reusing that enum's 9-variant, symmetric-precedence semantics
    /// (R-SA-021; see module doc above).
    pub fn precedence_rank(self) -> u8 {
        match self {
            AgentSource::Project => 0,
            AgentSource::User => 1,
            AgentSource::Package => 2,
            AgentSource::Builtin => 3,
        }
    }

    /// Only `User`/`Project` sources are writable through management (create/update/delete/
    /// rename) actions (R-SA-014).
    pub fn is_writable(self) -> bool {
        matches!(self, AgentSource::User | AgentSource::Project)
    }
}

/// A read filter over discovered agents, distinct from [`AgentSource`] (func-SA §4.1). Default
/// execution scope is `Both`. Used by management/introspection listings (which MUST include
/// disabled agents, R-SA-013) as well as by execution-time discovery (which MUST exclude
/// disabled agents, R-SA-013) via a separate flag carried alongside this scope by the discovery
/// entry points in `mod.rs` — this type alone only narrows *which directories* are scanned.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentReadScope {
    User,
    Project,
    #[default]
    Both,
}

// ---------------------------------------------------------------------------------------------
// Tool / system-prompt / output shapes referenced by AgentDefinition
// ---------------------------------------------------------------------------------------------

/// One entry of an agent's `tools` allowlist, split into its three possible shapes at parse time
/// (func-SA §4.1: "builtin-tool names vs. `mcp:`-prefixed vs. extension-path entries, split at
/// parse time"). Nested-fanout eligibility (R-SA-016/DI-SA-7) is an **exact-name** check against
/// [`ToolRef::Builtin`] only — an `mcp:`-prefixed entry with the same literal name MUST NOT
/// count, and no substring/fuzzy match is permitted; that check itself is owned by the spawn
/// boundary (R-SA-041 in arch-SA numbering / func-SA R-SA-016), not this type, but the shape
/// distinction that makes an exact check possible is fixed here.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ToolRef {
    /// A literal cyrup builtin-tool identifier (e.g. `"subagent"`, `"read"`, `"edit"`).
    Builtin(String),
    /// An `mcp:`-prefixed entry (`mcp:<server>.<tool>` or similar) — the literal frontmatter
    /// string is preserved verbatim, prefix included, so it can be round-tripped and re-emitted
    /// unchanged; it is never treated as equal to a [`ToolRef::Builtin`] of the same bare name.
    Mcp(String),
    /// An extension-path entry (a resolvable extension-owned tool identifier that is neither a
    /// builtin nor `mcp:`-prefixed).
    ExtensionPath(String),
}

impl ToolRef {
    /// The literal builtin-tool identifier this extension registers for subagent delegation
    /// (R-SA-016/DI-SA-7's exact-name check target). Canonical value lives here so every
    /// consumer (discovery's own nested-fanout-eligibility note, R-SA-022, and the spawn
    /// boundary's canonical enforcement, R-SA-041/R-SA-016) references the same literal.
    pub const SUBAGENT_BUILTIN_TOOL_NAME: &'static str = "subagent";

    /// Exact-name nested-fanout eligibility check (R-SA-016/DI-SA-7): true iff this ref is the
    /// literal builtin subagent-delegation tool — never true for an `mcp:`-prefixed or
    /// extension-path entry sharing the same bare name.
    pub fn is_subagent_builtin(&self) -> bool {
        matches!(self, ToolRef::Builtin(name) if name == Self::SUBAGENT_BUILTIN_TOOL_NAME)
    }
}

/// How an agent's own frontmatter `systemPrompt` body combines with the orchestrator-injected
/// scaffolding (acceptance contract, skill pointers, project context) around it (func-SA §4.1).
/// Discovery-time default depends on the agent's **local name** (R-SA-018): `Append` when local
/// name is exactly `"delegate"` (even if packaged), else `Replace`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SystemPromptMode {
    Append,
    Replace,
}

/// Where/how a run's final output is written, at the granularity `AgentDefinition::output`
/// (agent-level default) and `RunOptions::output_mode` (per-call override) both share (func-SA
/// §4.1/§4.3). `FileOnly` requires an accompanying output path to be present somewhere in the
/// resolved chain, or the run MUST fail fast before any subprocess is spawned (R-SA-025) — that
/// validation is owned by `exec/`, not this type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputMode {
    Inline,
    FileAndInline,
    FileOnly,
}

/// An agent-level default output path/mode pair (func-SA §4.1 `AgentDefinition::output`). A
/// per-call `RunOptions` may override either field independently; absence here means "no
/// agent-level default," not "inline" — the effective default is resolved by `exec/`'s config
/// layering (R-SA-133), not by this type.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputSpec {
    pub path: Option<PathBuf>,
    pub mode: Option<OutputMode>,
}

// ---------------------------------------------------------------------------------------------
// Three-state override delta (R-SA-011)
// ---------------------------------------------------------------------------------------------

/// Three-state per-field override delta (R-SA-011). MUST NOT collapse to a plain `Option<T>` —
/// that would lose the explicit-clear sentinel, conflating "the settings layer didn't mention
/// this field" with "the settings layer explicitly wants this field removed."
///
/// - `Unset` — the override delta says nothing about this field; leave the agent's own resolved
///   value (frontmatter or prior-layer default) untouched.
/// - `ExplicitClear` — the override delta explicitly requests this field be cleared/reset to its
///   type's absence (e.g. `None`, empty `Vec`), distinct from simply never having set it.
/// - `Value(T)` — the override delta supplies a concrete replacement value.
#[derive(Clone, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverrideField<T> {
    #[default]
    Unset,
    ExplicitClear,
    Value(T),
}

impl<T> OverrideField<T> {
    /// True unless this delta is `Unset` — i.e. the override delta says *something* (either an
    /// explicit clear or an explicit value) about this field. Used by `merge.rs`'s
    /// fill-unset-only walk (R-SA-010 custom-agent branch) to decide whether a field participates
    /// in override application at all.
    pub fn is_present(&self) -> bool {
        !matches!(self, OverrideField::Unset)
    }
}

// ---------------------------------------------------------------------------------------------
// Override provenance / settings shapes (R-SA-009..012, §4.1)
// ---------------------------------------------------------------------------------------------

/// Which settings scope produced an applied [`AgentOverrideInfo`] (R-SA-012's project-beats-user
/// precedence operates over exactly these two scopes; a `subagents.overrides.<name>` entry can
/// live at either).
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OverrideScope {
    User,
    Project,
}

/// Provenance attached to an [`AgentDefinition`] whose fields were adjusted by a settings-based
/// override (func-SA §4.1). Present only when at least one override field actually applied;
/// `base_snapshot` retains the pre-override definition so callers (notably management/diagnostic
/// surfaces) can show "what changed and from where" without re-deriving it from settings state a
/// second time.
#[derive(Clone, Debug)]
pub struct AgentOverrideInfo {
    pub scope: OverrideScope,
    pub settings_path: PathBuf,
    /// The agent definition exactly as parsed from disk, before this override was applied.
    /// Boxed to avoid inflating the common (no-override) `AgentDefinition` size for the rare
    /// overridden case.
    pub base_snapshot: Box<AgentDefinition>,
}

/// Per-field three-state override delta for one agent name, as read from
/// `subagents.overrides.<name>` in `cyrup-config`'s layered, untyped settings map (func-SA §4.1).
/// Field coverage mirrors the practically-overridable subset of [`AgentDefinition`] — the
/// remaining frontmatter-only fields (e.g. `system_prompt_body`, `skills`) are not exposed for
/// settings-based override in pi-subagents' own source contract and are therefore not modeled
/// here.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentOverrideConfig {
    pub model: OverrideField<String>,
    pub fallback_models: OverrideField<Vec<String>>,
    pub thinking: OverrideField<ThinkingLevel>,
    pub tools: OverrideField<Vec<ToolRef>>,
    pub system_prompt_mode: OverrideField<SystemPromptMode>,
    pub disabled: OverrideField<bool>,
    pub max_subagent_depth: OverrideField<u32>,
    pub completion_guard: OverrideField<bool>,
}

impl AgentOverrideConfig {
    /// True iff every field in this delta is `Unset` — an override entry that, once parsed,
    /// turned out to say nothing (distinct from the entry being entirely absent from settings).
    pub fn is_empty(&self) -> bool {
        !(self.model.is_present()
            || self.fallback_models.is_present()
            || self.thinking.is_present()
            || self.tools.is_present()
            || self.system_prompt_mode.is_present()
            || self.disabled.is_present()
            || self.max_subagent_depth.is_present()
            || self.completion_guard.is_present())
    }
}

/// The `subagents` block read from `cyrup-config`'s layered, untyped settings map (func-SA
/// §4.1). A malformed value at any field here MUST abort discovery with a surfaced error
/// (R-SA-009) — that validation is owned by the settings-parsing code in `mod.rs`/`merge.rs`,
/// not this type.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SubagentSettings {
    pub overrides: BTreeMap<String, AgentOverrideConfig>,
    pub default_model: Option<String>,
    pub disable_builtins: Option<bool>,
    pub disable_thinking: Option<bool>,
}

/// Provenance for how an [`AgentDefinition`]'s final resolved `model` field was determined
/// (func-SA §4.1 `AgentDefinition::model_source`) — distinguishes "the agent's own frontmatter
/// declared this model" from "a settings-layer override supplied it" from "the crate's shared
/// global default filled it in," so diagnostic/management surfaces (and `/subagents-doctor`,
/// R-SA-131) can report *why* a given agent resolves to a given model without re-deriving the
/// config-layering walk (R-SA-133) a second time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AgentModelSourceInfo {
    /// `model` came from the agent's own on-disk frontmatter.
    Frontmatter,
    /// `model` was supplied by a `subagents.overrides.<name>.model` settings-layer override.
    SettingsOverride,
    /// `model` was filled in from `subagents.defaultModel` (no frontmatter value, no per-agent
    /// override).
    SettingsDefault,
    /// No model was ever resolved from frontmatter or settings; the agent has none and a
    /// call-site/global fallback (owned by `exec/`, R-SA-041) will apply at run time.
    Unresolved,
}

// ---------------------------------------------------------------------------------------------
// AgentDefinition (R-SA-005..022)
// ---------------------------------------------------------------------------------------------

/// The parsed, resolved form of one agent `.md` file (func-SA §4.1). Deliberately does **not**
/// implement `cyrup_resources::discovery::Named` — see module doc.
#[derive(Clone, Debug)]
pub struct AgentDefinition {
    /// Fully-qualified runtime name: `{package}.{local_name}` when `package_name` is set, else
    /// the bare `local_name` (R-SA-008). Selection at execution time matches against this field
    /// via exact string equality only.
    pub name: String,
    /// The agent's local (pre-packaging) name — the value R-SA-018's name-sensitive defaults
    /// (`systemPromptMode`/`inheritProjectContext`) are computed from, and the value that
    /// participates in `name` construction above.
    pub local_name: String,
    pub package_name: Option<String>,
    pub description: String,
    /// `None` = no allowlist restriction (all builtin tools available); `Some(vec![])` = no
    /// tools; `Some(populated)` = exactly this allowlist. Distinct from `extensions` below, which
    /// has an independently-meaningful `None`/empty/populated tri-state (func-SA §4.1).
    pub tools: Option<Vec<ToolRef>>,
    /// `None` = all extensions visible; `Some(vec![])` = none; `Some(populated)` = allowlist
    /// (func-SA §4.1).
    pub extensions: Option<Vec<String>>,
    /// Child-only extension paths — visible to a spawned subagent even when not visible to the
    /// orchestrator itself.
    pub subagent_only_extensions: Vec<String>,
    pub model: Option<ModelId>,
    pub fallback_models: Vec<ModelId>,
    pub thinking: Option<ThinkingLevel>,
    /// Defaults to `Append` when `local_name == "delegate"`, else `Replace` (R-SA-018) — the
    /// default is computed by `frontmatter.rs` at parse time; this field always holds the final
    /// resolved value, never a sentinel for "not yet defaulted."
    pub system_prompt_mode: SystemPromptMode,
    /// Defaults to `true` when `local_name == "delegate"`, else `false` (R-SA-018).
    pub inherit_project_context: bool,
    pub inherit_skills: bool,
    /// Skill-pointer names (not full content) proactively injected into this agent's assembled
    /// system prompt at spawn time (R-SA-017) — orthogonal to any on-demand skill-content loading
    /// the child performs for itself once running.
    pub skills: Vec<String>,
    /// Pre-declared read-context default for calls that omit an explicit `reads` list.
    pub default_reads: Option<Vec<PathBuf>>,
    /// Pre-declared progress-visibility default for calls that omit an explicit setting.
    pub default_progress: Option<bool>,
    /// Agent-level default output path/mode (func-SA §4.1).
    pub output: Option<OutputSpec>,
    /// `Some(false)` disables the completion-mutation guard for this agent entirely (R-SA-034);
    /// `None`/`Some(true)` leaves the guard active subject to that subsystem's own read-only-tools
    /// short-circuit.
    pub completion_guard: Option<bool>,
    /// Parsed but **unenforced in v1** (func-SA §4.1): round-tripped for forward compatibility.
    /// MUST NOT be silently dropped from `extra_fields`/serialization even though no code path
    /// currently consults it. Enforcement (if ever added) is out of scope for this crate's
    /// current build-out phases and is not tracked by any file this crate currently owns.
    pub interactive: Option<bool>,
    pub max_subagent_depth: Option<u32>,
    /// `None` = fall through to this crate's own default (`ContextMode::Fresh`, computed by
    /// `merge.rs`/`exec/`, not defaulted eagerly here so `present_fields` can still distinguish
    /// "agent declared `defaultContext`" from "agent said nothing").
    pub default_context: Option<ContextMode>,
    pub disabled: Option<bool>,
    /// The agent's own frontmatter-body prose, prior to any orchestrator-injected scaffolding
    /// (acceptance contract, skill pointers, project context) — combined per `system_prompt_mode`
    /// by `exec/` at spawn time, never mutated here.
    pub system_prompt_body: String,
    pub source: AgentSource,
    pub file_path: PathBuf,
    /// Which frontmatter keys were literally present on disk — required for R-SA-010's
    /// fill-unset-only override semantics (a custom-agent override MUST be blocked for any field
    /// present here, regardless of the override's own value).
    pub present_fields: HashSet<String>,
    /// Unknown-key round-trip preservation: any frontmatter key not recognized by this crate's
    /// parser is preserved verbatim here (as its raw string value) so re-serialization does not
    /// silently drop caller-authored data.
    pub extra_fields: BTreeMap<String, String>,
    /// Populated only when a settings-based override actually applied to this definition
    /// (R-SA-010/011/012).
    pub override_info: Option<AgentOverrideInfo>,
    /// Provenance for how `model` above was ultimately resolved (see [`AgentModelSourceInfo`]).
    pub model_source: Option<AgentModelSourceInfo>,
}

impl AgentDefinition {
    /// R-SA-008: runtime/qualified name construction — `{package}.{local_name}` when a package
    /// is set, else the bare local name. Exposed as an associated function (rather than folded
    /// silently into a constructor) so `frontmatter.rs`/`merge.rs` can (re)compute `name`
    /// explicitly and visibly at every point it might change (e.g. after package-context
    /// resolution).
    pub fn qualified_name(local_name: &str, package_name: Option<&str>) -> String {
        match package_name {
            Some(pkg) if !pkg.is_empty() => format!("{pkg}.{local_name}"),
            _ => local_name.to_string(),
        }
    }

    /// R-SA-016/DI-SA-7: nested-fanout eligibility is an exact-name check on this agent's
    /// *resolved* `tools` field only — `None` (no allowlist restriction, i.e. all builtins
    /// available) also counts as eligible, since an unrestricted agent implicitly has access to
    /// the builtin subagent-delegation tool. Canonical enforcement point remains the spawn
    /// boundary (R-SA-041 arch-numbering); this helper exists so discovery-time diagnostics can
    /// answer the same question without duplicating the match logic.
    pub fn is_nested_fanout_eligible(&self) -> bool {
        match &self.tools {
            None => true,
            Some(refs) => refs.iter().any(ToolRef::is_subagent_builtin),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Chain definitions (R-SA-015)
// ---------------------------------------------------------------------------------------------

/// One parsed chain (`.chain.md`/`.chain.json`) definition (func-SA §4.1/§4.2). `steps`' element
/// type, `RunnerStep`, is canonically owned by `spawn::chain_graph` (arch-SA §2.2) — this module
/// only references it, never redefines or re-derives its shape, matching this crate's
/// one-canonical-owner convention for cross-module types (see `fork_context.rs`'s own doc for the
/// same pattern applied to `ContextMode`). `spawn::chain_graph` is built out in a later phase of
/// this crate's build-out (arch-SA §2.2's Phase 3); until then this field type-checks against a
/// forward declaration that phase is solely responsible for providing.
#[derive(Clone, Debug)]
pub struct ChainDefinition {
    pub name: String,
    pub description: String,
    pub source: AgentSource,
    pub file_path: PathBuf,
    pub steps: Vec<crate::spawn::chain_graph::RunnerStep>,
}

/// A non-fatal per-file parse error surfaced by list/get operations over chain files (func-SA
/// §4.1). Never aborts discovery of sibling chain files (R-SA-009's three-way throw/silent-skip/
/// diagnostic distinction: a malformed chain file is the "diagnostic" case, neither the abort
/// reserved for malformed settings nor the silent skip reserved for malformed agent frontmatter).
#[derive(Clone, Debug)]
pub struct ChainDiscoveryDiagnostic {
    pub file_path: PathBuf,
    pub source: AgentSource,
    pub message: String,
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[test]
    fn agent_source_precedence_rank_orders_project_highest() {
        assert!(AgentSource::Project.precedence_rank() < AgentSource::User.precedence_rank());
        assert!(AgentSource::User.precedence_rank() < AgentSource::Package.precedence_rank());
        assert!(AgentSource::Package.precedence_rank() < AgentSource::Builtin.precedence_rank());
    }

    #[test]
    fn only_user_and_project_sources_are_writable() {
        assert!(!AgentSource::Builtin.is_writable());
        assert!(!AgentSource::Package.is_writable());
        assert!(AgentSource::User.is_writable());
        assert!(AgentSource::Project.is_writable());
    }

    #[test]
    fn qualified_name_uses_package_prefix_only_when_present() {
        assert_eq!(
            AgentDefinition::qualified_name("reviewer", Some("acme")),
            "acme.reviewer"
        );
        assert_eq!(
            AgentDefinition::qualified_name("reviewer", None),
            "reviewer"
        );
        assert_eq!(
            AgentDefinition::qualified_name("reviewer", Some("")),
            "reviewer"
        );
    }

    #[test]
    fn tool_ref_exact_name_match_excludes_mcp_and_extension_variants() {
        let builtin = ToolRef::Builtin(ToolRef::SUBAGENT_BUILTIN_TOOL_NAME.to_string());
        let mcp = ToolRef::Mcp(format!("mcp:{}", ToolRef::SUBAGENT_BUILTIN_TOOL_NAME));
        let ext = ToolRef::ExtensionPath(ToolRef::SUBAGENT_BUILTIN_TOOL_NAME.to_string());

        assert!(builtin.is_subagent_builtin());
        assert!(!mcp.is_subagent_builtin());
        assert!(!ext.is_subagent_builtin());
    }

    #[test]
    fn override_field_is_present_distinguishes_unset_from_clear_and_value() {
        let unset: OverrideField<u32> = OverrideField::Unset;
        let cleared: OverrideField<u32> = OverrideField::ExplicitClear;
        let valued: OverrideField<u32> = OverrideField::Value(7);

        assert!(!unset.is_present());
        assert!(cleared.is_present());
        assert!(valued.is_present());
    }

    #[test]
    fn agent_override_config_default_is_empty() {
        let cfg = AgentOverrideConfig::default();
        assert!(cfg.is_empty());
    }

    #[test]
    fn agent_override_config_with_one_field_set_is_not_empty() {
        let cfg = AgentOverrideConfig {
            thinking: OverrideField::Value(ThinkingLevel::High),
            ..Default::default()
        };
        assert!(!cfg.is_empty());
    }

    fn sample_agent(tools: Option<Vec<ToolRef>>) -> AgentDefinition {
        AgentDefinition {
            name: "reviewer".to_string(),
            local_name: "reviewer".to_string(),
            package_name: None,
            description: "reviews things".to_string(),
            tools,
            extensions: None,
            subagent_only_extensions: Vec::new(),
            model: None,
            fallback_models: Vec::new(),
            thinking: None,
            system_prompt_mode: SystemPromptMode::Replace,
            inherit_project_context: false,
            inherit_skills: false,
            skills: Vec::new(),
            default_reads: None,
            default_progress: None,
            output: None,
            completion_guard: None,
            interactive: None,
            max_subagent_depth: None,
            default_context: None,
            disabled: None,
            system_prompt_body: String::new(),
            source: AgentSource::User,
            file_path: PathBuf::from("/tmp/reviewer.md"),
            present_fields: HashSet::new(),
            extra_fields: BTreeMap::new(),
            override_info: None,
            model_source: None,
        }
    }

    #[test]
    fn nested_fanout_eligibility_requires_exact_builtin_name() {
        // No allowlist restriction at all: implicitly eligible.
        assert!(sample_agent(None).is_nested_fanout_eligible());

        // Exact literal builtin identifier: eligible.
        let with_builtin = sample_agent(Some(vec![ToolRef::Builtin("subagent".to_string())]));
        assert!(with_builtin.is_nested_fanout_eligible());

        // `mcp:`-prefixed entry with the same bare name: NOT eligible (R-SA-016/DI-SA-7).
        let with_mcp = sample_agent(Some(vec![ToolRef::Mcp("mcp:subagent".to_string())]));
        assert!(!with_mcp.is_nested_fanout_eligible());

        // Extension-path entry with the same bare name: NOT eligible.
        let with_ext = sample_agent(Some(vec![ToolRef::ExtensionPath("subagent".to_string())]));
        assert!(!with_ext.is_nested_fanout_eligible());

        // Empty allowlist: NOT eligible.
        let with_empty = sample_agent(Some(Vec::new()));
        assert!(!with_empty.is_nested_fanout_eligible());
    }

    #[test]
    fn agent_read_scope_default_is_both() {
        assert_eq!(AgentReadScope::default(), AgentReadScope::Both);
    }
}
