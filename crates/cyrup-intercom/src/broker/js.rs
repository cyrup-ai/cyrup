//! JavaScript value semantics reproduced verbatim, for the handful of places where the protocol
//! echoes an untyped JSON value back to a peer (`v0.9.2 broker/broker.ts:1371,:1382`) or reads one
//! as a truthiness test (`v0.10.1 broker/broker.ts:1041,:1045`).
//!
//! These are not conveniences: a peer may be matching on the exact string the broker sends back, so
//! `String(x)` and `!!x` are ported rather than approximated. Split out of `broker/mod.rs` because
//! they are pure functions with no broker state involved at all.

/// `Number.MAX_SAFE_INTEGER` (`2^53 - 1`) — the bound `Number.isSafeInteger` enforces. Defined here
/// rather than imported from `transport::protocol`'s private one, the precedent
/// `broker/runtime_claim.rs:50` already sets.
const JS_MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// `typeof v === "number" && Number.isSafeInteger(v) && v >= 0` — the guard pi applies to
/// `msg.expectedRevision` before it reaches the state manager (`v0.9.2 broker/broker.ts:1417`, and
/// again at `extension-state.ts:132`).
///
/// A value-level twin of `transport::protocol`'s `js_safe_integer` DESERIALIZER, which cannot be
/// reused here: the extension handlers read a raw [`serde_json::Value`] frame and never deserialize
/// a typed `ClientMessage`.
///
/// `None` is every rejected shape at once: absent, `null`, non-numeric, fractional, negative, or
/// above `2^53 - 1`. JS treats `-0` as a safe integer that is not `< 0`, so it maps to `Some(0)`.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the f64 arm is reached only after fract() == 0.0 and a 0..=2^53-1 range check, so the \
              cast is exact and non-negative by construction"
)]
pub(super) fn js_safe_u64(v: Option<&serde_json::Value>) -> Option<u64> {
    let n = v?.as_number()?;
    if let Some(u) = n.as_u64() {
        return (u <= JS_MAX_SAFE_INTEGER).then_some(u);
    }
    let f = n.as_f64()?;
    if !f.is_finite() || f.fract() != 0.0 || f < 0.0 || f > JS_MAX_SAFE_INTEGER as f64 {
        return None;
    }
    Some(f as u64)
}

/// `String(msg.namespace || "")` — the expression pi echoes into the two `extension_state_result`
/// frames it emits *before* `namespace` has been type-checked
/// (`v0.9.2 broker/broker.ts:1371` and `:1382`). Those are the only two places in the protocol
/// where an arbitrary untyped JSON value is coerced to a string, so the JS coercion is reproduced
/// here rather than approximated: the field is echoed back to a peer that may be matching on it.
///
/// `||` short-circuits on every JS falsy value, so `undefined`/`null`/`false`/`0`/`""` all yield
/// `""`; anything else goes through `ToString`.
///
/// [CYRUP-DELTA] Number formatting agrees with JS for every integral value under `1e21` and for
/// the shortest-round-trip decimals both runtimes emit, but not for JS's exponent notation
/// (`String(1e21)` is `"1e+21"` upstream and `1e21` here). JSON cannot carry `NaN`/`Infinity` at
/// all, so those cases are unreachable rather than divergent.
pub(super) fn js_string_or_empty(v: Option<&serde_json::Value>) -> String {
    match v {
        None => String::new(),
        Some(v) if js_is_falsy(v) => String::new(),
        Some(v) => js_to_string(v),
    }
}

/// JS falsiness for the JSON value subset (`undefined` is the `None` arm of the caller).
fn js_is_falsy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => true,
        serde_json::Value::Bool(b) => !b,
        serde_json::Value::Number(n) => n.as_f64() == Some(0.0),
        serde_json::Value::String(s) => s.is_empty(),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => false,
    }
}

/// JS `ToString` for the JSON value subset, WITHOUT the `|| ""` falsy short-circuit
/// [`js_string_or_empty`] applies. `v0.9.2 broker/broker.ts:1394` writes `String(namespace)` where
/// `:1371` and `:1382` write `String(msg.namespace || "")`, and the difference is observable:
/// `namespace: 0` echoes `"0"` there and `""` in the two earlier branches.
/// Arrays go through `Array.prototype.join(",")`, which
/// renders `null` elements as the empty string and recurses into nested arrays; every plain object
/// stringifies to `"[object Object]"`.
pub(super) fn js_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => js_number_to_string(n),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(|it| match it {
                serde_json::Value::Null => String::new(),
                other => js_to_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        serde_json::Value::Object(_) => "[object Object]".to_string(),
    }
}

/// JS `Number::toString`. `1.0` is the integer `1` upstream, so an integral `f64` is printed
/// without its fractional part; everything else falls back to serde's shortest round-trip form,
/// which matches JS across the ordinary decimal range.
fn js_number_to_string(n: &serde_json::Number) -> String {
    if let Some(i) = n.as_i64() {
        return i.to_string();
    }
    if let Some(u) = n.as_u64() {
        return u.to_string();
    }
    match n.as_f64() {
        Some(f) if f.is_finite() && f.fract() == 0.0 && f.abs() < 1e21 => format!("{f:.0}"),
        _ => n.to_string(),
    }
}

/// `info.runtimeFallbackAlias` read as a JS **truthiness** test, the way the mailbox identity guard
/// reads it (`v0.10.1 broker/broker.ts:1041`, `:1045`).
///
/// `undefined` and `false` are both falsy upstream, so an explicit `runtimeFallbackAlias: false` —
/// which cyrup's own presence path can send (`transport/protocol.rs:727`) — must NOT disqualify a
/// session from owning its mailbox identity. `Option::is_some` would.
pub(super) const fn js_truthy_alias(alias: Option<bool>) -> bool {
    matches!(alias, Some(true))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use serde_json::json;

    /// `String(msg.namespace || "")`, the coercion pi applies to the raw `namespace` before it has
    /// been type-checked (`v0.9.2 broker/broker.ts:1371,1382`). Every case below is what node
    /// prints for the same expression.
    #[test]
    fn js_string_or_empty_matches_the_js_coercion() {
        // `||` short-circuits on every falsy value.
        assert_eq!(js_string_or_empty(None), "");
        assert_eq!(js_string_or_empty(Some(&json!(null))), "");
        assert_eq!(js_string_or_empty(Some(&json!(false))), "");
        assert_eq!(js_string_or_empty(Some(&json!(0))), "");
        assert_eq!(js_string_or_empty(Some(&json!(0.0))), "");
        assert_eq!(js_string_or_empty(Some(&json!(""))), "");
        // Truthy values go through `ToString`.
        assert_eq!(js_string_or_empty(Some(&json!("ns"))), "ns");
        assert_eq!(js_string_or_empty(Some(&json!(42))), "42");
        assert_eq!(js_string_or_empty(Some(&json!(-7))), "-7");
        assert_eq!(js_string_or_empty(Some(&json!(42.5))), "42.5");
        // `1.0` is the integer `1` in JS; serde would otherwise print "1.0".
        assert_eq!(js_string_or_empty(Some(&json!(1.0_f64))), "1");
        assert_eq!(js_string_or_empty(Some(&json!(true))), "true");
        assert_eq!(
            js_string_or_empty(Some(&json!({"a": 1}))),
            "[object Object]"
        );
        // `Array.prototype.join(",")`: null elements render empty and nesting flattens.
        assert_eq!(js_string_or_empty(Some(&json!([1, 2]))), "1,2");
        assert_eq!(js_string_or_empty(Some(&json!([null]))), "");
        assert_eq!(js_string_or_empty(Some(&json!([[1, 2], 3]))), "1,2,3");
        assert_eq!(js_string_or_empty(Some(&json!([{}]))), "[object Object]");
        // A non-empty array is truthy even when it joins to "" — `String([null] || "")` is `""`
        // via `join`, not via the `||`, and both paths agree here.
        assert_eq!(js_string_or_empty(Some(&json!([]))), "");
    }
}
