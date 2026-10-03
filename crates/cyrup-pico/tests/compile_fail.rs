//! Compile-fail proof for the ADR-0030 §2 rows this crate claims `unrepresentable` or `typestate`.
//!
//! ADR-0030 §7's rule: a guarantee carried **only by a signature** needs a case proving the invalid
//! program is rejected, because deleting the signature is invisible in a diff and no runtime test
//! turns red. §7 warrants compile-fail tests *"for exactly four guarantees"*; this suite has those
//! four plus the canary PICO5-PLAN S4 asks for by name, plus one case per later slice's own
//! `unrepresentable`/`typestate` row, plus the four S0 proved against its sketch types and re-pointed
//! here when the spike crate was retired.
//!
//! | case | guarantee | ADR-0030 |
//! |---|---|---|
//! | `a_draft_cannot_outlive_the_commit_callback` | `G-INV-6` (escape) | §7 case 1, F2 |
//! | `a_draft_cannot_be_returned_from_the_commit_callback` | `G-INV-6` (return) | §7 case 1, F2; PICO5-PLAN S0 |
//! | `a_draft_cannot_be_spawned` | `G-INV-6` (spawn) | §7 case 1, F2; S0 |
//! | `a_future_borrowing_the_transaction_cannot_be_spawned` | `G-INV-4` (reachable path) | §7 case 2, F2 |
//! | `the_commit_future_is_not_send` | what §10's `_not_send` marker costs, not a guarantee | §7 case 2's measurement, F1; S0 |
//! | `a_table_read_after_a_table_write_does_not_compile` | `G-READ-BEFORE-FIRST-TABLE-WRITE` | §7 case 3, F2 |
//! | `a_session_mut_cannot_be_obtained_from_an_uncertain_outcome` | `G-INV-8` | §7 case 4, F1 |
//! | `a_session_mut_cannot_be_cloned` | `G-INV-7` (one committer, kernel seam) | §2.1 invariant 7, F1 |
//! | `a_session_mut_cannot_be_used_after_commit` | `G-INV-7` (one committer, kernel seam) | §2.1 invariant 7, F1 |
//! | `a_publication_cannot_be_forged` | `G-INV-2`, `G-INV-3` (emitter half) | §2.1 invariants 2–3, F5 |
//! | `a_publication_has_no_public_constructor` | `G-INV-2`, `G-INV-3` (emitter half) | §2.1 invariants 2–3, F5 |
//! | `a_nested_commit_cannot_be_started_from_the_transaction` | `G-INV-4` (nested commit) | F2; PICO5-PLAN S4's canary |
//! | `a_doc_handle_cannot_cross_transactions` | F2's branded `'tx` handle | F2; S0 |
//! | `a_sync_observer_cannot_report_failure` | `G-SYNC-OBSERVERS-CAPTURE-ONLY` (1/3) | §2.3 `spec.md:1527-1533`, F1 |
//! | `a_sync_observer_cannot_await` | `G-SYNC-OBSERVERS-CAPTURE-ONLY` (2/3) | §2.3, F1 |
//! | `a_sync_observer_cannot_reach_a_session_handle` | `G-SYNC-OBSERVERS-CAPTURE-ONLY` (3/3) | §2.3, F1 |
//! | `a_sync_observer_cannot_retain_the_publication` | `G-INV-3` (observer half) | §2.1 invariant 3, F5 |
//! | `a_published_revision_has_no_mutable_path` | `G-DOC-SOURCE-NO-MUTABLE-OBJECT` | §2.3 `spec.md:1126`; PICO5-PLAN S6's named canary |
//! | `a_frame_context_cannot_inherit_the_producers_cancellation` | invariant 7's split; ADR-0030 open question 6 | §9.2 `spec.md:3889` |
//! | `a_watch_cannot_install_a_second_listener` | `DocWatch::start` consuming | `spec.md:3797`; PICO5-PLAN S6 |
//! | `an_attachment_cannot_be_activated_twice` | `Attachment::activate` consuming | `spec.md:1515-1519`; PICO5-PLAN S6 |
//! | `fork_as_of_requires_rewindable_history` | `G-SCOPE-DETERMINES-LIFETIME` (fork) | §2.3 `spec.md:1068-1069`, F6 §C; PICO5-PLAN S5 |
//! | `a_session_document_cannot_declare_a_history_policy` | `G-SCOPE-DETERMINES-LIFETIME` (policy) | §2.3 `spec.md:923-925`; S5 |
//! | `a_task_document_cannot_be_addressed_in_a_conversation` | `G-SCOPE-DETERMINES-LIFETIME` (scope) | §2.3 `spec.md:1070-1071`; S5 |
//! | `a_family_cannot_be_addressed_without_a_key` | `G-SCOPE-DETERMINES-LIFETIME` (shape) | §2.3 `spec.md:4341-4342`; S5 |
//! | `a_fourth_document_scope_cannot_be_declared` | the closed set of three scopes | §2.3 `spec.md:927-943`, F6 §A's sealing pattern; S5 |
//! | `only_the_typed_acquisition_creates_a_document` | `G-ONLY-TYPED-ACQUISITION-CREATES` | §2.3 `spec.md:1219`; S5 |
//! | `a_copied_document_has_no_value` | `G-COPY-SOURCE-SNAPSHOT-ISOLATION` (metadata half) | §2.3 `spec.md:1502-1504`; PICO5-PLAN S9 |
//! | `a_forked_document_cannot_be_forged` | `G-FORK-POLICY-PERSISTED` (*"never from the token"*) | §2.3 `spec.md:1478-1479`, `:1117-1120`; S9 |
//! | `a_fork_policy_cannot_be_supplied_by_the_caller` | `G-FORK-POLICY-PERSISTED` (the signature) | §2.3 `spec.md:1478-1479`; S9 |
//!
//! `G-INV-2` and `G-INV-3`'s *emitter* half take **two** files between them, and the split is the one
//! thing about this suite that is not obvious. `Publication`'s name is `pub` — ADR-0030 §10 writes the
//! type `pub(crate)` and in the same block puts it in a public trait's method signature, which is
//! `E0446` — so the type is nameable from a `trybuild` file and both invalid programs are writable:
//! `Publication::new(..)` is `E0624` and `Publication { .. }` is `E0451`. They cannot share a file,
//! because `E0624` is a type-check error and `E0451` comes from the privacy pass, which does not run
//! once type-checking has failed; one file would pin only the constructor half and leave `_seal: ()`
//! unproved. `Durable` is the type that genuinely has no constructor to name — it is `pub(crate)`, so
//! its half of F5's chain is carried by the privacy boundary itself.
//!
//! The snapshots in `compile-fail/*.stderr` are rustc output and are therefore toolchain-sensitive.
//! Regenerate with `TRYBUILD=overwrite cargo test -p cyrup-pico --test compile_fail` after a
//! toolchain bump, and read the diff: a changed **error code** is a change in the guarantee, a changed
//! **wording** is not.

#[test]
fn kernel_misuse_does_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
