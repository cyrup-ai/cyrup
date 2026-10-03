//! ADR-0030 §2.2: `spec.md:4321-4323`'s *"the Session is the sole committer, so backends add no second
//! commit mutex"* is classified **unrepresentable in-process**, and the mechanism is
//! `commit(&mut self, ..)` with the handle owned exclusively by one non-`Clone` `SessionMut` (F1, F3).
//!
//! ADR-0030 §9 lists that signature as one of **three** things that are *"irreversible and must be right
//! before any backend exists"*. Its enforcement is one `&mut`, which is precisely the kind of thing a
//! later refactor relaxes to `&self` to make a backend shareable — so it gets a case.
//!
//! Note that no async runtime is needed: holding both futures at once is already the error.

use cyrup_pico_store::{BatchBuilder, ConversationRecord, Cx, MemoryStore, ROOT_CONVERSATION_ID, Storage};

fn main() {
    let cx = Cx::detached();
    let mut store = MemoryStore::new();

    let mut first = BatchBuilder::new();
    first
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .unwrap();
    let one = first.build().unwrap();

    let mut second = BatchBuilder::new();
    second
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .unwrap();
    let two = second.build().unwrap();

    // Two commits in flight against one store: a second concurrent committer is a borrow error.
    let a = store.commit(one, &cx);
    let b = store.commit(two, &cx);
    drop((a, b));

    // Nor can a commit be issued through a shared handle.
    let shared: &MemoryStore = &MemoryStore::new();
    let mut third = BatchBuilder::new();
    third
        .conversation(ConversationRecord {
            id: ROOT_CONVERSATION_ID,
            parent: None,
            owner: None,
        })
        .unwrap();
    let _ = shared.commit(third.build().unwrap(), &cx);
}
