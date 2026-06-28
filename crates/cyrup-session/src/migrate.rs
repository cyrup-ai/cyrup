//! Version migration (arch-04 §6.2, R-04-004). v1 (linear, no ids) → v2 (tree) → v3
//! (unified-roles). cyrup always writes [`crate::header::CURRENT_VERSION`]; loading an older file
//! migrates in memory and (when persisted) triggers a one-time rewrite.

use std::collections::HashSet;

use cyrup_core::EntryId;
use serde_json::Value;

use crate::entry::{Entry, KnownEntry};
use crate::header::{SessionHeader, CURRENT_VERSION};

/// Migrate header + entries to the current version in place. Returns `true` if anything changed.
pub fn to_current(header: &mut SessionHeader, entries: &mut [Entry]) -> bool {
    let v = header.effective_version();
    if v >= CURRENT_VERSION {
        return false;
    }
    if v < 2 {
        v1_to_v2(entries);
    }
    // v2→v3 renames the legacy `hookMessage` message role to `custom`. cyrup-core's `Message`
    // model has no `hookMessage` arm, so such roles already round-trip as `Entry::Unknown`; no
    // structural change is required here beyond bumping the version.
    header.version = Some(CURRENT_VERSION);
    true
}

/// v1→v2: mint collision-checked 8-hex ids and chain `parentId` linearly (each entry's parent is
/// the previous entry). Entries that already carry an id keep it. Legacy id-less entries (stored
/// verbatim as `Entry::Unknown`) are given an id + parent and re-typed if their `type` is known.
fn v1_to_v2(entries: &mut [Entry]) {
    let mut used: HashSet<EntryId> = entries
        .iter()
        .filter_map(|e| match e {
            Entry::Known(k) => {
                let id = k.base().id.clone();
                (!id.as_str().is_empty()).then_some(id)
            }
            Entry::Unknown(v) => v.get("id").and_then(Value::as_str).map(EntryId::from),
        })
        .collect();

    let mut prev: Option<EntryId> = None;
    for e in entries.iter_mut() {
        match e {
            Entry::Known(k) => {
                let base = k.base_mut();
                if base.id.as_str().is_empty() {
                    let id = mint_unique(&used);
                    used.insert(id.clone());
                    base.id = id;
                }
                base.parent_id = prev.clone();
            }
            Entry::Unknown(v) => {
                if let Some(obj) = v.as_object_mut() {
                    let needs_id =
                        obj.get("id").and_then(Value::as_str).is_none_or(str::is_empty);
                    if needs_id {
                        let id = mint_unique(&used);
                        used.insert(id.clone());
                        obj.insert("id".to_string(), Value::String(id.to_string()));
                    }
                    match &prev {
                        Some(p) => {
                            obj.insert("parentId".to_string(), Value::String(p.to_string()));
                        }
                        None => {
                            obj.insert("parentId".to_string(), Value::Null);
                        }
                    }
                    // Re-type now that the entry is well-formed (e.g. a v1 `message`).
                    if let Ok(known) = serde_json::from_value::<KnownEntry>(Value::Object(obj.clone()))
                    {
                        *e = Entry::Known(known);
                    }
                }
            }
        }
        prev = Some(e.id());
    }
}

fn mint_unique(used: &HashSet<EntryId>) -> EntryId {
    loop {
        let id = crate::ids::gen_short_id();
        if !used.contains(&id) {
            return id;
        }
    }
}
