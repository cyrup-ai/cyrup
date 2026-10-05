//! Port of upstream's `folds store entries from the root, ignoring malformed data`
//! (`test/agent-session-codemode.test.ts:516-533` @v1.0.1) plus the entry shape.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use serde_json::{Value, json};

use super::{
    BranchCustomEntry, CODEMODE_STORE_ENTRY_TYPE, CodemodeStoreEntryData, read_codemode_store,
};
use crate::types::CodemodeStoreWrites;

fn entry(data: Value, custom_type: &str) -> BranchCustomEntry {
    BranchCustomEntry {
        custom_type: custom_type.to_owned(),
        data: Some(data),
    }
}

fn store_entry(data: Value) -> BranchCustomEntry {
    entry(data, CODEMODE_STORE_ENTRY_TYPE)
}

#[test]
fn folds_store_entries_from_the_root_ignoring_malformed_data() {
    let folded = read_codemode_store(&[
        store_entry(json!({ "set": { "a": 1, "b": { "c": 2 } }, "delete": [] })),
        store_entry(json!({ "set": { "a": 3 }, "delete": ["b"] })),
        // No `delete` array: not a store entry.
        store_entry(json!({ "set": { "z": 1 } })),
        // Another extension's entry that happens to look like one.
        entry(
            json!({ "set": { "other": 1 }, "delete": [] }),
            "other-extension",
        ),
    ]);
    assert_eq!(Value::Object(folded), json!({ "a": 3 }));
}

#[test]
fn an_entry_without_data_and_non_string_deletes_are_skipped() {
    let folded = read_codemode_store(&[
        store_entry(json!({ "set": { "a": 1 }, "delete": [] })),
        BranchCustomEntry {
            custom_type: CODEMODE_STORE_ENTRY_TYPE.to_owned(),
            data: None,
        },
        // One non-string delete makes the whole entry malformed (`deleted.every(string)`).
        store_entry(json!({ "set": { "b": 2 }, "delete": ["a", 7] })),
        // `set` must be an object.
        store_entry(json!({ "set": [1], "delete": ["a"] })),
        store_entry(json!({ "set": null, "delete": ["a"] })),
    ]);
    assert_eq!(Value::Object(folded), json!({ "a": 1 }));
}

/// Deletes apply before the sets of the same entry (`execute.ts:223-224`), so a key in both ends up
/// set; a later entry's delete removes it.
#[test]
fn within_one_entry_deletes_apply_before_sets() {
    let folded = read_codemode_store(&[
        store_entry(json!({ "set": { "k": 1 }, "delete": [] })),
        store_entry(json!({ "set": { "k": 2 }, "delete": ["k"] })),
    ]);
    assert_eq!(Value::Object(folded), json!({ "k": 2 }));

    let folded = read_codemode_store(&[
        store_entry(json!({ "set": { "k": 1 }, "delete": [] })),
        store_entry(json!({ "set": {}, "delete": ["k"] })),
    ]);
    assert!(folded.is_empty());
}

/// An entry is appended only when the script wrote something (`execute.ts:405-408`).
#[test]
fn writes_become_an_entry_only_when_non_empty() {
    assert_eq!(
        CodemodeStoreEntryData::from_writes(CodemodeStoreWrites::default()),
        None
    );
    let mut writes = CodemodeStoreWrites::default();
    writes.delete.push("gone".to_owned());
    let data = CodemodeStoreEntryData::from_writes(writes).unwrap();
    assert_eq!(
        serde_json::to_value(&data).unwrap(),
        json!({ "set": {}, "delete": ["gone"] })
    );
}
