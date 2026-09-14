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
//! `runs/foreground/subagent-executor.ts:4139`, inside the `workflowScript` branch. When this file
//! was ported cyrup had no `workflowScript` runtime at all, so it was ported in full and with its
//! own tests precisely so that wiring one up later would be a CALL-SITE change rather than a
//! second port of this file. That bet paid: cyrup now has that runtime, and the whole of the
//! wiring is [`MissionWorkflowStateStore`] below plus one `state: Some(...)` at
//! `extension/tool/routing.rs`'s WORKFLOW arm — this module's rules were not touched to get there.
//! It is still deliberately NOT wired into a made-up cyrup-only surface, because upstream exposes
//! no other one.
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

/// pi `validateStateKey` (`workflow-state.ts:21-26`) — `STATE_KEY_PATTERN` is byte-identical to
/// the workflow key grammar, so the check delegates to [`crate::workflows::WorkflowKey`] (the
/// crate's ONE declaration of it, SCOPE_3 §A.3) while the refusal keeps upstream's exact text —
/// the parser owns the grammar, this call site owns the wording.
fn validate_state_key(value: &str) -> MissionResult<&str> {
    if crate::workflows::WorkflowKey::parse(value).is_ok() {
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
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect(),
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
    Ok(MissionWorkflowState {
        path: mission_state_path(location, mission_id)?,
        values: None,
    })
}

/// [`MissionWorkflowState`] behind the engine's [`WorkflowStateStore`] trait — the adapter that
/// turns this module's "wiring status" note into a call-site change.
///
/// Upstream needs no equivalent type: its `{ path, get, set }` object already satisfies the
/// engine's structural contract at `subagent-executor.ts:4139`. Rust needs a nominal impl because
/// the two signatures disagree twice — [`MissionWorkflowState::get`] takes `&mut self` (the lazy
/// once-only load mutates the cached map, so the FIRST read is a write) and both halves return
/// [`MissionResult`], while the trait is `&self` and `Result<_, String>`.
///
/// [`tokio::sync::Mutex`] rather than [`std::sync::RwLock`]/[`std::sync::Mutex`] for two
/// independent reasons: a reader/writer split would buy nothing when `get` needs `&mut` just like
/// `set`, and the lock is taken inside an `async fn`, where a contended `std::sync` guard would
/// block the executor thread outright instead of yielding.
///
/// [`WorkflowStateStore`]: crate::workflows::scripted::WorkflowStateStore
#[derive(Debug)]
pub struct MissionWorkflowStateStore {
    inner: tokio::sync::Mutex<MissionWorkflowState>,
}

impl MissionWorkflowStateStore {
    /// pi `createMissionWorkflowState(...)` as the `workflowScript` branch consumes it — the
    /// constructor takes exactly what [`create_mission_workflow_state`] takes, because it IS that
    /// call plus the lock.
    ///
    /// Construction does **zero** I/O: [`MissionWorkflowState`] is lazy (`values: None` until the
    /// first access), so only [`mission_state_path`]'s traversal guard runs here. That is what
    /// makes it safe for the call site to build a store for every mission-bound workflow, even one
    /// whose script never mentions `state` — and the call site has no choice, since the resulting
    /// `is_some()` must be known before the script is validated.
    ///
    /// # Errors
    ///
    /// [`MissionError::Invalid`] when `mission_id` is not a valid mission id.
    pub fn create(location: &MissionStoreLocation, mission_id: &str) -> MissionResult<Self> {
        Ok(Self {
            inner: tokio::sync::Mutex::new(create_mission_workflow_state(location, mission_id)?),
        })
    }
}

#[async_trait::async_trait]
impl crate::workflows::scripted::WorkflowStateStore for MissionWorkflowStateStore {
    /// Delegates, and ONLY delegates. The key grammar is [`validate_state_key`]'s (and the
    /// engine's own `validate_key` before that); the 256 KiB ceiling and the lazy load are
    /// [`MissionWorkflowState::get`]'s. A second check here would be a second place to get it
    /// wrong.
    ///
    /// [`MissionError`]'s `Display` is deliberately verbatim upstream text (`Invalid(String)`
    /// renders bare, with no prefix), so `to_string` hands the guest exactly the refusal the port
    /// promises — no wrapper sentence is added.
    async fn get(&self, key: &str) -> Result<Option<Value>, String> {
        let mut guard = self.inner.lock().await;
        // `MissionWorkflowState` is BLOCKING `std::fs`. The whole-file read is bounded by
        // `MISSION_STATE_MAX_BYTES` (256 KiB) and happens at most once per run (the map is
        // authoritative afterwards), so it is taken inline rather than through `spawn_blocking` —
        // which would need the `&mut` guard to cross a thread boundary for a read that is, at
        // worst, a quarter-megabyte off the page cache.
        guard.get(key).map_err(|e| e.to_string())
    }

    /// Delegates to [`MissionWorkflowState::set`], which validates the key, validates the value
    /// (`assert_workflow_json_value`, already called at its own line — not re-called here),
    /// measures the SERIALIZED candidate against the 256 KiB ceiling, writes atomically, and only
    /// then adopts the map. The size ceiling is the one invariant the engine does not also
    /// enforce, which makes its refusal the proof that this adapter is delegating rather than
    /// re-implementing.
    async fn set(&self, key: &str, value: Value) -> Result<(), String> {
        let mut guard = self.inner.lock().await;
        guard.set(key, value).map_err(|e| e.to_string())
    }
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
        state
            .set("phase", serde_json::json!({"step": 2, "done": false}))
            .unwrap();
        assert_eq!(
            state.get("phase").unwrap(),
            Some(serde_json::json!({"step": 2, "done": false}))
        );

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
            let mode = std::fs::metadata(state.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
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
        assert_eq!(
            state
                .set("nope/slash", Value::Null)
                .unwrap_err()
                .to_string(),
            expected
        );
        assert_eq!(
            state.get(&"k".repeat(129)).unwrap_err().to_string(),
            expected,
            "129 characters is one too many"
        );
        assert!(
            state.get(&"k".repeat(128)).is_ok(),
            "128 characters is the ceiling"
        );
    }

    #[test]
    fn an_oversized_set_is_refused_and_nothing_is_written() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        let huge = Value::String("x".repeat(MISSION_STATE_MAX_BYTES + 10));
        let err = state.set("big", huge).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Mission state exceeds the 256 KiB limit ("),
            "{err}"
        );
        assert!(
            !state.path().exists(),
            "a refused set must not create the file"
        );
    }

    #[test]
    fn an_oversized_file_on_disk_is_refused_on_read() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let path = mission_state_path(&loc, "m-1").unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            format!("{{\"k\":\"{}\"}}", "x".repeat(MISSION_STATE_MAX_BYTES)),
        )
        .unwrap();
        let mut state = create_mission_workflow_state(&loc, "m-1").unwrap();
        let err = state.get("k").unwrap_err();
        assert!(
            err.to_string().contains("exceeds the 256 KiB limit"),
            "{err}"
        );
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
        assert!(
            err.to_string().contains("root must be a JSON object"),
            "{err}"
        );
    }

    /// WORKFLOW_20 — the adapter proven where it is actually consumed: through the engine's
    /// trait, over the real file. The SECOND store is the cross-process durability proof without
    /// paying for a subprocess — two stores share nothing but `state.json`, so a value the second
    /// one reads back can only have come off disk.
    #[tokio::test]
    async fn the_store_round_trips_through_the_file_behind_the_engine_trait() {
        use crate::workflows::scripted::WorkflowStateStore as _;

        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let store = MissionWorkflowStateStore::create(&loc, "m-1").unwrap();
        assert_eq!(store.get("phase").await.unwrap(), None);
        store
            .set("phase", serde_json::json!({"step": 2, "done": false}))
            .await
            .unwrap();
        assert_eq!(
            store.get("phase").await.unwrap(),
            Some(serde_json::json!({"step": 2, "done": false}))
        );

        let reopened = MissionWorkflowStateStore::create(&loc, "m-1").unwrap();
        assert_eq!(
            reopened.get("phase").await.unwrap(),
            Some(serde_json::json!({"step": 2, "done": false}))
        );
        let raw = std::fs::read_to_string(mission_state_path(&loc, "m-1").unwrap()).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&raw).unwrap(),
            serde_json::json!({"phase": {"step": 2, "done": false}}),
            "the whole object is rewritten on every set, so the file IS the state"
        );
    }

    /// The adapter's `map_err` is the whole of its error handling, and that is the point:
    /// [`MissionError::Invalid`] renders BARE, so upstream's refusals reach the guest
    /// character-for-character with no wrapper sentence prepended.
    ///
    /// The 256 KiB refusal carries extra weight: nothing in the engine bounds a value's size, so
    /// that sentence can ONLY have come from [`MissionWorkflowState::set`]. If it ever stops
    /// appearing, the adapter has stopped delegating.
    #[tokio::test]
    async fn the_store_forwards_upstreams_refusals_verbatim_and_writes_nothing() {
        use crate::workflows::scripted::WorkflowStateStore as _;

        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let store = MissionWorkflowStateStore::create(&loc, "m-1").unwrap();

        let huge = Value::String("x".repeat(MISSION_STATE_MAX_BYTES + 10));
        let err = store.set("big", huge).await.unwrap_err();
        assert!(
            err.starts_with("Mission state exceeds the 256 KiB limit ("),
            "{err}"
        );

        let expected = "state key must be 1-128 characters using letters, numbers, '.', '_' or \
                        '-', and start with a letter or number.";
        assert_eq!(store.get("has space").await.unwrap_err(), expected);
        assert_eq!(
            store.set("nope/slash", Value::Null).await.unwrap_err(),
            expected
        );

        assert!(
            !mission_state_path(&loc, "m-1").unwrap().exists(),
            "every call above was refused, so nothing may have been written"
        );
    }

    /// The traversal guard runs at CONSTRUCTION, not on first use — which is what lets the call
    /// site build a store for every mission-bound workflow and still fail the tool call, rather
    /// than discovering a malformed id from inside the guest realm.
    #[test]
    fn the_store_refuses_a_traversing_mission_id_at_construction() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        assert!(MissionWorkflowStateStore::create(&loc, "../escape").is_err());
    }

    #[test]
    fn assert_workflow_json_value_accepts_every_representable_value() {
        assert!(
            assert_workflow_json_value(
                &serde_json::json!({"a": [1, "two", null, {"b": true}]}),
                "value"
            )
            .is_ok()
        );
    }
}
