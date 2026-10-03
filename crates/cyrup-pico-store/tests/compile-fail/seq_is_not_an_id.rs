//! ADR-0030 §2.2 row 1: a commit sequence reaching an entity-id parameter is the failure that
//! shows a user a confident, wrong past. `spec.md:247` says the distinct brand exists *"to prevent
//! an entity ID from being used as a document commit point"*; upstream both are `number`, so the
//! brand is erased and a bare cast restores it. Here neither direction compiles.

use cyrup_pico_store::{DocumentPoint, EntryId, Seq};

fn needs_an_entry(_id: EntryId) {}
fn needs_a_sequence(_seq: Seq) {}

fn main() {
    let seq: Seq = serde_json::from_str("41").unwrap();
    let id: EntryId = serde_json::from_str("41").unwrap();

    needs_an_entry(seq);
    needs_a_sequence(id);

    // Nor can an id stand in for a read point.
    let _point: DocumentPoint = DocumentPoint::At(id);
}
