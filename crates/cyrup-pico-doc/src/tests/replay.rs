//! `ReplayPlan::parse`'s three corruption arms, and `materialize`'s round trips.
//!
//! Table-tested over **literal record sets**, with no storage anywhere — which is the point of
//! ADR-0030 F6 §C: *"replay is directly unit-testable with literal record sets and no storage."*

use crate::tests::{base, delta, list, p, root, ver};
use crate::{Corruption, DocValue, Op, ReplayPlan, materialize};

/// A base alone materializes to itself, with no deltas counted.
#[test]
fn a_base_alone_materializes_to_itself() {
    let value = root(&[("a", DocValue::integer(1))]);
    let plan = ReplayPlan::parse(&[base(1, value.clone())]).expect("a base is replayable");
    assert_eq!(materialize(&plan), value);
    assert_eq!(plan.deltas_since_base(), 0);
    assert_eq!(plan.version().version(), ver(1));
}

/// `spec.md:4348-4349`: *"It selects the newest applicable base, applies its ordered Chord delta
/// tail."* So records before the newest base are not read at all — which is also why a legitimately
/// reclaimed prefix is not corruption.
#[test]
fn the_newest_base_wins_and_earlier_records_are_not_read() {
    let stale = root(&[("from", DocValue::string("stale"))]);
    let fresh = root(&[("from", DocValue::string("fresh"))]);
    let plan = ReplayPlan::parse(&[
        base(1, stale),
        delta(
            1,
            &[Op::Set {
                path: p(&["ignored"]),
                value: DocValue::Null,
            }],
        ),
        base(2, fresh.clone()),
    ])
    .expect("replayable");
    assert_eq!(materialize(&plan), fresh);
    assert_eq!(plan.version().version(), ver(2));
    assert_eq!(plan.deltas_since_base(), 0);
}

/// The tail is replayed in order, and `deltas_since_base` is the count `spec.md:1392-1394` describes:
/// deltas **already stored** after the newest base.
#[test]
fn a_tail_replays_in_order_and_is_counted() {
    let plan = ReplayPlan::parse(&[
        base(3, root(&[("out", DocValue::string(""))])),
        delta(
            3,
            &[Op::AppendStr {
                path: p(&["out"]),
                text: "a".into(),
            }],
        ),
        delta(
            3,
            &[Op::AppendStr {
                path: p(&["out"]),
                text: "b".into(),
            }],
        ),
    ])
    .expect("replayable");
    assert_eq!(materialize(&plan).get("out"), Some(&DocValue::string("ab")));
    assert_eq!(plan.deltas_since_base(), 2);
}

/// The three corruption arms, as a table over literal record sets. Each row is a shape the
/// specification names at `spec.md:4357-4359`.
#[test]
fn the_corruption_arms_are_exactly_the_three_the_spec_names() {
    let good = root(&[("out", DocValue::string("x"))]);

    // 1. A delta tail with no base before it.
    let missing_base = ReplayPlan::parse(&[delta(
        1,
        &[Op::AppendStr {
            path: p(&["out"]),
            text: "y".into(),
        }],
    )])
    .expect_err("a tail without its base is not replayable");
    assert!(
        matches!(missing_base, Corruption::MissingRequiredBase),
        "{missing_base}"
    );

    // 1b. An empty record set, for an incarnation a backend already resolved: still a missing base,
    // because `spec.md:1374` says creation always stores one. Absence is the backend's `Ok(None)`
    // from the RECORD lookup, which happens before this call.
    let empty = ReplayPlan::parse(&[]).expect_err("a known incarnation has a base");
    assert!(matches!(empty, Corruption::MissingRequiredBase), "{empty}");

    // 2. A version change inside a delta tail.
    let crossed = ReplayPlan::parse(&[
        base(1, good.clone()),
        delta(
            1,
            &[Op::AppendStr {
                path: p(&["out"]),
                text: "y".into(),
            }],
        ),
        delta(
            2,
            &[Op::AppendStr {
                path: p(&["out"]),
                text: "z".into(),
            }],
        ),
    ])
    .expect_err("a delta cannot cross a version boundary");
    assert!(
        matches!(
            crossed,
            Corruption::VersionChangeInDeltaTail {
                base: b,
                found: f,
                at_delta: 1
            } if b == ver(1) && f == ver(2)
        ),
        "{crossed}"
    );

    // 3. An operation that cannot be applied.
    let inapplicable = ReplayPlan::parse(&[
        base(1, good),
        delta(
            1,
            &[Op::Delete {
                path: p(&["never-existed"]),
            }],
        ),
    ])
    .expect_err("an inapplicable operation is corruption, not absence");
    assert!(
        matches!(
            inapplicable,
            Corruption::OperationNotApplicable {
                at_delta: 0,
                at_op: 0,
                ..
            }
        ),
        "{inapplicable}"
    );
}

/// The first delta of a tail is the one that is easiest to misreport, so its index is pinned.
#[test]
fn a_version_change_on_the_first_delta_is_reported_at_index_zero() {
    let err = ReplayPlan::parse(&[
        base(1, root(&[("a", DocValue::integer(1))])),
        delta(
            2,
            &[Op::Set {
                path: p(&["a"]),
                value: DocValue::integer(2),
            }],
        ),
    ])
    .expect_err("crosses a version");
    assert!(
        matches!(
            err,
            Corruption::VersionChangeInDeltaTail { at_delta: 0, .. }
        ),
        "{err}"
    );
}

/// `materialize` is total: once a plan exists, there is no failure left to represent. The round trip
/// asserted here is `base + ops -> value`, against `apply` computing the same thing — two independent
/// paths to one answer.
#[test]
fn materialize_round_trips_against_apply() {
    let start = root(&[
        ("out", DocValue::string("")),
        ("items", list(&[DocValue::integer(1)])),
    ]);
    let ops = [
        Op::AppendStr {
            path: p(&["out"]),
            text: "hello".into(),
        },
        Op::Splice {
            path: p(&["items"]),
            at: 1,
            remove: 0,
            items: vec![DocValue::integer(2)].into(),
        },
    ];

    let plan = ReplayPlan::parse(&[base(1, start.clone()), delta(1, &ops)]).expect("replayable");
    let direct = crate::apply(&start, &ops).expect("applies");
    assert_eq!(materialize(&plan), direct);
}

/// A root replacement inside a tail is just a delta, and replay honours it — `spec.md:1407-1409`'s
/// *"a Chord root-replacement operation remains a delta."*
#[test]
fn a_root_replacement_inside_a_tail_replays_as_a_delta() {
    let replacement = root(&[("fresh", DocValue::Bool(true))]);
    let plan = ReplayPlan::parse(&[
        base(1, root(&[("old", DocValue::Bool(false))])),
        delta(1, &[Op::ReplaceRoot(replacement.clone())]),
    ])
    .expect("replayable");
    assert_eq!(materialize(&plan), replacement);
    assert_eq!(plan.deltas_since_base(), 1);
}

/// Records round-trip through JSON in the shape `spec.md:4237-4240` gives for `DocumentContent`.
#[test]
fn stored_content_round_trips_in_the_spec_s_wire_shape() {
    let base_record = base(2, root(&[("a", DocValue::integer(1))]));
    let json = serde_json::to_string(&base_record).expect("serializes");
    assert_eq!(json, r#"{"kind":"base","version":2,"value":{"a":1}}"#);
    assert_eq!(
        serde_json::from_str::<crate::StoredContent>(&json).expect("round trips"),
        base_record
    );

    let delta_record = delta(
        2,
        &[Op::Set {
            path: p(&["a"]),
            value: DocValue::integer(2),
        }],
    );
    let json = serde_json::to_string(&delta_record).expect("serializes");
    assert_eq!(
        json,
        r#"{"kind":"delta","version":2,"ops":[["s",["a"],2]]}"#
    );
    assert_eq!(
        serde_json::from_str::<crate::StoredContent>(&json).expect("round trips"),
        delta_record
    );
}

/// A stored record whose version is 0, a string or a float fails the decode rather than being
/// normalised — the hand-written `Deserialize` ADR-0030 §10 requires, exercised through the record
/// that actually carries it.
#[test]
fn a_record_with_an_invalid_version_fails_the_decode() {
    for bad in ["0", "\"2\"", "2.5", "-2", "4294967296"] {
        let json = format!(r#"{{"kind":"base","version":{bad},"value":{{}}}}"#);
        assert!(
            serde_json::from_str::<crate::StoredContent>(&json).is_err(),
            "a version of {bad} should not decode"
        );
    }
}
