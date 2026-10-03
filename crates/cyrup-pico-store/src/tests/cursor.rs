//! Cursor tests (ADR-0030 F6 §B).
//!
//! The cross-*scan* half is a compile error and lives in
//! `tests/compile-fail/cursors_do_not_cross_scans.rs`. What is testable at runtime is the cross-*store*
//! half, which cannot be a type error because two stores have the same types.

use core::num::NonZeroU128;

use crate::{CursorBytes, EntryCursor, StoreId};

fn store(n: u128) -> StoreId {
    StoreId::new(NonZeroU128::new(n).expect("test store ids are nonzero"))
}

#[test]
fn a_cursor_reads_only_in_the_store_that_issued_it() {
    let issuer = store(7);
    let other = store(8);
    let cursor = EntryCursor::new(CursorBytes::new(issuer, vec![0, 0, 0, 0, 0, 0, 0, 41]));

    assert_eq!(
        cursor.payload_for(issuer).expect("its own store reads it"),
        &[0, 0, 0, 0, 0, 0, 0, 41]
    );
    let refused = cursor
        .payload_for(other)
        .expect_err("another store must not read it");
    assert_eq!(refused.issued_by, issuer);
    assert_eq!(refused.presented_to, other);
}

/// The payload is backend-private, so `Debug` must not print it: a `Debug` that did would be the
/// serialisation ADR-0030 §10's serde table withholds.
#[test]
fn debug_names_the_store_and_the_length_but_not_the_payload() {
    let rendered = format!("{:?}", CursorBytes::new(store(7), vec![1, 2, 3]));
    assert!(rendered.contains("3 bytes"), "{rendered}");
    assert!(!rendered.contains('1'), "{rendered}");
}
