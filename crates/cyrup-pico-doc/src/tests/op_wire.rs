//! The seven tuples' wire form, both directions.
//!
//! ADR-0030 §10's serde table is why `Deserialize` is hand-written here: every operation constructor
//! establishes an invariant — a safe path, a nonzero trim, a real permutation — and a derived impl
//! would rebuild all three from a damaged file. These cases are the proof the constructors are on the
//! decode path, not just the construction path.

use crate::tests::p;
use crate::{DocValue, Op, OpBatch, Path, Permutation, Seg, TrimLen};

/// Each tuple, round-tripped, with its exact bytes pinned — because the bytes are the contract a
/// replica stream agrees on.
#[test]
fn every_tuple_round_trips_in_chord_s_exact_shape() {
    let cases: Vec<(Op, &str)> = vec![
        (
            Op::ReplaceRoot(crate::tests::root(&[("a", DocValue::integer(1))])),
            r#"["r",{"a":1}]"#,
        ),
        (
            Op::Set {
                path: p(&["a", "b"]),
                value: DocValue::string("v"),
            },
            r#"["s",["a","b"],"v"]"#,
        ),
        (Op::Delete { path: p(&["a"]) }, r#"["d",["a"]]"#),
        (
            Op::AppendStr {
                path: p(&["a"]),
                text: "txt".into(),
            },
            r#"["a",["a"],"txt"]"#,
        ),
        (
            Op::TrimStrFront {
                path: p(&["a"]),
                count: TrimLen::parse(3).expect("nonzero"),
            },
            r#"["t",["a"],3]"#,
        ),
        (
            Op::Splice {
                path: Path::new([Seg::key("l").expect("safe"), Seg::index(1)]).expect("safe"),
                at: 0,
                remove: 2,
                items: vec![DocValue::Null].into(),
            },
            r#"["p",["l",1],0,2,[null]]"#,
        ),
        (
            Op::Permute {
                path: p(&["l"]),
                perm: Permutation::parse(vec![1, 0]).expect("a permutation"),
            },
            r#"["m",["l"],[1,0]]"#,
        ),
    ];

    for (op, expected) in cases {
        let json = serde_json::to_string(&op).expect("serializes");
        assert_eq!(json, expected, "wire form of {:?}", op.tag());
        assert_eq!(
            serde_json::from_str::<Op>(&json).expect("round trips"),
            op,
            "round trip of {:?}",
            op.tag()
        );
    }
}

/// A decoded batch is a batch, and an empty one decodes to the empty batch rather than failing.
#[test]
fn an_op_batch_round_trips_and_the_empty_one_is_legal() {
    let batch = OpBatch::new(vec![Op::Delete { path: p(&["a"]) }]);
    let json = serde_json::to_string(&batch).expect("serializes");
    assert_eq!(json, r#"[["d",["a"]]]"#);
    assert_eq!(
        serde_json::from_str::<OpBatch>(&json).expect("round trips"),
        batch
    );

    assert!(
        serde_json::from_str::<OpBatch>("[]")
            .expect("the empty batch decodes")
            .is_empty()
    );
}

/// The invariants a derived `Deserialize` would have rebuilt out of a damaged file.
#[test]
fn the_decode_path_goes_through_the_validating_constructors() {
    let cases = [
        (
            r#"["t",["a"],0]"#,
            "a zero trim is a no-op, not an operation",
        ),
        (
            r#"["m",["l"],[0,0]]"#,
            "a repeated source index is not a permutation",
        ),
        (
            r#"["m",["l"],[0,2]]"#,
            "an out-of-range source index is not a permutation",
        ),
        (r#"["s",["__proto__"],1]"#, "a reserved path segment"),
        (
            r#"["q",["a"],1]"#,
            "an unknown tag; Chord has exactly seven",
        ),
        (r#"["s",["a"]]"#, "a set with no value"),
        (r#"["d",["a"],1]"#, "a delete with a trailing element"),
        (
            r#"["r","not an object"]"#,
            "a root replacement whose payload is not an object",
        ),
        (r#"["p",["l"],0,1]"#, "a splice with no items array"),
        (r#"[]"#, "an empty tuple has no tag"),
    ];
    for (json, why) in cases {
        assert!(
            serde_json::from_str::<Op>(json).is_err(),
            "{why}: {json} should not decode"
        );
    }
}

/// A numeric segment decodes as an index and a string segment as a key, so `["l", 1]` and
/// `["l", "1"]` are different paths — which they must be, because one addresses an array element and
/// the other an object member.
#[test]
fn a_numeric_segment_and_its_string_spelling_decode_differently() {
    let indexed: Op = serde_json::from_str(r#"["d",["l",1]]"#).expect("decodes");
    let keyed: Op = serde_json::from_str(r#"["d",["l","1"]]"#).expect("decodes");
    assert_ne!(indexed, keyed);
    match (indexed, keyed) {
        (Op::Delete { path: a }, Op::Delete { path: b }) => {
            assert_eq!(a.segments()[1], Seg::Index(1));
            assert_eq!(b.segments()[1], Seg::Key("1".into()));
        }
        _ => panic!("both decode as deletes"),
    }
}
