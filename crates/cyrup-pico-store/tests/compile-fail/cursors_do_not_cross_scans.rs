//! ADR-0030 §2.2 classifies `spec.md:4320-4321` — *"a cursor round-trips into the same scan on the same
//! storage, and nowhere else"* — as **unrepresentable cross-scan**, replacing upstream's
//! `type Cursor = Readonly<Record<string, JsonValue>>`: *"one structural bag for every scan and every
//! backend"*.
//!
//! The failure it prevents is the quiet one: *"the scan resumes from a position meaning something else;
//! a history page has a hole, with no error and no log line."* Cross-*store* reuse cannot be a type error
//! and is a `StoreId` compare (`src/tests/cursor.rs`); cross-*scan* reuse is this.

use cyrup_pico_store::{Cx, EntryCursor, EntryQuery, MemoryStore, ROOT_CONVERSATION_ID, Storage, TaskQuery};

fn main() {
    let cx = Cx::detached();
    let store = MemoryStore::new();
    let limit = cyrup_pico_store::PageLimit::parse(10).unwrap();

    let entries = store
        .scan_entries(&EntryQuery::all(ROOT_CONVERSATION_ID), limit, None, &cx);
    drop(entries);

    // An entry cursor is not a task cursor, whatever it is made of.
    let forged = EntryCursor::new(cyrup_pico_store::CursorBytes::new(store.store_id(), vec![0; 8]));
    let _ = store.scan_tasks(&TaskQuery::default(), limit, Some(forged), &cx);

    // Nor is a cursor's payload readable without naming a store.
    let other = EntryCursor::new(cyrup_pico_store::CursorBytes::new(store.store_id(), vec![0; 8]));
    let _bytes = other.payload();
}
