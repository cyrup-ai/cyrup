//! `StoreId`.

use core::num::NonZeroU128;

use crate::StoreId;

fn store(raw: u128) -> StoreId {
    StoreId::new(NonZeroU128::new(raw).expect("test store ids are nonzero"))
}

#[test]
fn a_store_id_round_trips_as_fixed_width_lowercase_hex() {
    let id = store(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef);
    assert_eq!(
        serde_json::to_string(&id).expect("store ids serialize"),
        "\"0123456789abcdef0123456789abcdef\""
    );
    assert_eq!(
        serde_json::from_str::<StoreId>("\"0123456789abcdef0123456789abcdef\"")
            .expect("round trip"),
        id
    );
}

#[test]
fn a_store_id_is_padded_so_the_wire_form_is_always_the_same_width() {
    assert_eq!(store(1).to_string(), "00000000000000000000000000000001");
}

#[test]
fn a_store_id_accepts_uppercase_hex_on_the_way_in() {
    assert_eq!(
        serde_json::from_str::<StoreId>("\"0123456789ABCDEF0123456789ABCDEF\"").expect("uppercase"),
        store(0x0123_4567_89ab_cdef_0123_4567_89ab_cdef)
    );
}

#[test]
fn a_store_id_rejects_the_shapes_a_damaged_identity_record_can_hold() {
    let cases: &[(&str, &str)] = &[
        ("a number", "1"),
        ("a float", "1.5"),
        ("a negative", "-1"),
        ("a boolean", "true"),
        ("null", "null"),
        ("an array", "[\"00000000000000000000000000000001\"]"),
        ("an object", "{\"store\":\"1\"}"),
        ("a short string", "\"0123\""),
        ("a long string", "\"0123456789abcdef0123456789abcdef0\""),
        ("a non-hex string", "\"0123456789abcdef0123456789abcdeg\""),
        ("all zeroes", "\"00000000000000000000000000000000\""),
    ];
    for (shape, json) in cases {
        assert!(
            serde_json::from_str::<StoreId>(json).is_err(),
            "StoreId accepted {shape} ({json})"
        );
    }
}

/// The cross-store cursor check ADR-0030 F6 §B costs one comparison.
#[test]
fn two_store_ids_compare_by_value() {
    assert_eq!(store(7), store(7));
    assert_ne!(store(7), store(8));
    assert_eq!(format!("{:?}", store(7)), format!("StoreId({})", store(7)));
}
