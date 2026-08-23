//! The crate's one poison-recovery policy for `std::sync::Mutex`.
//!
//! arch-00 forbids panicking in library code (the workspace denies `clippy::unwrap_used`,
//! `clippy::expect_used` and `clippy::panic`, and this crate opts in), so a `Mutex` guard is never
//! taken with `.lock().unwrap()`. Poisoning here is not a correctness signal: every `Mutex` in this
//! crate guards a plain snapshot/registry value, and a panic on some other thread must not wedge
//! model selection, the event fan-out, or a read view (R-00-009). The policy is therefore "recover
//! the guard and carry on", stated exactly once — in [`lock`].

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Lock `m`, ignoring poisoning (never panics; arch-00 no-panic).
///
/// A poisoned lock yields its inner guard via [`PoisonError::into_inner`] — the data is a snapshot
/// value, so the worst case is reading state a panicking thread left half-written, which is
/// strictly better than propagating that panic into every reader.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}
