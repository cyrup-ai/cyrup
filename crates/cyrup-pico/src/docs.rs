//! The in-memory authority index: one [`Tracker`] per live incarnation, and the pointer swaps that
//! adoption consists of.
//!
//! # Why this is a `RwLock<HashMap<..>>` and not `arc_swap::ArcSwap`
//!
//! ADR-0030 §8 lists `arc-swap` among `cyrup-pico`'s dependencies, and for a *single* cell it would
//! be the right type. The authority here is not a cell, it is a **map** of them plus the per-document
//! revision that [`Tracker::adopt`] compares against — `spec.md:1305-1310`'s *"accepts only a
//! prepared result from that tracker at its current revision"*. Putting the map behind an `ArcSwap`
//! would mean allocating and publishing a **new map** on every adoption, which is precisely what
//! §2.3's *"adoption is a synchronous pointer swap — no diff, no apply, no allocation, no callback"*
//! forbids; putting an `ArcSwap` inside each entry would duplicate `Tracker`'s revision rule in a
//! second place and give the stale-preparation check nothing to compare against.
//!
//! A write lock is also not a contention question here, because F1 already decided there is exactly
//! one writer: [`SessionMut`] is not `Clone`, never in an `Arc`, and lives in one actor. The lock is
//! held for the length of a `HashMap::insert`, with no `await` inside it. The deviation from §8's
//! dependency list is recorded rather than taken silently.
//!
//! # How adoption stays allocation-free
//!
//! [`DocIndex::reserve`] is called during **preparation**, before the storage commit, and
//! [`DocIndex::adopt`] then inserts into capacity that already exists. The [`Swap`] list is likewise
//! built during preparation, so adoption moves values rather than building them. ADR-0030 F5 asks for
//! this to be *"assert\[ed\] with a test that counts allocations"* rather than claimed, and
//! `src/tests/adoption.rs` is that test.
//!
//! [`SessionMut`]: crate::SessionMut

use std::collections::HashMap;
use std::sync::RwLock;

use cyrup_pico_doc::{DocRoot, OpenChange, Prepared, Revision, Tracker};
use cyrup_pico_store::{DocumentAddress, DocumentId};

use crate::witness::AdoptionCause;

/// One pointer swap, assembled during preparation and applied during adoption.
#[derive(Debug)]
pub(crate) enum Swap {
    /// An incarnation this commit created becomes the live authority at its address.
    Create {
        /// The new incarnation.
        id: DocumentId,
        /// Its logical address, moved in so adoption does not clone a [`Kind`](cyrup_pico_store::Kind).
        address: DocumentAddress,
        /// Its first value. Already an `Arc`, so this is a refcount, not a copy.
        value: DocRoot,
    },
    /// An existing incarnation's authority is **replaced wholesale**, because this commit migrated
    /// its stored value to the token's definition version (`spec.md:1441-1444`).
    ///
    /// Not an [`Swap::Adopt`], and the difference is the whole reason this variant exists. A
    /// migration's candidate is prepared over a tracker built from the *stored* value, not over the
    /// authority's current revision — because the authority still holds the pre-migration value and
    /// publishing the migrated one before storage committed would break invariant 2. So there is no
    /// revision for [`Tracker::adopt`] to match, and the swap installs a new [`Tracker`] at the same
    /// id. Reaching the authority at all is what `spec.md:1463-1465`'s required base pays for.
    Rebase {
        /// The incarnation.
        id: DocumentId,
        /// Its logical address, which does not change.
        address: DocumentAddress,
        /// The migrated, now durable value.
        value: DocRoot,
    },
    /// An existing incarnation adopts a prepared change.
    Adopt {
        /// The incarnation.
        id: DocumentId,
        /// The preparation, which already holds the computed value.
        prepared: Prepared,
    },
    /// An incarnation stops being the live authority at its address.
    ///
    /// Its final content, if any, is in the [`Batch`](cyrup_pico_store::Batch) and in the
    /// publication's [`Change`](crate::Change) — `spec.md:1242-1244`'s *"retirement of an acquired
    /// draft persists its final content before retirement"*. There is nothing to adopt, because a
    /// retired incarnation has no live reader: a later read of that address resolves a **new**
    /// incarnation, and a historical read goes to storage.
    Retire {
        /// The incarnation leaving.
        id: DocumentId,
        /// The address it vacates.
        address: DocumentAddress,
    },
}

/// The live document authority.
#[derive(Debug, Default)]
pub(crate) struct DocIndex {
    cells: RwLock<Cells>,
}

#[derive(Debug, Default)]
struct Cells {
    /// Every incarnation loaded or created in this Session, by id.
    by_id: HashMap<DocumentId, Tracker>,
    /// Which incarnation is live at each logical address.
    current: HashMap<DocumentAddress, DocumentId>,
}

/// The lock was poisoned by a panic while the authority was being read or swapped.
///
/// Reported rather than ignored, because a panic inside the swap window is exactly the state F1's
/// *"a panic in a line observer still unwinds"* note is about: the authority may be half-swapped, and
/// continuing would prepare the next change against a baseline nobody can describe. A caller turns
/// this into [`AdoptionCause`] or into a read failure, never into `None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, thiserror::Error)]
#[error("the document authority is poisoned: a panic unwound through a swap")]
pub struct AuthorityPoisoned;

impl DocIndex {
    /// An empty index.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// The live incarnation at one address.
    pub(crate) fn current_id(
        &self,
        address: &DocumentAddress,
    ) -> Result<Option<DocumentId>, AuthorityPoisoned> {
        let cells = self.cells.read().map_err(|_| AuthorityPoisoned)?;
        Ok(cells.current.get(address).copied())
    }

    /// The committed value of one incarnation, as a shareable immutable revision.
    ///
    /// `spec.md:4528-4530`: *"every published revision is immutable for all time"*. The returned
    /// [`DocRoot`] is an `Arc` over a type with no interior-mutable variant, so this is a refcount
    /// bump and the caller cannot reach a `&mut` path from it (ADR-0030 F4).
    pub(crate) fn value_of(&self, id: DocumentId) -> Result<Option<DocRoot>, AuthorityPoisoned> {
        let cells = self.cells.read().map_err(|_| AuthorityPoisoned)?;
        Ok(cells.by_id.get(&id).map(|t| t.value().clone()))
    }

    /// One incarnation's committed value **and** the tracker revision it is at, in one acquisition.
    ///
    /// PICO5-PLAN S6's registration needs the pair, not the two halves: a value read at one moment and
    /// a revision read at another cannot be compared against a published frame's revision, and either
    /// order of two separate reads admits a stale or a duplicated first frame. The whole of
    /// `crates/cyrup-pico/src/slot.rs`'s ordering table rests on this being **one** lock.
    ///
    /// `None` is absence — the incarnation is not in the authority, which for a subscription means its
    /// address has no live incarnation to bind to.
    pub(crate) fn baseline_of(
        &self,
        id: DocumentId,
    ) -> Result<Option<(DocRoot, Revision)>, AuthorityPoisoned> {
        let cells = self.cells.read().map_err(|_| AuthorityPoisoned)?;
        Ok(cells
            .by_id
            .get(&id)
            .map(|t| (t.value().clone(), t.revision())))
    }

    /// Open a change over an incarnation's current authority revision.
    ///
    /// `None` when the incarnation is not loaded — the caller then reads storage and calls
    /// [`DocIndex::load`].
    pub(crate) fn begin_change(
        &self,
        id: DocumentId,
    ) -> Result<Option<OpenChange>, AuthorityPoisoned> {
        let cells = self.cells.read().map_err(|_| AuthorityPoisoned)?;
        Ok(cells.by_id.get(&id).map(Tracker::begin_change))
    }

    /// Record a **committed** value read from storage as the live authority at its address.
    ///
    /// This publishes nothing and does not violate invariant 2: the value was already durable before
    /// this call — loading is how the Session learns what is already true, which is why
    /// `spec.md:1245` can say *"ordinary documents are not scanned at open"* and still have a
    /// coherent authority.
    pub(crate) fn load(
        &self,
        id: DocumentId,
        address: DocumentAddress,
        value: DocRoot,
    ) -> Result<OpenChange, AuthorityPoisoned> {
        let mut cells = self.cells.write().map_err(|_| AuthorityPoisoned)?;
        let tracker = cells
            .by_id
            .entry(id)
            .or_insert_with(|| Tracker::track(value));
        let change = tracker.begin_change();
        cells.current.insert(address, id);
        Ok(change)
    }

    /// Make room for `creations` new incarnations, **before** the storage commit.
    ///
    /// This is the allocation [`DocIndex::adopt`] must not make. Called from preparation, where
    /// allocating is free of consequence because nothing durable has happened yet.
    pub(crate) fn reserve(&self, creations: usize) -> Result<(), AuthorityPoisoned> {
        if creations == 0 {
            return Ok(());
        }
        let mut cells = self.cells.write().map_err(|_| AuthorityPoisoned)?;
        cells.by_id.reserve(creations);
        cells.current.reserve(creations);
        Ok(())
    }

    /// Apply the prepared swaps. **The only mutating path**, and it allocates nothing.
    ///
    /// Runs after storage committed, where failure is unrecoverable
    /// ([`AdoptionFailed`](crate::AdoptionFailed)), so everything that could have failed — path
    /// walking, operation application, value construction, capacity growth — already happened during
    /// preparation. What is left is two `HashMap` writes per document and one revision increment.
    pub(crate) fn adopt(&self, swaps: Vec<Swap>) -> Result<(), AdoptionCause> {
        let mut cells = self
            .cells
            .write()
            .map_err(|_| AdoptionCause::AuthorityPoisoned)?;
        for swap in swaps {
            match swap {
                Swap::Create { id, address, value } | Swap::Rebase { id, address, value } => {
                    cells.by_id.insert(id, Tracker::track(value));
                    cells.current.insert(address, id);
                }
                Swap::Adopt { id, prepared } => {
                    let Some(tracker) = cells.by_id.get_mut(&id) else {
                        return Err(AdoptionCause::DocumentVanished(id));
                    };
                    tracker
                        .adopt(prepared)
                        .map_err(|stale| AdoptionCause::StalePreparation {
                            prepared_against: stale.prepared_against.get(),
                            authority_at: stale.tracker_at.get(),
                        })?;
                }
                Swap::Retire { id, address } => {
                    cells.by_id.remove(&id);
                    if cells.current.get(&address) == Some(&id) {
                        cells.current.remove(&address);
                    }
                }
            }
        }
        Ok(())
    }

    /// The two maps' capacities, so a test can assert that [`DocIndex::adopt`] did not grow them.
    ///
    /// Feature-gated with [`DocIndex::forget`] for the same reason: both exist only for the window
    /// [`crate::fault`] opens, and a production build should not carry either.
    #[cfg(feature = "fault-injection")]
    pub(crate) fn capacities(&self) -> (usize, usize) {
        self.cells.read().map_or((0, 0), |cells| {
            (cells.by_id.capacity(), cells.current.capacity())
        })
    }

    /// Drop one incarnation from the authority. **The fault seam, and nothing else uses it.**
    ///
    /// See [`crate::fault`] for why a seam into this window exists at all.
    #[cfg(feature = "fault-injection")]
    pub(crate) fn forget(&self, id: DocumentId) {
        if let Ok(mut cells) = self.cells.write() {
            cells.by_id.remove(&id);
        }
    }
}
