//! The candidate filter and the bounded operator diagnostic it produces.
//!
//! Ports pi `filterFallbackCandidates` (`model-exclusions.ts:328-354`), `isExcluded` (`:275-279`)
//! and `findModelExclusion` (`:287-295`), plus the `model-fallback.ts` half that renders what was
//! dropped: `sanitizeModelExclusionDiagnostic` (`:296-301`), `formatModelExclusionExpiry`
//! (`:303-307`), `formatExcludedCandidateEvidence` (`:309-316`), `MODEL_UNAVAILABLE_PATTERN`
//! (`:318`), `ignoreStaleModelUnavailableExclusion` (`:320-324`) and
//! `ZERO_USABLE_MODEL_CANDIDATES_ERROR` (`:426-427`).

use cyrup_core::ModelId;

use crate::exec::fallback::{LowerLiteral, RetryPattern, any_line_matches, lower};
use crate::exec::model_exclusions::entry::{
    MAX_DATE_TIMESTAMP_MS, ModelExclusion, parse_model_key,
};
use crate::exec::model_exclusions::store::ModelExclusionStore;

/// pi `MODEL_EXCLUSION_DIAGNOSTIC_MAX_LENGTH` (`model-fallback.ts:293`).
const MODEL_EXCLUSION_DIAGNOSTIC_MAX_LENGTH: usize = 240;

/// pi `MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES` (`model-fallback.ts:294`).
///
/// The store caps at 200 entries and a ladder can in principle be that long; an unbounded
/// diagnostic over it is a wall of text at exactly the moment an operator needs one line.
const MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES: usize = 20;

/// pi `ZERO_USABLE_MODEL_CANDIDATES_ERROR` (`model-fallback.ts:426-427`), verbatim.
pub const ZERO_USABLE_MODEL_CANDIDATES_ERROR: &str =
    "No usable subagent models remain after registry, scope, and cached-exclusion filtering.";

/// pi `sanitizeModelExclusionDiagnostic` (`model-fallback.ts:296-301`), in upstream's order:
/// collapse control runs, trim, substitute `fallback` when empty, **redact**, then truncate.
///
/// **Redacting before truncating is the load-bearing half of that order.** The `reason` is raw
/// provider error text on its way to a log and to the operator's terminal; truncating first can
/// cut a credential in half so the redactor no longer recognises it and the leading bytes ship. The
/// same regression is already pinned elsewhere in this crate by
/// `tests/verify_memo_and_redaction.rs`'s
/// `redaction_runs_before_the_output_is_bounded_so_a_straddling_secret_cannot_leak`.
///
/// The collapsed set is upstream's `/[\u0000-\u001f\u007f\u2028\u2029]+/g` — C0 controls, DEL, and
/// the two Unicode line separators. The last two are not `char::is_control` in Rust and would
/// otherwise let a reason break the one-entry-per-line diagnostic apart.
///
/// [CYRUP-DELTA] the bound is 240 **chars**, where JS `.slice(0, 240)` counts UTF-16 code units.
/// The two agree for the ASCII provider text this path actually carries, and slicing by code unit
/// is not expressible in Rust without risking a split `char` — while the bound's purpose ("one
/// line, not a wall of text") is served identically either way.
#[must_use]
pub fn sanitize_model_exclusion_diagnostic(value: Option<&str>, fallback: &str) -> String {
    let mut normalized = String::new();
    let mut in_control_run = false;
    for ch in value.unwrap_or_default().chars() {
        if matches!(ch, '\u{0}'..='\u{1f}' | '\u{7f}' | '\u{2028}' | '\u{2029}') {
            if !in_control_run {
                normalized.push(' ');
                in_control_run = true;
            }
        } else {
            normalized.push(ch);
            in_control_run = false;
        }
    }
    let trimmed = normalized.trim();
    let source = if trimmed.is_empty() {
        fallback
    } else {
        trimmed
    };
    crate::watchdog::permission_arbiter::redact_secret_values(source)
        .chars()
        .take(MODEL_EXCLUSION_DIAGNOSTIC_MAX_LENGTH)
        .collect()
}

/// pi `formatModelExclusionExpiry` (`model-fallback.ts:303-307`).
///
/// `"unknown"` covers both of upstream's escapes — a non-finite number, and a finite one outside
/// the `Date` range (`new Date(9e15).getTime()` is `NaN`). An `i64` is always finite here, so only
/// the range check survives; it is not dead, because a recorded `expires_at` is `now + ttl` and the
/// TTL ceiling is deliberately set to keep that sum in range rather than to make it impossible.
#[must_use]
pub fn format_model_exclusion_expiry(expires_at: i64) -> String {
    if expires_at.unsigned_abs() > MAX_DATE_TIMESTAMP_MS.unsigned_abs() {
        return "unknown".to_string();
    }
    crate::background::run_status::format_iso8601_millis(expires_at)
}

/// pi `formatExcludedCandidateEvidence` (`model-fallback.ts:309-316`) — one entry, em dash, four
/// labelled fields, every one of them sanitized.
///
/// The three fallbacks are upstream's and are not interchangeable: `"unknown"` for a candidate or
/// model that sanitized away to nothing, `"unspecified"` for an absent provider (a bare candidate
/// under a provider-less entry genuinely has none), and `"runtime-failure"` for an absent reason —
/// the same literal [`super::store::ModelExclusionStore::record_model_failure`] stores by default.
#[must_use]
pub fn format_excluded_candidate_evidence(
    candidate: &ModelId,
    exclusion: &ModelExclusion,
) -> String {
    let key = parse_model_key(candidate.as_str());
    let display_candidate =
        sanitize_model_exclusion_diagnostic(Some(candidate.as_str()), "unknown");
    let display_model = sanitize_model_exclusion_diagnostic(Some(key.model_id.as_str()), "unknown");
    let provider = key
        .provider
        .as_ref()
        .map(cyrup_core::ProviderId::as_str)
        .or_else(|| {
            exclusion
                .target
                .provider()
                .map(cyrup_core::ProviderId::as_str)
        });
    let display_provider = sanitize_model_exclusion_diagnostic(provider, "unspecified");
    let reason =
        sanitize_model_exclusion_diagnostic(exclusion.reason.as_deref(), "runtime-failure");
    let expires = format_model_exclusion_expiry(exclusion.expires_at);
    format!(
        "{display_candidate} — model: {display_model}; provider: {display_provider}; reason: {reason}; expires: {expires}"
    )
}

/// pi `MODEL_UNAVAILABLE_PATTERN` (`model-fallback.ts:318`),
/// `/(?:model.*(?:not found|unavailable|disabled)|unknown model)/i`, decomposed into the four rows
/// [`crate::exec::fallback::RETRYABLE_MODEL_FAILURE_PATTERNS`] already carries for the same regex
/// shapes — so the two tables cannot drift into disagreeing about what "model unavailable" reads
/// like.
const MODEL_UNAVAILABLE_PATTERNS: &[RetryPattern] = &[
    RetryPattern::Then(lower!("model"), lower!("not found")),
    RetryPattern::Then(lower!("model"), lower!("unavailable")),
    RetryPattern::Then(lower!("model"), lower!("disabled")),
    RetryPattern::Contains(lower!("unknown model")),
];

/// pi `ignoreStaleModelUnavailableExclusion` (`model-fallback.ts:320-324`).
///
/// A `model-unavailable` exclusion says the registry did not have the model at the moment of
/// failure. If the model is in `available_models` NOW, that has changed, and honouring the
/// exclusion would pin a working model out for the full TTL over a transient availability blip.
/// Every other exclusion reason (a rate limit, a 401) is untouched — those say nothing about the
/// registry.
///
/// [CYRUP-DELTA] the suffix splitter is the STRICT
/// [`crate::exec::spawn_plan::split_known_thinking_suffix`], where upstream uses the unconditional
/// `splitThinkingSuffix` (`model-fallback.ts:13-19`). Substituting it is a narrowing in general
/// (`openai/gpt-5:preview` keeps its `:preview` here), and in cyrup that narrowing is the CORRECT
/// direction: a candidate only reaches this function after
/// [`crate::exec::fallback::build_model_candidates`] filtered the ladder to `available_models`, so
/// the spelling being looked up is the spelling that list holds. The unconditional splitter exists
/// privately in two other modules (`extension/models/mod.rs`, `watchdog/model_selection.rs`) and a
/// third copy is not worth a narrowing that cannot fire on this path.
#[must_use]
pub fn ignore_stale_model_unavailable_exclusion(
    candidate: &ModelId,
    exclusion: &ModelExclusion,
    available_models: &[ModelId],
) -> bool {
    let reason = exclusion.reason.as_deref().unwrap_or("");
    if !any_line_matches(reason, MODEL_UNAVAILABLE_PATTERNS) {
        return false;
    }
    let (base, _thinking) =
        crate::exec::spawn_plan::split_known_thinking_suffix(candidate.as_str());
    available_models.iter().any(|model| model.as_str() == base)
}

/// pi `onExcluded(candidate, exclusion)` (`model-exclusions.ts:330`) — the diagnostic hook.
///
/// A named alias rather than the bare `&dyn Fn`: the pair below appears on three signatures
/// (the options bag, [`ModelExclusionStore::find_active_exclusion`] and
/// [`ModelExclusionStore::find_model_exclusion`]), and three spellings of one callback shape is
/// how they drift.
pub type ExclusionObserver<'a> = &'a dyn Fn(&ModelId, &ModelExclusion);

/// pi `ignoreExclusion(candidate, exclusion) -> boolean` (`model-exclusions.ts:331`) — the escape
/// hatch, wired in production to [`ignore_stale_model_unavailable_exclusion`].
pub type ExclusionOverride<'a> = &'a dyn Fn(&ModelId, &ModelExclusion) -> bool;

/// One candidate the filter dropped, with the entry that dropped it — pi's inline
/// `ExcludedCandidate` (`model-fallback.ts:462`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExcludedCandidate {
    pub candidate: ModelId,
    pub exclusion: ModelExclusion,
}

/// What the exclusion filter dropped, carried out alongside the ladder.
///
/// This is the shape [`crate::exec::fallback::build_model_candidates_scoped`] already established
/// for scope warnings — "the ladder plus every violation observed" — applied to the second
/// diagnostic the ladder now produces. It exists because
/// [`crate::exec::fallback::build_model_candidates`] is `#[must_use] -> Vec<ModelId>` whose doc
/// promises it *"never fails and never panics"*: upstream `throw`s the zero-candidates error from
/// inside the builder, which in Rust would turn that contract (and five call sites, two of them
/// `pub`) into a `Result` for a diagnostic the existing empty-ladder site can render instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelExclusionEvidence {
    /// How many candidates the ladder held BEFORE the exclusion filter ran.
    ///
    /// pi's precondition for raising the zero-candidates error at all is `candidates.length > 0`
    /// (`model-fallback.ts:514`): a ladder that was already empty — no model configured, everything
    /// dropped by the `available_models` allowlist — is not an exclusion problem and must keep the
    /// caller's own empty-ladder message rather than gaining a bogus "excluded" diagnostic.
    pre_filter_count: usize,
    /// Every candidate dropped, counted — including the ones past
    /// [`MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES`] that are summarised as `; ... and N more`.
    excluded_count: usize,
    /// The first [`MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES`] drops, in ladder order.
    shown: Vec<ExcludedCandidate>,
}

impl ModelExclusionEvidence {
    /// The drops this evidence will actually render.
    #[must_use]
    pub fn shown(&self) -> &[ExcludedCandidate] {
        &self.shown
    }

    /// Every drop, counted — [`Self::shown`] is capped, this is not.
    #[must_use]
    pub fn excluded_count(&self) -> usize {
        self.excluded_count
    }

    /// pi's zero-candidates `throw` (`model-fallback.ts:512-520`), rendered rather than raised.
    ///
    /// `None` when upstream would not have thrown: the pre-filter ladder was empty, so the caller's
    /// own "empty fallback ladder" message is the honest one.
    #[must_use]
    pub fn zero_usable_candidates_error(&self) -> Option<String> {
        if self.pre_filter_count == 0 {
            return None;
        }
        if self.shown.is_empty() {
            return Some(ZERO_USABLE_MODEL_CANDIDATES_ERROR.to_string());
        }
        let entries = self
            .shown
            .iter()
            .map(|excluded| {
                format_excluded_candidate_evidence(&excluded.candidate, &excluded.exclusion)
            })
            .collect::<Vec<_>>()
            .join("; ");
        let omitted = self.excluded_count.saturating_sub(self.shown.len());
        let overflow = if omitted > 0 {
            format!("; ... and {omitted} more")
        } else {
            String::new()
        };
        Some(format!(
            "{ZERO_USABLE_MODEL_CANDIDATES_ERROR} (excluded: {entries}{overflow})"
        ))
    }
}

/// pi `filterFallbackCandidates`' options bag (`model-exclusions.ts:328-332`).
#[derive(Default, Clone, Copy)]
pub struct FilterOptions<'a> {
    /// The comparison instant. `None` reads [`crate::time::now_epoch_millis`].
    pub now: Option<i64>,
    /// pi `onExcluded` — the diagnostic hook. The filter already returns
    /// [`ModelExclusionEvidence`], which is the typed form of what upstream could only deliver as a
    /// callback; this stays a parameter so a caller wanting to observe drops as they happen (a
    /// test asserting the per-skip warning, a surface rendering them live) need not diff two lists.
    pub on_excluded: Option<ExclusionObserver<'a>>,
    /// pi `ignoreExclusion` — the escape hatch, wired in production to
    /// [`ignore_stale_model_unavailable_exclusion`].
    pub ignore_exclusion: Option<ExclusionOverride<'a>>,
}

/// pi `filterFallbackCandidates` (`model-exclusions.ts:328-354`): preserve order, drop duplicates,
/// drop excluded.
///
/// Every skipped candidate raises upstream's own warning verbatim (`model-fallback.ts:471`) — the
/// operator needs to know a model was silently not tried, and which cached failure did it.
pub fn filter_fallback_candidates(
    store: &ModelExclusionStore,
    candidates: &[ModelId],
    options: FilterOptions<'_>,
) -> (Vec<ModelId>, ModelExclusionEvidence) {
    let now = options.now.unwrap_or_else(crate::time::now_epoch_millis);
    let mut evidence = ModelExclusionEvidence {
        pre_filter_count: candidates.len(),
        ..ModelExclusionEvidence::default()
    };
    let mut filtered: Vec<ModelId> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        if candidate.as_str().is_empty() || filtered.contains(candidate) {
            continue;
        }
        if let Some(exclusion) =
            store.find_active_exclusion(candidate, now, options.ignore_exclusion)
        {
            evidence.excluded_count += 1;
            if evidence.shown.len() < MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES {
                evidence.shown.push(ExcludedCandidate {
                    candidate: candidate.clone(),
                    exclusion: exclusion.clone(),
                });
            }
            if let Some(on_excluded) = options.on_excluded {
                on_excluded(candidate, &exclusion);
            }
            let display_candidate =
                sanitize_model_exclusion_diagnostic(Some(candidate.as_str()), "unknown");
            let reason =
                sanitize_model_exclusion_diagnostic(exclusion.reason.as_deref(), "runtime-failure");
            let expires = format_model_exclusion_expiry(exclusion.expires_at);
            tracing::warn!(
                "[cyrup-ext-subagents] Skipping model '{display_candidate}' due to a cached exclusion (reason: {reason}; expires: {expires})."
            );
            continue;
        }
        filtered.push(candidate.clone());
    }
    (filtered, evidence)
}

#[cfg(test)]
mod tests {
    use cyrup_core::ProviderId;

    use super::*;
    use crate::exec::model_exclusions::entry::ModelExclusionTarget;

    fn exclusion(reason: Option<&str>, expires_at: i64) -> ModelExclusion {
        ModelExclusion {
            target: ModelExclusionTarget::Model {
                model_id: ModelId::from("gpt-4"),
                provider: Some(ProviderId::from("openai")),
            },
            reason: reason.map(str::to_string),
            recorded_at: 0,
            expires_at,
        }
    }

    #[test]
    fn the_sanitiser_redacts_before_it_truncates_so_a_straddling_secret_cannot_leak() {
        // The secret starts at char 235, so a truncate-first implementation would slice it to five
        // characters, the redactor would no longer recognise it, and the prefix would ship.
        let padding = "a".repeat(235);
        let raw = format!("{padding} sk-abcdefghijklmnop");
        let sanitized = sanitize_model_exclusion_diagnostic(Some(&raw), "runtime-failure");
        assert!(
            !sanitized.contains("sk-abcd"),
            "a straddling secret leaked: {sanitized}"
        );
        assert_eq!(
            sanitized.chars().count(),
            MODEL_EXCLUSION_DIAGNOSTIC_MAX_LENGTH
        );
    }

    #[test]
    fn control_runs_collapse_to_one_space_and_an_empty_value_becomes_the_fallback() {
        assert_eq!(
            sanitize_model_exclusion_diagnostic(Some("a\u{0}\u{1}\u{7f}b\u{2028}c"), "unknown"),
            "a b c"
        );
        assert_eq!(
            sanitize_model_exclusion_diagnostic(Some("\u{0}\u{1}"), "runtime-failure"),
            "runtime-failure"
        );
        assert_eq!(
            sanitize_model_exclusion_diagnostic(None, "unspecified"),
            "unspecified"
        );
    }

    #[test]
    fn the_evidence_line_is_upstreams_em_dash_shape_with_all_four_labels() {
        assert_eq!(
            format_excluded_candidate_evidence(
                &ModelId::from("openai/gpt-4"),
                &exclusion(Some("429 rate limit"), 1_700_000_000_123)
            ),
            "openai/gpt-4 — model: gpt-4; provider: openai; reason: 429 rate limit; expires: 2023-11-14T22:13:20.123Z"
        );
    }

    #[test]
    fn a_missing_provider_and_reason_use_upstreams_two_distinct_fallbacks() {
        let bare = ModelExclusion {
            target: ModelExclusionTarget::Model {
                model_id: ModelId::from("gpt-4"),
                provider: None,
            },
            reason: None,
            recorded_at: 0,
            expires_at: 1_700_000_000_123,
        };
        let rendered = format_excluded_candidate_evidence(&ModelId::from("gpt-4"), &bare);
        assert!(rendered.contains("provider: unspecified"), "{rendered}");
        assert!(rendered.contains("reason: runtime-failure"), "{rendered}");
    }

    #[test]
    fn an_out_of_range_expiry_renders_as_unknown_rather_than_a_nonsense_date() {
        assert_eq!(
            format_model_exclusion_expiry(MAX_DATE_TIMESTAMP_MS + 1),
            "unknown"
        );
        assert_eq!(format_model_exclusion_expiry(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn only_a_model_unavailable_reason_is_ignorable_and_only_when_the_model_is_back() {
        let available = vec![ModelId::from("openai/gpt-4")];
        let candidate = ModelId::from("openai/gpt-4");
        assert!(ignore_stale_model_unavailable_exclusion(
            &candidate,
            &exclusion(Some("model not found: gpt-4"), 1),
            &available
        ));
        assert!(ignore_stale_model_unavailable_exclusion(
            &candidate,
            &exclusion(Some("unknown model"), 1),
            &available
        ));
        // A rate limit says nothing about the registry.
        assert!(!ignore_stale_model_unavailable_exclusion(
            &candidate,
            &exclusion(Some("429 rate limit"), 1),
            &available
        ));
        // Still unavailable: the exclusion stands.
        assert!(!ignore_stale_model_unavailable_exclusion(
            &candidate,
            &exclusion(Some("model unavailable"), 1),
            &[]
        ));
    }

    #[test]
    fn a_thinking_suffixed_candidate_matches_its_base_id_in_available_models() {
        assert!(ignore_stale_model_unavailable_exclusion(
            &ModelId::from("openai/gpt-4:high"),
            &exclusion(Some("model unavailable"), 1),
            &[ModelId::from("openai/gpt-4")]
        ));
    }

    #[test]
    fn an_already_empty_ladder_produces_no_exclusion_error() {
        let evidence = ModelExclusionEvidence::default();
        assert_eq!(evidence.zero_usable_candidates_error(), None);
    }

    #[test]
    fn the_evidence_is_bounded_and_carries_upstreams_overflow_phrasing() {
        let shown: Vec<ExcludedCandidate> = (0..MODEL_EXCLUSION_DIAGNOSTIC_MAX_ENTRIES)
            .map(|i| ExcludedCandidate {
                candidate: ModelId::from(format!("openai/gpt-{i}")),
                exclusion: exclusion(Some("429"), 1_700_000_000_123),
            })
            .collect();
        let evidence = ModelExclusionEvidence {
            pre_filter_count: 25,
            excluded_count: 25,
            shown,
        };
        let rendered = evidence
            .zero_usable_candidates_error()
            .unwrap_or_else(|| "MISSING".to_string());
        assert!(
            rendered.starts_with(ZERO_USABLE_MODEL_CANDIDATES_ERROR),
            "{rendered}"
        );
        assert!(rendered.ends_with("; ... and 5 more)"), "{rendered}");
        assert_eq!(rendered.matches(" — model: ").count(), 20);
    }
}
