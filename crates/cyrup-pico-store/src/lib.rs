//! `cyrup-pico-store` — the Pico5 durability kernel's storage layer (ADR-0030 §8).
//!
//! Two slices of `docs/PICO5-PLAN.md` live here. **S1** is the identity and value newtypes, which
//! come first because they appear in every signature in this crate and in `cyrup-pico-doc` and
//! retrofitting them is the expensive direction (`docs/RUST-DESIGN-REVIEW.md:147`). **S3** is the
//! storage contract on top of them: the persisted [`records`], the keyed [`Batch`], the [`Storage`]
//! trait, the two-class [`CommitError`], the per-scan cursors, [`MemoryStore`] as §11.1's reference
//! semantics, and the conformance suite behind `feature = "conformance"`.
//!
//! # The three things ADR-0030 §9 calls irreversible
//!
//! §9 recommends landing the contract before any durable engine, and names what must be right first,
//! *"because every type-level guarantee in this document is built out of them"*:
//!
//! 1. **the two-class failure outcome** — [`CommitError::Rejected`] versus
//!    [`CommitError::Uncertain`], with [`RejectedReason`] closed and no `From<std::io::Error>`
//!    anywhere near it (see [`error`]);
//! 2. **the keyed batch** — five `BTreeMap`s and one [`DocumentCommand`] per incarnation, which
//!    deletes four of F3's five illegal states and the per-backend normalisation pass with them
//!    (see [`batch`]);
//! 3. **the `&mut self` single-committer signature** — which turns `spec.md:4321-4323`'s
//!    *"storage implementations do not add a second caller-facing commit mutex"* from a property
//!    documented of the caller into a borrow error (see [`storage`]).
//!
//! # The conformance suite is the contract
//!
//! `spec.md:4369` makes the **suite**, not the trait, what makes backends swappable, and ADR-0030 §8
//! requires it to run with **no `Session`** — because §10's trust split (`spec.md:4272-4277`) is
//! exactly what it tests. So it lives here, behind `feature = "conformance"`, drives a
//! `&mut dyn Storage`, and is why [`Storage`] is written with `#[async_trait]` rather than a native
//! `async fn`: a native one is not dyn-compatible.
//!
//! # What these types are for
//!
//! Upstream (pi `v1.0.0`, `packages/durable/src/ids.ts`) every id and every commit sequence is a
//! plain JavaScript `number`; the `Id<"entry">` brands are erased at runtime and applied by two
//! bare casts, eleven lines in total. `spec.md:242-245` states the consequence plainly: *"ID brands
//! are compile-time only. IDs and sequences remain ordinary numbers in memory, JSON, JSONL, and
//! SQLite."* The failure that permits is a sequence reaching `snapshotAsOf` as though it were an
//! entity id — a confident, wrong answer about the past. Here [`Id<K>`] and [`Seq`] are genuinely
//! distinct at runtime and cross-kind confusion does not compile (ADR-0030 §2.2 row 1,
//! `unrepresentable`).
//!
//! # Construction paths, all of them
//!
//! `docs/RUST-DESIGN-REVIEW.md:73` makes the serde audit mandatory, and this subject persists and
//! re-reads every record on every open, so **serde is the main construction path** rather than an
//! afterthought. Every type here therefore follows ADR-0030 §10's serde table literally:
//!
//! * `Deserialize` is **hand-written** for [`Id<K>`], [`Seq`], [`DefVersion`], [`PageLimit`],
//!   [`Lifetime`], [`DocumentPoint`], [`StoreId`] and [`Kind`]. A derived impl over a private
//!   `NonZeroU64` still accepts any nonzero number, `Lifetime`'s would accept an inverted interval, and
//!   `Kind`'s would accept the empty kind and a kind containing a newline. Each impl requires the one
//!   wire shape it means, rejects a string, a float, a negative and zero, and builds through the
//!   private constructor.
//! * `Deserialize` is **withheld entirely** from [`RawId`], which exists only so a diagnostic can
//!   name a number without the number becoming an id again; from [`CursorBytes`] and the five scan
//!   cursors, so no host bytes can become a cursor for any scan; from [`Batch`], because a map with
//!   duplicate keys would silently last-wins every illegal state F3 deleted; and from
//!   [`RejectedReason`], [`Corruption`] and [`UncertainCommit`], which must be loggable without
//!   becoming a construction path.
//! * `Deserialize` is **derived** for the persisted [`records`], under ADR-0030 §10's condition: no
//!   `#[serde(borrow)]` and no `Cow<'_, _>` anywhere in them, so every decoded value is
//!   `'static`-owned — which is what makes a read's `Arc` return type sound across a reopen. Where a
//!   record's shape carries an invariant the derive cannot bypass it, because the shape *is* the enum:
//!   `{"history":"latest","fork":"asOf"}` has no variant to decode into.
//! * No type here has `Default` (no valid default exists for any of them), `From<u64>`, or an
//!   accessor that returns an id's number in a form the id can be rebuilt from.
//!
//! [`Id<K>`] has no public constructor at all: ADR-0030 F6 §A puts allocation on the storage
//! handle, so `Storage::mint` (S3) is the only allocator and [`ROOT_CONVERSATION_ID`] is the one
//! id the specification fixes rather than mints (`spec.md:249`).
//!
//! # ADR-0030 open question 8, resolved here: the `EntryId` collision
//!
//! **Decision: this crate owns the name `EntryId` for [`Id<Entry>`], and `cyrup_core::EntryId`
//! keeps its name unchanged. Neither side is renamed.**
//!
//! The question was whether to hold a distinct `cyrup_pico_store::EntryId` or rename one side.
//! Three facts settle it:
//!
//! 1. **The two types share no construction path.** `cyrup_core::EntryId` is
//!    `pub struct EntryId(pub Arc<str>)` with `From<&str>`, `From<String>` and a public field
//!    (`crates/cyrup-core/src/lib.rs:93`, via `str_id!`). This one is a `NonZeroU64` behind a
//!    private field with no `From<u64>` and no public constructor. There is no coercion, no
//!    `Deref`, and no shared trait between them, so a session-level string token cannot flow where
//!    a mintable numeric id belongs no matter how the names are spelled.
//! 2. **The collision is a compile error wherever it could matter.** A module that imports both
//!    unqualified is rejected by `E0252` — it must write `as PicoEntryId`, a module path, or an
//!    alias, and in doing so says which it means. A rename would replace that forced statement
//!    with a silent, correct-looking import.
//! 3. **Renaming the other side is a 325-use edit outside this slice's scope.** `EntryId` appears
//!    325 times across 30-plus files in `crates/cyrup-*`, almost all of it `cyrup-session` and
//!    `cyrup-tui` session-tree code that has nothing to do with Pico5. Renaming it would be a
//!    large diff whose only benefit is avoiding an error the compiler already raises, and ADR-0030
//!    §10's own sketch writes `pub type EntryId = Id<Entry>;  // NOT cyrup_core::EntryId`.
//!
//! The structural half of the decision is the part that is actually load-bearing, and it is
//! enforced in `Cargo.toml` rather than by convention: **`cyrup-pico-store` does not depend on
//! `cyrup-core`.** With no edge there is no module in which both names are in scope by accident.
//! Consumers that genuinely need both — `cyrup-session-svc`, eventually — alias at the import.

#![forbid(unsafe_code)]

pub mod batch;
#[cfg(feature = "conformance")]
pub mod conformance;
mod cursor;
mod cx;
mod de;
pub mod error;
mod id;
mod kind;
mod lifetime;
mod memory;
mod page;
pub mod query;
pub mod records;
mod seq;
pub mod storage;
mod store_id;

/// A document definition version.
///
/// **Re-exported from `cyrup-pico-doc`, not defined here**, and the move is forced rather than
/// stylistic. ADR-0030 §10 lists `DefVersion` in its *identity* block beside [`Id<K>`] and [`Seq`],
/// and PICO5-PLAN S1 defined it in this crate on that reading. ADR-0030 §8's dependency direction
/// overrides it: this crate depends on `cyrup-pico-doc` (S3's `DocumentBase` takes `DocRoot`,
/// `DocumentContent::Delta` takes `StoredVersion`), and `cyrup_pico_doc::StoredContent` carries a
/// version in both arms — so a `DefVersion` defined here would make the two crates a cycle Cargo
/// refuses. The name, the hand-written `Deserialize` and every S1 test are unchanged; only the
/// defining module moved.
pub use cyrup_pico_doc::DefVersion;
pub use id::{
    Conversation, Document, Entry, Id, IdKind, IdKindTag, ROOT_CONVERSATION_ID, RawId, Submission,
    Task,
};
pub use lifetime::{InvertedLifetime, Lifetime};
pub use page::{PageLimit, PageLimitError};
pub use seq::{DocumentPoint, Seq};
pub use store_id::StoreId;

pub use batch::{
    AlreadyStaged, Batch, BatchBuilder, BatchParts, CopySource, DocumentBase, DocumentCommand,
    DocumentContent, Retire,
};
pub use cursor::{
    ConversationCursor, CursorBytes, DocumentCursor, EntryCursor, SubmissionCursor, TaskCursor,
    WrongStore,
};
pub use cx::Cx;
pub use error::{
    CommitError, CopySourceMismatch, Corruption, RejectedReason, StorageFailure, UncertainCommit,
};
pub use kind::{Kind, MAX_KIND_LEN, NotAKind, RESERVED_PREFIX};
pub use memory::MemoryStore;
pub use query::{
    CommittedEntry, ConversationQuery, DocumentQuery, EntryQuery, Page, StoredDocument,
    SubmissionQuery, TaskQuery,
};
pub use records::{
    ContextEdit, ConversationOwner, ConversationParent, ConversationRecord, ConversationSemantics,
    DocumentAddress, DocumentCreate, DocumentRecord, DocumentScope, EditAction, EntryRecord,
    HeadMarker, InputSubmission, JoinPolicy, LatestFork, RewindableFork, ScopeRef,
    SubmissionRecord, SubmissionState, SubmissionStatus, SubmissionType, TaskOutcome,
    TaskOutcomeError, TaskRecord, TaskState, TaskStatus, WriteSubmission,
};
pub use storage::{Storage, StorageExt};

/// A document definition version's witness, re-exported for the same reason as [`DefVersion`].
///
/// A [`DocumentContent::Delta`] takes one, and the only way to obtain one is
/// [`cyrup_pico_doc::ReplayPlan::parse`] — which is what makes *"a delta cannot claim a version it did
/// not read"* (ADR-0030 F6 §C) true rather than aspirational. [`StoredDocument::version`] is how a
/// read hands it out.
pub use cyrup_pico_doc::StoredVersion;

/// A conversation id: `Id<Conversation>` (`spec.md:243`).
pub type ConversationId = Id<Conversation>;
/// An entry id: `Id<Entry>`.
///
/// **Not** `cyrup_core::EntryId`, which is an `Arc<str>` session token. See this module's
/// documentation for ADR-0030 open question 8 and why both keep the name.
pub type EntryId = Id<Entry>;
/// A task id: `Id<Task>`.
///
/// Upstream carries the task's result type as a second brand parameter
/// (`type TaskId<Result = unknown>`, `spec.md:245`). That half is deliberately absent: §5's durable
/// task machine is out of scope (ADR-0029), and ADR-0030 §2.4 records that when it is built the
/// task state is an `enum` reconstructed from storage, not a compile-time parameter.
pub type TaskId = Id<Task>;
/// A submission id: `Id<Submission>`.
pub type SubmissionId = Id<Submission>;
/// A document incarnation id: `Id<Document>`.
pub type DocumentId = Id<Document>;

#[cfg(test)]
mod tests;
