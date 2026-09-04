//! The subagent-child env-hint contract.

use crate::extension::env::has_subagent_env_hint;
use crate::extension::{CHILD_ENV_VAR, SUBAGENT_ENV_HINT_KEYS};

/// PERM-001 (second gap). pi ORs the three [`SUBAGENT_ENV_HINT_KEYS`] on ANY non-empty value
/// (`index.ts:93-103`, `permission-forwarding.ts:9`). The pre-fix predicate was a strict
/// `std::env::var(CHILD_ENV_VAR) == Some("1")` on ONE key, so every case below except the very
/// first classified a real subagent child as a ROOT — which wires the LOCAL ask dialog into a
/// process with no human attached, and its `ask` dies there instead of forwarding to the
/// parent's spool.
#[test]
fn subagent_env_hint_ors_any_non_empty_value_across_all_three_keys() {
    let hint = |pairs: &[(&str, &str)]| {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect();
        has_subagent_env_hint(|key| owned.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()))
    };

    // The one case the old strict `== Some("1")` predicate already got right.
    assert!(hint(&[(CHILD_ENV_VAR, "1")]));

    // pi tests `entry.length > 0`, not equality with "1" — any non-empty value is a hint.
    assert!(hint(&[(CHILD_ENV_VAR, "0")]));
    assert!(hint(&[(CHILD_ENV_VAR, "true")]));
    assert!(hint(&[(CHILD_ENV_VAR, "yes")]));

    // ...and either of the two sibling keys alone is enough (pi's OR over three keys).
    assert!(hint(&[(SUBAGENT_ENV_HINT_KEYS[1], "run-abc123")]));
    assert!(hint(&[(SUBAGENT_ENV_HINT_KEYS[2], "reviewer")]));

    // Negatives: nothing set, and set-but-blank (pi trims before the length test).
    assert!(!hint(&[]));
    assert!(!hint(&[(CHILD_ENV_VAR, "")]));
    assert!(!hint(&[(CHILD_ENV_VAR, "   ")]));
    assert!(!hint(&[
        (CHILD_ENV_VAR, ""),
        (SUBAGENT_ENV_HINT_KEYS[1], "  "),
        (SUBAGENT_ENV_HINT_KEYS[2], "\t"),
    ]));

    // An unrelated var is never a hint.
    assert!(!hint(&[("CYRUP_SOMETHING_ELSE", "1")]));
}

/// The hint keys are exactly the strings `cyrup-ext-subagents` writes into a child's spawn
/// overlay. Pinned so a rename on either side fails here rather than silently producing a gate
/// that never recognizes a child (aliasing already prevents drift for two of the three; this
/// pins the resulting VALUES, which are also the cross-crate contract).
#[test]
fn subagent_env_hint_keys_match_the_spawn_overlay_contract() {
    assert_eq!(
        SUBAGENT_ENV_HINT_KEYS,
        [
            "CYRUP_SUBAGENT_CHILD",
            "CYRUP_SUBAGENT_RUN_ID",
            "CYRUP_SUBAGENT_AGENT_NAME"
        ]
    );
    assert_eq!(CHILD_ENV_VAR, SUBAGENT_ENV_HINT_KEYS[0]);
}
