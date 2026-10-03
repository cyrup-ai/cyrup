//! ADR-0030 F3's first two unspellable states: *"two content commands for one incarnation (one map
//! value)"* and *"an incarnation retired twice (`Retire` is not a count)"*.
//!
//! A built [`Batch`](cyrup_pico_store::Batch) hands out its document commands by **shared** reference,
//! so a second command for one incarnation cannot be added after assembly. During assembly it is
//! `AlreadyStaged`, which `src/tests/batch.rs` covers; the shape is what no runtime test can cover.
//!
//! On its own because `E0596` comes from the borrow checker, which runs after type checking: a file that
//! also names a missing method reports that instead.

use cyrup_pico_store::{BatchBuilder, ConversationRecord, DocumentCommand, DocumentId, ROOT_CONVERSATION_ID};

fn main() {
    let mut builder = BatchBuilder::new();
    builder
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .unwrap();
    let batch = builder.build().unwrap();

    let id: DocumentId = serde_json::from_str("9").unwrap();
    batch.documents().insert(id, DocumentCommand::RetireOnly);
}
