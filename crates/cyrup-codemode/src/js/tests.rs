#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::cmp::Ordering;

use serde_json::json;

use super::*;

#[test]
fn number_to_string_follows_ecmascript() {
    // Each expectation is what `String(x)` prints in V8.
    let cases: [(f64, &str); 14] = [
        (0.0, "0"),
        (-0.0, "0"),
        (1.0, "1"),
        (-1.5, "-1.5"),
        (0.1, "0.1"),
        (100.0, "100"),
        (1e21, "1e+21"),
        (1e-7, "1e-7"),
        (0.000_001, "0.000001"),
        (123_456_789_012_345_680_000.0, "123456789012345680000"),
        (1.5e300, "1.5e+300"),
        (5e-324, "5e-324"),
        (9_007_199_254_740_992.0, "9007199254740992"),
        (1.797_693_134_862_315_7e308, "1.7976931348623157e+308"),
    ];
    for (value, expected) in cases {
        assert_eq!(number_to_string(value), expected, "{value:e}");
    }
}

#[test]
fn json_stringify_matches_json_stringify() {
    let value: Value = serde_json::from_str(
        r#"{"b":1.0,"2":2e3,"a":[null,true," \u007f😀\n"],"1":{"y":-0,"x":1E21}}"#,
    )
    .unwrap();
    // Integer-like keys first, ascending; numbers as doubles; U+2028 left alone; escapes as JSON.
    assert_eq!(
        json_stringify(&value),
        "{\"1\":{\"y\":0,\"x\":1e+21},\"2\":2000,\"b\":1,\"a\":[null,true,\"\u{2028}\u{7f}\u{1f600}\\n\"]}"
    );
}

#[test]
fn own_keys_puts_array_indices_first() {
    let value: Value = serde_json::from_str(
        r#"{"b":0,"10":0,"a":0,"2":0,"01":0,"4294967294":0,"4294967295":0,"-1":0}"#,
    )
    .unwrap();
    let keys = own_keys(value.as_object().unwrap());
    assert_eq!(
        keys,
        ["2", "10", "4294967294", "b", "a", "01", "4294967295", "-1"]
    );
}

#[test]
fn trim_uses_the_ecmascript_whitespace_set() {
    // U+FEFF is trimmed by JavaScript and not by Rust; U+0085 the other way round.
    assert_eq!(js_trim("\u{feff} x \u{a0}\u{2028}"), "x");
    assert_eq!(js_trim("\u{85}x\u{85}"), "\u{85}x\u{85}");
    assert_eq!(js_trim_start("\u{feff}\t x "), "x ");
    assert!("\u{85}x".trim().len() < "\u{85}x".len());
}

#[test]
fn utf16_len_counts_code_units() {
    assert_eq!(utf16_len("a\u{1f600}é"), 4);
}

#[test]
fn string_order_is_utf16_code_unit_order() {
    // U+1F600 is D83D DE00 in UTF-16, which sorts before U+FF5E; as UTF-8 it sorts after.
    assert_eq!(js_string_cmp("\u{1f600}", "\u{ff5e}"), Ordering::Less);
    assert_eq!("\u{1f600}".cmp("\u{ff5e}"), Ordering::Greater);
}

#[test]
fn split_lines_splits_on_lf_and_crlf_only() {
    assert_eq!(split_lines("a\r\nb\nc"), ["a", "b", "c"]);
    assert_eq!(split_lines("a\rb"), ["a\rb"]);
    assert_eq!(split_lines("a\r\r\nb"), ["a\r", "b"]);
    assert_eq!(split_lines("a\r"), ["a\r"]);
    assert_eq!(split_lines(""), [""]);
}

#[test]
fn decode_uri_component_throws_where_javascript_does() {
    assert_eq!(decode_uri_component("a%20b%C3%A9").as_deref(), Some("a bé"));
    assert_eq!(decode_uri_component("%"), None);
    assert_eq!(decode_uri_component("%4"), None);
    assert_eq!(decode_uri_component("%zz"), None);
    assert_eq!(decode_uri_component("%z4"), None);
    assert_eq!(decode_uri_component("%4z"), None);
    assert_eq!(decode_uri_component("%E0%A4%A"), None);
    assert_eq!(decode_uri_component("%C0%80"), None);
    assert_eq!(decode_uri_component("%ED%A0%80"), None);
}

#[test]
fn math_log_is_v8s_math_log_bit_for_bit() {
    // `testdata/math_log.json`: `Math.log(x)` as Node (V8) computed it, for BM25 idf arguments,
    // magnitudes across the whole range, arguments within 2**-45 of 1, subnormals and the edges.
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../testdata/math_log.json")).unwrap();
    assert!(cases.len() > 400);
    let number = |value: &Value| match value {
        Value::String(text) => match text.as_str() {
            "Infinity" => f64::INFINITY,
            "-Infinity" => f64::NEG_INFINITY,
            _ => f64::NAN,
        },
        other => other.as_f64().unwrap(),
    };
    for case in &cases {
        let (x, want) = (number(&case["x"]), number(&case["log"]));
        let got = math_log(x);
        assert_eq!(
            got.to_bits(),
            want.to_bits(),
            "log({x:e}) = {got:e}, V8 {want:e}"
        );
    }
}

#[test]
fn json_stringify_of_numbers_in_arrays() {
    assert_eq!(json_stringify(&json!([1.0, 0.5, 1e21])), "[1,0.5,1e+21]");
}
