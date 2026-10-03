//! Drives PICO5-PLAN S3's conformance suite against this backend.
//!
//! `spec.md:4369` makes the **suite**, not the trait, what makes backends swappable — *"the semantic
//! conformance suite covers memory, SQLite, and JSONL"* — and ADR-0030 §9 lands it before any durable
//! backend exists precisely so this crate *"has something to be judged against on its first day rather
//! than its last"*. This file is that judgement, through the public API and the `conformance` feature,
//! with no access to anything crate-private.
//!
//! Note what the factory does: a **fresh directory per case**, because the suite's own contract is that
//! each case gets a fresh store. The reopen cases this slice owes are not here — see the crate
//! documentation for why a statement about a medium cannot be a case `MemoryStore` also has to pass.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_pico_store::conformance::{CASES, Report, StoreFactory, run_all};
use cyrup_pico_store::{Storage, StorageFailure};
use cyrup_pico_store_jsonl::{Durability, JsonlOptions, JsonlStore, StoreLock};

/// Opens one JSONL store per case, in a directory of its own.
///
/// The `TempDir`s are kept alive for the whole run: dropping one would delete the store under the case
/// still using it.
struct Jsonl {
    durability: Durability,
    dirs: Vec<tempfile::TempDir>,
}

#[async_trait::async_trait]
impl StoreFactory for Jsonl {
    async fn open(&mut self) -> Result<Box<dyn Storage>, StorageFailure> {
        let dir = tempfile::tempdir().map_err(StorageFailure::Io)?;
        let lock = StoreLock::acquire(dir.path())
            .map_err(|e| StorageFailure::Io(std::io::Error::other(e.to_string())))?;
        let store =
            JsonlStore::open_for_write(dir.path(), lock, JsonlOptions::new(self.durability))?;
        self.dirs.push(dir);
        Ok(Box::new(store))
    }
}

/// The backend is conformant at the **strong** tier.
#[tokio::test]
async fn the_jsonl_backend_is_conformant_with_power_loss_durability() {
    let mut factory = Jsonl {
        durability: Durability::PowerLoss,
        dirs: Vec::new(),
    };
    let report: Report = run_all(&mut factory).await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.len(), CASES.len());
}

/// And at the weak one, because `spec.md:4393-4395` makes the tiers differ in *"durability envelope and
/// performance"* and in nothing else. A case that passed at one tier and failed at the other would mean
/// the envelope had leaked into the semantics.
#[tokio::test]
async fn the_jsonl_backend_is_conformant_with_process_crash_durability() {
    let mut factory = Jsonl {
        durability: Durability::ProcessCrash,
        dirs: Vec::new(),
    };
    let report: Report = run_all(&mut factory).await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.len(), CASES.len());
}
