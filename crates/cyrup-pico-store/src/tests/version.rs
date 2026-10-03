//! `DefVersion`.

use core::num::NonZeroU32;

use super::REJECTED_SHAPES;
use crate::DefVersion;

#[test]
fn def_version_rejects_every_shape_that_is_not_an_unsigned_integer() {
    for (shape, json) in REJECTED_SHAPES {
        assert!(
            serde_json::from_str::<DefVersion>(json).is_err(),
            "DefVersion accepted a {shape} ({json})"
        );
    }
}

/// A damaged file can hold a number past `u32::MAX`. Narrowing it silently would make two
/// different stored versions compare equal, and comparing versions is the whole of migration
/// (`spec.md:1441-1464`).
#[test]
fn def_version_rejects_a_value_past_u32_max_rather_than_narrowing_it() {
    let json = (u64::from(u32::MAX) + 1).to_string();
    assert!(serde_json::from_str::<DefVersion>(&json).is_err());
    assert!(serde_json::from_str::<DefVersion>(&u32::MAX.to_string()).is_ok());
}

#[test]
fn def_version_round_trips_through_json_as_a_bare_number() {
    let v = DefVersion::new(NonZeroU32::new(3).expect("nonzero"));
    assert_eq!(serde_json::to_string(&v).expect("versions serialize"), "3");
    assert_eq!(
        serde_json::from_str::<DefVersion>("3").expect("round trip"),
        v
    );
    assert_eq!(v.to_string(), "3");
}

#[test]
fn the_first_version_is_one_and_versions_order() {
    assert_eq!(
        DefVersion::FIRST,
        DefVersion::new(NonZeroU32::new(1).expect("nonzero"))
    );
    assert!(DefVersion::FIRST < DefVersion::new(NonZeroU32::new(2).expect("nonzero")));
}
