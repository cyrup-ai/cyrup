//! Compile-fail proof for the ADR-0030 §2 rows this crate classifies `unrepresentable`.
//!
//! ADR-0030 §7's rule: a guarantee carried **only by a signature** needs a case proving the invalid
//! program is rejected, because deleting the signature is invisible in a diff and no runtime test
//! turns red. Every row below is such a guarantee. Nothing here checks a runtime rejection — those
//! are ordinary unit tests in `src/tests/`, and `DocValue::parse`'s non-finite check is deliberately
//! among them, because §2.3 classifies that one `guarded` rather than `unrepresentable`.
//!
//! | case | guarantee | ADR-0030 §2 row |
//! |---|---|---|
//! | `published_revision_has_no_mutable_view` | `G-TRUSTED-IMMUTABLE` | §2.2 `spec.md:4528-4530` |
//! | `document_value_has_no_bypass_for_parse` | `G-INV-6` (value half) | §2.1 invariant 6, §2.3 `spec.md:72, 1356-1369` |
//! | `a_read_does_not_borrow_from_the_backend` | `G-OWNERSHIP-BOUNDARY` | §2.2 `spec.md:1560-1566` |
//! | `replay_plan_cannot_be_forged` | `G-CORRUPTION-NOT-ABSENCE` | §2.2 `spec.md:4352-4360` |
//! | `stored_version_cannot_be_forged` | `G-VERSION-PER-RECORD` | §2.3 `spec.md:1119-1120, 4366-4367` |
//!
//! The snapshots in `compile-fail/*.stderr` are rustc output and are therefore toolchain-sensitive.
//! Regenerate them with `TRYBUILD=overwrite cargo test -p cyrup-pico-doc --test compile_fail` after a
//! toolchain bump, and read the diff: a changed *error code* is a change in the guarantee, a changed
//! *wording* is not.

#[test]
fn document_core_misuse_does_not_compile() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/compile-fail/*.rs");
}
