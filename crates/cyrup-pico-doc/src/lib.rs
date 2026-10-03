//! `cyrup-pico-doc` — the Pico5 durability kernel's **functional core** (ADR-0030 §8).
//!
//! Everything in this crate is a total function over explicit inputs, unit-testable with literal
//! values. There is no `tokio`, no storage, no `Session` and no `async`, and the absences are the
//! crate's contract rather than an accident of what has been written so far: ADR-0030 §8's second
//! load-bearing reason for the split is *"keeping them behind a crate boundary with no `tokio` edge
//! is what keeps them that way."*
//!
//! # What is here, and the one finding it implements
//!
//! ADR-0030 F4 — *one owned, `Arc`-shared immutable document value* — is the highest-value finding in
//! the design, and §4 is its before/after. Upstream's guarantee is a sentence
//! (`packages/chord/src/delta/README.md`): *"Immutability is an ownership contract. Nothing is frozen
//! or defensively copied, so an illegal mutation is not detected. It silently corrupts state."* It is
//! operationalised as a ten-row mutation-rights table, a revocation `Proxy`, a per-access seal check,
//! a validating copy walk at every placement, `copyJson()` at five call sites, and a recursive deep
//! clone on **both** storage paths — whose own comment
//! (`packages/durable/src/storage/memory.ts:213-218`) admits it exists to *simulate* the ownership
//! boundary SQLite and JSONL get free.
//!
//! Here that is [`DocValue`]: an enum with **no interior-mutable variant**, behind `Arc`. The table
//! becomes the type, the copy walk becomes a move, and structural sharing becomes `Arc::make_mut`.
//! What survives is exactly one runtime check, and it is a check a naive port would have deleted
//! without noticing — see [`DocValue::parse`].
//!
//! # The guarantees this crate discharges
//!
//! | handle | ADR-0030 §2 row | enforcement | where |
//! |---|---|---|---|
//! | `G-INV-6` (value half) | §2.1 invariant 6; §2.3 `spec.md:72, 1356-1369` | `unrepresentable` for eight of Chord's ten rejected shapes, `guarded` for non-finite floats at one boundary | [`DocValue::parse`] |
//! | `G-TRUSTED-IMMUTABLE` | §2.2 `spec.md:4528-4530` | `unrepresentable`, conditional on the no-interior-mutability rule | [`DocValue`], [`Tracker`] |
//! | `G-OWNERSHIP-BOUNDARY` | §2.2 `spec.md:1560-1566` | `unrepresentable` — `CYRUP-DELTA`: same guarantee, mechanism deleted | [`materialize`], [`apply()`] |
//! | `G-CORRUPTION-NOT-ABSENCE` | §2.2 `spec.md:4352-4360` | `unrepresentable` to confuse the channels | [`ReplayPlan::parse`] |
//! | `G-VERSION-PER-RECORD` | §2.3 `spec.md:1119-1120, 4366-4367` | `unrepresentable` downstream of the parser | [`StoredVersion`], [`ReplayPlan::parse`] |
//!
//! Each `unrepresentable` claim carries a `trybuild` compile-fail case in `tests/compile-fail/`,
//! because ADR-0030 §7's rule is that a guarantee enforced only by a signature needs a case proving
//! the invalid program is rejected — otherwise a well-meaning later change undoes it with nothing
//! turning red.
//!
//! # Where `DefVersion` lives, and why it is not in `cyrup-pico-store`
//!
//! ADR-0030 §10 lists `DefVersion` in its *identity* block beside `Id<K>` and `Seq`, which reads as
//! though it belongs in `cyrup-pico-store` with them, and PICO5-PLAN S1 put it there. ADR-0030 §8's
//! dependency direction overrides that, and it is not a preference: `cyrup-pico-store` depends on
//! **this** crate, because S3's `DocumentBase` takes [`DocRoot`] and `DocumentContent::Delta` takes
//! [`StoredVersion`]. [`StoredContent`] carries a version in both arms, so the version must be here
//! or the two crates are a cycle Cargo refuses. `cyrup-pico-store` re-exports it, so
//! `cyrup_pico_store::DefVersion` still resolves and S1's parser tests are untouched. The
//! discrepancy is recorded rather than papered over.
//!
//! # What is deliberately *not* here
//!
//! * `Draft<'d, T>` and the typed document surface — S4/S5. [`OpenChange`] is the untyped recorder it
//!   borrows, and ADR-0030 F2's sketch is literally `Draft<'d, T> { ch: &'d mut OpenChange, .. }`.
//! * `Seq`, `Id<K>`, `Lifetime`, `DocumentPoint` — S1, in `cyrup-pico-store`. This crate has no edge
//!   there, so a commit sequence is not in scope to confuse with a revision number.
//! * `Batch`, `DocumentCommand`, `Storage` — S3.

#![forbid(unsafe_code)]

mod apply;
mod change;
mod checkpoint;
mod de;
mod op;
mod parse;
mod path;
mod replay;
mod value;
mod version;

pub use apply::{apply, apply_batches};
pub use change::{OpenChange, Prepared, Revision, StalePreparation, Tracker};
pub use checkpoint::{BaseRequirement, CheckpointInput, Representation, choose_representation};
pub use op::{NotAPermutation, Op, OpBatch, Permutation, TrimLen, ZeroTrim};
pub use path::{Path, PathError, RESERVED_SEGMENTS, Seg, UnsafePathError};
pub use replay::{Corruption, ReplayPlan, StoredContent, materialize};
pub use value::{DocMap, DocRoot, DocValue, JsonNum, MAX_DEPTH, NotStrictJson};
pub use version::{DefVersion, StoredVersion, VersionFit, classify_version};

#[cfg(test)]
mod tests;
