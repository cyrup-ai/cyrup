//! [`stable_json`] / [`stable_json_digest`] — the canonical-JSON digest, ports pi
//! `shared/launch-contract.ts:9-23`.

use sha2::{Digest as _, Sha256};

/// pi `stableJson` (`launch-contract.ts:9-19`): arrays element-wise, objects with **sorted keys**,
/// scalars as their plain JSON rendering.
///
/// [CYRUP-DELTA] Upstream sorts object keys with `localeCompare` — ICU collation, under which
/// `"a"` orders before `"B"`. This port sorts by **Unicode scalar / byte order** (`"B"` before
/// `"a"`), so for an object with mixed-case keys the canonical string — and therefore the digest —
/// differs from pi's. Every digest in this crate is both produced and compared by this
/// implementation (a permit's `script_digest` is written by [`super::WorkflowResourcePermit::issue`]
/// and re-derived by its `consume`), so cyrup is self-consistent; a reader comparing a cyrup
/// digest against a pi digest across the wire must expect a mismatch on mixed-case keys, and this
/// comment is where that answer lives.
///
/// pi's other two rules port structurally: `undefined`-valued entries are dropped (`:13`) —
/// [`serde_json::Value`] has no `undefined`, and `Value::Null` is a real value that is **kept**,
/// matching JS `null` — and scalars fall through to `JSON.stringify`.
#[must_use]
pub fn stable_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Array(items) => {
            let rendered: Vec<String> = items.iter().map(stable_json).collect();
            format!("[{}]", rendered.join(","))
        }
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(&String, &serde_json::Value)> = map.iter().collect();
            entries.sort_by(|(left, _), (right, _)| left.as_bytes().cmp(right.as_bytes()));
            let rendered: Vec<String> = entries
                .into_iter()
                .map(|(key, entry)| {
                    format!(
                        "{}:{}",
                        serde_json::Value::String(key.clone()),
                        stable_json(entry)
                    )
                })
                .collect();
            format!("{{{}}}", rendered.join(","))
        }
        // Null / Bool / Number / String — `Value`'s `Display` IS compact JSON, i.e.
        // `JSON.stringify(value)` for a scalar.
        scalar => scalar.to_string(),
    }
}

/// pi `stableJsonDigest` (`launch-contract.ts:21-23`): `sha256(stableJson(value))`, hex.
///
/// The explicit hex fold follows `watchdog/change_signature.rs`'s `sha256_hex`: `sha2` 0.11's
/// digest output does not implement `LowerHex`, so `{:x}` does not compile on it.
#[must_use]
pub fn stable_json_digest(value: &serde_json::Value) -> String {
    Sha256::digest(stable_json(value).as_bytes()).iter().fold(
        String::with_capacity(64),
        |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        },
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::*;

    /// Key order in the input never changes the canonical string; nesting, arrays and kept nulls
    /// all render; and the byte-order divergence from `localeCompare` is pinned (`B` < `a`).
    #[test]
    fn stable_json_canonicalizes_by_byte_order() {
        let one = json!({ "b": 1, "a": { "y": [1, "two", null], "x": true } });
        let two = json!({ "a": { "x": true, "y": [1, "two", null] }, "b": 1 });
        assert_eq!(stable_json(&one), stable_json(&two));
        assert_eq!(
            stable_json(&one),
            "{\"a\":{\"x\":true,\"y\":[1,\"two\",null]},\"b\":1}"
        );
        // The documented delta: byte order puts "B" (0x42) before "a" (0x61); localeCompare
        // would not.
        assert_eq!(stable_json(&json!({ "a": 1, "B": 2 })), "{\"B\":2,\"a\":1}");
        // A bare string degenerates to JSON.stringify — the shape the resource digest uses.
        assert_eq!(stable_json(&json!("script\n")), "\"script\\n\"");
    }

    /// The digest is stable across key order and distinguishes distinct values.
    #[test]
    fn digest_is_stable_and_discriminating() {
        let one = stable_json_digest(&json!({ "b": 1, "a": 2 }));
        let two = stable_json_digest(&json!({ "a": 2, "b": 1 }));
        let other = stable_json_digest(&json!({ "a": 2, "b": 2 }));
        assert_eq!(one, two);
        assert_ne!(one, other);
        assert_eq!(one.len(), 64, "sha-256 hex");
    }
}
