//! ADR-0030 §2.1 row 1 classifies *"one commit is atomic across all record and document writes"* as
//! **unrepresentable** on the Session's side: *"keyed `Batch` taken by value… Session cannot split a
//! change"*. That holds only while a `Batch` has exactly one construction path, and
//! [`BatchBuilder`](cyrup_pico_store::BatchBuilder) is it.
//!
//! This case is on its own because `E0451` is raised by the privacy pass, which runs *after* type
//! checking — so a file that also contains a type error reports that instead, and the privacy guarantee
//! would go unpinned.

use std::collections::BTreeMap;

use cyrup_pico_store::Batch;

fn main() {
    let _forged = Batch {
        conversations: BTreeMap::new(),
        entries: BTreeMap::new(),
        tasks: BTreeMap::new(),
        submissions: BTreeMap::new(),
        documents: BTreeMap::new(),
    };
}
