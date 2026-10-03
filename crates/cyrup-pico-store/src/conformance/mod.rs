//! The semantic conformance suite (`spec.md:4369`; ADR-0030 §8, §9, §10).
//!
//! > The semantic conformance suite covers memory, SQLite, and JSONL. — `spec.md:4369`
//!
//! # Why the suite is the contract, and not the trait
//!
//! [`Storage`] states what a backend must do; nothing in it can state that a backend **did** it.
//! ADR-0030 F3 is explicit about the largest example: *"atomicity in the medium is untouched… nothing
//! stops a backend partially applying it — that is conformance plus fault injection."* The same holds
//! for the honesty of a [`CommitError::Rejected`] classification, for sequence and id monotonicity
//! across a reopen, and for detachment. §10:4369 therefore makes the **suite** what makes backends
//! swappable, and ADR-0030 §9 lands it in this slice — before any durable backend exists — precisely
//! so `cyrup-pico-store-jsonl` (S7) and, if the trigger fires, `cyrup-pico-store-sqlite` (S12) have
//! something to be judged against on their first day rather than their last.
//!
//! # No `Session`, and that is the whole design
//!
//! ADR-0030 §8's first load-bearing reason for the crate split:
//!
//! > **The conformance suite must not depend on the Session.** §10's trust split is *"storage enforces
//! > atomicity, global id ownership, immutable conversation/entry creation, document record
//! > consistency and detachment; the Session owns semantic validity"* (`spec.md:4272-4277`). A suite
//! > that needs a Session to drive a backend cannot test that split.
//!
//! So every case here drives a `&mut dyn Storage` and builds its batches with
//! [`BatchBuilder`](crate::BatchBuilder). There is no `cyrup-pico` edge, and there cannot be one: this
//! module lives in the crate `cyrup-pico` depends on. That is also why [`Storage`] is written with
//! `#[async_trait]` rather than native `async fn` — a native one is not dyn-compatible, and the suite
//! would have to be generic over the backend, which is the shape that lets a Session edge creep in.
//!
//! # Running it
//!
//! Behind `feature = "conformance"`, so the cases are absent from an ordinary build of a crate that
//! only wants the contract. A backend crate adds
//! `cyrup-pico-store = { workspace = true, features = ["conformance"] }` as a **dev**-dependency,
//! implements [`StoreFactory`], and asserts on the [`Report`]:
//!
//! ```ignore
//! let report = cyrup_pico_store::conformance::run_all(&mut MyFactory).await;
//! assert!(report.is_green(), "{report}");
//! ```
//!
//! # What is here now, and what is not
//!
//! PICO5-PLAN S3 lands the harness and its first cases: read-your-commit, sequence strict increase,
//! mint monotonicity, detachment, and the two cross-batch rejections. The reopen suite (S7), fault
//! injection at three points (S8) and the query-plan suite (S10) extend it through [`StoreFactory`],
//! which is why the factory — not a bare `&mut dyn Storage` — is the entry point: a reopen case needs
//! to open the *same* store twice, and only the factory can know what that means.

mod cases;

use core::fmt;
use core::future::Future;
use core::pin::Pin;

use crate::{CommitError, Storage, StorageFailure};

pub use cases::CASES;

/// Opens the store under test.
///
/// Each case gets a **fresh** store, because a case that inherited another's records would be testing
/// the order the cases happen to run in.
///
/// S7's reopen suite is the reason this is a trait with a name rather than a closure: a reopen case
/// needs to close a store and open *the same* one again, and a durable factory can offer that while
/// [`MemoryStore`](crate::MemoryStore) cannot. The addition is a method with a default
/// implementation, not a change to this signature.
#[async_trait::async_trait]
pub trait StoreFactory: Send {
    /// Open an empty store.
    ///
    /// # Errors
    ///
    /// [`StorageFailure`] if the backend cannot be opened at all, which fails the suite rather than
    /// one case.
    async fn open(&mut self) -> Result<Box<dyn Storage>, StorageFailure>;
}

/// Why one case failed.
///
/// A case returns this rather than panicking, for two reasons: the workspace denies `panic!` and
/// `unwrap` outside a documented allow block, and a [`Report`] naming every failing case is more use
/// to a backend author than the first assertion that blew up.
#[derive(Debug, thiserror::Error)]
pub enum Failure {
    /// The backend did something the contract forbids.
    #[error("{0}")]
    Contract(String),
    /// A commit the case expected to succeed did not.
    #[error("an expected commit failed: {0}")]
    Commit(#[from] CommitError),
    /// A read the case expected to succeed did not.
    #[error("an expected read failed: {0}")]
    Read(#[from] StorageFailure),
    /// The case could not set itself up — a staging collision, an exhausted namespace.
    #[error("the case could not be set up: {0}")]
    Setup(String),
}

impl Failure {
    /// A contract violation, described.
    #[must_use]
    pub fn contract(what: impl Into<String>) -> Self {
        Self::Contract(what.into())
    }

    /// A setup failure, described.
    #[must_use]
    pub fn setup(what: impl Into<String>) -> Self {
        Self::Setup(what.into())
    }
}

/// One conformance case.
pub struct Case {
    /// Its name, as the report prints it.
    pub name: &'static str,
    /// What it asserts, and which line of the specification says so.
    pub about: &'static str,
    /// The case itself.
    pub run: fn(&mut dyn Storage) -> CaseFuture<'_>,
}

/// A case's future. Boxed because [`Case`] is a plain `fn` pointer in a `const` table, which cannot
/// name an `async fn`'s opaque return type.
pub type CaseFuture<'a> = Pin<Box<dyn Future<Output = Result<(), Failure>> + Send + 'a>>;

/// What one case did.
#[derive(Debug)]
pub struct Outcome {
    /// Which case.
    pub name: &'static str,
    /// Whether it held, and why not.
    pub result: Result<(), Failure>,
}

/// What the suite found.
#[derive(Debug)]
pub struct Report {
    /// One outcome per case, in the order [`CASES`] declares them.
    pub outcomes: Vec<Outcome>,
}

impl Report {
    /// Whether every case held.
    #[must_use]
    pub fn is_green(&self) -> bool {
        self.outcomes.iter().all(|o| o.result.is_ok())
    }

    /// How many cases ran.
    #[must_use]
    pub fn len(&self) -> usize {
        self.outcomes.len()
    }

    /// Whether no case ran, which is itself a failure to report.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.outcomes.is_empty()
    }

    /// How many cases failed.
    #[must_use]
    pub fn failures(&self) -> usize {
        self.outcomes.iter().filter(|o| o.result.is_err()).count()
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "{} of {} conformance cases held",
            self.len() - self.failures(),
            self.len()
        )?;
        for outcome in &self.outcomes {
            match &outcome.result {
                Ok(()) => writeln!(f, "  ok   {}", outcome.name)?,
                Err(e) => writeln!(f, "  FAIL {}: {e}", outcome.name)?,
            }
        }
        Ok(())
    }
}

/// Run every case against a fresh store from `factory`.
///
/// Never fails as a whole: a backend that cannot be opened is reported as every case failing to set
/// up, because a suite that returned one error would tell a backend author less than one that tells
/// them nothing worked.
pub async fn run_all(factory: &mut dyn StoreFactory) -> Report {
    let mut outcomes = Vec::with_capacity(CASES.len());
    for case in CASES {
        let result = match factory.open().await {
            Ok(mut store) => (case.run)(store.as_mut()).await,
            Err(e) => Err(Failure::setup(format!(
                "the store could not be opened: {e}"
            ))),
        };
        outcomes.push(Outcome {
            name: case.name,
            result,
        });
    }
    Report { outcomes }
}
