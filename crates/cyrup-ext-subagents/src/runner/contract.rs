//! SUBA-074 — the stage-1 subset of `pi-subagents/src/runs/shared/external-cli-contract.ts`
//! (167 lines @v0.57.0): the code-owned adapter id set, the capability-narrowing parser, and the
//! reserved-selection-name guard. The rest of that file (`resolveExternalCliRunnerStatus`,
//! `normalizeExternalCliRunnerStatus`, `externalCliReceiptMetadata`) is stage 2 and lands with the
//! runner itself.

use std::collections::BTreeMap;

use serde_json::Value;

/// `CODE_OWNED_EXTERNAL_CLI_ADAPTER_IDS` (`external-cli-contract.ts:26-33`), in upstream's order —
/// the order is load-bearing because [`CODE_OWNED_ADAPTER_LABEL`] is built from it and appears
/// verbatim in a user-facing refusal.
pub const CODE_OWNED_ADAPTER_IDS: [&str; 6] = [
    "codex-exec",
    "codex-exec-writer",
    "claude-code",
    "claude-code-writer",
    "cursor-agent",
    "cursor-agent-writer",
];

/// `CODE_OWNED_EXTERNAL_CLI_ADAPTER_LABEL` — each id single-quoted, comma-joined. A `const` string
/// rather than a computed join so the refusal text cannot drift from the array above without the
/// accompanying assertion failing.
pub const CODE_OWNED_ADAPTER_LABEL: &str =
    "'codex-exec', 'codex-exec-writer', 'claude-code', 'claude-code-writer', 'cursor-agent', 'cursor-agent-writer'";

/// `isCodeOwnedExternalCliAdapterId` (`:38-40`).
#[must_use]
pub fn is_code_owned_adapter_id(value: &str) -> bool {
    CODE_OWNED_ADAPTER_IDS.contains(&value)
}

/// The seven narrowable capability keys — `Object.keys(UNSUPPORTED)` (`:8-18`). `stop` is
/// deliberately ABSENT: it is the one capability an external adapter always has, so it is not
/// narrowable and naming it is an unsupported-field error.
const CAPABILITY_KEYS: [&str; 7] = [
    "steer",
    "resume",
    "structuredOutput",
    "toolEvents",
    "supervisor",
    "forkContext",
    "extensionBindings",
];

/// `ExternalCliCapabilityNarrowing` — the parsed narrowing map. Every value is `false` by
/// construction ([`parse_capability_narrowing`] refuses anything else), and the map is kept rather
/// than collapsed to a key set so an explicit empty object round-trips as an empty object rather
/// than as "absent".
pub type ExternalCliCapabilityNarrowing = BTreeMap<String, bool>;

/// `parseExternalCliCapabilityNarrowing(value, label)` (`:65-76`).
///
/// # Errors
///
/// Upstream's three refusals, verbatim: a non-object, an unsupported key, and — the load-bearing
/// one — any value that is not literally `false`, because user config may only ever NARROW a
/// code-owned adapter's capabilities, never widen them.
pub fn parse_capability_narrowing(
    value: Option<&Value>,
    label: &str,
) -> Result<Option<ExternalCliCapabilityNarrowing>, String> {
    let Some(value) = value else {
        return Ok(None);
    };
    let Some(object) = value.as_object() else {
        return Err(format!("{label} must be an object."));
    };
    let unknown: Vec<&str> = object
        .keys()
        .map(String::as_str)
        .filter(|key| !CAPABILITY_KEYS.contains(key))
        .collect();
    if !unknown.is_empty() {
        return Err(format!("{label} has unsupported fields: {}.", unknown.join(", ")));
    }
    let mut narrowed = ExternalCliCapabilityNarrowing::new();
    for (key, setting) in object {
        if setting != &Value::Bool(false) {
            return Err(format!(
                "{label}.{key} may only be false; user config cannot widen code-owned external \
                 adapter capabilities."
            ));
        }
        narrowed.insert(key.clone(), false);
    }
    Ok(Some(narrowed))
}

/// `RESERVED_READ_ONLY_ADAPTERS` (`:42-46`) — `(reserved name, writer twin, access word)`. The
/// access word differs per adapter and is interpolated into the refusal, so the three rows are NOT
/// interchangeable.
const RESERVED_READ_ONLY_ADAPTERS: [(&str, &str, &str); 3] = [
    ("claude-code", "claude-code-writer", "file-write"),
    ("codex-exec", "codex-exec-writer", "workspace-write"),
    ("cursor-agent", "cursor-agent-writer", "workspace-write"),
];

/// `validateCodeOwnedProfileRunner` (`:48-63`) — the reserved-selection-name guard.
///
/// An agent reachable by the selection name `claude-code`/`codex-exec`/`cursor-agent` MUST actually
/// be that read-only adapter. Without this a hand-written agent can squat the reserved name and be
/// selected in place of the sandboxed read-only profile — the same class of silent widening this
/// whole item exists to close, which is why it is stage 1 and not stage 2.
///
/// `selection_names` is upstream's `[name, localName?, ...aliases]`.
#[must_use]
pub fn validate_code_owned_profile_runner(
    selection_names: &[&str],
    runner_adapter: Option<&str>,
) -> Option<String> {
    for (name, writer, access) in RESERVED_READ_ONLY_ADAPTERS {
        if selection_names.contains(&name) && runner_adapter != Some(name) {
            return Some(format!(
                "Selection name '{name}' is reserved for the read-only '{name}' adapter. Use \
                 '{writer}' for explicit {access} access."
            ));
        }
    }
    None
}
