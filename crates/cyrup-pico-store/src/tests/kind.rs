//! [`Kind`] parser tests: one per rejected shape, per `RUST-DESIGN-REVIEW.md`'s parse rule.

use crate::{Kind, MAX_KIND_LEN, NotAKind, RESERVED_PREFIX};

#[test]
fn a_kind_cannot_be_empty() {
    assert_eq!(Kind::parse(""), Err(NotAKind::Empty));
}

#[test]
fn a_kind_cannot_be_longer_than_the_limit() {
    let long = "k".repeat(MAX_KIND_LEN + 1);
    assert_eq!(
        Kind::parse(&long),
        Err(NotAKind::TooLong {
            found: MAX_KIND_LEN + 1,
            max: MAX_KIND_LEN
        })
    );
    assert!(Kind::parse(&"k".repeat(MAX_KIND_LEN)).is_ok());
}

/// The one that matters: `main.jsonl` is one record per line, so a kind containing a newline could
/// forge a record boundary.
#[test]
fn a_kind_cannot_contain_a_newline() {
    assert_eq!(
        Kind::parse("cyrup.\nmessage"),
        Err(NotAKind::ControlCharacter { code: 0x0A })
    );
}

#[test]
fn a_kind_cannot_contain_any_other_control_character() {
    for (name, s) in [("nul", "a\0b"), ("tab", "a\tb"), ("escape", "a\x1bb")] {
        assert!(
            matches!(Kind::parse(s), Err(NotAKind::ControlCharacter { .. })),
            "{name} was accepted"
        );
    }
}

#[test]
fn the_reserved_prefix_is_reported_but_not_rejected() {
    // Rejection belongs to definition registration (PICO5-PLAN S5), which is the only place that knows
    // whether the registering party is a third party. The storage layer must be able to write the
    // kernel's own kinds through this same type.
    let reserved = Kind::parse("cyrup.generation").expect("a reserved kind still parses");
    assert!(reserved.is_reserved());
    assert!(reserved.as_str().starts_with(RESERVED_PREFIX));
    let ordinary =
        Kind::parse("pi.generation").expect("an upstream-prefixed kind is ordinary here");
    assert!(!ordinary.is_reserved());
}

#[test]
fn deserialize_goes_through_the_parser() {
    assert!(serde_json::from_str::<Kind>("\"cyrup.message\"").is_ok());
    for (shape, json) in [
        ("empty", "\"\""),
        ("newline", "\"a\\nb\""),
        ("number", "7"),
        ("null", "null"),
        ("array", "[\"a\"]"),
        ("object", "{\"kind\":\"a\"}"),
        ("boolean", "true"),
    ] {
        assert!(
            serde_json::from_str::<Kind>(json).is_err(),
            "a {shape} kind was accepted by Deserialize"
        );
    }
}

#[test]
fn a_kind_round_trips_as_a_bare_string() {
    let kind = Kind::parse("cyrup.message").expect("parses");
    let json = serde_json::to_string(&kind).expect("serializes");
    assert_eq!(json, "\"cyrup.message\"");
    assert_eq!(
        serde_json::from_str::<Kind>(&json).expect("re-parses"),
        kind
    );
}
