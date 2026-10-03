//! ADR-0030 F6 §A: `IdKind` is sealed and implemented *"for exactly the five kinds"*. A sixth kind
//! is a storage-format event, not something a downstream crate adds.

use cyrup_pico_store::{Id, IdKind, IdKindTag};

struct Ledger;

impl IdKind for Ledger {
    const KIND: IdKindTag = IdKindTag::Entry;
}

fn main() {
    let _: Option<Id<Ledger>> = None;
}
