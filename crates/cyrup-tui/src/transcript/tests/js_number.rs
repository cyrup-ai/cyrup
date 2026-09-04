#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]
//! `js_number` is `String(n)` for a `Number` out of `JSON.parse` — the fold every numeric header
//! suffix applies (read's `:<start>-<end>`, bash's `(timeout Ns)`, grep/find/ls's `limit`).

// `js_number` is used only by `tool_args` itself and by this test, so it is imported from where it
// is defined rather than forced into `transcript`'s re-export list as a dead name. That explicit
// import is also the whole of what this file needs, so the module glob it used to rely on is gone.
use crate::transcript::tool_args::js_number;

#[test]
fn js_number_matches_ecmascript_number_to_string() {
    // Fixed band — Rust's `Display` and `String(n)` agree, including `2.0 -> "2"`.
    assert_eq!(js_number(5.0), "5");
    assert_eq!(js_number(5.5), "5.5");
    assert_eq!(js_number(1234.5), "1234.5");
    assert_eq!(js_number(0.1), "0.1");
    assert_eq!(js_number(100.0), "100");
    assert_eq!(js_number(-0.5), "-0.5");
    // `String(-0) === "0"`.
    assert_eq!(js_number(-0.0), "0");
    assert_eq!(js_number(0.0), "0");
    // Band edges stay fixed: decimal-point position 21 and -5.
    assert_eq!(js_number(1e20), "100000000000000000000");
    assert_eq!(js_number(1e-6), "0.000001");
    assert_eq!(js_number(1e-5), "0.00001");
    assert_eq!(js_number(1.234e-6), "0.000001234");
    // Outside the band ECMA-262 switches to a SIGNED exponent; Rust's `Display` never does.
    assert_eq!(js_number(1e21), "1e+21");
    assert_eq!(js_number(1.5e22), "1.5e+22");
    assert_eq!(js_number(-1e21), "-1e+21");
    assert_eq!(js_number(1e-7), "1e-7");
    assert_eq!(js_number(5e-324), "5e-324");
    assert_eq!(js_number(f64::MAX), "1.7976931348623157e+308");
    // `as_f64` narrows `9007199254740993` exactly as `JSON.parse` does.
    assert_eq!(js_number(9007199254740993.0), "9007199254740992");
    assert_eq!(js_number(123456789012345680000.0), "123456789012345680000");
}

#[test]
fn js_number_breaks_shortest_form_ties_toward_the_even_digit() {
    // `-1149636667324797.25` sits exactly midway between the 17-digit decimals `…97.2` and `…97.3`.
    // Both round-trip, so a shortest-form printer may pick either; ECMA-262 `Number::toString` step
    // 5 requires the EVEN one, and V8 prints `-1149636667324797.2`. Rust's `Display` picks `…97.3`,
    // so `format!("{n}")` alone is not `String(n)` even inside the fixed band.
    let x = f64::from_bits(0xc310_565a_94b4_e5f5);
    assert_eq!(
        format!("{x}"),
        "-1149636667324797.3",
        "Rust `Display` ties away from the even digit"
    );
    assert_eq!(js_number(x), "-1149636667324797.2");
    assert_eq!(js_number(-x), "1149636667324797.2");
}

#[test]
fn js_number_is_total_for_non_json_doubles() {
    // JSON carries neither, but `String(n)` is defined for both and the header must never print
    // Rust's `inf`/`NaN` spelling if one ever reaches it.
    assert_eq!(js_number(f64::INFINITY), "Infinity");
    assert_eq!(js_number(f64::NEG_INFINITY), "-Infinity");
    assert_eq!(js_number(f64::NAN), "NaN");
}
