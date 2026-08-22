//! Model resolution against the provider registry: candidate lookup, override resolution and
//! provenance formatting for the `models` report.

pub(crate) mod classify;
pub(crate) mod probe;

use crate::discovery::types::{
    AgentDefinition, AgentModelSourceInfo, LayeredOverrideSettings, OverrideScope,
};

/// pi's `INHERIT_MODEL` sentinel (`runs/shared/model-fallback.ts:22`): a persona's `model` set to
/// the literal string `"inherit"` requests the parent session's model exactly as if `model` were
/// unset — it is NOT a real model id to resolve against the catalog or print verbatim.
///
/// Re-exported from its canonical owner, [`crate::exec::fallback`], which is where the LAUNCH path
/// applies it (this module's use is the `models` report formatter).
use crate::exec::fallback::INHERIT_MODEL_SENTINEL;

/// pi `splitThinkingSuffix` (`runs/shared/model-fallback.ts:13-19`): split a model string on its
/// LAST `:`, isolating a trailing thinking-level suffix (`:high`, `:off`, ...) from the base model
/// id. No suffix present -> empty suffix, base = the whole string.
fn split_thinking_suffix(model: &str) -> (&str, &str) {
    match model.rfind(':') {
        Some(idx) => (model.get(..idx).unwrap_or(model), model.get(idx..).unwrap_or("")),
        None => (model, ""),
    }
}

/// One entry of pi's `ctx.modelRegistry.getAvailable()` (`shared/model-info.ts`'s `ModelInfo`),
/// reduced to the three fields `resolve_model_candidate` consults.
pub(crate) struct AvailableModelEntry {
    provider: String,
    id: String,
    full_id: String,
}

/// pi's `ctx.modelRegistry.getAvailable()` (`profiles.ts:529`, `agent-management.ts:169`) — the
/// model registry every model-facing subagents command consults — bound here to the REAL built-in
/// provider registry, [`cyrup_provider::catalog::builtin_catalog`], i.e. every model every
/// registered provider ships.
///
/// [CYRUP-DELTA] pi's `getAvailable()` additionally filters to providers whose auth is configured
/// (`ai/src/models.ts:522-542` @pi v0.84.1), so this is the credential-BLIND registry:
/// `getModels()`, pi's "complete synchronous catalog" (`ai/src/models.ts:131,164` @pi v0.84.1). That
/// is a strictly wider list than pi's, never a narrower one, so no model pi would offer is hidden
/// here.
///
/// **The reason recorded here was stale and is corrected (PROV-041).** It previously read
/// "`cyrup-provider` has no `checkAuth`/`getAvailable` port yet (PROV-003 — cyrup ships no login
/// flow at all)". Both halves are false at HEAD: `Models::check_auth`
/// (`cyrup-provider/src/collection.rs:367`), `Models::get_available` (`:408`), `Models::login`
/// (`:426`) and `Models::logout` (`:474`) all exist (PROV-003, PROV-031 and PROV-032 are closed).
/// What remains is a WIRING gap, not a missing port: this accessor is a free function over the
/// static [`cyrup_provider::catalog::builtin_catalog`] and holds no `Models` instance — hence no
/// credential store to filter against. Reaching pi's `getAvailable()` here means giving this call
/// site the session's `Models`, which is a change to this crate's model-registry seam, not another
/// port of cyrup-provider.
pub(crate) fn registry_models() -> &'static [cyrup_provider::Model] {
    cyrup_provider::catalog::builtin_catalog()
}

/// [`registry_models`] projected onto the three fields `resolve_model_candidate` consults.
pub(crate) fn registry_available_models() -> Vec<AvailableModelEntry> {
    registry_models()
        .iter()
        .map(|m| AvailableModelEntry {
            provider: m.provider.as_str().to_string(),
            id: m.id.as_str().to_string(),
            full_id: format!("{}/{}", m.provider.as_str(), m.id.as_str()),
        })
        .collect()
}

/// pi `resolveModelCandidate` (`runs/shared/model-fallback.ts:148-164`): resolve a bare or
/// fully-qualified model string against the available-model list. A `provider/id` string passes
/// through unchanged; a bare id resolves to its `fullId` when exactly one available model matches
/// (or, when multiple providers offer the same bare id, the `preferred_provider`'s match wins); an
/// unmatched bare id (no available models, or an ambiguous match with no preferred-provider hit)
/// passes through unchanged — pi's fallback `return model`.
fn resolve_model_candidate(
    model: Option<&str>,
    available: &[AvailableModelEntry],
    preferred_provider: Option<&str>,
) -> Option<String> {
    let model = model?;
    if model.is_empty() {
        return None;
    }
    if model.contains('/') {
        return Some(model.to_string());
    }
    if available.is_empty() {
        return Some(model.to_string());
    }
    let (base_model, thinking_suffix) = split_thinking_suffix(model);
    let matches: Vec<&AvailableModelEntry> =
        available.iter().filter(|entry| entry.id == base_model).collect();
    if let Some(preferred) = preferred_provider
        && let Some(m) = matches.iter().find(|entry| entry.provider == preferred)
    {
        return Some(format!("{}{thinking_suffix}", m.full_id));
    }
    if matches.len() != 1 {
        return Some(model.to_string());
    }
    let only = matches.into_iter().next()?;
    Some(format!("{}{thinking_suffix}", only.full_id))
}

/// pi `resolveSubagentModelOverride` (`runs/shared/model-fallback.ts:196-220`): the effective model a
/// discovered builtin persona resolves to. `requested_model` unset, empty, or the `"inherit"`
/// sentinel all resolve to the live parent session model (`provider/id`) when one is bound, else
/// `None` (pi's "(unresolved)" case); any other explicit value is resolved via
/// [`resolve_model_candidate`].
pub(crate) fn resolve_subagent_model_override(
    requested_model: Option<&str>,
    parent_model: Option<(&str, &str)>,
    available: &[AvailableModelEntry],
    preferred_provider: Option<&str>,
) -> Option<String> {
    let trimmed = requested_model.map(str::trim).unwrap_or("");
    let explicit = (!trimmed.is_empty() && trimmed != INHERIT_MODEL_SENTINEL).then_some(trimmed);
    match explicit {
        None => parent_model.map(|(provider, id)| format!("{provider}/{id}")),
        Some(explicit) => resolve_model_candidate(Some(explicit), available, preferred_provider),
    }
}

/// pi `resolveSubagentDefaultModel` (`agents.ts:921-933`): which scope's `subagents.defaultModel`
/// wins (project beats user when the project scope exists and declares one). `merge.rs`'s
/// `apply_default_model` already guarantees that whenever an agent's `model_source` is still
/// [`AgentModelSourceInfo::SettingsDefault`], `model` equals exactly the value this same
/// precedence resolves to (any override that changes `model` also resets `model_source` away from
/// `SettingsDefault`) — so [`format_model_source`] only needs the WINNING SCOPE from here, not the
/// value itself, to render pi's scope-qualified `"{scope} defaultModel"` provenance.
pub(crate) fn resolve_default_model_scope(settings: &LayeredOverrideSettings) -> Option<&'static str> {
    if settings.project_settings_path.is_some() && settings.project.default_model.is_some() {
        return Some("project");
    }
    if settings.user.default_model.is_some() {
        return Some("user");
    }
    None
}

/// Provenance of a builtin persona's resolved model (pi `formatModelSource`,
/// agent-management.ts:787-800). `default_model_scope` is [`resolve_default_model_scope`]'s
/// result for the current discovery run.
pub(crate) fn format_model_source(
    agent: &AgentDefinition,
    current_session_model: Option<&str>,
    default_model_scope: Option<&str>,
) -> String {
    // pi `agent.override && agent.model !== agent.override.base.model` (agent-management.ts:788-790):
    // the override branch fires only when the override actually changed the resolved model, not
    // merely because an override happens to be recorded (e.g. it only touched `disabled`/`tools`).
    if let Some(override_info) = &agent.override_info
        && agent.model != override_info.base_snapshot.model
    {
        let scope = match override_info.scope {
            OverrideScope::User => "user",
            OverrideScope::Project => "project",
        };
        return format!("{scope} override");
    }
    // pi `agent.modelSource?.type === "subagents.defaultModel" && agent.model === agent.modelSource.model`
    // (agent-management.ts:791-793): scope-qualified provenance, gated on the model still matching
    // what the default actually supplied (see this function's doc for why the value check is
    // redundant here and the scope alone suffices).
    if agent.model_source == Some(AgentModelSourceInfo::SettingsDefault)
        && let Some(scope) = default_model_scope
    {
        return format!("{scope} defaultModel");
    }
    if agent.model.is_some() {
        return "builtin agent config".to_string();
    }
    if current_session_model.is_some() {
        return "inherits current session model".to_string();
    }
    "inherit requested, but no current session model is available".to_string()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;
    use crate::extension::host::SubagentsExtension;

    /// [`SubagentsExtension::provider_ranked_full_ids_from_catalog`] must drop a probed-unavailable
    /// model entirely (pi `catalogModelIsUsable`) rather than still ranking it — proven against the
    /// REAL model registry so this exercises the actual registry cross-reference lookup, not just
    /// a synthetic fixture.
    #[test]
    fn provider_ranked_full_ids_from_catalog_drops_unusable_probe_results() {
        let anthropic_model = registry_models()
            .iter()
            .find(|m| m.provider.as_str() == "anthropic")
            .expect("the registry must carry at least one anthropic model for this test");
        let full_id = format!("anthropic/{}", anthropic_model.id.as_str());

        let usable_catalog = crate::registration::profiles::ProviderModelCatalog {
            provider: "anthropic".to_string(),
            refreshed_at_epoch_ms: 0,
            max_age_days: 7,
            sources: vec![],
            models: vec![crate::registration::profiles::ProviderCatalogModel {
                id: anthropic_model.id.as_str().to_string(),
                full_id: full_id.clone(),
                profile_rank: 10,
                probe_status: "ok".to_string(),
            }],
        };
        let ranked =
            SubagentsExtension::provider_ranked_full_ids_from_catalog("anthropic", &usable_catalog);
        assert_eq!(ranked, vec![full_id.clone()]);

        let unusable_catalog = crate::registration::profiles::ProviderModelCatalog {
            models: vec![crate::registration::profiles::ProviderCatalogModel {
                probe_status: "unavailable".to_string(),
                ..usable_catalog.models.first().expect("one model").clone()
            }],
            ..usable_catalog
        };
        let ranked_after_unavailable =
            SubagentsExtension::provider_ranked_full_ids_from_catalog("anthropic", &unusable_catalog);
        assert!(
            ranked_after_unavailable.is_empty(),
            "an unavailable-probe model must be filtered out of the ranked list entirely"
        );
    }

}
