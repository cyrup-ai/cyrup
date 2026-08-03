//! Read-modify-**write** of the `subagents.agentOverrides.<name>` block of a `settings.json`
//! (SUBA-005) — a port of pi-subagents' three settings writers
//! (`src/agents/agents.ts:1033-1120` at v0.34.0):
//!
//! | pi | here |
//! |---|---|
//! | `mergeBuiltinAgentOverride(cwd, name, scope, fields)` | [`merge_builtin_agent_override`] |
//! | `removeBuiltinAgentOverride(cwd, name, scope)` | [`remove_builtin_agent_override`] |
//! | `removeBuiltinAgentOverrideFields(cwd, name, scope, fields)` | [`remove_builtin_agent_override_fields`] |
//!
//! pi resolves the target path from `(cwd, scope)` internally via
//! `getUserAgentSettingsPath()`/`getProjectAgentSettingsPath(cwd)`; cyrup already carries both
//! resolved paths on [`crate::discovery::types::LayeredOverrideSettings`] (`user_settings_path` /
//! `project_settings_path`), so these functions take the path directly and the scope→path decision
//! stays in the one place that already owns it. That also keeps this module free of any
//! directory-resolution logic of its own, matching `discovery/mod.rs`'s standing rule.
//!
//! **Everything here goes through [`serde_json::Value`], never the typed
//! [`crate::discovery::types::SubagentSettings`]** — and that is load-bearing, not stylistic. A
//! `settings.json` is cyrup's *whole* settings document (`subagents` is one key among many, and the
//! `subagents` block itself may carry keys this crate's version does not know). Round-tripping it
//! through the typed struct would serialize back only the fields that struct declares, silently
//! deleting every unrelated key in the file — turning "disable one agent" into "wipe the user's
//! settings". The untyped path shallow-merges into, or deletes from, exactly one nested object and
//! preserves every sibling byte-for-byte modulo re-serialization.
//!
//! Empty-container pruning mirrors pi exactly: removing the last field of an override entry drops
//! the entry, dropping the last entry drops `agentOverrides`, and dropping the last `subagents` key
//! drops `subagents` — so an enable/reset round-trip leaves a settings file that is byte-identical
//! in structure to one that never had the override (no `"subagents": {}` residue).

use std::path::Path;

use serde_json::{Map, Value};

use crate::error::SubagentError;

/// pi `readSettingsFileStrict` (`agents.ts:551-572`): an **absent** file reads as the empty object
/// (the common "no settings yet" case, not an error); an unreadable file, a file that does not
/// parse as JSON, and a file whose top level is not a JSON object each abort with that function's
/// verbatim message. The messages are shared with `read_subagent_settings_file`'s reader-side
/// equivalents on purpose — a malformed settings file must read the same whether discovery or a
/// management write is the one that noticed.
fn read_settings_file_strict(path: &Path) -> Result<Map<String, Value>, SubagentError> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Map::new()),
        Err(e) => {
            return Err(SubagentError::MalformedSettings(format!(
                "Failed to read settings file '{}': {e}",
                path.display()
            )));
        }
    };
    let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
        SubagentError::MalformedSettings(format!(
            "Failed to parse settings file '{}': {e}",
            path.display()
        ))
    })?;
    match parsed {
        Value::Object(map) => Ok(map),
        _ => Err(SubagentError::MalformedSettings(format!(
            "Settings file '{}' must contain a JSON object.",
            path.display()
        ))),
    }
}

/// pi `writeSettingsFile` (`agents.ts:574-577`): `mkdir -p` the parent, then write
/// `JSON.stringify(settings, null, 2) + "\n"`. The two-space indent and the trailing newline are
/// matched exactly so a cyrup-written settings file is diff-clean against a pi-written one.
fn write_settings_file(path: &Path, settings: &Map<String, Value>) -> Result<(), SubagentError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(SubagentError::Spawn)?;
    }
    let mut body = serde_json::to_string_pretty(&Value::Object(settings.clone()))
        .map_err(|e| SubagentError::Spawn(std::io::Error::other(e)))?;
    body.push('\n');
    std::fs::write(path, body).map_err(SubagentError::Spawn)
}

/// Borrow `settings.subagents.agentOverrides` as an object, if all three levels are objects.
fn overrides_of(settings: &Map<String, Value>) -> Option<&Map<String, Value>> {
    settings.get("subagents")?.as_object()?.get("agentOverrides")?.as_object()
}

/// Re-attach a (possibly now-empty) `agentOverrides` map under `subagents`, pruning empties exactly
/// as pi does: an empty `agentOverrides` is **deleted** rather than written as `{}`, and a
/// `subagents` block left with no keys at all is deleted in turn.
fn store_overrides(settings: &mut Map<String, Value>, next_overrides: Map<String, Value>) {
    let mut subagents = settings
        .get("subagents")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    if next_overrides.is_empty() {
        subagents.remove("agentOverrides");
    } else {
        subagents.insert("agentOverrides".to_string(), Value::Object(next_overrides));
    }
    if subagents.is_empty() {
        settings.remove("subagents");
    } else {
        settings.insert("subagents".to_string(), Value::Object(subagents));
    }
}

/// pi `mergeBuiltinAgentOverride` (`agents.ts:1058-1085`): shallow-merge `fields` into
/// `subagents.agentOverrides.<name>` in the settings file at `path`, creating every missing level,
/// and return that path.
///
/// "Shallow" is pi's semantics verbatim: an existing entry's other fields survive, a same-named
/// field is replaced outright (no recursive merge into a nested value). A non-object value already
/// sitting at `subagents`, `agentOverrides`, or the entry itself is treated as absent and replaced
/// — pi's `typeof x === "object" && !Array.isArray(x)` guards do the same, and discovery already
/// tolerates those shapes rather than erroring on them.
///
/// # Errors
///
/// [`SubagentError::MalformedSettings`] if the file exists but is unreadable / not JSON / not a JSON
/// object; [`SubagentError::Spawn`] on a genuine write failure.
pub fn merge_builtin_agent_override(
    path: &Path,
    name: &str,
    fields: &Map<String, Value>,
) -> Result<(), SubagentError> {
    let mut settings = read_settings_file_strict(path)?;
    let mut next_overrides = overrides_of(&settings).cloned().unwrap_or_default();
    let mut entry = next_overrides.get(name).and_then(Value::as_object).cloned().unwrap_or_default();
    for (key, value) in fields {
        entry.insert(key.clone(), value.clone());
    }
    next_overrides.insert(name.to_string(), Value::Object(entry));
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &settings)
}

/// pi `removeBuiltinAgentOverride` (`agents.ts:1033-1057`): delete the WHOLE
/// `subagents.agentOverrides.<name>` entry. Returns whether anything was actually removed — the
/// caller (`reset`) reports "Removed &lt;scope&gt; settings override at &lt;path&gt;" only on `true`,
/// and pi distinguishes the same way.
///
/// An absent file, an absent/non-object `subagents`, an absent/non-object `agentOverrides`, or an
/// absent entry all return `false` **without writing anything** — a no-op reset must not create or
/// rewrite a settings file.
///
/// # Errors
///
/// As [`merge_builtin_agent_override`].
pub fn remove_builtin_agent_override(path: &Path, name: &str) -> Result<bool, SubagentError> {
    let mut settings = read_settings_file_strict(path)?;
    let Some(overrides) = overrides_of(&settings) else {
        return Ok(false);
    };
    if !overrides.contains_key(name) {
        return Ok(false);
    }
    let mut next_overrides = overrides.clone();
    next_overrides.remove(name);
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &settings)?;
    Ok(true)
}

/// pi `removeBuiltinAgentOverrideFields` (`agents.ts:1086-1120`): delete only the named `fields`
/// from `subagents.agentOverrides.<name>`, leaving the entry's other fields intact — and delete the
/// entry entirely if that emptied it. Returns whether any field was actually present and removed;
/// `false` means nothing was written.
///
/// This is what `enable` uses (removing just `disabled`), so an agent carrying a
/// `{ disabled: true, model: "…" }` override keeps its model override when re-enabled — the
/// distinguishing behavior versus [`remove_builtin_agent_override`], which `reset` uses to clear the
/// entry wholesale.
///
/// # Errors
///
/// As [`merge_builtin_agent_override`].
pub fn remove_builtin_agent_override_fields(
    path: &Path,
    name: &str,
    fields: &[&str],
) -> Result<bool, SubagentError> {
    let mut settings = read_settings_file_strict(path)?;
    let Some(overrides) = overrides_of(&settings) else {
        return Ok(false);
    };
    let Some(entry) = overrides.get(name).and_then(Value::as_object) else {
        return Ok(false);
    };
    let mut next_entry = entry.clone();
    let mut removed = false;
    for field in fields {
        if next_entry.remove(*field).is_some() {
            removed = true;
        }
    }
    if !removed {
        return Ok(false);
    }
    let mut next_overrides = overrides.clone();
    if next_entry.is_empty() {
        next_overrides.remove(name);
    } else {
        next_overrides.insert(name.to_string(), Value::Object(next_entry));
    }
    store_overrides(&mut settings, next_overrides);
    write_settings_file(path, &settings)?;
    Ok(true)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    fn field(key: &str, value: Value) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert(key.to_string(), value);
        m
    }

    fn read(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn merge_creates_every_missing_level_in_an_absent_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("settings.json");
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true))).unwrap();
        assert_eq!(read(&path)["subagents"]["agentOverrides"]["scout"]["disabled"], Value::Bool(true));
    }

    #[test]
    fn merge_preserves_every_unrelated_key_in_the_document() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"theme":"dark","mcpServers":{"a":{"command":"x"}},"subagents":{"defaultModel":"anthropic/m","unknownFutureKey":42,"agentOverrides":{"worker":{"model":"anthropic/w"}}}}"#,
        )
        .unwrap();
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true))).unwrap();
        let after = read(&path);
        // The whole rest of the document survives — this is the failure mode a typed round-trip has.
        assert_eq!(after["theme"], Value::String("dark".to_string()));
        assert_eq!(after["mcpServers"]["a"]["command"], Value::String("x".to_string()));
        assert_eq!(after["subagents"]["defaultModel"], Value::String("anthropic/m".to_string()));
        assert_eq!(after["subagents"]["unknownFutureKey"], Value::Number(42.into()));
        assert_eq!(
            after["subagents"]["agentOverrides"]["worker"]["model"],
            Value::String("anthropic/w".to_string())
        );
        assert_eq!(after["subagents"]["agentOverrides"]["scout"]["disabled"], Value::Bool(true));
    }

    #[test]
    fn merge_is_shallow_and_keeps_the_entrys_other_fields() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"subagents":{"agentOverrides":{"scout":{"model":"anthropic/x"}}}}"#)
            .unwrap();
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true))).unwrap();
        let entry = &read(&path)["subagents"]["agentOverrides"]["scout"];
        assert_eq!(entry["model"], Value::String("anthropic/x".to_string()));
        assert_eq!(entry["disabled"], Value::Bool(true));
    }

    #[test]
    fn remove_fields_keeps_siblings_and_prunes_only_what_it_emptied() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true,"model":"anthropic/x"}}}}"#,
        )
        .unwrap();
        assert!(remove_builtin_agent_override_fields(&path, "scout", &["disabled"]).unwrap());
        let entry = &read(&path)["subagents"]["agentOverrides"]["scout"];
        assert_eq!(entry["model"], Value::String("anthropic/x".to_string()));
        assert!(entry.get("disabled").is_none());
    }

    #[test]
    fn remove_last_field_prunes_entry_overrides_and_subagents() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, r#"{"theme":"dark","subagents":{"agentOverrides":{"scout":{"disabled":true}}}}"#)
            .unwrap();
        assert!(remove_builtin_agent_override_fields(&path, "scout", &["disabled"]).unwrap());
        let after = read(&path);
        assert_eq!(after["theme"], Value::String("dark".to_string()));
        assert!(after.get("subagents").is_none(), "empty subagents block must be pruned: {after}");
    }

    #[test]
    fn remove_entry_prunes_but_leaves_sibling_overrides() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{"subagents":{"agentOverrides":{"scout":{"disabled":true},"worker":{"model":"m"}}}}"#,
        )
        .unwrap();
        assert!(remove_builtin_agent_override(&path, "scout").unwrap());
        let overrides = &read(&path)["subagents"]["agentOverrides"];
        assert!(overrides.get("scout").is_none());
        assert_eq!(overrides["worker"]["model"], Value::String("m".to_string()));
    }

    #[test]
    fn removals_are_no_ops_when_nothing_matches_and_never_create_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        assert!(!remove_builtin_agent_override(&path, "scout").unwrap());
        assert!(!remove_builtin_agent_override_fields(&path, "scout", &["disabled"]).unwrap());
        assert!(!path.exists(), "a no-op removal must not create a settings file");

        std::fs::write(&path, r#"{"subagents":{"agentOverrides":{"scout":{"model":"m"}}}}"#).unwrap();
        assert!(!remove_builtin_agent_override_fields(&path, "scout", &["disabled"]).unwrap());
        assert_eq!(read(&path)["subagents"]["agentOverrides"]["scout"]["model"], Value::String("m".to_string()));
    }

    #[test]
    fn a_malformed_settings_file_aborts_rather_than_being_overwritten() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true)))
            .expect_err("malformed settings must abort");
        assert!(matches!(err, SubagentError::MalformedSettings(_)), "{err}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn written_file_uses_two_space_indent_and_a_trailing_newline() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        merge_builtin_agent_override(&path, "scout", &field("disabled", Value::Bool(true))).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.ends_with("}\n"), "{raw:?}");
        assert!(raw.contains("\n  \"subagents\""), "{raw:?}");
    }
}
