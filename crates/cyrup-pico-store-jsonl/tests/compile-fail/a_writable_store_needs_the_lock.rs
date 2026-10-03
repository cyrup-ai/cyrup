//! ADR-0030 F6 §D: *"a writable backend cannot be built without the proof, so 'forgot to lock' is a
//! compile error."*
//!
//! The lock is a **required argument**, not an option and not a flag, so the program that opens a store
//! for writing without taking it is not a program.

use std::path::Path;

use cyrup_pico_store_jsonl::{Durability, JsonlOptions, JsonlStore};

fn main() {
    let dir = Path::new("/tmp/does-not-matter");
    // No `StoreLock`. Two cyrup processes in one directory is the failure this refuses to compile.
    let _store = JsonlStore::open_for_write(dir, JsonlOptions::new(Durability::PowerLoss));
}
