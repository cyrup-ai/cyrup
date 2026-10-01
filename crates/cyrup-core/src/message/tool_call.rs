//! The model-issued [`ToolCall`] and its self-tagging serializer (func-01 §4.4).

use crate::ToolCallId;
use crate::lazy_args::LazyArgs;

/// A model-issued tool call (func-01 §4.4).
///
/// Pi's `ToolCall` data type ALWAYS carries `type: "toolCall"` (types.ts:344-345). cyrup makes the
/// bare struct self-tag via a manual [`serde::Serialize`] that emits `type` first, in Pi's
/// declaration order (`type`, `id`, `name`, `arguments`, `thoughtSignature?`, `namespace?`). This is the single
/// source of truth for the discriminant: [`crate::Content::ToolCall`] delegates here (so it does NOT
/// inject a second `type` — no duplicate key), and `StreamEvent::ToolCallEnd.tool_call` serializes
/// the bare struct directly. [`serde::Deserialize`] is derived (it tolerates the extra `type` key
/// present in Pi input — no field binds it — keeping read 1:1 with Pi).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub id: ToolCallId,
    pub name: String,
    /// Tool arguments. Pi types this as `Record<string, any>` — always a JSON object (types.ts:348);
    /// cyrup mirrors that exactly with [`LazyArgs`], which derefs to, serializes as and
    /// deserializes from `serde_json::Map<String, Value>`, so the type still cannot hold a
    /// non-object (array/string/number/null). Decoders that tolerate streaming partial-JSON yield
    /// an empty object (`{}`) for incomplete/invalid input rather than a scalar. The indirection
    /// exists so a streamed snapshot can carry the raw argument buffer and build the map only if
    /// something reads it (PERF-001).
    pub arguments: LazyArgs,
    /// Provider-opaque (Google); stripped on cross-provider handoff (func-01 R-01-030).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub thought_signature: Option<String>,
    /// OpenAI Responses namespace for calls to dynamically loaded or namespaced tools (Pi
    /// `ToolCall.namespace`, types.ts:392-393). Captured by the Responses decoder and replayed on
    /// the wire only for the same provider, api and model (DRIFT-058); every other provider ignores
    /// it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub namespace: Option<String>,
}

impl serde::Serialize for ToolCall {
    /// Self-tagging serializer: emits `type: "toolCall"` first (Pi `ToolCall.type`, types.ts:345),
    /// then `id`, `name`, `arguments`, `thoughtSignature` and `namespace` (each only when present)
    /// — byte-1:1 with Pi's `ToolCall` interface (types.ts:387-394). Single source of the discriminant: callers that
    /// embed a `ToolCall` (the [`crate::Content::ToolCall`] variant, `StreamEvent::ToolCallEnd.tool_call`)
    /// delegate here rather than injecting their own `type`, so the key is never duplicated.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        let len = 4
            + usize::from(self.thought_signature.is_some())
            + usize::from(self.namespace.is_some());
        let mut st = serializer.serialize_struct("ToolCall", len)?;
        st.serialize_field("type", "toolCall")?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("arguments", &self.arguments)?;
        match &self.thought_signature {
            Some(sig) => st.serialize_field("thoughtSignature", sig)?,
            None => st.skip_field("thoughtSignature")?,
        }
        match &self.namespace {
            Some(ns) => st.serialize_field("namespace", ns)?,
            None => st.skip_field("namespace")?,
        }
        st.end()
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::message::*;

    #[test]
    fn bare_tool_call_self_tags_with_exactly_one_type_key() {
        // Req 1 + 4: Pi's `ToolCall` always carries `type:"toolCall"` (types.ts:344-345). The bare
        // struct self-tags exactly once, in Pi field order, and round-trips.
        let tc = ToolCall {
            id: "tc1".into(),
            name: "read".into(),
            arguments: serde_json::Map::new().into(),
            thought_signature: None,
            namespace: None,
        };
        let s = serde_json::to_string(&tc).expect("serialize");
        assert_eq!(
            s.matches("\"type\"").count(),
            1,
            "exactly one type key: {s}"
        );
        let v = serde_json::to_value(&tc).expect("serialize");
        assert_eq!(v["type"], "toolCall");
        assert_eq!(v["id"], "tc1");
        assert_eq!(v["name"], "read");
        assert!(v["arguments"].is_object());
        assert!(
            v.get("thoughtSignature").is_none(),
            "omitted when None: {v}"
        );
        // Round-trip (req 4).
        assert_eq!(
            serde_json::from_value::<ToolCall>(v).expect("deserialize"),
            tc
        );
        // thoughtSignature is emitted (camelCase) when present and still round-trips.
        let tc_sig = ToolCall {
            thought_signature: Some("sig".into()),
            ..tc.clone()
        };
        let vs = serde_json::to_value(&tc_sig).expect("serialize");
        assert_eq!(vs["thoughtSignature"], "sig");
        assert_eq!(
            serde_json::from_value::<ToolCall>(vs).expect("deserialize"),
            tc_sig
        );
    }

    #[test]
    fn namespace_serializes_after_thought_signature_and_round_trips() {
        // DRIFT-058: Pi's `ToolCall` declares `namespace?` after `thoughtSignature?`
        // (types.ts:392-393), so the JSONL key order is type, id, name, arguments,
        // thoughtSignature, namespace.
        let tc = ToolCall {
            id: "tc1".into(),
            name: "lookup".into(),
            arguments: serde_json::Map::new().into(),
            thought_signature: Some("sig".into()),
            namespace: Some("dynamic_tools".into()),
        };
        let s = serde_json::to_string(&tc).expect("serialize");
        assert_eq!(
            s,
            r#"{"type":"toolCall","id":"tc1","name":"lookup","arguments":{},"thoughtSignature":"sig","namespace":"dynamic_tools"}"#
        );
        assert_eq!(
            serde_json::from_str::<ToolCall>(&s).expect("round trip"),
            tc
        );
        // Without a signature the namespace still follows `arguments` directly.
        let no_sig = ToolCall {
            thought_signature: None,
            ..tc.clone()
        };
        let s = serde_json::to_string(&no_sig).expect("serialize");
        assert_eq!(
            s,
            r#"{"type":"toolCall","id":"tc1","name":"lookup","arguments":{},"namespace":"dynamic_tools"}"#
        );
        assert_eq!(
            serde_json::from_str::<ToolCall>(&s).expect("round trip"),
            no_sig
        );
        // Absent stays absent (no `"namespace":null`), and a namespaced call embedded in `Content`
        // carries the key through the delegating serializer.
        let plain = ToolCall {
            namespace: None,
            ..no_sig
        };
        assert!(
            !serde_json::to_string(&plain)
                .expect("serialize")
                .contains("namespace")
        );
        let c = serde_json::to_value(Content::ToolCall(tc)).expect("serialize");
        assert_eq!(c["namespace"], "dynamic_tools");
    }

    #[test]
    fn tool_call_arguments_reject_non_object() {
        // gap 11: Pi types ToolCall.arguments as Record<string, any> — a scalar/array is rejected.
        let ok = serde_json::json!({
            "type": "toolCall", "id": "t", "name": "n", "arguments": { "a": 1 },
        });
        let tc: Content = serde_json::from_value(ok).expect("object arguments deserialize");
        assert!(matches!(tc, Content::ToolCall(_)));
        let bad = serde_json::json!({
            "type": "toolCall", "id": "t", "name": "n", "arguments": [1, 2, 3],
        });
        assert!(serde_json::from_value::<Content>(bad).is_err());
    }
}
