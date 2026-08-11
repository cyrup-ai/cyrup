//! The emission guard — a 1:1 port of `pi-subagents/src/watchdog/emission-guard.ts` (123 lines
//! @v0.43.0).
//!
//! One review turn can call `watchdog_warn` many times; this is the stateful filter that decides
//! which of those calls actually become warnings. **The decision order is the contract**, not just
//! the set of rejections — `evaluate` (`emission-guard.ts:96-125`) runs six steps in a fixed
//! sequence and each later step is reachable only because an earlier one let the warning through:
//!
//! 1. **content-free** (`:97-99`) — any of `summary`/`evidence`/`recommendedAction` normalizing to
//!    empty or to a known filler phrase rejects with NO identity attached. This runs first, so a
//!    filler warning never consumes the per-update budget and never enters the dedup history.
//! 2. identity + prior-severity lookup (`:100-103`) — `escalation` is `concern -> blocker` on an
//!    already-accepted *underlying* identity (severity-free), which is why the two identity
//!    functions exist separately.
//! 3. **update-budget** (`:104-109`) — at most ONE warning per model update, the sole exception
//!    being an escalation of *that same update's* accepted warning. Checked BEFORE the dedup and
//!    max-warnings gates, so a second distinct warning in one update is reported as `update-budget`,
//!    never as `duplicate`.
//! 4. **duplicate** (`:110`) — a non-escalating repeat of an identity already accepted.
//! 5. **max-warnings** (`:111-113`) — the accepted-count ceiling, which an escalation bypasses.
//! 6. accept (`:115-124`) — record severity, count/append to history ONLY on a first sighting
//!    (`if (!priorSeverity)`, `:117-120`), evict the oldest identities past the history limit
//!    **without** decrementing the accepted count, then arm the per-update slot.
//!
//! Two consequences of that ordering are easy to get wrong and are asserted in this module's tests:
//! step 6's counter increments only for genuinely new identities (an escalation re-writes the map
//! entry but does not grow `accepted_count`), and step 6's eviction shrinks the dedup memory while
//! leaving the ceiling consumed — evicting an identity does NOT give the ceiling a slot back.
//!
//! [CYRUP-DELTA] `normalizeWatchdogEmissionText`'s `/[^\p{L}\p{N}]+/gu` becomes
//! `!(is_alphabetic() || is_numeric())`. `char::is_numeric` is exactly `\p{N}`; `char::is_alphabetic`
//! is the Alphabetic derived property, a strict superset of `\p{L}` by the Other_Alphabetic
//! combining marks (which `\p{N}`'s Nl overlap already covers for the rest). After the preceding
//! NFKC + lowercase, the only inputs that could observe the difference are standalone combining
//! marks in warning prose, which would in any case be surrounded by retained letters.

use std::collections::HashMap;
use unicode_normalization::UnicodeNormalization;

use crate::watchdog::types::{WatchdogSeverity, WatchdogWarning};

/// `WatchdogEmissionSuppressionReason` (`emission-guard.ts:3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogEmissionSuppressionReason {
    /// Some required field normalized to empty or to a [`CONTENT_FREE_PHRASES`] filler.
    ContentFree,
    /// A non-escalating repeat of an already-accepted underlying identity.
    Duplicate,
    /// The accepted-count ceiling is reached and this is not an escalation.
    MaxWarnings,
    /// This model update already emitted its one warning.
    UpdateBudget,
}

impl WatchdogEmissionSuppressionReason {
    /// The upstream wire string for this reason.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            WatchdogEmissionSuppressionReason::ContentFree => "content-free",
            WatchdogEmissionSuppressionReason::Duplicate => "duplicate",
            WatchdogEmissionSuppressionReason::MaxWarnings => "max-warnings",
            WatchdogEmissionSuppressionReason::UpdateBudget => "update-budget",
        }
    }
}

/// `WatchdogEmissionDecision` (`emission-guard.ts:5-15`) — the accepted/rejected union. The rejected
/// arm carries the two identities **optionally**, because the `content-free` rejection is taken
/// before they are computed at all.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchdogEmissionDecision {
    /// The warning is emitted.
    Accepted {
        /// `severity \n underlying` — the identity stamped onto the warning details.
        identity: String,
        /// The severity-free identity the dedup map is keyed on.
        underlying_identity: String,
        /// True when this accept upgraded an already-accepted `concern` to `blocker`.
        escalation: bool,
    },
    /// The warning is suppressed.
    Rejected {
        /// Why.
        reason: WatchdogEmissionSuppressionReason,
        /// Present for every reason except [`WatchdogEmissionSuppressionReason::ContentFree`].
        identity: Option<String>,
        /// Present for every reason except [`WatchdogEmissionSuppressionReason::ContentFree`].
        underlying_identity: Option<String>,
    },
}

impl WatchdogEmissionDecision {
    /// Whether the warning may be emitted — upstream's `decision.accepted` discriminant.
    #[must_use]
    pub const fn accepted(&self) -> bool {
        matches!(self, WatchdogEmissionDecision::Accepted { .. })
    }

    /// The identity, for whichever arm carries one.
    #[must_use]
    pub fn identity(&self) -> Option<&str> {
        match self {
            WatchdogEmissionDecision::Accepted { identity, .. } => Some(identity.as_str()),
            WatchdogEmissionDecision::Rejected { identity, .. } => identity.as_deref(),
        }
    }
}

/// `CONTENT_FREE_PHRASES` (`emission-guard.ts:22-40`), post-normalization forms. Note `"n a"` — the
/// normalized form of `N/A`, since `/` is stripped to a space by the same normalizer.
pub const CONTENT_FREE_PHRASES: &[&str] = &[
    "stop",
    "done",
    "complete",
    "completed",
    "no issue",
    "no issues",
    "no concern",
    "no concerns",
    "nothing to add",
    "lgtm",
    "looks good",
    "looks good to me",
    "all good",
    "ok",
    "okay",
    "none",
    "n a",
];

/// The default dedup-history depth (`emission-guard.ts:75`'s `options.dedupeHistoryLimit ?? 200`).
pub const DEFAULT_DEDUPE_HISTORY_LIMIT: usize = 200;

/// `normalizeWatchdogEmissionText` (`emission-guard.ts:42-50`): NFKC, lowercase, drop the three
/// apostrophe forms outright (so `don't` and `dont` share an identity), collapse every other
/// non-alphanumeric run to a single space, then trim.
///
/// The apostrophe removal must happen BEFORE the non-alphanumeric collapse, or `don't` would
/// normalize to `don t` rather than `dont` and the two spellings would dedup apart.
#[must_use]
pub fn normalize_watchdog_emission_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    for ch in value.nfkc().flat_map(char::to_lowercase) {
        // `.replace(/[’'`]/g, "")` — removed, not spaced.
        if ch == '\u{2019}' || ch == '\'' || ch == '`' {
            continue;
        }
        if ch.is_alphabetic() || ch.is_numeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(ch);
        } else {
            pending_space = true;
        }
    }
    out
}

/// `watchdogWarningUnderlyingIdentity` (`emission-guard.ts:52-54`): the severity-FREE identity, so a
/// `concern` and a `blocker` describing the same finding collide on purpose — that collision is what
/// makes escalation detectable.
#[must_use]
pub fn watchdog_warning_underlying_identity(summary: &str, evidence: &str) -> String {
    format!(
        "{}\n{}",
        normalize_watchdog_emission_text(summary),
        normalize_watchdog_emission_text(evidence)
    )
}

/// `watchdogWarningIdentity` (`emission-guard.ts:56-58`): severity-prefixed, the value stamped onto
/// [`crate::watchdog::types::WatchdogWarningDetails::identity`] and compared by the runtime's
/// auto-follow stalemate counter.
#[must_use]
pub fn watchdog_warning_identity(
    severity: WatchdogSeverity,
    summary: &str,
    evidence: &str,
) -> String {
    format!(
        "{}\n{}",
        severity.as_str(),
        watchdog_warning_underlying_identity(summary, evidence)
    )
}

/// `isContentFree` (`emission-guard.ts:60-63`).
fn is_content_free(value: &str) -> bool {
    let normalized = normalize_watchdog_emission_text(value);
    normalized.is_empty() || CONTENT_FREE_PHRASES.contains(&normalized.as_str())
}

/// `WatchdogEmissionGuardOptions` (`emission-guard.ts:17-20`).
#[derive(Debug, Clone, Default)]
pub struct WatchdogEmissionGuardOptions {
    /// The accepted-count ceiling; `None` is upstream's `null` (unbounded).
    pub max_warnings: Option<u32>,
    /// How many underlying identities are remembered; `None` takes
    /// [`DEFAULT_DEDUPE_HISTORY_LIMIT`].
    pub dedupe_history_limit: Option<usize>,
}

/// `WatchdogEmissionGuard` (`emission-guard.ts:65-127`).
#[derive(Debug)]
pub struct WatchdogEmissionGuard {
    max_warnings: Option<u32>,
    dedupe_history_limit: usize,
    accepted_count: u32,
    accepted_by_underlying_identity: HashMap<String, WatchdogSeverity>,
    history_order: Vec<String>,
    update_accepted_underlying_identity: Option<String>,
    update_accepted_severity: Option<WatchdogSeverity>,
}

impl Default for WatchdogEmissionGuard {
    fn default() -> Self {
        Self::new(WatchdogEmissionGuardOptions::default())
    }
}

impl WatchdogEmissionGuard {
    /// `constructor` (`emission-guard.ts:74-77`).
    #[must_use]
    pub fn new(options: WatchdogEmissionGuardOptions) -> Self {
        Self {
            max_warnings: options.max_warnings,
            dedupe_history_limit: options
                .dedupe_history_limit
                .unwrap_or(DEFAULT_DEDUPE_HISTORY_LIMIT),
            accepted_count: 0,
            accepted_by_underlying_identity: HashMap::new(),
            history_order: Vec::new(),
            update_accepted_underlying_identity: None,
            update_accepted_severity: None,
        }
    }

    /// `startModelUpdate` (`emission-guard.ts:79-82`): re-arm the one-warning-per-update slot.
    ///
    /// The runtime calls this exactly once per agent-end boundary, BEFORE LSP collection
    /// (`runtime.ts:389`), so the boundary's LSP-derived warning and the review model's own warning
    /// contend for the same single slot — the LSP warning, being displayed first, wins it.
    pub fn start_model_update(&mut self) {
        self.update_accepted_underlying_identity = None;
        self.update_accepted_severity = None;
    }

    /// `reset` (`emission-guard.ts:84-89`): forget the accepted count, the dedup map, the history
    /// order, and (via [`Self::start_model_update`]) the per-update slot.
    pub fn reset(&mut self) {
        self.accepted_count = 0;
        self.accepted_by_underlying_identity.clear();
        self.history_order.clear();
        self.start_model_update();
    }

    /// `evaluate` (`emission-guard.ts:91-126`) — the six ordered steps documented at the top of this
    /// module.
    pub fn evaluate(&mut self, warning: &WatchdogWarning) -> WatchdogEmissionDecision {
        // (1) content-free, before any identity exists.
        if is_content_free(&warning.summary)
            || is_content_free(&warning.evidence)
            || is_content_free(&warning.recommended_action)
        {
            return WatchdogEmissionDecision::Rejected {
                reason: WatchdogEmissionSuppressionReason::ContentFree,
                identity: None,
                underlying_identity: None,
            };
        }

        // (2) identities + escalation.
        let underlying_identity =
            watchdog_warning_underlying_identity(&warning.summary, &warning.evidence);
        let identity =
            watchdog_warning_identity(warning.severity, &warning.summary, &warning.evidence);
        let prior_severity = self.accepted_by_underlying_identity.get(&underlying_identity).copied();
        let escalation = prior_severity == Some(WatchdogSeverity::Concern)
            && warning.severity == WatchdogSeverity::Blocker;

        // (3) update budget — one warning per model update, escalation of THIS update excepted.
        if let Some(update_identity) = self.update_accepted_underlying_identity.as_deref() {
            let update_escalation = update_identity == underlying_identity
                && self.update_accepted_severity == Some(WatchdogSeverity::Concern)
                && warning.severity == WatchdogSeverity::Blocker;
            if !update_escalation {
                return WatchdogEmissionDecision::Rejected {
                    reason: WatchdogEmissionSuppressionReason::UpdateBudget,
                    identity: Some(identity),
                    underlying_identity: Some(underlying_identity),
                };
            }
        }

        // (4) duplicate.
        if prior_severity.is_some() && !escalation {
            return WatchdogEmissionDecision::Rejected {
                reason: WatchdogEmissionSuppressionReason::Duplicate,
                identity: Some(identity),
                underlying_identity: Some(underlying_identity),
            };
        }

        // (5) ceiling — bypassed by an escalation.
        if let Some(max) = self.max_warnings
            && self.accepted_count >= max
            && !escalation
        {
            return WatchdogEmissionDecision::Rejected {
                reason: WatchdogEmissionSuppressionReason::MaxWarnings,
                identity: Some(identity),
                underlying_identity: Some(underlying_identity),
            };
        }

        // (6) accept.
        self.accepted_by_underlying_identity
            .insert(underlying_identity.clone(), warning.severity);
        // `if (!priorSeverity)` — an escalation re-writes the map entry without consuming another
        // slot of the ceiling or another history entry.
        if prior_severity.is_none() {
            self.accepted_count = self.accepted_count.saturating_add(1);
            self.history_order.push(underlying_identity.clone());
        }
        // Evict the oldest identities past the limit. Deliberately does NOT decrement
        // `accepted_count`: the ceiling counts warnings ever accepted, not identities remembered.
        while self.history_order.len() > self.dedupe_history_limit {
            let stale = self.history_order.remove(0);
            if !stale.is_empty() {
                self.accepted_by_underlying_identity.remove(&stale);
            }
        }
        self.update_accepted_underlying_identity = Some(underlying_identity.clone());
        self.update_accepted_severity = Some(warning.severity);
        WatchdogEmissionDecision::Accepted {
            identity,
            underlying_identity,
            escalation,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    fn warn(severity: WatchdogSeverity, summary: &str) -> WatchdogWarning {
        WatchdogWarning::new(severity, summary, "evidence text", "do the thing")
    }

    #[test]
    fn normalization_strips_apostrophes_before_collapsing_punctuation() {
        assert_eq!(normalize_watchdog_emission_text("Don't  STOP!"), "dont stop");
        assert_eq!(normalize_watchdog_emission_text("don\u{2019}t"), "dont");
        assert_eq!(normalize_watchdog_emission_text("N/A"), "n a");
        assert_eq!(normalize_watchdog_emission_text("   ---   "), "");
        // NFKC folds the compatibility ligature and the fullwidth digit.
        assert_eq!(normalize_watchdog_emission_text("\u{FB01}le \u{FF11}"), "file 1");
    }

    #[test]
    fn content_free_is_checked_before_identities_exist() {
        let mut guard = WatchdogEmissionGuard::default();
        let decision = guard.evaluate(&WatchdogWarning::new(
            WatchdogSeverity::Concern,
            "LGTM",
            "evidence",
            "action",
        ));
        assert_eq!(
            decision,
            WatchdogEmissionDecision::Rejected {
                reason: WatchdogEmissionSuppressionReason::ContentFree,
                identity: None,
                underlying_identity: None,
            }
        );
        // ... and it consumed neither the update slot nor the count: a real warning still lands.
        assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, "real finding")).accepted());
    }

    #[test]
    fn update_budget_is_checked_before_duplicate_and_before_max_warnings() {
        let mut guard = WatchdogEmissionGuard::new(WatchdogEmissionGuardOptions {
            max_warnings: Some(1),
            dedupe_history_limit: None,
        });
        assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, "first")).accepted());
        // A SECOND, DISTINCT warning inside the same update: upstream reports `update-budget`,
        // not `max-warnings`, because step 3 precedes step 5.
        match guard.evaluate(&warn(WatchdogSeverity::Concern, "second")) {
            WatchdogEmissionDecision::Rejected { reason, identity, .. } => {
                assert_eq!(reason, WatchdogEmissionSuppressionReason::UpdateBudget);
                assert!(identity.is_some(), "non-content-free rejections carry identities");
            }
            other => panic!("expected update-budget rejection, got {other:?}"),
        }
        // A REPEAT of the accepted warning inside the same update is also `update-budget`, not
        // `duplicate` — same ordering.
        match guard.evaluate(&warn(WatchdogSeverity::Concern, "first")) {
            WatchdogEmissionDecision::Rejected { reason, .. } => {
                assert_eq!(reason, WatchdogEmissionSuppressionReason::UpdateBudget);
            }
            other => panic!("expected update-budget rejection, got {other:?}"),
        }
    }

    #[test]
    fn escalation_within_one_update_is_the_sole_budget_exception() {
        let mut guard = WatchdogEmissionGuard::default();
        assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, "same finding")).accepted());
        let decision = guard.evaluate(&warn(WatchdogSeverity::Blocker, "same finding"));
        assert_eq!(
            decision,
            WatchdogEmissionDecision::Accepted {
                identity: watchdog_warning_identity(
                    WatchdogSeverity::Blocker,
                    "same finding",
                    "evidence text"
                ),
                underlying_identity: watchdog_warning_underlying_identity(
                    "same finding",
                    "evidence text"
                ),
                escalation: true,
            }
        );
        // A blocker->concern move is NOT an escalation: it is a duplicate on the next update.
        guard.start_model_update();
        match guard.evaluate(&warn(WatchdogSeverity::Concern, "same finding")) {
            WatchdogEmissionDecision::Rejected { reason, .. } => {
                assert_eq!(reason, WatchdogEmissionSuppressionReason::Duplicate);
            }
            other => panic!("expected duplicate rejection, got {other:?}"),
        }
    }

    #[test]
    fn escalation_does_not_consume_another_slot_of_the_ceiling() {
        let mut guard = WatchdogEmissionGuard::new(WatchdogEmissionGuardOptions {
            max_warnings: Some(1),
            dedupe_history_limit: None,
        });
        assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, "one")).accepted());
        assert!(guard.evaluate(&warn(WatchdogSeverity::Blocker, "one")).accepted());
        guard.start_model_update();
        // The ceiling is still 1/1 — the escalation re-wrote the entry rather than adding one.
        match guard.evaluate(&warn(WatchdogSeverity::Concern, "two")) {
            WatchdogEmissionDecision::Rejected { reason, .. } => {
                assert_eq!(reason, WatchdogEmissionSuppressionReason::MaxWarnings);
            }
            other => panic!("expected max-warnings rejection, got {other:?}"),
        }
    }

    #[test]
    fn history_eviction_shrinks_dedup_memory_without_refunding_the_ceiling() {
        let mut guard = WatchdogEmissionGuard::new(WatchdogEmissionGuardOptions {
            max_warnings: Some(3),
            dedupe_history_limit: Some(2),
        });
        for name in ["a", "b", "c"] {
            guard.start_model_update();
            assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, name)).accepted(), "{name}");
        }
        // "a" was evicted from the dedup map (limit 2), so it is no longer a `duplicate` — but the
        // ceiling has been consumed three times, so it now rejects as `max-warnings`.
        guard.start_model_update();
        match guard.evaluate(&warn(WatchdogSeverity::Concern, "a")) {
            WatchdogEmissionDecision::Rejected { reason, .. } => {
                assert_eq!(reason, WatchdogEmissionSuppressionReason::MaxWarnings);
            }
            other => panic!("expected max-warnings rejection, got {other:?}"),
        }
        // "b" is still remembered, so it is a `duplicate` — which proves the eviction was ordered
        // oldest-first rather than wholesale.
        guard.start_model_update();
        match guard.evaluate(&warn(WatchdogSeverity::Concern, "b")) {
            WatchdogEmissionDecision::Rejected { reason, .. } => {
                assert_eq!(reason, WatchdogEmissionSuppressionReason::Duplicate);
            }
            other => panic!("expected duplicate rejection, got {other:?}"),
        }
    }

    #[test]
    fn reset_clears_count_history_and_the_update_slot() {
        let mut guard = WatchdogEmissionGuard::new(WatchdogEmissionGuardOptions {
            max_warnings: Some(1),
            dedupe_history_limit: None,
        });
        assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, "one")).accepted());
        guard.reset();
        assert!(guard.evaluate(&warn(WatchdogSeverity::Concern, "one")).accepted());
    }

    #[test]
    fn identity_is_severity_prefixed_and_underlying_identity_is_not() {
        let concern = watchdog_warning_identity(WatchdogSeverity::Concern, "S", "E");
        let blocker = watchdog_warning_identity(WatchdogSeverity::Blocker, "S", "E");
        assert_ne!(concern, blocker);
        assert_eq!(
            watchdog_warning_underlying_identity("S", "E"),
            watchdog_warning_underlying_identity("s!!!", "  e  ")
        );
    }
}
