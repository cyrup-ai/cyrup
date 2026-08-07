//! JS-number coercion for tool parameters (arch-03 §6; R-03-038).
//!
//! Every numeric parameter on Pi's built-in tools is declared `Type.Number` — never
//! `Type.Integer`, never with a `minimum` (read.ts:22-23, grep.ts:31-34, ls.ts:16, find.ts:25,
//! bash.ts:42) — and Pi performs **no runtime schema validation**: `wrapToolDefinition`
//! (tool-definition-wrapper.ts:16-18) hands the model's parsed JSON straight to `execute`. So
//! `{"limit": 10.0}` and `{"offset": -1}` are legal inputs upstream; JS coerces them at the point
//! of use and the tool still returns a result.
//!
//! Modeling those fields as `usize` made `serde_json::from_value` reject the whole call with an
//! `invalid` error before the tool ran — a hard failure where Pi succeeds. Tool inputs therefore
//! deserialize as [`f64`] (matching `bash`'s already-corrected `timeout`, bash.rs:24) and are
//! folded to an integer here, at the same place JS would fold them.

/// ECMA-262 §7.1.5 `ToIntegerOrInfinity`: the coercion `Array.prototype.slice`, `String.slice`
/// and every other index-taking builtin applies to a `Number` argument — `NaN` becomes `0` and
/// anything else truncates **toward zero** (`2.9 -> 2`, `-2.9 -> -2`).
///
/// Rust's `f64 as i64` cast is already saturating with `NaN -> 0` and truncates toward zero, so
/// this is that cast plus the citation; the named function exists so call sites read as the JS
/// operation they are porting rather than as an unexplained cast. Infinities saturate to
/// `i64::{MAX,MIN}` instead of `±Infinity`, which is unreachable from JSON input anyway (the
/// grammar has no `Infinity`/`NaN` literal).
#[allow(clippy::cast_possible_truncation)]
pub(crate) fn to_integer(v: f64) -> i64 {
    v as i64
}

/// [`to_integer`] followed by a clamp into `usize`, for the parameters that are counts or 0-based
/// indices. Negative values become `0`; values past `usize::MAX` saturate.
///
/// This is *not* an extra clamp invented by cyrup: each caller documents the Pi expression that
/// makes a negative value behave as zero there (`Math.max(0, …)` in read.ts:271, an
/// immediately-satisfied `results.length >= effectiveLimit` in ls.ts:156, `fd --max-results` in
/// find.ts:241). Callers whose Pi counterpart clamps to something else (grep's
/// `Math.max(1, limit ?? DEFAULT_LIMIT)`, grep.ts:189) apply their own clamp on top.
pub(crate) fn to_count(v: f64) -> usize {
    let n = to_integer(v);
    if n < 0 {
        0
    } else {
        usize::try_from(n).unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    // ECMA-262 §7.1.5 ground truth, reproduced in Node:
    //   [2.9, -2.9, 0, -0, 10].map(n => Math.trunc(n))  // => [2, -2, 0, -0, 10]
    #[test]
    fn to_integer_truncates_toward_zero() {
        assert_eq!(to_integer(2.9), 2);
        assert_eq!(to_integer(-2.9), -2);
        assert_eq!(to_integer(10.0), 10);
        assert_eq!(to_integer(f64::NAN), 0);
    }

    #[test]
    fn to_count_floors_negatives_at_zero() {
        assert_eq!(to_count(-1.0), 0);
        assert_eq!(to_count(-0.5), 0);
        assert_eq!(to_count(10.0), 10);
        assert_eq!(to_count(2.9), 2);
    }
}
