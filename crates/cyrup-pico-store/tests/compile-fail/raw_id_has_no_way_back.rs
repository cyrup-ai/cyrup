//! ADR-0030 §10's serde table: a diagnostic value must not become a construction path. `RawId`
//! erases a kind for `RejectedReason::IdAlreadyOwned`; nothing maps it back, and it has no
//! `Deserialize`, so a corrupted log line or an IPC hop cannot turn a number into an id under a
//! kind of the reader's choosing.

use cyrup_pico_store::{EntryId, RawId};

fn main() {
    let id: EntryId = serde_json::from_str("41").unwrap();
    let raw = RawId::from(id);

    let _: EntryId = EntryId::from(raw);
    let _: RawId = serde_json::from_str("41").unwrap();
}
