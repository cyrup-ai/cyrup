//! `DiskStore::rewrite` must leave its replacement durable, not merely visible
//! (PICO5-PLAN S11; ADR-0030 §3 F6 §D, §14 open question 1).
//!
//! The first test here is the regression test for the defect: it fails against the pre-S11 tree,
//! where `rewrite` ended at `std::fs::rename` and never asked for the rewritten directory entry
//! back. See `crate::durable` for why the mechanism is what is asserted and the power-loss outcome
//! is not.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::header::SessionHeader;
use crate::store::{DiskStore, SessionStore};

fn header(cwd: &str) -> SessionHeader {
    SessionHeader::new("s11".into(), cwd, "2026-01-01T00:00:00Z")
}

/// THE S11 REGRESSION TEST. Red before `rewrite` routed through `durable_rename`, green after.
#[cfg(unix)]
#[test]
fn rewrite_fsyncs_the_directory_its_rename_rewrote() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s11-fsync.jsonl");
    let mut store = DiskStore::new(&path);

    assert!(
        !crate::durable::dir_was_fsynced(dir.path()),
        "a fresh tempdir cannot already have been synced"
    );

    store
        .rewrite(&header(&dir.path().to_string_lossy()), &[])
        .unwrap();

    assert!(
        crate::durable::dir_was_fsynced(dir.path()),
        "rewrite's rename is durable only if the parent directory is fsynced after it"
    );
}

/// `rewrite` over a live file: the replacement is the whole content, the temp sibling is gone, and
/// the file is still a loadable session. Guards the functional half of routing through the helper.
#[test]
fn rewrite_replaces_a_live_file_and_leaves_no_temp_sibling() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s11-replace.jsonl");
    let mut store = DiskStore::new(&path);

    store
        .create_exclusive(&header("/proj/before"), &[])
        .unwrap();
    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("/proj/before")
    );

    store.rewrite(&header("/proj/after"), &[]).unwrap();

    let body = std::fs::read_to_string(&path).unwrap();
    assert!(body.contains("/proj/after"));
    assert!(!body.contains("/proj/before"), "rewrite must not append");
    assert!(body.ends_with('\n'));
    assert!(
        !path.with_extension("jsonl.tmp").exists(),
        "the temp sibling must be consumed by the rename"
    );
}

/// `rewrite` creates the parent directory chain it needs. Pinned because `durable_rename` now also
/// opens that directory, so a missing-parent regression would surface as an `fsync` failure rather
/// than as the `create_dir_all` it really is.
#[test]
fn rewrite_creates_a_missing_parent_chain() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep").join("deeper").join("s11.jsonl");
    let mut store = DiskStore::new(&path);

    store.rewrite(&header("/proj/deep"), &[]).unwrap();

    assert!(
        std::fs::read_to_string(&path)
            .unwrap()
            .contains("/proj/deep")
    );
}
