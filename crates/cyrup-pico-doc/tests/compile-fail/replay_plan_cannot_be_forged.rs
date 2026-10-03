//! `G-CORRUPTION-NOT-ABSENCE` — ADR-0030 §2.2 (`spec.md:4352-4360`): *"a legitimately absent record
//! is never reported as damaged data, or vice versa"*, with `ReplayPlan::parse` as *"where the two
//! corruption rules live"* (F6 §C).
//!
//! The guarantee is that `materialize` is **downstream of the parser**: there is no way to materialize
//! a value from records whose three corruption rules were never checked. Upstream's separation is
//! entirely *which code path threw*, so the equivalent mistake there is one `catch` away.

use cyrup_pico_doc::{DocRoot, ReplayPlan, StoredContent, materialize};

fn main() {
    // No struct literal: the fields are private, so a plan cannot be asserted into existence.
    let forged = ReplayPlan {
        value: DocRoot::empty(),
    };

    // And `materialize` takes a plan, not records: the unchecked path has no signature.
    let records: &[StoredContent] = &[];
    let _ = materialize(records);

    let _ = forged;
}
