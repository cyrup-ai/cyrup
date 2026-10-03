//! `spec.md:1305-1310`: *"`adopt()` ... performs only a synchronous pointer swap to the
//! already-computed immutable `value`. It performs no diffing, application, allocation, or
//! callback."*
//!
//! ADR-0030 F5 says no type proves that and asks for it to be asserted by counting. There are two
//! halves and this module has the one that needs no `#[global_allocator]`:
//!
//! * **the map-growth half** — the authority's two `HashMap`s must not grow during adoption, because
//!   growing one is the allocation the window is not allowed to make. The capacities are read through
//!   the fault seam before and after, so the two readings bracket exactly the adoption and nothing
//!   else;
//! * **the swap half** — what the authority holds afterwards is the value preparation already
//!   computed, not a value adoption rebuilt.
//!
//! The malloc-counting half is `tests/adoption_is_allocation_free.rs`: a counting
//! `#[global_allocator]` needs `unsafe`, which this crate forbids, so it lives in an integration test
//! where the `forbid` does not reach.

use std::sync::Arc;
use std::sync::Mutex;

use cyrup_pico_store::Cx;

use super::store_double::FaultyStore;
use super::{live_address, path};
use crate::{CommitOutcome, SessionMut};

/// Adoption grows neither authority map, and swaps in the value preparation already built.
#[tokio::test]
async fn adoption_allocates_no_map_capacity_and_rebuilds_no_value() {
    let (store, _switch) = FaultyStore::new();
    let (session, handle) = SessionMut::open(Box::new(store));
    let cx = Cx::detached();
    let address = live_address();

    // Create the document, so the measured commit is an ordinary adoption into a tracker that is
    // already there — the hot path a throttled live document takes, and the one ADR-0030 §2.3's
    // *"a 2 MB live document copies 2 MB per throttled commit"* failure mode is about.
    let CommitOutcome::Committed {
        session: handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "charging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the creating commit must succeed");
    };

    /// The two map capacities, read on one side of the adoption window.
    type Capacities = Option<(usize, usize)>;

    let measured: Arc<Mutex<(Capacities, Capacities)>> = Arc::new(Mutex::new((None, None)));
    {
        let m = Arc::clone(&measured);
        crate::fault::set_before_adopt(move |window| {
            if let Ok(mut slot) = m.lock() {
                slot.0 = Some(window.capacities());
            }
        });
        let m = Arc::clone(&measured);
        crate::fault::set_after_adopt(move |window| {
            if let Ok(mut slot) = m.lock() {
                slot.1 = Some(window.capacities());
            }
        });
    }
    let CommitOutcome::Committed {
        session: _handle, ..
    } = handle
        .commit(&cx, async |mut tx| {
            let doc = tx.doc(&address, ()).await?;
            tx.draft(doc)?.set(&path("phase"), "discharging")?;
            Ok(((), tx.writing()))
        })
        .await
    else {
        panic!("the measured commit must succeed");
    };
    crate::fault::clear();

    let (before, after) = {
        let slot = measured.lock().expect("the measurement is not poisoned");
        (
            slot.0.expect("the before hook ran"),
            slot.1.expect("the after hook ran"),
        )
    };
    assert_eq!(
        before, after,
        "adoption grew an authority map, which means it allocated"
    );

    let adopted = session
        .snapshot(address.address())
        .expect("the authority is healthy")
        .expect("the document is present");
    let phase = match adopted.get("phase") {
        Some(cyrup_pico_doc::DocValue::Str(s)) => Some(&**s),
        _ => None,
    };
    assert_eq!(
        phase,
        Some("discharging"),
        "the swap is a swap: the authority is the value preparation computed"
    );
}
