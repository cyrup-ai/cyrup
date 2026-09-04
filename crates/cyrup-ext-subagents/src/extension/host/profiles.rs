//! The provider catalog cache and the generated model profiles the `/subagents-*` commands
//! read and refresh.

use std::path::{Path, PathBuf};

use crate::background::atomic::write_atomic_json;
use crate::error::SubagentError;
use crate::extension::host::SubagentsExtension;
use crate::extension::models::classify::{
    RankedCandidate, build_classification_context, classify_model, combined_cost, filter_dominated,
};
use crate::extension::models::probe::{probe_model, probe_status_is_usable};
use crate::extension::models::registry_models;

impl SubagentsExtension {
    // ---------------------------------------------------------------------------------------
    // /subagents-models, /subagents-refresh-provider-models, /subagents-generate-profiles,
    // /subagents-check-profile: cyrup-provider model-registry backed, with REAL live-probe
    // subprocess classification (pi `probeModel`/`classifyModel`, profiles.ts:250-335) — see
    // the free functions just above [`SubagentsExtension::provider_ranked_full_ids`] for the
    // ported probe/classification pipeline.
    // ---------------------------------------------------------------------------------------

    /// The path `registration/doctor.rs`'s `check_provider_catalog_freshness` (R-SA-131 item f)
    /// reads: refreshing/generating a provider catalog also touches this shared freshness marker so
    /// `/subagents-doctor`'s freshness check observes that a refresh genuinely ran.
    fn provider_catalog_cache_path(&self, cwd: &Path) -> PathBuf {
        let _ = cwd;
        self.home()
            .join(".cyrup")
            .join("subagents")
            .join("provider-catalog-cache.json")
    }

    /// A just-refreshed catalog's usable, RANKED, non-dominated `provider/id` full-id list — the
    /// pure, synchronous half of pi `generateProfilesForProvider`'s pipeline (profiles.ts:615-616:
    /// `catalog.models.filter(catalogModelIsUsable)` then `filterDominatedModels`, ordered by
    /// `derived.profileRank` ascending since [`SubagentsExtension::write_provider_catalog_file`] already sorted
    /// `catalog.models` that way — profiles.ts:398-400). Cross-references each catalog entry's
    /// `probe_status`/`profile_rank` (computed once, by the real live-probe pass that wrote
    /// `catalog`) against the model registry for the `cost`/`reasoning`/`context_window`/`max_tokens`
    /// axes `dominates` needs, so a caller never re-probes.
    pub(crate) fn provider_ranked_full_ids_from_catalog(
        provider: &str,
        catalog: &crate::registration::profiles::ProviderModelCatalog,
    ) -> Vec<String> {
        let registry = registry_models();
        let mut candidates: Vec<RankedCandidate> = Vec::new();
        for entry in &catalog.models {
            if !probe_status_is_usable(&entry.probe_status) {
                continue;
            }
            let Some(m) = registry
                .iter()
                .find(|sm| sm.provider.as_str() == provider && sm.id.as_str() == entry.id)
            else {
                continue;
            };
            candidates.push(RankedCandidate {
                full_id: entry.full_id.clone(),
                cost: combined_cost(&m.cost).unwrap_or(0.0),
                profile_rank: entry.profile_rank,
                reasoning: m.reasoning,
                context_window: m.context_window,
                max_tokens: m.max_tokens,
            });
        }
        let mut candidates = filter_dominated(candidates);
        candidates.sort_by(|a, b| {
            a.profile_rank
                .cmp(&b.profile_rank)
                .then_with(|| a.full_id.cmp(&b.full_id))
        });
        candidates.into_iter().map(|c| c.full_id).collect()
    }

    /// Build and persist a per-provider [`crate::registration::profiles::ProviderModelCatalog`]
    /// from the model registry ([`registry_models`], pi's `ctx.modelRegistry.getAvailable()`),
    /// REAL-probing every candidate model via [`probe_model`] and classifying it
    /// via [`classify_model`] (pi `refreshProviderModelCatalog`, profiles.ts:510-566), sorted by
    /// `profileRank` ascending then `fullId` (pi profiles.ts:542), plus refreshing the shared
    /// doctor freshness marker. Returns the model count.
    async fn write_provider_catalog_file(&self, provider: &str) -> Result<usize, SubagentError> {
        let matches: Vec<cyrup_provider::Model> = registry_models()
            .iter()
            .filter(|m| m.provider.as_str() == provider)
            .cloned()
            .collect();
        let ctx = build_classification_context(&matches);
        let mut models: Vec<crate::registration::profiles::ProviderCatalogModel> =
            Vec::with_capacity(matches.len());
        for m in &matches {
            let full_id = format!("{}/{}", m.provider.as_str(), m.id.as_str());
            let classification = classify_model(m, &ctx);
            let probe = probe_model(
                &full_id,
                self.executor()
                    .config_snapshot()
                    .await
                    .spawn_command
                    .as_ref(),
            )
            .await;
            models.push(crate::registration::profiles::ProviderCatalogModel {
                id: m.id.as_str().to_string(),
                full_id,
                profile_rank: classification.profile_rank,
                probe_status: probe.status.as_str().to_string(),
            });
        }
        // pi `models.sort((a,b) => a.derived.profileRank - b.derived.profileRank ||
        // a.fullId.localeCompare(b.fullId))`, profiles.ts:567.
        models.sort_by(|a, b| {
            a.profile_rank
                .cmp(&b.profile_rank)
                .then_with(|| a.full_id.cmp(&b.full_id))
        });
        let model_count = models.len();
        let file = crate::registration::profiles::ProviderModelCatalog {
            provider: provider.to_string(),
            refreshed_at_epoch_ms: u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0),
            max_age_days: crate::registration::profiles::DEFAULT_PROVIDER_MODELS_MAX_AGE_DAYS,
            // pi `sources` (profiles.ts:572): `["runtime-registry", ...(probe ? ["live-probe"] :
            // []), "heuristic-classifier"]` — this port always probes (no exposed `--no-probe`
            // slash-command flag, matching every real pi call site, which never passes
            // `probe: false` either).
            sources: vec![
                "runtime-registry".to_string(),
                "live-probe".to_string(),
                "heuristic-classifier".to_string(),
            ],
            models,
        };
        crate::registration::profiles::write_provider_catalog(&self.profiles_dir(), &file)?;

        // Also touch the shared freshness marker `registration/doctor.rs` stats (R-SA-131 item f).
        let cache_path = self.provider_catalog_cache_path(Path::new("."));
        if let Some(parent) = cache_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(SubagentError::Spawn)?;
        }
        let marker = serde_json::json!({
            "provider": provider,
            "modelCount": model_count,
            "refreshedAtEpochMs": file.refreshed_at_epoch_ms,
        });
        write_atomic_json(&cache_path, &marker)
            .await
            .map_err(SubagentError::Spawn)?;
        Ok(model_count)
    }

    /// `/subagents-refresh-provider-models <provider> [--force]` (pi `refreshProviderModelCatalog`,
    /// profiles.ts:489-577). Writes a per-provider catalog file under
    /// `providers/<provider>.models.json`, REAL-probing + classifying every candidate model
    /// ([`SubagentsExtension::write_provider_catalog_file`]); honors `--force` by reusing a still-fresh cache when
    /// `!force` and rewriting otherwise.
    pub(crate) async fn refresh_provider_catalog_cache(
        &self,
        cwd: &Path,
        provider: &str,
        force: bool,
    ) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        let profiles_dir = self.profiles_dir();

        // --force gate (pi `if (!options.force) { ... reuse fresh ... }`, profiles.ts:498-503):
        // a still-fresh cache is reused verbatim unless --force forces a rewrite. `.filter` keeps
        // the still-fresh cache and drops a stale one, avoiding nested `if`s.
        let fresh_cache = if force {
            None
        } else {
            crate::registration::profiles::read_provider_catalog(&profiles_dir, provider)?.filter(
                |existing| {
                    !crate::registration::profiles::is_provider_catalog_stale(
                        existing,
                        u64::try_from(crate::time::now_epoch_millis()).unwrap_or(0),
                        existing.max_age_days,
                    )
                },
            )
        };
        if let Some(existing) = fresh_cache {
            return Ok(format!(
                "subagents-refresh-provider-models: provider '{provider}' — fresh cache reused \
                 ({} model(s)); pass --force to rewrite.",
                existing.models.len()
            ));
        }

        // pi `if (availableModels.length === 0) throw new Error(...)` (profiles.ts:530-532) — a
        // command ERROR, not an informational success string.
        let has_models = registry_models()
            .iter()
            .any(|m| m.provider.as_str() == provider);
        if !has_models {
            return Err(SubagentError::MalformedSettings(format!(
                "No models found in the current registry for provider '{provider}'."
            )));
        }
        let _ = cwd;
        let model_count = self.write_provider_catalog_file(provider).await?;

        Ok(format!(
            "subagents-refresh-provider-models: refreshed catalog cache for '{provider}' \
             ({model_count} model(s)), live-probed and classified."
        ))
    }

    /// `/subagents-generate-profiles <provider>` (pi `generateProfilesForProvider`,
    /// profiles.ts:579-606). Refreshes the per-provider catalog (REAL-probing + classifying every
    /// candidate model), filters to usable + non-dominated models, then writes `<provider>.quota`
    /// and `<provider>.quality` profiles — EACH carrying the full 8-agent tier map PLUS a
    /// representative `subagents.defaultModel` (the medium tier, the fallback for non-builtin
    /// agents) ([`crate::registration::profiles::build_profile_file`]).
    pub(crate) async fn generate_provider_profiles(
        &self,
        provider: &str,
    ) -> Result<String, SubagentError> {
        crate::registration::profiles::validate_profile_name(provider)?;
        // pi's refreshProviderModelCatalog (called internally by generateProfilesForProvider,
        // profiles.ts:586) throws BEFORE any probing when the registry has zero models
        // (profiles.ts:530-532) — checked here, up front, so this mirrors that ordering exactly.
        let has_models = registry_models()
            .iter()
            .any(|m| m.provider.as_str() == provider);
        if !has_models {
            return Err(SubagentError::MalformedSettings(format!(
                "No models found in the current registry for provider '{provider}'."
            )));
        }
        // pi's generateProfilesForProvider refreshes the catalog first (profiles.ts:586).
        self.write_provider_catalog_file(provider).await?;

        let profiles_dir = self.profiles_dir();
        let catalog =
            crate::registration::profiles::read_provider_catalog(&profiles_dir, provider)?
                .ok_or_else(|| {
                    SubagentError::MalformedSettings(format!(
                        "provider catalog for '{provider}' is missing immediately after refresh"
                    ))
                })?;
        let ranked = Self::provider_ranked_full_ids_from_catalog(provider, &catalog);
        // pi `if (profileModels.length === 0) throw new Error(...)` (profiles.ts:593-595) — a
        // command ERROR, not an informational success string.
        if ranked.is_empty() {
            return Err(SubagentError::MalformedSettings(format!(
                "Provider '{provider}' has no usable models after filtering."
            )));
        }

        let generated = crate::registration::profiles::generate_provider_profiles(
            &profiles_dir,
            provider,
            &ranked,
        )?;

        Ok(format!(
            "Generated subagent profiles\n\
             Provider: {provider}\n\
             Quota: {quota}\n  cheap={qc}\n  medium={qm}\n  strong={qs}\n\
             Quality: {quality}\n  cheap={lc}\n  medium={lm}\n  strong={ls}\n\
             (8-agent tier map; live-probed and classified)",
            quota = generated.quota_path.display(),
            qc = generated.quota_models.cheap,
            qm = generated.quota_models.medium,
            qs = generated.quota_models.strong,
            quality = generated.quality_path.display(),
            lc = generated.quality_models.cheap,
            lm = generated.quality_models.medium,
            ls = generated.quality_models.strong,
        ))
    }

    /// This extension's resolved home — one of the four roots in `SubagentExtensionConfig::roots`.
    ///
    /// No `unwrap_or_else(home_dir)` fallback any more: the roots are resolved once, at
    /// construction, so there is no "or go read the environment" arm left to answer differently
    /// from the one the rest of this extension used.
    pub(crate) fn home(&self) -> PathBuf {
        self.roots.home().to_path_buf()
    }

    pub(crate) fn profiles_dir(&self) -> PathBuf {
        self.home()
            .join(".cyrup")
            .join("subagents")
            .join("profiles")
    }

    /// The user-scope `settings.json` the extension's discovery reads its `subagents.*` layer back
    /// from (`~/.cyrup/agents/settings.json` — the SAME file [`crate::extension::SubagentExecutor::discovery_config`] loads the
    /// user settings from). `/subagents-load-profile` writes the loaded profile's `subagents` block
    /// here so the next discovery pass picks it up, exactly as pi's `applySubagentProfile` writes to
    /// the same `getUserSettingsPath()` its discovery reads.
    fn user_settings_path(&self) -> PathBuf {
        self.home()
            .join(".cyrup")
            .join("agents")
            .join("settings.json")
    }

    /// `/subagents-load-profile <name>`: load the named profile and REPLACE ONLY the `subagents`
    /// key of the user settings file (pi `applySubagentProfile`, slash-commands.ts:852-883). Then
    /// surface the profile's `worker`-tier model (pi `getProfileWorkerModel`) as the model the user
    /// may want to switch the live session to.
    ///
    /// pi additionally *offers an interactive confirm* to switch the running session's model to the
    /// worker model, but ONLY when the host exposes `pi.setModel` + `ctx.modelRegistry`
    /// (slash-commands.ts:848-866); when it does not, pi falls straight through to the
    /// `else if (workerModel)` branch and simply reports the worker model (line 1167). A native
    /// `NativeExtension::execute_command` `HostCtx` exposes neither a `set_model` control op nor an
    /// interactive `confirm` today (that live session-model switch is the outer-layer UI tier,
    /// tracked separately) — so this reproduces pi's exact non-interactive branch: settings are
    /// written for real, and the worker model is reported.
    pub(crate) async fn load_profile_into_settings(
        &self,
        name: &str,
    ) -> Result<String, SubagentError> {
        let profiles_dir = self.profiles_dir();
        let profile = crate::registration::profiles::load_profile(&profiles_dir, name)?;
        let worker_model = crate::registration::profiles::profile_worker_model(&profile);
        let settings_path = self.user_settings_path();
        crate::registration::profiles::apply_profile_to_settings_file(&settings_path, &profile)?;

        let profile_path = crate::registration::profiles::profile_path(&profiles_dir, name)?;
        let mut lines = vec![
            format!("Loaded subagent profile: {name}"),
            format!("Profile: {}", profile_path.display()),
            format!("Updated: {}", settings_path.display()),
        ];
        if let Some(model) = worker_model {
            lines.push(format!("Profile worker model: {model}"));
        }
        Ok(lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    /// pi `refreshProviderModelCatalog` throws `"No models found in the current registry for
    /// provider '...'."` (profiles.ts:530-532) when the registry has zero models for the provider —
    /// cyrup used to return `Ok("... nothing to refresh...")` for this exact case instead. The
    /// unknown-provider check runs BEFORE any filesystem write, so this is safe to exercise without
    /// `CYRUP_HOME` sandboxing (no real `~/.cyrup` write happens on this path).
    #[tokio::test]
    async fn refresh_provider_catalog_cache_errors_for_an_unknown_provider() {
        let ext = SubagentsExtension::new();
        let cwd = std::env::temp_dir();
        let result = ext
            .refresh_provider_catalog_cache(&cwd, "totally-unknown-provider-xyz", false)
            .await;
        match result {
            Err(SubagentError::MalformedSettings(msg)) => {
                assert!(
                    msg.contains("No models found in the current registry for provider"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!(
                "expected an Err(MalformedSettings) for an unknown provider (pi throws here), got {other:?}"
            ),
        }
    }

    /// pi `generateProfilesForProvider` -> `refreshProviderModelCatalog` throws the identical
    /// "No models found..." error (profiles.ts:530-532, invoked at profiles.ts:513-601) BEFORE any
    /// usable-model filtering — cyrup used to return `Ok("... nothing to generate...")` instead.
    /// Also safe without `CYRUP_HOME` sandboxing: the unknown-provider check is the very first
    /// thing this handler does, before any filesystem write.
    #[tokio::test]
    async fn generate_provider_profiles_errors_for_an_unknown_provider() {
        let ext = SubagentsExtension::new();
        let result = ext
            .generate_provider_profiles("totally-unknown-provider-xyz")
            .await;
        match result {
            Err(SubagentError::MalformedSettings(msg)) => {
                assert!(
                    msg.contains("No models found in the current registry for provider"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!(
                "expected an Err(MalformedSettings) for an unknown provider (pi throws here), got {other:?}"
            ),
        }
    }
}
