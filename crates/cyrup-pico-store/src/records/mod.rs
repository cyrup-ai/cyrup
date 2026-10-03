//! The persisted records (`spec.md:80-288`, `:1066-1120`, `:1596-1620`).
//!
//! Five record types, one per id kind, because ADR-0030 F3's [`Batch`] has one keyed map per kind
//! and `spec.md:4274-4275` makes *"one number is owned by one record of one type"* a storage
//! obligation.
//!
//! # Opaque payloads are [`DocValue`], not `serde_json::Value`
//!
//! `EntryRecord::data`, `TaskRecord::input`, a task checkpoint and a submission's failure detail are
//! all `JsonValue` upstream: payloads the kernel stores and never interprets. They are
//! [`DocValue`] here, and the choice is load-bearing in three ways rather than stylistic.
//!
//! 1. **Detachment is free.** `spec.md:4375-4378` requires every retained write value and every read
//!    result to be detached from the caller's memory, and §11.1 says the memory backend's recursive
//!    clone *"deliberately simulates the ownership boundary naturally created by SQLite
//!    encoding/decoding and JSONL serialization"*. `DocValue` has no interior mutability and shares
//!    structurally through `Arc`, so cloning a record into an index is a refcount bump that cannot
//!    alias anything mutably — ADR-0030 F4's `CYRUP-DELTA`, applied to record payloads as well as to
//!    document values.
//! 2. **Non-finite floats cannot enter.** `DocValue`'s only construction path from host data is
//!    `DocValue::parse`, which rejects `NaN` and the infinities. `serde_json::Value` accepts a
//!    `Number` that `serde_json` itself will later refuse to serialise — the failure
//!    `spec.md:1356-1369` exists to prevent, one restart later.
//! 3. **There is no `cyrup-core` edge to take.** A record's `model` field is a transcript message
//!    upstream; this crate must not depend on `cyrup-core` (ADR-0030 open question 8), and the
//!    kernel does not read those messages — `spec.md:4272-4274` gives semantic validity to the
//!    Session. An opaque strict-JSON value is exactly what storage owes them.
//!
//! # Serde
//!
//! Every type here is `Serialize` **and** `Deserialize`, per ADR-0030 §10's last serde row, with its
//! condition honoured literally: **no `#[serde(borrow)]` and no `Cow<'_, _>` anywhere in them.**
//! Every decoded value is `'static`-owned, which is what makes a read's `Arc` return type sound
//! across a reopen or a cache eviction.
//!
//! Where a record's shape carries an invariant, the derived impl cannot bypass it, because the
//! *shape itself* is the enum: a [`TaskRecord`] whose state is terminal has no memos field to fill,
//! and a [`DocumentRecord`] at `history: "latest"` has no `fork: "asOf"` to name. A derived
//! `Deserialize` over those enums rejects the illegal wire form with an unknown-variant error, which
//! the backend reports as [`Corruption::RecordMalformed`].
//!
//! [`Batch`]: crate::Batch
//! [`DocValue`]: cyrup_pico_doc::DocValue
//! [`Corruption::RecordMalformed`]: crate::Corruption::RecordMalformed

mod conversation;
mod document;
mod entry;
mod submission;
mod task;

pub use conversation::{ConversationOwner, ConversationParent, ConversationRecord};
pub use document::{
    ConversationSemantics, DocumentAddress, DocumentCreate, DocumentRecord, DocumentScope,
    LatestFork, RewindableFork, ScopeRef,
};
pub use entry::{ContextEdit, EditAction, EntryRecord, HeadMarker};
pub use submission::{
    InputSubmission, SubmissionRecord, SubmissionState, SubmissionStatus, SubmissionType,
    WriteSubmission,
};
pub use task::{JoinPolicy, TaskOutcome, TaskOutcomeError, TaskRecord, TaskState, TaskStatus};
