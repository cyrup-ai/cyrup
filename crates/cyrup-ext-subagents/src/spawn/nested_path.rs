//! Nested-run path addressing — a faithful port of pi-subagents'
//! `src/runs/shared/nested-path.ts`.
//!
//! A "nested path" is the ancestry chain of a nested subagent run, encoded into a child's
//! environment (`CYRUP_SUBAGENT_PARENT_PATH`) so a grandparent can reconstruct which of its
//! descendants a relayed event belongs to. Every id along the path must survive
//! [`is_safe_nested_path_id`] (no path separators, no `..`, bounded length) before it is trusted,
//! and the chain is capped at [`MAX_NESTED_PATH_ENTRIES`] entries.

use std::path::Path;

/// Maximum length (in characters) of a safe nested id token (pi `MAX_NESTED_ID_LENGTH`).
const MAX_NESTED_ID_LENGTH: usize = 128;

/// Maximum number of ancestry entries retained in a nested path (pi `MAX_NESTED_PATH_ENTRIES`).
pub const MAX_NESTED_PATH_ENTRIES: usize = 4;

/// One entry in a nested run's ancestry chain (pi `NestedPathEntry`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct NestedPathEntry {
    /// The ancestor run's id.
    pub run_id: String,
    /// The step index within that ancestor that spawned the next hop, if known.
    #[serde(rename = "stepIndex", default, skip_serializing_if = "Option::is_none")]
    pub step_index: Option<i64>,
    /// The agent name at that hop, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
}

// Serde field names must be camelCase to match pi's on-the-wire JSON.
impl NestedPathEntry {
    fn from_json(value: &serde_json::Value) -> Option<Self> {
        let obj = value.as_object()?;
        let run_id = obj.get("runId")?;
        let run_id = run_id.as_str()?;
        if !is_safe_nested_path_id_str(run_id) {
            return None;
        }
        Some(Self {
            run_id: run_id.to_string(),
            step_index: obj.get("stepIndex").and_then(finite_number),
            agent: obj.get("agent").and_then(|v| non_empty_string(v, 128)),
        })
    }

    fn to_json(&self) -> serde_json::Value {
        let mut map = serde_json::Map::new();
        map.insert("runId".to_string(), serde_json::Value::String(self.run_id.clone()));
        if let Some(step_index) = self.step_index {
            map.insert("stepIndex".to_string(), serde_json::Value::from(step_index));
        }
        if let Some(agent) = &self.agent {
            map.insert("agent".to_string(), serde_json::Value::String(agent.clone()));
        }
        serde_json::Value::Object(map)
    }
}

/// pi `isSafeNestedPathId`: a non-empty, bounded string with no path separators and no `..`.
#[must_use]
pub fn is_safe_nested_path_id_str(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NESTED_ID_LENGTH
        && !Path::new(value).is_absolute()
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains("..")
}

/// pi `isSafeNestedPathId` for an arbitrary JSON value (the type-guard form): true only for a
/// string that passes [`is_safe_nested_path_id_str`].
#[must_use]
pub fn is_safe_nested_id(value: &serde_json::Value) -> bool {
    value.as_str().is_some_and(is_safe_nested_path_id_str)
}

fn finite_number(value: &serde_json::Value) -> Option<i64> {
    // pi `finiteNumber` accepts any finite JS number; nested step indices are integers on the wire.
    if let Some(i) = value.as_i64() {
        Some(i)
    } else {
        value.as_f64().filter(|f| f.is_finite()).map(|f| f as i64)
    }
}

fn non_empty_string(value: &serde_json::Value, max: usize) -> Option<String> {
    value
        .as_str()
        .filter(|s| !s.is_empty())
        .map(|s| s.chars().take(max).collect())
}

/// pi `sanitizeNestedPath`: keep only well-formed entries, capped at [`MAX_NESTED_PATH_ENTRIES`].
#[must_use]
pub fn sanitize_nested_path(value: &serde_json::Value) -> Vec<NestedPathEntry> {
    let serde_json::Value::Array(items) = value else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(NestedPathEntry::from_json)
        .take(MAX_NESTED_PATH_ENTRIES)
        .collect()
}

/// pi `parseNestedPathEnv`: parse a `CYRUP_SUBAGENT_PARENT_PATH` env value into sanitized entries.
#[must_use]
pub fn parse_nested_path_env(value: Option<&str>) -> Vec<NestedPathEntry> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Vec::new();
    };
    match serde_json::from_str::<serde_json::Value>(value) {
        Ok(parsed) => sanitize_nested_path(&parsed),
        Err(_) => Vec::new(),
    }
}

/// pi `encodeNestedPathEnv`: encode entries back to the env string form (empty when nothing
/// survives sanitization).
#[must_use]
pub fn encode_nested_path_env(value: &[NestedPathEntry]) -> String {
    let json = serde_json::Value::Array(value.iter().map(NestedPathEntry::to_json).collect());
    let sanitized = sanitize_nested_path(&json);
    if sanitized.is_empty() {
        return String::new();
    }
    let array = serde_json::Value::Array(sanitized.iter().map(NestedPathEntry::to_json).collect());
    serde_json::to_string(&array).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

    use super::*;

    #[test]
    fn safe_id_rejects_separators_dotdot_and_empty() {
        assert!(is_safe_nested_path_id_str("run-abc"));
        assert!(!is_safe_nested_path_id_str(""));
        assert!(!is_safe_nested_path_id_str("../unsafe"));
        assert!(!is_safe_nested_path_id_str("a/b"));
        assert!(!is_safe_nested_path_id_str("a\\b"));
        assert!(!is_safe_nested_path_id_str(&"x".repeat(129)));
    }

    #[test]
    fn sanitize_drops_unsafe_entries_and_caps_length() {
        let value = serde_json::json!([
            { "runId": "root-run", "stepIndex": 0, "agent": "root-agent" },
            { "runId": "../unsafe", "stepIndex": 1, "agent": "bad" },
            { "runId": "a", "stepIndex": 2 },
            { "runId": "b" },
            { "runId": "c" },
            { "runId": "d" },
        ]);
        let entries = sanitize_nested_path(&value);
        assert_eq!(entries.len(), MAX_NESTED_PATH_ENTRIES);
        assert_eq!(entries[0].run_id, "root-run");
        assert_eq!(entries[0].step_index, Some(0));
        assert_eq!(entries[0].agent.as_deref(), Some("root-agent"));
        // "../unsafe" is dropped, so the second surviving entry is "a".
        assert_eq!(entries[1].run_id, "a");
    }

    #[test]
    fn env_round_trip_preserves_sanitized_entries() {
        let entries = vec![
            NestedPathEntry { run_id: "root-run".into(), step_index: Some(0), agent: Some("root".into()) },
            NestedPathEntry { run_id: "child".into(), step_index: Some(2), agent: None },
        ];
        let encoded = encode_nested_path_env(&entries);
        let decoded = parse_nested_path_env(Some(&encoded));
        assert_eq!(decoded, entries);
    }

    #[test]
    fn parse_env_ignores_invalid_json_and_empty() {
        assert!(parse_nested_path_env(None).is_empty());
        assert!(parse_nested_path_env(Some("")).is_empty());
        assert!(parse_nested_path_env(Some("{not json")).is_empty());
    }
}
