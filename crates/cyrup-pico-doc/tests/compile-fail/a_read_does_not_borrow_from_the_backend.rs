//! `G-OWNERSHIP-BOUNDARY` — ADR-0030 §2.2 (`spec.md:1560-1566`): *"storage and the Session never
//! alias caller memory."*
//!
//! ADR-0030 F4's *"guarantee not gained"* item 4 says the freeness of deleting `memory.ts:92-100`'s
//! recursive clone *"holds only if reads return owned or `Arc<immutable>` values, never a reference
//! into a cache"*, and that a backend author wanting `&DocValue` from a cache *"hits the `async
//! fn`-in-trait lifetime wall and will reach for a workaround."* This case is the wall, in the pure
//! half where it can be shown: a reference taken out of a replay plan is bounded by that plan, so the
//! workaround does not compile, and `materialize` — which returns an owned root — is the path that
//! does.

use cyrup_pico_doc::{DocRoot, ReplayPlan, StoredContent, materialize};

/// The shape a cache-returning read would want: hand back a borrow that outlives the thing it came
/// from.
fn cached_read(records: &[StoredContent]) -> &DocRoot {
    let plan = ReplayPlan::parse(records).expect("replayable");
    plan.value()
}

/// What is actually available, for contrast — owned, in O(1), because a `DocRoot` is an `Arc`.
fn owned_read(records: &[StoredContent]) -> DocRoot {
    materialize(&ReplayPlan::parse(records).expect("replayable"))
}

fn main() {
    let _ = cached_read(&[]);
    let _ = owned_read(&[]);
}
