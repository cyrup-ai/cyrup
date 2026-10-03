//! ADR-0030 §10's serde table, on `Batch`: `Serialize` **no**, `Deserialize` **never** — *"an in-process
//! assembly product. A derived `Deserialize` would reintroduce every illegal state through a map with
//! duplicate keys silently last-wins, which is the quiet version of F3's bug."*
//!
//! A JSONL or SQLite backend persists the **records** and the content, never the batch, so nothing needs
//! this impl; what it would provide is a way to reconstitute a commit from bytes nobody validated.
//!
//! The second half of the case closes the other door: [`Batch::into_parts`] is a decomposition for a
//! backend to apply, and there is deliberately no inverse. A `from_parts` would be a public constructor
//! wearing a different name.

use std::collections::BTreeMap;

use cyrup_pico_store::{Batch, BatchParts};

fn main() {
    let _decoded: Batch = serde_json::from_str("{}").unwrap();

    let _rebuilt = Batch::from_parts(BatchParts {
        conversations: BTreeMap::new(),
        entries: BTreeMap::new(),
        tasks: BTreeMap::new(),
        submissions: BTreeMap::new(),
        documents: BTreeMap::new(),
    });
}
