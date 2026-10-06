//! ADR-0030 F5's one runtime obligation, done the way it asks: *"keep it a small `#[inline]` function
//! and assert it with a test that counts allocations."*
//!
//! `spec.md:1305-1310`: *"`adopt()` ... performs only a synchronous pointer swap to the
//! already-computed immutable `value`. It performs no diffing, application, allocation, or
//! callback."* ADR-0030 §2.3 says why it matters and it is not performance: *"adoption runs after
//! storage committed, where failure is unrecoverable: anything that can fail there turns a committed
//! write into a poisoned Session."* An allocation can fail.
//!
//! # Why this lives in an integration test
//!
//! Counting allocations needs a `#[global_allocator]`, and implementing [`GlobalAlloc`] needs
//! `unsafe` — which `cyrup-pico`'s `#![forbid(unsafe_code)]` rules out, correctly. A `tests/` target
//! is a separate crate, so the `forbid` does not reach it and the one `unsafe impl` the measurement
//! needs is confined to a file that ships in no binary.
//!
//! The `src/tests/adoption.rs` companion asserts the same thing without a counter, by reading the
//! authority's map capacities on both sides of the window. Two measurements of one property, because
//! each one's blind spot is the other's: a capacity reading cannot see an allocation that was not a
//! map growth, and a malloc counter cannot say *which* allocation it saw.
//!
//! # Why the counter is thread-local
//!
//! Each case arms a counter around its own adoption, and `cargo test` runs the two cases
//! CONCURRENTLY on two threads of one process. That is the shape that has to be right: the merge
//! gate is `cargo test --workspace` — the root `Cargo.toml` says so, and `.config/nextest.toml`'s
//! own header concedes it (*"`cargo test --workspace` alone remains the gate until someone
//! validates a full-workspace nextest run"*) — so the per-process isolation `cargo nextest` would
//! give is not something this file may assume.
//!
//! While the counter and the flag were process-global atomics, a `#[global_allocator]` could not
//! tell the two threads apart: one case's ordinary allocations landed in the other's count, and
//! either case's before-adopt hook could zero the other's counter mid-measurement. Measured on one
//! prebuilt binary, that was **10 failures in 40** default-shape runs, in either case
//! interchangeably — and `--test-threads=1` only hid it.
//!
//! So both live in a [`thread_local!`], which makes the measurement match the seam it reads:
//! `fault::set_before_adopt` installs a hook on the CALLING THREAD (`src/fault.rs`'s `BEFORE_ADOPT`
//! is itself a `thread_local!`), and `#[tokio::test(flavor = "current_thread")]` polls the whole
//! commit on that same thread. Each case therefore counts exactly the allocations its own adoption
//! made and nothing else, and the two may run in parallel because they no longer share state. No
//! lock, and no `--test-threads=1` for the reader to remember.
//!
//! `tests/line_hold_budget.rs` keeps its sibling case as a plain `async fn` rather than a second
//! `#[test]`, and that is still right for it: the line-hold budget is process-global *in the
//! kernel* and no test can make it per-thread. This file's state is the test's own, so it can be,
//! and both cases stay named tests.
//!
//! [`GlobalAlloc`]: std::alloc::GlobalAlloc

#![allow(unsafe_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use cyrup_pico::{CommitOutcome, DocToken, SessionMut};
use cyrup_pico_doc::{Path, Seg};
use cyrup_pico_store::{Cx, MemoryStore};

thread_local! {
    /// Allocations seen **on this thread** while [`ARMED`] is set on it.
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    /// Whether to count on this thread. Set by the before-adopt hook and cleared by the after-adopt
    /// hook, so the counted window is exactly `Tx::adopt` — on the one thread that armed it.
    static ARMED: Cell<bool> = const { Cell::new(false) };
}

/// Count one allocation, if the allocating thread is the armed one.
///
/// [`LocalKey::try_with`] and not `with`: this runs inside [`GlobalAlloc`], where a panic aborts the
/// process. Both thread-locals are `const`-initialised and neither `Cell` is `Drop`, so there is no
/// destructor to have run and no lazy initialisation that could itself allocate — but the allocator
/// is the one place where being wrong about that costs an abort rather than a red test, so a miss is
/// simply not counted.
///
/// [`LocalKey::try_with`]: std::thread::LocalKey::try_with
#[inline]
fn count_allocation() {
    if ARMED.try_with(Cell::get).unwrap_or(false) {
        let _ = ALLOCATIONS.try_with(|seen| seen.set(seen.get() + 1));
    }
}

struct Counting;

// SAFETY: every method forwards to `System`, unchanged, after a thread-local increment. The counter adds
// no aliasing, no layout assumption and no ownership claim of its own.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }
}

#[global_allocator]
static ALLOC: Counting = Counting;

/// The definition this file speaks. `spec.md:1224-1225`: *"ordinary definitions are not registered"*, so
/// a definition is a type and the token is built where it is used.
struct Live;

#[derive(serde::Serialize)]
struct Empty {}

impl cyrup_pico::DocDef for Live {
    type Value = Empty;
    type Place = cyrup_pico::SessionScoped;
    type Shape = cyrup_pico::Singleton;
    type Seed = ();
    const KIND: &'static str = "test.live";
    const VERSION: u32 = 1;
    const POLICY: () = ();
    fn initial((): ()) -> Empty {
        Empty {}
    }
}

fn address() -> cyrup_pico::DocAddress<Live> {
    DocToken::<Live>::define()
        .expect("test.live is a definition")
        .at()
}

fn path(key: &str) -> Path {
    Path::new([Seg::key(key).expect("a safe key")]).expect("a safe path")
}

#[tokio::test(flavor = "current_thread")]
async fn adoption_performs_no_allocation() {
    let (session, handle) = SessionMut::open(Box::new(MemoryStore::new()));
    let cx = Cx::detached();
    let addr = address();

    // Create the document first. The creating commit DOES allocate — it inserts a new incarnation
    // into the authority — and `DocIndex::reserve` makes that allocation happen during preparation,
    // before storage was admitted, which is the whole trick.
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&addr, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };

    cyrup_pico::fault::set_before_adopt(|_window| {
        ALLOCATIONS.set(0);
        ARMED.set(true);
    });
    cyrup_pico::fault::set_after_adopt(|_window| {
        ARMED.set(false);
    });

    // The measured commit: an ordinary change to a document the authority already holds. This is the
    // throttled-live-document path, which is the one that runs thousands of times in a session.
    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&addr, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the measured commit must succeed");
    };
    cyrup_pico::fault::clear();
    ARMED.set(false);

    assert_eq!(
        ALLOCATIONS.get(),
        0,
        "adoption allocated: `spec.md:1305-1310` forbids it, and the window is unrecoverable"
    );
    assert_eq!(session.publications(), 2);
}

/// The same measurement for a commit that **creates** an incarnation, which is the case that could
/// plausibly allocate — it inserts into two `HashMap`s — and must not, because
/// [`DocIndex::reserve`] moved the growth into preparation.
#[tokio::test(flavor = "current_thread")]
async fn adopting_a_creation_performs_no_allocation_either() {
    let (session, handle) = SessionMut::open(Box::new(MemoryStore::new()));
    let cx = Cx::detached();
    let addr = address();

    cyrup_pico::fault::set_before_adopt(|_window| {
        ALLOCATIONS.set(0);
        ARMED.set(true);
    });
    cyrup_pico::fault::set_after_adopt(|_window| {
        ARMED.set(false);
    });

    let CommitOutcome::Committed { .. } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&addr, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };
    cyrup_pico::fault::clear();
    ARMED.set(false);

    assert_eq!(
        ALLOCATIONS.get(),
        0,
        "adopting a creation allocated: `DocIndex::reserve` is supposed to have made room already"
    );
    assert_eq!(session.publications(), 1);
}
