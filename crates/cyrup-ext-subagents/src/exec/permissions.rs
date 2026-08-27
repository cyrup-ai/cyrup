//! SUBA-073 — the PARENT half of `pi-subagents/src/runs/shared/permissions.ts` (99 lines
//! @v0.43.0): resolving `config.permissions` + an agent's own frontmatter rules into the policy a
//! spawned child receives. The CHILD half (decode, per-tool decision, audit) is already ported at
//! [`crate::watchdog::permission_arbiter`] — this file reuses it rather than re-deriving it; see
//! that module's own doc for why the split exists.

use serde_json::Value;

use crate::watchdog::permission_arbiter::{
    PermissionRuleDecision, PermissionRules, validate_permission_rules,
};

/// `validatePermissionConfig(value, label)` (`permissions.ts:35-41`) — validates the CONFIG-LEVEL
/// `{rules?}` wrapper (`config.permissions`), distinct from [`validate_permission_rules`], which
/// validates a bare rules map (agent frontmatter has no wrapper). Returns the inner rules map
/// directly since `{rules?}` carries nothing else.
///
/// # Errors
///
/// `{label} must be an object.` for a non-object/array/null value; `{label} has unsupported
/// fields: <sorted, comma-joined>.` for any key besides `rules`; everything
/// [`validate_permission_rules`] itself raises for `.rules`.
///
/// [CYRUP-DELTA] the unsupported-fields list is SORTED here for deterministic error text; upstream
/// preserves JS object key insertion order. Cosmetic only — the set of named keys is identical.
pub fn validate_permission_config(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<PermissionRules>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    let mut unknown: Vec<&str> =
        object.keys().map(String::as_str).filter(|k| *k != "rules").collect();
    unknown.sort_unstable();
    if !unknown.is_empty() {
        return Err(format!("{label} has unsupported fields: {}.", unknown.join(", ")));
    }
    validate_permission_rules(object.get("rules"), &format!("{label}.rules"))
}

/// `resolvePermissionRules(globalConfig?, agentRules?)` (`permissions.ts:44-47`) — merge global and
/// agent-level rules, agent wins on conflict, then strip any `Allow` entries (the enforcement
/// default anyway — [`crate::watchdog::permission_arbiter::permission_decision`] falls back to
/// `Allow` for anything absent, so keeping an explicit `Allow` on the wire is pure overhead and
/// upstream never does).
#[must_use]
pub fn resolve_permission_rules(
    global: Option<&PermissionRules>,
    agent: Option<&PermissionRules>,
) -> Option<PermissionRules> {
    let mut merged: PermissionRules = global.cloned().unwrap_or_default();
    if let Some(agent) = agent {
        merged.extend(agent.iter().map(|(k, v)| (k.clone(), *v)));
    }
    merged.retain(|_, decision| *decision != PermissionRuleDecision::Allow);
    (!merged.is_empty()).then_some(merged)
}

/// pi's 16 KiB cap (`permissions.ts:12`, `MAX_POLICY_BYTES`).
const MAX_POLICY_BYTES: usize = 16 * 1024;

/// `encodePermissionRules(rules)` (`permissions.ts:55-60`) — JSON-encode the resolved rules for
/// [`crate::watchdog::permission_arbiter::PERMISSION_POLICY_ENV`]. `None`/empty encodes to `None`
/// (upstream: no env var written at all, not an empty-object value — see the `spawn_plan.rs` call
/// site, which only inserts the env key when this returns `Some`).
///
/// # Errors
///
/// `Resolved permission policy is too large.` when the encoded JSON exceeds 16 KiB — upstream's
/// verbatim text.
pub fn encode_permission_rules(rules: Option<&PermissionRules>) -> Result<Option<String>, String> {
    let Some(rules) = rules.filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let encoded = serde_json::to_string(rules)
        .map_err(|e| format!("failed to encode permission policy: {e}"))?;
    if encoded.len() > MAX_POLICY_BYTES {
        return Err("Resolved permission policy is too large.".to_string());
    }
    Ok(Some(encoded))
}

/// Serialize a [`PermissionRules`] map to the compact-JSON string the agent-file frontmatter
/// writer needs (`discovery/management/frontmatter_write.rs`'s round-trip). `PermissionRuleDecision`
/// derives `Serialize` with `rename_all = "lowercase"`, so this is a direct pass-through — kept as
/// its own named function so the frontmatter writer does not need to reach past this module into
/// `watchdog::permission_arbiter` for a plain formatting concern.
#[must_use]
pub fn permission_rules_to_json_string(rules: &PermissionRules) -> String {
    serde_json::to_string(rules).unwrap_or_default()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn rules(pairs: &[(&str, PermissionRuleDecision)]) -> PermissionRules {
        pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect()
    }

    /// `validatePermissionConfig`'s own refusals (`permissions.ts:35-41`), byte-for-byte.
    #[test]
    fn validate_permission_config_rejects_non_objects_and_unknown_fields() {
        assert_eq!(validate_permission_config(None, "config.permissions").unwrap(), None);
        assert_eq!(
            validate_permission_config(Some(&serde_json::json!([])), "config.permissions")
                .unwrap_err(),
            "config.permissions must be an object."
        );
        assert_eq!(
            validate_permission_config(
                Some(&serde_json::json!({"rules": {}, "extra": 1, "another": 2})),
                "config.permissions"
            )
            .unwrap_err(),
            "config.permissions has unsupported fields: another, extra."
        );
    }

    /// The `{rules?}` wrapper unwraps to the SAME validation `validate_permission_rules` already
    /// performs directly — no double-porting, just a different entry shape.
    #[test]
    fn validate_permission_config_delegates_rules_validation() {
        let resolved = validate_permission_config(
            Some(&serde_json::json!({"rules": {"write": "deny"}})),
            "config.permissions",
        )
        .expect("valid");
        assert_eq!(resolved, Some(rules(&[("write", PermissionRuleDecision::Deny)])));

        let err = validate_permission_config(
            Some(&serde_json::json!({"rules": {"bash": "ask"}})),
            "config.permissions",
        )
        .unwrap_err();
        assert_eq!(
            err,
            "config.permissions.rules.bash is unsupported; pi-subagents leaves bash policy to pi-guard."
        );
    }

    /// `resolvePermissionRules` (`permissions.ts:44-47`): agent overrides global on conflict, and
    /// `allow` entries never survive onto the wire (the default anyway).
    #[test]
    fn resolve_permission_rules_merges_agent_over_global_and_strips_allow() {
        let global = rules(&[
            ("write", PermissionRuleDecision::Deny),
            ("edit", PermissionRuleDecision::Ask),
        ]);
        let agent = rules(&[
            ("write", PermissionRuleDecision::Allow),
            ("grep", PermissionRuleDecision::Deny),
        ]);
        let resolved = resolve_permission_rules(Some(&global), Some(&agent)).expect("non-empty");
        assert_eq!(
            resolved,
            rules(&[("edit", PermissionRuleDecision::Ask), ("grep", PermissionRuleDecision::Deny)]),
            "write must be gone (agent set it to allow, which is always stripped)"
        );

        // Neither rung set anything -> None.
        assert_eq!(resolve_permission_rules(None, None), None);
        // Only the global rung, entirely `allow` -> None after stripping.
        assert_eq!(
            resolve_permission_rules(Some(&rules(&[("write", PermissionRuleDecision::Allow)])), None),
            None
        );
    }

    /// `encodePermissionRules` (`permissions.ts:55-60`): `None`/empty -> `None`; a real policy
    /// encodes to compact JSON; oversize is refused with upstream's verbatim text.
    #[test]
    fn encode_permission_rules_encodes_or_refuses_oversize() {
        assert_eq!(encode_permission_rules(None).unwrap(), None);
        let empty = PermissionRules::new();
        assert_eq!(encode_permission_rules(Some(&empty)).unwrap(), None);

        let small = rules(&[("write", PermissionRuleDecision::Deny)]);
        assert_eq!(
            encode_permission_rules(Some(&small)).unwrap(),
            Some(r#"{"write":"deny"}"#.to_string())
        );

        // A policy whose encoded form exceeds 16 KiB is refused outright.
        let huge: PermissionRules = (0..2000)
            .map(|i| (format!("tool_{i:04}_with_a_long_enough_name_to_pad_bytes"), PermissionRuleDecision::Ask))
            .collect();
        assert_eq!(
            encode_permission_rules(Some(&huge)).unwrap_err(),
            "Resolved permission policy is too large."
        );
    }
}
