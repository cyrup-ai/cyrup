//! SUBA-064 — the `subagents.authorityPolicy` gate: a port of `pi-subagents/src/policy/authority.ts`
//! (present at BOTH v0.43.0 and v0.47.1, so this is unported baseline work, not drift).
//!
//! The policy names six privileged actions and, per action, one of three decisions — `auto` (just
//! do it), `confirm` (ask the user first) or `forbid` (refuse outright). Upstream consults it from
//! four sites at v0.43.0 (`subagent-executor.ts:4358` discardWorktree, `:4491` spawnBudgetGrant,
//! `worktree.ts:607`, `herdr/actions.ts:205-206`) and — the reason this item is not dormant —
//! from `subagent-executor.ts:4412-4423`, which maps `stop`→`stopRun` and `steer`→`steerRun`
//! immediately before dispatching them.
//!
//! Before this module existed, an operator who wrote `"authorityPolicy": {"stopRun": "forbid"}`
//! into `config.json` had the key silently dropped by serde — `registration/mod.rs`'s only
//! validator was `validate_missions` — and the action ran anyway. Accepted, unvalidated, inert.
//!
//! # Severity note, carried from the item so it is not rediscovered
//!
//! This is `medium` only because of WHICH actions cyrup can currently dispatch: of upstream's six
//! `AUTHORITY_ACTIONS`, only `stopRun` and `steerRun` are implemented here. `discardWorktree`,
//! `destructiveCleanup`, `spawnBudgetGrant` and `scheduleCreate` have no dispatch to bypass.
//! **Whoever lands `worktree.discard` or `destructiveCleanup` must wire them through
//! [`resolve_authority_decision`] in the same change** — shipping a destructive verb behind a
//! config key that is parsed and ignored is a permission bypass.

use serde::{Deserialize, Serialize};

/// pi `AUTHORITY_ACTIONS` (`policy/authority.ts:1-8`), in upstream's own order — which is the order
/// [`validate_authority_policy`]'s error message enumerates.
pub const AUTHORITY_ACTIONS: &[&str] = &[
    "discardWorktree",
    "destructiveCleanup",
    "spawnBudgetGrant",
    "scheduleCreate",
    "stopRun",
    "steerRun",
];

/// pi `AuthorityAction` (`policy/authority.ts:10`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthorityAction {
    DiscardWorktree,
    DestructiveCleanup,
    SpawnBudgetGrant,
    ScheduleCreate,
    StopRun,
    SteerRun,
}

impl AuthorityAction {
    /// The wire name this action is keyed by in `config.json`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DiscardWorktree => "discardWorktree",
            Self::DestructiveCleanup => "destructiveCleanup",
            Self::SpawnBudgetGrant => "spawnBudgetGrant",
            Self::ScheduleCreate => "scheduleCreate",
            Self::StopRun => "stopRun",
            Self::SteerRun => "steerRun",
        }
    }

    /// pi `subagent-executor.ts:4412` @v0.43.0:
    /// `action === "stop" ? "stopRun" : action === "steer" ? "steerRun" : action ===
    /// "schedule.create" ? "scheduleCreate" : undefined`.
    ///
    /// `schedule.create` is mapped here even though cyrup does not dispatch it yet (SUBA-016), so
    /// the verb arrives already gated rather than needing a second change to gate it.
    #[must_use]
    pub fn for_tool_action(action: &str) -> Option<Self> {
        match action {
            "stop" => Some(Self::StopRun),
            "steer" => Some(Self::SteerRun),
            "schedule.create" => Some(Self::ScheduleCreate),
            _ => None,
        }
    }

    /// pi `DEFAULT_AUTHORITY_POLICY` (`policy/authority.ts:14-21`): the three privileged/destructive
    /// actions default to `confirm`, the three ordinary ones to `auto`.
    #[must_use]
    pub fn default_decision(self) -> AuthorityDecision {
        match self {
            Self::DiscardWorktree | Self::DestructiveCleanup | Self::SpawnBudgetGrant => {
                AuthorityDecision::Confirm
            }
            Self::ScheduleCreate | Self::StopRun | Self::SteerRun => AuthorityDecision::Auto,
        }
    }
}

/// pi `AuthorityDecision` (`policy/authority.ts:11`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AuthorityDecision {
    /// Perform the action with no gate.
    Auto,
    /// Ask the user first; refuse when there is no interactive UI to ask through.
    Confirm,
    /// Refuse outright.
    Forbid,
}

/// pi `AuthorityPolicyConfig = Partial<Record<AuthorityAction, AuthorityDecision>>`
/// (`policy/authority.ts:12`). Every key optional; an absent key falls back to
/// [`AuthorityAction::default_decision`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorityPolicyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discard_worktree: Option<AuthorityDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_cleanup: Option<AuthorityDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spawn_budget_grant: Option<AuthorityDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule_create: Option<AuthorityDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_run: Option<AuthorityDecision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steer_run: Option<AuthorityDecision>,
}

impl AuthorityPolicyConfig {
    /// The configured decision for one action, or `None` when the key was omitted.
    #[must_use]
    pub fn get(&self, action: AuthorityAction) -> Option<AuthorityDecision> {
        match action {
            AuthorityAction::DiscardWorktree => self.discard_worktree,
            AuthorityAction::DestructiveCleanup => self.destructive_cleanup,
            AuthorityAction::SpawnBudgetGrant => self.spawn_budget_grant,
            AuthorityAction::ScheduleCreate => self.schedule_create,
            AuthorityAction::StopRun => self.stop_run,
            AuthorityAction::SteerRun => self.steer_run,
        }
    }
}

/// pi `resolveAuthorityDecision` (`policy/authority.ts:23-28`):
/// `input.policy?.[input.action] ?? DEFAULT_AUTHORITY_POLICY[input.action]`.
#[must_use]
pub fn resolve_authority_decision(
    action: AuthorityAction,
    policy: Option<&AuthorityPolicyConfig>,
) -> AuthorityDecision {
    policy
        .and_then(|p| p.get(action))
        .unwrap_or_else(|| action.default_decision())
}

/// pi `validateAuthorityPolicy` (`policy/authority.ts:30-45`), applied to the RAW config JSON.
///
/// Serde alone accepts an unknown action key and a bad decision string and silently drops both,
/// which is precisely the "accepted, unvalidated and inert" shape SUBA-064 names; upstream THROWS
/// on either. Both message templates are upstream's, verbatim, including the `label` parameter that
/// lets a nested caller name its own path.
///
/// # Errors
///
/// `<label> must be a JSON object`; `<label>.<action> is unknown; expected one of <list>`;
/// `<label>.<action> must be "auto", "confirm", or "forbid"`.
pub fn validate_authority_policy(
    value: Option<&serde_json::Value>,
    label: &str,
) -> Result<(), String> {
    let Some(value) = value else {
        return Ok(());
    };
    // pi's guard is `!value || typeof value !== "object" || Array.isArray(value)`, so an explicit
    // JSON `null` is refused too (`!null` is true in JS).
    let Some(policy) = value.as_object().filter(|_| !value.is_null()) else {
        return Err(format!("{label} must be a JSON object"));
    };
    for (action, decision) in policy {
        if !AUTHORITY_ACTIONS.contains(&action.as_str()) {
            return Err(format!(
                "{label}.{action} is unknown; expected one of {}",
                AUTHORITY_ACTIONS.join(", ")
            ));
        }
        if !matches!(decision.as_str(), Some("auto" | "confirm" | "forbid")) {
            return Err(format!(
                "{label}.{action} must be \"auto\", \"confirm\", or \"forbid\""
            ));
        }
    }
    Ok(())
}

/// pi `subagent-executor.ts:4415` — the `forbid` refusal text, verbatim. `action` is the TOOL verb
/// (`stop`/`steer`), not the policy key, matching upstream's `${action}` interpolation.
#[must_use]
pub fn forbidden_message(action: &str) -> String {
    format!("Authority policy forbids action '{action}'.")
}

/// pi `subagent-executor.ts:4419` — the no-UI refusal, verbatim.
#[must_use]
pub fn no_ui_message(action: &str) -> String {
    format!(
        "Authority policy requires user confirmation for action '{action}', but this session has \
         no interactive UI."
    )
}

/// pi `subagent-executor.ts:4420` — `ctx.ui.confirm(title, message)`'s title.
#[must_use]
pub fn confirm_prompt(action: &str) -> String {
    format!("Authorize subagent {action}?")
}

/// pi `subagent-executor.ts:4420` — `ctx.ui.confirm(title, message)`'s body.
#[must_use]
pub fn confirm_message(action: &str) -> String {
    format!("Authority policy requires confirmation before '{action}'.")
}

/// pi `subagent-executor.ts:4421` — the DECLINED text. Note upstream returns this WITHOUT
/// `isError: true`, unlike the forbid/no-UI refusals: declining a confirmation is a user choice,
/// not a failure.
#[must_use]
pub fn declined_message(action: &str) -> String {
    format!("Action '{action}' canceled; authority was not granted.")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// pi `DEFAULT_AUTHORITY_POLICY` (`policy/authority.ts:14-21`), pinned element by element: the
    /// three privileged actions default to `confirm` and the three ordinary ones to `auto`. Getting
    /// this backwards would either gate every stop behind a dialog or let a worktree discard run
    /// unasked.
    #[test]
    fn the_default_policy_is_pis_own() {
        assert_eq!(
            AuthorityAction::DiscardWorktree.default_decision(),
            AuthorityDecision::Confirm
        );
        assert_eq!(
            AuthorityAction::DestructiveCleanup.default_decision(),
            AuthorityDecision::Confirm
        );
        assert_eq!(
            AuthorityAction::SpawnBudgetGrant.default_decision(),
            AuthorityDecision::Confirm
        );
        assert_eq!(
            AuthorityAction::ScheduleCreate.default_decision(),
            AuthorityDecision::Auto
        );
        assert_eq!(AuthorityAction::StopRun.default_decision(), AuthorityDecision::Auto);
        assert_eq!(AuthorityAction::SteerRun.default_decision(), AuthorityDecision::Auto);
    }

    /// The tool-verb → policy-key mapping pi performs at `subagent-executor.ts:4412`. Anything else
    /// is ungated, which is why the `None` arm is asserted too — a mapping that accidentally
    /// matched, say, `status` would put a read-only verb behind a confirmation dialog.
    #[test]
    fn only_stop_steer_and_schedule_create_map_to_a_policy_action() {
        assert_eq!(
            AuthorityAction::for_tool_action("stop"),
            Some(AuthorityAction::StopRun)
        );
        assert_eq!(
            AuthorityAction::for_tool_action("steer"),
            Some(AuthorityAction::SteerRun)
        );
        assert_eq!(
            AuthorityAction::for_tool_action("schedule.create"),
            Some(AuthorityAction::ScheduleCreate)
        );
        for ungated in ["status", "interrupt", "resume", "append-step", "list", "delete"] {
            assert_eq!(AuthorityAction::for_tool_action(ungated), None, "{ungated}");
        }
    }

    /// SUBA-064's core: `policy?.[action] ?? DEFAULT[action]`.
    ///
    /// THE USER ACTION: an operator writes `"authorityPolicy": {"stopRun": "forbid"}`. Before this
    /// module the key was silently dropped and `{action:"stop", id}` ran anyway — a policy surface
    /// a user can configure and that does nothing.
    #[test]
    fn a_configured_decision_overrides_the_default_and_an_absent_one_does_not() {
        let policy = AuthorityPolicyConfig {
            stop_run: Some(AuthorityDecision::Forbid),
            ..AuthorityPolicyConfig::default()
        };
        assert_eq!(
            resolve_authority_decision(AuthorityAction::StopRun, Some(&policy)),
            AuthorityDecision::Forbid
        );
        // An unconfigured action keeps its default even when the block exists.
        assert_eq!(
            resolve_authority_decision(AuthorityAction::SteerRun, Some(&policy)),
            AuthorityDecision::Auto
        );
        assert_eq!(
            resolve_authority_decision(AuthorityAction::DiscardWorktree, Some(&policy)),
            AuthorityDecision::Confirm
        );
        // No block at all is the same as an empty one.
        assert_eq!(
            resolve_authority_decision(AuthorityAction::StopRun, None),
            AuthorityDecision::Auto
        );
    }

    /// The wire shape an operator actually writes, round-tripped through serde.
    #[test]
    fn the_policy_block_deserializes_from_pis_camel_case_keys() {
        let policy: AuthorityPolicyConfig = serde_json::from_value(serde_json::json!({
            "stopRun": "forbid",
            "steerRun": "confirm",
            "discardWorktree": "auto"
        }))
        .expect("policy parses");
        assert_eq!(policy.stop_run, Some(AuthorityDecision::Forbid));
        assert_eq!(policy.steer_run, Some(AuthorityDecision::Confirm));
        assert_eq!(policy.discard_worktree, Some(AuthorityDecision::Auto));
        assert_eq!(policy.destructive_cleanup, None);
    }

    /// pi `validateAuthorityPolicy` (`policy/authority.ts:30-45`) — both typed errors, verbatim.
    /// An unknown action key or a bad decision value must fail config load, not be ignored.
    #[test]
    fn validation_refuses_unknown_actions_and_bad_decisions_with_pis_text() {
        assert!(validate_authority_policy(None, "config.authorityPolicy").is_ok());
        assert!(
            validate_authority_policy(
                Some(&serde_json::json!({ "stopRun": "forbid" })),
                "config.authorityPolicy"
            )
            .is_ok()
        );

        assert_eq!(
            validate_authority_policy(
                Some(&serde_json::json!([])),
                "config.authorityPolicy"
            )
            .expect_err("an array is not an object"),
            "config.authorityPolicy must be a JSON object"
        );
        assert_eq!(
            validate_authority_policy(
                Some(&serde_json::json!({ "stopRunn": "forbid" })),
                "config.authorityPolicy"
            )
            .expect_err("unknown action key"),
            "config.authorityPolicy.stopRunn is unknown; expected one of discardWorktree, \
             destructiveCleanup, spawnBudgetGrant, scheduleCreate, stopRun, steerRun"
        );
        assert_eq!(
            validate_authority_policy(
                Some(&serde_json::json!({ "stopRun": "maybe" })),
                "config.authorityPolicy"
            )
            .expect_err("bad decision"),
            "config.authorityPolicy.stopRun must be \"auto\", \"confirm\", or \"forbid\""
        );
        // A non-string decision fails the same way (pi's `decision !== "auto" && ...`).
        assert_eq!(
            validate_authority_policy(
                Some(&serde_json::json!({ "stopRun": true })),
                "config.authorityPolicy"
            )
            .expect_err("bad decision"),
            "config.authorityPolicy.stopRun must be \"auto\", \"confirm\", or \"forbid\""
        );
    }

    /// The four model-visible strings, pinned by equality against upstream's own templates
    /// (`subagent-executor.ts:4415,4419,4420,4421` @v0.43.0). The `${action}` interpolation is the
    /// TOOL verb, not the policy key — `'stop'`, never `'stopRun'`.
    #[test]
    fn the_refusal_texts_are_pis_own() {
        assert_eq!(
            forbidden_message("stop"),
            "Authority policy forbids action 'stop'."
        );
        assert_eq!(
            no_ui_message("steer"),
            "Authority policy requires user confirmation for action 'steer', but this session has \
             no interactive UI."
        );
        assert_eq!(confirm_prompt("stop"), "Authorize subagent stop?");
        assert_eq!(
            confirm_message("stop"),
            "Authority policy requires confirmation before 'stop'."
        );
        assert_eq!(
            declined_message("stop"),
            "Action 'stop' canceled; authority was not granted."
        );
    }
}
