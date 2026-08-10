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

use cyrup_core::ModelId;

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
// Adjacently tagged (`tag` + `content`), not internally tagged: every variant is a newtype
// wrapping a bare `String`, and serde's internal tagging (`tag` alone) cannot represent a
// newtype-over-primitive variant — it can only inject the tag key into a variant that itself
// serializes as a map/struct, so a `Builtin("read")` would fail to serialize at all ("cannot
// serialize tagged newtype variant ToolRef::Builtin containing a string"). Adjacent tagging emits
// `{"kind":"builtin","content":"read"}`, which round-trips losslessly and still preserves the
// variant distinction (a bare-string form could not tell `Builtin("read")` from
// `ExtensionPath("read")`). This makes the `tools` field of the serializable `ResolvedAgentPersona`
// survive the RunnerConfig JSON round-trip that carries the real persona to the re-exec'd child.
#[derive(Clone, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(tag = "kind", content = "content", rename_all = "camelCase")]
pub enum ToolRef {
    /// A literal cyrup builtin-tool identifier (e.g. `"subagent"`, `"read"`, `"edit"`).
    Builtin(String),
    /// An `mcp:`-prefixed tool entry. The `mcp:` prefix is STRIPPED when this variant is built
    /// from a raw tools-list string (`from_tool_string`/`frontmatter::parse_tool_refs`/pi's
    /// `splitToolList`), so it holds the bare `<server>.<tool>` identifier; the `Mcp` variant tag
    /// itself (not a textual prefix) is what keeps it distinct from a [`ToolRef::Builtin`] of the
    /// same bare name across the adjacently-tagged serde round-trip.
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

    /// Split one raw `tools`-list string into its typed [`ToolRef`] shape, exactly as
    /// `frontmatter::parse_tool_refs` and pi's `splitToolList` (`agents.ts:438-452`) do: an
    /// `mcp:`-prefixed entry becomes [`ToolRef::Mcp`] with the `mcp:` PREFIX STRIPPED (so
    /// `"mcp:server.tool"` → `Mcp("server.tool")`, matching pi's `tool.slice(4)`); every other
    /// entry becomes [`ToolRef::Builtin`]. Used by this type's own string-form [`Deserialize`](
    /// serde::Deserialize) so a pi-shaped settings `tools: ["bash", "mcp:x"]` array deserializes
    /// identically to how the frontmatter parser splits the same list.
    pub fn from_tool_string(raw: &str) -> ToolRef {
        match raw.strip_prefix("mcp:") {
            Some(rest) => ToolRef::Mcp(rest.to_string()),
            None => ToolRef::Builtin(raw.to_string()),
        }
    }
}

// `ToolRef` SERIALIZES (derive above) as the adjacently-tagged `{"kind":..,"content":..}` map so the
// `Builtin`/`Mcp`/`ExtensionPath` distinction round-trips losslessly through the `RunnerConfig` JSON
// that carries a resolved persona to the re-exec'd child. Its `Deserialize`, hand-written here,
// accepts BOTH that map form (for the round-trip) AND a bare tool-name string (for pi-shaped
// `settings.json` `tools`/override lists, where an entry is just `"bash"` or `"mcp:x"`).
impl<'de> serde::Deserialize<'de> for ToolRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ToolRefVisitor;

        impl<'de> serde::de::Visitor<'de> for ToolRefVisitor {
            type Value = ToolRef;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(
                    "a tool-name string (e.g. \"read\" or \"mcp:server.tool\") or a {kind, content} object",
                )
            }

            fn visit_str<E>(self, v: &str) -> Result<ToolRef, E>
            where
                E: serde::de::Error,
            {
                Ok(ToolRef::from_tool_string(v))
            }

            fn visit_map<A>(self, mut map: A) -> Result<ToolRef, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut kind: Option<String> = None;
                let mut content: Option<String> = None;
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "kind" => {
                            if kind.is_some() {
                                return Err(serde::de::Error::duplicate_field("kind"));
                            }
                            kind = Some(map.next_value()?);
                        }
                        "content" => {
                            if content.is_some() {
                                return Err(serde::de::Error::duplicate_field("content"));
                            }
                            content = Some(map.next_value()?);
                        }
                        _ => {
                            let _: serde::de::IgnoredAny = map.next_value()?;
                        }
                    }
                }
                let kind = kind.ok_or_else(|| serde::de::Error::missing_field("kind"))?;
                let content = content.ok_or_else(|| serde::de::Error::missing_field("content"))?;
                match kind.as_str() {
                    "builtin" => Ok(ToolRef::Builtin(content)),
                    "mcp" => Ok(ToolRef::Mcp(content)),
                    "extensionPath" => Ok(ToolRef::ExtensionPath(content)),
                    other => Err(serde::de::Error::unknown_variant(
                        other,
                        &["builtin", "mcp", "extensionPath"],
                    )),
                }
            }
        }

        deserializer.deserialize_any(ToolRefVisitor)
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
#[derive(Clone, PartialEq, Eq, Debug, Default)]
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

    /// True iff this delta is `Unset`. Used as a `#[serde(skip_serializing_if)]` predicate on every
    /// [`AgentOverrideConfig`] field so an untouched override field is OMITTED from serialized
    /// settings/profile JSON — rather than emitted as `null`, which the three-state `Deserialize`
    /// below (which reads a JSON `false` as [`OverrideField::ExplicitClear`] and any other value as
    /// [`OverrideField::Value`]) could not round-trip back to `Unset`.
    pub fn is_unset(&self) -> bool {
        matches!(self, OverrideField::Unset)
    }
}

/// Deserialize-only sentinel that accepts EXCLUSIVELY the JSON literal boolean `false` — the shape
/// pi uses to explicitly CLEAR a `string | false` / `string[] | false` override field
/// (`agents.ts:66-77`). It is tried only AFTER a concrete `Value(T)` (see the `untagged` helper in
/// [`OverrideField`]'s `Deserialize`), which is what lets a genuinely boolean-typed override field
/// (`disabled`/`completionGuard`, where `false` is a real VALUE, not a clear) keep `false` as
/// `Value(false)` while a `string`/`array`-typed field reads `false` as
/// [`OverrideField::ExplicitClear`].
struct OverrideClearSentinel;

impl<'de> serde::Deserialize<'de> for OverrideClearSentinel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct FalseOnly;

        impl<'de> serde::de::Visitor<'de> for FalseOnly {
            type Value = OverrideClearSentinel;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("the literal boolean `false` (an explicit-clear sentinel)")
            }

            fn visit_bool<E>(self, v: bool) -> Result<OverrideClearSentinel, E>
            where
                E: serde::de::Error,
            {
                if v {
                    Err(E::invalid_value(serde::de::Unexpected::Bool(true), &self))
                } else {
                    Ok(OverrideClearSentinel)
                }
            }
        }

        deserializer.deserialize_bool(FalseOnly)
    }
}

impl<T: serde::Serialize> serde::Serialize for OverrideField<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            // Never actually reached for an [`AgentOverrideConfig`] field (each is
            // `skip_serializing_if = "OverrideField::is_unset"`); `null` is the least-surprising
            // fallback should any other serializer reach it directly.
            OverrideField::Unset => serializer.serialize_none(),
            OverrideField::ExplicitClear => serializer.serialize_bool(false),
            OverrideField::Value(v) => v.serialize(serializer),
        }
    }
}

impl<'de, T> serde::Deserialize<'de> for OverrideField<T>
where
    T: serde::Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Try a concrete `Value(T)` FIRST; only a genuine JSON `false` that is not itself a valid
        // `T` (i.e. a `string|false` / `array|false` field) falls through to the clear sentinel.
        // For a `bool`-typed `T` (`disabled`/`completionGuard`), `false` deserializes as
        // `Value(false)` and never reaches the sentinel — matching pi, where those fields are plain
        // booleans and `false` is a real value.
        #[derive(serde::Deserialize)]
        #[serde(untagged)]
        enum Raw<U> {
            Value(U),
            Clear(OverrideClearSentinel),
        }

        match Raw::<T>::deserialize(deserializer)? {
            Raw::Value(v) => Ok(OverrideField::Value(v)),
            Raw::Clear(OverrideClearSentinel) => Ok(OverrideField::ExplicitClear),
        }
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
/// `subagents.agentOverrides.<name>` in `cyrup-config`'s layered, untyped settings map (func-SA
/// §4.1). **This is a field-for-field port of pi's `BuiltinAgentOverrideConfig`
/// (`agents.ts:65-79`)** — every field below is exactly one pi override field, and pi has no
/// others:
///
/// - pi's `string | false` / `string[] | false` / `AgentDefaultContext | false` fields
///   (`model`/`fallbackModels`/`thinking`/`defaultContext`/`skills`/`tools`/
///   `subagentOnlyExtensions`) are three-state [`OverrideField<T>`] where a JSON `false`
///   deserializes to [`OverrideField::ExplicitClear`] (pi's explicit "reset this field" sentinel).
/// - pi's plain `boolean` / `SystemPromptMode` / `string` fields (no `| false`:
///   `systemPromptMode`/`inheritProjectContext`/`inheritSkills`/`disabled`/`systemPrompt`/
///   `completionGuard`) are [`OverrideField<T>`] that only ever carry
///   [`OverrideField::Value`]/[`OverrideField::Unset`] (a JSON `false` for a `bool`-typed field is
///   a real `Value(false)`, never a clear).
///
/// **Not present:** pi deliberately has NO `maxSubagentDepth` settings override — an agent's
/// `maxSubagentDepth` is a frontmatter / `config.json` concern only, resolved by
/// `registration::resolve_effective_config`, never by a per-agent settings-override delta. An
/// earlier port invented one; it is dropped here (Tier 7).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AgentOverrideConfig {
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub model: OverrideField<String>,
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub fallback_models: OverrideField<Vec<String>>,
    /// pi's `subagents.overrides.<name>.thinking` (`agents.ts:66,599-602`) is a `string | false`:
    /// an OPEN reasoning-level string (`"off"`, `"high"`, or any future/provider-specific value),
    /// or the literal `false` (explicit-clear). Modeled as `OverrideField<String>` — NOT a closed
    /// [`cyrup_core::ThinkingLevel`] enum — so an arbitrary pi thinking value (notably `"off"`, which
    /// the on-only enum cannot name) deserializes as `Value(..)` rather than erroring the whole
    /// settings load, and `false` still reads as [`OverrideField::ExplicitClear`].
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub thinking: OverrideField<String>,
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub system_prompt_mode: OverrideField<SystemPromptMode>,
    /// pi `inheritProjectContext?: boolean` (`agents.ts:70`). A plain boolean toggle (no `| false`
    /// clear form): `Value(true)`/`Value(false)` are both real settings.
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub inherit_project_context: OverrideField<bool>,
    /// pi `inheritSkills?: boolean` (`agents.ts:71`).
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub inherit_skills: OverrideField<bool>,
    /// pi `defaultContext?: AgentDefaultContext | false` (`agents.ts:72`) — `"fresh"`/`"fork"`
    /// deserialize to [`OverrideField::Value`], a JSON `false` to [`OverrideField::ExplicitClear`].
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub default_context: OverrideField<ContextMode>,
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub disabled: OverrideField<bool>,
    /// pi `systemPrompt?: string` (`agents.ts:74`). Applied to a BUILTIN agent's body only (pi's
    /// `applyBuiltinOverride` sets it; `applyCustomAgentOverride` deliberately omits it), replacing
    /// the builtin persona's own frontmatter prose.
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub system_prompt: OverrideField<String>,
    /// pi `skills?: string[] | false` (`agents.ts:75`) — the proactive skill-pointer list, or a
    /// JSON `false` to clear it.
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub skills: OverrideField<Vec<String>>,
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub tools: OverrideField<Vec<ToolRef>>,
    /// pi `subagentOnlyExtensions?: string[] | false` (`agents.ts:77`) — child-only extension
    /// paths, or a JSON `false` to clear them.
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub subagent_only_extensions: OverrideField<Vec<String>>,
    #[serde(skip_serializing_if = "OverrideField::is_unset")]
    pub completion_guard: OverrideField<bool>,
}

impl AgentOverrideConfig {
    /// True iff every field in this delta is `Unset` — an override entry that, once parsed,
    /// turned out to say nothing (distinct from the entry being entirely absent from settings).
    pub fn is_empty(&self) -> bool {
        !(self.model.is_present()
            || self.fallback_models.is_present()
            || self.thinking.is_present()
            || self.system_prompt_mode.is_present()
            || self.inherit_project_context.is_present()
            || self.inherit_skills.is_present()
            || self.default_context.is_present()
            || self.disabled.is_present()
            || self.system_prompt.is_present()
            || self.skills.is_present()
            || self.tools.is_present()
            || self.subagent_only_extensions.is_present()
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
    /// pi's `subagents.agentOverrides` (`agents.ts:127/705`) — per-agent-name override deltas. The
    /// Rust field keeps its role-name `overrides`, but the on-disk/JSON key is `agentOverrides` so a
    /// real pi-authored `settings.json` deserializes.
    #[serde(rename = "agentOverrides", skip_serializing_if = "BTreeMap::is_empty")]
    pub overrides: BTreeMap<String, AgentOverrideConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_builtins: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub disable_thinking: Option<bool>,
    /// pi's `subagents.modelScope` (`agents.ts:144/193/731`) — the optional allow-list policy
    /// constraining which models a subagent may run on (SUBA-003).
    ///
    /// **`skip_deserializing` is load-bearing**: pi validates this block with a bespoke parser
    /// (`parseModelScopeConfig`) whose diagnostics are part of R-SA-009's MUST-abort contract, so
    /// the field is populated by [`crate::discovery::parse_subagent_settings`] from the raw
    /// [`serde_json::Value`] rather than by serde's derived impl, which would report a generic
    /// type error instead. Any hand-built [`SubagentSettings`] therefore starts with no scope,
    /// which is the correct default (enforcement off).
    #[serde(default, skip_deserializing, skip_serializing_if = "Option::is_none")]
    pub model_scope: Option<crate::exec::model_scope::ModelScopeConfig>,
}

/// The user- and project-scope [`SubagentSettings`] pair, each with its own on-disk
/// `settings.json` path, carried **UNFLATTENED** so `merge.rs` can resolve project-beats-user
/// precedence at APPLICATION time and record the real winning scope + settings-file path in
/// [`AgentOverrideInfo`] — exactly as pi's `discoverAgents` holds `userSettings`/`projectSettings`
/// plus `userSettingsPath`/`projectSettingsPath` and hands all four to
/// `applyBuiltinOverrides`/`applyCustomAgentOverrides` (`agents.ts:785-943`, `1282-1298`).
///
/// Pre-flattening the two scopes into a single [`SubagentSettings`] (the pre-Tier-7 shape)
/// irrecoverably loses *which* scope an applied override came from — which is why the provenance
/// was previously always recorded as `Project` with the agent's own `.md` path. This two-scope
/// carrier is what lets `merge.rs` distinguish a `user`-scope override applied to a `project`-scope
/// agent (R-SA-012), and stamp `settings_path` with the real `settings.json` rather than the agent
/// file.
///
/// `project_settings_path` is `None` only when there is no project scope at all (no project root);
/// an existing-but-empty project `settings.json` is `Some(path)` with an empty
/// [`SubagentSettings::project`], mirroring pi's non-null `projectSettingsPath` + empty
/// `projectSettings` — the two are distinguished by the `is_some()` gate on every project-scope
/// application branch (a project override / bulk-disable / `disableThinking` only fires when the
/// project scope actually exists).
#[derive(Clone, Debug, Default)]
pub struct LayeredOverrideSettings {
    /// The user-scope `subagents.*` block (from `~/.cyrup/agents/settings.json`).
    pub user: SubagentSettings,
    /// The project-scope `subagents.*` block (from `<cwd>/.cyrup/agents/settings.json`); all-default
    /// when the project scope is absent or the file does not exist.
    pub project: SubagentSettings,
    /// The user `settings.json` path, recorded verbatim into [`AgentOverrideInfo::settings_path`]
    /// when a user-scope override (or user-scope `disableThinking`) applies.
    pub user_settings_path: PathBuf,
    /// The project `settings.json` path (`None` iff there is no project scope). Gates every
    /// project-scope application branch and is recorded into [`AgentOverrideInfo::settings_path`]
    /// when a project-scope override applies.
    pub project_settings_path: Option<PathBuf>,
}

impl LayeredOverrideSettings {
    /// The effective `subagents.modelScope` policy for this cwd — pi's
    /// `projectSettings.modelScope ?? userSettings.modelScope` (`agents.ts:1404`): the project
    /// scope wins outright when present (it is not merged field-by-field with the user scope),
    /// else the user scope applies, else there is no policy and enforcement is off.
    #[must_use]
    pub fn model_scope(&self) -> Option<crate::exec::model_scope::ModelScopeConfig> {
        self.project.model_scope.clone().or_else(|| self.user.model_scope.clone())
    }
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
// Per-agent persistent memory (`memory:` frontmatter) — pi `agents/agent-memory.ts`
// ---------------------------------------------------------------------------------------------

/// Which root a `memory:` scope resolves under — a direct port of pi's
/// `AgentMemoryConfig["scope"]` (`agent-memory.ts:54`), which admits exactly `"project"` and
/// `"user"` and treats every other value as "no memory config at all".
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryScope {
    /// `<projectConfigDir>/agent-memory/<path>` — resolved against the nearest project root
    /// (`agent-memory.ts:201-204`). An agent with a project scope and NO discoverable project root
    /// gets no memory block at all.
    Project,
    /// `<agentDir>/agent-memory/<path>` (`agent-memory.ts:199`).
    User,
}

/// One agent's `memory:` frontmatter block, parsed (pi `AgentMemoryConfig`). BOTH fields are
/// required — `parseMemoryFrontmatter` returns `undefined` unless `scope` is one of the two legal
/// values AND a non-empty `path` is present (`agent-memory.ts:53-58`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentMemoryConfig {
    pub scope: MemoryScope,
    /// A RELATIVE path under the scope root. Containment is enforced at resolve time
    /// (`crate::discovery::agent_memory::resolve_memory_dir`), not here — the parser stores the
    /// raw declared value so serialization round-trips exactly what the author wrote.
    pub path: String,
}

// ---------------------------------------------------------------------------------------------
// Per-agent tool budgets (`toolBudget:` frontmatter) — pi `runs/shared/tool-budget.ts`
// ---------------------------------------------------------------------------------------------

/// Which tools a hard-exhausted budget blocks — pi `ToolBudgetConfig["block"]`
/// (`shared/types.ts`), normalized by `normalizeToolBudgetBlock` (`tool-budget.ts:8-12`).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ToolBudgetBlock {
    /// The literal `"*"` — block EVERY tool once the hard limit is crossed.
    All(AllToolsMarker),
    /// An explicit tool-name list. An omitted `block` normalizes to pi's
    /// `DEFAULT_TOOL_BUDGET_BLOCK` (`["read", "grep", "find", "ls"]`, `tool-budget.ts:3`).
    Names(Vec<String>),
}

/// The `"*"` literal, as a type so [`ToolBudgetBlock`] can serialize back to the exact JSON shape
/// pi's schema advertises (`{"anyOf": [{"type":"array",…}, {"type":"string","enum":["*"]}]}`,
/// `schemas.ts:82-87`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AllToolsMarker;

impl serde::Serialize for AllToolsMarker {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str("*")
    }
}

impl<'de> serde::Deserialize<'de> for AllToolsMarker {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        if raw == "*" {
            Ok(AllToolsMarker)
        } else {
            Err(serde::de::Error::custom("expected \"*\""))
        }
    }
}

/// A VALIDATED, normalized tool budget — pi `ResolvedToolBudget`. Produced only by
/// [`crate::exec::tool_budget::validate_tool_budget_config`], never constructed straight from
/// user input, so every instance satisfies `hard >= 1`, `soft >= 1`, `soft <= hard` and a
/// non-empty `block`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolvedToolBudget {
    /// Tool calls allowed before [`ToolBudgetBlock`] tools start being refused. Integer >= 1.
    pub hard: u32,
    /// Optional advisory threshold: at this count the child is nudged once to wrap up. Integer
    /// >= 1 and <= `hard`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft: Option<u32>,
    pub block: ToolBudgetBlock,
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
    /// The agent's own frontmatter `thinking` value, held as pi's OPEN reasoning-level string
    /// (`AgentConfig.thinking?: string`, `agents.ts:103,171`) rather than a closed
    /// [`cyrup_core::ThinkingLevel`] enum. `None` means the frontmatter said nothing (unset);
    /// `Some("off")` is an EXPLICIT off (distinct from unset — the on-only enum could name neither);
    /// `Some("high")`/etc. are on-levels; any other string (a future or provider-specific level) is
    /// preserved verbatim rather than dropped. The wired child-spawn path suffixes the model id with
    /// `:<value>` via [`crate::exec::apply_thinking_suffix`], which recognizes `off` and every
    /// on-level, so an explicit `off` now reaches the child instead of being silently swallowed.
    pub thinking: Option<String>,
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
    /// pi `AgentConfig.defaultAsync` (`agents.ts:131`), from `async:` frontmatter. An agent-level
    /// LAUNCH DEFAULT: it applies ONLY when a single-agent call site omits `async` entirely
    /// (`applySingleAgentLaunchDefaults`, `subagent-executor.ts:1929-1946`). It never overrides an
    /// explicit call-site value, and never applies to a chain/parallel launch.
    pub default_async: Option<bool>,
    /// pi `AgentConfig.defaultTimeoutMs` (`agents.ts:132`), from `timeoutMs:` frontmatter. Same
    /// launch-default precedence as [`Self::default_async`], with the extra rule that an explicit
    /// call-site `maxRuntimeMs` (the alias of `timeoutMs`) ALSO suppresses it
    /// (`subagent-executor.ts:1937`).
    pub default_timeout_ms: Option<u64>,
    /// The agent's `memory:` scope (pi `AgentConfig.memory`, `agents.ts` + `agent-memory.ts`).
    /// `None` means the agent declared none, or declared one that failed validation (pi's
    /// `parseMemoryFrontmatter` returns `undefined` for both). When set, spawn time resolves it to
    /// a directory and folds a persistent-memory block into the child's system prompt — see
    /// [`crate::discovery::agent_memory::build_agent_memory_injection`].
    pub memory: Option<AgentMemoryConfig>,
    /// The agent's `toolBudget:` (pi `AgentConfig.toolBudget`, `agents.ts` + `tool-budget.ts`),
    /// already validated and normalized. `None` means the agent declared none. When set, spawn
    /// time encodes it into the child's `CYRUP_SUBAGENT_TOOL_BUDGET` env var and the child-side
    /// runtime enforces it (soft nudge + hard block).
    pub tool_budget: Option<ResolvedToolBudget>,
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

/// pi's `string | false` binding for a chain step's `output` field (`ChainStepConfig.output`,
/// `agents.ts:143`) — a named output path/value, or the literal `false` that disables this step's
/// output. Modeled explicitly (rather than collapsing to `Option<String>`) so a step's
/// `output: false` survives round-trip *distinct* from an absent `output`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ChainOutputBinding {
    /// A boolean toggle — pi's serializer only ever emits `false` here; preserved verbatim.
    Toggle(bool),
    /// A named output path/identifier.
    Name(String),
}

/// pi's `string[] | false` binding for a chain step's `reads`/`skills` fields
/// (`ChainStepConfig.reads`/`skills`, `agents.ts:145/147`) — an explicit list, or the literal
/// `false` that disables the default. Modeled explicitly so `reads: false`/`skills: false` survive
/// round-trip distinct from an absent field or an empty list.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum ChainListBinding {
    /// A boolean toggle — pi's serializer only ever emits `false` here; preserved verbatim.
    Toggle(bool),
    /// An explicit string list.
    List(Vec<String>),
}

/// One parsed chain STEP — a direct port of pi's `ChainStepConfig` (`agents.ts:136-156`). This is
/// the on-disk **authoring** shape a `.chain.md` `## <agent>` section or a `.chain.json` `chain[]`
/// element deserializes to (func-SA §4.1/§4.2): deliberately permissive and data-only, distinct
/// from the runtime dispatch form [`crate::spawn::chain_graph::RunnerStep`] (the `SingleStep |
/// ParallelGroup | DynamicGroup` union the orchestrator/executor maps this into at plan time, via
/// [`crate::discovery::chains::chain_step_to_runner_step`]).
///
/// The sequential fields (`agent`/`task`/`phase`/`label`/`as_`/`output_schema`/`output`/
/// `output_mode`/`reads`/`model`/`skills`/`progress`) are exactly the config lines pi's `.chain.md`
/// grammar (`chain-serializer.ts:9-85`) recognizes; `parallel`/`expand`/`collect`/`concurrency`/
/// `fail_fast`/`worktree`/`acceptance` are the additional `.chain.json`-only shapes for
/// static-parallel and dynamic-fanout steps, held as raw JSON so their deep validation stays in
/// [`crate::discovery::chains`] (`validate_chain_output_bindings`/`validate_acceptance_input`,
/// `chain-serializer.ts:128-199`) exactly as pi validates the raw objects.
#[derive(Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChainStepConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// pi's `as` — the named-output key later steps reference via `{outputs.<name>}`. Renamed from
    /// the Rust-reserved-word-adjacent `as_` for the on-disk `as` key.
    #[serde(rename = "as", skip_serializing_if = "Option::is_none")]
    pub as_: Option<String>,
    /// pi's `outputSchema` — a schema-file *path* string in `.chain.md` (inline objects are
    /// rejected at parse time), or an inline JSON Schema object in `.chain.json`. Held raw.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<ChainOutputBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reads: Option<ChainListBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<ChainListBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<bool>,
    /// `.chain.json`-only: a static-parallel step's task array, or a dynamic step's single
    /// template object. Held raw (its shape decides `RunnerStep` variant at plan time).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parallel: Option<serde_json::Value>,
    /// `.chain.json`-only dynamic-fanout `{ from: { output, path }, item, key, maxItems, onEmpty }`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expand: Option<serde_json::Value>,
    /// `.chain.json`-only dynamic-fanout `{ as, outputSchema }` collect spec.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collect: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub concurrency: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fail_fast: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
    /// pi's `AcceptanceInput` (`string | false | object`) held raw; validated by
    /// `validate_acceptance_input` at parse time, resolved to a runtime contract by a later phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance: Option<serde_json::Value>,
    /// Unknown/forward-compat keys preserved verbatim (pi keeps the raw step object, so
    /// runner-only fields like `cwd`/`skill`/`count` survive round-trip rather than being dropped).
    #[serde(flatten)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

/// One parsed chain (`.chain.md`/`.chain.json`) definition — a port of pi's `ChainConfig`
/// (`agents.ts:158-167`). `name` is the fully-qualified RUNTIME name (`{package}.{local_name}` when
/// packaged, else the bare `local_name`, via `buildRuntimeName`, `identity.ts:19-22`); `local_name`
/// and `package_name` retain the pre-qualification identity so management/serialization can
/// reconstruct the frontmatter/JSON `name`+`package` split (`frontmatterNameForConfig`). `steps`
/// holds the parsed [`ChainStepConfig`] authoring shapes; converting them to the runtime
/// [`crate::spawn::chain_graph::RunnerStep`] union is the orchestrator's plan-time job (see
/// [`crate::discovery::chains::chain_step_to_runner_step`]), not this type's.
#[derive(Clone, Debug)]
pub struct ChainDefinition {
    /// Fully-qualified runtime name (`buildRuntimeName(local_name, package_name)`).
    pub name: String,
    /// The chain's local (pre-packaging) name — the frontmatter/JSON `name` value verbatim.
    pub local_name: String,
    /// The sanitized package identifier, when the chain declared a valid `package` field.
    pub package_name: Option<String>,
    pub description: String,
    pub source: AgentSource,
    pub file_path: PathBuf,
    pub steps: Vec<ChainStepConfig>,
    /// Unknown frontmatter/JSON top-level keys (everything except `name`/`package`/`description`/
    /// `chain`) preserved verbatim for round-trip (pi's `ChainConfig.extraFields`).
    pub extra_fields: BTreeMap<String, String>,
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
            thinking: OverrideField::Value("high".to_string()),
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
            default_async: None,
            default_timeout_ms: None,
            memory: None,
            tool_budget: None,
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

    // -----------------------------------------------------------------------------------------
    // C2 serde shape: pi-authored `settings.json` values must deserialize into these types.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn tool_ref_deserializes_from_a_bare_string_splitting_mcp_prefix() {
        // pi settings `tools`/override lists are plain strings: `"bash"` / `"mcp:x"`.
        let builtin: ToolRef = serde_json::from_str("\"bash\"").expect("string builtin");
        assert_eq!(builtin, ToolRef::Builtin("bash".to_string()));
        let mcp: ToolRef = serde_json::from_str("\"mcp:xcodebuild_list_sims\"").expect("string mcp");
        assert_eq!(mcp, ToolRef::Mcp("xcodebuild_list_sims".to_string()));
    }

    #[test]
    fn tool_ref_round_trips_through_its_adjacently_tagged_map_form() {
        // The `RunnerConfig`/`ResolvedAgentPersona` boundary serializes as `{kind,content}` and must
        // deserialize back losslessly (Mcp stays Mcp, not collapsed to Builtin).
        for original in [
            ToolRef::Builtin("read".to_string()),
            ToolRef::Mcp("github.search".to_string()),
            ToolRef::ExtensionPath("./tools/x.ts".to_string()),
        ] {
            let json = serde_json::to_string(&original).expect("serialize");
            let back: ToolRef = serde_json::from_str(&json).expect("deserialize map form");
            assert_eq!(back, original, "map form must round-trip: {json}");
        }
    }

    #[test]
    fn override_field_string_reads_value_and_false_as_explicit_clear() {
        // `"model": "x"` -> Value; `"model": false` -> ExplicitClear (pi's `string | false`).
        let value: OverrideField<String> = serde_json::from_str("\"openai/gpt-5\"").expect("value");
        assert_eq!(value, OverrideField::Value("openai/gpt-5".to_string()));
        let clear: OverrideField<String> = serde_json::from_str("false").expect("clear");
        assert_eq!(clear, OverrideField::ExplicitClear);
    }

    #[test]
    fn override_field_bool_reads_false_as_value_not_clear() {
        // `disabled`/`completionGuard` are plain booleans: `false` is a real VALUE, never a clear.
        let f: OverrideField<bool> = serde_json::from_str("false").expect("bool false");
        assert_eq!(f, OverrideField::Value(false));
        let t: OverrideField<bool> = serde_json::from_str("true").expect("bool true");
        assert_eq!(t, OverrideField::Value(true));
    }

    #[test]
    fn override_field_string_rejects_a_non_false_boolean() {
        // pi throws on `model: true`; our three-state deserialize must error too.
        let r: Result<OverrideField<String>, _> = serde_json::from_str("true");
        assert!(r.is_err(), "model: true must be rejected");
    }

    #[test]
    fn subagent_settings_deserializes_a_pi_shaped_block() {
        let raw = serde_json::json!({
            "agentOverrides": {
                "reviewer": { "model": "openai/gpt-5.4", "thinking": "xhigh", "completionGuard": false },
                "implementer": { "tools": ["bash", "mcp:xcodebuild_list_sims"] }
            },
            "defaultModel": "deepseek-v4-flash",
            "disableBuiltins": true
        });
        let settings: SubagentSettings = serde_json::from_value(raw).expect("pi-shaped settings");
        assert_eq!(settings.default_model.as_deref(), Some("deepseek-v4-flash"));
        assert_eq!(settings.disable_builtins, Some(true));

        let reviewer = settings.overrides.get("reviewer").expect("reviewer override");
        assert_eq!(reviewer.model, OverrideField::Value("openai/gpt-5.4".to_string()));
        assert_eq!(reviewer.thinking, OverrideField::Value("xhigh".to_string()));
        assert_eq!(reviewer.completion_guard, OverrideField::Value(false));

        let implementer = settings.overrides.get("implementer").expect("implementer override");
        assert_eq!(
            implementer.tools,
            OverrideField::Value(vec![
                ToolRef::Builtin("bash".to_string()),
                ToolRef::Mcp("xcodebuild_list_sims".to_string()),
            ])
        );
    }

    #[test]
    fn agent_override_config_deserializes_all_pi_overridable_fields() {
        // pi `BuiltinAgentOverrideConfig` (agents.ts:65-79): the six fields an earlier port dropped
        // (`inheritProjectContext`/`inheritSkills`/`defaultContext`/`systemPrompt`/`skills`/
        // `subagentOnlyExtensions`) must now deserialize, with the `| false` fields reading a JSON
        // `false` as an explicit clear and the plain-bool fields reading `false` as a real value.
        let full: AgentOverrideConfig = serde_json::from_value(serde_json::json!({
            "inheritProjectContext": true,
            "inheritSkills": false,
            "defaultContext": "fork",
            "systemPrompt": "Base prompt",
            "skills": ["tdd", "safe-bash"],
            "subagentOnlyExtensions": ["./tools/child-review.ts"]
        }))
        .expect("six-field override deserializes");
        assert_eq!(full.inherit_project_context, OverrideField::Value(true));
        assert_eq!(full.inherit_skills, OverrideField::Value(false));
        assert_eq!(full.default_context, OverrideField::Value(ContextMode::Fork));
        assert_eq!(full.system_prompt, OverrideField::Value("Base prompt".to_string()));
        assert_eq!(
            full.skills,
            OverrideField::Value(vec!["tdd".to_string(), "safe-bash".to_string()])
        );
        assert_eq!(
            full.subagent_only_extensions,
            OverrideField::Value(vec!["./tools/child-review.ts".to_string()])
        );

        // The `AgentDefaultContext | false` / `string[] | false` fields clear explicitly on `false`.
        let cleared: AgentOverrideConfig = serde_json::from_value(serde_json::json!({
            "defaultContext": false,
            "skills": false,
            "subagentOnlyExtensions": false
        }))
        .expect("cleared override deserializes");
        assert_eq!(cleared.default_context, OverrideField::ExplicitClear);
        assert_eq!(cleared.skills, OverrideField::ExplicitClear);
        assert_eq!(cleared.subagent_only_extensions, OverrideField::ExplicitClear);
    }

    #[test]
    fn override_thinking_is_an_open_string_off_and_arbitrary_values_survive() {
        // pi's override `thinking` is `string | false`; an explicit `"off"` and any arbitrary
        // (future/provider-specific) level must deserialize as a concrete Value, NOT error the whole
        // settings load the way a closed on-only enum would, and `false` stays an explicit clear.
        let raw = serde_json::json!({
            "agentOverrides": {
                "a": { "thinking": "off" },
                "b": { "thinking": "super-duper" },
                "c": { "thinking": false }
            }
        });
        let settings: SubagentSettings = serde_json::from_value(raw).expect("open thinking settings");
        assert_eq!(
            settings.overrides.get("a").expect("a").thinking,
            OverrideField::Value("off".to_string()),
            "explicit off must survive as a Value, distinct from unset"
        );
        assert_eq!(
            settings.overrides.get("b").expect("b").thinking,
            OverrideField::Value("super-duper".to_string()),
            "an arbitrary pi thinking value must not be dropped"
        );
        assert_eq!(
            settings.overrides.get("c").expect("c").thinking,
            OverrideField::ExplicitClear,
            "`thinking: false` remains an explicit clear"
        );
    }

    #[test]
    fn agent_override_config_round_trips_only_the_fields_it_sets() {
        // An override with only `model` set serializes WITHOUT the untouched (`Unset`) fields, and
        // reads back identically — the profile round-trip contract.
        let cfg = AgentOverrideConfig {
            model: OverrideField::Value("claude-opus".to_string()),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize override");
        assert_eq!(json, "{\"model\":\"claude-opus\"}");
        let back: AgentOverrideConfig = serde_json::from_str(&json).expect("deserialize override");
        assert_eq!(back.model, OverrideField::Value("claude-opus".to_string()));
        assert!(back.thinking.is_unset());
        assert!(back.tools.is_unset());
    }
}
