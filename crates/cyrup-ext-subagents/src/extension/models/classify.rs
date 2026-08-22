//! Heuristic model classification and ranking (pi `classifyModel`): capability bands, cost
//! normalization, Pareto filtering and the profile-check report.


use crate::extension::models::registry_models;
use crate::extension::models::probe::{probe_model, ProbeOutcome};

/// pi `extractVersionScore` (profiles.ts:167-171): the max of every `\d+(\.\d+)?` numeric token in
/// `id`, or `0.0` if none. Hand-rolled digit-run scan (no `regex` dependency in this crate) —
/// semantically identical to pi's global regex match + `Math.max`.
fn extract_version_score(id: &str) -> f64 {
    let bytes = id.as_bytes();
    let mut i = 0usize;
    let mut best: Option<f64> = None;
    let is_digit_at = |bytes: &[u8], idx: usize| bytes.get(idx).is_some_and(u8::is_ascii_digit);
    while let Some(&b) = bytes.get(i) {
        if b.is_ascii_digit() {
            let start = i;
            while is_digit_at(bytes, i) {
                i += 1;
            }
            if bytes.get(i) == Some(&b'.') && is_digit_at(bytes, i + 1) {
                i += 1;
                while is_digit_at(bytes, i) {
                    i += 1;
                }
            }
            if let Some(token) = bytes.get(start..i).and_then(|slice| std::str::from_utf8(slice).ok())
                && let Ok(value) = token.parse::<f64>()
                && value.is_finite()
            {
                best = Some(best.map_or(value, |b: f64| b.max(value)));
            }
        } else {
            i += 1;
        }
    }
    best.unwrap_or(0.0)
}

/// pi `modelNameTokens` (profiles.ts:173-180): lowercase, insert a space at every
/// letter-then-digit / digit-then-letter boundary, then split on runs of anything outside
/// `[a-z0-9.]`, dropping empty tokens. A single left-to-right scan reproduces pi's two sequential
/// global regex replaces (letter→digit, then digit→letter) exactly for every adjacent-character
/// transition, since both only ever look at one boundary at a time.
fn model_name_tokens(model_name: &str) -> Vec<String> {
    let lower = model_name.to_lowercase();
    let chars: Vec<char> = lower.chars().collect();
    let mut spaced = String::with_capacity(lower.len() + 4);
    for (idx, ch) in chars.iter().enumerate() {
        spaced.push(*ch);
        if let Some(next) = chars.get(idx + 1) {
            let cur_alpha = ch.is_ascii_lowercase();
            let cur_digit = ch.is_ascii_digit();
            let next_alpha = next.is_ascii_lowercase();
            let next_digit = next.is_ascii_digit();
            if (cur_alpha && next_digit) || (cur_digit && next_alpha) {
                spaced.push(' ');
            }
        }
    }
    spaced
        .split(|c: char| !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.'))
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// pi `inferProfileBand` (profiles.ts:182-189): a coarse 0..=4 capability band inferred purely
/// from name tokens (spark/flash/nano/tiny/instant → 0; mini/haiku/small → 1; opus/max/ultra/pro →
/// 4; sonnet/turbo/plus → 3; anything else → 2).
fn infer_profile_band(model_name: &str) -> u8 {
    let tokens: std::collections::HashSet<String> =
        model_name_tokens(model_name).into_iter().collect();
    let has = |list: &[&str]| list.iter().any(|t| tokens.contains(*t));
    if has(&["spark", "flash", "nano", "tiny", "instant"]) {
        return 0;
    }
    if has(&["mini", "haiku", "small"]) {
        return 1;
    }
    if has(&["opus", "max", "ultra", "pro"]) {
        return 4;
    }
    if has(&["sonnet", "turbo", "plus"]) {
        return 3;
    }
    2
}

/// pi `combinedCost` (profiles.ts:216-221): the sum of every finite cost field. Since
/// `cyrup_provider::ModelCost`'s fields are required (never `Option`), this always yields
/// `Some(sum)` for a registry model (pi's `undefined` branch is reachable only when the
/// registry omits cost metadata entirely, which the embedded-catalog schema never does).
pub(crate) fn combined_cost(cost: &cyrup_provider::ModelCost) -> Option<f64> {
    let values = [cost.input, cost.output, cost.cache_read, cost.cache_write];
    let filtered: Vec<f64> = values.into_iter().filter(|v| v.is_finite()).collect();
    if filtered.is_empty() { None } else { Some(filtered.iter().sum()) }
}

/// pi's `NumericStats` (profiles.ts:205-208): the min/max of a value set, used to min-max
/// normalize a raw metric into `0.0..=1.0`.
#[derive(Clone, Copy, Debug)]
struct NumericStats {
    min: f64,
    max: f64,
}

/// pi `collectStats` (profiles.ts:223-227): `None` when every input is missing/non-finite.
fn collect_stats(values: &[Option<f64>]) -> Option<NumericStats> {
    let filtered: Vec<f64> = values.iter().filter_map(|v| v.filter(|x| x.is_finite())).collect();
    if filtered.is_empty() {
        return None;
    }
    let min = filtered.iter().copied().fold(f64::INFINITY, f64::min);
    let max = filtered.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    Some(NumericStats { min, max })
}

/// pi `normalize` (profiles.ts:229-233): min-max normalize `value` into `stats`' range; a
/// degenerate (all-equal) range normalizes to `0.5`.
fn normalize(value: Option<f64>, stats: Option<&NumericStats>) -> Option<f64> {
    let value = value?;
    let stats = stats?;
    if stats.max <= stats.min {
        return Some(0.5);
    }
    Some((value - stats.min) / (stats.max - stats.min))
}

/// pi `ClassificationContext` (profiles.ts:210-214), built once per provider-filtered candidate
/// set (pi `buildClassificationContext`, profiles.ts:235-241). pi's sibling `cost` stat feeds only
/// `costTier`/`latencyTier` (profiles.ts:306-312) — NEITHER of which contributes to `profileRank`
/// (profiles.ts:315-325's `qualitySignals` never includes `costNorm`) — so it is not modeled here; see
/// [`ModelClassification`]'s doc comment for why `profile_rank` is the only field this port keeps.
pub(crate) struct ClassificationContext {
    context_window: Option<NumericStats>,
    max_tokens: Option<NumericStats>,
}

pub(crate) fn build_classification_context(models: &[cyrup_provider::Model]) -> ClassificationContext {
    ClassificationContext {
        context_window: collect_stats(
            &models.iter().map(|m| Some(m.context_window as f64)).collect::<Vec<_>>(),
        ),
        max_tokens: collect_stats(
            &models.iter().map(|m| Some(m.max_tokens as f64)).collect::<Vec<_>>(),
        ),
    }
}

/// The result of pi `classifyModel` (profiles.ts:250-308), trimmed to the one field
/// `provider_ranked_full_ids`/`write_provider_catalog_file`/[`dominates`] actually consume as a
/// sort/selection key: `profile_rank` (pi `derived.profileRank`, profiles.ts:54/298). pi's sibling
/// `costTier`/`qualityTier`/`latencyTier`/`recommendedRoleTier`/`recommendedAgents`/
/// `classificationSources` fields feed only informational catalog-JSON display and the
/// `heuristicFallbackCount` reporting this port does not surface (a scope trim noted at this
/// crate's call sites) — every RANKING/FILTERING decision pi actually makes (tier selection,
/// `dominatesModel`) keys on `profileRank` alone, which this struct preserves byte-for-byte.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ModelClassification {
    pub(crate) profile_rank: i64,
}

/// pi `classifyModel` (profiles.ts:250-308): the full heuristic + official-metadata blended
/// classification, reduced to its `profileRank` output (see [`ModelClassification`]'s doc comment).
/// See the module-level doc comment above for why this crate's registry input always has
/// "official metadata" (pi's `hasOfficialMetadata` is always `true` here).
pub(crate) fn classify_model(model: &cyrup_provider::Model, ctx: &ClassificationContext) -> ModelClassification {
    let model_name = if model.name.trim().is_empty() { model.id.as_str() } else { model.name.as_str() };
    let tokens: std::collections::HashSet<String> = model_name_tokens(model_name).into_iter().collect();
    let band = infer_profile_band(model_name);
    let version_score = extract_version_score(model.id.as_str());
    let context_norm = normalize(Some(model.context_window as f64), ctx.context_window.as_ref());
    let max_tokens_norm = normalize(Some(model.max_tokens as f64), ctx.max_tokens.as_ref());

    let heuristic_base = f64::from(band) / 4.0;
    let mut quality_signals: Vec<f64> = vec![heuristic_base];
    if let Some(v) = context_norm {
        quality_signals.push(v);
    }
    if let Some(v) = max_tokens_norm {
        quality_signals.push(v);
    }
    quality_signals.push(if model.reasoning { 1.0 } else { 0.0 });

    let latency_hints_fast = ["highspeed", "flash", "instant", "turbo"]
        .iter()
        .any(|t| tokens.contains(*t));

    #[allow(clippy::cast_precision_loss)]
    let mut quality_score = quality_signals.iter().sum::<f64>() / quality_signals.len() as f64;
    if latency_hints_fast {
        quality_score -= 0.2;
    }
    quality_score = quality_score.clamp(0.0, 1.0);

    let latency_penalty: i64 = if latency_hints_fast { 125 } else { 0 };
    let profile_rank =
        (quality_score * 100.0 * 10.0).round() as i64 + (version_score * 25.0).round() as i64 - latency_penalty;

    ModelClassification { profile_rank }
}

/// One usable, ranked candidate for [`filter_dominated`] (pi's `ProviderModelCatalogModel` fields
/// `dominatesModel`, profiles.ts:382-396, actually reads: `observed.cost`, `derived.profileRank`,
/// `observed.reasoning`, `observed.contextWindow`, `observed.maxTokens`).
#[derive(Clone, Debug)]
pub(crate) struct RankedCandidate {
    pub(crate) full_id: String,
    pub(crate) cost: f64,
    pub(crate) profile_rank: i64,
    pub(crate) reasoning: bool,
    pub(crate) context_window: u64,
    pub(crate) max_tokens: u64,
}

/// pi `dominatesModel` (profiles.ts:382-396): `a` dominates `b` when `a` is never worse on any
/// axis (cheaper-or-equal, ranked-at-least-as-high, reasoning-at-least-as-good, context/max-tokens
/// at-least-as-large) AND strictly better on at least one. Since this crate's `cost` is always
/// defined (never pi's `undefined` short-circuit — see [`combined_cost`]'s doc comment), that
/// branch of pi's function is unreachable here.
fn dominates(a: &RankedCandidate, b: &RankedCandidate) -> bool {
    if a.cost > b.cost {
        return false;
    }
    if a.profile_rank < b.profile_rank {
        return false;
    }
    if u8::from(a.reasoning) < u8::from(b.reasoning) {
        return false;
    }
    if a.context_window < b.context_window {
        return false;
    }
    if a.max_tokens < b.max_tokens {
        return false;
    }
    a.cost < b.cost
        || a.profile_rank > b.profile_rank
        || (a.reasoning && !b.reasoning)
        || a.context_window > b.context_window
        || a.max_tokens > b.max_tokens
}

/// pi `filterDominatedModels` (profiles.ts:398-400): drop every candidate that some OTHER
/// candidate in the set dominates. Identifies "the other candidate" by pointer identity
/// ([`std::ptr::eq`]) rather than a numeric index, so this never indexes `candidates` directly
/// (clippy's `indexing_slicing`, denied outside `#[cfg(test)]` by this crate's own lints).
pub(crate) fn filter_dominated(candidates: Vec<RankedCandidate>) -> Vec<RankedCandidate> {
    let keep: Vec<bool> = candidates
        .iter()
        .map(|candidate| {
            !candidates
                .iter()
                .any(|other| !std::ptr::eq(other, candidate) && dominates(other, candidate))
        })
        .collect();
    candidates.into_iter().zip(keep).filter(|(_, k)| *k).map(|(c, _)| c).collect()
}

/// Render `/subagents-check-profile`'s report (pi `checkSubagentProfile`, profiles.ts:608-637):
/// for every `overrides.<agent>.model` the profile declares (pi does NOT check `defaultModel` —
/// `entries` at profiles.ts:639-641 only ever walks `profile.subagents.agentOverrides`), resolve it
/// against the model registry ([`registry_models`]) and REAL-probe the resolved full id
/// (or the raw string when unresolved) via [`probe_model`], with a per-probed-id cache so the same
/// model is never probed twice in one report (pi's `probeCache`, profiles.ts:642).
pub(crate) async fn render_profile_check_report(
    name: &str,
    profile: &crate::registration::profiles::NamedProfile,
) -> String {
    // pi's `entries` (profiles.ts:639-641) walks ONLY `agentOverrides`, never `defaultModel`.
    let mut refs: Vec<(String, String)> = Vec::new();
    for (agent_name, over) in &profile.subagents.overrides {
        if let crate::discovery::types::OverrideField::Value(model) = &over.model {
            let trimmed = model.trim();
            if !trimmed.is_empty() {
                refs.push((agent_name.clone(), trimmed.to_string()));
            }
        }
    }

    if refs.is_empty() {
        return format!("subagents-check-profile '{name}': no model references declared.");
    }

    // Recognize BOTH bare ids (`gpt-4o`) and fully-qualified `provider/id` refs (`openai/gpt-4o`)
    // — pi's `findModelInfo` resolves either form against `ctx.modelRegistry.getAvailable()`.
    let mut known: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for m in registry_models() {
        let full_id = format!("{}/{}", m.provider.as_str(), m.id.as_str());
        known.entry(m.id.as_str().to_string()).or_insert_with(|| full_id.clone());
        known.entry(full_id.clone()).or_insert(full_id);
    }

    let mut probe_cache: std::collections::HashMap<String, ProbeOutcome> = std::collections::HashMap::new();
    let mut out = format!("subagents-check-profile '{name}':\n");
    for (agent, model) in refs {
        let resolved_full_id = known.get(&model).cloned();
        let in_registry = resolved_full_id.is_some();
        let probe_id = resolved_full_id.unwrap_or_else(|| model.clone());
        let probe = match probe_cache.get(&probe_id) {
            Some(cached) => cached.clone(),
            None => {
                let result = probe_model(&probe_id).await;
                probe_cache.insert(probe_id.clone(), result.clone());
                result
            }
        };
        let message = probe
            .message
            .as_deref()
            .map(|m| m.lines().next().unwrap_or(""))
            .filter(|line| !line.is_empty())
            .map(|line| format!(" ({line})"))
            .unwrap_or_default();
        out.push_str(&format!(
            "  {agent} → {model} — registry {}; probe {}{message}\n",
            if in_registry { "ok" } else { "missing" },
            probe.status.as_str(),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

    use super::*;

    // =============================================================================================
    // "profiles" unit divergence fixes: real live-probe classification/ranking (pi
    // `probeModel`/`classifyModel`/`refreshProviderModelCatalog`/`generateProfilesForProvider`,
    // profiles.ts:267-630) + Ok-vs-Err on empty-provider paths (profiles.ts:603-630/593-595).
    // =============================================================================================

    fn test_model(
        provider: &str,
        id: &str,
        name: &str,
        cost_total: f64,
        context_window: u64,
        max_tokens: u64,
        reasoning: bool,
    ) -> cyrup_provider::Model {
        cyrup_provider::Model {
            id: cyrup_core::ModelId::from(id),
            name: name.to_string(),
            api: cyrup_core::ApiId::from("test-api"),
            provider: cyrup_core::ProviderId::from(provider),
            base_url: "https://example.invalid".to_string(),
            reasoning,
            input: vec![cyrup_provider::Modality::Text],
            cost: cyrup_provider::ModelCost {
                input: cost_total / 2.0,
                output: cost_total / 2.0,
                cache_read: 0.0,
                cache_write: 0.0,
                tiers: None,
            },
            context_window,
            max_tokens,
            // AGENT-026 added `Model.sampling_params` after this fixture was written. This crate
            // never reads it (the model report and the scope checks are cost/context-window
            // driven), so the fixture states the unset form rather than inventing a value.
            sampling_params: None,
            thinking_level_map: None,
            compat: None,
            headers: None,
        }
    }

    /// pi `extractVersionScore` (profiles.ts:167-171): the max numeric token, decimals included.
    #[test]
    fn extract_version_score_takes_the_max_numeric_token() {
        assert_eq!(extract_version_score("claude-3-5-sonnet"), 5.0);
        assert_eq!(extract_version_score("gpt-4o"), 4.0);
        assert_eq!(extract_version_score("gemini-1.5-pro"), 1.5);
        assert_eq!(extract_version_score("no-numbers-here"), 0.0);
    }

    /// pi `modelNameTokens`/`inferProfileBand` (profiles.ts:173-189).
    #[test]
    fn infer_profile_band_recognizes_known_name_tokens() {
        assert_eq!(infer_profile_band("Claude Haiku 4.5"), 1);
        assert_eq!(infer_profile_band("Claude Opus 4.5"), 4);
        assert_eq!(infer_profile_band("Claude Sonnet 4.5"), 3);
        assert_eq!(infer_profile_band("Gemini 2.0 Flash"), 0);
        assert_eq!(infer_profile_band("Totally Unbranded Model"), 2);
    }

    /// THE core regression this unit's dossier item 3 flags: cyrup used to rank a provider's
    /// models by raw ascending `cost.input + cost.output` (`provider_ranked_full_ids`'s old body),
    /// NOT by pi's `derived.profileRank` (profiles.ts:298, driven by capability heuristics, not
    /// price). Construct two models where cost order and capability order are OPPOSITE — an
    /// expensive-but-weak model and a cheap-but-strong one — and assert `classify_model` ranks the
    /// weak model lower (as pi's `profileRank` does), even though it is the pricier of the two.
    /// The pre-fix cost-ascending sort would have put the cheap/strong model FIRST (i.e. into the
    /// "cheap" tier) and the expensive/weak model LAST (the "strong" tier) — exactly backwards.
    #[test]
    fn classify_model_ranks_by_capability_not_raw_cost() {
        let expensive_but_weak =
            test_model("acme", "acme-nano-1", "Acme Nano 1", 100.0, 4_000, 1_000, false);
        let cheap_but_strong =
            test_model("acme", "acme-opus-9", "Acme Opus 9", 2.0, 200_000, 64_000, true);
        let ctx = build_classification_context(&[expensive_but_weak.clone(), cheap_but_strong.clone()]);

        let weak_rank = classify_model(&expensive_but_weak, &ctx).profile_rank;
        let strong_rank = classify_model(&cheap_but_strong, &ctx).profile_rank;

        assert!(
            weak_rank < strong_rank,
            "the weak/expensive model must rank BELOW the strong/cheap one (profileRank {weak_rank} vs {strong_rank})"
        );
        // The pre-fix behavior (ascending raw cost) would order these the OTHER way: cheap (2.0)
        // before expensive (100.0) — i.e. strong before weak. Confirm the two orderings actually
        // disagree, so this test is a genuine regression proof, not a vacuous assertion.
        let cost_ascending_puts_strong_first =
            combined_cost(&cheap_but_strong.cost) < combined_cost(&expensive_but_weak.cost);
        assert!(cost_ascending_puts_strong_first, "test fixture must actually invert cost vs capability");
    }

    /// pi `dominatesModel`/`filterDominatedModels` (profiles.ts:382-400): a candidate that is
    /// cheaper-or-equal, ranked-at-least-as-high, and never worse on reasoning/context/max-tokens —
    /// with at least one strict improvement — dominates and drops the other.
    #[test]
    fn filter_dominated_drops_strictly_worse_candidates() {
        let dominated = RankedCandidate {
            full_id: "acme/weak-and-pricier".to_string(),
            cost: 10.0,
            profile_rank: 5,
            reasoning: false,
            context_window: 1_000,
            max_tokens: 100,
        };
        let dominator = RankedCandidate {
            full_id: "acme/strong-and-cheaper".to_string(),
            cost: 5.0,
            profile_rank: 50,
            reasoning: true,
            context_window: 2_000,
            max_tokens: 200,
        };
        let incomparable = RankedCandidate {
            full_id: "acme/cheap-but-narrow".to_string(),
            cost: 1.0,
            profile_rank: 1,
            reasoning: false,
            context_window: 500,
            max_tokens: 50,
        };
        let kept = filter_dominated(vec![dominated, dominator.clone(), incomparable.clone()]);
        let kept_ids: Vec<&str> = kept.iter().map(|c| c.full_id.as_str()).collect();
        assert!(!kept_ids.contains(&"acme/weak-and-pricier"), "the dominated candidate must be dropped");
        assert!(kept_ids.contains(&"acme/strong-and-cheaper"));
        assert!(kept_ids.contains(&"acme/cheap-but-narrow"), "an incomparable (Pareto-optimal) candidate must survive");
    }

}
