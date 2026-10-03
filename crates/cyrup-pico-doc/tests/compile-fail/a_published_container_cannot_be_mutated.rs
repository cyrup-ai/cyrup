//! `G-TRUSTED-IMMUTABLE`, the borrow half — ADR-0030 §2.2 (`spec.md:4528-4530`).
//!
//! ADR-0030 F4's concrete failure mode, verified from source rather than asserted: *"A TUI mount holds
//! `state.value` ... and a renderer sorts `value.tools` in place to display them ordered. `tools` is a
//! tracker-owned container shared with the authority revision and with every other observer's retained
//! revision. §12 says plainly that runtime freezing is not provided, so nothing throws: the committed
//! revision every other mount and every replica computed from is now reordered, with no write, no
//! commit and no event to attribute it to."*
//!
//! Here it is two borrow errors. `Arc` offers no `&mut`, `Arc::get_mut` requires uniqueness (and the
//! refcount is >= 2 for anything reachable from a published revision, by construction — see
//! `Tracker::begin_change`), and `Arc::make_mut` clones when shared, which *is* copy-on-write and is
//! correct.
//!
//! In its own file because the sibling case's name-resolution errors abort the compile before borrow
//! checking runs, which would make this error silently absent from the snapshot.

use cyrup_pico_doc::{DocRoot, DocValue};

fn main() {
    let published: DocRoot = DocRoot::empty();
    let retained = published.clone().into_value();

    if let DocValue::List(items) = &retained {
        // The renderer's in-place sort, which upstream cannot detect.
        items.sort_by(|_, _| core::cmp::Ordering::Equal);
        items.push(DocValue::Null);
    }
}
