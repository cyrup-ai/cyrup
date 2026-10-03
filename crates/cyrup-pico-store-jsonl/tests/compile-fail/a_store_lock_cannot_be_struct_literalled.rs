//! The proof cannot be manufactured by a caller that did not take the lock: `StoreLock::acquire` is the
//! only constructor, because the fields are private.
//!
//! A case of its own rather than a paragraph in another, because rustc stops after the first pass that
//! produces an error — a file that also tried to clone the lock would pin only whichever of the two
//! rustc reached first.

use std::path::{Path, PathBuf};

use cyrup_pico_store_jsonl::StoreLock;

fn main() {
    let dir: PathBuf = Path::new("/tmp/does-not-matter").to_path_buf();
    let _forged = StoreLock {
        dir,
        file: std::fs::File::open("/dev/null").unwrap(),
    };
}
