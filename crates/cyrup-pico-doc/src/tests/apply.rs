//! The seven operations, applied. Table-driven where the cases are shapes rather than stories.

use crate::tests::{list, p, root};
use crate::{DocValue, Op, OpBatch, Path, PathError, Permutation, TrimLen, apply, apply_batches};

/// Every operation, once, on one root — so a regression names the tuple that broke.
#[test]
fn the_seven_operations_each_do_what_their_tuple_says() {
    let before = root(&[
        ("out", DocValue::string("ab")),
        ("gone", DocValue::integer(1)),
        ("items", list(&[DocValue::integer(1), DocValue::integer(2)])),
        ("perm", list(&[DocValue::integer(7), DocValue::integer(8)])),
        ("trim", DocValue::string("xyz")),
    ]);

    let after = apply(
        &before,
        &[
            Op::Set {
                path: p(&["set"]),
                value: DocValue::Bool(true),
            },
            Op::Delete { path: p(&["gone"]) },
            Op::AppendStr {
                path: p(&["out"]),
                text: "c".into(),
            },
            Op::TrimStrFront {
                path: p(&["trim"]),
                count: TrimLen::parse(1).expect("nonzero"),
            },
            Op::Splice {
                path: p(&["items"]),
                at: 1,
                remove: 1,
                items: vec![DocValue::integer(9), DocValue::integer(10)].into(),
            },
            Op::Permute {
                path: p(&["perm"]),
                perm: Permutation::parse(vec![1, 0]).expect("a permutation"),
            },
        ],
    )
    .expect("every operation applies");

    assert_eq!(after.get("set"), Some(&DocValue::Bool(true)));
    assert_eq!(after.get("gone"), None);
    assert_eq!(after.get("out"), Some(&DocValue::string("abc")));
    assert_eq!(after.get("trim"), Some(&DocValue::string("yz")));
    assert_eq!(
        after.get("items"),
        Some(&list(&[
            DocValue::integer(1),
            DocValue::integer(9),
            DocValue::integer(10)
        ]))
    );
    assert_eq!(
        after.get("perm"),
        Some(&list(&[DocValue::integer(8), DocValue::integer(7)]))
    );
}

/// `["r", value]` takes a root, so a root replacement whose payload is a string is unrepresentable
/// rather than rejected (`spec.md:4240` types every stored base's value as `JsonObject`).
#[test]
fn a_root_replacement_replaces_the_whole_root() {
    let before = root(&[("a", DocValue::integer(1))]);
    let replacement = root(&[("b", DocValue::integer(2))]);
    let after = apply(&before, &[Op::ReplaceRoot(replacement.clone())]).expect("applies");
    assert_eq!(after, replacement);
    assert_eq!(
        before.get("a"),
        Some(&DocValue::integer(1)),
        "base untouched"
    );
}

#[test]
fn apply_batches_replays_in_order_without_concatenating() {
    let before = root(&[("out", DocValue::string(""))]);
    let batches = [
        OpBatch::new(vec![Op::AppendStr {
            path: p(&["out"]),
            text: "a".into(),
        }]),
        OpBatch::new(vec![Op::AppendStr {
            path: p(&["out"]),
            text: "b".into(),
        }]),
    ];
    let after = apply_batches(&before, batches.iter()).expect("applies");
    assert_eq!(after.get("out"), Some(&DocValue::string("ab")));
}

/// A set one past the end grows the array; two past would leave a hole and is refused
/// (*"Writing past the next index and deleting an element throw."*).
#[test]
fn a_set_may_grow_an_array_by_one_but_not_leave_a_hole() {
    let before = root(&[("l", list(&[DocValue::integer(1)]))]);

    let grown = apply(
        &before,
        &[Op::Set {
            path: Path::new([crate::Seg::key("l").unwrap(), crate::Seg::index(1)])
                .expect("safe path"),
            value: DocValue::integer(2),
        }],
    )
    .expect("one past the end appends");
    assert_eq!(
        grown.get("l"),
        Some(&list(&[DocValue::integer(1), DocValue::integer(2)]))
    );

    let err = apply(
        &before,
        &[Op::Set {
            path: Path::new([crate::Seg::key("l").unwrap(), crate::Seg::index(2)])
                .expect("safe path"),
            value: DocValue::integer(3),
        }],
    )
    .expect_err("two past the end would leave a hole");
    assert!(
        matches!(err, PathError::SetWouldLeaveAHole { index: 2, len: 1 }),
        "{err}"
    );
}

/// Arrays stay dense, so a delete of an element is refused and a splice is the way to remove one.
#[test]
fn deleting_an_array_element_is_refused_because_arrays_stay_dense() {
    let before = root(&[("l", list(&[DocValue::integer(1)]))]);
    let err = apply(
        &before,
        &[Op::Delete {
            path: Path::new([crate::Seg::key("l").unwrap(), crate::Seg::index(0)])
                .expect("safe path"),
        }],
    )
    .expect_err("dense arrays");
    assert!(matches!(err, PathError::DeleteOfArrayElement), "{err}");
}

/// A deletion must not reorder the surviving members: insertion order is the document's order in the
/// bytes, so `shift_remove` is the correct primitive and `swap_remove` is not.
#[test]
fn a_delete_preserves_the_order_of_the_surviving_members() {
    let before = root(&[
        ("a", DocValue::integer(1)),
        ("b", DocValue::integer(2)),
        ("c", DocValue::integer(3)),
    ]);
    let after = apply(&before, &[Op::Delete { path: p(&["a"]) }]).expect("applies");
    assert_eq!(serde_json::to_string(&after).unwrap(), r#"{"b":2,"c":3}"#);
}

/// `["t", path, count]` counts **bytes**, which settles ADR-0030 §14 open question 7 as §14's own
/// provisional answer. The reasoning and the recorded incompatibility are on `apply::trim_str_front`;
/// this is the test that pins the unit, so the decision cannot be reverted silently.
#[test]
fn a_trim_counts_bytes_not_characters_and_not_utf16_code_units() {
    // "é" is 2 bytes in UTF-8 and 1 UTF-16 code unit; "😀" is 4 bytes and 2 UTF-16 code units. So the
    // three candidate units give three different answers here, and only one of them can pass.
    let before = root(&[("s", DocValue::string("é😀tail"))]);

    let two = apply(
        &before,
        &[Op::TrimStrFront {
            path: p(&["s"]),
            count: TrimLen::parse(2).expect("nonzero"),
        }],
    )
    .expect("two bytes is the whole of \u{e9}");
    assert_eq!(two.get("s"), Some(&DocValue::string("😀tail")));

    let six = apply(
        &before,
        &[Op::TrimStrFront {
            path: p(&["s"]),
            count: TrimLen::parse(6).expect("nonzero"),
        }],
    )
    .expect("2 + 4 bytes");
    assert_eq!(six.get("s"), Some(&DocValue::string("tail")));
}

/// A count landing inside a multi-byte character is refused rather than producing a broken string. The
/// count is persisted, so a damaged record can name any offset — this is a real arm, not hygiene.
#[test]
fn a_trim_that_lands_inside_a_character_is_refused() {
    let before = root(&[("s", DocValue::string("é😀tail"))]);
    for count in [1_u32, 3, 4, 5] {
        let err = apply(
            &before,
            &[Op::TrimStrFront {
                path: p(&["s"]),
                count: TrimLen::parse(count).expect("nonzero"),
            }],
        )
        .expect_err("not a character boundary");
        assert!(
            matches!(err, PathError::TrimNotACharBoundary { count: c } if c == count),
            "{count}: {err}"
        );
    }
}

#[test]
fn a_trim_past_the_end_is_refused() {
    let before = root(&[("s", DocValue::string("ab"))]);
    let err = apply(
        &before,
        &[Op::TrimStrFront {
            path: p(&["s"]),
            count: TrimLen::parse(3).expect("nonzero"),
        }],
    )
    .expect_err("past the end");
    assert!(
        matches!(err, PathError::TrimPastEnd { count: 3, len: 2 }),
        "{err}"
    );
}

#[test]
fn a_trim_of_the_whole_string_leaves_it_empty() {
    let before = root(&[("s", DocValue::string("ab"))]);
    let after = apply(
        &before,
        &[Op::TrimStrFront {
            path: p(&["s"]),
            count: TrimLen::parse(2).expect("nonzero"),
        }],
    )
    .expect("exactly the whole string");
    assert_eq!(after.get("s"), Some(&DocValue::string("")));
}

/// A zero trim is a no-op, not an operation: a batch that is "nonempty" only because it holds no-ops
/// would defeat `spec.md:1357`'s empty-batch rule, so the length is nonzero by construction.
#[test]
fn a_zero_trim_is_unrepresentable() {
    assert!(TrimLen::parse(0).is_err());
    assert!(TrimLen::parse(1).is_ok());
}

/// A permutation is validated at construction, so "two positions claim one source" and "a position
/// claims a source that does not exist" never reach the applier.
#[test]
fn a_non_permutation_is_rejected_at_construction() {
    use crate::NotAPermutation;
    assert!(matches!(
        Permutation::parse(vec![0, 0]),
        Err(NotAPermutation::Repeated { index: 0 })
    ));
    assert!(matches!(
        Permutation::parse(vec![0, 2]),
        Err(NotAPermutation::OutOfRange { index: 2, len: 2 })
    ));
    assert!(Permutation::parse(vec![1, 0]).is_ok());
}

#[test]
fn a_permutation_of_the_wrong_length_is_refused_at_apply() {
    let before = root(&[("l", list(&[DocValue::integer(1)]))]);
    let err = apply(
        &before,
        &[Op::Permute {
            path: p(&["l"]),
            perm: Permutation::parse(vec![1, 0]).expect("a permutation"),
        }],
    )
    .expect_err("lengths must match");
    assert!(
        matches!(
            err,
            PathError::PermutationLengthMismatch {
                perm_len: 2,
                len: 1
            }
        ),
        "{err}"
    );
}

#[test]
fn a_splice_out_of_range_is_refused() {
    let before = root(&[("l", list(&[DocValue::integer(1)]))]);
    let start = apply(
        &before,
        &[Op::Splice {
            path: p(&["l"]),
            at: 2,
            remove: 0,
            items: Vec::new().into(),
        }],
    )
    .expect_err("start past the end");
    assert!(
        matches!(start, PathError::SpliceStartOutOfRange { at: 2, len: 1 }),
        "{start}"
    );

    let remove = apply(
        &before,
        &[Op::Splice {
            path: p(&["l"]),
            at: 0,
            remove: 2,
            items: Vec::new().into(),
        }],
    )
    .expect_err("removing more than there is");
    assert!(
        matches!(
            remove,
            PathError::SpliceRemoveOutOfRange {
                at: 0,
                remove: 2,
                len: 1
            }
        ),
        "{remove}"
    );
}

/// Each `PathError` arm names a shape pi's appliers also refuse. Table-driven: the point is the
/// mapping from shape to named arm, not six stories.
#[test]
fn a_path_that_addresses_nothing_names_what_went_wrong() {
    let before = root(&[
        ("obj", root(&[("k", DocValue::integer(1))]).into_value()),
        ("num", DocValue::integer(1)),
        ("l", list(&[DocValue::integer(1)])),
    ]);

    /// A row: what is being tried, the operation, and which named arm must come back.
    type Case = (&'static str, Op, fn(&PathError) -> bool);

    let cases: Vec<Case> = vec![
        (
            "a key step into a number",
            Op::Set {
                path: p(&["num", "k"]),
                value: DocValue::Null,
            },
            |e| matches!(e, PathError::NotAnObject { .. }),
        ),
        (
            "an index step into an object",
            Op::Splice {
                path: Path::new([crate::Seg::key("obj").unwrap(), crate::Seg::index(0)]).unwrap(),
                at: 0,
                remove: 0,
                items: vec![DocValue::Null].into(),
            },
            |e| matches!(e, PathError::NotAnArray { .. }),
        ),
        (
            "a delete of a member that is not there",
            Op::Delete {
                path: p(&["missing"]),
            },
            |e| matches!(e, PathError::NoSuchKey { .. }),
        ),
        (
            "a string operation on a number",
            Op::AppendStr {
                path: p(&["num"]),
                text: "x".into(),
            },
            |e| matches!(e, PathError::NotAString { .. }),
        ),
        (
            "an array operation on an object",
            Op::Permute {
                path: p(&["obj"]),
                perm: Permutation::parse(Vec::new()).unwrap(),
            },
            |e| matches!(e, PathError::OperationNeedsAnArray { .. }),
        ),
        (
            "a set at the root",
            Op::Set {
                path: Path::root(),
                value: DocValue::Null,
            },
            |e| matches!(e, PathError::RootIsNotAddressable),
        ),
        (
            "a step through a missing member",
            Op::Set {
                path: p(&["missing", "k"]),
                value: DocValue::Null,
            },
            |e| matches!(e, PathError::NoSuchKey { .. }),
        ),
        (
            "an index past the end on the way down",
            Op::AppendStr {
                path: Path::new([
                    crate::Seg::key("l").unwrap(),
                    crate::Seg::index(5),
                    crate::Seg::key("x").unwrap(),
                ])
                .unwrap(),
                text: "x".into(),
            },
            |e| matches!(e, PathError::IndexOutOfRange { index: 5, len: 1 }),
        ),
    ];

    for (what, op, is_expected) in cases {
        let err = apply(&before, &[op]).expect_err(what);
        assert!(is_expected(&err), "{what}: got {err}");
    }
}

/// A failed operation leaves the caller's base exactly as it was: `apply` builds a new root and
/// returns `Err` without ever having had a mutable view of the base.
#[test]
fn a_failed_batch_leaves_the_base_untouched() {
    let before = root(&[("a", DocValue::integer(1))]);
    let snapshot = before.clone();
    let _err = apply(
        &before,
        &[
            Op::Set {
                path: p(&["b"]),
                value: DocValue::integer(2),
            },
            Op::Delete {
                path: p(&["missing"]),
            },
        ],
    )
    .expect_err("the second operation fails");
    assert_eq!(before, snapshot);
    assert_eq!(before.get("b"), None);
}
