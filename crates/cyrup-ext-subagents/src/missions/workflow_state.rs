//! `state.get(key)` / `state.set(key, value)` — a 1:1 port of
//! `pi-subagents/src/missions/workflow-state.ts` (77 lines @v0.43.0).
//!
//! A mission's **workflow state** is a single JSON object persisted at
//! `<missionDir>/<missionId>/state.json`, read lazily and written whole on every `set`. It is the
//! durable scratchpad a long-running mission carries across runs and across processes; upstream
//! exposes it to a `workflowScript` as `await state.get(k)` / `await state.set(k, v)`.
//!
//! Three invariants, all ported:
//!
//! 1. **Key grammar** (`STATE_KEY_PATTERN`, `workflow-state.ts:8`): 1-128 characters, alphanumeric
//!    first, then alphanumerics/`.`/`_`/`-`. Enforced on BOTH `get` and `set`.
//! 2. **256 KiB ceiling** ([`MISSION_STATE_MAX_BYTES`]), checked twice — once against the file as
//!    READ (so an oversized file on disk is refused rather than loaded) and once against the
//!    serialized candidate BEFORE it is written (so an oversized `set` never lands).
//! 3. **Lazy, once-only load.** The file is read on first access and the in-memory map is
//!    authoritative afterwards; a `set` updates the map only after the write succeeds.
//!
//! # Wiring status
//!
//! [`mission_state_path`] has two production callers, both ported:
//! `actions.rs`'s `mission.show` renders it as the `State: <path>` line
//! (`missions/actions.ts:360`), and `goal_driver.rs` reads the file to derive a mission's next
//! ready action (`missions/goal-driver.ts:89`).
//!
//! [`create_mission_workflow_state`] has exactly ONE caller upstream —
//! `runs/foreground/subagent-executor.ts:4139`, inside the `workflowScript` branch — and cyrup has
//! no `workflowScript` runtime at all (the identifier appears nowhere in this crate; see
//! `extension.rs::normalize_public_subagent_execution`'s own note on that gap). It is ported here,
//! in full and with its own tests, so that the `workflowScript` port is a call-site change rather
//! than a second port of this file; it is deliberately NOT wired into a made-up cyrup-only
//! surface, because upstream exposes no other one.
//!
//! # [CYRUP-DELTA] `assertWorkflowJsonValue`
//!
//! Upstream validates with `assertWorkflowJsonValue`
//! (`pi-subagents/src/workflows/scripted-workflow.ts:234-255`), which rejects five things a
//! JavaScript value can be but a JSON value cannot: non-finite numbers, cycles, sparse array
//! entries, non-plain prototypes, and symbol keys. A [`serde_json::Value`] is unable to represent
//! four of those five by construction — it is a tree (no cycles), its arrays are dense, its
//! objects are plain maps, and it has no symbols. [`assert_workflow_json_value`] therefore checks
//! the ONE that survives (finiteness) and documents the rest, rather than pretending to test
//! conditions that are unreachable in this representation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{MissionError, MissionResult, MissionStoreLocation};

/// pi `MISSION_STATE_MAX_BYTES` (`workflow-state.ts:9`): 256 KiB.
pub const MISSION_STATE_MAX_BYTES: usize = 256 * 1024;

/// pi `missionStatePath` (`workflow-state.ts:17-19`): `<missionDir>/<missionId>/state.json`.
///
/// The mission id is a PATH COMPONENT here, so it goes through
/// [`super::store::validate_mission_id_str`] — the same traversal guard
/// [`super::store::mission_record_path`] applies.
///
/// # Errors
///
/// [`MissionError::Invalid`] when `mission_id` is not a valid mission id.
pub fn mission_state_path(
    location: &MissionStoreLocation,
    mission_id: &str,
) -> MissionResult<PathBuf> {
    let id = super::store::validate_mission_id_str(mission_id, "missionId")?;
    Ok(location.mission_dir.join(id).join("state.json"))
}

/// pi `validateStateKey` (`workflow-state.ts:21-26`) — `STATE_KEY_PATTERN`
/// (`^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`) with upstream's exact refusal text.
fn validate_state_key(value: &str) -> MissionResult<&str> {
    let mut chars = value.chars();
    let ok = match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {
            let mut tail = 0usize;
            chars.all(|c| {
                tail += 1;
                tail <= 127 && (c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
            })
        }
        _ => false,
    };
    if ok {
        Ok(value)
    } else {
        Err(MissionError::invalid(
            "state key must be 1-128 characters using letters, numbers, '.', '_' or '-', and \
             start with a letter or number.",
        ))
    }
}

/// pi `assertWorkflowJsonValue` (`workflows/scripted-workflow.ts:234-255`), narrowed to what a
/// [`Value`] can actually express — see this module's `[CYRUP-DELTA]` note.
///
/// # Errors
///
/// [`MissionError::Invalid`] when the value contains a non-finite number.
pub fn assert_workflow_json_value(value: &Value, path: &str) -> MissionResult<()> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(n) => {
            // `serde_json` refuses to construct a non-finite `Number` through its safe API, so
            // this is a belt-and-braces check against a future `arbitrary_precision` build rather
            // than a reachable branch today. Upstream's message is reproduced exactly.
            if n.as_f64().is_none_or(f64::is_finite) {
                Ok(())
            } else {
                Err(MissionError::invalid(format!(
                    "{path} must contain only finite JSON numbers."
                )))
            }
        }
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                assert_workflow_json_value(item, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(map) => {
            for (key, entry) in map {
                assert_workflow_json_value(entry, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
    }
}

/// pi's `MissionWorkflowState` interface (`workflow-state.ts:11-15`) — `{ path, get, set }` over
/// one mission's `state.json`.
#[derive(Debug)]
pub struct MissionWorkflowState {
    /// The file this state is persisted to (upstream's `path` field, read by callers for
    /// diagnostics).
    path: PathBuf,
    /// `undefined` until the first access; upstream's `loaded` flag plus `values` map, collapsed
    /// into one `Option` since "loaded" and "has a map" are the same fact.
    ///
    /// A [`BTreeMap`] rather than `serde_json::Map` so iteration/serialization order is
    /// deterministic regardless of the `preserve_order` feature — the file is rewritten whole on
    /// every `set`, so a stable order keeps the on-disk diff minimal.
    values: Option<BTreeMap<String, Value>>,
}

impl MissionWorkflowState {
    /// The state file's path (upstream's `path` field).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// pi's `load` closure (`workflow-state.ts:33-57`): read once, refuse an oversized or
    /// malformed file, treat a missing file as an empty map.
    fn load(&mut self) -> MissionResult<&BTreeMap<String, Value>> {
        if self.values.is_none() {
            let raw = match std::fs::read_to_string(&self.path) {
                Ok(raw) => raw,
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    self.values = Some(BTreeMap::new());
                    return Ok(self.values.get_or_insert_with(BTreeMap::new));
                }
                Err(err) => {
                    return Err(MissionError::invalid(format!(
                        "Failed to read mission state '{}': {err}",
                        self.path.display()
                    )));
                }
            };
            let bytes = raw.len();
            if bytes > MISSION_STATE_MAX_BYTES {
                return Err(MissionError::invalid(format!(
                    "Mission state file '{}' exceeds the 256 KiB limit ({bytes} bytes).",
                    self.path.display()
                )));
            }
            let parsed: Value = serde_json::from_str(&raw).map_err(|e| {
                MissionError::invalid(format!(
                    "Invalid mission state file '{}': {e}",
                    self.path.display()
                ))
            })?;
            let object = parsed.as_object().ok_or_else(|| {
                MissionError::invalid(format!(
                    "Invalid mission state file '{}': root must be a JSON object",
                    self.path.display()
                ))
            })?;
            assert_workflow_json_value(&parsed, "mission state").map_err(|e| {
                MissionError::invalid(format!(
                    "Invalid mission state file '{}': {e}",
                    self.path.display()
                ))
            })?;
            self.values = Some(
                object.iter().map(|(key, value)| (key.clone(), value.clone())).collect(),
            );
        }
        Ok(self.values.get_or_insert_with(BTreeMap::new))
    }

    /// pi's `get` (`workflow-state.ts:61-65`). A key that is absent yields `None`.
    ///
    /// # Errors
    ///
    /// [`MissionError::Invalid`] for an invalid key, or when the state file cannot be loaded.
    pub fn get(&mut self, key: &str) -> MissionResult<Option<Value>> {
        let valid_key = validate_state_key(key)?.to_string();
        Ok(self.load()?.get(&valid_key).cloned())
    }

    /// pi's `set` (`workflow-state.ts:66-75`): validate the key, validate the value, check the
    /// SERIALIZED size of the candidate map, persist it atomically, and only then adopt it
    /// in memory.
    ///
    /// # Errors
    ///
    /// [`MissionError::Invalid`] for an invalid key/value or an over-budget result;
    /// [`MissionError::Io`] when the write fails.
    pub fn set(&mut self, key: &str, value: Value) -> MissionResult<()> {
        let valid_key = validate_state_key(key)?.to_string();
        assert_workflow_json_value(&value, &format!("state.set('{valid_key}') value"))?;
        let mut next = self.load()?.clone();
        next.insert(valid_key, value);
        // `Buffer.byteLength(JSON.stringify(next, null, 2))` — the PRETTY form is what is measured
        // upstream and what is written, so the same rendering is measured here.
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|e| MissionError::invalid(e.to_string()))?
            .len();
        if bytes > MISSION_STATE_MAX_BYTES {
            return Err(MissionError::invalid(format!(
                "Mission state exceeds the 256 KiB limit ({bytes} bytes; maximum \
                 {MISSION_STATE_MAX_BYTES} bytes)."
            )));
        }
        super::write_private_atomic_json(&self.path, &next)?;
        self.values = Some(next);
        Ok(())
    }
}

/// pi `createMissionWorkflowState` (`workflow-state.ts:28-77`).
///
/// See this module's "Wiring status" note: upstream's only caller is the `workflowScript` runtime,
/// which cyrup does not have.
///
/// # Errors
///
/// [`MissionError::Invalid`] when `mission_id` is not a valid mission id.
pub fn create_mission_workflow_state(
    location: &MissionStoreLocation,
    mission_id: &str,
) -> MissionResult<MissionWorkflowState> {
    Ok(MissionWorkflowState { path: mission_state_path(location, mission_id)?, values: None })
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;

    fn location(root: &Path) -> MissionStoreLocation {
        MissionStoreLocation {
            project_root: root.to_path_buf(),
            mission_dir: root.join("missions"),
            global_index_dir: root.join("index"),
            write_global_index: false,
            retain_terminal: None,
        }
    }

    #[test]
    fn state_path_is_mission_dir_slash_id_slash_state_json() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        assert_eq!(
            mission_state_path(&loc, "m-1").unwrap(),
            tmp.path().join("missions").join("m-1").join("state.json")
        );
        assert!(mission_state_path(&loc, "../escape").is_err());
    }

    #[test]
    fn set_then_get_round_trips_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        assert_eq!(state.get("phase").unwrap(), None);
        state.set("phase", serde_json::json!({"step": 2, "done": false})).unwrap();
        assert_eq!(state.get("phase").unwrap(), Some(serde_json::json!({"step": 2, "done": false})));

        // A SECOND, independent handle sees the persisted value — the file, not the map, is the
        // source of truth across processes.
        let mut reopened = create_mission_workflow_state(&loc, "m-1").unwrap();
        assert_eq!(
            reopened.get("phase").unwrap(),
            Some(serde_json::json!({"step": 2, "done": false}))
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode =
                std::fs::metadata(state.path()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    fn invalid_keys_are_refused_on_both_get_and_set() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        let expected = "state key must be 1-128 characters using letters, numbers, '.', '_' or \
                        '-', and start with a letter or number.";
        assert_eq!(state.get("").unwrap_err().to_string(), expected);
        assert_eq!(state.get("_leading").unwrap_err().to_string(), expected);
        assert_eq!(state.get("has space").unwrap_err().to_string(), expected);
        assert_eq!(state.set("nope/slash", Value::Null).unwrap_err().to_string(), expected);
        assert_eq!(
            state.get(&"k".repeat(129)).unwrap_err().to_string(),
            expected,
            "129 characters is one too many"
        );
        assert!(state.get(&"k".repeat(128)).is_ok(), "128 characters is the ceiling");
    }

    #[test]
    fn an_oversized_set_is_refused_and_nothing_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        let huge = Value::String("x".repeat(MISSION_STATE_MAX_BYTES + 10));
        let err = state.set("big", huge).unwrap_err();
        assert!(
            err.to_string().starts_with("Mission state exceeds the 256 KiB limit ("),
            "{err}"
        );
        assert!(!state.path().exists(), "a refused set must not create the file");
    }

    #[test]
    fn an_oversized_file_on_disk_is_refused_on_read() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let path = mission_state_path(&loc, "m-1").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, format!("{{\"k\":\"{}\"}}", "x".repeat(MISSION_STATE_MAX_BYTES)))
            .unwrap();
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        let err = state.get("k").unwrap_err();
        assert!(err.to_string().contains("exceeds the 256 KiB limit"), "{err}");
    }

    #[test]
    fn a_non_object_root_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let path = mission_state_path(&loc, "m-1").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "[1,2,3]").unwrap();
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        let err = state.get("k").unwrap_err();
        assert!(err.to_string().contains("root must be a JSON object"), "{err}");
    }

    #[test]
    fn assert_workflow_json_value_accepts_every_representable_value() {
        assert!(assert_workflow_json_value(
            &serde_json::json!({"a": [1, "two", null, {"b": true}]}),
            "value"
        )
        .is_ok());
    }
}
