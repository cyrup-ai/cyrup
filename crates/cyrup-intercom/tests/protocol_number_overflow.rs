//! `1e400` must not kill a frame — JS parses it to `Infinity` and pi relays it as `null`.
//!
//! pi reads every frame with `JSON.parse` (`v0.9.2 broker/framing.ts:36-42`). ECMA-262 rounds an
//! overflowing literal to `±Infinity`; `typeof Infinity === "number"` is true, so pi's guards pass
//! and the broker relays the message. `JSON.stringify` then emits `null` for the non-finite value,
//! so what a downstream peer actually receives is `null`.
//!
//! `serde_json` refuses the literal outright with `number out of range`, failing the WHOLE frame —
//! including in positions cyrup does not model and pi never type-checks. That made a frame pi
//! serves normally into a disconnect, on a socket every local process can reach.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_intercom::transport::framing::from_frame_slice;
use serde_json::{json, Value};

#[test]
fn an_overflowing_literal_in_an_unmodelled_key_no_longer_kills_the_frame() {
    // THE DEFECT: pre-fix this returned Err("number out of range") and the caller dropped the
    // connection. pi delivers this message with the key nulled.
    let raw = br#"{"type":"send","to":"beta","message":{"id":"m1","timestamp":1,"unknownKey":1e400,"content":{"text":"hi"}}}"#;
    let v: Value = from_frame_slice(raw).expect("an overflowing literal must not fail the frame");
    assert_eq!(v["type"], json!("send"));
    assert_eq!(v["message"]["id"], json!("m1"));
    assert_eq!(
        v["message"]["unknownKey"],
        Value::Null,
        "pi relays a non-finite as `null` (JSON.stringify); cyrup must see the same value"
    );
    // The modelled neighbours must be untouched, and an integer must stay an integer.
    assert_eq!(v["message"]["timestamp"], json!(1));
}

#[test]
fn overflow_is_nulled_at_every_depth_and_sign() {
    let raw = br#"{"a":1e400,"b":[-1e400,{"c":1e309}],"d":{"e":{"f":1e400}}}"#;
    let v: Value = from_frame_slice(raw).expect("parses");
    assert_eq!(v["a"], Value::Null);
    assert_eq!(v["b"][0], Value::Null);
    assert_eq!(v["b"][1]["c"], Value::Null);
    assert_eq!(v["d"]["e"]["f"], Value::Null);
}

#[test]
fn in_range_numbers_are_untouched() {
    // MIRROR: the rewrite must fire ONLY on overflow. `1e300` is finite and must survive exactly,
    // and a large integer must not be turned into a float on the way through.
    let raw = br#"{"big":1e300,"neg":-1e300,"int":1700000000000,"zero":0,"frac":1.5,"exp":1.0e2}"#;
    let v: Value = from_frame_slice(raw).expect("parses");
    assert_eq!(v["big"], json!(1e300));
    assert_eq!(v["neg"], json!(-1e300));
    assert_eq!(v["int"], json!(1_700_000_000_000i64));
    assert!(v["int"].is_i64(), "an integer must relay AS an integer");
    assert_eq!(v["zero"], json!(0));
    assert_eq!(v["frac"], json!(1.5));
}

#[test]
fn a_numeric_looking_substring_inside_a_string_is_never_rewritten() {
    // The scanner must track string boundaries. If it did not, this text would be corrupted — and
    // corrupting message bodies is far worse than the bug being fixed.
    let raw = br#"{"text":"the value 1e400 appears here","other":1e400}"#;
    let v: Value = from_frame_slice(raw).expect("parses");
    assert_eq!(
        v["text"],
        json!("the value 1e400 appears here"),
        "a number inside a string must survive verbatim"
    );
    assert_eq!(v["other"], Value::Null, "but a real numeric literal is nulled");
}

#[test]
fn an_escaped_quote_does_not_end_a_string_early() {
    // If the scanner mishandled `\"`, it would think the string ended and start rewriting inside
    // the remaining text.
    let raw = br#"{"text":"he said \"1e400\" loudly","after":1e400}"#;
    let v: Value = from_frame_slice(raw).expect("parses");
    assert_eq!(v["text"], json!("he said \"1e400\" loudly"));
    assert_eq!(v["after"], Value::Null);
}

#[test]
fn genuinely_malformed_json_is_still_rejected() {
    // MIRROR: leniency is scoped to numeric overflow. Everything else must still fail, or this
    // "fix" becomes an input-validation hole on a socket any local process can reach.
    for bad in [
        &b"{\"a\":}"[..],
        &b"{\"a\" \"b\"}"[..],
        &b"{unquoted:1}"[..],
        &b"[1,2"[..],
        &b"{\"a\":1e400"[..], // overflow AND truncated — the rewrite must not rescue it
        &b"not json at all"[..],
    ] {
        assert!(
            from_frame_slice::<Value>(bad).is_err(),
            "malformed JSON must still be rejected: {}",
            String::from_utf8_lossy(bad)
        );
    }
}
