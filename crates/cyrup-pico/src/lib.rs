//! `cyrup-pico` — the Pico5 durability kernel's **imperative shell** (ADR-0030 §8).
//!
//! PICO5-PLAN **S4**, **S5** and **S6** live here: the mutation line, the transaction, drafts, the
//! durability witnesses, the typed document surface, and observation. S4 is the architectural
//! commitment of the whole design and the slice S0 existed to de-risk.
//!
//! # The three mechanisms, and what each one deletes upstream
//!
//! **F1 — one mutation handle, and an outcome that does not give it back.** [`Session`] is clonable
//! and can only read; [`SessionMut`] is not `Clone`, never in an `Arc`, owns `Box<dyn Storage>`, and
//! is **consumed** by [`SessionMut::commit`]. [`CommitOutcome`] returns it in three variants and not
//! in the fourth. That replaces a `#poison` flag checked on every entry point, an
//! `instanceof StorageRejected` test, and `spec.md:4581`'s request that callers not catch and
//! continue.
//!
//! **F2 — `Tx<Reading> → Tx<Writing>`, and a draft that borrows the transaction.** The four table
//! readers exist on one state and not the other, so `ReadAfterWrite` is deleted as an error class;
//! [`Draft`] borrows the transaction, so the revoking `Proxy`, the `#sealed` flag and
//! `#assertOpen()` on every access are deleted; and [`Tx`] holds no Session handle and no effect
//! capability, so a nested commit has no method to call.
//!
//! **F5 — a witness chain from durability to publication.** `Durable` is minted at one call site,
//! consumed by [`Tx::adopt`], which is the only producer of a [`Publication`], which is consumed by
//! the only publish there is. Publishing before durability has no spelling.
//!
//! # The guarantees this slice discharges
//!
//! | handle | ADR-0030 §2 row | enforcement | where the proof is |
//! |---|---|---|---|
//! | `G-INV-2` | §2.1 invariant 2 | `unrepresentable` | [`Publication`]'s sealed private fields and crate-private constructor: `a_publication_cannot_be_forged` (`E0451`), `a_publication_has_no_public_constructor` (`E0624`) |
//! | `G-INV-3` (emitter half) | §2.1 invariant 3 | `unrepresentable` | the same witness: a second emitter cannot construct the argument — the same two cases |
//! | `G-INV-4` | §2.1 invariant 4 | `unrepresentable` (reachable path, nested commit) + `guarded` (captured handle) + `checked` ([`hold`]) | `a_future_borrowing_the_transaction_cannot_be_spawned`, `a_nested_commit_cannot_be_started_from_the_transaction` |
//! | `G-INV-6` | §2.1 invariant 6 | `unrepresentable` | `a_draft_cannot_outlive_the_commit_callback`, `a_draft_cannot_be_returned_from_the_commit_callback`, `a_draft_cannot_be_spawned`; [`Draft::set`] for the value half |
//! | `G-INV-7` | §2.1 invariant 7 | `unrepresentable` (one committer, cannot fail, cannot re-enter) + `checked` (continuity of the hold) | `a_session_mut_cannot_be_cloned`, `a_session_mut_cannot_be_used_after_commit` at the kernel seam and `cyrup-pico-store`'s `one_committer_at_a_time` at the storage seam; [`Line`] for the hold |
//! | `G-INV-8` | §2.1 invariant 8 | `typestate` (consuming) at the kernel; `checked` at the [`Committer`] boundary | `a_session_mut_cannot_be_obtained_from_an_uncertain_outcome` |
//! | `G-READ-BEFORE-FIRST-TABLE-WRITE` | §2.1 invariant 4's table rules, `spec.md:1553-1557` | `typestate` | `a_table_read_after_a_table_write_does_not_compile` |
//! | `G-PRE-ADMISSION-ROLLBACK` | §2.1 invariant 8, second sentence | `guarded` — see below | every [`RollbackReason`] arm arrives in [`CommitOutcome::RolledBack`], which carries the handle; `cyrup-pico-store`'s `io_error_is_not_a_rejection` closes the way in |
//! | `G-INV-3` (observer half) | §2.1 invariant 3 | `unrepresentable` | an observer is handed `&Publication` and cannot move, clone or forge one: `a_sync_observer_cannot_retain_the_publication` |
//! | `G-SYNC-OBSERVERS-CAPTURE-ONLY` | §2.3 `spec.md:1527-1533` | `unrepresentable` for all three prohibitions; `checked` for panics | `a_sync_observer_cannot_report_failure`, `a_sync_observer_cannot_await`, `a_sync_observer_cannot_reach_a_session_handle`; [`Observers::publish_to`](observe) for the panic half |
//! | `G-DOC-SOURCE-NO-MUTABLE-OBJECT` | §2.3 `spec.md:1126`, `:3822` | `unrepresentable` | [`Revision`] has no `&mut` path: `a_published_revision_has_no_mutable_path` |
//! | `G-SCOPE-DETERMINES-LIFETIME` | §2.3 `spec.md:923-943`, `:1066-1071` | `unrepresentable` in the token (four halves) | `fork_as_of_requires_rewindable_history`, `a_session_document_cannot_declare_a_history_policy`, `a_task_document_cannot_be_addressed_in_a_conversation`, `a_family_cannot_be_addressed_without_a_key` |
//! | `G-ONLY-TYPED-ACQUISITION-CREATES` | §2.3 `spec.md:1219` | `unrepresentable` (privacy) | `only_the_typed_acquisition_creates_a_document` |
//! | `G-TOKEN-AGREES-WITH-RECORD` | §2.3 `spec.md:1063-1065`, `:1117-1120` | `checked`, **deliberately** | [`TxError::TokenDisagreesWithRecord`]; `a_token_that_redeclares_history_is_refused` |
//! | `G-MIGRATION-ACCESS-DRIVEN` | §2.3 `spec.md:1441-1469` | `enum` ([`cyrup_pico_doc::VersionFit`]) | the four arms, one test each, in `src/tests/migration.rs` |
//! | `G-FIRST-WRITE-AFTER-MIGRATION-IS-BASE` | §2.3 `spec.md:1463-1465` | `guarded` by a flag that cannot be cleared | `Continuity::Migrated`; `an_older_version_migrates_and_the_first_write_is_a_required_base` |
//! | `G-CHECKPOINT-ONCE` | §2.3 `spec.md:1389-1401` | `guarded` + pure core | [`cyrup_pico_doc::choose_representation`]'s single call site; `the_checkpoint_predicate_runs_once_per_change_on_a_count_that_excludes_it` |
//!
//! Every `unrepresentable` and `typestate` claim above carries a `trybuild` case, because ADR-0030
//! §7's rule is that a guarantee carried **only by a signature** needs a case proving the invalid
//! program is rejected: deleting the signature is invisible in a diff and no runtime test turns red.
//! All of them are in `tests/compile-fail/` except `G-INV-7`'s storage half, which is
//! `cyrup-pico-store`'s `one_committer_at_a_time` — the `&mut self` it pins is that crate's signature,
//! so the case lives with the signature.
//!
//! `G-PRE-ADMISSION-ROLLBACK` reads `guarded` and not `typestate` because a compile-fail case cannot
//! state it. The claim is that the handle **is** in the rollback arm, and a compile error only ever
//! proves an absence; there is no invalid program to reject, and removing `session` from
//! [`CommitOutcome::RolledBack`] breaks every caller's `match` loudly rather than silently. What *can*
//! go wrong quietly is an uncertain failure arriving as a rollback, and that is closed in the type:
//! [`RollbackReason::Rejected`] carries a **closed** [`RejectedReason`] with no string arm and no
//! `From<std::io::Error>`, which `cyrup-pico-store`'s `io_error_is_not_a_rejection` pins. ADR-0030
//! §2.1's invariant-8 row keeps `typestate` for the *first* sentence — the consuming
//! [`SessionMut::commit`] — which `a_session_mut_cannot_be_obtained_from_an_uncertain_outcome` proves.
//!
//! [`RejectedReason`]: cyrup_pico_store::RejectedReason
//!
//! # The runtime checks that stay
//!
//! ADR-0030 §7 is explicit — *"do not delete the runtime checks yet"* — and this slice deletes none.
//! `Durable` is crate-private, so a backend cannot be *made* to produce one; invariant 4 is `guarded`
//! and a captured effect handle still compiles ([`hold`] is its budget); a backend's failure
//! classification can still lie (PICO5-PLAN S8's fault injection is what judges that).
//!
//! # PICO5-PLAN S6, and the two names it had to choose
//!
//! S6 is observation and publication: [`CommitObserver`] and [`CloseObserver`] on the line,
//! [`DocState`] and [`DocWatch`] off it, [`Disposer`], [`Revision`], [`Frame`] and [`FrameContext`].
//! Two places where the plan's literal wording could not be taken, both recorded here rather than
//! papered over:
//!
//! * **`Revision<T>`'s accessor is `raw() -> &DocRoot`, not `-> &Arc<DocRoot>`.** S2 made
//!   [`DocRoot`](cyrup_pico_doc::DocRoot) a newtype *over* `Arc<DocMap>`, so the plan's
//!   `Arc<DocRoot>` would be an `Arc` over an `Arc`: a second allocation and a second indirection
//!   buying no sharing that the inner one does not already provide. The guarantee the signature
//!   carries — a shared reference, never `&mut`, to an immutable value — is unchanged.
//! * **`cyrup_pico::Revision<T>` and `cyrup_pico_doc::Revision` are different types with the same
//!   name, and neither is renamed.** The second is S2's per-tracker adoption counter; this one is a
//!   published document revision tagged with the type it was acquired as. They share no construction
//!   path, so a module that imports both unqualified is rejected by `E0252` and must say which it
//!   means — which is the outcome ADR-0030 open question 8 argued for over a rename, and the same
//!   argument applies unchanged.
//!
//! S6 also leaves **ADR-0030 open question 6 open on purpose**: cyrup still has no value-carrying
//! context, so [`FrameContext::for_commit`](FrameContext) produces an empty one. The *values* half of
//! that type is provisional; the *no token field* half is the guarantee and is not.
//!
//! # PICO5-PLAN S5, and the one check it deleted
//!
//! S5 is documents in the session: [`DocDef`] and [`DocToken`] in [`def`], the six typed
//! [`DocAddress`] constructors, the token-versus-record agreement check, get-or-create as the only
//! creating path, [`Tx::retire_doc`], and the four-way migration ladder with the required base that
//! follows it.
//!
//! It deletes one of S4's runtime checks, and names it rather than letting it vanish:
//! `TxError::ScopeDisagreesWithAddress` is **gone**, because a [`DocAddress`] writes the storage
//! address and the full [`DocumentScope`](cyrup_pico_store::DocumentScope) from one argument in one
//! constructor. ADR-0030 §7 says not to delete runtime checks *yet*; this is not a deletion on
//! confidence but on a state that stopped being expressible, and
//! `every_constructor_writes_an_address_that_agrees_with_its_own_policy` checks the property across
//! all six constructors — which the check it replaces could only ever see one of at a time.
//!
//! The check it does **not** delete is the one ADR-0030 §2.3 calls *"correct as a check"*:
//! [`TxError::TokenDisagreesWithRecord`]. Extension code reloads independently of its data, so the
//! only authority on a stored document's scope and history is the persisted record
//! (`spec.md:1063-1065`), and preferring the token is how a `latest` document starts answering
//! historical reads from legitimately reclaimed records.
//!
//! # PICO5-PLAN S9, and the one thing it could not take from the plan's wording
//!
//! S9 is forks and copy-source isolation: [`Tx::fork_conversation`], the [`fork`] module's [`Fork`]
//! and [`ForkedDocument`], [`Copied`] on the publication, and
//! [`PreparationFailed::ForkSourceWritten`]. Its three guarantees:
//!
//! | handle | ADR-0030 §2 row | enforcement | where the proof is |
//! |---|---|---|---|
//! | `G-FORK-POINT-ONE-ENTRY` | §2.3 `spec.md:1473-1477` | `checked` | [`Tx::fork_conversation`] resolves one [`ConversationParent`](cyrup_pico_store::ConversationParent) through `Storage::entry_in`; `a_fork_point_must_be_visible_in_the_parents_history` |
//! | `G-FORK-POLICY-PERSISTED` | §2.3 `spec.md:1478-1479` | `checked`, plus `unrepresentable` for *"never from the token"* | the fork path takes no definition and no token; `a_fork_policy_cannot_be_supplied_by_the_caller`, `a_forked_document_cannot_be_forged` |
//! | `G-COPY-SOURCE-SNAPSHOT-ISOLATION` | §2.3 `spec.md:1491-1506`, `:4313-4318` | `checked` pre-admission, and `unrepresentable` for *"copy metadata is never a document value"* | `src/tests/forks.rs`; `a_copied_document_has_no_value` |
//!
//! The one place the plan's wording could not be taken literally is the **name**: S9 says
//! `ForkPolicy::{AsOf, Current, Initial}`, and what exists is [`RewindableFork`] plus [`LatestFork`],
//! because ADR-0030 §2.3 and S1/S3 split the policy across the two histories so that
//! `{"history":"latest","fork":"asOf"}` is an unknown-variant error rather than a validated pair
//! (`tests/compile-fail/fork_as_of_requires_rewindable_history.rs`). A single `ForkPolicy` would
//! reunite them and give that illegal pair a spelling again, so the fork matches on
//! [`ConversationSemantics::fork`]'s widening instead — which is the method S1 added *for this slice*
//! and names it so. ADR-0030 wins; the plan's name is the discrepancy.
//!
//! [`RewindableFork`]: cyrup_pico_store::RewindableFork
//! [`LatestFork`]: cyrup_pico_store::LatestFork
//! [`ConversationSemantics::fork`]: cyrup_pico_store::ConversationSemantics::fork
//!
//! # What is deliberately not here
//!
//! * **§5's durable task machine** — out of build scope entirely (ADR-0029). [`Tx::stage_task`] is
//!   named `stage_` for that reason.

#![forbid(unsafe_code)]

mod committer;
mod def;
mod docs;
#[cfg(feature = "fault-injection")]
pub mod fault;
pub mod fork;
pub mod hold;
mod observe;
mod outcome;
mod revision;
mod session;
mod slot;
mod state;
mod tx;
mod watch;
mod witness;

pub use committer::{Committer, Line, LineClosed};
pub use def::{
    ConversationScoped, DocAddress, DocDef, DocToken, Family, FamilyKey, MAX_FAMILY_KEY_LEN,
    MigrateFn, Migration, MigrationFailed, NotADefinition, NotAFamilyKey, Placement, SessionScoped,
    Shape, Singleton, TaskScoped,
};
pub use docs::AuthorityPoisoned;
pub use fork::{Fork, ForkedDocument};
pub use observe::{CloseObserver, Closing, CommitObserver, Disposer};
pub use outcome::{
    CommitOutcome, CommitReply, RollbackReason, Settled, UncertainCommit, UncertainKind,
};
pub use revision::{BrokenDocument, Frame, FrameContext, Revision};
pub use session::{Session, SessionMut, SessionShared};
pub use slot::{ListenerFailed, MAX_PENDING_FRAMES, WatchEnd};
pub use state::{Attachment, DocState};
pub use tx::{
    CallbackError, CheckpointPredicate, ConversationOwnership, DocHandle, Draft, DraftError,
    EntryDraft, PreparationFailed, Reading, Tx, TxError, Writing,
};
pub use watch::{DocWatch, Watching};
pub use witness::{AdoptionCause, AdoptionFailed, Change, Copied, Publication};

#[cfg(test)]
mod tests;
