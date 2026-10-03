//! ADR-0030 F6 §D: `open_read_only` takes *"no lock and **has no `commit`**"*.
//!
//! Not a refused commit — an absent one. A read-only store that implemented `Storage` would have the
//! trait's `commit` and would have to refuse at runtime; this one is `Reader` and nothing else, so the
//! method does not exist to call.

use std::path::Path;

use cyrup_pico_store::{Batch, BatchBuilder, ConversationRecord, Cx, ROOT_CONVERSATION_ID};
use cyrup_pico_store_jsonl::JsonlStore;

fn main() {
    let store = JsonlStore::open_read_only(Path::new("/tmp/does-not-matter")).unwrap();
    let mut builder = BatchBuilder::new();
    builder
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .unwrap();
    let batch: Batch = builder.build().unwrap();
    let _ = store.commit(batch, &Cx::detached());
}
