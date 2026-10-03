//! The two **synchronous line observers**, their idempotent disposer, and the registry
//! [`crate::SessionShared::publish`] iterates.
//!
//! # The split this module exists to make unrepresentable
//!
//! `spec.md:1523-1525` draws one line and then draws it again: *"Commit observers run on the line;
//! document-state and watch user callbacks run later."* … *"Both return idempotent disposers; their
//! listeners must not throw, block, or call Session APIs. Document-state subscribers and watch
//! listeners still run later, off the line."*
//!
//! Two capabilities, not one with a flag:
//!
//! | | runs | signature | may |
//! |---|---|---|---|
//! | [`CommitObserver`] / [`CloseObserver`] | **on** the mutation line, synchronously | `fn(&_) -> ()` | capture already-immutable state |
//! | [`crate::DocState`] / [`crate::DocWatch`] listener | **off** the line, later | `async fn(..) -> Result<..>` | await, fail, own asynchronous work |
//!
//! Upstream has one sentence for the first row and a second sentence for the second, and nothing
//! distinguishes them at a call site. Here they are **different types with no conversion between
//! them**, so a listener written for one cannot be registered as the other — which is what
//! `G-SYNC-OBSERVERS-CAPTURE-ONLY` means once it stops being prose.
//!
//! # The three prohibitions, as the absence of three things
//!
//! ADR-0030 §2.3's row for `spec.md:1527-1533` classifies all three *unrepresentable*:
//!
//! * **cannot report failure** — [`CommitObserver::observe`] returns `()`. A `Result` return is
//!   `E0053` at the `impl`, pinned by
//!   `tests/compile-fail/a_sync_observer_cannot_report_failure.rs`. Upstream's failure mode is
//!   precise and bad: *"a throwing listener truncates the iteration **and** reports a durable,
//!   adopted commit as a failure"*.
//! * **cannot await** — the method is not `async` and the trait has no associated future. `E0053`
//!   again, pinned by `tests/compile-fail/a_sync_observer_cannot_await.rs`. An observer that could
//!   await would be inside the held line, which is invariant 4.
//! * **cannot re-enter the Session** — the argument is `&Publication` (or [`&Closing`](Closing)),
//!   and neither carries a [`Session`](crate::Session), a [`SessionMut`](crate::SessionMut) or a
//!   [`Committer`](crate::Committer). There is no accessor to call, pinned by
//!   `tests/compile-fail/a_sync_observer_cannot_reach_a_session_handle.rs`.
//!
//! And *"may only capture immutable state"* is not a rule an observer is asked to follow: every field
//! it can reach is a [`DocRoot`](cyrup_pico_doc::DocRoot) or a `Copy` id, and the publication itself
//! cannot be moved out of the borrow —
//! `tests/compile-fail/a_sync_observer_cannot_retain_the_publication.rs`.
//!
//! # The one thing that stays a runtime check, and why
//!
//! A panic. ADR-0030 F5 states the obligation and names it as the shell's: *"a throwing line observer
//! still truncates the iteration, which is a shell obligation (run them under `catch_unwind`; advance
//! all or none), not a type one."* [`Observers::publish_to`] is that `catch_unwind`, and the test is
//! `a_panicking_observer_does_not_truncate_the_publication`. It must be a **debug** test: F1 notes
//! that `[profile.release]`'s `panic = "abort"` (`Cargo.toml:447`) *"means a release build dies
//! instead of partially advancing, which removes pi's partial-advance hazard degenerately rather than
//! solving it"*.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, RwLock, Weak};

use cyrup_pico_store::StoreId;

use crate::witness::Publication;

/// A synchronous observer of committed publications.
///
/// Registered with [`Session::subscribe_commits`](crate::Session::subscribe_commits). Every closure
/// of the right shape is one, through the blanket impl below, so the common case needs no type.
///
/// Runs **on** the mutation line, after adoption, while the hold is still held. Takes the publication
/// by shared reference, returns nothing, is not `async`, and is given no Session handle.
pub trait CommitObserver: Send + Sync + 'static {
    /// Observe one publication.
    ///
    /// The publication is borrowed for the length of this call and no longer: it cannot be moved
    /// out, cloned or stored, so whatever the observer keeps is a clone of an already-immutable
    /// value.
    fn observe(&self, publication: &Publication);
}

impl<F: Fn(&Publication) + Send + Sync + 'static> CommitObserver for F {
    fn observe(&self, publication: &Publication) {
        self(publication);
    }
}

/// A synchronous observer of the Session closing.
///
/// `spec.md:1530-1532`: *"`subscribeClose()` observes close synchronously when it begins, after
/// admission is sealed."* Registered with
/// [`Session::subscribe_close`](crate::Session::subscribe_close) and run from
/// [`SessionMut::close`](crate::SessionMut::close) **after** the one-way seal is taken, which is why
/// close takes `&mut self` rather than `self` — S4's own documentation says so: *"which is also what
/// lets PICO5-PLAN S6's close observers run after admission is sealed and still have a Session to be
/// about."*
///
/// A separate trait from [`CommitObserver`], not the same trait with a flag: the two observe
/// different events and there is no shape in which one is the other.
pub trait CloseObserver: Send + Sync + 'static {
    /// Observe the close beginning.
    fn observe_close(&self, closing: &Closing);
}

impl<F: Fn(&Closing) + Send + Sync + 'static> CloseObserver for F {
    fn observe_close(&self, closing: &Closing) {
        self(closing);
    }
}

/// What a [`CloseObserver`] is told, which is **not** a Session.
///
/// Sealed exactly as [`Publication`] is, and for the same reason: nothing outside this module may
/// construct one, so a close cannot be announced by anything other than the close. Every field is
/// `Copy` immutable state, so *"may only capture immutable state"* is again a property of the
/// argument rather than a rule.
#[derive(Debug)]
pub struct Closing {
    store: StoreId,
    publications: u64,
    _seal: (),
}

impl Closing {
    /// **One call site**, in [`SessionMut::close`](crate::SessionMut::close).
    pub(crate) const fn new(store: StoreId, publications: u64) -> Self {
        Self {
            store,
            publications,
            _seal: (),
        }
    }

    /// The store being closed.
    #[must_use]
    pub const fn store_id(&self) -> StoreId {
        self.store
    }

    /// How many commits this Session published before the close began.
    #[must_use]
    pub const fn publications(&self) -> u64 {
        self.publications
    }
}

/// Something a [`Disposer`] can retire.
///
/// Object-safe on purpose: one [`Disposer`] type serves commit observers, close observers and
/// document subscriptions, so there is one idempotence argument to check rather than three.
pub(crate) trait Retirable: Send + Sync {
    /// Retire this registration. **Must be idempotent** — called by both
    /// [`Disposer::dispose`] and [`Disposer`]'s `Drop`.
    fn retire(&self);

    /// Whether it is already retired.
    fn is_retired(&self) -> bool;
}

/// A registration's live flag, for a registry whose entry needs nothing else.
#[derive(Debug, Default)]
pub(crate) struct Live(core::sync::atomic::AtomicBool);

impl Live {
    /// A live registration.
    pub(crate) fn new() -> Self {
        Self(core::sync::atomic::AtomicBool::new(true))
    }
}

impl Retirable for Live {
    fn retire(&self) {
        self.0.store(false, core::sync::atomic::Ordering::Release);
    }

    fn is_retired(&self) -> bool {
        !self.0.load(core::sync::atomic::Ordering::Acquire)
    }
}

/// An **idempotent by construction** cancellation of one registration.
///
/// `spec.md:1529`: *"Both return idempotent disposers."* Upstream's idempotence is a guard inside the
/// disposer's body; here it is the two things this type is made of and nothing else:
///
/// * a [`Weak`] slot — so a disposer that outlives its registry entry upgrades to `None` and has
///   nothing to do, rather than touching freed state or holding the registration alive;
/// * [`Drop`] — so a dropped disposer disposes, and *forgetting* to dispose is not a leak of a live
///   observer. [`Disposer::leak`] is the explicit opt-out for a registration meant to last the
///   Session's life, because "dispose on drop" must not be defeated by accident.
///
/// Idempotence then needs no guard: retiring is one atomic store of `false`, which is a no-op the
/// second time, and `Disposer` is **not `Clone`**, so two disposers for one registration do not
/// exist.
#[derive(Debug)]
#[must_use = "a Disposer IS the registration's lifetime: dropping it unregisters the observer. \
Bind it for as long as the observation should last, or call `Disposer::leak` to say that it \
should last the Session's life."]
pub struct Disposer {
    slot: Weak<dyn Retirable>,
    /// `true` once [`Disposer::leak`] gave up the drop.
    leaked: bool,
}

impl Disposer {
    /// Build a disposer over one registration.
    pub(crate) fn new(slot: &Arc<dyn Retirable>) -> Self {
        Self {
            slot: Arc::downgrade(slot),
            leaked: false,
        }
    }

    /// Retire the registration. Idempotent, and safe after the registry is gone.
    pub fn dispose(&self) {
        if let Some(slot) = self.slot.upgrade() {
            slot.retire();
        }
    }

    /// Whether the registration is retired — by this disposer, or because its registry closed.
    #[must_use]
    pub fn is_disposed(&self) -> bool {
        self.slot.upgrade().is_none_or(|slot| slot.is_retired())
    }

    /// Keep the registration for the Session's life, giving up the drop.
    ///
    /// The explicit form of *"I meant not to dispose this"*. Without it the only way to keep a
    /// subscription alive is to hold a value whose purpose is not obvious, and a reviewer cannot
    /// tell a deliberate permanent observer from a forgotten binding.
    pub fn leak(mut self) {
        self.leaked = true;
    }
}

impl Drop for Disposer {
    fn drop(&mut self) {
        if !self.leaked {
            self.dispose();
        }
    }
}

/// One registered commit observer.
struct CommitEntry {
    live: Live,
    observer: Box<dyn CommitObserver>,
}

impl Retirable for CommitEntry {
    fn retire(&self) {
        self.live.retire();
    }

    fn is_retired(&self) -> bool {
        self.live.is_retired()
    }
}

/// One registered close observer.
struct CloseEntry {
    live: Live,
    observer: Box<dyn CloseObserver>,
}

impl Retirable for CloseEntry {
    fn retire(&self) {
        self.live.retire();
    }

    fn is_retired(&self) -> bool {
        self.live.is_retired()
    }
}

/// The two synchronous observer registries, and the counter the all-or-nothing test reads.
#[derive(Default)]
pub(crate) struct Observers {
    commit: RwLock<Vec<Arc<CommitEntry>>>,
    close: RwLock<Vec<Arc<CloseEntry>>>,
    /// How many observer calls unwound in this process. A **diagnostic**, in the same spirit as
    /// [`crate::hold::overruns`]: a panicking observer is a bug in the observer, and the kernel's
    /// obligation is to carry on and to make the bug visible rather than to convert it into a
    /// durability incident.
    panics: core::sync::atomic::AtomicU64,
}

impl core::fmt::Debug for Observers {
    /// Hand-written because neither observer trait is `Debug`, and widening them would put a
    /// formatting obligation on every closure a host registers.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Observers")
            .field(
                "commit",
                &self.commit.read().map(|v| v.len()).unwrap_or_default(),
            )
            .field(
                "close",
                &self.close.read().map(|v| v.len()).unwrap_or_default(),
            )
            .field("panics", &self.panics())
            .finish()
    }
}

impl Observers {
    /// Register a commit observer. Returns its disposer.
    pub(crate) fn add_commit(&self, observer: Box<dyn CommitObserver>) -> Disposer {
        let entry: Arc<CommitEntry> = Arc::new(CommitEntry {
            live: Live::new(),
            observer,
        });
        let handle: Arc<dyn Retirable> = Arc::clone(&entry) as Arc<dyn Retirable>;
        let disposer = Disposer::new(&handle);
        if let Ok(mut list) = self.commit.write() {
            list.retain(|e| !e.is_retired());
            list.push(entry);
        }
        disposer
    }

    /// Register a close observer. Returns its disposer.
    pub(crate) fn add_close(&self, observer: Box<dyn CloseObserver>) -> Disposer {
        let entry: Arc<CloseEntry> = Arc::new(CloseEntry {
            live: Live::new(),
            observer,
        });
        let handle: Arc<dyn Retirable> = Arc::clone(&entry) as Arc<dyn Retirable>;
        let disposer = Disposer::new(&handle);
        if let Ok(mut list) = self.close.write() {
            list.retain(|e| !e.is_retired());
            list.push(entry);
        }
        disposer
    }

    /// Hand one publication to every live commit observer. **All of them, or none.**
    ///
    /// The list is cloned out of the lock before any observer runs — a handful of refcount bumps —
    /// for two reasons. An observer must not be able to deadlock the registry by registering or
    /// disposing from inside its own call, and a panic must not unwind through a held
    /// [`RwLock`] write guard and poison the registry for every later publication.
    ///
    /// Each call is wrapped in [`catch_unwind`], so one observer's panic advances the others instead
    /// of truncating the iteration, and the commit that is already durable and already adopted is
    /// still reported as committed. Under `panic = "abort"` the wrapper is inert and the process
    /// dies; that is ADR-0030 F1's *"removes pi's partial-advance hazard degenerately"*, and it is
    /// why the test for this is a debug test.
    pub(crate) fn publish_to(&self, publication: &Publication) {
        let live: Vec<Arc<CommitEntry>> = match self.commit.read() {
            Ok(list) => list
                .iter()
                .filter(|e| !e.is_retired())
                .map(Arc::clone)
                .collect(),
            // A poisoned registry means a panic escaped *this* function once before, which it
            // cannot: every call below is caught. Observing nothing is the only honest answer, and
            // it is recorded rather than silent.
            Err(_) => {
                self.panics
                    .fetch_add(1, core::sync::atomic::Ordering::Release);
                return;
            }
        };
        for entry in live {
            if catch_unwind(AssertUnwindSafe(|| entry.observer.observe(publication))).is_err() {
                self.panics
                    .fetch_add(1, core::sync::atomic::Ordering::Release);
            }
        }
    }

    /// Hand one [`Closing`] to every live close observer, under the same discipline.
    pub(crate) fn close_to(&self, closing: &Closing) {
        let live: Vec<Arc<CloseEntry>> = match self.close.read() {
            Ok(list) => list
                .iter()
                .filter(|e| !e.is_retired())
                .map(Arc::clone)
                .collect(),
            Err(_) => {
                self.panics
                    .fetch_add(1, core::sync::atomic::Ordering::Release);
                return;
            }
        };
        for entry in live {
            if catch_unwind(AssertUnwindSafe(|| entry.observer.observe_close(closing))).is_err() {
                self.panics
                    .fetch_add(1, core::sync::atomic::Ordering::Release);
            }
        }
    }

    /// How many observer calls have unwound in this Session.
    pub(crate) fn panics(&self) -> u64 {
        self.panics.load(core::sync::atomic::Ordering::Acquire)
    }
}
