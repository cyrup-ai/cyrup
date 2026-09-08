//! Delivery ownership and the session gate — the single implementation of "is this mine?".
//!
//! # One rule, one implementation
//!
//! Upstream writes the identity predicate three times — `result-watcher.ts:225-228`, its lease
//! wrapper at `:232-236`, and an independent copy in `notify.ts:572-575` — and the session
//! predicate eight times, in two subtly different strictness classes. That duplication is how a
//! rule drifts. This module holds each exactly once:
//!
//! * [`SessionGate`] — *may I act on this session's run?* Used by the control operations
//!   (stop/steer/interrupt/dismiss), listings and the job tracker.
//! * [`OwnershipSnapshot::owns`] — *may I consume this completion?* Strictly stronger: it also
//!   requires the [`crate::identity::CompletionOwnerId`] to match, because a session id outlives
//!   the process that minted it.
//! * [`Attribution`] — the three-way decision the result watcher makes, as a pure function of a
//!   result and a snapshot, handing back a DIFFERENT TYPE per band so the permission travels in
//!   the value rather than in a predicate the caller must remember to consult.
//!
//! **Do not unify the first two.** Giving `SessionGate` the owner check locks headless embedders
//! out of controlling their own runs; taking it away from `owns` starts delivering foreign
//! results. They answer different questions and their doc comments say so.
//!
//! # Functional core, imperative shell
//!
//! [`ResultDeliveryOwnership`] is the live, mutex-guarded state. Everything that makes a *decision*
//! consumes an [`OwnershipSnapshot`] — a plain value taken once before a scan. The shell touches
//! the lock and the filesystem; the core decides. That seam is what lets the entire decision
//! surface be unit-tested without a tempdir, a runtime, or a mock.
//!
//! # File layout — one file, one concern
//!
//! ```text
//! mod.rs           facade; no logic
//! gate.rs          SessionGate — "may I act on this session's run?"
//! ownership.rs     ResultDeliveryOwnership + OwnershipSnapshot — "may I consume this completion?"
//! custody.rs       Attribution + the per-band types — the watcher's three-way decision
//! receipt.rs       DeliveryReceipt + CompletionDelivery — proof that a notification landed
//! ```

mod custody;
mod gate;
mod ownership;
mod receipt;

pub use custody::{Attribution, ObservableCompletion, OwnedCompletion};
pub use gate::SessionGate;
pub use ownership::{OwnershipSnapshot, ResultDeliveryOwnership};
pub use receipt::{CompletionDelivery, DeliveryReceipt};
