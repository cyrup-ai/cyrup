//! `choose_representation` — the decision table of `spec.md:1374-1401`, and the
//! "evaluated exactly once, and not at all when it must not be" rules.

use core::cell::Cell;

use crate::tests::{p, root};
use crate::{
    BaseRequirement, CheckpointInput, DocValue, Prepared, Representation, Tracker,
    choose_representation,
};

/// A prepared change with a nonempty batch.
fn nonempty() -> Prepared {
    let tracker = Tracker::track(root(&[("n", DocValue::integer(1))]));
    let mut change = tracker.begin_change();
    change.set(&p(&["n"]), DocValue::integer(2)).expect("sets");
    change.prepare().expect("prepares")
}

/// A prepared change with an empty batch.
fn empty() -> Prepared {
    let tracker = Tracker::track(root(&[("n", DocValue::integer(1))]));
    tracker.begin_change().prepare().expect("prepares")
}

/// The whole table, in one test, because it is a table and not four stories.
#[test]
fn the_representation_table_is_the_spec_s() {
    let calls = Cell::new(0_u32);
    let yes = |_: CheckpointInput<'_>| {
        calls.set(calls.get() + 1);
        true
    };
    let no = |_: CheckpointInput<'_>| {
        calls.set(calls.get() + 1);
        false
    };

    // Required: a base, and the predicate is NOT called.
    calls.set(0);
    assert_eq!(
        choose_representation(BaseRequirement::Required, &nonempty(), 5, Some(&yes)),
        Representation::Base
    );
    assert_eq!(
        calls.get(),
        0,
        "creation and version transitions do not call it"
    );

    // Required with an EMPTY batch: still a base. `spec.md:1360-1361`.
    calls.set(0);
    assert_eq!(
        choose_representation(BaseRequirement::Required, &empty(), 0, Some(&no)),
        Representation::Base
    );
    assert_eq!(calls.get(), 0);

    // Optional + empty: nothing, and the predicate is NOT called.
    calls.set(0);
    assert_eq!(
        choose_representation(BaseRequirement::Optional, &empty(), 5, Some(&yes)),
        Representation::Nothing
    );
    assert_eq!(calls.get(), 0);

    // Optional + nonempty + predicate true: a base, predicate called exactly once.
    calls.set(0);
    assert_eq!(
        choose_representation(BaseRequirement::Optional, &nonempty(), 5, Some(&yes)),
        Representation::Base
    );
    assert_eq!(calls.get(), 1, "evaluated exactly once");

    // Optional + nonempty + predicate false: a delta, predicate called exactly once.
    calls.set(0);
    assert_eq!(
        choose_representation(BaseRequirement::Optional, &nonempty(), 5, Some(&no)),
        Representation::Delta
    );
    assert_eq!(calls.get(), 1);

    // Optional + nonempty + no predicate at all: a delta. `spec.md:1390`'s `?? false`.
    assert_eq!(
        choose_representation(BaseRequirement::Optional, &nonempty(), 5, None),
        Representation::Delta
    );
}

/// The predicate sees exactly `(candidate, ops, deltas_since_base)` — `spec.md:1390`'s three
/// arguments and nothing else. There is no storage handle in the type to read from, which is how
/// *"evaluation performs no Storage read"* stops being a rule and becomes a property.
#[test]
fn the_predicate_sees_the_candidate_the_ops_and_the_count_and_nothing_else() {
    let prepared = nonempty();
    let seen = Cell::new(None);
    let predicate = |input: CheckpointInput<'_>| {
        seen.set(Some((
            input.candidate.get("n").cloned(),
            input.ops.len(),
            input.deltas_since_base,
        )));
        false
    };

    let _ = choose_representation(BaseRequirement::Optional, &prepared, 41, Some(&predicate));

    assert_eq!(
        seen.into_inner(),
        Some((Some(DocValue::integer(2)), 1, 41)),
        "the predicate must see the prepared candidate, not the base"
    );
}

/// A root-replacement batch is still a delta unless the definition selected a checkpoint
/// (`spec.md:1407-1409`). This is the row a port gets wrong by treating `["r", value]` as a base.
#[test]
fn a_root_replacement_is_still_a_delta_without_a_checkpoint() {
    let tracker = Tracker::track(root(&[("a", DocValue::integer(1))]));
    let prepared = tracker.prepare_replace(root(&[("b", DocValue::integer(2))]));
    assert!(prepared.ops().ops()[0].is_root_replacement());
    assert_eq!(
        choose_representation(BaseRequirement::Optional, &prepared, 0, None),
        Representation::Delta
    );
}

/// A predicate keyed on the count behaves monotonically in it — the common definition shape
/// ("checkpoint every N deltas"), and the one that would break if the count included the change
/// being evaluated.
#[test]
fn a_count_keyed_predicate_sees_the_count_excluding_this_change() {
    let prepared = nonempty();
    let every_four = |input: CheckpointInput<'_>| input.deltas_since_base >= 4;
    let at = |n| choose_representation(BaseRequirement::Optional, &prepared, n, Some(&every_four));
    assert_eq!(at(3), Representation::Delta);
    assert_eq!(at(4), Representation::Base);
}
