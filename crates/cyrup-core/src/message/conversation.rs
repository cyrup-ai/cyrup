//! The role-tagged [`Message`] enum — one conversation turn (func-01 §4.2).

use super::assistant::AssistantMessage;
use super::content::{Content, de_tool_result_content, de_user_content};
use super::usage::Usage;
use crate::ToolCallId;

/// A conversation message (func-01 §4.2). Custom (extension/app) message types live in
/// `cyrup-agent`'s `AgentMessage` wrapper and are filtered before the model call (func-02).
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(
    tag = "role",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Message {
    User {
        /// Pi `UserMessage.content: string | (TextContent | ImageContent)[]` (types.ts:379). On
        /// READ, a bare JSON string is accepted and promoted to a single text block; the array form
        /// is NOT validated against the role union (SESS-027 — see [`de_user_content`]). On WRITE, the content array
        /// is ALWAYS emitted — every real Pi entry point that builds a `UserMessage` constructs the
        /// array form `[{type:"text",text}]` (`agent.ts:389`, `agent-harness.ts:38`,
        /// `agent-session.ts:1117`) and Pi's session write path (`session-manager.ts:940,952,959`)
        /// is a pure `JSON.stringify(entry)` with no shape transform, so Pi never collapses a
        /// single-text user turn to the bare-string shorthand on write. cyrup matches those bytes.
        #[serde(default, deserialize_with = "de_user_content")]
        content: Vec<Content>,
        timestamp: i64,
    },
    Assistant(AssistantMessage),
    ToolResult {
        tool_call_id: ToolCallId,
        tool_name: String,
        /// Pi `ToolResultMessage.content: (TextContent | ImageContent)[]` (`ai/src/types.ts:402`).
        /// SESS-027: the union is compile-time TS; deserialization is READ-TOLERANT and rejects no
        /// block type (see [`de_tool_result_content`]).
        #[serde(default, deserialize_with = "de_tool_result_content")]
        content: Vec<Content>,
        #[serde(default)]
        is_error: bool,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        details: Option<serde_json::Value>,
        /// Usage from the tool execution itself, if available. NOT part of main LLM context
        /// accounting (Pi `ToolResultMessage.usage`, types.ts:421-422). Absent when `None`, exactly
        /// as Pi's `JSON.stringify` drops the `undefined` key it assigns unconditionally
        /// (agent-loop.ts:782).
        #[serde(skip_serializing_if = "Option::is_none", default)]
        usage: Option<Usage>,
        /// Names from the tool list that became available after this result (Pi
        /// `ToolResultMessage.addedToolNames`, types.ts:423-428). Providers with native deferred
        /// tool loading use it as the load point; others ignore it. Pi writes the key only when the
        /// array is non-empty (`...(x?.length ? {addedToolNames: x} : {})`, agent-loop.ts:783), so
        /// an empty vec is absent on the wire.
        #[serde(skip_serializing_if = "Vec::is_empty", default)]
        added_tool_names: Vec<String>,
        timestamp: i64,
    },
}

impl serde::Serialize for Message {
    /// Manual serializer so the `role` discriminant appears EXACTLY ONCE and in Pi's field order.
    /// `Assistant` delegates to [`AssistantMessage`]'s self-tagging serializer (which emits
    /// `role:"assistant"` first, then Pi's order); `User`/`ToolResult` write their own `role` then
    /// their fields. A derived internally-tagged `Serialize` would DOUBLE the `role` key for the
    /// `Assistant` arm now that its struct self-tags. `Deserialize` stays derived (`tag = "role"`).
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        match self {
            Message::User { content, timestamp } => {
                let mut st = serializer.serialize_struct("Message", 3)?;
                st.serialize_field("role", "user")?;
                // Always the content array — Pi's real entry points build `[{type:"text",text}]`
                // for every user turn and its write path (`JSON.stringify`, no transform) never
                // collapses a single-text turn to the bare-string shorthand. The bare-string form
                // is READ-tolerated (`de_user_content`) for legacy/foreign JSONL, not written.
                st.serialize_field("content", content)?;
                st.serialize_field("timestamp", timestamp)?;
                st.end()
            }
            Message::Assistant(m) => m.serialize(serializer),
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                is_error,
                details,
                usage,
                added_tool_names,
                timestamp,
            } => {
                // The key ORDER is pi's `createToolResultMessage` object literal
                // (`pi/packages/agent/src/agent-loop.ts:773-787` @v0.83.0; the literal is
                // `:774-786`): role `:775`, toolCallId `:776`, toolName `:777`, content `:780`,
                // details `:781`, usage `:782`, the `...addedToolNames` conditional spread `:783`,
                // **isError `:784`**, timestamp `:785`. pi's session write path is a bare
                // `JSON.stringify(entry)`, so that literal IS the on-disk byte order.
                //
                // PROV-020: `isError` used to be emitted immediately after `content`, three keys
                // too early, which falsified this serializer's whole reason to exist. The previous
                // comment here claimed the new keys "sit next to `details` so every pre-existing key
                // position is unchanged" — that claim was wrong, and it is why the defect survived
                // two passes.
                //
                // `usage` / `addedToolNames` are OMITTED when absent, reproducing pi's bytes: it
                // assigns `usage: finalized.result.usage` (an `undefined` key `JSON.stringify`
                // drops) and spreads `addedToolNames` only when non-empty (agent-loop.ts:782-783).
                let len = 6
                    + usize::from(details.is_some())
                    + usize::from(usage.is_some())
                    + usize::from(!added_tool_names.is_empty());
                let mut st = serializer.serialize_struct("Message", len)?;
                st.serialize_field("role", "toolResult")?;
                st.serialize_field("toolCallId", tool_call_id)?;
                st.serialize_field("toolName", tool_name)?;
                st.serialize_field("content", content)?;
                match details {
                    Some(d) => st.serialize_field("details", d)?,
                    None => st.skip_field("details")?,
                }
                match usage {
                    Some(u) => st.serialize_field("usage", u)?,
                    None => st.skip_field("usage")?,
                }
                if added_tool_names.is_empty() {
                    st.skip_field("addedToolNames")?;
                } else {
                    st.serialize_field("addedToolNames", added_tool_names)?;
                }
                st.serialize_field("isError", is_error)?;
                st.serialize_field("timestamp", timestamp)?;
                st.end()
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_message_uses_camelcase_fields() {
        let m = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "read".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            timestamp: 0,
            usage: None,
            added_tool_names: Vec::new(),
        };
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(v["role"], "toolResult");
        assert_eq!(v["toolCallId"], "tc1");
        assert_eq!(v["toolName"], "read");
        assert_eq!(v["isError"], false);
    }

    /// AGENT-004/005 — a tool result carrying `usage` + `addedToolNames` writes both keys next to
    /// `details` and survives a serialize → deserialize → serialize cycle byte-identically. This is
    /// the on-disk shape (the JSONL session file persists this exact struct).
    #[test]
    fn tool_result_usage_and_added_tool_names_round_trip_byte_identically() {
        let m = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "loader".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: Some(serde_json::json!({ "d": 1 })),
            usage: Some(Usage {
                input: 11,
                output: 22,
                total_tokens: 33,
                ..Usage::default()
            }),
            added_tool_names: vec!["late".to_string()],
            timestamp: 7,
        };
        let first = serde_json::to_string(&m).expect("serialize");
        // Both keys present, and positioned after `details` (Pi keeps them adjacent, types.ts:419-428).
        assert!(first.contains(r#""usage":{"#), "{first}");
        assert!(first.contains(r#""addedToolNames":["late"]"#), "{first}");

        // PROV-020 — the FULL key order of pi's `createToolResultMessage` literal
        // (`agent-loop.ts:773-787` @v0.83.0): … details, usage, ...addedToolNames, isError,
        // timestamp. `isError` used to be emitted right after `content`, three keys too early, so
        // a cyrup-exported `toolResult` line was not byte-identical to pi's — the single property
        // this hand-written serializer exists to provide. Red before the fix.
        let at = |k: &str| {
            first
                .find(k)
                .unwrap_or_else(|| panic!("{k} missing in {first}"))
        };
        assert!(at(r#""content""#) < at(r#""details""#));
        assert!(at(r#""details""#) < at(r#""usage""#));
        assert!(at(r#""usage""#) < at(r#""addedToolNames""#));
        assert!(at(r#""addedToolNames""#) < at(r#""isError""#));
        assert!(at(r#""isError""#) < at(r#""timestamp""#));

        let back: Message = serde_json::from_str(&first).expect("deserialize");
        assert_eq!(back, m, "value round-trips");
        assert_eq!(
            serde_json::to_string(&back).expect("re-serialize"),
            first,
            "bytes round-trip"
        );
    }

    /// BACKWARD compatibility — NEW code reading an OLD session file. The two keys are absent, so
    /// `#[serde(default)]` yields `None`/`[]`, and re-export reproduces the ORIGINAL bytes exactly:
    /// a pre-change session file is not rewritten or corrupted by the widened struct.
    #[test]
    fn old_shape_tool_result_reads_and_re_exports_unchanged() {
        let old = concat!(
            r#"{"role":"toolResult","toolCallId":"tc1","toolName":"read","#,
            r#""content":[{"type":"text","text":"ok"}],"isError":false,"timestamp":7}"#
        );
        let m: Message = serde_json::from_str(old).expect("old shape parses");
        match &m {
            Message::ToolResult {
                usage,
                added_tool_names,
                ..
            } => {
                assert_eq!(usage, &None, "absent `usage` defaults to None");
                assert!(
                    added_tool_names.is_empty(),
                    "absent `addedToolNames` defaults to []"
                );
            }
            other => panic!("expected a tool result, got {other:?}"),
        }
        assert_eq!(
            serde_json::to_string(&m).expect("re-serialize"),
            old,
            "byte-identical re-export"
        );
    }

    /// FORWARD compatibility — OLD code reading a NEW session file. `OldToolResult` mirrors the
    /// pre-change variant exactly (same serde attrs, no `usage`/`addedToolNames`); nothing in the
    /// message model carries `deny_unknown_fields`, so the two extra keys are silently DROPPED and
    /// the entry still parses. It does NOT fail to deserialize, which is what would demote the line
    /// to `Entry::Unknown` in the session reader. The loss is lossy-but-non-fatal, exactly as for
    /// any other forward-added key.
    #[test]
    fn new_shape_tool_result_still_parses_under_the_pre_change_shape() {
        #[derive(serde::Deserialize, serde::Serialize)]
        #[serde(rename_all = "camelCase")]
        struct OldToolResult {
            role: String,
            tool_call_id: String,
            tool_name: String,
            content: Vec<Content>,
            #[serde(default)]
            is_error: bool,
            #[serde(skip_serializing_if = "Option::is_none", default)]
            details: Option<serde_json::Value>,
            timestamp: i64,
        }

        let new_msg = Message::ToolResult {
            tool_call_id: "tc1".into(),
            tool_name: "loader".into(),
            content: vec![Content::text("ok")],
            is_error: false,
            details: None,
            usage: Some(Usage {
                input: 11,
                output: 22,
                total_tokens: 33,
                ..Usage::default()
            }),
            added_tool_names: vec!["late".to_string()],
            timestamp: 7,
        };
        let new_bytes = serde_json::to_string(&new_msg).expect("serialize");

        let old: OldToolResult =
            serde_json::from_str(&new_bytes).expect("pre-change shape still parses new bytes");
        assert_eq!(old.role, "toolResult");
        assert_eq!(old.tool_name, "loader");
        assert_eq!(old.timestamp, 7);
        // Old code re-exports without the two keys (lossy, non-fatal).
        let re = serde_json::to_string(&old).expect("re-serialize");
        assert!(!re.contains("usage"), "{re}");
        assert!(!re.contains("addedToolNames"), "{re}");
        // And the NEW reader recovers defaults from those old bytes without error.
        let back: Message = serde_json::from_str(&re).expect("new reader parses old bytes");
        match back {
            Message::ToolResult {
                usage,
                added_tool_names,
                ..
            } => {
                assert_eq!(usage, None);
                assert!(added_tool_names.is_empty());
            }
            other => panic!("expected a tool result, got {other:?}"),
        }
    }

    #[test]
    fn user_content_serializes_single_text_as_array_like_pi() {
        // Every real Pi entry point builds the ARRAY form `[{type:"text",text}]` for a single-text
        // user turn (agent.ts:389, agent-harness.ts:38, agent-session.ts:1117) and Pi's write path
        // (session-manager.ts:940,952,959 — pure JSON.stringify) never collapses it to a bare
        // string. cyrup must emit the same bytes, even for a single signature-less text block.
        let m = Message::User {
            content: vec![Content::text("hi")],
            timestamp: 7,
        };
        let v = serde_json::to_value(&m).expect("serialize");
        assert_eq!(
            v["content"],
            serde_json::json!([{ "type": "text", "text": "hi" }])
        );
        let back: Message = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, m);
        // The bare-string shorthand is still READ-tolerated for legacy/foreign JSONL, promoting to
        // a single text block (Pi's `content: string | Content[]` union accepts it on load).
        let legacy: Message = serde_json::from_value(
            serde_json::json!({ "role": "user", "content": "hi", "timestamp": 7 }),
        )
        .expect("deserialize bare-string legacy shorthand");
        assert_eq!(legacy, m);
        // A text block carrying a signature stays the array form (the signature must survive).
        let m2 = Message::User {
            content: vec![Content::text_with_signature("hi", "sig")],
            timestamp: 0,
        };
        let v2 = serde_json::to_value(&m2).expect("serialize");
        assert!(v2["content"].is_array());
        // Two blocks / an image stay the array form.
        let m3 = Message::User {
            content: vec![Content::text("a"), Content::text("b")],
            timestamp: 0,
        };
        assert!(serde_json::to_value(&m3).expect("serialize")["content"].is_array());
    }
}
