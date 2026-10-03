//! ADR-0030 §2.2 classifies `spec.md:4363` — *"storage applies content before retirement, independent
//! of write-array order"* — as **unrepresentable**, with the reason stated as a deletion rather than a
//! check: *"no array: order is not expressible (F3)."*
//!
//! That guarantee is the absence of an API, which makes it exactly the kind ADR-0030 §7 says is worth a
//! `trybuild` case: *"it is the one guarantee here that replaces a named upstream error class with
//! nothing but an absent method, so its deletion is invisible in a diff."* A convenience `writes()`
//! accessor, added in good faith to make a backend's apply loop read like pi's `StorageWrite[]`, would
//! put the ordering hazard back with no test turning red — unless this one does.
//!
//! The same applies to a mutable accessor: with one, a batch could be *amended* after assembly, and the
//! single-content-command-per-incarnation property would move from the shape to a convention.

use cyrup_pico_store::{BatchBuilder, ConversationRecord, ROOT_CONVERSATION_ID};

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

    // There is no ordered list of writes to iterate, in either direction.
    let _writes = batch.writes();
    let _owned = batch.into_writes();

    // And no mutable view of the document commands to amend.
    let _ = batch.documents_mut();
}
