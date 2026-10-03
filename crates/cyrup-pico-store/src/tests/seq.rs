//! `Seq` and `DocumentPoint`.

use core::num::NonZeroU64;

use super::{REJECTED_SHAPES, seq};
use crate::{DocumentPoint, Seq};

#[test]
fn seq_rejects_every_shape_that_is_not_an_unsigned_integer() {
    for (shape, json) in REJECTED_SHAPES {
        assert!(
            serde_json::from_str::<Seq>(json).is_err(),
            "Seq accepted a {shape} ({json})"
        );
    }
}

#[test]
fn seq_round_trips_through_json_as_a_bare_number() {
    let s = seq(7);
    assert_eq!(serde_json::to_string(&s).expect("seqs serialize"), "7");
    assert_eq!(serde_json::from_str::<Seq>("7").expect("round trip"), s);
}

#[test]
fn the_first_sequence_is_one() {
    assert_eq!(Seq::FIRST, seq(1));
}

#[test]
fn successor_increases_and_stops_rather_than_wrapping() {
    assert_eq!(seq(1).successor(), Some(seq(2)));
    let last = Seq::new(NonZeroU64::new(u64::MAX).expect("nonzero"));
    assert_eq!(
        last.successor(),
        None,
        "wrapping would make the store's sequence silently non-increasing"
    );
}

#[test]
fn sequences_order_so_a_half_open_interval_can_be_tested() {
    assert!(seq(1) < seq(2));
    assert!(seq(2) <= seq(2));
}

// --------------------------------------------------------------- DocumentPoint ------------------

#[test]
fn a_document_point_accepts_a_sequence_or_exactly_the_token_current() {
    assert_eq!(
        serde_json::from_str::<DocumentPoint>("7").expect("a number is a point"),
        DocumentPoint::At(seq(7))
    );
    assert_eq!(
        serde_json::from_str::<DocumentPoint>("\"current\"").expect("the token is a point"),
        DocumentPoint::Current
    );
}

/// The derived untagged impl would accept any string as `Current` by falling through; only the
/// exact token is that variant.
#[test]
fn a_document_point_rejects_any_other_string() {
    for json in ["\"\"", "\"Current\"", "\"CURRENT\"", "\"latest\"", "\"7\""] {
        assert!(
            serde_json::from_str::<DocumentPoint>(json).is_err(),
            "DocumentPoint accepted {json} as a read point"
        );
    }
}

#[test]
fn a_document_point_rejects_every_other_shape() {
    for (shape, json) in REJECTED_SHAPES {
        if *shape == "string" {
            // Covered with the full set of rejected strings above.
            continue;
        }
        assert!(
            serde_json::from_str::<DocumentPoint>(json).is_err(),
            "DocumentPoint accepted a {shape} ({json})"
        );
    }
}

#[test]
fn a_document_point_round_trips_in_upstreams_wire_form() {
    assert_eq!(
        serde_json::to_string(&DocumentPoint::At(seq(7))).expect("points serialize"),
        "7"
    );
    assert_eq!(
        serde_json::to_string(&DocumentPoint::Current).expect("points serialize"),
        "\"current\"",
        "spec.md:4224 is `Seq | \"current\"`"
    );
    assert_eq!(DocumentPoint::Current.to_string(), "current");
    assert_eq!(DocumentPoint::At(seq(7)).to_string(), "7");
}
