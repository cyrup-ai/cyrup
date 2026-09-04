//! Version migration (arch-04 §6.2, R-04-004). v1 (linear, no ids) → v2 (tree) → v3
//! (unified-roles). cyrup always writes [`crate::header::CURRENT_VERSION`]; loading an older file
//! migrates in memory and (when persisted) triggers a one-time rewrite.

use std::collections::HashSet;

use cyrup_core::EntryId;
use serde_json::Value;

use crate::entry::{Entry, KnownEntry};
use crate::header::{CURRENT_VERSION, SessionHeader};

/// Migrate header + entries to the current version in place. Returns `true` if anything changed.
pub fn to_current(header: &mut SessionHeader, entries: &mut [Entry]) -> bool {
    let v = header.effective_version();
    if v >= CURRENT_VERSION {
        return false;
    }
    if v < 2 {
        v1_to_v2(entries);
    }
    if v < 3 {
        v2_to_v3(entries);
    }
    header.version = Some(CURRENT_VERSION);
    true
}

/// v2→v3: rename the legacy `hookMessage` message role to `custom` so it stays in LLM context (Pi
/// `migrateV2ToV3`, `session-manager.ts:255-270`). A `hookMessage`-role message fails core parsing
/// and is held as [`Entry::Unknown`]; after the rename it re-parses into the `custom`
/// [`crate::agent_message::AgentMessage`] arm and once again contributes to context / cut-points.
fn v2_to_v3(entries: &mut [Entry]) {
    for e in entries.iter_mut() {
        let Entry::Unknown(v) = e else { continue };
        let is_message = v.get("type").and_then(Value::as_str) == Some("message");
        let is_hook = v
            .get("message")
            .and_then(|m| m.get("role"))
            .and_then(Value::as_str)
            == Some("hookMessage");
        if !is_message || !is_hook {
            continue;
        }
        if let Some(role) = v
            .get_mut("message")
            .and_then(Value::as_object_mut)
            .and_then(|m| m.get_mut("role"))
        {
            *role = Value::String("custom".to_string());
        }
        if let Ok(known) = serde_json::from_value::<KnownEntry>(v.clone()) {
            *e = Entry::Known(known);
        }
    }
}

/// v1→v2: mint collision-checked 8-hex ids and chain `parentId` linearly (each entry's parent is
/// the previous entry). Entries that already carry an id keep it. Legacy id-less entries (stored
/// verbatim as `Entry::Unknown`) are given an id + parent and re-typed if their `type` is known.
///
/// **Two deliberate spellings, both unobservable on any file pi wrote** (walked statement-by-
/// statement against `migrateV1ToV2`, `session-manager.ts:229-256` @v0.83.0, closing the
/// `migrate.rs` blind spot the 2026-08-12 area-03 audit named):
///
/// 1. Pi assigns `entry.id = generateId(ids)` **unconditionally**; cyrup keeps a pre-existing id.
///    A v1 entry has no `id` — that is the whole point of the v1→v2 migration — so the two agree
///    on every v1 file, and they can only differ for a hand-built file that declares `version < 2`
///    while already carrying ids. Preserving such an id keeps any external reference to it valid.
/// 2. Pi's `generateId(byId)` retries while `byId.has(id)`, but `migrateV1ToV2` **never calls
///    `ids.add(...)`**, so pi's collision check is a no-op on an always-empty set and pi can mint
///    the same 8-hex id twice. cyrup seeds `used` from the surviving ids and inserts each minted
///    one. This can only ever change the outcome in the case pi gets wrong.
///
/// A v1 `compaction` entry referenced its first-kept entry by numeric `firstKeptEntryIndex`; Pi
/// `migrateV1ToV2` (`session-manager.ts:241-250`) resolves that index into the freshly-id'd entry
/// array's `firstKeptEntryId` and drops the index field, otherwise the entry can never parse as a
/// `Compaction` and the compaction boundary is lost in context building. Pi's index is taken over
/// the **header-inclusive** file array (the `type:"session"` line is at index 0); cyrup's `entries`
/// slice excludes the header, so the equivalent cyrup position is `firstKeptEntryIndex - 1` (index
/// 0 ⇒ the session header ⇒ no conversion, mirroring Pi's `targetEntry.type !== "session"` guard).
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

    // Ids assigned so far, by cyrup position, so a `compaction` can resolve its (earlier)
    // `firstKeptEntryIndex` target the same way Pi reads `entries[index].id` mid-pass.
    let mut assigned: Vec<EntryId> = Vec::with_capacity(entries.len());
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
                    let needs_id = obj
                        .get("id")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty);
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
                    convert_first_kept_index(obj, &assigned);
                    // Re-type now that the entry is well-formed (e.g. a v1 `message` or the
                    // just-converted `compaction`).
                    if let Ok(known) =
                        serde_json::from_value::<KnownEntry>(Value::Object(obj.clone()))
                    {
                        *e = Entry::Known(known);
                    }
                }
            }
        }
        let id = e.id();
        assigned.push(id.clone());
        prev = Some(id);
    }
}

/// Convert a v1 `compaction` entry's numeric `firstKeptEntryIndex` into a `firstKeptEntryId` using
/// the already-assigned ids, then remove the index field (Pi `session-manager.ts:240-249`). The
/// target is always an earlier entry (the first-kept entry precedes the appended compaction), so it
/// is present in `assigned`; a `0` index (the session header) or an unresolved index drops the
/// field without setting an id — exactly as Pi skips when `targetEntry` is the session entry.
fn convert_first_kept_index(obj: &mut serde_json::Map<String, Value>, assigned: &[EntryId]) {
    if obj.get("type").and_then(Value::as_str) != Some("compaction") {
        return;
    }
    // Pi's guard is `typeof comp.firstKeptEntryIndex === "number"` — true for a NEGATIVE and for a
    // FRACTIONAL value too, and the `delete comp.firstKeptEntryIndex` that follows it runs on every
    // one of those. Matching on `as_u64` alone returned early for both, leaving the dead v1
    // `firstKeptEntryIndex` key on the entry that the migration rewrite then persisted. Read the
    // value as an `f64` (JSON numbers are doubles upstream) and always strip the key.
    let Some(raw) = obj.get("firstKeptEntryIndex").and_then(Value::as_f64) else {
        return;
    };
    // `entries[idx]` is a JS property access, so the index is coerced with `ToString`: `1.0`
    // stringifies to `"1"` and hits element 1, while `1.5` stringifies to `"1.5"`, matches no
    // element, and yields `undefined` (Pi's `if (targetEntry && …)` then skips). A negative index
    // is likewise a miss, since JS arrays have no negative-index elements.
    #[allow(clippy::float_cmp)]
    let integral = raw >= 0.0 && raw.fract() == 0.0 && raw <= u64::MAX as f64;
    // Header-inclusive (Pi) index → cyrup position; index 0 is the session header.
    if integral
        && let Some(pos) = (raw as usize).checked_sub(1)
        && let Some(target) = assigned.get(pos)
    {
        obj.insert(
            "firstKeptEntryId".to_string(),
            Value::String(target.to_string()),
        );
    }
    obj.remove("firstKeptEntryIndex");
}

fn mint_unique(used: &HashSet<EntryId>) -> EntryId {
    loop {
        let id = crate::ids::gen_short_id();
        if !used.contains(&id) {
            return id;
        }
    }
}
