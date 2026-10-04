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

/// `deepMergeSettings(base, overrides)` (Pi `core/settings-manager.ts:250-255` @v1.0.0): the
/// generic [`deep_merge`] plus the one `defaultTools` special case.
///
/// CFG-097. Upstream's merge is not uniform any more:
///
/// ```text
/// const merged = deepMergeObjects(base, overrides) as Settings;
/// const defaultTools = mergeDefaultTools(base.defaultTools, overrides.defaultTools);
/// return defaultTools === undefined ? merged : { ...merged, defaultTools };
/// ```
///
/// The special case is applied at the TOP LEVEL ONLY — a nested `defaultTools` key inside some
/// other object is merged by the generic rule, because upstream reads `base.defaultTools` and
/// `overrides.defaultTools` off the two settings objects directly and never recurses with that
/// knowledge. [`super::merge_default_tools`] holds the rule itself.
///
/// Every settings-layer merge goes through here (upstream `:413`, `:594`, `:604`, `:632`, `:637`,
/// `:737`, `:755`); [`deep_merge`] stays as the generic primitive for everything that is not a
/// settings layer.
pub fn deep_merge_settings(base: &Value, over: &Value) -> Value {
    let merged = deep_merge(base, over);
    // `base.defaultTools` / `overrides.defaultTools`: `None` is upstream's `undefined`, i.e. the
    // key is absent. A JSON `null` is NOT folded into that — upstream distinguishes the two
    // (`overrides !== undefined` is true for `null`, which then falls through to the
    // malformed-value leg and replaces), and cyrup's `deep_merge` likewise treats `null` as a real
    // value rather than skipping it.
    let resolved = super::merge_default_tools(base.get("defaultTools"), over.get("defaultTools"));
    match (merged, resolved) {
        (Value::Object(mut obj), Some(tools)) => {
            obj.insert("defaultTools".to_string(), tools);
            Value::Object(obj)
        }
        (merged, _) => merged,
    }
}
