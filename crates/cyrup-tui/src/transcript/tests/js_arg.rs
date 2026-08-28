#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
//! The three JS operators pi uses on header arguments, and the ways they disagree.
//!
//! Pi does not apply one coercion to these arguments — it applies a DIFFERENT operator at each
//! site, and the three part company precisely on `null`. Reading them all through
//! `Value::as_f64` collapsed the difference into "is it a number", so every non-number silently
//! rendered nothing.
//!
//! Scope note: these pin the pure functions that encode the rules, plus `read_line_range`, which is
//! the one site that is itself a pure function. The other three sites (`grep`/`find`/`ls` `limit`
//! headers and bash's `timeout`) apply their operator in one line inside a renderer that has no
//! test harness in this crate, so their WIRING is not covered here — only the operators they call.

use crate::transcript::*;
use serde_json::json;

/// THE POINT OF THE ITEM. One value, three sites, three different answers.
///
/// - `limit` (grep/find/ls, `grep.ts:89`) — `!== undefined`, so a present `null` renders `"null"`.
/// - `timeout` (bash, `bash.ts:241`) — truthiness, so `null` drops the suffix entirely.
/// - `offset` (read, `read.ts:75`) — `??`, so `null` becomes `1`.
///
/// A single shared rule cannot satisfy all three, which is why they are ported separately.
#[test]
fn the_three_operators_disagree_on_null() {
    // `!== undefined` — present, so it renders, as the string "null".
    assert_eq!(js_arg(&json!(null)), "null");
    // truthiness — falsy, so the whole suffix is dropped.
    assert!(!js_truthy(&json!(null)));
    // `?? 1` — nullish, so it becomes 1 and the range still renders.
    assert_eq!(read_line_range(&json!({"offset": null})).unwrap(), ":1");
}

#[test]
fn js_arg_is_ecmascript_to_string() {
    assert_eq!(js_arg(&json!(null)), "null");
    assert_eq!(js_arg(&json!(true)), "true");
    assert_eq!(js_arg(&json!(false)), "false");
    assert_eq!(js_arg(&json!(50)), "50");
    // Whole floats lose the `.0`, via `js_number` — `String(50.0) === "50"`.
    assert_eq!(js_arg(&json!(50.0)), "50");
    assert_eq!(js_arg(&json!(1.5)), "1.5");
    assert_eq!(js_arg(&json!("50")), "50");
    // `String([1,2]) === "1,2"`, and a `null` ELEMENT renders empty rather than "null".
    assert_eq!(js_arg(&json!([1, 2])), "1,2");
    assert_eq!(js_arg(&json!([1, null, 2])), "1,,2");
    assert_eq!(js_arg(&json!([])), "");
    assert_eq!(js_arg(&json!({"a": 1})), "[object Object]");
}

#[test]
fn js_truthy_is_ecmascript_to_boolean() {
    for falsy in [json!(null), json!(false), json!(0), json!(-0.0), json!("")] {
        assert!(!js_truthy(&falsy), "{falsy} must be falsy");
    }
    // `"0"` is a NON-EMPTY string and therefore truthy — the case a numeric test gets wrong.
    for truthy in [json!(true), json!(1), json!(-1), json!("0"), json!("x"), json!([]), json!({})] {
        assert!(js_truthy(&truthy), "{truthy} must be truthy");
    }
}

/// `read.ts:74-77` is four rules in four lines. Each is pinned here.
#[test]
fn read_line_range_ports_all_four_rules() {
    // Rule 1 — presence gate: neither key present, no range at all.
    assert_eq!(read_line_range(&json!({})), None);
    assert_eq!(read_line_range(&json!({"path": "x"})), None);

    // Rule 1 again — an explicit `null` IS present, so the gate passes.
    assert_eq!(read_line_range(&json!({"offset": null})).unwrap(), ":1");

    // Rule 2 — `?? 1` yields the VALUE, not a number, so a string offset survives verbatim.
    assert_eq!(read_line_range(&json!({"offset": 5})).unwrap(), ":5");
    assert_eq!(read_line_range(&json!({"offset": "5"})).unwrap(), ":5");
    assert_eq!(read_line_range(&json!({"offset": true})).unwrap(), ":true");

    // Rule 3 — `limit !== undefined`, so a `null` limit still computes: `1 + null - 1` is `0`.
    // Rule 4 then drops the `-<end>` half because `0` is falsy.
    assert_eq!(read_line_range(&json!({"limit": null})).unwrap(), ":1");
    assert_eq!(read_line_range(&json!({"offset": 1, "limit": 0})).unwrap(), ":1");

    // The ordinary case.
    assert_eq!(read_line_range(&json!({"offset": 1, "limit": 3})).unwrap(), ":1-3");
    assert_eq!(read_line_range(&json!({"offset": 10, "limit": 5})).unwrap(), ":10-14");

    // JS `+` is string CONCATENATION when either side is a string, and the trailing `- 1` then
    // coerces back to a number: `1 + "5"` is `"15"`, and `"15" - 1` is `14`. Absurd, and exactly
    // what pi renders.
    assert_eq!(read_line_range(&json!({"offset": 1, "limit": "5"})).unwrap(), ":1-14");
}
