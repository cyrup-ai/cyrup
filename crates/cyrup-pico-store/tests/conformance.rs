//! Drives PICO5-PLAN S3's conformance suite against the reference backend.
//!
//! This is an **integration** test on purpose: it links `cyrup_pico_store` the way
//! `cyrup-pico-store-jsonl` (S7) and `cyrup-pico-store-sqlite` (S12) will, through the public API and
//! the `conformance` feature, with no access to anything crate-private. If a case here needed something
//! private, the suite would not be runnable by the backends it exists to judge — which is the one thing
//! `spec.md:4369` asks of it.
//!
//! Note what is absent: a `Session`. ADR-0030 §8's first load-bearing reason for the crate split is
//! that §10's trust split (`spec.md:4272-4277`) cannot be tested by a suite that needs one.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_pico_store::conformance::{CASES, Report, StoreFactory, run_all};
use cyrup_pico_store::{MemoryStore, Storage, StorageFailure};

/// Opens a fresh [`MemoryStore`] per case.
struct Memory;

#[async_trait::async_trait]
impl StoreFactory for Memory {
    async fn open(&mut self) -> Result<Box<dyn Storage>, StorageFailure> {
        Ok(Box::new(MemoryStore::new()))
    }
}

#[tokio::test]
async fn the_memory_backend_is_conformant() {
    let report: Report = run_all(&mut Memory).await;
    assert!(report.is_green(), "{report}");
    assert_eq!(report.len(), CASES.len());
    assert!(
        !report.is_empty(),
        "a suite that runs nothing proves nothing"
    );
}

/// The report is what a backend author reads, so its failure path is worth one test of its own: a
/// backend that cannot be opened must fail every case rather than erroring once.
#[tokio::test]
async fn a_backend_that_cannot_open_fails_every_case() {
    struct Broken;

    #[async_trait::async_trait]
    impl StoreFactory for Broken {
        async fn open(&mut self) -> Result<Box<dyn Storage>, StorageFailure> {
            Err(StorageFailure::Io(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "the store directory is not writable",
            )))
        }
    }

    let report = run_all(&mut Broken).await;
    assert!(!report.is_green());
    assert_eq!(report.failures(), CASES.len());
    assert!(
        report.to_string().contains("not writable"),
        "the report must carry the reason: {report}"
    );
}
