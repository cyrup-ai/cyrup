//! One exclusion record, and the two rules that decide what it applies to.
//!
//! Ports the entry half of pi `runs/shared/model-exclusions.ts` @v0.64.0: the
//! `ModelExclusionTarget` union (`:9`), the `ModelExclusion` shape (`:11-15`), `entryMatches`
//! (`:263-270`), `dedupKey` (`:187-189`) and `parseModelKey` (`:317-322`).
//!
//! [`parse_model_key`] and [`ModelExclusion::matches`] live in ONE file on purpose — upstream's own
//! doc (`:314-315`) requires the split to "stay in lock-step with the matching inside `isExcluded`",
//! because a failure recorded under one spelling and looked up under another is an exclusion that
//! silently never fires.

use cyrup_core::{ModelId, ProviderId};

/// pi `MAX_DATE_TIMESTAMP_MS` (`model-exclusions.ts:29`) — JavaScript's maximum `Date` timestamp,
/// and the ceiling every PERSISTED `recordedAt`/`expiresAt` is validated against.
///
/// Distinct from [`super::MAX_MODEL_EXCLUSION_TTL_MS`], which bounds a configured TTL: a TTL is a
/// duration added to `now`, so its ceiling is deliberately lower (`8e15` vs `8.64e15`) to keep the
/// SUM below this one. Both are upstream's, and conflating them lets a corrupt file produce an
/// expiry that overflows the clock arithmetic every comparison in this module depends on.
pub const MAX_DATE_TIMESTAMP_MS: i64 = 8_640_000_000_000_000;

/// What an exclusion applies to — pi `ModelExclusionTarget` (`model-exclusions.ts:9`).
///
/// A union upstream (`{modelId, provider?} | {provider, modelId?: never}`), and an enum here for
/// the reason upstream spells the `never` out: a record with NEITHER field is meaningless — it is
/// the one shape `readPersistedExclusion` rejects outright with `"must include modelId or
/// provider"` (`:181`) — and a record with both interpreted loosely would let `openai/gpt-4`
/// exclude `github-copilot/gpt-4`. Two `Option`s on a struct admit both mistakes; this admits
/// neither, and the first of them stops being a runtime validation rule at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelExclusionTarget {
    /// A specific model, optionally narrowed to one provider.
    Model {
        model_id: ModelId,
        provider: Option<ProviderId>,
    },
    /// Every model of one provider — what a quota or auth failure produces.
    Provider { provider: ProviderId },
}

impl ModelExclusionTarget {
    /// The model half, or `None` for a provider-wide entry.
    #[must_use]
    pub fn model_id(&self) -> Option<&ModelId> {
        match self {
            Self::Model { model_id, .. } => Some(model_id),
            Self::Provider { .. } => None,
        }
    }

    /// The provider half, present on a provider-wide entry and optional on a model entry.
    #[must_use]
    pub fn provider(&self) -> Option<&ProviderId> {
        match self {
            Self::Model { provider, .. } => provider.as_ref(),
            Self::Provider { provider } => Some(provider),
        }
    }

    /// pi `dedupKey` (`model-exclusions.ts:187-189`): `` `${provider ?? ""}|${modelId ?? ""}` ``.
    ///
    /// The separator is load-bearing in the same way upstream's is — provider-wide `openai` keys as
    /// `openai|` and model-only `openai` keys as `|openai`, so the two never collide.
    #[must_use]
    pub(crate) fn dedup_key(&self) -> String {
        let provider = self.provider().map_or("", ProviderId::as_str);
        let model_id = self.model_id().map_or("", ModelId::as_str);
        format!("{provider}|{model_id}")
    }
}

/// One live exclusion — pi `ModelExclusion` (`model-exclusions.ts:11-15`).
///
/// `recorded_at`/`expires_at` are this crate's single epoch-millisecond width ([`crate::time`]),
/// not a `SystemTime`: they are compared against timestamps minted by other processes (the
/// detached background runner records into the SAME file this one filters against), and the wire
/// format is upstream's `number`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelExclusion {
    pub target: ModelExclusionTarget,
    /// The raw provider error text that produced this record. Optional on the wire (`:12`);
    /// [`super::store::ModelExclusionStore::record_model_failure`] always fills it, defaulting to
    /// `"runtime-failure"` (`:224`).
    pub reason: Option<String>,
    pub recorded_at: i64,
    pub expires_at: i64,
}

impl ModelExclusion {
    /// pi `entryMatches` (`model-exclusions.ts:263-270`), whose two branches are asymmetric on
    /// purpose (upstream's doc, `:253-262`):
    ///
    /// * a **model** entry matches that `model_id`, and when *both* the entry and the candidate
    ///   carry a provider the providers must agree — so `openai/gpt-4` does not exclude
    ///   `github-copilot/gpt-4`. When either side's provider is absent the check is skipped rather
    ///   than failed, because an entry recorded from a bare id genuinely does not know which
    ///   provider served it;
    /// * a **provider** entry matches every model of that provider — what a quota or auth failure
    ///   produces, where the model was never the problem;
    /// * an expired entry (`expires_at <= now`) matches nothing, whatever its target. Expiry is
    ///   checked HERE rather than only at prune time so a store that has not been pruned since the
    ///   TTL lapsed still behaves correctly.
    #[must_use]
    pub fn matches(
        &self,
        candidate_model_id: &str,
        candidate_provider: Option<&str>,
        now: i64,
    ) -> bool {
        if self.expires_at <= now {
            return false;
        }
        match &self.target {
            ModelExclusionTarget::Model { model_id, provider } => {
                if model_id.as_str() != candidate_model_id {
                    return false;
                }
                match (provider, candidate_provider) {
                    (Some(entry_provider), Some(candidate_provider)) => {
                        entry_provider.as_str() == candidate_provider
                    }
                    _ => true,
                }
            }
            ModelExclusionTarget::Provider { provider } => {
                candidate_provider.is_some_and(|candidate| provider.as_str() == candidate)
            }
        }
    }
}

/// A candidate `fullId` split into the two halves an exclusion is keyed on — pi `parseModelKey`'s
/// `{ provider?, modelId }` (`model-exclusions.ts:317-322`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelKey {
    pub provider: Option<ProviderId>,
    pub model_id: ModelId,
}

/// pi `parseModelKey` (`model-exclusions.ts:317-322`, doc `:307-316`).
///
/// A thinking suffix is stripped FIRST, then the base id is split on its **first** `/` — so
/// `openrouter/google/gemini-flash` is provider `openrouter`, model `google/gemini-flash`, and a
/// bare `gpt-5` has no provider at all.
///
/// The suffix splitter is [`crate::exec::spawn_plan::split_known_thinking_suffix`], the STRICT one
/// (pi `splitKnownThinkingSuffix`, `shared/model-info.ts:39-47`) — upstream calls exactly that here
/// and the unconditional `splitThinkingSuffix` only in `ignoreStaleModelUnavailableExclusion`
/// (`model-fallback.ts:322`). The two are not interchangeable: the unconditional form truncates
/// `openai/gpt-5:preview` to `openai/gpt-5`, which would record an exclusion against a model id
/// that does not exist.
///
/// An id whose provider half or model half would be EMPTY (`"/gpt-5"`, `"openai/"`) keeps the whole
/// base as the model id and reports no provider, matching [`crate::exec::fallback::provider_of`]'s
/// rule rather than minting a `ProviderId("")` that `entryMatches` would then compare against.
#[must_use]
pub fn parse_model_key(full_id: &str) -> ModelKey {
    let (base, _thinking) = crate::exec::spawn_plan::split_known_thinking_suffix(full_id);
    match base.split_once('/') {
        Some((provider, model_id)) if !provider.is_empty() && !model_id.is_empty() => ModelKey {
            provider: Some(ProviderId::from(provider)),
            model_id: ModelId::from(model_id),
        },
        _ => ModelKey {
            provider: None,
            model_id: ModelId::from(base),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_entry(model_id: &str, provider: Option<&str>, expires_at: i64) -> ModelExclusion {
        ModelExclusion {
            target: ModelExclusionTarget::Model {
                model_id: ModelId::from(model_id),
                provider: provider.map(ProviderId::from),
            },
            reason: None,
            recorded_at: 0,
            expires_at,
        }
    }

    #[test]
    fn a_provider_qualified_model_entry_does_not_match_another_providers_same_model() {
        let entry = model_entry("gpt-4", Some("openai"), 1_000);
        assert!(entry.matches("gpt-4", Some("openai"), 0));
        assert!(!entry.matches("gpt-4", Some("github-copilot"), 0));
    }

    #[test]
    fn a_model_entry_without_a_provider_matches_the_model_under_any_provider() {
        let entry = model_entry("gpt-4", None, 1_000);
        assert!(entry.matches("gpt-4", Some("openai"), 0));
        assert!(entry.matches("gpt-4", None, 0));
        assert!(!entry.matches("gpt-5", Some("openai"), 0));
    }

    #[test]
    fn a_provider_entry_matches_every_model_of_that_provider_and_nothing_bare() {
        let entry = ModelExclusion {
            target: ModelExclusionTarget::Provider {
                provider: ProviderId::from("openai"),
            },
            reason: None,
            recorded_at: 0,
            expires_at: 1_000,
        };
        assert!(entry.matches("gpt-4", Some("openai"), 0));
        assert!(entry.matches("gpt-5", Some("openai"), 0));
        assert!(!entry.matches("gpt-4", Some("anthropic"), 0));
        assert!(!entry.matches("gpt-4", None, 0));
    }

    #[test]
    fn an_expired_entry_matches_nothing() {
        let entry = model_entry("gpt-4", Some("openai"), 1_000);
        assert!(!entry.matches("gpt-4", Some("openai"), 1_000));
        assert!(!entry.matches("gpt-4", Some("openai"), 1_001));
    }

    #[test]
    fn parse_model_key_splits_on_the_first_slash_after_stripping_a_thinking_suffix() {
        let key = parse_model_key("openrouter/google/gemini-flash");
        assert_eq!(
            key.provider.as_ref().map(ProviderId::as_str),
            Some("openrouter")
        );
        assert_eq!(key.model_id.as_str(), "google/gemini-flash");

        let suffixed = parse_model_key("openai/gpt-5:high");
        assert_eq!(
            suffixed.provider.as_ref().map(ProviderId::as_str),
            Some("openai")
        );
        assert_eq!(suffixed.model_id.as_str(), "gpt-5");

        // `:preview` is NOT a thinking level, so the strict splitter keeps it.
        let unknown_suffix = parse_model_key("openai/gpt-5:preview");
        assert_eq!(unknown_suffix.model_id.as_str(), "gpt-5:preview");

        let bare = parse_model_key("gpt-5");
        assert_eq!(bare.provider, None);
        assert_eq!(bare.model_id.as_str(), "gpt-5");
    }

    #[test]
    fn a_half_empty_qualified_id_reports_no_provider() {
        assert_eq!(parse_model_key("/gpt-5").provider, None);
        assert_eq!(parse_model_key("/gpt-5").model_id.as_str(), "/gpt-5");
        assert_eq!(parse_model_key("openai/").provider, None);
    }

    #[test]
    fn the_dedup_key_separator_keeps_a_provider_entry_distinct_from_a_bare_model_entry() {
        let provider_wide = ModelExclusionTarget::Provider {
            provider: ProviderId::from("openai"),
        };
        let bare_model = ModelExclusionTarget::Model {
            model_id: ModelId::from("openai"),
            provider: None,
        };
        assert_eq!(provider_wide.dedup_key(), "openai|");
        assert_eq!(bare_model.dedup_key(), "|openai");
        assert_ne!(provider_wide.dedup_key(), bare_model.dedup_key());
    }
}
