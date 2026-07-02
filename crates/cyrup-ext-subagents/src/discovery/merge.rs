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

use super::types::{
    AgentDefinition, AgentOverrideConfig, AgentOverrideInfo, AgentSource, OverrideField,
    OverrideScope, SubagentSettings,
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
    settings: &SubagentSettings,
) -> Result<HashMap<String, AgentDefinition>, SubagentError> {
    let mut merged = merge_tiers(tiers);
    apply_overrides(&mut merged, settings)?;
    Ok(merged)
}

// -------------------------------------------------------------------------------------------
// Override application (R-SA-010/011/012, §6.2.1)
// -------------------------------------------------------------------------------------------

/// Apply `subagents.overrides.<name>` settings-based overrides to an already-merged agent map, in
/// place (arch-SA §6.2.1). Three passes, strictly ordered:
///
/// 1. **Builtin branch (R-SA-010)**: for each [`AgentSource::Builtin`] entry with a matching
///    override, unconditionally overwrite every field the override delta actually states
///    (`Value`/`ExplicitClear`; `Unset` fields are left untouched) — full-replace semantics,
///    ignoring `present_fields` entirely (a builtin agent's frontmatter is not user-authored, so
///    "field already present on disk" carries no fill-unset-only meaning for it).
/// 2. **Custom branch (R-SA-010)**: for each [`AgentSource::User`]/[`AgentSource::Project`] entry
///    with a matching override, apply a field from the delta **only when that field was absent
///    from the agent's own on-disk frontmatter** (`!present_fields.contains(field_name)`) —
///    fill-unset-only semantics. An explicitly-present field on disk blocks the override for that
///    field regardless of the override's own value, even if the two happen to agree.
/// 3. **Global `disableThinking` pass (R-SA-012)**, run last, applied to every entry regardless of
///    source: project-scope `disableThinking` wins over user-scope, and is skipped per-agent if
///    that same agent's own override (in the *same* scope that would otherwise apply
///    `disableThinking`) already explicitly set `thinking` — an override that names a concrete
///    thinking level for an agent must not then be silently clobbered by the blanket
///    `disableThinking` knob from that same scope.
///
/// Per R-SA-012, "project override checked before user override; only one is ever applied" governs
/// per-agent *override delta* selection: [`SubagentSettings`] as modeled here carries a single
/// flattened `overrides` map (already the caller-resolved, single winning scope per agent name —
/// `discovery/mod.rs`'s settings-layering responsibility, R-SA-133, not this function's), so this
/// function's own scope-precedence responsibility is limited to the `disableThinking` pass, which
/// is the one field in [`SubagentSettings`] that is NOT itself agent-scoped and therefore
/// genuinely needs its own explicit project-over-user resolution here.
pub fn apply_overrides(
    merged: &mut HashMap<String, AgentDefinition>,
    settings: &SubagentSettings,
) -> Result<(), SubagentError> {
    // Pass 1 + 2: per-agent `subagents.overrides.<name>` deltas.
    for (name, delta) in &settings.overrides {
        let Some(agent) = merged.get_mut(name) else {
            // An override targeting an agent name that discovery never found is not itself a
            // malformed-settings condition (R-SA-009 reserves that abort for a malformed *shape*
            // of the overrides map, not for a dangling name reference) — silently no-op.
            continue;
        };
        match agent.source {
            AgentSource::Builtin => apply_builtin_override(agent, delta),
            AgentSource::User | AgentSource::Project => apply_custom_override(agent, delta),
            // Package-sourced agents are not exposed for settings-based override in
            // pi-subagents' own source contract (only Builtin full-replace and User/Project
            // fill-unset-only are specified, R-SA-010) — left untouched.
            AgentSource::Package => {}
        }
    }

    // Pass 3: global disableThinking, project-scope wins over user-scope (R-SA-012), run last so
    // it observes any `thinking` value pass 1/2 may have just set.
    apply_disable_thinking_pass(merged, settings);

    Ok(())
}

/// R-SA-010's builtin branch: unconditionally overwrite every field the delta actually states.
/// `Unset` fields are left alone; `ExplicitClear` resets a field to its type's "absent" value;
/// `Value(v)` replaces it with `v`. Also records [`AgentOverrideInfo`] provenance (`scope` is
/// deliberately not distinguished as User vs. Project *for the builtin branch* — a builtin
/// override's originating settings scope is orthogonal to R-SA-010's full-replace semantics, and
/// `discovery/mod.rs` is responsible for supplying `settings.overrides` already resolved to the
/// single winning scope, R-SA-133 — so `OverrideScope::Project` is used here as a fixed sentinel
/// meaning "settings-resolved," not a claim about which literal scope file the delta came from).
fn apply_builtin_override(agent: &mut AgentDefinition, delta: &AgentOverrideConfig) {
    if delta.is_empty() {
        return;
    }
    let base_snapshot = Box::new(agent.clone());

    apply_field_full_replace(&mut agent.model, &delta.model, None, |v| {
        Some(cyrup_core::ModelId::from(v.clone()))
    });
    apply_field_full_replace(
        &mut agent.fallback_models,
        &delta.fallback_models,
        Vec::new(),
        |v| v.iter().cloned().map(cyrup_core::ModelId::from).collect(),
    );
    apply_field_full_replace(&mut agent.thinking, &delta.thinking, None, |v| Some(*v));
    apply_field_full_replace(&mut agent.tools, &delta.tools, None, |v| Some(v.clone()));
    apply_field_full_replace(
        &mut agent.system_prompt_mode,
        &delta.system_prompt_mode,
        crate::discovery::types::SystemPromptMode::Replace,
        |v| *v,
    );
    apply_field_full_replace(&mut agent.disabled, &delta.disabled, None, |v| Some(*v));
    apply_field_full_replace(
        &mut agent.max_subagent_depth,
        &delta.max_subagent_depth,
        None,
        |v| Some(*v),
    );
    apply_field_full_replace(
        &mut agent.completion_guard,
        &delta.completion_guard,
        None,
        |v| Some(*v),
    );

    agent.override_info = Some(AgentOverrideInfo {
        scope: OverrideScope::Project,
        settings_path: agent.file_path.clone(),
        base_snapshot,
    });
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

/// R-SA-010's custom (User/Project) branch: fill-unset-only. A field from the delta is applied
/// **only when that field's key was absent from the agent's own on-disk frontmatter**
/// (`!agent.present_fields.contains(field_name)`) — an explicitly-present field on disk blocks the
/// override for that field unconditionally, regardless of the override delta's own value (even an
/// `ExplicitClear` is blocked by presence: presence itself, not the override's intent, is the
/// gate).
fn apply_custom_override(agent: &mut AgentDefinition, delta: &AgentOverrideConfig) {
    if delta.is_empty() {
        return;
    }
    let base_snapshot = Box::new(agent.clone());
    let mut applied_any = false;

    applied_any |= apply_field_fill_unset(
        &mut agent.model,
        "model",
        &agent.present_fields,
        &delta.model,
        None,
        |v| Some(cyrup_core::ModelId::from(v.clone())),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.fallback_models,
        "fallbackModels",
        &agent.present_fields,
        &delta.fallback_models,
        Vec::new(),
        |v| v.iter().cloned().map(cyrup_core::ModelId::from).collect(),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.thinking,
        "thinking",
        &agent.present_fields,
        &delta.thinking,
        None,
        |v| Some(*v),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.tools,
        "tools",
        &agent.present_fields,
        &delta.tools,
        None,
        |v| Some(v.clone()),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.system_prompt_mode,
        "systemPromptMode",
        &agent.present_fields,
        &delta.system_prompt_mode,
        crate::discovery::types::SystemPromptMode::Replace,
        |v| *v,
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.disabled,
        "disabled",
        &agent.present_fields,
        &delta.disabled,
        None,
        |v| Some(*v),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.max_subagent_depth,
        "maxSubagentDepth",
        &agent.present_fields,
        &delta.max_subagent_depth,
        None,
        |v| Some(*v),
    );
    applied_any |= apply_field_fill_unset(
        &mut agent.completion_guard,
        "completionGuard",
        &agent.present_fields,
        &delta.completion_guard,
        None,
        |v| Some(*v),
    );

    if applied_any {
        let scope = match agent.source {
            AgentSource::Project => OverrideScope::Project,
            _ => OverrideScope::User,
        };
        agent.override_info = Some(AgentOverrideInfo {
            scope,
            settings_path: agent.file_path.clone(),
            base_snapshot,
        });
    }
}

/// One field's fill-unset-only application (R-SA-010 custom branch). Returns `true` iff the field
/// was actually applied (i.e. it was both present-in-delta and absent-from-disk) — used by the
/// caller to decide whether [`AgentOverrideInfo`] provenance should be recorded at all (R-SA-010's
/// data model note: "present only when at least one override field actually applied"). Takes
/// `clear_value` by parameter (mirroring [`apply_field_full_replace`]'s own signature/rationale)
/// rather than an `F: Default` bound, so this helper works uniformly across `Option<_>`/`Vec<_>`
/// fields AND plain-enum fields like `SystemPromptMode` that have no `Default` impl of their own.
fn apply_field_fill_unset<T, F>(
    target: &mut F,
    field_name: &str,
    present_fields: &std::collections::HashSet<String>,
    delta: &OverrideField<T>,
    clear_value: F,
    to_target: impl FnOnce(&T) -> F,
) -> bool {
    if present_fields.contains(field_name) {
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

/// R-SA-012: project-scope `subagents.disableThinking` wins over user-scope, applied after
/// per-agent overrides (pass 1/2 above already ran), and skipped per-agent when that agent's own
/// override already explicitly set `thinking` in the scope that would otherwise supply
/// `disableThinking` — an override that names a concrete thinking level must not be silently
/// clobbered by the blanket knob from the same scope.
///
/// [`SubagentSettings`] as modeled in this crate carries one flattened `disable_thinking: Option<
/// bool>` (already scope-resolved by `discovery/mod.rs`'s settings-layering step, R-SA-133 — the
/// project-vs-user precedence for *which value wins* is that caller's responsibility, mirroring
/// how `overrides` is already a single flattened map rather than two parallel per-scope maps).
/// This function's own remaining responsibility is exactly the "skipped per-agent if that agent's
/// own override already set `thinking`" carve-out, which needs the per-agent `overrides` map to
/// evaluate — the project-over-user scope selection for the *blanket* flag itself is encoded by
/// `discovery/mod.rs` resolving `settings.disable_thinking` from the correct scope before calling
/// this function at all.
fn apply_disable_thinking_pass(
    merged: &mut HashMap<String, AgentDefinition>,
    settings: &SubagentSettings,
) {
    let Some(true) = settings.disable_thinking else {
        return;
    };
    for (name, agent) in merged.iter_mut() {
        let already_set_thinking_via_override = settings
            .overrides
            .get(name)
            .is_some_and(|delta| delta.thinking.is_present());
        if already_set_thinking_via_override {
            continue;
        }
        agent.thinking = None;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use cyrup_core::ThinkingLevel;

    use super::*;
    use crate::discovery::types::{OutputSpec, SystemPromptMode, ToolRef};

    fn agent(name: &str, source: AgentSource, file_path: &str) -> AgentDefinition {
        AgentDefinition {
            name: name.to_string(),
            local_name: name.to_string(),
            package_name: None,
            description: format!("{name} description"),
            tools: None,
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
        a.thinking = Some(ThinkingLevel::Low);
        a.present_fields.insert("thinking".to_string());
        merged.insert("delegate".to_string(), a);

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "delegate".to_string(),
            AgentOverrideConfig {
                thinking: OverrideField::Value(ThinkingLevel::High),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("delegate").expect("present");
        assert_eq!(
            updated.thinking,
            Some(ThinkingLevel::High),
            "builtin branch overwrites even a field present on disk"
        );
        assert!(updated.override_info.is_some());
    }

    #[test]
    fn builtin_override_explicit_clear_resets_to_default() {
        let mut merged = HashMap::new();
        let mut a = agent("delegate", AgentSource::Builtin, "/builtin/delegate.md");
        a.disabled = Some(true);
        merged.insert("delegate".to_string(), a);

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "delegate".to_string(),
            AgentOverrideConfig {
                disabled: OverrideField::ExplicitClear,
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("delegate").expect("present").disabled, None);
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

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "reviewer".to_string(),
            AgentOverrideConfig {
                model: OverrideField::Value("openai/gpt-5".to_string()),
                thinking: OverrideField::Value(ThinkingLevel::High),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };

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
            Some(ThinkingLevel::High),
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

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "reviewer".to_string(),
            AgentOverrideConfig {
                completion_guard: OverrideField::ExplicitClear,
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };

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
        let settings = SubagentSettings::default();
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
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "pkgagent".to_string(),
            AgentOverrideConfig {
                thinking: OverrideField::Value(ThinkingLevel::High),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };
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
        let mut overrides = BTreeMap::new();
        overrides.insert("reviewer".to_string(), AgentOverrideConfig::default());
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert!(merged.get("reviewer").expect("present").override_info.is_none());
    }

    #[test]
    fn override_targeting_unknown_agent_name_is_silently_ignored() {
        let mut merged: HashMap<String, AgentDefinition> = HashMap::new();
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "ghost".to_string(),
            AgentOverrideConfig {
                thinking: OverrideField::Value(ThinkingLevel::High),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };
        // Must not error and must not panic.
        apply_overrides(&mut merged, &settings).expect("apply succeeds even with a dangling name");
        assert!(merged.is_empty());
    }

    #[test]
    fn custom_override_full_field_coverage_applies_every_absent_field() {
        let mut merged = HashMap::new();
        merged.insert(
            "reviewer".to_string(),
            agent("reviewer", AgentSource::Project, "/proj/reviewer.md"),
        );
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "reviewer".to_string(),
            AgentOverrideConfig {
                model: OverrideField::Value("openai/gpt-5".to_string()),
                fallback_models: OverrideField::Value(vec!["anthropic/claude-sonnet-4".to_string()]),
                thinking: OverrideField::Value(ThinkingLevel::Medium),
                tools: OverrideField::Value(vec![ToolRef::Builtin("read".to_string())]),
                system_prompt_mode: OverrideField::Value(SystemPromptMode::Append),
                disabled: OverrideField::Value(true),
                max_subagent_depth: OverrideField::Value(3),
                completion_guard: OverrideField::Value(false),
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        let updated = merged.get("reviewer").expect("present");
        assert_eq!(updated.model, Some("openai/gpt-5".into()));
        assert_eq!(updated.fallback_models, vec!["anthropic/claude-sonnet-4".into()]);
        assert_eq!(updated.thinking, Some(ThinkingLevel::Medium));
        assert_eq!(updated.tools, Some(vec![ToolRef::Builtin("read".to_string())]));
        assert_eq!(updated.system_prompt_mode, SystemPromptMode::Append);
        assert_eq!(updated.disabled, Some(true));
        assert_eq!(updated.max_subagent_depth, Some(3));
        assert_eq!(updated.completion_guard, Some(false));
        assert!(updated.override_info.is_some());
    }

    // -----------------------------------------------------------------------------------------
    // R-SA-012: project disableThinking wins over user; skip-if-override-set-thinking carve-out
    // -----------------------------------------------------------------------------------------

    #[test]
    fn disable_thinking_pass_clears_thinking_when_flag_set() {
        let mut merged = HashMap::new();
        let mut a = agent("worker", AgentSource::Project, "/proj/worker.md");
        a.thinking = Some(ThinkingLevel::High);
        merged.insert("worker".to_string(), a);

        let settings = SubagentSettings {
            disable_thinking: Some(true),
            ..Default::default()
        };
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("worker").expect("present").thinking, None);
    }

    #[test]
    fn disable_thinking_pass_is_no_op_when_flag_absent_or_false() {
        let mut merged = HashMap::new();
        let mut a = agent("worker", AgentSource::Project, "/proj/worker.md");
        a.thinking = Some(ThinkingLevel::High);
        merged.insert("worker".to_string(), a);

        let settings = SubagentSettings::default();
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("worker").expect("present").thinking, Some(ThinkingLevel::High));

        let mut merged2 = HashMap::new();
        let mut a2 = agent("worker2", AgentSource::Project, "/proj/worker2.md");
        a2.thinking = Some(ThinkingLevel::High);
        merged2.insert("worker2".to_string(), a2);
        let settings_false = SubagentSettings {
            disable_thinking: Some(false),
            ..Default::default()
        };
        apply_overrides(&mut merged2, &settings_false).expect("apply succeeds");
        assert_eq!(
            merged2.get("worker2").expect("present").thinking,
            Some(ThinkingLevel::High)
        );
    }

    #[test]
    fn disable_thinking_pass_skips_agent_whose_own_override_already_set_thinking() {
        let mut merged = HashMap::new();
        // Absent on disk, so the per-agent override applies first (pass 1/2), setting a concrete
        // thinking level; the subsequent disableThinking pass must then leave it alone.
        merged.insert(
            "worker".to_string(),
            agent("worker", AgentSource::Project, "/proj/worker.md"),
        );

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "worker".to_string(),
            AgentOverrideConfig {
                thinking: OverrideField::Value(ThinkingLevel::Xhigh),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            disable_thinking: Some(true),
            ..Default::default()
        };

        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(
            merged.get("worker").expect("present").thinking,
            Some(ThinkingLevel::Xhigh),
            "an override that explicitly set thinking must survive the disableThinking pass"
        );
    }

    #[test]
    fn disable_thinking_pass_applies_to_agents_with_no_override_entry_at_all() {
        let mut merged = HashMap::new();
        let mut a = agent("bystander", AgentSource::User, "/user/bystander.md");
        a.thinking = Some(ThinkingLevel::Low);
        merged.insert("bystander".to_string(), a);

        let settings = SubagentSettings {
            disable_thinking: Some(true),
            ..Default::default()
        };
        apply_overrides(&mut merged, &settings).expect("apply succeeds");
        assert_eq!(merged.get("bystander").expect("present").thinking, None);
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
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "reviewer".to_string(),
            AgentOverrideConfig {
                model: OverrideField::Value("openai/gpt-5".to_string()),
                ..Default::default()
            },
        );
        overrides.insert(
            "delegate".to_string(),
            AgentOverrideConfig {
                disabled: OverrideField::Value(true),
                ..Default::default()
            },
        );
        let settings = SubagentSettings {
            overrides,
            ..Default::default()
        };

        let merged = discover_and_merge(tiers, &settings).expect("merge succeeds");
        assert_eq!(merged.len(), 2);
        assert_eq!(merged.get("reviewer").expect("present").model, Some("openai/gpt-5".into()));
        assert_eq!(merged.get("delegate").expect("present").disabled, Some(true));
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
}
