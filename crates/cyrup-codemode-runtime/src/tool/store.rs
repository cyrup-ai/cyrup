//! The `codemode-store` session entries and their branch-scoped replay (pi
//! `extensions/codemode/tool.ts:53-58` `CODEMODE_STORE_ENTRY_TYPE` / `CodemodeStoreEntryData` and
//! `execute.ts:205-228` `isStoreEntryData` / `readCodemodeStore` @v1.0.1, CODE-009).
//!
//! The sandbox persists nothing: [`read_codemode_store`] folds the entries on the current branch
//! into the values `load()` sees, and a successful script's [`CodemodeStoreWrites`] come back as one
//! [`CodemodeStoreEntryData`] for the session to append. Because the fold runs over the branch
//! path only, each branch sees the values written on its own path.
//!
//! # Production call path
//!
//! [`super::execute::execute_codemode`] calls [`read_codemode_store`] over
//! [`CodemodeHost::branch_custom_entries`](super::host::CodemodeHost::branch_custom_entries)
//! before every script and [`CodemodeStoreEntryData::from_writes`] after a successful one.

use serde_json::{Map, Value};

use crate::types::CodemodeStoreWrites;

/// Custom entry type holding one script's `store()` writes: [`CodemodeStoreEntryData`]
/// (`tool.ts:53`).
pub const CODEMODE_STORE_ENTRY_TYPE: &str = "codemode-store";

/// The `data` of a `codemode-store` entry (`tool.ts:55-58`): the keys a script set, then the keys it
/// deleted.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CodemodeStoreEntryData {
    pub set: Map<String, Value>,
    pub delete: Vec<String>,
}

impl CodemodeStoreEntryData {
    /// The entry a script's writes become, or `None` when it wrote nothing (`execute.ts:405-408`:
    /// an entry is appended only when `set` or `delete` is non-empty).
    #[must_use]
    pub fn from_writes(writes: CodemodeStoreWrites) -> Option<Self> {
        if writes.set.is_empty() && writes.delete.is_empty() {
            return None;
        }
        Some(Self {
            set: writes.set,
            delete: writes.delete,
        })
    }

    /// `isStoreEntryData` (`execute.ts:205-215`): `set` is an object and `delete` an array of
    /// strings. Anything else on the branch under this entry type is ignored, not an error.
    fn parse(data: &Value) -> Option<Self> {
        let object = data.as_object()?;
        let set = object.get("set")?.as_object()?;
        let delete = object
            .get("delete")?
            .as_array()?
            .iter()
            .map(|key| key.as_str().map(str::to_owned))
            .collect::<Option<Vec<_>>>()?;
        Some(Self {
            set: set.clone(),
            delete,
        })
    }
}

/// A `custom` session entry as the store replay needs it: its type and its `data`.
#[derive(Clone, Debug, PartialEq)]
pub struct BranchCustomEntry {
    pub custom_type: String,
    /// `None` is an entry without `data`.
    pub data: Option<Value>,
}

/// The values of `load()` (`execute.ts:217-228`): the `codemode-store` entries of `branch`, applied
/// from the root. Within one entry the deletes apply before the sets, so a key both deleted and set
/// in one entry ends up set. Entries of other types and entries with malformed data are skipped.
#[must_use]
pub fn read_codemode_store(branch: &[BranchCustomEntry]) -> Map<String, Value> {
    let mut store = Map::new();
    for entry in branch {
        if entry.custom_type != CODEMODE_STORE_ENTRY_TYPE {
            continue;
        }
        let Some(data) = entry.data.as_ref().and_then(CodemodeStoreEntryData::parse) else {
            continue;
        };
        for key in &data.delete {
            store.shift_remove(key);
        }
        for (key, value) in data.set {
            store.insert(key, value);
        }
    }
    store
}

#[cfg(test)]
mod tests;
