//! The layer deep-merge (R-07-001): objects merge per-key, arrays and primitives replace.

use serde_json::Value;

/// Deep-merge `over` onto `base` (R-07-001): objects merge recursively per-key; primitives and
/// arrays replace wholesale (matching Pi).
pub fn deep_merge(base: &Value, over: &Value) -> Value {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            let mut out = b.clone();
            for (k, ov) in o {
                let merged = match out.get(k) {
                    Some(bv) => deep_merge(bv, ov),
                    None => ov.clone(),
                };
                out.insert(k.clone(), merged);
            }
            Value::Object(out)
        }
        // arrays + primitives: `over` wins
        (_, over) => over.clone(),
    }
}
