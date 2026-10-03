//! `DocValue::parse` — the one runtime check ADR-0030 F4 leaves standing.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::tests::root;
use crate::{DocRoot, DocValue, MAX_DEPTH, NotStrictJson};

/// **The regression this type exists to prevent** (ADR-0030 F4, §7).
///
/// The second half of this test is the load-bearing half: it pins what `serde_json` actually does,
/// read from the dependency rather than recalled, so that if anyone reintroduces `to_value` on the
/// document path the two assertions sit side by side and say exactly what was lost.
#[test]
fn nan_is_rejected_rather_than_becoming_null() {
    let err = DocValue::parse(&f64::NAN).expect_err("NaN is not strict JSON");
    assert!(matches!(err, NotStrictJson::NonFinite(n) if n.is_nan()));

    // What a port using `serde_json::to_value` would have done instead: lose the number, raise
    // nothing, and persist `null` for all time.
    let via_serde_json = serde_json::to_value(f64::NAN).expect("to_value does not error on NaN");
    assert_eq!(
        via_serde_json,
        serde_json::Value::Null,
        "if this ever fails, serde_json changed and ADR-0030 F4's citation needs re-reading"
    );
}

#[test]
fn positive_and_negative_infinity_are_rejected_too() {
    for v in [f64::INFINITY, f64::NEG_INFINITY] {
        let err = DocValue::parse(&v).expect_err("an infinity is not strict JSON");
        assert!(matches!(err, NotStrictJson::NonFinite(_)), "{err}");
    }
}

#[test]
fn an_f32_nan_is_rejected_on_its_own_path() {
    let err = DocValue::parse(&f32::NAN).expect_err("NaN is not strict JSON");
    assert!(matches!(err, NotStrictJson::NonFinite(_)), "{err}");
}

/// A `NaN` nested inside a struct is rejected, not dropped: the whole parse fails, so the draft is
/// never touched. Upstream's equivalent is *"throws at the offending assignment, before the draft
/// changes"* (`spec.md:1362`).
#[test]
fn a_nan_nested_in_a_struct_fails_the_whole_parse() {
    #[derive(Serialize)]
    struct Cost {
        label: &'static str,
        average: f64,
    }

    let err = DocValue::parse(&Cost {
        label: "tokens",
        average: f64::NAN,
    })
    .expect_err("a nested NaN is still not strict JSON");
    assert!(matches!(err, NotStrictJson::NonFinite(_)), "{err}");
}

/// A host `Serialize` over a cyclic `Rc` graph recurses without bound, and `#![forbid(unsafe_code)]`
/// does not protect against a stack overflow (ADR-0030 F4, *"guarantee not gained"* item 3).
#[test]
fn nesting_past_the_depth_limit_is_rejected() {
    /// Serializes as `[[[..]]]`, `depth` deep.
    struct Deep(u32);

    impl Serialize for Deep {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            if self.0 == 0 {
                return s.serialize_i64(0);
            }
            s.collect_seq([Deep(self.0 - 1)].iter())
        }
    }

    let err = DocValue::parse(&Deep(MAX_DEPTH + 1)).expect_err("too deep");
    assert!(matches!(err, NotStrictJson::TooDeep), "{err}");
}

#[test]
fn nesting_at_the_depth_limit_is_accepted() {
    struct Deep(u32);

    impl Serialize for Deep {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            if self.0 == 0 {
                return s.serialize_i64(0);
            }
            s.collect_seq([Deep(self.0 - 1)].iter())
        }
    }

    DocValue::parse(&Deep(MAX_DEPTH)).expect("exactly at the limit is fine");
}

/// A decoded value carries the same limit, because *"a damaged file can nest arbitrarily — that is
/// corruption, not absence"* (ADR-0030 §10's serde table).
#[test]
fn the_decode_path_carries_the_same_depth_limit() {
    let opens = "[".repeat((MAX_DEPTH + 2) as usize);
    let closes = "]".repeat((MAX_DEPTH + 2) as usize);
    let json = format!("{opens}{closes}");
    assert!(serde_json::from_str::<DocValue>(&json).is_err());
}

#[test]
fn an_object_key_that_is_not_a_string_is_rejected() {
    let mut numeric_keys = BTreeMap::new();
    numeric_keys.insert(7_u32, "seven");
    let err = DocValue::parse(&numeric_keys).expect_err("JSON object keys are strings");
    assert!(
        matches!(err, NotStrictJson::KeyIsNotAString { found: "number" }),
        "{err}"
    );
}

#[test]
fn an_i128_past_the_json_range_is_rejected_rather_than_truncated() {
    let err = DocValue::parse(&(i128::from(i64::MAX) + 1)).expect_err("out of range");
    assert!(matches!(err, NotStrictJson::IntegerOutOfRange(_)), "{err}");
}

#[test]
fn a_root_that_is_not_an_object_is_rejected_by_name() {
    let err = DocRoot::parse(&"not an object").expect_err("a root is an object");
    assert!(
        matches!(err, NotStrictJson::RootIsNotAnObject { found: "string" }),
        "{err}"
    );
}

#[test]
fn ordinary_strict_json_parses_to_the_shape_it_names() {
    #[derive(Serialize)]
    struct State {
        output: String,
        entries: Vec<u32>,
        done: bool,
        nothing: Option<u8>,
    }

    let parsed = DocRoot::parse(&State {
        output: "done\n".to_owned(),
        entries: vec![1, 2],
        done: true,
        nothing: None,
    })
    .expect("strict JSON");

    assert_eq!(parsed.get("output"), Some(&DocValue::string("done\n")));
    assert_eq!(
        parsed.get("entries").and_then(DocValue::as_list),
        Some(&[DocValue::integer(1), DocValue::integer(2)][..])
    );
    assert_eq!(parsed.get("done"), Some(&DocValue::Bool(true)));
    assert_eq!(parsed.get("nothing"), Some(&DocValue::Null));
}

/// `IndexMap`'s `PartialEq` is order-insensitive, which is Chord's *"equality ignores key order"*.
/// `serde_json/preserve_order` keeps insertion order in the bytes. Both halves matter, and they are
/// different halves — so both are asserted here.
#[test]
fn equality_ignores_key_order_while_the_bytes_preserve_it() {
    let a = root(&[("x", DocValue::integer(1)), ("y", DocValue::integer(2))]);
    let b = root(&[("y", DocValue::integer(2)), ("x", DocValue::integer(1))]);

    assert_eq!(a, b, "equality ignores key order");
    assert_eq!(serde_json::to_string(&a).unwrap(), r#"{"x":1,"y":2}"#);
    assert_eq!(serde_json::to_string(&b).unwrap(), r#"{"y":2,"x":1}"#);
}

#[test]
fn a_document_value_round_trips_through_json() {
    let value = root(&[
        ("s", DocValue::string("text")),
        ("n", DocValue::number(1.5).expect("finite")),
        (
            "l",
            crate::tests::list(&[DocValue::Null, DocValue::Bool(false)]),
        ),
        ("m", root(&[("inner", DocValue::integer(-3))]).into_value()),
    ]);
    let json = serde_json::to_string(&value).expect("serializes");
    let back: DocRoot = serde_json::from_str(&json).expect("round trips");
    assert_eq!(value, back);
}

/// JSON has no `NaN` literal, so the decode direction cannot forge what `parse` rejects — the
/// asymmetry ADR-0030 F4 turns on, asserted rather than assumed.
#[test]
fn json_text_cannot_carry_a_non_finite_number() {
    assert!(serde_json::from_str::<DocValue>("NaN").is_err());
    assert!(serde_json::from_str::<DocValue>("Infinity").is_err());
}
