//! Structural sharing, and the canary for the refcount-at-least-two invariant.
//!
//! These two properties are the whole of `G-TRUSTED-IMMUTABLE` on the value side, and neither can be
//! asserted any way but by pointer identity — which is why `Arc::ptr_eq` appears here and nowhere
//! else.

use std::sync::Arc;

use crate::tests::{list, p, root};
use crate::{DocValue, Op, Tracker, apply};

/// Pull the `Arc` out of a list value, for an identity comparison.
fn list_arc(value: &DocValue) -> &Arc<Vec<DocValue>> {
    match value {
        DocValue::List(items) => items,
        other => panic!("expected a list, found {}", other.type_name()),
    }
}

/// ADR-0030 §2.3: *"unchanged subtrees are structurally shared between successive revisions"* —
/// upstream an implementation detail of `applyImmutable`, here a refcount consequence of
/// `Arc::make_mut`. The failure it prevents is *"a 2 MB live document copies 2 MB per throttled
/// commit."*
#[test]
fn an_untouched_subtree_is_the_same_allocation_before_and_after_a_change() {
    let untouched = list(&[DocValue::integer(1), DocValue::integer(2)]);
    let before = root(&[
        ("touched", DocValue::string("a")),
        ("untouched", untouched),
        (
            "deep",
            root(&[("inner", list(&[DocValue::integer(3)]))]).into_value(),
        ),
    ]);

    let after = apply(
        &before,
        &[Op::AppendStr {
            path: p(&["touched"]),
            text: "b".into(),
        }],
    )
    .expect("applies");

    let before_untouched = before.get("untouched").expect("present");
    let after_untouched = after.get("untouched").expect("present");
    assert!(
        Arc::ptr_eq(list_arc(before_untouched), list_arc(after_untouched)),
        "an untouched sibling subtree was copied instead of shared"
    );

    // And one level deeper, because the interesting failure is a spine walk that copies too much.
    let before_deep = before.get("deep").and_then(DocValue::as_map).expect("map");
    let after_deep = after.get("deep").and_then(DocValue::as_map).expect("map");
    assert!(Arc::ptr_eq(
        list_arc(before_deep.get("inner").expect("present")),
        list_arc(after_deep.get("inner").expect("present")),
    ));

    assert_eq!(after.get("touched"), Some(&DocValue::string("ab")));
}

/// **The canary PICO5-PLAN S2 asks for.**
///
/// `Tracker::begin_change` clones the authority root's `Arc`, putting every container reachable from
/// the published revision at refcount >= 2. That single line is what makes every `Arc::make_mut`
/// below it copy instead of mutating the published revision in place — so it is the whole of
/// `spec.md:4528-4530`'s *"trusted immutable revisions"*, and if it is ever changed to a move this
/// test is what fails.
///
/// Upstream cannot have this test: `packages/chord/src/delta/README.md` says plainly that *"nothing
/// is frozen or defensively copied, so an illegal mutation is not detected. It silently corrupts
/// state."*
#[test]
fn authority_revision_is_untouched_by_a_draft_write() {
    let tracker = Tracker::track(root(&[
        ("output", DocValue::string("")),
        ("entries", list(&[DocValue::integer(1)])),
    ]));
    let authority_before = tracker.value().clone();
    let entries_arc_before = Arc::clone(list_arc(tracker.value().get("entries").expect("present")));

    let mut change = tracker.begin_change();
    change
        .append_str(&p(&["output"]), "done\n")
        .expect("appends");
    change
        .set(&p(&["output"]), DocValue::string("replaced"))
        .expect("sets");

    // The authority revision is byte-for-byte what it was, and the same allocation.
    assert_eq!(tracker.value(), &authority_before);
    assert_eq!(tracker.value().get("output"), Some(&DocValue::string("")));
    assert!(tracker.value().shares_allocation_with(&authority_before));
    // The subtree the change never touched is still shared with it, not copied.
    assert!(Arc::ptr_eq(
        &entries_arc_before,
        list_arc(
            change
                .read()
                .as_map()
                .expect("map")
                .get("entries")
                .expect("present")
        ),
    ));

    // And the candidate did move.
    let prepared = change.prepare().expect("prepares");
    assert_eq!(
        prepared.value().get("output"),
        Some(&DocValue::string("replaced"))
    );
    assert_eq!(prepared.ops().len(), 2);
}

/// Adoption is a pointer swap: the adopted value **is** the prepared candidate, the same allocation,
/// so `spec.md:1305-1310`'s *"no diffing, application, allocation, or callback"* is visible rather
/// than asserted in prose.
#[test]
fn adoption_swaps_the_pointer_to_the_prepared_candidate() {
    let mut tracker = Tracker::track(root(&[("n", DocValue::integer(1))]));
    let mut change = tracker.begin_change();
    change.set(&p(&["n"]), DocValue::integer(2)).expect("sets");
    let prepared = change.prepare().expect("prepares");
    let candidate = prepared.value().clone();

    tracker.adopt(prepared).expect("adopts");

    assert!(tracker.value().shares_allocation_with(&candidate));
    assert_eq!(tracker.revision().get(), 1);
}

/// *"Several changes may be open or prepared from one revision. Adopting one makes all others
/// stale."* One check over a consumed value, rather than upstream's four mechanisms.
#[test]
fn a_competing_preparation_is_stale_after_an_adoption() {
    let mut tracker = Tracker::track(root(&[("n", DocValue::integer(1))]));

    let mut first = tracker.begin_change();
    first.set(&p(&["n"]), DocValue::integer(2)).expect("sets");
    let mut second = tracker.begin_change();
    second.set(&p(&["n"]), DocValue::integer(3)).expect("sets");

    tracker
        .adopt(first.prepare().expect("prepares"))
        .expect("the first adoption succeeds");
    let err = tracker
        .adopt(second.prepare().expect("prepares"))
        .expect_err("the competitor is stale");
    assert_eq!(err.prepared_against.get(), 0);
    assert_eq!(err.tracker_at.get(), 1);
    assert_eq!(tracker.value().get("n"), Some(&DocValue::integer(2)));
}

/// `prepareReplace` is a whole-root operation, not a diff: an O(1) ownership move, and a
/// deeply-equal replacement keeps the current root rather than emitting an op.
#[test]
fn prepare_replace_moves_a_root_in_o1_and_normalises_an_equal_one() {
    let tracker = Tracker::track(root(&[
        ("a", DocValue::integer(1)),
        ("b", DocValue::integer(2)),
    ]));

    let changed = tracker.prepare_replace(root(&[("a", DocValue::integer(9))]));
    assert_eq!(changed.ops().len(), 1);
    assert!(changed.ops().ops()[0].is_root_replacement());

    // Deeply equal, keys in the other order: no operation, and the current root is kept.
    let equal = tracker.prepare_replace(root(&[
        ("b", DocValue::integer(2)),
        ("a", DocValue::integer(1)),
    ]));
    assert!(equal.is_empty());
    assert!(equal.value().shares_allocation_with(tracker.value()));
}

/// *"Writes restored to their original value and deeply equal container assignments usually normalize
/// to empty."* Our equality is `IndexMap`'s, so the key-order case normalises too — Chord's exact
/// semantics, free from the container (ADR-0030 §4).
#[test]
fn a_deeply_equal_assignment_records_no_operation() {
    let tracker = Tracker::track(root(&[(
        "cfg",
        root(&[("x", DocValue::integer(1)), ("y", DocValue::integer(2))]).into_value(),
    )]));

    let mut change = tracker.begin_change();
    change
        .set(
            &p(&["cfg"]),
            root(&[("y", DocValue::integer(2)), ("x", DocValue::integer(1))]).into_value(),
        )
        .expect("sets");
    let prepared = change.prepare().expect("prepares");
    assert!(
        prepared.is_empty(),
        "an assignment equal up to key order should normalise to an empty batch"
    );
}

/// The other half of `spec.md:1357-1358`, and it is the half a port gets wrong: *"A nonempty
/// structural batch whose final value is deeply equal to its base remains a valid durable change and
/// publication."* So emptiness is of the **batch**, and no whole-value comparison may be added.
#[test]
fn a_nonempty_batch_equal_to_its_base_is_still_a_real_change() {
    let tracker = Tracker::track(root(&[(
        "l",
        list(&[DocValue::integer(1), DocValue::integer(2)]),
    )]));

    let mut change = tracker.begin_change();
    // Remove both and put them back: a structural batch whose result equals the base.
    change
        .splice(&p(&["l"]), 0, 2, Vec::new())
        .expect("removes");
    change
        .splice(
            &p(&["l"]),
            0,
            0,
            vec![DocValue::integer(1), DocValue::integer(2)],
        )
        .expect("inserts");
    let prepared = change.prepare().expect("prepares");

    assert!(!prepared.is_empty(), "the batch is nonempty");
    assert_eq!(
        prepared.value(),
        prepared.base(),
        "and the values are equal"
    );
}

/// Read-your-writes inside the change, without the candidate ever being visible as `&mut`.
#[test]
fn a_change_reads_its_own_writes() {
    let tracker = Tracker::track(root(&[("n", DocValue::integer(1))]));
    let mut change = tracker.begin_change();
    change.set(&p(&["n"]), DocValue::integer(2)).expect("sets");
    assert_eq!(change.read_at(&p(&["n"])), Some(&DocValue::integer(2)));
    assert_eq!(tracker.value().get("n"), Some(&DocValue::integer(1)));
}

/// A rejected operation leaves neither the candidate nor the batch changed — upstream's *"throws at
/// the offending assignment, before the draft changes"*, with nothing to roll back.
#[test]
fn a_rejected_operation_leaves_the_candidate_and_the_batch_alone() {
    let tracker = Tracker::track(root(&[("n", DocValue::integer(1))]));
    let mut change = tracker.begin_change();
    let before = change.read().clone();
    let _err = change
        .append_str(&p(&["n"]), "text")
        .expect_err("a string operation on a number");
    assert_eq!(change.read(), &before);
    assert!(change.prepare().expect("prepares").is_empty());
}
