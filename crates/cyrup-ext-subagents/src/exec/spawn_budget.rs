//! SUBA-046 — the per-session subagent spawn budget: the Rust port of pi-subagents'
//! `src/runs/shared/spawn-budget.ts` (in-baseline at v0.43.0 and present at v0.47.1).
//!
//! # The defect this closes
//!
//! cyrup already counted spawns and refused past the cap, but the cap was TERMINAL: there was no
//! grant path, no snapshot, and no way to see how much budget was left. Upstream's design is that
//! the cap is a speed bump with an explicitly confirmed grant behind it — the refusal text even
//! says so ("Grant budget explicitly from the root interactive session"). Worse, cyrup's own
//! child-safe tool description already ADVERTISED `grant-spawn-budget` to the model
//! (`extension.rs`'s `CHILD_SAFE_SUBAGENT_TOOL_DESCRIPTION`), so a model that read the description
//! and called the verb landed on the unknown-action arm.
//!
//! # Two behaviours that came across with it, and were absent before
//!
//! 1. **`0` means UNLIMITED, not "refuse everything".** `resolveMaxSubagentSpawnsPerSession`
//!    (`shared/types.ts:1970-1975`) maps a configured `0` to `undefined`, and `preflightSpawnBudget`
//!    treats a `null` limit as no cap at all. cyrup compared `used + requested > max_spawns`
//!    against the raw config value, so `"maxSubagentSpawnsPerSession": 0` refused the FIRST
//!    delegation of every session with "0/0 used".
//! 2. **The env override.** Upstream reads `PI_SUBAGENT_MAX_SPAWNS_PER_SESSION` FIRST and lets it
//!    win over config, including its own `0 ⇒ unlimited` mapping. cyrup had no counterpart; ported
//!    here into this crate's `CYRUP_SUBAGENT_*` naming family with the `PI_` name retained as the
//!    documented compatibility alias, matching how every other env var in this crate is renamed.
//!
//! # Shape
//!
//! [`SpawnBudgetCounters`] is pi's `state.subagentSpawns` (`shared/types.ts:842`) and lives in the
//! extension's `Mutex`; every function here is a pure transformation over it, exactly as upstream's
//! are over `state`. [`SpawnBudgetSnapshot`] is pi's `SpawnBudgetSnapshot` (`:940-948`), serialized
//! into a tool result's `details.spawnBudget` so the cap is observable even without a grant.

use serde::{Deserialize, Serialize};

/// pi `MAX_GRANT_HISTORY` (`spawn-budget.ts:8`) — the grant log is a bounded tail, so a session
/// that grants repeatedly cannot grow the state without bound.
pub const MAX_GRANT_HISTORY: usize = 20;

/// pi `PI_SUBAGENT_MAX_SPAWNS_PER_SESSION` (`shared/types.ts:1971`), in this crate's `CYRUP_`
/// naming family.
pub const MAX_SPAWNS_PER_SESSION_ENV: &str = "CYRUP_SUBAGENT_MAX_SPAWNS_PER_SESSION";

/// The upstream spelling of [`MAX_SPAWNS_PER_SESSION_ENV`], honoured as a compatibility alias so a
/// pi user's existing environment keeps working (the same aliasing convention the rest of this
/// crate's env surface uses).
pub const MAX_SPAWNS_PER_SESSION_ENV_PI_ALIAS: &str = "PI_SUBAGENT_MAX_SPAWNS_PER_SESSION";

/// One recorded grant (pi `SpawnBudgetGrant`, `shared/types.ts`), kept so `details.spawnBudget`
/// can show what was granted, when, and against which limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnBudgetGrant {
    /// The session the grant was applied to (pi's `string`; a headless session has no id and
    /// cannot grant at all, so this is never empty in practice).
    pub session_id: String,
    /// Launches added by this grant.
    pub amount: u32,
    /// Epoch milliseconds at which the grant was applied.
    pub granted_at: i64,
    /// The effective limit BEFORE this grant.
    pub previous_limit: u32,
    /// The effective limit after it.
    pub limit: u32,
}

/// pi `SubagentState.subagentSpawns` (`shared/types.ts:842`) — one session's live counters.
///
/// `session_id` is the session `count` was accumulated under (pi's `string | null`, so a
/// headless/unpersisted session is a legitimate identity that still accumulates), and
/// `configured_limit` is the RESOLVED cap for that session — `None` meaning unlimited, which is
/// what upstream's `null` means everywhere below.
#[derive(Debug, Default, Clone)]
pub struct SpawnBudgetCounters {
    /// The session these counters belong to.
    pub session_id: Option<String>,
    /// Spawns already billed to this session.
    pub count: u32,
    /// The resolved configured cap, `None` = unlimited.
    pub configured_limit: Option<u32>,
    /// Launches added by explicit grants.
    pub granted: u32,
    /// The bounded grant log.
    pub grant_history: Vec<SpawnBudgetGrant>,
}

/// pi `SpawnBudgetSnapshot` (`shared/types.ts:940-948`) — the read-only projection attached to
/// tool results as `details.spawnBudget`. `None` fields are upstream's `null`s (unlimited).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpawnBudgetSnapshot {
    /// Spawns billed so far.
    pub used: u32,
    /// The configured cap before grants; `None` = unlimited.
    pub configured_limit: Option<u32>,
    /// Launches added by grants (always 0 when unlimited, as upstream forces).
    pub granted: u32,
    /// The effective cap (`configured_limit + granted`); `None` = unlimited.
    pub limit: Option<u32>,
    /// Launches still available; `None` = unlimited.
    pub remaining: Option<u32>,
    /// How much MORE may still be granted — upstream caps total grants at the original configured
    /// limit, so this is `configured_limit - granted`; `None` = unlimited.
    pub grant_remaining: Option<u32>,
    /// The bounded grant log.
    pub grant_history: Vec<SpawnBudgetGrant>,
}

/// pi `resolveMaxSubagentSpawnsPerSession` (`shared/types.ts:1970-1975`): the env override wins
/// over config, and `0` on EITHER surface means unlimited (`undefined`), never "refuse everything".
///
/// A non-numeric or negative value is ignored on both surfaces (pi's `normalizeNonNegativeInteger`
/// returns `undefined`), so a typo'd env var falls through to config rather than disabling the cap.
#[must_use]
pub fn resolve_max_spawns_per_session(configured: u32) -> Option<u32> {
    let from_env = std::env::var(MAX_SPAWNS_PER_SESSION_ENV)
        .or_else(|_| std::env::var(MAX_SPAWNS_PER_SESSION_ENV_PI_ALIAS))
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok());
    match from_env {
        Some(0) => None,
        Some(value) => Some(value),
        None if configured == 0 => None,
        None => Some(configured),
    }
}

/// pi `sessionState` (`spawn-budget.ts:10-27`): re-key the counters when the session changed, and
/// (re)resolve the configured limit. Returns the counters ready for use.
///
/// Resetting on a session change is what makes a long-lived process start a second session with a
/// fresh budget, and it is the reason `configured_limit` is stored rather than passed at every
/// call: a grant has to survive the next `reserve`, which only knows the config value.
pub fn session_state(
    counters: &mut SpawnBudgetCounters,
    session_id: Option<&str>,
    configured: u32,
) {
    let session_id = session_id.map(str::to_string);
    if counters.session_id != session_id {
        *counters = SpawnBudgetCounters {
            session_id,
            count: 0,
            configured_limit: resolve_max_spawns_per_session(configured),
            granted: 0,
            grant_history: Vec::new(),
        };
        return;
    }
    // pi `if (counters.configuredLimit === undefined) counters.configuredLimit = resolve(…)`: an
    // already-keyed session keeps the limit it was created with, so a config reload mid-session
    // cannot silently move a cap a grant was measured against.
    if counters.configured_limit.is_none() && counters.granted == 0 && counters.count == 0 {
        counters.configured_limit = resolve_max_spawns_per_session(configured);
    }
}

/// pi `getSpawnBudgetSnapshot` (`spawn-budget.ts:29-47`).
#[must_use]
pub fn snapshot(counters: &SpawnBudgetCounters) -> SpawnBudgetSnapshot {
    let configured_limit = counters.configured_limit;
    // pi: `granted = configuredLimit === null ? 0 : counters.granted` — an unlimited session
    // reports no grants even if one was somehow recorded, so the two fields cannot disagree.
    let granted = configured_limit.map_or(0, |_| counters.granted);
    let limit = configured_limit.map(|configured| configured.saturating_add(granted));
    SpawnBudgetSnapshot {
        used: counters.count,
        configured_limit,
        granted,
        limit,
        remaining: limit.map(|limit| limit.saturating_sub(counters.count)),
        grant_remaining: configured_limit.map(|configured| configured.saturating_sub(granted)),
        grant_history: counters.grant_history.clone(),
    }
}

/// pi `formatSpawnBudgetSummary` (`spawn-budget.ts:49-52`).
#[must_use]
pub fn format_spawn_budget_summary(snapshot: &SpawnBudgetSnapshot) -> String {
    let (Some(limit), Some(configured), Some(remaining), Some(grant_remaining)) = (
        snapshot.limit,
        snapshot.configured_limit,
        snapshot.remaining,
        snapshot.grant_remaining,
    ) else {
        return "unlimited".to_string();
    };
    format!(
        "{}/{limit} used, {remaining} remaining (configured {configured}; granted {}; grant \
         allowance {grant_remaining})",
        snapshot.used, snapshot.granted
    )
}

/// pi `formatSpawnBudget` (`spawn-budget.ts:54-56`).
#[must_use]
pub fn format_spawn_budget(snapshot: &SpawnBudgetSnapshot) -> String {
    format!("Spawn budget: {}", format_spawn_budget_summary(snapshot))
}

/// pi `preflightSpawnBudget` (`spawn-budget.ts:58-70`) — check `requested` against the effective
/// cap without charging it. `Err` carries pi's verbatim refusal text.
///
/// # Errors
///
/// The over-limit notice when the declared run does not fit in what remains.
pub fn preflight_spawn_budget(
    snapshot: &SpawnBudgetSnapshot,
    requested: u32,
) -> Result<(), String> {
    let (Some(limit), Some(remaining)) = (snapshot.limit, snapshot.remaining) else {
        return Ok(());
    };
    if requested == 0 || requested <= remaining {
        return Ok(());
    }
    Err(format!(
        "Subagent spawn limit reached for this session ({}/{limit} used, {requested} requested). \
         {remaining} remaining; the declared run cannot fit, so no children were started. Grant \
         budget explicitly from the root interactive session or start a new session.",
        snapshot.used
    ))
}

/// pi `preflightSpawnBudgetGrant` (`spawn-budget.ts:85-105`) — validate a grant WITHOUT applying
/// it, so the confirmation prompt can show the real numbers first.
///
/// # Errors
///
/// pi's three verbatim refusals: a non-positive `additional`, a session with no configured cap
/// (nothing to grant against), and a grant larger than the remaining grant allowance.
pub fn preflight_spawn_budget_grant(
    snapshot: &SpawnBudgetSnapshot,
    additional: i64,
) -> Result<u32, String> {
    let Ok(additional) = u32::try_from(additional) else {
        return Err(
            "action='grant-spawn-budget' requires additional to be a positive integer.".to_string(),
        );
    };
    if additional == 0 {
        return Err(
            "action='grant-spawn-budget' requires additional to be a positive integer.".to_string(),
        );
    }
    let (Some(_configured), Some(_limit)) = (snapshot.configured_limit, snapshot.limit) else {
        return Err(
            "The current session has no configured spawn cap, so it does not need a budget grant."
                .to_string(),
        );
    };
    let grant_remaining = snapshot.grant_remaining.unwrap_or(0);
    if additional > grant_remaining {
        return Err(format!(
            "Spawn budget grant rejected: {additional} requested but only {grant_remaining} of the \
             original configured limit remains grantable."
        ));
    }
    Ok(additional)
}

/// pi `grantSpawnBudget` (`spawn-budget.ts:107-127`) — apply a validated grant and return the new
/// snapshot.
///
/// # Errors
///
/// Re-runs [`preflight_spawn_budget_grant`] against the CURRENT snapshot (pi does the same), so a
/// grant that became invalid between preview and application is refused rather than applied.
pub fn grant_spawn_budget(
    counters: &mut SpawnBudgetCounters,
    additional: i64,
    now: i64,
) -> Result<SpawnBudgetSnapshot, String> {
    let before = snapshot(counters);
    let additional = preflight_spawn_budget_grant(&before, additional)?;
    let Some(previous_limit) = before.limit else {
        return Err(
            "The current session has no configured spawn cap, so it does not need a budget grant."
                .to_string(),
        );
    };
    counters.granted = counters.granted.saturating_add(additional);
    counters.grant_history.push(SpawnBudgetGrant {
        session_id: counters.session_id.clone().unwrap_or_default(),
        amount: additional,
        granted_at: now,
        previous_limit,
        limit: previous_limit.saturating_add(additional),
    });
    if counters.grant_history.len() > MAX_GRANT_HISTORY {
        // pi `.slice(-MAX_GRANT_HISTORY)`: keep the newest entries, drop the oldest.
        let excess = counters.grant_history.len() - MAX_GRANT_HISTORY;
        counters.grant_history.drain(0..excess);
    }
    Ok(snapshot(counters))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn capped(limit: u32, used: u32) -> SpawnBudgetCounters {
        SpawnBudgetCounters {
            session_id: Some("session-a".to_string()),
            count: used,
            configured_limit: Some(limit),
            granted: 0,
            grant_history: Vec::new(),
        }
    }

    /// pi `shared/types.ts:1970-1975` — a configured `0` is UNLIMITED. cyrup compared against the
    /// raw config value, so `0` refused the first delegation of every session.
    #[test]
    fn a_configured_zero_cap_means_unlimited_not_refuse_everything() {
        assert_eq!(resolve_max_spawns_per_session(0), None);
        assert_eq!(resolve_max_spawns_per_session(40), Some(40));

        let mut counters = SpawnBudgetCounters::default();
        session_state(&mut counters, Some("session-a"), 0);
        let snap = snapshot(&counters);
        assert_eq!(snap.limit, None, "no cap at all");
        assert_eq!(format_spawn_budget_summary(&snap), "unlimited");
        assert!(preflight_spawn_budget(&snap, 1_000).is_ok());
    }

    /// pi `preflightSpawnBudget` (`spawn-budget.ts:58-70`) — the refusal text names the numbers and
    /// points at the grant path, which now exists.
    #[test]
    fn an_over_cap_request_is_refused_with_pis_verbatim_text() {
        let snap = snapshot(&capped(2, 2));
        let err = preflight_spawn_budget(&snap, 1).expect_err("the cap is reached");
        assert_eq!(
            err,
            "Subagent spawn limit reached for this session (2/2 used, 1 requested). 0 remaining; \
             the declared run cannot fit, so no children were started. Grant budget explicitly \
             from the root interactive session or start a new session."
        );
        // pi's comparison lets a request land EXACTLY on the cap.
        assert!(preflight_spawn_budget(&snapshot(&capped(2, 1)), 1).is_ok());
        // `requested == 0` never touches the budget (pi's `requested <= 0` short-circuit).
        assert!(preflight_spawn_budget(&snap, 0).is_ok());
    }

    /// THE user-facing behaviour SUBA-046 exists for: an exhausted cap is a speed bump, not the end
    /// of the session. pi `grantSpawnBudget` (`spawn-budget.ts:107-127`).
    #[test]
    fn a_grant_reopens_an_exhausted_cap_and_is_itself_bounded() {
        let mut counters = capped(2, 2);
        assert!(preflight_spawn_budget(&snapshot(&counters), 1).is_err());

        let after = grant_spawn_budget(&mut counters, 2, 1_700_000_000_000)
            .expect("a grant within the allowance applies");
        assert_eq!(after.limit, Some(4));
        assert_eq!(after.remaining, Some(2));
        assert_eq!(after.granted, 2);
        // Total grants may never exceed the ORIGINAL configured limit, so the allowance is now 0.
        assert_eq!(after.grant_remaining, Some(0));
        assert!(preflight_spawn_budget(&after, 2).is_ok());

        let err = grant_spawn_budget(&mut counters, 1, 1_700_000_000_001)
            .expect_err("the grant allowance is exhausted");
        assert_eq!(
            err,
            "Spawn budget grant rejected: 1 requested but only 0 of the original configured limit \
             remains grantable."
        );

        assert_eq!(after.grant_history.len(), 1);
        assert_eq!(after.grant_history[0].previous_limit, 2);
        assert_eq!(after.grant_history[0].limit, 4);
        assert_eq!(after.grant_history[0].granted_at, 1_700_000_000_000);
    }

    /// pi `preflightSpawnBudgetGrant`'s first two refusals (`spawn-budget.ts:88-96`).
    #[test]
    fn a_non_positive_grant_and_an_uncapped_session_are_both_refused_with_pis_text() {
        let capped_snapshot = snapshot(&capped(2, 0));
        for bad in [0_i64, -1] {
            assert_eq!(
                preflight_spawn_budget_grant(&capped_snapshot, bad).expect_err("refused"),
                "action='grant-spawn-budget' requires additional to be a positive integer."
            );
        }

        let mut uncapped = SpawnBudgetCounters::default();
        session_state(&mut uncapped, Some("session-a"), 0);
        assert_eq!(
            preflight_spawn_budget_grant(&snapshot(&uncapped), 1).expect_err("refused"),
            "The current session has no configured spawn cap, so it does not need a budget grant."
        );
    }

    /// pi `sessionState` (`spawn-budget.ts:10-27`) — a session change resets EVERYTHING, including
    /// grants, so one session's grant cannot leak into the next.
    #[test]
    fn a_session_change_resets_the_counters_and_the_grants() {
        let mut counters = capped(2, 2);
        grant_spawn_budget(&mut counters, 1, 1).expect("grant applies");
        assert_eq!(counters.granted, 1);

        session_state(&mut counters, Some("session-b"), 2);
        assert_eq!(counters.count, 0);
        assert_eq!(counters.granted, 0);
        assert!(counters.grant_history.is_empty());
        assert_eq!(counters.configured_limit, Some(2));
    }

    /// pi `MAX_GRANT_HISTORY` (`spawn-budget.ts:8`) — the log is a bounded tail keeping the NEWEST
    /// entries.
    #[test]
    fn the_grant_history_is_a_bounded_tail_of_the_newest_grants() {
        let mut counters = capped(100, 0);
        for n in 0..(MAX_GRANT_HISTORY as i64 + 5) {
            counters.granted = 0; // keep the allowance open; only the history is under test here
            grant_spawn_budget(&mut counters, 1, n).expect("grant applies");
        }
        assert_eq!(counters.grant_history.len(), MAX_GRANT_HISTORY);
        assert_eq!(
            counters.grant_history[MAX_GRANT_HISTORY - 1].granted_at,
            MAX_GRANT_HISTORY as i64 + 4,
            "the newest grant survives"
        );
    }

    /// The snapshot is what a tool result carries as `details.spawnBudget`, so its wire shape is
    /// pi's camelCase one.
    #[test]
    fn the_snapshot_serializes_with_pis_camel_case_keys() {
        let value = serde_json::to_value(snapshot(&capped(2, 1))).expect("serializes");
        for key in ["used", "configuredLimit", "granted", "limit", "remaining", "grantRemaining", "grantHistory"] {
            assert!(value.get(key).is_some(), "missing '{key}' in {value}");
        }
    }
}
