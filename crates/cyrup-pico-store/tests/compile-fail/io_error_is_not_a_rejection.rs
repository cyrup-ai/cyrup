//! ADR-0030 §2.2, on `spec.md:4307-4311`'s two-class failure contract: *"**unrepresentable** to omit the
//! choice or to classify I/O as recoverable"*. §9 lists it first among the three irreversible decisions,
//! *"because it shapes every caller's error handling"*.
//!
//! The mechanism is an absence: there is no `From<std::io::Error> for CommitError`, so `?` cannot decide
//! for a backend whether a failed write left nothing behind. The cost of getting it wrong is stated in
//! both directions in §2.2 — *"classified wrong one way, a bad fork source costs the user the whole live
//! session; wrong the other way, the Session runs on a baseline that already disagrees with the disk"* —
//! and an impl added for convenience would silently pick the wrong one every time.
//!
//! `StorageFailure`, by contrast, *does* take an `io::Error`: a failed **read** has no ambiguity to
//! resolve. That line is the guarantee, and this case is where it is drawn.

use std::io;

use cyrup_pico_store::{CommitError, RejectedReason, StorageFailure};

fn commit_the_batch() -> Result<(), CommitError> {
    // A backend's write path. `?` must not be able to classify this.
    let _: () = failing_write()?;
    Ok(())
}

fn failing_write() -> Result<(), io::Error> {
    Err(io::Error::other("the device detached"))
}

fn main() {
    let _ = commit_the_batch();

    // Nor explicitly.
    let _converted: CommitError = io::Error::other("no").into();

    // And a rejection cannot be fabricated from a string, because the enum is closed.
    let _invented = RejectedReason::Other("something went wrong".to_owned());

    // A read failure is different, and this line must keep compiling: it is the contrast that makes the
    // absence above a decision rather than an oversight.
    let _read: StorageFailure = io::Error::other("yes").into();
}
