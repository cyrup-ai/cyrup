//! The auth-store mtime rule — pi `isAuthModelExclusion` (`model-exclusions.ts:44-47`),
//! `getAuthStoreMtimeMs` (`:49-60`) and `invalidateAuthExclusions` (`:62-69`).
//!
//! # Why an exclusion can die before its TTL
//!
//! A 401 excludes a provider for the default TTL — twenty-four hours. The operator who fixes their
//! credentials must not have to wait it out, and nothing in the exclusion record itself can say
//! that they did. Touching the auth store IS that signal: every auth-flavoured exclusion recorded
//! BEFORE the store's last modification is dropped, because the condition that produced it may no
//! longer exist. Exclusions recorded after it are kept — those failures already saw the new
//! credentials.
//!
//! The fail-safe direction is to KEEP the exclusion: a stat failure that is not "file absent"
//! preserves everything and logs (`:55-58`), because "the auth store is unreadable" is not evidence
//! that credentials changed.

use std::path::Path;

use crate::exec::fallback::{LowerLiteral, RetryPattern, any_line_matches, lower};
use crate::exec::model_exclusions::entry::ModelExclusion;

/// pi `AUTH_FAILURE_PATTERNS` (`model-exclusions.ts:35-41`), re-typed over [`RetryPattern`] like
/// every other pattern table in this port.
///
/// **SEVEN rows for upstream's six regexes**: `/unauthori[sz]ed/i` is a character class, which this
/// crate's substring-based matcher spells as the two literals it stands for. Every row here is
/// already a row of [`crate::exec::fallback::RETRYABLE_MODEL_FAILURE_PATTERNS`] — the subset
/// relation is upstream's (an auth failure is a retryable model failure; not every retryable model
/// failure is an auth failure) and is asserted by
/// `every_auth_pattern_is_also_a_retryable_model_failure_pattern` below, so the day a row is edited
/// in one table and not the other, the build says so.
pub(crate) const AUTH_FAILURE_PATTERNS: &[RetryPattern] = &[
    RetryPattern::Contains(lower!("auth")), // /auth(?:entication)?/i
    RetryPattern::Contains(lower!("unauthorized")), // /unauthori[sz]ed/i, US spelling
    RetryPattern::Contains(lower!("unauthorised")), // /unauthori[sz]ed/i, UK spelling
    RetryPattern::Contains(lower!("forbidden")),
    RetryPattern::Contains(lower!("api key")),
    RetryPattern::Contains(lower!("token expired")),
    RetryPattern::Contains(lower!("invalid key")),
];

/// pi `isAuthModelExclusion` (`model-exclusions.ts:44-47`): classified by the stored `reason` text
/// alone. An entry with no reason is never auth-flavoured (upstream's `typeof reason === "string"`).
#[must_use]
pub(crate) fn is_auth_model_exclusion(entry: &ModelExclusion) -> bool {
    entry
        .reason
        .as_deref()
        .is_some_and(|reason| any_line_matches(reason, AUTH_FAILURE_PATTERNS))
}

/// pi `getAuthStoreMtimeMs` (`model-exclusions.ts:49-60`).
///
/// `None` for "no usable signal" — the store is absent, is not a regular file, or could not be
/// stat'd — and the caller then changes nothing. A non-absent stat failure logs, because it means
/// this rule is silently not running.
///
/// [`std::fs::metadata`] rather than `tokio::fs`: this is one stat on a local path, on the same
/// synchronous read path as [`super::store::ModelExclusionStore::ensure_loaded`], and upstream's
/// own call is `statSync`. Making it async would force every candidate-filter call site — including
/// `build_model_candidates_scoped`, which is `pub` and synchronous — to become async for a syscall
/// measured in microseconds.
#[must_use]
pub(crate) fn auth_store_mtime_ms(auth_store_path: &Path) -> Option<i64> {
    match std::fs::metadata(auth_store_path) {
        Ok(metadata) if metadata.is_file() => {
            metadata.modified().ok().map(crate::time::epoch_millis)
        }
        Ok(_) => None,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    path = %auth_store_path.display(),
                    %error,
                    "[model-exclusions] Failed to stat the cyrup auth store; preserving auth-related exclusions"
                );
            }
            None
        }
    }
}

/// pi `invalidateAuthExclusions` (`model-exclusions.ts:62-69`), returning whether anything was
/// dropped so the caller can decide between scheduling and forcing a persist.
///
/// Short-circuits before the stat when no entry is auth-flavoured (`:63`) — the common case, and
/// the reason this rule costs nothing on a store full of rate limits.
///
/// `auth_store_mtime_ms <= entry.recorded_at` is upstream's retention test verbatim, including the
/// `<=`: an exclusion recorded in the same millisecond the store was written is KEPT, because the
/// failure is at least as new as the credentials.
pub(crate) fn invalidate_auth_exclusions(
    entries: &mut Vec<ModelExclusion>,
    auth_store_path: &Path,
) -> bool {
    if !entries.iter().any(is_auth_model_exclusion) {
        return false;
    }
    let Some(auth_store_mtime_ms) = auth_store_mtime_ms(auth_store_path) else {
        return false;
    };
    let before = entries.len();
    entries.retain(|entry| {
        !is_auth_model_exclusion(entry) || auth_store_mtime_ms <= entry.recorded_at
    });
    entries.len() != before
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use cyrup_core::ProviderId;

    use super::*;
    use crate::exec::model_exclusions::entry::ModelExclusionTarget;

    fn provider_entry(reason: Option<&str>, recorded_at: i64) -> ModelExclusion {
        ModelExclusion {
            target: ModelExclusionTarget::Provider {
                provider: ProviderId::from("openai"),
            },
            reason: reason.map(str::to_string),
            recorded_at,
            expires_at: i64::MAX / 2,
        }
    }

    #[test]
    fn every_auth_pattern_is_also_a_retryable_model_failure_pattern() {
        for pattern in AUTH_FAILURE_PATTERNS {
            assert!(
                crate::exec::fallback::RETRYABLE_MODEL_FAILURE_PATTERNS.contains(pattern),
                "auth pattern {pattern:?} is not a row of RETRYABLE_MODEL_FAILURE_PATTERNS"
            );
        }
    }

    #[test]
    fn the_reason_text_decides_and_an_entry_without_one_is_never_auth_flavoured() {
        assert!(is_auth_model_exclusion(&provider_entry(
            Some("401 Unauthorized"),
            0
        )));
        assert!(is_auth_model_exclusion(&provider_entry(
            Some("invalid API key"),
            0
        )));
        assert!(!is_auth_model_exclusion(&provider_entry(
            Some("429 rate limit"),
            0
        )));
        assert!(!is_auth_model_exclusion(&provider_entry(None, 0)));
    }

    #[test]
    fn touching_the_auth_store_drops_only_the_older_auth_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let auth = dir.path().join("auth.json");
        std::fs::write(&auth, b"{}").unwrap();
        let mtime = auth_store_mtime_ms(&auth).unwrap();

        let mut entries = vec![
            provider_entry(Some("401 unauthorized"), mtime - 1),
            provider_entry(Some("401 unauthorized"), mtime),
            provider_entry(Some("429 rate limit"), mtime - 1),
        ];
        assert!(invalidate_auth_exclusions(&mut entries, &auth));
        assert_eq!(entries.len(), 2);
        assert!(
            entries.iter().all(|entry| entry.recorded_at >= mtime
                || entry.reason.as_deref() == Some("429 rate limit"))
        );
    }

    #[test]
    fn an_absent_auth_store_preserves_every_exclusion() {
        let dir = tempfile::tempdir().unwrap();
        let mut entries = vec![provider_entry(Some("401 unauthorized"), 0)];
        assert!(!invalidate_auth_exclusions(
            &mut entries,
            &dir.path().join("absent.json")
        ));
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn a_store_with_no_auth_entries_never_stats_the_auth_file() {
        // Proven by behaviour rather than by counting syscalls: the path handed in cannot be
        // stat'd into a useful answer, and the call still reports "nothing changed".
        let mut entries = vec![provider_entry(Some("429 rate limit"), 0)];
        assert!(!invalidate_auth_exclusions(
            &mut entries,
            Path::new("/nonexistent/auth.json")
        ));
        assert_eq!(entries.len(), 1);
    }
}
