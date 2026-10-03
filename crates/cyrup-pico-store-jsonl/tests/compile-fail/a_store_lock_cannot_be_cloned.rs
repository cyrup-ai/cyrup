//! One lock cannot become two writable stores.
//!
//! `StoreLock` is RAII and deliberately not `Clone`: a second copy would release the `flock` when the
//! first one dropped, leaving a store that believes it is held exclusively and is not.

use cyrup_pico_store_jsonl::StoreLock;

fn main() {
    let lock = StoreLock::acquire(std::path::Path::new("/tmp/does-not-matter")).unwrap();
    let _second = lock.clone();
}
