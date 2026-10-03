//! `Lifetime`: the constructor's two cases and the `<=`/`<` asymmetry at both boundaries.

use super::seq;
use crate::{InvertedLifetime, Lifetime};

// ------------------------------------------------------- the constructor's two cases ------------

/// `spec.md:1114`: *"A creation retired in the same commit has an empty lifetime."* So this is a
/// reachable state and must be accepted — rejecting it would make a create-and-retire in one
/// commit unrepresentable, which is a legal batch.
#[test]
fn the_empty_lifetime_is_legal() {
    let lt = Lifetime::new(seq(5), Some(seq(5))).expect("retired in its creating commit");
    assert!(lt.is_empty());
    assert!(!lt.is_open());
}

/// No commit can produce this, so a value carrying it came from a damaged record or from code that
/// swapped two arguments. It is rejected rather than normalised, because an inverted interval makes
/// `contains` answer `false` at every sequence: a live incarnation silently disappears from every
/// historical read, with no error and no log line.
#[test]
fn an_inverted_lifetime_is_rejected() {
    assert_eq!(
        Lifetime::new(seq(5), Some(seq(4))),
        Err(InvertedLifetime {
            created_at: seq(5),
            retired_at: seq(4),
        })
    );
}

#[test]
fn an_unretired_lifetime_has_no_upper_bound() {
    let lt = Lifetime::open(seq(5));
    assert!(lt.is_open());
    assert!(!lt.is_empty());
    assert_eq!(Lifetime::new(seq(5), None), Ok(lt));
}

#[test]
fn retiring_accepts_the_creating_sequence_and_rejects_anything_before_it() {
    let lt = Lifetime::open(seq(5));
    assert!(lt.retire(seq(5)).expect("empty is legal").is_empty());
    assert!(lt.retire(seq(6)).is_ok());
    assert!(lt.retire(seq(4)).is_err());
}

// ----------------------------------------------- the <=/< asymmetry at both boundaries ----------

/// The whole point of the type. `created_at <= at < retired_at`: the lower bound is **inclusive**
/// and the upper bound is **exclusive**, so a retire-then-create at one logical address neither
/// overlaps nor gaps at the boundary sequence. Upstream writes this test at each of its two read
/// sites; here it has one implementation and this is the table that pins it.
#[test]
fn contains_is_inclusive_below_and_exclusive_above() {
    struct Case {
        name: &'static str,
        created_at: u64,
        retired_at: Option<u64>,
        at: u64,
        expected: bool,
    }

    let cases = [
        Case {
            name: "one before the lower bound",
            created_at: 5,
            retired_at: Some(8),
            at: 4,
            expected: false,
        },
        Case {
            name: "exactly the lower bound is a member (<=)",
            created_at: 5,
            retired_at: Some(8),
            at: 5,
            expected: true,
        },
        Case {
            name: "inside",
            created_at: 5,
            retired_at: Some(8),
            at: 6,
            expected: true,
        },
        Case {
            name: "one before the upper bound is a member",
            created_at: 5,
            retired_at: Some(8),
            at: 7,
            expected: true,
        },
        Case {
            name: "exactly the upper bound is NOT a member (<)",
            created_at: 5,
            retired_at: Some(8),
            at: 8,
            expected: false,
        },
        Case {
            name: "past the upper bound",
            created_at: 5,
            retired_at: Some(8),
            at: 9,
            expected: false,
        },
        Case {
            name: "unretired: the lower bound is a member",
            created_at: 5,
            retired_at: None,
            at: 5,
            expected: true,
        },
        Case {
            name: "unretired: nothing above is excluded",
            created_at: 5,
            retired_at: None,
            at: u64::MAX,
            expected: true,
        },
        Case {
            name: "unretired: below the lower bound is still not a member",
            created_at: 5,
            retired_at: None,
            at: 4,
            expected: false,
        },
        Case {
            name: "empty: not even its own creating sequence",
            created_at: 5,
            retired_at: Some(5),
            at: 5,
            expected: false,
        },
        Case {
            name: "empty: nor the one before",
            created_at: 5,
            retired_at: Some(5),
            at: 4,
            expected: false,
        },
        Case {
            name: "empty: nor the one after",
            created_at: 5,
            retired_at: Some(5),
            at: 6,
            expected: false,
        },
    ];

    for case in cases {
        let lt = Lifetime::new(seq(case.created_at), case.retired_at.map(seq))
            .expect("every case in this table is a legal interval");
        assert_eq!(
            lt.contains(seq(case.at)),
            case.expected,
            "{}: contains({}) on [{}, {:?})",
            case.name,
            case.at,
            case.created_at,
            case.retired_at
        );
    }
}

/// The boundary read as the pair of incarnations a retire-then-create at one address actually
/// produces: the old one retires at 8, the new one is created at 8, and sequence 8 belongs to
/// exactly one of them.
#[test]
fn retire_then_create_at_one_address_neither_overlaps_nor_gaps_at_the_boundary() {
    let old = Lifetime::new(seq(5), Some(seq(8))).expect("legal");
    let new = Lifetime::open(seq(8));
    for at in 5..=10 {
        let at = seq(at);
        assert!(
            old.contains(at) ^ new.contains(at),
            "sequence {at} must belong to exactly one incarnation"
        );
    }
}

// ------------------------------------------------------------------ the parser -----------------

#[test]
fn a_lifetime_round_trips_as_the_two_sibling_fields_upstream_persists() {
    let retired = Lifetime::new(seq(5), Some(seq(8))).expect("legal");
    let json = serde_json::to_string(&retired).expect("lifetimes serialize");
    assert_eq!(json, "{\"createdAt\":5,\"retiredAt\":8}");
    assert_eq!(
        serde_json::from_str::<Lifetime>(&json).expect("round trip"),
        retired
    );

    let open = Lifetime::open(seq(5));
    let json = serde_json::to_string(&open).expect("lifetimes serialize");
    assert_eq!(
        json, "{\"createdAt\":5}",
        "an unretired incarnation has no upper bound, so the field is absent"
    );
    assert_eq!(
        serde_json::from_str::<Lifetime>(&json).expect("round trip"),
        open
    );
}

/// The one rejection a derived impl over the two private fields would **not** make. It is also the
/// reason `Deserialize` is hand-written here rather than merely preferred: a damaged record must
/// fail the open, not reappear as an incarnation that is a member of nothing.
#[test]
fn the_parser_rejects_an_inverted_interval_from_storage() {
    let err = serde_json::from_str::<Lifetime>("{\"createdAt\":5,\"retiredAt\":4}")
        .expect_err("an inverted interval is corruption");
    assert!(
        err.to_string().contains("inverted"),
        "the error must name what is wrong: {err}"
    );
}

#[test]
fn the_parser_rejects_every_other_damaged_shape() {
    let cases: &[(&str, &str)] = &[
        ("a missing createdAt", "{\"retiredAt\":8}"),
        ("a zero createdAt", "{\"createdAt\":0}"),
        ("a zero retiredAt", "{\"createdAt\":5,\"retiredAt\":0}"),
        ("a string createdAt", "{\"createdAt\":\"5\"}"),
        ("a float createdAt", "{\"createdAt\":5.5}"),
        ("a negative createdAt", "{\"createdAt\":-5}"),
        ("a duplicate createdAt", "{\"createdAt\":5,\"createdAt\":6}"),
        ("a number", "5"),
        ("a string", "\"5\""),
        ("an array", "[5,8]"),
        ("null", "null"),
    ];
    for (shape, json) in cases {
        assert!(
            serde_json::from_str::<Lifetime>(json).is_err(),
            "Lifetime accepted {shape} ({json})"
        );
    }
}

/// `null` is how a JSON writer that always emits both fields says "not retired", and the empty
/// lifetime must still survive the round trip.
#[test]
fn the_parser_reads_a_null_retired_at_as_unretired() {
    assert_eq!(
        serde_json::from_str::<Lifetime>("{\"createdAt\":5,\"retiredAt\":null}").expect("legal"),
        Lifetime::open(seq(5))
    );
    assert_eq!(
        serde_json::from_str::<Lifetime>("{\"createdAt\":5,\"retiredAt\":5}").expect("legal"),
        Lifetime::new(seq(5), Some(seq(5))).expect("legal")
    );
}

/// S3's `DocumentRecord` reaches this impl through `#[serde(flatten)]`, which hands the visitor the
/// whole record's map, so unknown keys must be ignored rather than rejected.
#[test]
fn the_parser_ignores_the_rest_of_the_record_it_is_flattened_into() {
    assert_eq!(
        serde_json::from_str::<Lifetime>(
            "{\"id\":41,\"kind\":\"pi.notes\",\"createdAt\":5,\"scope\":{},\"retiredAt\":8}"
        )
        .expect("legal"),
        Lifetime::new(seq(5), Some(seq(8))).expect("legal")
    );
}
