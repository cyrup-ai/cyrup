//! A `cyrup-llama-cpp-wire` definition against the real server's answer to the same request.
//!
//! The rule: **every key the definition claims is present in the real answer, with the same JSON
//! type**, recursively through objects and arrays. Values are the definition's own (a fake has
//! no tokenizer and no clock), and keys the real server sends that the definition leaves out are
//! allowed (the definitions document which ones they leave out and why). An INTEGER in the
//! definition requires an integer in the real answer, because a client deserializing into an
//! integer type rejects `1.5`; a float in the definition accepts any number.
//!
//! The literal bodies (the 404, the 401, the error envelope) are stronger than this and are
//! compared byte for byte by the caller.

use serde_json::Value;

/// The JSON type of a value, with integers told apart from other numbers.
fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(number) if number.is_f64() => "number",
        Value::Number(_) => "integer",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// `null`, or the kind and the value: `object {}`, `integer 1`.
fn show(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        other => format!("{} {other}", kind(other)),
    }
}

fn compatible(defined: &Value, real: &Value) -> bool {
    match (kind(defined), kind(real)) {
        (d, r) if d == r => true,
        // A float in the definition accepts a real integer: both read as `f64`.
        ("number", "integer") => true,
        _ => false,
    }
}

fn walk(defined: &Value, real: &Value, path: &str, out: &mut Vec<String>) {
    if !compatible(defined, real) {
        out.push(format!(
            "{path}: the definition has {}, the real server sent {}",
            show(defined),
            show(real)
        ));
        return;
    }
    match (defined, real) {
        (Value::Object(defined), Value::Object(real)) => {
            for (key, value) in defined {
                match real.get(key) {
                    Some(real_value) => walk(value, real_value, &format!("{path}.{key}"), out),
                    None => out.push(format!(
                        "{path}.{key}: the definition has it, the real server did not send it"
                    )),
                }
            }
        }
        (Value::Array(defined), Value::Array(real)) => {
            if !defined.is_empty() && real.is_empty() {
                out.push(format!(
                    "{path}: the definition has {} element(s), the real server sent none",
                    defined.len()
                ));
            }
            // Element i against element i; a definition longer than the answer compares its
            // extra elements against the answer's first (same shape, more of them).
            for (index, value) in defined.iter().enumerate() {
                if let Some(real_value) = real.get(index).or_else(|| real.first()) {
                    walk(value, real_value, &format!("{path}[{index}]"), out);
                }
            }
        }
        _ => {}
    }
}

/// Every disagreement between `defined` and `real`, one line each, `[]` when they agree.
pub fn mismatches(defined: &Value, real: &Value) -> Vec<String> {
    let mut out = Vec::new();
    walk(defined, real, "$", &mut out);
    out
}

/// Fail, naming the definition and every disagreement, unless `real` conforms to `defined`.
#[track_caller]
pub fn assert_conforms(what: &str, defined: &Value, real: &Value) {
    let found = mismatches(defined, real);
    assert!(
        found.is_empty(),
        "{what}: the real llama-server disagrees with cyrup-llama-cpp-wire's definition:\n  {}\nreal \
         answer: {real}",
        found.join("\n  ")
    );
}

/// The comparator itself, with no server: it must catch each kind of disagreement it exists for,
/// or a live run proves nothing.
mod comparator {
    use serde_json::json;

    use super::mismatches;

    #[test]
    fn extra_real_keys_and_different_values_agree() {
        assert!(
            mismatches(
                &json!({ "a": 1, "b": { "c": "x" }, "d": [1.5] }),
                &json!({ "a": 7, "b": { "c": "y", "e": null }, "d": [2, 3.25], "z": true }),
            )
            .is_empty()
        );
    }

    #[test]
    fn a_missing_key_disagrees() {
        let found = mismatches(&json!({ "a": { "b": 1 } }), &json!({ "a": {} }));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].starts_with("$.a.b:"), "{found:?}");
    }

    #[test]
    fn an_object_where_the_server_sends_null_disagrees() {
        let found = mismatches(&json!({ "params": {} }), &json!({ "params": null }));
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("object {}"), "{found:?}");
        assert!(found[0].contains("null"), "{found:?}");
    }

    #[test]
    fn an_integer_where_the_server_sends_a_float_disagrees() {
        assert_eq!(
            mismatches(&json!({ "n": 1 }), &json!({ "n": 1.5 })).len(),
            1
        );
        assert!(mismatches(&json!({ "n": 1.5 }), &json!({ "n": 1 })).is_empty());
    }

    #[test]
    fn array_elements_are_compared_and_an_empty_answer_disagrees() {
        assert_eq!(
            mismatches(&json!([{ "id": 1 }]), &json!([{ "id": "x" }])).len(),
            1
        );
        assert_eq!(mismatches(&json!([{ "id": 1 }]), &json!([])).len(), 1);
        assert!(mismatches(&json!([]), &json!([1, 2])).is_empty());
    }
}
