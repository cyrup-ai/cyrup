//! The two renders this module owns: the armed-subscription listing that hangs off the `status`
//! action, and the wake message a settled subscription injects.
//!
//! Ports pi `formatWaitSubscriptions` (`wait-subscriptions.ts:89-97`) and the `pi.sendMessage`
//! object at `:189-199`.

use super::{SubscriptionOutcome, WaitSubscriptionRecord};
use crate::background::wait_completions::WaitCompletion;

/// pi's `customType` for the wake message (`:190`).
pub const SUBSCRIPTION_MESSAGE_CUSTOM_TYPE: &str = "subagent-wait-subscription";

/// pi `formatWaitSubscriptions` (`:89-97`) — the armed listing, or `None` when nothing is armed.
///
/// Three details a paraphrase loses, all of them upstream's and all of them pinned by the tests
/// below:
///
/// * the sort is by `createdAt` ASCENDING (`:90`), not by token and not by expiry;
/// * empty renders as `None`, never `Some("")` — upstream returns `undefined` (`:91`) so the
///   caller's `[a, b].filter(Boolean).join("\n\n")` omits it entirely rather than emitting a
///   trailing blank block;
/// * the remaining time is `Math.max(0, expiresAt - now)` with a bare `ms` suffix (`:94`) — NOT
///   [`crate::background::wait::format_duration`], which would render `1800000` as `30m0s`. An
///   already-expired record therefore reads `0ms`, not a negative number.
///
/// # Session scoping is a property of the CALLER, not of this function
///
/// Upstream reads `state.waitSubscriptions`, whose only two writers are `arm` (`:306`, which
/// stamps `state.currentSessionId` at `:291-292`) and `restore` (`:328`, behind the `:327`
/// session gate). The foreign sweep never inserts. So no filtering happens here and none is
/// needed — but that is an invariant of [`super::WaitSubscriptionManager::restore`], and a
/// regression there would be invisible at this function. Both ends are tested.
#[must_use]
pub fn format_wait_subscriptions(
    subscriptions: &[WaitSubscriptionRecord],
    now: i64,
) -> Option<String> {
    if subscriptions.is_empty() {
        return None;
    }
    let mut sorted: Vec<&WaitSubscriptionRecord> = subscriptions.iter().collect();
    // `sort_by_key` is stable, so records minted within the same millisecond keep the order the
    // caller handed them in rather than being permuted run-to-run.
    sorted.sort_by_key(|record| record.created_at);
    let mut lines = Vec::with_capacity(sorted.len() + 1);
    lines.push(format!("Armed wait subscriptions ({}):", sorted.len()));
    for record in sorted {
        lines.push(format!(
            "- {}: {} run {}, timeout in {}ms",
            record.token,
            record.target_kind,
            record.run_id.as_str(),
            record.expires_at.saturating_sub(now).max(0),
        ));
    }
    Some(lines.join("\n"))
}

/// pi's wake-message `content` (`:191`), verbatim.
#[must_use]
pub fn subscription_message_content(
    record: &WaitSubscriptionRecord,
    outcome: SubscriptionOutcome,
    detail: &str,
) -> String {
    format!(
        "Wait subscription {} fired for run {}: {}. {detail}",
        record.token,
        record.run_id.as_str(),
        outcome.as_str(),
    )
}

/// pi's wake-message `details` (`:193-198`), verbatim — including that `completions` is OMITTED
/// rather than emitted as `[]` when there is nothing to replay (`...(completion ? … : {})`).
#[must_use]
pub fn subscription_message_details(
    record: &WaitSubscriptionRecord,
    outcome: SubscriptionOutcome,
    completion: Option<&WaitCompletion>,
) -> serde_json::Value {
    let mut details = serde_json::json!({
        "token": record.token.as_str(),
        "runId": record.run_id.as_str(),
        "outcome": outcome.as_str(),
    });
    // The literal above is an object; the `let … else` is the crate's no-unwrap discipline
    // (`Cargo.toml:101-102` denies both `unwrap_used` and `expect_used`), not a real branch.
    let Some(map) = details.as_object_mut() else {
        return details;
    };
    if let Some(completion) = completion
        && let Ok(value) = serde_json::to_value(completion)
    {
        map.insert("completions".into(), serde_json::json!([value]));
    }
    details
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::record::{SubscriptionToken, SubscriptionVersion, WaitTargetKind};
    use super::*;
    use crate::background::RunId;
    use crate::identity::SessionId;

    fn record(token: &str, created_at: i64, expires_at: i64) -> WaitSubscriptionRecord {
        WaitSubscriptionRecord {
            version: SubscriptionVersion,
            token: SubscriptionToken::parse(token).unwrap(),
            session_id: SessionId::parse("sess-a").unwrap(),
            target_kind: WaitTargetKind::Async,
            run_id: RunId::from_token("run-1"),
            requested_id: "run".to_string(),
            created_at,
            expires_at,
        }
    }

    const TOKEN_A: &str = "11111111-1111-4111-8111-111111111111";
    const TOKEN_B: &str = "22222222-2222-4222-9222-222222222222";

    /// Empty is `None`, never `Some("")` — otherwise `status`'s join emits a trailing blank block.
    #[test]
    fn nothing_armed_renders_as_none() {
        assert_eq!(format_wait_subscriptions(&[], 0), None);
    }

    /// The exact string, the `createdAt`-ascending order, and the `max(0, …)` clamp.
    #[test]
    fn the_listing_is_sorted_by_created_at_and_clamps_an_expired_record_to_zero() {
        // Handed in newest-first on purpose: the sort, not the caller, decides.
        let subscriptions = vec![
            record(TOKEN_B, 200, 5_000),
            // Already expired: `expiresAt - now` is negative and must render `0ms`.
            record(TOKEN_A, 100, 900),
        ];
        let rendered = format_wait_subscriptions(&subscriptions, 1_000).unwrap();
        assert_eq!(
            rendered,
            format!(
                "Armed wait subscriptions (2):\n- {TOKEN_A}: async run run-1, timeout in 0ms\n- \
                 {TOKEN_B}: async run run-1, timeout in 4000ms"
            )
        );
        // The unit suffix is a bare `ms`, NOT `format_duration`'s `4.0s`.
        assert!(!rendered.contains("4.0s"));
    }

    /// The target kind is rendered with pi's wire word, for both variants.
    #[test]
    fn the_listing_renders_the_target_kind_verbatim() {
        let mut foreground = record(TOKEN_A, 1, 2);
        foreground.target_kind = WaitTargetKind::Foreground;
        let rendered = format_wait_subscriptions(&[foreground], 0).unwrap();
        assert!(rendered.contains("foreground run run-1"), "{rendered}");
    }

    /// The wake message: pi's `content` and `details`, including the OMITTED `completions` key.
    #[test]
    fn the_wake_message_matches_upstreams_shape() {
        let record = record(TOKEN_A, 1, 2);
        assert_eq!(
            subscription_message_content(
                &record,
                SubscriptionOutcome::TimedOut,
                "The targeted run may still be active.",
            ),
            format!(
                "Wait subscription {TOKEN_A} fired for run run-1: timed out. The targeted run may \
                 still be active."
            )
        );
        let details = subscription_message_details(&record, SubscriptionOutcome::Completed, None);
        assert_eq!(details["outcome"], serde_json::json!("completed"));
        assert_eq!(details["runId"], serde_json::json!("run-1"));
        assert!(
            details.get("completions").is_none(),
            "pi omits the key entirely when there is no completion: {details}"
        );

        let completion = WaitCompletion {
            run_id: "run-1".to_string(),
            archive_path: Some("/archives/run-1.json".to_string()),
            ..WaitCompletion::default()
        };
        let details = subscription_message_details(
            &record,
            SubscriptionOutcome::Completed,
            Some(&completion),
        );
        assert_eq!(
            details["completions"][0]["runId"],
            serde_json::json!("run-1")
        );
        assert_eq!(
            details["completions"][0]["archivePath"],
            serde_json::json!("/archives/run-1.json")
        );
    }
}
