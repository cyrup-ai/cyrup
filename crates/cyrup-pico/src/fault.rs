//! The adoption window, opened for a test. **`feature = "fault-injection"`, off by default.**
//!
//! # Why a seam exists at all
//!
//! ADR-0030 §7 requires fault injection to hit **three** distinguishable points, *"not two"*: before
//! admission; admitted-then-unknown; and **committed-then-adoption-failed**. It then says why the
//! third one matters — *"it is the one a port will skip and the one pi poisons on that nobody
//! expects, and it is why `UncertainKind` has two variants."*
//!
//! The first two points are reachable from outside: a callback can fail, and a backend can return
//! [`CommitError::Uncertain`](cyrup_pico_store::CommitError::Uncertain). The third is **not**, and
//! that is by design — the whole of ADR-0030 F1 and F5 is that between a successful
//! `Storage::commit` and a successful adoption there is nothing a caller can reach. A guarantee whose
//! failure mode is unreachable is also untestable, so the window is opened here, behind a feature a
//! production build never turns on.
//!
//! PICO5-PLAN S8 is the slice that uses this in anger; S4 uses it for two cases ADR-0030 names
//! explicitly: *"both `Uncertain` paths killing the loop and closing every ticket"*, and *"`adopt`
//! being allocation-free (assert by counting)"* — the second needs the window bracketed, because
//! counting allocations across a whole commit measures preparation, which is *supposed* to allocate.
//!
//! # What it cannot do
//!
//! It cannot forge a [`Durable`](crate::Publication) or a [`Publication`](crate::Publication), cannot
//! commit, and cannot publish. [`AdoptWindow`] offers exactly one operation — forget one incarnation —
//! which is the minimum that makes the adoption-failure arm reachable.

use core::cell::RefCell;

use cyrup_pico_store::DocumentId;

use crate::docs::DocIndex;

type BeforeHook = Box<dyn Fn(&AdoptWindow<'_>)>;
type AfterHook = Box<dyn Fn(&AdoptWindow<'_>)>;

thread_local! {
    static BEFORE_ADOPT: RefCell<Option<BeforeHook>> = const { RefCell::new(None) };
    static AFTER_ADOPT: RefCell<Option<AfterHook>> = const { RefCell::new(None) };
}

/// The authority index, during the one window where nothing else can reach it.
pub struct AdoptWindow<'a> {
    docs: &'a DocIndex,
}

impl AdoptWindow<'_> {
    /// Drop one incarnation from the authority, so the adoption that follows fails with
    /// [`AdoptionCause::DocumentVanished`](crate::AdoptionCause::DocumentVanished).
    ///
    /// This is a *simulation* of the hazard, not the hazard: a real adoption failure is a panic
    /// unwinding or a poisoned lock. What it reproduces faithfully is the thing under test — storage
    /// committed, memory did not, and the Session must be reopened with nothing to reconcile.
    pub fn forget(&self, id: DocumentId) {
        self.docs.forget(id);
    }

    /// The authority's two map capacities.
    ///
    /// Read before and after adoption, the pair brackets exactly the window `spec.md:1305-1310`
    /// forbids allocation in: if either number moved, a `HashMap` grew, and growing one is an
    /// allocation. This is the half of ADR-0030 F5's *"assert it with a test that counts
    /// allocations"* that needs no `#[global_allocator]`.
    #[must_use]
    pub fn capacities(&self) -> (usize, usize) {
        self.docs.capacities()
    }
}

/// Run `hook` on this thread immediately before the next adoption.
pub fn set_before_adopt(hook: impl Fn(&AdoptWindow<'_>) + 'static) {
    BEFORE_ADOPT.with_borrow_mut(|slot| *slot = Some(Box::new(hook)));
}

/// Run `hook` on this thread immediately after the next adoption returns.
pub fn set_after_adopt(hook: impl Fn(&AdoptWindow<'_>) + 'static) {
    AFTER_ADOPT.with_borrow_mut(|slot| *slot = Some(Box::new(hook)));
}

/// Remove both hooks on this thread.
pub fn clear() {
    BEFORE_ADOPT.with_borrow_mut(|slot| *slot = None);
    AFTER_ADOPT.with_borrow_mut(|slot| *slot = None);
}

/// Fire the before-adopt hook. Called from the kernel's one adoption site.
pub(crate) fn fire_before_adopt(docs: &DocIndex) {
    let window = AdoptWindow { docs };
    BEFORE_ADOPT.with_borrow(|slot| {
        if let Some(hook) = slot.as_ref() {
            hook(&window);
        }
    });
}

/// Fire the after-adopt hook.
pub(crate) fn fire_after_adopt(docs: &DocIndex) {
    let window = AdoptWindow { docs };
    AFTER_ADOPT.with_borrow(|slot| {
        if let Some(hook) = slot.as_ref() {
            hook(&window);
        }
    });
}
