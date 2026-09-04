//! Four-tier Builtin/Package/User/Project precedence merge and settings-override application
//! (func-SA §5.1 R-SA-001/002/004/009/010/011/012/020/021; arch-SA §6.2/§6.2.1).
//!
//! **Correction #2 (binding, restated from `discovery/types.rs`'s module doc):** this module
//! implements the merge as a **plain, bespoke algorithm** — ordinary `HashMap`/`Vec` reduction
//! keyed on [`AgentDefinition::name`] — and deliberately does **not** build on
//! `cyrup_resources::discovery::ResourceSet<T>`. That primitive's `build` function
//! (`crates/cyrup-resources/src/discovery.rs`) performs a single stable-sort-by-
//! `ResourceScope::precedence_rank`-then-first-insertion-at-a-given-rank-wins dedup — a
//! *symmetric* rule fine for Pi's 9-variant skill/prompt/theme `ResourceScope`, but structurally
//! unable to express R-SA-002's deliberately **asymmetric** rule:
//!
//! - **Package tier**: the *first* package root that defines a given agent name wins
//!   (first-seen-wins), enforced by refusing to overwrite an already-present name.
//! - **User tier** and **Project tier** (each independently): the *last* directory/file scanned
//!   (in fixed, alphabetical-by-filename scan order, R-SA-004) wins (last-seen-wins), enforced by
//!   unconditionally overwriting any prior entry for the same name.
//!
//! These two reduction rules are opposite operations (refuse-to-overwrite vs.
//! always-overwrite) applied to structurally identical `(name, AgentDefinition)` streams — they
//! are NOT unified into one rule anywhere in this file, per R-SA-002's explicit "MUST NOT be
//! unified" clause. See [`reduce_first_seen_wins`] and [`reduce_last_seen_wins`] below, which stay
//! textually separate on purpose (a shared "one function with a `wins: bool` flag" refactor would
//! blur exactly the asymmetry this module exists to preserve legibly).
//!
//! Discovery **plumbing** (directory walks, package-manifest `agents` field resolution) is owned
//! by `discovery/mod.rs`, which calls into this module with already-parsed
//! `Vec<AgentDefinition>` per tier/scan-scope (each individually already in R-SA-004 alphabetical
//! scan order) — this module performs no filesystem I/O of its own.

use std::collections::HashMap;
use std::path::PathBuf;

use super::types::{
    AgentDefinition, AgentModelSourceInfo, AgentOverrideConfig, AgentOverrideInfo, AgentSource,
    LayeredOverrideSettings, OutputSpec, OverrideField, OverrideScope, ToolsOverrideField,
};
use crate::error::SubagentError;

// -------------------------------------------------------------------------------------------
// Tier-internal dedup (R-SA-002)
// -------------------------------------------------------------------------------------------

/// Package-tier dedup (R-SA-002): the **first** package root that defines a given agent name
/// wins. `candidates` MUST already be in fixed package-scan order (the order package roots are
/// enumerated in, per `discovery/mod.rs`'s manifest resolution) — this function does not itself
/// determine or validate that order, only respects it via ordinary iteration.
///
/// A later candidate sharing an already-seen name is silently dropped (not an error, not a
/// diagnostic) — this mirrors R-SA-002's plain first-wins framing and R-SA-009's reservation of
/// diagnostics for chain files / errors for malformed settings, neither of which this
/// intra-package-tier collision is.
fn reduce_first_seen_wins(candidates: Vec<AgentDefinition>) -> HashMap<String, AgentDefinition> {
    let mut by_name: HashMap<String, AgentDefinition> = HashMap::with_capacity(candidates.len());
    for candidate in candidates {
        by_name.entry(candidate.name.clone()).or_insert(candidate);
    }
    by_name
}

/// User-tier / Project-tier dedup (R-SA-002, applied **independently** to each of those two
/// tiers): the **last** directory/file scanned wins. `candidates` MUST already be in fixed scan
/// order (alphabetical-by-filename directory walk, R-SA-004) — this function does not itself walk
/// directories, only respects the order it is handed.
///
/// A later candidate sharing an already-seen name unconditionally overwrites the earlier one —
/// the opposite operation from [`reduce_first_seen_wins`], kept as a textually distinct function
/// (not a `wins_first: bool` parameter on one shared helper) so R-SA-002's "MUST NOT be unified
/// into one consistent rule" is visible in the code shape, not just in a comment.
fn reduce_last_seen_wins(candidates: Vec<AgentDefinition>) -> HashMap<String, AgentDefinition> {
    let mut by_name: HashMap<String, AgentDefinition> = HashMap::with_capacity(candidates.len());
    for candidate in candidates {
        by_name.insert(candidate.name.clone(), candidate);
    }
    by_name
}

// -------------------------------------------------------------------------------------------
// Four-tier cross-scope merge (R-SA-001)
// -------------------------------------------------------------------------------------------

/// The four already-tier-deduped agent sets handed to [`merge_tiers`], one per [`AgentSource`]
/// variant. Each `Vec` here is expected to already be free of same-tier name collisions (the
/// caller applies [`reduce_first_seen_wins`]/[`reduce_last_seen_wins`] per tier, per R-SA-002,
/// before constructing this struct) — [`merge_tiers`] itself only resolves **cross-tier**
/// collisions per R-SA-001's fixed precedence order.
#[derive(Debug, Default)]
pub struct TieredAgents {
    pub builtin: Vec<AgentDefinition>,
    pub package: Vec<AgentDefinition>,
    pub user: Vec<AgentDefinition>,
    pub project: Vec<AgentDefinition>,
}

/// Merge four already-tier-deduped agent sets into one final `name -> AgentDefinition` map,
/// resolving cross-tier collisions per R-SA-001's fixed precedence: **Project beats User beats
/// Package beats Builtin**. Implemented as a plain `HashMap` reduction (not `ResourceSet<T>`, see
/// module doc): entries are inserted lowest-precedence-first (`Builtin` first, `Project` last) so
/// each subsequent insertion for the same name unconditionally overwrites a lower-precedence
/// entry, mirroring [`AgentSource::precedence_rank`]'s "lower rank wins" contract without
/// depending on that method directly (the insertion *order* below, not a runtime rank comparison,
/// is what encodes the precedence — kept in lockstep with `precedence_rank` by the
/// `merge_tiers_matches_precedence_rank_ordering` test below).
pub fn merge_tiers(tiers: TieredAgents) -> HashMap<String, AgentDefinition> {
    let package_by_name = reduce_first_seen_wins(tiers.package);
    let builtin_by_name = reduce_first_seen_wins(tiers.builtin);
    let user_by_name = reduce_last_seen_wins(tiers.user);
    let project_by_name = reduce_last_seen_wins(tiers.project);

    let mut merged: HashMap<String, AgentDefinition> = HashMap::with_capacity(
        builtin_by_name.len() + package_by_name.len() + user_by_name.len() + project_by_name.len(),
    );
    // Lowest precedence first; each later extend() call overwrites same-name entries from an
    // earlier, lower-precedence tier — R-SA-001: Project > User > Package > Builtin.
    merged.extend(builtin_by_name);
    merged.extend(package_by_name);
    merged.extend(user_by_name);
    merged.extend(project_by_name);
    merged
}

/// Convenience wrapper matching arch-SA §6.2's top-level `discover_agents` shape: takes each
/// tier's already-scanned (but not yet tier-deduped) candidate list, applies R-SA-002's per-tier
/// dedup, merges cross-tier per R-SA-001, then applies settings-based overrides (R-SA-010/011/012,
/// §6.2.1). Returns the final `name -> AgentDefinition` map. Discovery-scope filtering (R-SA-013's
/// disabled-agent visibility policy, `AgentReadScope` narrowing) is owned by `discovery/mod.rs`,
/// applied to this function's output — not duplicated here.
pub fn discover_and_merge(
    tiers: TieredAgents,
    settings: &LayeredOverrideSettings,
) -> Result<HashMap<String, AgentDefinition>, SubagentError> {
    let mut merged = merge_tiers(tiers);
    apply_overrides(&mut merged, settings)?;
    Ok(merged)
}

// -------------------------------------------------------------------------------------------
// Override application (R-SA-010/011/012, §6.2.1)
// -------------------------------------------------------------------------------------------

/// Apply the `subagents.*` settings layer to an already-merged agent map, in place (arch-SA
/// §6.2.1; a direct port of pi's `applySubagentDefaultModel` + `applyBuiltinOverrides` +
/// `applyCustomAgentOverrides`, `agents.ts:935-1213`, driven from `discoverAgents`,
/// `agents.ts:1731-1780`).
///
/// **Tier 7 — real two-scope threading.** [`LayeredOverrideSettings`] carries the user- and
/// project-scope settings UNFLATTENED (each with its own `settings.json` path), so this function
/// resolves project-beats-user precedence at APPLICATION time and records the true winning scope +
/// settings-file path in [`AgentOverrideInfo`] — not the pre-Tier-7 shape that flattened both
/// scopes into one map (losing which scope an override came from) and always stamped `Project` /
/// the agent's own `.md` path.
///
/// pi runs its passes per TIER before merging; because merge never alters the fields these passes
/// read, running them here over the already-merged winner (dispatched by `agent.source`) is
/// observably identical — and the surviving winner is the only agent whose provenance is visible:
///
/// - `applySubagentDefaultModel` (`agents.ts:935-944`): fill every model-less agent from the
///   resolved (project-over-user) `defaultModel`.
/// - `applyBuiltinOverrides` (`agents.ts:1039-1092`): for each [`AgentSource::Builtin`] agent, in
///   pi's exact branch order — project override ▷ project bulk-disable ▷ user override ▷ user
///   bulk-disable ▷ none — then the `disableThinking` clear (BUILTINS ONLY, skipped when the
///   winning-scope override already set `thinking`).
/// - `applyCustomAgentOverrides` (`agents.ts:1193-1213`): for each [`AgentSource::User`]/
///   [`AgentSource::Project`] agent, project override ▷ user override (fill-unset-only; the applied
///   `scope` is the SETTINGS scope, never the agent's own source). `systemPrompt` and
///   `disableBuiltins`/`disableThinking` are BUILTIN-only — custom agents never see them.
pub fn apply_overrides(
    merged: &mut HashMap<String, AgentDefinition>,
    settings: &LayeredOverrideSettings,
) -> Result<(), SubagentError> {
    // applySubagentDefaultModel: resolve the winning defaultModel (project scope wins when the
    // project scope exists and declares one), then fill every model-less agent BEFORE per-agent
    // overrides run so an explicit `agentOverrides.<name>.model` still wins by overwriting it.
    apply_default_model(merged, resolve_default_model(settings).as_deref());
    // applySubagentDefaults (agents.ts:986-996) runs model -> thinking -> extensions, all three
    // BEFORE per-agent overrides, and all three fill-only-if-unset.
    apply_default_thinking(merged, resolve_default_thinking(settings).as_deref());
    apply_default_extensions(merged, resolve_default_extensions(settings).as_deref());

    // applyBuiltinOverrides header (agents.ts:792-798): resolve the bulk-disable / disableThinking
    // scope selection ONCE, up front, exactly as pi does before mapping over the builtin list.
    let project_scoped = settings.project_settings_path.is_some();
    let project_bulk_disabled = project_scoped && settings.project.disable_builtins == Some(true);
    // pi: userBulkDisabled only when the project scope said NOTHING about disableBuiltins — a
    // project `disableBuiltins: false` re-enables what a user `true` disabled.
    let user_bulk_disabled =
        settings.project.disable_builtins.is_none() && settings.user.disable_builtins == Some(true);
    let project_thinking_configured = project_scoped && settings.project.disable_thinking.is_some();
    let disable_thinking = if project_thinking_configured {
        settings.project.disable_thinking == Some(true)
    } else {
        settings.user.disable_thinking == Some(true)
    };
    let disable_thinking_meta = match (project_thinking_configured, &settings.project_settings_path)
    {
        (true, Some(path)) => (OverrideScope::Project, path.clone()),
        _ => (OverrideScope::User, settings.user_settings_path.clone()),
    };

    for agent in merged.values_mut() {
        match agent.source {
            AgentSource::Builtin => apply_builtin_agent(
                agent,
                settings,
                project_bulk_disabled,
                user_bulk_disabled,
                project_thinking_configured,
                disable_thinking,
                &disable_thinking_meta,
            ),
            AgentSource::User | AgentSource::Project => apply_custom_agent(agent, settings),
            // Package-sourced agents are not exposed for settings-based override in pi's own source
            // contract (only Builtin full-replace and User/Project fill-unset-only) — left untouched.
            // SUBA-084: a Runtime agent never enters this map at all (`mergeRuntimeAgents` appends
            // AFTER discovery's override application, `runtime-agent-registry.ts:428` @v0.64.0), so
            // the arm exists only for totality and applies nothing.
            AgentSource::Package | AgentSource::Runtime => {}
        }
    }

    Ok(())
}

/// pi `resolveSubagentDefaultModel` (`agents.ts:921-933`): the project-scope `defaultModel` wins
/// when the project scope exists and declares one, else the user-scope value (or `None`).
fn resolve_default_model(settings: &LayeredOverrideSettings) -> Option<String> {
    if settings.project_settings_path.is_some()
        && let Some(dm) = settings.project.default_model.as_ref()
    {
        return Some(dm.clone());
    }
    settings.user.default_model.clone()
}

/// pi `applySubagentDefaultModel` (`agents.ts:935-944`): fill every agent that has no resolved
/// `model` from the (already project-over-user resolved) `defaultModel`, stamping
/// [`AgentModelSourceInfo::SettingsDefault`] provenance so management/`/subagents-doctor` surfaces
/// can report *why* the model resolved that way. Runs before per-agent overrides so an explicit
/// `agentOverrides.<name>.model` still wins by overwriting the filled default. A `None` default is
/// a no-op, leaving a frontmatter-model agent's `model_source` untouched.
fn apply_default_model(merged: &mut HashMap<String, AgentDefinition>, default_model: Option<&str>) {
    let Some(dm) = default_model else {
        return;
    };
    for agent in merged.values_mut() {
        if agent.model.is_none() {
            agent.model = Some(cyrup_core::ModelId::from(dm.to_string()));
            agent.model_source = Some(AgentModelSourceInfo::SettingsDefault);
        }
    }
}

/// pi `resolveSubagentDefaultThinking` (`agents.ts:946-953`): the project-scope value wins when the
/// project scope exists and declares one, else the user-scope value.
fn resolve_default_thinking(settings: &LayeredOverrideSettings) -> Option<String> {
    if settings.project_settings_path.is_some()
        && let Some(dt) = settings.project.default_thinking.as_ref()
    {
        return Some(dt.clone());
    }
    settings.user.default_thinking.clone()
}

/// pi `applySubagentDefaultThinking` (`agents.ts:955-964`): fill every agent whose `thinking` is
/// UNSET from the resolved `defaultThinking`. `Some("off")` is an EXPLICIT off (see
/// [`AgentDefinition::thinking`]) and therefore counts as set — pi's guard is
/// `agent.thinking !== undefined`, and its `thinking: false` (cyrup's `Some("off")`) is likewise not
/// `undefined`, so an agent that explicitly opted out is never re-armed by the default.
///
/// Runs BEFORE per-agent overrides so an explicit `agentOverrides.<name>.thinking` still wins, and
/// before the builtin `disableThinking` clear so `disableThinking: true` still strips it.
fn apply_default_thinking(
    merged: &mut HashMap<String, AgentDefinition>,
    default_thinking: Option<&str>,
) {
    let Some(dt) = default_thinking else {
        return;
    };
    for agent in merged.values_mut() {
        if agent.thinking.is_none() {
            agent.thinking = Some(dt.to_string());
        }
    }
}

/// pi `resolveSubagentDefaultExtensions` (`agents.ts:966-973`): project-wins-outright, same shape as
/// [`resolve_default_thinking`].
fn resolve_default_extensions(settings: &LayeredOverrideSettings) -> Option<Vec<String>> {
    if settings.project_settings_path.is_some()
        && let Some(de) = settings.project.default_extensions.as_ref()
    {
        return Some(de.clone());
    }
    settings.user.default_extensions.clone()
}

/// pi `applySubagentDefaultExtensions` (`agents.ts:975-984`): fill every agent whose `extensions` is
/// UNSET (`None` — "all extensions visible") from the resolved `defaultExtensions`, and stamp
/// [`AgentDefinition::extensions_from_default`] so the value is never mistaken for the agent's own
/// declaration by `editable_base`/`clone_override_base`.
///
/// An agent that declared `extensions:` explicitly — INCLUDING an explicitly-empty list, which is
/// `Some(vec![])` and means "no extensions" — keeps its own value, matching pi's
/// `agent.extensions !== undefined` guard.
fn apply_default_extensions(
    merged: &mut HashMap<String, AgentDefinition>,
    default_extensions: Option<&[String]>,
) {
    let Some(de) = default_extensions else {
        return;
    };
    for agent in merged.values_mut() {
        if agent.extensions.is_none() {
            agent.extensions = Some(de.to_vec());
            agent.extensions_from_default = true;
        }
    }
}

/// pi `applyBuiltinOverrides`' per-agent body (`agents.ts:1059-1090`) for ONE builtin agent: pick the
/// single winning override / bulk-disable in pi's exact branch order (project override ▷ project
/// bulk-disable ▷ user override ▷ user bulk-disable ▷ none), apply it with the true settings scope +
/// path, then run the `disableThinking` clear unless the winning-scope override explicitly set
/// `thinking`. Every project-scope branch is additionally gated on the project scope actually
/// existing (`project_settings_path.is_some()`), matching pi's `projectSettingsPath !== null` guard.
fn apply_builtin_agent(
    agent: &mut AgentDefinition,
    settings: &LayeredOverrideSettings,
    project_bulk_disabled: bool,
    user_bulk_disabled: bool,
    project_thinking_configured: bool,
    disable_thinking: bool,
    disable_thinking_meta: &(OverrideScope, PathBuf),
) {
    let project_override = settings.project.overrides.get(&agent.name);
    let user_override = settings.user.overrides.get(&agent.name);
    let mut explicit_thinking_override = false;

    if let (Some(delta), Some(path)) = (project_override, settings.project_settings_path.as_ref()) {
        apply_builtin_override(agent, delta, OverrideScope::Project, path.clone());
        explicit_thinking_override = delta.thinking.is_present();
    } else if project_bulk_disabled {
        if let Some(path) = settings.project_settings_path.as_ref() {
            apply_builtin_override(agent, &disable_delta(), OverrideScope::Project, path.clone());
        }
    } else if let Some(delta) = user_override {
        apply_builtin_override(
            agent,
            delta,
            OverrideScope::User,
            settings.user_settings_path.clone(),
        );
        // pi (agents.ts:825): a user override's `thinking` counts as an explicit opt-in ONLY when
        // the project scope did not configure `disableThinking` (a project `disableThinking`
        // overrides a user per-agent `thinking` — agent-overrides.test.ts:193-213).
        explicit_thinking_override = !project_thinking_configured && delta.thinking.is_present();
    } else if user_bulk_disabled {
        apply_builtin_override(
            agent,
            &disable_delta(),
            OverrideScope::User,
            settings.user_settings_path.clone(),
        );
    }

    // applyGlobalThinking / clearBuiltinThinking (agents.ts:776-803).
    if disable_thinking && !explicit_thinking_override {
        clear_builtin_thinking(agent, disable_thinking_meta.0, disable_thinking_meta.1.clone());
    }
}

/// The `{ disabled: true }` override delta pi's bulk-disable arms pass to `applyBuiltinOverride`
/// (`agents.ts:1070/831`).
fn disable_delta() -> AgentOverrideConfig {
    AgentOverrideConfig {
        disabled: OverrideField::Value(true),
        ..AgentOverrideConfig::default()
    }
}

/// pi `applyBuiltinOverride` (`agents.ts:998-1031`): full-replace every field the delta states
/// (`Value` sets, `ExplicitClear` resets to the field's absent value, `Unset` is left alone), and
/// record [`AgentOverrideInfo`] provenance with `base` = the agent snapshot BEFORE this override.
/// pi's callers only ever pass a non-empty delta — a parsed override entry with no fields is dropped
/// at read time (`parseBuiltinOverrideEntry` returns `undefined`), and the bulk-disable arms pass an
/// explicit `{ disabled: true }`. Because this crate's settings deserialize CAN yield an all-`Unset`
/// entry (serde does not drop it), the `is_empty` short-circuit here reproduces pi's "empty entries
/// are never applied" — an empty delta records no provenance. `systemPrompt` (the builtin body
/// replacement) is applied here and ONLY here (pi's custom-agent branch omits it).
fn apply_builtin_override(
    agent: &mut AgentDefinition,
    delta: &AgentOverrideConfig,
    scope: OverrideScope,
    settings_path: PathBuf,
) {
    if delta.is_empty() {
        return;
    }
    let base_snapshot = Box::new(agent.clone());

    // pi `description` (agents.ts:1258): a plain replace. Only `Value` applies — deliberately NOT
    // `apply_field_full_replace`, whose `ExplicitClear` arm would blank the description. pi declares
    // no `| false` clear form for this key, but `OverrideField<String>`'s untagged `Deserialize`
    // still yields `ExplicitClear` for a JSON `false` (the `Value(String)` arm rejects a bool), so
    // the state is reachable from a settings file and has to be decided rather than fallen into.
    // Ignoring is the safe answer: `description` is the text the parent model reads when choosing
    // an agent, so a nonsensical value must not erase the agent's selectability. The custom path
    // below already ignores it; the two must not disagree on the same settings key.
    if let OverrideField::Value(v) = &delta.description {
        agent.description = v.clone();
    }
    apply_output_override(agent, &delta.output);
    // pi `defaultReads`/`false` -> `delete next.defaultReads` (agents.ts:1261), i.e. `None` ("the
    // agent declared no default reads"), NOT an empty list.
    apply_field_full_replace(&mut agent.default_reads, &delta.default_reads, None, |v| {
        Some(v.iter().map(PathBuf::from).collect())
    });
    apply_field_full_replace(&mut agent.model, &delta.model, None, |v| {
        Some(cyrup_core::ModelId::from(v.clone()))
    });
    // Record model provenance when the override touched `model`: a concrete value is a
    // settings-override source; an explicit clear leaves the agent with no model (source cleared).
    match &delta.model {
        OverrideField::Value(_) => {
            agent.model_source = Some(AgentModelSourceInfo::SettingsOverride);
        }
        OverrideField::ExplicitClear => agent.model_source = None,
        OverrideField::Unset => {}
    }
    apply_field_full_replace(
        &mut agent.fallback_models,
        &delta.fallback_models,
        Vec::new(),
        |v| v.iter().cloned().map(cyrup_core::ModelId::from).collect(),
    );
    apply_field_full_replace(&mut agent.thinking, &delta.thinking, None, |v| Some(v.clone()));
    apply_field_full_replace(
        &mut agent.system_prompt_mode,
        &delta.system_prompt_mode,
        crate::discovery::types::SystemPromptMode::Replace,
        |v| *v,
    );
    apply_field_full_replace(
        &mut agent.inherit_project_context,
        &delta.inherit_project_context,
        false,
        |v| *v,
    );
    apply_field_full_replace(&mut agent.inherit_skills, &delta.inherit_skills, false, |v| *v);
    apply_field_full_replace(&mut agent.default_context, &delta.default_context, None, |v| {
        Some(*v)
    });
    apply_field_full_replace(&mut agent.disabled, &delta.disabled, None, |v| Some(*v));
    // pi `systemPrompt` (agents.ts:1018): replace the BUILTIN persona's own body prose.
    apply_field_full_replace(
        &mut agent.system_prompt_body,
        &delta.system_prompt,
        String::new(),
        |v| v.clone(),
    );
    apply_field_full_replace(&mut agent.skills, &delta.skills, Vec::new(), |v| v.clone());
    apply_tools_override(&mut agent.tools, &delta.tools);
    // SUBA-092 / pi `agents.ts:1404` @v0.64.0: `excludeTools`/`false` -> `delete next.excludeTools`
    // (`None`), else a copy of the override's list. `:1405`: `allowNestedSubagents` is a plain
    // boolean assignment (no clear form reachable — see the field doc).
    apply_field_full_replace(&mut agent.exclude_tools, &delta.exclude_tools, None, |v| {
        Some(v.clone())
    });
    apply_field_full_replace(
        &mut agent.allow_nested_subagents,
        &delta.allow_nested_subagents,
        None,
        |v| Some(*v),
    );
    // pi `extensions`/`false` -> `delete next.extensions` (agents.ts:1282). `None` here means "all
    // extensions visible", which is the opposite of `Some(vec![])` ("none") — so, unlike `tools`,
    // the clear value IS `None`.
    //
    // Deliberately does NOT touch [`AgentDefinition::extensions_from_default`], in either
    // direction. Not setting it is the point: an explicit override is not a default. Not CLEARING a
    // `true` left by `apply_default_extensions` is also deliberate, and needs saying because the
    // ordering makes it reachable — `apply_default_extensions` runs BEFORE per-agent overrides
    // (`apply_overrides`, above) and stamps the flag on every agent it fills, so an override that
    // then replaces the list leaves a merged definition claiming default-provenance for an
    // override-supplied value. That staleness is upstream's too (`agents.ts:1282` likewise never
    // clears `extensionsFromDefault`) and is unreachable in practice: the sole consumer,
    // `management::handlers::editable_base`, reads the pre-override `base_snapshot` whenever an
    // override applied, never this post-override field.
    apply_field_full_replace(&mut agent.extensions, &delta.extensions, None, |v| {
        Some(v.clone())
    });
    apply_field_full_replace(
        &mut agent.subagent_only_extensions,
        &delta.subagent_only_extensions,
        Vec::new(),
        |v| v.clone(),
    );
    apply_field_full_replace(
        &mut agent.completion_guard,
        &delta.completion_guard,
        None,
        |v| Some(*v),
    );
    // pi `toolBudget`/`false` -> `delete next.toolBudget` (agents.ts:1285).
    apply_field_full_replace(&mut agent.tool_budget, &delta.tool_budget, None, |v| {
        Some(v.clone())
    });

    agent.override_info = Some(AgentOverrideInfo {
        scope,
        settings_path,
        base_snapshot,
    });
}

/// pi `clearBuiltinThinking` (`agents.ts:1033-1037`): a no-op when the agent has no `thinking`;
/// otherwise drop `thinking` and record disable-thinking provenance ONLY if the agent has no
/// override recorded yet (a per-agent override already applied in this pass keeps its own
/// provenance/base). The snapshot is captured BEFORE the clear.
fn clear_builtin_thinking(
    agent: &mut AgentDefinition,
    scope: OverrideScope,
    settings_path: PathBuf,
) {
    if agent.thinking.is_none() {
        return;
    }
    if agent.override_info.is_none() {
        let base_snapshot = Box::new(agent.clone());
        agent.override_info = Some(AgentOverrideInfo {
            scope,
            settings_path,
            base_snapshot,
        });
    }
    agent.thinking = None;
}

/// One field's full-replace application (R-SA-010 builtin branch): `Unset` is a no-op,
/// `ExplicitClear` resets `*target` to the caller-supplied `clear_value` (the field's own
/// "absent"/neutral value — `None` for `Option<_>` fields, `Vec::new()` for list fields, or a
/// fixed sentinel enum variant for a field with no natural `Option`/empty shape, e.g.
/// `SystemPromptMode::Replace`), `Value(v)` resets `*target` to `to_target(v)`. Generic over the
/// override's carried type `T` and the field's own type `F` (they differ for e.g.
/// `Option<ModelId>` fields overridden by a bare `OverrideField<String>`), so every call site above
/// stays a single readable line instead of a hand-written match arm per field. Takes `clear_value`
/// by parameter (rather than an `F: Default` bound) so this helper works uniformly across
/// `Option<_>`/`Vec<_>` fields AND plain-enum fields like `SystemPromptMode` that have no
/// `Default` impl of their own — R-SA-011's three-state contract applies to every override field
/// alike, not only ones with a natural zero value.
fn apply_field_full_replace<T, F>(
    target: &mut F,
    delta: &OverrideField<T>,
    clear_value: F,
    to_target: impl FnOnce(&T) -> F,
) {
    match delta {
        OverrideField::Unset => {}
        OverrideField::ExplicitClear => *target = clear_value,
        OverrideField::Value(v) => *target = to_target(v),
    }
}

/// pi `applyToolsOverride` (`agents.ts:1237-1246`). Deliberately NOT expressed through
/// [`apply_field_full_replace`]: the four states map to three DIFFERENT non-`Unset` outcomes, and
/// the middle two are opposites at a security boundary —
///
/// - `false` -> `splitToolList([])` -> `Some(vec![])`, the EMPTY allowlist ("no tools at all");
/// - `"inherit"` -> `delete target.tools` -> `None`, NO allowlist ("the full parent tool surface").
///
/// Collapsing them would hand an agent every tool where the operator asked for none. Shared by
/// both apply paths, exactly as pi shares the one function between `applyBuiltinOverride` and
/// `applyCustomAgentOverride`. pi additionally deletes/repopulates `mcpDirectTools` alongside;
/// this crate carries MCP direct tools inside the same list, so the one assignment covers both.
fn apply_tools_override(
    target: &mut Option<Vec<super::types::ToolRef>>,
    delta: &ToolsOverrideField,
) {
    match delta {
        ToolsOverrideField::Unset => {}
        ToolsOverrideField::ExplicitClear => *target = Some(Vec::new()),
        ToolsOverrideField::Inherit => *target = None,
        ToolsOverrideField::Value(tools) => *target = Some(tools.clone()),
    }
}

/// pi's `output` override arm (`agents.ts:1259`), adapted to this crate's merged
/// [`OutputSpec`].
///
/// Upstream `output` (a path string) and `outputMode` are two INDEPENDENT `AgentConfig` fields, so
/// `next.output = override.output` leaves `outputMode` untouched. This crate stores the pair as one
/// [`OutputSpec`], so a naive whole-struct replace would silently drop an already-resolved
/// `mode` — a behavioural divergence from pi that no test of `output` alone would catch. Hence the
/// concrete arm rebuilds the spec around the EXISTING mode, and only the explicit clear (pi's
/// `delete next.output`) drops the whole thing.
fn apply_output_override(agent: &mut AgentDefinition, delta: &OverrideField<String>) {
    match delta {
        OverrideField::Unset => {}
        OverrideField::ExplicitClear => agent.output = None,
        OverrideField::Value(path) => {
            let mode = agent.output.as_ref().and_then(|spec| spec.mode);
            agent.output = Some(OutputSpec {
                path: Some(PathBuf::from(path)),
                mode,
            });
        }
    }
}

/// pi `applyCustomAgentOverrides`' per-agent body (`agents.ts:1200-1212`): project override wins over
/// user override; only ONE is ever applied, with the SETTINGS scope/path (never the agent's own
/// source). The project branch is gated on the project scope actually existing.
fn apply_custom_agent(agent: &mut AgentDefinition, settings: &LayeredOverrideSettings) {
    if let (Some(delta), Some(path)) = (
        settings.project.overrides.get(&agent.name),
        settings.project_settings_path.as_ref(),
    ) {
        apply_custom_override(agent, delta, OverrideScope::Project, path.clone());
    } else if let Some(delta) = settings.user.overrides.get(&agent.name) {
        apply_custom_override(
            agent,
            delta,
            OverrideScope::User,
            settings.user_settings_path.clone(),
        );
    }
}

/// pi `applyCustomAgentOverride` (`agents.ts:1099-1191`): fill-unset-only — a field is applied only
/// when its frontmatter key was absent from the agent's own on-disk frontmatter (an explicitly
/// present field blocks the override for that field unconditionally, regardless of the delta's own
/// value). `systemPrompt` is deliberately NOT a custom-agent override (pi omits it here — it only
/// ever replaces a BUILTIN body). Provenance uses the passed settings `scope`/`settings_path`
/// (never the agent's own source), recorded only when at least one field actually applied.
fn apply_custom_override(
    agent: &mut AgentDefinition,
    delta: &AgentOverrideConfig,
    scope: OverrideScope,
    settings_path: PathBuf,
) {
    let base_snapshot = Box::new(agent.clone());
    let mut applied_any = false;

    // pi `description` custom arm (agents.ts:1380-1383): UNCONDITIONAL — no frontmatter gate and
    // no clear form, unlike every fill below it.
    if let OverrideField::Value(v) = &delta.description {
        agent.description = v.clone();
        applied_any = true;
    }
    // pi `fill("output", ["output"], ...)` (agents.ts:1384-1386) — gated, but the concrete arm has
    // to preserve `mode`, so it cannot go through `apply_field_fill_unset` (which cannot read the
    // target). Mirrors the `disabled` arm below in hand-rolling the gate.
    if !agent.present_fields.contains("output") && delta.output.is_present() {
        apply_output_override(agent, &delta.output);
        applied_any = true;
    }
    applied_any |= apply_field_fill_unset(
        &mut agent.default_reads,
        &["defaultReads"],
        &agent.present_fields,
        &delta.default_reads,
        None,
        |v| Some(v.iter().map(PathBuf::from).collect()),
    );
    let model_applied = apply_field_fill_unset(
        &mut agent.model,
        &["model"],
        &agent.present_fields,
        &delta.model,
        None,
        |v| Some(cyrup_core::ModelId::from(v.clone())),
    );
    if model_applied {
        // The fill only runs when `model` was absent from disk AND the delta stated it; a concrete
        // value is a settings-override source, an explicit clear leaves no model.
        agent.model_source = match &delta.model {
            OverrideField::Value(_) => Some(AgentModelSourceInfo::SettingsOverride),
            _ => None,
        };
    }
    applied_any |= model_applied;
    applied_any |= apply_field_fill_unset(
        &mut agent.fallback_models,
        &["fallbackModels"],
        &agent.present_fields,
        &delta.fallback_models,
        Vec::new(),
        |v| v.iter().cloned().map(cyrup_core::ModelId::from).collect(),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.thinking,
        &["thinking"],
        &agent.present_fields,
        &delta.thinking,
        None,
        |v| Some(v.clone()),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.system_prompt_mode,
        &["systemPromptMode"],
        &agent.present_fields,
        &delta.system_prompt_mode,
        crate::discovery::types::SystemPromptMode::Replace,
        |v| *v,
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.inherit_project_context,
        &["inheritProjectContext"],
        &agent.present_fields,
        &delta.inherit_project_context,
        false,
        |v| *v,
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.inherit_skills,
        &["inheritSkills"],
        &agent.present_fields,
        &delta.inherit_skills,
        false,
        |v| *v,
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.default_context,
        &["defaultContext"],
        &agent.present_fields,
        &delta.default_context,
        None,
        |v| Some(*v),
    );
    // pi `disabled` custom arm (agents.ts:893-896) gates on the runtime VALUE (`agent.disabled ===
    // undefined`), not frontmatter-field presence, and has no `| false` clear form.
    if agent.disabled.is_none()
        && let OverrideField::Value(v) = &delta.disabled
    {
        agent.disabled = Some(*v);
        applied_any = true;
    }
    // pi checks BOTH `skill` and `skills` frontmatter keys for the skills fill (agents.ts:898).
    applied_any |= apply_field_fill_unset(
        &mut agent.skills,
        &["skill", "skills"],
        &agent.present_fields,
        &delta.skills,
        Vec::new(),
        |v| v.clone(),
    );
    // pi `tools` custom arm (agents.ts:1438-1441): the frontmatter gate is hand-rolled upstream
    // too, because the four-state apply is a shared function rather than a value assignment.
    if !agent.present_fields.contains("tools") && delta.tools.is_present() {
        apply_tools_override(&mut agent.tools, &delta.tools);
        applied_any = true;
    }
    // SUBA-092 / pi `agents.ts:1547-1552` @v0.62.0: `fill("excludeTools", ["excludeTools"],
    // override.excludeTools === false ? undefined : [...])` and `fill("allowNestedSubagents",
    // ["allowNestedSubagents"], ...)` — the same frontmatter-presence gate as every fill above.
    // (At v0.64.0 `31562d76` collapsed `applyCustomAgentOverride` into `applyBuiltinOverride` for
    // EVERY field, so the gate no longer exists upstream for any key; that is a cross-field
    // precedence change to R-SA-010 and is tracked separately, not folded into this item.)
    applied_any |= apply_field_fill_unset(
        &mut agent.exclude_tools,
        &["excludeTools"],
        &agent.present_fields,
        &delta.exclude_tools,
        None,
        |v| Some(v.clone()),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.allow_nested_subagents,
        &["allowNestedSubagents"],
        &agent.present_fields,
        &delta.allow_nested_subagents,
        None,
        |v| Some(*v),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.extensions,
        &["extensions"],
        &agent.present_fields,
        &delta.extensions,
        None,
        |v| Some(v.clone()),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.subagent_only_extensions,
        &["subagentOnlyExtensions"],
        &agent.present_fields,
        &delta.subagent_only_extensions,
        Vec::new(),
        |v| v.clone(),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.completion_guard,
        &["completionGuard"],
        &agent.present_fields,
        &delta.completion_guard,
        None,
        |v| Some(*v),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.tool_budget,
        &["toolBudget"],
        &agent.present_fields,
        &delta.tool_budget,
        None,
        |v| Some(v.clone()),
    );

    if applied_any {
        agent.override_info = Some(AgentOverrideInfo {
            scope,
            settings_path,
            base_snapshot,
        });
    }
}

/// One field's fill-unset-only application (R-SA-010 custom branch). Returns `true` iff the field
/// was actually applied (i.e. it was both present-in-delta and absent-from-disk) — used by the
/// caller to decide whether [`AgentOverrideInfo`] provenance should be recorded at all (R-SA-010's
/// data model note: "present only when at least one override field actually applied"). Takes a
/// slice of `frontmatter_fields` (rather than a single name) because pi's `fill` gate checks a
/// LIST of keys for at least one field — notably `skills`, which pi blocks on either `skill` OR
/// `skills` being present on disk (`agents.ts:898`). Takes `clear_value` by parameter (rather than
/// an `F: Default` bound) so this helper works uniformly across `Option<_>`/`Vec<_>` fields AND
/// plain-value fields like `bool`/`SystemPromptMode` that have no `Default` impl of their own.
fn apply_field_fill_unset<T, F>(
    target: &mut F,
    frontmatter_fields: &[&str],
    present_fields: &std::collections::HashSet<String>,
    delta: &OverrideField<T>,
    clear_value: F,
    to_target: impl FnOnce(&T) -> F,
) -> bool {
    if frontmatter_fields
        .iter()
        .any(|field| present_fields.contains(*field))
    {
        // Explicitly present on disk: the override is blocked for this field, full stop —
        // regardless of the delta's own value or the delta's own Unset/ExplicitClear/Value state.
        return false;
    }
    match delta {
        OverrideField::Unset => false,
        OverrideField::ExplicitClear => {
            *target = clear_value;
            true
        }
        OverrideField::Value(v) => {
            *target = to_target(v);
            true
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use super::*;
    use crate::discovery::types::{
        OutputSpec, ResolvedToolBudget, SubagentSettings, SystemPromptMode, ToolBudgetBlock, ToolRef,
    };
    use crate::fork_context::ContextMode;

    // Two fixed settings-file paths the two-scope helpers below stamp into provenance, so every
    // override-scope assertion can check the EXACT `settings.json` path (never the agent's own `.md`).
    const USER_SETTINGS: &str = "/user/settings.json";
    const PROJECT_SETTINGS: &str = "/proj/settings.json";

    /// A [`LayeredOverrideSettings`] carrying only user-scope settings, with a project scope that
    /// EXISTS (non-`None` path, mirroring pi's always-non-null `projectSettingsPath`) but is empty —
    /// the common "user customized, project didn't" shape.
    fn user_scope(user: SubagentSettings) -> LayeredOverrideSettings {
        LayeredOverrideSettings {
            user,
            project: SubagentSettings::default(),
            user_settings_path: PathBuf::from(USER_SETTINGS),
            project_settings_path: Some(PathBuf::from(PROJECT_SETTINGS)),
        }
    }

    /// A [`LayeredOverrideSettings`] carrying only project-scope settings (empty user scope).
    fn project_scope(project: SubagentSettings) -> LayeredOverrideSettings {
        LayeredOverrideSettings {
            user: SubagentSettings::default(),
            project,
            user_settings_path: PathBuf::from(USER_SETTINGS),
            project_settings_path: Some(PathBuf::from(PROJECT_SETTINGS)),
        }
    }

    /// Both scopes populated, each with its own settings path (project wins per pi).
    fn two_scope(user: SubagentSettings, project: SubagentSettings) -> LayeredOverrideSettings {
        LayeredOverrideSettings {
            user,
            project,
            user_settings_path: PathBuf::from(USER_SETTINGS),
            project_settings_path: Some(PathBuf::from(PROJECT_SETTINGS)),
        }
    }

    fn settings_with_override(name: &str, cfg: AgentOverrideConfig) -> SubagentSettings {
        let mut overrides = BTreeMap::new();
        overrides.insert(name.to_string(), cfg);
        SubagentSettings {
            overrides,
            ..Default::default()
        }
    }

    fn agent(name: &str, source: AgentSource, file_path: &str) -> AgentDefinition {
        AgentDefinition {
            default_turn_budget: None,
            default_acceptance: None,
            acceptance_role: None,
            permission_rules: None,
            runner: None,
            name: name.to_string(),
            local_name: name.to_string(),
            package_name: None,
            description: format!("{name} description"),
            aliases: Vec::new(),
            tools: None,
            extensions: None,
            extensions_from_default: false,
            subagent_only_extensions: Vec::new(),
            exclude_tools: None,
            allow_nested_subagents: None,
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
            system_prompt_body: format!("{name} body"),
            source,
            file_path: PathBuf::from(file_path),
            present_fields: Default::default(),
            extra_fields: Default::default(),
            override_info: None,
            model_source: None,
        }
    }

    fn agent_with_present(
        name: &str,
        source: AgentSource,
        file_path: &str,
        present: &[&str],
    ) -> AgentDefinition {
        let mut a = agent(name, source, file_path);
        a.present_fields = present.iter().map(|s| s.to_string()).collect();
        a
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-001: four-tier cross-scope precedence (project beats user beats package beats builtin)
    // -----------------------------------------------------------------------------------------

    #[test]
    fn project_wins_over_user_wins_over_package_wins_over_builtin_on_name_collision() {
        let tiers = TieredAgents {
            builtin: vec![agent("reviewer", AgentSource::Builtin, "/builtin/reviewer.md")],
            package: vec![agent("reviewer", AgentSource::Package, "/pkg/reviewer.md")],
            user: vec![agent("reviewer", AgentSource::User, "/user/reviewer.md")],
            project: vec![agent("reviewer", AgentSource::Project, "/proj/reviewer.md")],
        };
        let merged = merge_tiers(tiers);
        let winner = merged.get("reviewer").expect("reviewer present");
        assert_eq!(winner.source, AgentSource::Project);
        assert_eq!(winner.file_path, PathBuf::from("/proj/reviewer.md"));
    }

    #[test]
    fn user_wins_over_package_and_builtin_when_no_project_entry() {
        let tiers = TieredAgents {
            builtin: vec![agent("scout", AgentSource::Builtin, "/builtin/scout.md")],
            package: vec![agent("scout", AgentSource::Package, "/pkg/scout.md")],
            user: vec![agent("scout", AgentSource::User, "/user/scout.md")],
            project: vec![],
        };
        let merged = merge_tiers(tiers);
        assert_eq!(merged.get("scout").expect("present").source, AgentSource::User);
    }

    #[test]
    fn package_wins_over_builtin_when_no_user_or_project_entry() {
        let tiers = TieredAgents {
            builtin: vec![agent("worker", AgentSource::Builtin, "/builtin/worker.md")],
            package: vec![agent("worker", AgentSource::Package, "/pkg/worker.md")],
            user: vec![],
            project: vec![],
        };
        let merged = merge_tiers(tiers);
        assert_eq!(merged.get("worker").expect("present").source, AgentSource::Package);
    }

    #[test]
    fn builtin_alone_survives_with_no_higher_tier_entries() {
        let tiers = TieredAgents {
            builtin: vec![agent("delegate", AgentSource::Builtin, "/builtin/delegate.md")],
            package: vec![],
            user: vec![],
            project: vec![],
        };
        let merged = merge_tiers(tiers);
        assert_eq!(merged.get("delegate").expect("present").source, AgentSource::Builtin);
    }

    #[test]
    fn non_colliding_names_across_all_four_tiers_all_survive() {
        let tiers = TieredAgents {
            builtin: vec![agent("a", AgentSource::Builtin, "/b/a.md")],
            package: vec![agent("b", AgentSource::Package, "/p/b.md")],
            user: vec![agent("c", AgentSource::User, "/u/c.md")],
            project: vec![agent("d", AgentSource::Project, "/j/d.md")],
        };
        let merged = merge_tiers(tiers);
        assert_eq!(merged.len(), 4);
        assert!(merged.contains_key("a"));
        assert!(merged.contains_key("b"));
        assert!(merged.contains_key("c"));
        assert!(merged.contains_key("d"));
    }

    #[test]
    fn merge_tiers_matches_precedence_rank_ordering() {
        // Sanity-check the insertion order used by `merge_tiers` against
        // `AgentSource::precedence_rank`'s independently-declared "lower rank wins" contract, so
        // the two never silently drift apart.
        assert!(AgentSource::Project.precedence_rank() < AgentSource::User.precedence_rank());
        assert!(AgentSource::User.precedence_rank() < AgentSource::Package.precedence_rank());
        assert!(AgentSource::Package.precedence_rank() < AgentSource::Builtin.precedence_rank());
        // SUBA-084: the fifth variant never enters `merge_tiers` (runtime agents are merged
        // afterwards by `runtime_registry::merge_runtime_agents`, which fails closed on a
        // collision); its rank only keeps the "lower wins" contract total, pinned here so an
        // enum extension cannot silently re-rank it into the four-tier merge.
        assert_eq!(
            AgentSource::Runtime.precedence_rank(),
            AgentSource::Project.precedence_rank()
        );
        assert!(!AgentSource::Runtime.is_writable());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-002: the asymmetric dedup rule — package is first-wins, user/project are last-wins.
    // A fixture where naive "first-wins-everywhere" would give a DIFFERENT, WRONG answer than the
    // real "last-wins-for-user/project" rule.
    // -----------------------------------------------------------------------------------------

    #[test]
    fn package_tier_is_first_seen_wins() {
        let candidates = vec![
            agent("scout", AgentSource::Package, "/pkg/root-a/scout.md"),
            agent("scout", AgentSource::Package, "/pkg/root-b/scout.md"),
            agent("scout", AgentSource::Package, "/pkg/root-c/scout.md"),
        ];
        let by_name = reduce_first_seen_wins(candidates);
        assert_eq!(
            by_name.get("scout").expect("present").file_path,
            PathBuf::from("/pkg/root-a/scout.md"),
            "package tier must keep the FIRST package root's definition"
        );
    }

    #[test]
    fn user_tier_is_last_seen_wins_not_first_seen_wins() {
        // If this were (incorrectly) reduced with first-seen-wins, the winner would be
        // "/user/dir-a/scout.md" — the REAL rule (last-seen-wins) must instead keep
        // "/user/dir-c/scout.md", the last one scanned. This is the concrete asymmetry fixture.
        let candidates = vec![
            agent("scout", AgentSource::User, "/user/dir-a/scout.md"),
            agent("scout", AgentSource::User, "/user/dir-b/scout.md"),
            agent("scout", AgentSource::User, "/user/dir-c/scout.md"),
        ];
        let naive_first_wins_answer = PathBuf::from("/user/dir-a/scout.md");
        let real_last_wins_answer = PathBuf::from("/user/dir-c/scout.md");

        let by_name = reduce_last_seen_wins(candidates);
        let winner = &by_name.get("scout").expect("present").file_path;

        assert_ne!(
            *winner, naive_first_wins_answer,
            "user tier must NOT behave like first-seen-wins"
        );
        assert_eq!(
            *winner, real_last_wins_answer,
            "user tier must keep the LAST directory scanned's definition"
        );
    }

    #[test]
    fn project_tier_is_last_seen_wins_independently_of_user_tier() {
        let candidates = vec![
            agent("worker", AgentSource::Project, "/proj/dir-a/worker.md"),
            agent("worker", AgentSource::Project, "/proj/dir-b/worker.md"),
        ];
        let by_name = reduce_last_seen_wins(candidates);
        assert_eq!(
            by_name.get("worker").expect("present").file_path,
            PathBuf::from("/proj/dir-b/worker.md")
        );
    }

    #[test]
    fn package_and_user_tiers_disagree_on_the_same_input_shape_proving_asymmetry() {
        // The exact same 3-element candidate shape (only source/paths differ) fed through each
        // tier's real reduction function must produce OPPOSITE winners (first vs. last), proving
        // the two rules are genuinely different code paths, not one rule wearing two names.
        let package_candidates = vec![
            agent("x", AgentSource::Package, "/pkg/1.md"),
            agent("x", AgentSource::Package, "/pkg/2.md"),
            agent("x", AgentSource::Package, "/pkg/3.md"),
        ];
        let user_candidates = vec![
            agent("x", AgentSource::User, "/usr/1.md"),
            agent("x", AgentSource::User, "/usr/2.md"),
            agent("x", AgentSource::User, "/usr/3.md"),
        ];

        let package_winner = reduce_first_seen_wins(package_candidates)
            .get("x")
            .expect("present")
            .file_path
            .clone();
        let user_winner = reduce_last_seen_wins(user_candidates)
            .get("x")
            .expect("present")
            .file_path
            .clone();

        assert_eq!(package_winner, PathBuf::from("/pkg/1.md"), "package: FIRST wins");
        assert_eq!(user_winner, PathBuf::from("/usr/3.md"), "user: LAST wins");
    }

    #[test]
    fn end_to_end_merge_exercises_both_asymmetric_rules_and_cross_tier_precedence_together() {
        // A full four-tier fixture: package has three roots for "scout" (first must win among
        // them), user has three dirs for "scout" (last must win among them) AND user overall
        // still loses to project on the cross-tier merge, project has one entry for "scout".
        let tiers = TieredAgents {
            builtin: vec![agent("scout", AgentSource::Builtin, "/builtin/scout.md")],
            package: vec![
                agent("scout", AgentSource::Package, "/pkg/root-a/scout.md"),
                agent("scout", AgentSource::Package, "/pkg/root-b/scout.md"),
            ],
            user: vec![
                agent("scout", AgentSource::User, "/user/dir-a/scout.md"),
                agent("scout", AgentSource::User, "/user/dir-b/scout.md"),
            ],
            project: vec![agent("scout", AgentSource::Project, "/proj/scout.md")],
        };
        let merged = merge_tiers(tiers);
        // Project tier wins the cross-tier merge outright, regardless of what won within package
        // or user tiers individually.
        assert_eq!(
            merged.get("scout").expect("present").file_path,
            PathBuf::from("/proj/scout.md")
        );
    }

    #[test]
    fn when_project_absent_the_intra_tier_asymmetric_winners_surface_correctly() {
        // Remove the project entry so the user tier's own (last-wins) winner is what surfaces at
        // the top level, proving the asymmetric intra-tier rule's effect is actually visible
        // end-to-end, not just in the isolated reduce_* unit tests above.
        let tiers = TieredAgents {
            builtin: vec![agent("scout", AgentSource::Builtin, "/builtin/scout.md")],
            package: vec![
                agent("scout", AgentSource::Package, "/pkg/root-a/scout.md"),
                agent("scout", AgentSource::Package, "/pkg/root-b/scout.md"),
            ],
            user: vec![
                agent("scout", AgentSource::User, "/user/dir-a/scout.md"),
                agent("scout", AgentSource::User, "/user/dir-b/scout.md"),
                agent("scout", AgentSource::User, "/user/dir-c/scout.md"),
            ],
            project: vec![],
        };
        let merged = merge_tiers(tiers);
        assert_eq!(
            merged.get("scout").expect("present").file_path,
            PathBuf::from("/user/dir-c/scout.md"),
            "user tier's last-wins winner must surface when no project entry beats it"
        );
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-010/011: fill-unset-only override semantics
    // -----------------------------------------------------------------------------------------

    #[test]
    fn builtin_override_fully_replaces_every_delta_field_unconditionally() {
        let mut merged = HashMap::new();
        let mut a = agent("delegate", AgentSource::Builtin, "/builtin/delegate.md");
        a.thinking = Some("low".to_string());
        a.present_fields.insert("thinking".to_string());
        merged.insert("delegate".to_string(), a);

        let settings = user_scope(settings_with_override(
            "delegate",
            AgentOverrideConfig {
                thinking: OverrideField::Value("high".to_string()),
                ..Default::default()
            },
        ));

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("delegate").expect("present");
        assert_eq!(
            updated.thinking,
            Some("high".to_string()),
            "builtin branch overwrites even a field present on disk"
        );
        let info = updated.override_info.as_ref().expect("override recorded");
        assert_eq!(info.scope, OverrideScope::User);
        assert_eq!(
            info.settings_path,
            PathBuf::from(USER_SETTINGS),
            "provenance path must be the settings.json, not the agent's own .md"
        );
    }

    #[test]
    fn builtin_override_explicit_clear_resets_to_default() {
        let mut merged = HashMap::new();
        let mut a = agent("delegate", AgentSource::Builtin, "/builtin/delegate.md");
        a.disabled = Some(true);
        merged.insert("delegate".to_string(), a);

        let settings = user_scope(settings_with_override(
            "delegate",
            AgentOverrideConfig {
                disabled: OverrideField::ExplicitClear,
                ..Default::default()
            },
        ));

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("delegate").expect("present").disabled, None);
    }

    #[test]
    fn system_prompt_override_replaces_a_builtin_body() {
        // pi `applyBuiltinOverride` (agents.ts:1018): a `systemPrompt` override replaces the builtin
        // persona's own body prose. This is one of the six fields an earlier port dropped.
        let mut merged = HashMap::new();
        let mut a = agent("reviewer", AgentSource::Builtin, "/builtin/reviewer.md");
        a.system_prompt_body = "original reviewer body".to_string();
        merged.insert("reviewer".to_string(), a);

        let settings = user_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                system_prompt: OverrideField::Value("You are the overridden reviewer.".to_string()),
                ..Default::default()
            },
        ));

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(
            updated.system_prompt_body, "You are the overridden reviewer.",
            "systemPrompt override must replace the builtin body"
        );
        assert_eq!(
            updated.override_info.as_ref().expect("recorded").scope,
            OverrideScope::User
        );
    }

    #[test]
    fn system_prompt_override_does_not_apply_to_a_custom_agent() {
        // pi's `applyCustomAgentOverride` omits `systemPrompt` entirely (only `applyBuiltinOverride`
        // sets it) — a custom agent's body is never replaced by a settings override.
        let mut merged = HashMap::new();
        let mut a = agent("implementer", AgentSource::Project, "/proj/impl.md");
        a.system_prompt_body = "custom implementer body".to_string();
        merged.insert("implementer".to_string(), a);

        let settings = project_scope(settings_with_override(
            "implementer",
            AgentOverrideConfig {
                system_prompt: OverrideField::Value("SHOULD NOT APPLY".to_string()),
                ..Default::default()
            },
        ));

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("implementer").expect("present");
        assert_eq!(
            updated.system_prompt_body, "custom implementer body",
            "systemPrompt is builtin-only; a custom agent body must be untouched"
        );
        // systemPrompt was the ONLY delta field and it is not a custom-agent field, so nothing
        // applied and no provenance is recorded.
        assert!(updated.override_info.is_none());
    }

    #[test]
    fn custom_agent_override_applies_only_to_absent_fields() {
        // "model" is present on disk -> override MUST be blocked for it.
        // "thinking" is absent on disk -> override MUST apply.
        let mut merged = HashMap::new();
        let mut a = agent_with_present(
            "reviewer",
            AgentSource::Project,
            "/proj/reviewer.md",
            &["model"],
        );
        a.model = Some("anthropic/claude-sonnet-4".into());
        merged.insert("reviewer".to_string(), a);

        let settings = project_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                model: OverrideField::Value("openai/gpt-5".to_string()),
                thinking: OverrideField::Value("high".to_string()),
                ..Default::default()
            },
        ));

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(
            updated.model,
            Some("anthropic/claude-sonnet-4".into()),
            "present-on-disk field must block the override even though the override supplied a \
             different value"
        );
        assert_eq!(
            updated.thinking,
            Some("high".to_string()),
            "absent-on-disk field must accept the override value"
        );
    }

    #[test]
    fn custom_agent_override_blocks_explicit_clear_when_field_present_on_disk() {
        let mut merged = HashMap::new();
        let a = agent_with_present(
            "reviewer",
            AgentSource::User,
            "/user/reviewer.md",
            &["completionGuard"],
        );
        merged.insert("reviewer".to_string(), a);

        let settings = user_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                completion_guard: OverrideField::ExplicitClear,
                ..Default::default()
            },
        ));

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        // Presence on disk blocks the override outright — even though the on-disk value here
        // happens to already be `None` (the test only marks the field "present", not populated),
        // the point is the override must not be recorded as having "applied".
        assert!(merged.get("reviewer").expect("present").override_info.is_none());
    }

    #[test]
    fn custom_agent_override_with_no_matching_delta_leaves_agent_untouched() {
        let mut merged = HashMap::new();
        merged.insert(
            "untouched".to_string(),
            agent("untouched", AgentSource::User, "/user/untouched.md"),
        );
        let settings = user_scope(SubagentSettings::default());
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert!(merged.get("untouched").expect("present").override_info.is_none());
    }

    #[test]
    fn package_sourced_agent_is_not_touched_by_settings_overrides() {
        let mut merged = HashMap::new();
        merged.insert(
            "pkgagent".to_string(),
            agent("pkgagent", AgentSource::Package, "/pkg/pkgagent.md"),
        );
        let settings = user_scope(settings_with_override(
            "pkgagent",
            AgentOverrideConfig {
                thinking: OverrideField::Value("high".to_string()),
                ..Default::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("pkgagent").expect("present");
        assert_eq!(updated.thinking, None, "package-sourced agents are not overridable");
        assert!(updated.override_info.is_none());
    }

    #[test]
    fn empty_override_delta_is_a_no_op_and_records_no_provenance() {
        let mut merged = HashMap::new();
        merged.insert(
            "reviewer".to_string(),
            agent("reviewer", AgentSource::Project, "/proj/reviewer.md"),
        );
        let settings = project_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig::default(),
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert!(merged.get("reviewer").expect("present").override_info.is_none());
    }

    #[test]
    fn override_targeting_unknown_agent_name_is_silently_ignored() {
        let mut merged: HashMap<String, AgentDefinition> = HashMap::new();
        let settings = user_scope(settings_with_override(
            "ghost",
            AgentOverrideConfig {
                thinking: OverrideField::Value("high".to_string()),
                ..Default::default()
            },
        ));
        // Must not error and must not panic.
        apply_overrides(&mut merged, &settings).expect("apply succeeds even with a dangling name");
        assert!(merged.is_empty());
    }

    #[test]
    fn user_override_on_a_project_agent_records_user_scope_and_settings_path() {
        // pi agent-overrides.test.ts:374-385: a project-SOURCED custom agent with ONLY a user-scope
        // override entry gets the override applied at scope "user" (the SETTINGS scope), NOT the
        // agent's own `project` source — and the provenance path is the user `settings.json`, not
        // the agent's `.md`. This is the exact provenance defect Tier 7 fixes.
        let mut merged = HashMap::new();
        merged.insert(
            "implementer".to_string(),
            agent("implementer", AgentSource::Project, "/proj/impl.md"),
        );
        let settings = user_scope(settings_with_override(
            "implementer",
            AgentOverrideConfig {
                model: OverrideField::Value("anthropic/claude-sonnet-4-6".to_string()),
                ..Default::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("implementer").expect("present");
        assert_eq!(updated.model, Some("anthropic/claude-sonnet-4-6".into()));
        let info = updated.override_info.as_ref().expect("override recorded");
        assert_eq!(
            info.scope,
            OverrideScope::User,
            "scope is the settings scope (user), NOT the agent's own project source"
        );
        assert_eq!(
            info.settings_path,
            PathBuf::from(USER_SETTINGS),
            "settings_path is the user settings.json, NOT the agent's own .md"
        );
    }

    #[test]
    fn project_override_beats_user_override_on_a_custom_agent() {
        // pi agent-overrides.test.ts:387-401: a same-named user+project override on a project custom
        // agent -> the PROJECT override wins, at scope "project".
        let mut merged = HashMap::new();
        merged.insert(
            "implementer".to_string(),
            agent("implementer", AgentSource::Project, "/proj/impl.md"),
        );
        let settings = two_scope(
            settings_with_override(
                "implementer",
                AgentOverrideConfig {
                    model: OverrideField::Value("anthropic/claude-sonnet-4-6".to_string()),
                    ..Default::default()
                },
            ),
            settings_with_override(
                "implementer",
                AgentOverrideConfig {
                    model: OverrideField::Value("openai/gpt-5.4".to_string()),
                    ..Default::default()
                },
            ),
        );
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("implementer").expect("present");
        assert_eq!(updated.model, Some("openai/gpt-5.4".into()), "project override wins");
        assert_eq!(
            updated.override_info.as_ref().expect("recorded").scope,
            OverrideScope::Project
        );
    }

    #[test]
    fn custom_override_full_field_coverage_applies_every_absent_field() {
        let mut merged = HashMap::new();
        merged.insert(
            "reviewer".to_string(),
            agent("reviewer", AgentSource::Project, "/proj/reviewer.md"),
        );
        let settings = project_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                model: OverrideField::Value("openai/gpt-5".to_string()),
                fallback_models: OverrideField::Value(vec!["anthropic/claude-sonnet-4".to_string()]),
                thinking: OverrideField::Value("medium".to_string()),
                system_prompt_mode: OverrideField::Value(SystemPromptMode::Append),
                inherit_project_context: OverrideField::Value(true),
                inherit_skills: OverrideField::Value(true),
                default_context: OverrideField::Value(ContextMode::Fork),
                disabled: OverrideField::Value(true),
                // systemPrompt is set but must NOT apply to a custom agent (builtin-only field).
                system_prompt: OverrideField::Value("should not apply".to_string()),
                skills: OverrideField::Value(vec!["tdd".to_string()]),
                tools: ToolsOverrideField::Value(vec![ToolRef::Builtin("read".to_string())]),
                exclude_tools: OverrideField::Value(vec!["bash".to_string()]),
                allow_nested_subagents: OverrideField::Value(true),
                subagent_only_extensions: OverrideField::Value(vec![
                    "./tools/child-review.ts".to_string(),
                ]),
                completion_guard: OverrideField::Value(false),
                description: OverrideField::Value("overridden description".to_string()),
                output: OverrideField::Value("./out/review.md".to_string()),
                default_reads: OverrideField::Value(vec!["./AGENTS.md".to_string()]),
                extensions: OverrideField::Value(vec!["./ext/review.ts".to_string()]),
                tool_budget: OverrideField::Value(ResolvedToolBudget {
                    hard: 40,
                    soft: Some(30),
                    block: ToolBudgetBlock::Names(vec!["read".to_string()]),
                }),
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(updated.model, Some("openai/gpt-5".into()));
        // SUBA-092: the two v0.62.0 keys fill like every other absent-on-disk field.
        assert_eq!(updated.exclude_tools, Some(vec!["bash".to_string()]));
        assert_eq!(updated.allow_nested_subagents, Some(true));
        assert_eq!(updated.fallback_models, vec!["anthropic/claude-sonnet-4".into()]);
        assert_eq!(updated.thinking, Some("medium".to_string()));
        assert_eq!(updated.system_prompt_mode, SystemPromptMode::Append);
        assert!(updated.inherit_project_context);
        assert!(updated.inherit_skills);
        assert_eq!(updated.default_context, Some(ContextMode::Fork));
        assert_eq!(updated.disabled, Some(true));
        assert_eq!(
            updated.system_prompt_body, "reviewer body",
            "systemPrompt is builtin-only and must not touch a custom agent's body"
        );
        assert_eq!(updated.skills, vec!["tdd".to_string()]);
        assert_eq!(updated.tools, Some(vec![ToolRef::Builtin("read".to_string())]));
        assert_eq!(
            updated.subagent_only_extensions,
            vec!["./tools/child-review.ts".to_string()]
        );
        assert_eq!(updated.completion_guard, Some(false));
        // SUBA-081's five added fields, on the custom (fill-unset) path.
        assert_eq!(updated.description, "overridden description");
        assert_eq!(
            updated.output,
            Some(OutputSpec {
                path: Some(PathBuf::from("./out/review.md")),
                mode: None,
            })
        );
        assert_eq!(updated.default_reads, Some(vec![PathBuf::from("./AGENTS.md")]));
        assert_eq!(updated.extensions, Some(vec!["./ext/review.ts".to_string()]));
        assert_eq!(
            updated.tool_budget,
            Some(ResolvedToolBudget {
                hard: 40,
                soft: Some(30),
                block: ToolBudgetBlock::Names(vec!["read".to_string()]),
            })
        );
        assert!(updated.override_info.is_some());
    }

    // -----------------------------------------------------------------------------------------
    // SUBA-081: the five previously-unmodeled override fields, plus `tools: "inherit"`
    // -----------------------------------------------------------------------------------------

    /// A full delta for the five fields SUBA-081 added, used by the builtin-path tests below.
    fn suba081_delta() -> AgentOverrideConfig {
        AgentOverrideConfig {
            description: OverrideField::Value("from settings".to_string()),
            output: OverrideField::Value("./out/report.md".to_string()),
            default_reads: OverrideField::Value(vec!["./docs/spec.md".to_string()]),
            extensions: OverrideField::Value(vec!["./ext/a.ts".to_string()]),
            tool_budget: OverrideField::Value(ResolvedToolBudget {
                hard: 12,
                soft: None,
                block: ToolBudgetBlock::Names(vec!["bash".to_string()]),
            }),
            ..AgentOverrideConfig::default()
        }
    }

    /// The same five fields, each stating pi's `false` explicit-clear.
    fn suba081_clear_delta() -> AgentOverrideConfig {
        AgentOverrideConfig {
            output: OverrideField::ExplicitClear,
            default_reads: OverrideField::ExplicitClear,
            extensions: OverrideField::ExplicitClear,
            tool_budget: OverrideField::ExplicitClear,
            ..AgentOverrideConfig::default()
        }
    }

    #[test]
    fn builtin_override_applies_the_five_suba081_fields() {
        let mut merged = HashMap::new();
        merged.insert(
            "worker".to_string(),
            agent("worker", AgentSource::Builtin, "/builtin/worker.md"),
        );
        let settings = user_scope(settings_with_override("worker", suba081_delta()));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("worker").expect("present");
        assert_eq!(updated.description, "from settings");
        assert_eq!(
            updated.output,
            Some(OutputSpec {
                path: Some(PathBuf::from("./out/report.md")),
                mode: None,
            })
        );
        assert_eq!(updated.default_reads, Some(vec![PathBuf::from("./docs/spec.md")]));
        assert_eq!(updated.extensions, Some(vec!["./ext/a.ts".to_string()]));
        assert_eq!(
            updated.tool_budget,
            Some(ResolvedToolBudget {
                hard: 12,
                soft: None,
                block: ToolBudgetBlock::Names(vec!["bash".to_string()]),
            })
        );
    }

    #[test]
    fn builtin_override_false_clears_the_four_clearable_suba081_fields() {
        let mut merged = HashMap::new();
        let mut a = agent("worker", AgentSource::Builtin, "/builtin/worker.md");
        a.output = Some(OutputSpec {
            path: Some(PathBuf::from("./own.md")),
            mode: Some(crate::discovery::types::OutputMode::FileOnly),
        });
        a.default_reads = Some(vec![PathBuf::from("./own-read.md")]);
        a.extensions = Some(vec!["./own-ext.ts".to_string()]);
        a.tool_budget = Some(ResolvedToolBudget {
            hard: 5,
            soft: None,
            block: ToolBudgetBlock::Names(vec!["read".to_string()]),
        });
        merged.insert("worker".to_string(), a);

        let settings = user_scope(settings_with_override("worker", suba081_clear_delta()));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("worker").expect("present");
        // pi `delete next.<field>` for all four — `None`, never an empty collection.
        assert_eq!(updated.output, None);
        assert_eq!(updated.default_reads, None);
        assert_eq!(updated.extensions, None);
        assert_eq!(updated.tool_budget, None);
    }

    #[test]
    fn output_override_replaces_the_path_and_preserves_the_resolved_mode() {
        // Upstream `output` and `outputMode` are INDEPENDENT AgentConfig fields (agents.ts:1259-1260),
        // so overriding the path never disturbs the mode. This crate merges the pair into one
        // `OutputSpec`, which is exactly where a naive whole-struct replace would silently drop it.
        let mut merged = HashMap::new();
        let mut a = agent("worker", AgentSource::Builtin, "/builtin/worker.md");
        a.output = Some(OutputSpec {
            path: Some(PathBuf::from("./old.md")),
            mode: Some(crate::discovery::types::OutputMode::FileAndInline),
        });
        merged.insert("worker".to_string(), a);

        let settings = user_scope(settings_with_override(
            "worker",
            AgentOverrideConfig {
                output: OverrideField::Value("./new.md".to_string()),
                ..AgentOverrideConfig::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("worker").expect("present").output,
            Some(OutputSpec {
                path: Some(PathBuf::from("./new.md")),
                mode: Some(crate::discovery::types::OutputMode::FileAndInline),
            }),
            "a concrete `output` override must replace only the path"
        );
    }

    #[test]
    fn tools_inherit_drops_the_allowlist_while_false_empties_it() {
        // pi `applyToolsOverride` (agents.ts:1237-1246): `"inherit"` deletes the field (no
        // restriction), `false` sets the EMPTY allowlist (no tools). Opposite outcomes.
        for (delta, expected, label) in [
            (ToolsOverrideField::Inherit, None, "inherit"),
            (ToolsOverrideField::ExplicitClear, Some(Vec::new()), "false"),
        ] {
            let mut merged = HashMap::new();
            let mut a = agent("worker", AgentSource::Builtin, "/builtin/worker.md");
            a.tools = Some(vec![ToolRef::Builtin("bash".to_string())]);
            merged.insert("worker".to_string(), a);

            let settings = user_scope(settings_with_override(
                "worker",
                AgentOverrideConfig {
                    tools: delta,
                    ..AgentOverrideConfig::default()
                },
            ));
            apply_overrides(&mut merged, &settings).expect("apply succeeds");
            assert_eq!(
                merged.get("worker").expect("present").tools,
                expected,
                "tools: {label}"
            );
        }
    }

    #[test]
    fn custom_override_tools_inherit_is_blocked_by_frontmatter_tools() {
        // The four-arm apply still goes through the fill-unset gate on the custom path: an agent
        // that declared `tools:` on disk keeps its own list, `"inherit"` notwithstanding.
        let mut merged = HashMap::new();
        let mut a = agent("reviewer", AgentSource::Project, "/proj/reviewer.md");
        a.tools = Some(vec![ToolRef::Builtin("read".to_string())]);
        a.present_fields.insert("tools".to_string());
        merged.insert("reviewer".to_string(), a);

        let settings = project_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                tools: ToolsOverrideField::Inherit,
                ..AgentOverrideConfig::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(updated.tools, Some(vec![ToolRef::Builtin("read".to_string())]));
        assert!(
            updated.override_info.is_none(),
            "a fully-blocked delta applies nothing, so it records no provenance"
        );
    }

    #[test]
    fn description_false_is_ignored_identically_on_both_apply_paths() {
        // `description` has no `| false` form upstream, but `OverrideField<String>`'s untagged
        // Deserialize still reaches `ExplicitClear` for a JSON `false` (the `Value(String)` arm
        // rejects a bool), so the state IS reachable from a settings file. Built from the real wire
        // shape rather than the variant, so this also pins that deserialize step.
        let delta: AgentOverrideConfig =
            serde_json::from_value(serde_json::json!({ "description": false }))
                .expect("`description: false` deserializes");
        assert_eq!(
            delta.description,
            OverrideField::ExplicitClear,
            "a JSON `false` must still reach the clear sentinel — if this ever changes to `Unset`, \
             the apply sites below are testing nothing"
        );

        // pi assigns the `false` into a string-typed field (agents.ts:1258/:1380), so there is no
        // parity target; both paths must IGNORE it rather than blank the text agent selection
        // depends on, and — the actual defect this pins — must not disagree with each other.
        for (source, path) in [
            (AgentSource::Builtin, "/builtin/worker.md"),
            (AgentSource::Project, "/proj/worker.md"),
        ] {
            let mut merged = HashMap::new();
            merged.insert("worker".to_string(), agent("worker", source, path));
            let settings = user_scope(settings_with_override("worker", delta.clone()));
            apply_overrides(&mut merged, &settings).expect("apply succeeds");
            assert_eq!(
                merged.get("worker").expect("present").description,
                "worker description",
                "`description: false` must leave the description untouched (source: {source:?})"
            );
        }
    }

    #[test]
    fn custom_override_description_ignores_the_frontmatter_gate() {
        // pi's custom arm sets `description` unconditionally (agents.ts:1380-1383) — unlike every
        // other custom-path field, an on-disk `description:` does NOT block it.
        let mut merged = HashMap::new();
        let mut a = agent("reviewer", AgentSource::Project, "/proj/reviewer.md");
        a.present_fields.insert("description".to_string());
        merged.insert("reviewer".to_string(), a);

        let settings = project_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                description: OverrideField::Value("settings wins".to_string()),
                ..AgentOverrideConfig::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("reviewer").expect("present").description,
            "settings wins"
        );
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-012: project disableThinking wins over user; skip-if-override-set-thinking carve-out
    // -----------------------------------------------------------------------------------------

    #[test]
    fn disable_thinking_pass_clears_thinking_when_flag_set() {
        let mut merged = HashMap::new();
        let mut a = agent("worker", AgentSource::Builtin, "/builtin/worker.md");
        a.thinking = Some("high".to_string());
        merged.insert("worker".to_string(), a);

        let settings = user_scope(SubagentSettings {
            disable_thinking: Some(true),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("worker").expect("present").thinking, None);
    }

    #[test]
    fn disable_thinking_does_not_touch_custom_user_or_project_agents() {
        // BUILTINS ONLY (pi `applyBuiltinOverrides`): a custom User/Project agent's own frontmatter
        // `thinking` survives the global `disableThinking` knob untouched, while a sibling builtin's
        // is cleared — the required "disableThinking does not touch custom agents" behavior.
        let mut merged = HashMap::new();
        let mut u = agent("custom-user", AgentSource::User, "/user/custom.md");
        u.thinking = Some("high".to_string());
        merged.insert("custom-user".to_string(), u);
        let mut p = agent("custom-proj", AgentSource::Project, "/proj/custom.md");
        p.thinking = Some("low".to_string());
        merged.insert("custom-proj".to_string(), p);
        let mut b = agent("reviewer", AgentSource::Builtin, "/builtin/reviewer.md");
        b.thinking = Some("medium".to_string());
        merged.insert("reviewer".to_string(), b);

        let settings = user_scope(SubagentSettings {
            disable_thinking: Some(true),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("custom-user").expect("present").thinking,
            Some("high".to_string()),
            "custom user agent thinking must survive disableThinking"
        );
        assert_eq!(
            merged.get("custom-proj").expect("present").thinking,
            Some("low".to_string()),
            "custom project agent thinking must survive disableThinking"
        );
        assert_eq!(
            merged.get("reviewer").expect("present").thinking,
            None,
            "a sibling builtin's thinking IS cleared"
        );
    }

    #[test]
    fn disable_thinking_builtin_with_no_override_records_the_settings_path() {
        // pi agent-overrides.test.ts:151-169: a user `disableThinking` clearing a builtin with no
        // per-agent override records provenance whose path is the user `settings.json`.
        let mut merged = HashMap::new();
        let mut a = agent("reviewer", AgentSource::Builtin, "/builtin/reviewer.md");
        a.thinking = Some("high".to_string());
        merged.insert("reviewer".to_string(), a);

        let settings = user_scope(SubagentSettings {
            disable_thinking: Some(true),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(updated.thinking, None);
        let info = updated.override_info.as_ref().expect("disableThinking records provenance");
        assert_eq!(info.scope, OverrideScope::User);
        assert_eq!(info.settings_path, PathBuf::from(USER_SETTINGS));
    }

    #[test]
    fn disable_thinking_pass_is_no_op_when_flag_absent_or_false() {
        let mut merged = HashMap::new();
        let mut a = agent("worker", AgentSource::Builtin, "/builtin/worker.md");
        a.thinking = Some("high".to_string());
        merged.insert("worker".to_string(), a);

        let settings = user_scope(SubagentSettings::default());
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("worker").expect("present").thinking, Some("high".to_string()));

        let mut merged2 = HashMap::new();
        let mut a2 = agent("worker2", AgentSource::Builtin, "/builtin/worker2.md");
        a2.thinking = Some("high".to_string());
        merged2.insert("worker2".to_string(), a2);
        let settings_false = user_scope(SubagentSettings {
            disable_thinking: Some(false),
            ..Default::default()
        });
        apply_overrides(&mut merged2, &settings_false).expect("apply succeeds");
        assert_eq!(
            merged2.get("worker2").expect("present").thinking,
            Some("high".to_string())
        );
    }

    #[test]
    fn disable_thinking_pass_skips_agent_whose_own_override_already_set_thinking() {
        // pi agent-overrides.test.ts:172-191: a same-scope explicit `thinking` override opts back in.
        let mut merged = HashMap::new();
        // Absent on disk, so the per-agent override applies first, setting a concrete thinking
        // level; the subsequent disableThinking clear must then leave it alone.
        merged.insert(
            "worker".to_string(),
            agent("worker", AgentSource::Builtin, "/builtin/worker.md"),
        );

        let settings = user_scope(SubagentSettings {
            overrides: settings_with_override(
                "worker",
                AgentOverrideConfig {
                    thinking: OverrideField::Value("xhigh".to_string()),
                    ..Default::default()
                },
            )
            .overrides,
            disable_thinking: Some(true),
            ..Default::default()
        });

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("worker").expect("present").thinking,
            Some("xhigh".to_string()),
            "an override that explicitly set thinking must survive the disableThinking pass"
        );
    }

    #[test]
    fn project_disable_thinking_overrides_a_user_thinking_override() {
        // pi agent-overrides.test.ts:193-213: a project-scope `disableThinking` clears the builtin
        // even though a USER per-agent override requested a concrete `thinking` — a user override's
        // thinking does NOT opt back in when the project scope configured disableThinking.
        let mut merged = HashMap::new();
        merged.insert(
            "reviewer".to_string(),
            agent("reviewer", AgentSource::Builtin, "/builtin/reviewer.md"),
        );
        let settings = two_scope(
            settings_with_override(
                "reviewer",
                AgentOverrideConfig {
                    thinking: OverrideField::Value("xhigh".to_string()),
                    ..Default::default()
                },
            ),
            SubagentSettings {
                disable_thinking: Some(true),
                ..Default::default()
            },
        );
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("reviewer").expect("present").thinking,
            None,
            "project disableThinking must beat a user per-agent thinking override"
        );
    }

    #[test]
    fn disable_thinking_pass_applies_to_builtins_with_no_override_entry_at_all() {
        let mut merged = HashMap::new();
        let mut a = agent("bystander", AgentSource::Builtin, "/builtin/bystander.md");
        a.thinking = Some("low".to_string());
        merged.insert("bystander".to_string(), a);

        let settings = user_scope(SubagentSettings {
            disable_thinking: Some(true),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("bystander").expect("present").thinking, None);
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-133: subagents.defaultModel fills model-less agents across every tier
    // -----------------------------------------------------------------------------------------

    #[test]
    fn default_model_fills_model_less_agents_across_all_tiers_and_records_source() {
        let mut merged = HashMap::new();
        merged.insert("b".to_string(), agent("b", AgentSource::Builtin, "/b/b.md"));
        merged.insert("u".to_string(), agent("u", AgentSource::User, "/u/u.md"));
        merged.insert("p".to_string(), agent("p", AgentSource::Project, "/p/p.md"));
        let mut with_model = agent("k", AgentSource::User, "/u/k.md");
        with_model.model = Some("google/gemini-3-pro".into());
        merged.insert("k".to_string(), with_model);

        let settings = user_scope(SubagentSettings {
            default_model: Some("deepseek-v4-flash".to_string()),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");

        for name in ["b", "u", "p"] {
            let a = merged.get(name).expect("present");
            assert_eq!(a.model, Some("deepseek-v4-flash".into()), "{name} filled from default");
            assert_eq!(
                a.model_source,
                Some(AgentModelSourceInfo::SettingsDefault),
                "{name} model source is defaultModel"
            );
        }
        // An agent with its own model keeps it and is NOT stamped SettingsDefault.
        let k = merged.get("k").expect("present");
        assert_eq!(k.model, Some("google/gemini-3-pro".into()));
        assert_eq!(k.model_source, None);
    }

    #[test]
    fn project_default_model_beats_user_default_model() {
        // pi agent-overrides.test.ts:87-99: project `defaultModel` wins over a user one.
        let mut merged = HashMap::new();
        merged.insert("worker".to_string(), agent("worker", AgentSource::Builtin, "/b/worker.md"));
        let settings = two_scope(
            SubagentSettings {
                default_model: Some("deepseek-v4-flash".to_string()),
                ..Default::default()
            },
            SubagentSettings {
                default_model: Some("deepseek-v4-pro".to_string()),
                ..Default::default()
            },
        );
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("worker").expect("present").model,
            Some("deepseek-v4-pro".into()),
            "project defaultModel must win over the user defaultModel"
        );
    }

    #[test]
    fn per_agent_model_override_wins_over_default_model_and_records_override_source() {
        let mut merged = HashMap::new();
        merged.insert("oracle".to_string(), agent("oracle", AgentSource::Builtin, "/b/oracle.md"));
        merged.insert("scout".to_string(), agent("scout", AgentSource::Builtin, "/b/scout.md"));

        let settings = user_scope(SubagentSettings {
            overrides: settings_with_override(
                "oracle",
                AgentOverrideConfig {
                    model: OverrideField::Value("deepseek-v4-pro".to_string()),
                    ..Default::default()
                },
            )
            .overrides,
            default_model: Some("deepseek-v4-flash".to_string()),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");

        let oracle = merged.get("oracle").expect("present");
        assert_eq!(oracle.model, Some("deepseek-v4-pro".into()), "override beats default");
        assert_eq!(oracle.model_source, Some(AgentModelSourceInfo::SettingsOverride));
        // No-override builtin still gets the default.
        assert_eq!(merged.get("scout").expect("present").model, Some("deepseek-v4-flash".into()));
    }

    #[test]
    fn project_override_beats_user_override_on_a_builtin() {
        // pi agent-overrides.test.ts:247-262: a same-named user+project override on a builtin -> the
        // PROJECT override wins entirely, at scope "project" with the project settings.json path.
        let mut merged = HashMap::new();
        merged.insert(
            "reviewer".to_string(),
            agent("reviewer", AgentSource::Builtin, "/b/reviewer.md"),
        );
        let settings = two_scope(
            settings_with_override(
                "reviewer",
                AgentOverrideConfig {
                    model: OverrideField::Value("openai/gpt-5.4".to_string()),
                    ..Default::default()
                },
            ),
            settings_with_override(
                "reviewer",
                AgentOverrideConfig {
                    model: OverrideField::Value("openai-codex/gpt-5.4-mini".to_string()),
                    thinking: OverrideField::Value("high".to_string()),
                    ..Default::default()
                },
            ),
        );
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(updated.model, Some("openai-codex/gpt-5.4-mini".into()), "project override wins");
        assert_eq!(updated.thinking, Some("high".to_string()));
        let info = updated.override_info.as_ref().expect("recorded");
        assert_eq!(info.scope, OverrideScope::Project);
        assert_eq!(info.settings_path, PathBuf::from(PROJECT_SETTINGS));
    }

    #[test]
    fn model_false_clears_even_when_default_model_is_present() {
        // pi `agent-overrides.test.ts:72/84`: `model: false` -> undefined, defeating defaultModel.
        let mut merged = HashMap::new();
        merged.insert("reviewer".to_string(), agent("reviewer", AgentSource::Builtin, "/b/reviewer.md"));
        let settings = user_scope(SubagentSettings {
            overrides: settings_with_override(
                "reviewer",
                AgentOverrideConfig {
                    model: OverrideField::ExplicitClear,
                    ..Default::default()
                },
            )
            .overrides,
            default_model: Some("deepseek-v4-flash".to_string()),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("reviewer").expect("present").model, None);
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-012: subagents.disableBuiltins bulk-disables builtins only
    // -----------------------------------------------------------------------------------------

    #[test]
    fn disable_builtins_disables_builtins_but_not_custom_agents() {
        let mut merged = HashMap::new();
        merged.insert("reviewer".to_string(), agent("reviewer", AgentSource::Builtin, "/b/reviewer.md"));
        merged.insert("implementer".to_string(), agent("implementer", AgentSource::Project, "/p/impl.md"));

        let settings = user_scope(SubagentSettings {
            disable_builtins: Some(true),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("reviewer").expect("present").disabled, Some(true));
        assert_ne!(
            merged.get("implementer").expect("present").disabled,
            Some(true),
            "disableBuiltins must not disable custom agents"
        );
    }

    #[test]
    fn disable_builtins_false_or_absent_leaves_builtins_enabled() {
        for flag in [None, Some(false)] {
            let mut merged = HashMap::new();
            merged.insert("reviewer".to_string(), agent("reviewer", AgentSource::Builtin, "/b/reviewer.md"));
            let settings = user_scope(SubagentSettings {
                disable_builtins: flag,
                ..Default::default()
            });
            apply_overrides(&mut merged, &settings).expect("apply succeeds");
            assert_ne!(
                merged.get("reviewer").expect("present").disabled,
                Some(true),
                "disableBuiltins={flag:?} must not disable"
            );
        }
    }

    #[test]
    fn project_disable_builtins_false_re_enables_a_user_true() {
        // pi (agents.ts:793): userBulkDisabled only when the project scope said NOTHING about
        // disableBuiltins — a project `false` re-enables what a user `true` disabled.
        let mut merged = HashMap::new();
        merged.insert(
            "reviewer".to_string(),
            agent("reviewer", AgentSource::Builtin, "/b/reviewer.md"),
        );
        let settings = two_scope(
            SubagentSettings {
                disable_builtins: Some(true),
                ..Default::default()
            },
            SubagentSettings {
                disable_builtins: Some(false),
                ..Default::default()
            },
        );
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_ne!(
            merged.get("reviewer").expect("present").disabled,
            Some(true),
            "a project disableBuiltins:false must re-enable a user-disabled builtin"
        );
    }

    #[test]
    fn disable_builtins_skips_a_builtin_carrying_a_per_agent_override() {
        // A per-agent override takes precedence over the bulk-disable branch (pi checks it first),
        // so the builtin stays enabled and the override's own fields apply.
        let mut merged = HashMap::new();
        merged.insert("reviewer".to_string(), agent("reviewer", AgentSource::Builtin, "/b/reviewer.md"));
        let settings = user_scope(SubagentSettings {
            overrides: settings_with_override(
                "reviewer",
                AgentOverrideConfig {
                    model: OverrideField::Value("openai/gpt-5".to_string()),
                    ..Default::default()
                },
            )
            .overrides,
            disable_builtins: Some(true),
            ..Default::default()
        });
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let reviewer = merged.get("reviewer").expect("present");
        assert_ne!(reviewer.disabled, Some(true), "override beats bulk-disable");
        assert_eq!(reviewer.model, Some("openai/gpt-5".into()));
    }

    // -----------------------------------------------------------------------------------------
    // Full end-to-end: discover_and_merge wraps merge_tiers + apply_overrides
    // -----------------------------------------------------------------------------------------

    #[test]
    fn discover_and_merge_end_to_end_applies_precedence_then_overrides() {
        let tiers = TieredAgents {
            builtin: vec![agent("delegate", AgentSource::Builtin, "/builtin/delegate.md")],
            package: vec![],
            user: vec![],
            project: vec![agent_with_present(
                "reviewer",
                AgentSource::Project,
                "/proj/reviewer.md",
                &[],
            )],
        };
        // The project-agent `reviewer` override lives in the project scope; the builtin `delegate`
        // disable lives in the user scope — exercising both branches with their true provenance.
        let settings = two_scope(
            settings_with_override(
                "delegate",
                AgentOverrideConfig {
                    disabled: OverrideField::Value(true),
                    ..Default::default()
                },
            ),
            settings_with_override(
                "reviewer",
                AgentOverrideConfig {
                    model: OverrideField::Value("openai/gpt-5".to_string()),
                    ..Default::default()
                },
            ),
        );

        let merged = discover_and_merge(tiers, &settings).expect("merge succeeds");
        assert_eq!(merged.len(), 2);
        let reviewer = merged.get("reviewer").expect("present");
        assert_eq!(reviewer.model, Some("openai/gpt-5".into()));
        assert_eq!(
            reviewer.override_info.as_ref().expect("recorded").scope,
            OverrideScope::Project
        );
        let delegate = merged.get("delegate").expect("present");
        assert_eq!(delegate.disabled, Some(true));
        assert_eq!(
            delegate.override_info.as_ref().expect("recorded").scope,
            OverrideScope::User
        );
    }

    // Sanity: OutputSpec import is exercised elsewhere in the crate's own types tests; referenced
    // here only to keep this test module's imports honest against unused-import lint drift if a
    // future edit removes the last direct use above.
    #[test]
    fn output_spec_type_is_reachable_from_this_module() {
        let _ = OutputSpec {
            path: None,
            mode: None,
        };
    }


    /// SUBA-092 — the builtin arm (`agents.ts:1404-1405` @v0.64.0): an override's `excludeTools`
    /// replaces the agent's own unconditionally, a JSON `false` deletes it (`None`, never `[]`),
    /// and `allowNestedSubagents` is a plain boolean assignment.
    #[test]
    fn builtin_override_applies_and_clears_exclude_tools_and_allow_nested_subagents() {
        let mut merged = HashMap::new();
        let mut a = agent("delegate", AgentSource::Builtin, "/builtin/delegate.md");
        a.exclude_tools = Some(vec!["write".to_string()]);
        a.present_fields.insert("excludeTools".to_string());
        a.allow_nested_subagents = Some(false);
        a.present_fields.insert("allowNestedSubagents".to_string());
        merged.insert("delegate".to_string(), a);

        let settings = user_scope(settings_with_override(
            "delegate",
            AgentOverrideConfig {
                exclude_tools: OverrideField::Value(vec!["bash".to_string()]),
                allow_nested_subagents: OverrideField::Value(true),
                ..Default::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("delegate").expect("present");
        assert_eq!(
            updated.exclude_tools,
            Some(vec!["bash".to_string()]),
            "builtin branch overwrites even a field present on disk"
        );
        assert_eq!(updated.allow_nested_subagents, Some(true));
        assert!(updated.override_info.is_some());

        // `excludeTools: false` -> `delete next.excludeTools`.
        let settings = user_scope(settings_with_override(
            "delegate",
            AgentOverrideConfig {
                exclude_tools: OverrideField::ExplicitClear,
                ..Default::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("delegate").expect("present").exclude_tools, None);
    }

    /// SUBA-092 — the custom arm (`agents.ts:1547-1552` @v0.62.0, `fill(...)`): both fields obey
    /// the frontmatter-presence gate every other custom fill obeys — an `excludeTools:` or
    /// `allowNestedSubagents:` key present on disk blocks the override for THAT field only.
    #[test]
    fn custom_override_fills_exclude_tools_and_allow_nested_subagents_only_when_absent_on_disk() {
        let mut merged = HashMap::new();
        let mut a = agent_with_present(
            "reviewer",
            AgentSource::Project,
            "/proj/reviewer.md",
            &["excludeTools"],
        );
        a.exclude_tools = Some(vec!["write".to_string()]);
        merged.insert("reviewer".to_string(), a);

        let settings = project_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                exclude_tools: OverrideField::Value(vec!["bash".to_string()]),
                allow_nested_subagents: OverrideField::Value(true),
                ..Default::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(
            updated.exclude_tools,
            Some(vec!["write".to_string()]),
            "present-on-disk excludeTools must block the override"
        );
        assert_eq!(
            updated.allow_nested_subagents,
            Some(true),
            "absent-on-disk allowNestedSubagents must accept the override"
        );
        assert!(updated.override_info.is_some(), "one field applied, so provenance is recorded");

        // Neither key on disk: both fill, and a `false` clear on excludeTools yields `None`.
        let mut merged = HashMap::new();
        let mut a = agent("reviewer", AgentSource::User, "/user/reviewer.md");
        a.exclude_tools = Some(vec!["write".to_string()]);
        merged.insert("reviewer".to_string(), a);
        let settings = user_scope(settings_with_override(
            "reviewer",
            AgentOverrideConfig {
                exclude_tools: OverrideField::ExplicitClear,
                allow_nested_subagents: OverrideField::Value(false),
                ..Default::default()
            },
        ));
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(updated.exclude_tools, None);
        assert_eq!(updated.allow_nested_subagents, Some(false));
    }
}
