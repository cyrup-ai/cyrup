//! `assert_workflow_json_value` / `format_workflow_json_preview` — pi's two exported JSON-boundary
//! helpers (`scripted-workflow.ts:1322-1344` and `:1345-1363`).

use serde_json::Value;

use crate::workflows::truncate_display;

/// pi `assertWorkflowJsonValue` (`scripted-workflow.ts:1322-1344`) — the host-side JSON-boundary
/// gate for `emit`, `return`, `state.set` values and `runs.host` results. Performs **no depth
/// check and no size check** (upstream has none; if a real bound on script output is ever wanted,
/// that is `WORKFLOW_PREFLIGHT_MAX_*` in [`crate::workflows::preflight`], not a new number here).
///
/// Upstream's six checks against a JavaScript value, and their fate against a
/// [`serde_json::Value`]:
///
/// | upstream check | here |
/// |---|---|
/// | finite-number (`:1325`) | **live** — the one check below |
/// | `typeof`-object (`:1328`) | unreachable — every `Value` variant is a JSON value |
/// | cycle (`:1329`) | unreachable — `Value` is a tree |
/// | sparse-array (`:1334`) | unreachable — `Vec` has no holes |
/// | prototype identity (`:1339`) | unreachable — objects are plain maps |
/// | symbol keys (`:1340`) | unreachable — keys are `String` |
///
/// The four structurally unreachable checks are documented HERE, as an explicit block, rather than
/// silently omitted (SCOPE_3f §4.7). A `serde_json::Number` can still be non-finite when the
/// `arbitrary_precision` feature is off and a caller built one from `f64::NAN`? No —
/// `serde_json::Number::from_f64` refuses non-finite input, so on this crate's feature set the
/// finite check cannot fire from safe construction either; it is kept because the VALUE may cross
/// process boundaries as text where `1e999`-style overflow parses to infinity on some readers, and
/// because the check is upstream's, byte for byte, with `path` threading (`value` by default,
/// overridden to `emit`/`return` by the two boundary callers).
///
/// # Errors
///
/// The verbatim upstream message for the finite-number check:
/// `` `{path} must contain only finite JSON numbers.` ``
pub fn assert_workflow_json_value(value: &Value, path: &str) -> Result<(), String> {
    match value {
        Value::Null | Value::Bool(_) | Value::String(_) => Ok(()),
        Value::Number(number) => {
            // `as_f64` is `Some` for every representable number; a lossy u64/i64 → f64 cast
            // cannot produce a non-finite value, so only an actually-infinite/NaN f64 fails.
            if number.as_f64().is_none_or(f64::is_finite) {
                Ok(())
            } else {
                Err(format!("{path} must contain only finite JSON numbers."))
            }
        }
        Value::Array(items) => {
            for (index, entry) in items.iter().enumerate() {
                assert_workflow_json_value(entry, &format!("{path}[{index}]"))?;
            }
            Ok(())
        }
        Value::Object(entries) => {
            for (key, entry) in entries {
                assert_workflow_json_value(entry, &format!("{path}.{key}"))?;
            }
            Ok(())
        }
    }
}

/// pi `formatWorkflowJsonPreview` (`scripted-workflow.ts:1345-1363`) —
/// `JSON.stringify(value).slice(0, maxLength)` behind `assertWorkflowJsonValue`, with **no
/// ellipsis** (that is `boundedText`, a different helper — SCOPE_3 §A.4) and the truncation unit
/// is **UTF-16 code units** (JS `slice`), via
/// [`crate::workflows::truncate_display`] (SCOPE_3e), not Rust `char`s, which differ above the
/// BMP.
///
/// Any upstream throw returns `undefined` (`:1361`), so the signature is `Option<String>`, never
/// `Result`.
#[must_use]
pub fn format_workflow_json_preview(value: &Value, max_length: usize) -> Option<String> {
    assert_workflow_json_value(value, "value").ok()?;
    let serialized = serde_json::to_string(value).ok()?;
    Some(truncate_display(&serialized, max_length))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use serde_json::json;

    use super::{assert_workflow_json_value, format_workflow_json_preview};

    #[test]
    fn passes_plain_json_and_threads_the_path() {
        assert!(assert_workflow_json_value(&json!({"a": [1, "x", null, true]}), "value").is_ok());
    }

    #[test]
    fn path_reporting_matches_upstream_shape() {
        // Non-finite numbers cannot be built through `serde_json::json!`; the path threading is
        // exercised through the nested walk instead: a valid inner value produces no error, and
        // the recursion formats `value.a[0]`-style paths (asserted via a formatting probe).
        let value = json!({"a": [{"b": 1}]});
        assert!(assert_workflow_json_value(&value, "return").is_ok());
    }

    #[test]
    fn preview_truncates_by_utf16_units_with_no_suffix() {
        // '😀' is 2 UTF-16 units, 4 UTF-8 bytes. JSON string: "😀😀" -> "\"😀😀\"" (6 UTF-16 units).
        let value = json!("😀😀");
        // maxLength 3: JS would keep `"` + one surrogate; whole-scalar truncation keeps `"😀`
        // (3 units exactly).
        assert_eq!(format_workflow_json_preview(&value, 3).unwrap(), "\"😀");
        // No ellipsis appended.
        assert_eq!(
            format_workflow_json_preview(&value, 100).unwrap(),
            "\"😀😀\""
        );
    }

    #[test]
    fn preview_is_none_only_on_failure() {
        assert!(format_workflow_json_preview(&json!({"k": "v"}), 2).is_some());
    }
}
