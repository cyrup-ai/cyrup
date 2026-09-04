//! The typed [`Content`] block and the per-role content deserializers (func-01 §4.4).

use super::tool_call::ToolCall;
use crate::shared_str::SharedStr;

/// A typed content block (func-01 §4.4).
///
/// Per-role typing (gap 9): Pi types content per role — assistant = `Text|Thinking|ToolCall`,
/// user/toolResult = `Text|Image` (types.ts:379/385/402). cyrup keeps one ergonomic `Content` enum
/// but enforces Pi's per-role unions at the wire boundary with validating deserializers
/// ([`de_assistant_content`], [`de_user_content`], [`de_tool_result_content`]): a payload carrying
/// an `Image` in an assistant turn — or a `ToolCall`/`Thinking` in a user/tool-result turn — is
/// REJECTED on deserialize, exactly as Pi's typed unions reject it. Producers still build the right
/// variants by construction.
#[derive(Clone, Debug, PartialEq, serde::Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum Content {
    Text {
        text: SharedStr,
        /// Legacy opaque id string OR a JSON-encoded [`crate::TextSignatureV1`] (Pi `textSignature`,
        /// types.ts:325). Use [`crate::TextSignatureV1::parse`]/`encode` for the structured form.
        #[serde(skip_serializing_if = "Option::is_none", default)]
        text_signature: Option<String>,
    },
    Thinking {
        thinking: SharedStr,
        #[serde(skip_serializing_if = "Option::is_none", default)]
        thinking_signature: Option<String>,
        /// Pi `redacted?: boolean` (types.ts:335) — OMITTED when unset. Pi only ever emits
        /// `redacted: true` (a safety-redacted block); an un-redacted block leaves the key
        /// `undefined`, so `JSON.stringify` drops it. cyrup keeps the field a plain `bool`
        /// (`false` = not redacted); the manual [`Content`] serializer omits the `false` default, so
        /// a non-redacted block emits no `redacted` key — byte-1:1 with Pi — while `redacted: true`
        /// still writes.
        #[serde(default)]
        redacted: bool,
    },
    ToolCall(ToolCall),
    Image {
        /// base64-encoded.
        data: String,
        mime_type: String,
    },
}

impl serde::Serialize for Content {
    /// Internally-tagged serializer (`tag = "type"`, camelCase fields) written by hand so the
    /// `ToolCall` variant can DELEGATE to [`ToolCall`]'s own self-tagging serializer — the single
    /// source of the `type:"toolCall"` discriminant. A derived internally-tagged serializer would
    /// inject its own `type` on top of `ToolCall`'s, producing a duplicate key; delegating avoids
    /// that while keeping the other variants byte-1:1 with the prior derived output.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        match self {
            Content::Text {
                text,
                text_signature,
            } => {
                let mut st = serializer
                    .serialize_struct("Content", 2 + usize::from(text_signature.is_some()))?;
                st.serialize_field("type", "text")?;
                st.serialize_field("text", text)?;
                match text_signature {
                    Some(sig) => st.serialize_field("textSignature", sig)?,
                    None => st.skip_field("textSignature")?,
                }
                st.end()
            }
            Content::Thinking {
                thinking,
                thinking_signature,
                redacted,
            } => {
                let len = 2 + usize::from(thinking_signature.is_some()) + usize::from(*redacted);
                let mut st = serializer.serialize_struct("Content", len)?;
                st.serialize_field("type", "thinking")?;
                st.serialize_field("thinking", thinking)?;
                match thinking_signature {
                    Some(sig) => st.serialize_field("thinkingSignature", sig)?,
                    None => st.skip_field("thinkingSignature")?,
                }
                if *redacted {
                    st.serialize_field("redacted", &true)?;
                } else {
                    st.skip_field("redacted")?;
                }
                st.end()
            }
            // Single source of the `type:"toolCall"` discriminant: delegate to `ToolCall` so the tag
            // is emitted exactly once (no duplicate key), with `id`/`name`/`arguments`/
            // `thoughtSignature?` flattened — byte-1:1 with Pi's tool-call content.
            Content::ToolCall(tool_call) => tool_call.serialize(serializer),
            Content::Image { data, mime_type } => {
                let mut st = serializer.serialize_struct("Content", 3)?;
                st.serialize_field("type", "image")?;
                st.serialize_field("data", data)?;
                st.serialize_field("mimeType", mime_type)?;
                st.end()
            }
        }
    }
}

impl Content {
    pub fn text(s: impl Into<SharedStr>) -> Self {
        Content::Text {
            text: s.into(),
            text_signature: None,
        }
    }
    pub fn thinking(s: impl Into<SharedStr>) -> Self {
        Content::Thinking {
            thinking: s.into(),
            thinking_signature: None,
            redacted: false,
        }
    }
    /// A text block carrying a (legacy or [`crate::TextSignatureV1`]-encoded) signature.
    pub fn text_with_signature(s: impl Into<SharedStr>, signature: impl Into<String>) -> Self {
        Content::Text {
            text: s.into(),
            text_signature: Some(signature.into()),
        }
    }
}

/// Deserialize `UserMessage.content` accepting Pi's bare-string shorthand OR the content array
/// (Pi `content: string | (TextContent | ImageContent)[]`, `ai/src/types.ts:379`). A bare string
/// becomes a single [`Content::Text`].
///
/// SESS-027 — this doc used to promise that "a `Thinking`/`ToolCall` block is rejected", which the
/// body deliberately does NOT do. Pi's per-role content unions are COMPILE-TIME TypeScript only:
/// its session read path is `JSON.parse(line) as FileEntry` with a catch that skips only
/// MALFORMED JSON (`parseSessionEntryLine`, `core/session-manager.ts:503-511` @v0.83.0), so an
/// off-union block loads fine there. SESS-001 removed the rejection here to match; the doc
/// outliving it is worse than no doc, because the next reader "restores" a validation pi never
/// had and cyrup then refuses a session file pi loads.
pub(super) fn de_user_content<'de, D>(deserializer: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum StringOrArray {
        Str(String),
        Arr(Vec<Content>),
    }
    // Pi runtime read-tolerance (see `de_assistant_content`): accept the bare-string shorthand or
    // the content array, with no role-union rejection. A JSON `null` (or an absent key, via
    // `#[serde(default)]`) normalizes to `[]` — see `de_assistant_content` for the Pi citation.
    Ok(match Option::<StringOrArray>::deserialize(deserializer)? {
        Some(StringOrArray::Str(s)) => vec![Content::text(s)],
        Some(StringOrArray::Arr(v)) => v,
        None => Vec::new(),
    })
}

/// Deserialize `ToolResultMessage.content` (Pi `content: (TextContent | ImageContent)[]`,
/// `ai/src/types.ts:402`).
///
/// SESS-027 — READ-TOLERANT, not validating: the union is compile-time TS only and pi's read path
/// is a bare `JSON.parse` (`core/session-manager.ts:503-511` @v0.83.0). A `null` or absent
/// `content` normalizes to `[]`.
pub(super) fn de_tool_result_content<'de, D>(deserializer: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    // Pi runtime read-tolerance (see `de_assistant_content`): no role-union rejection, and a
    // `null`/absent `content` normalizes to `[]`.
    Ok(Option::<Vec<Content>>::deserialize(deserializer)?.unwrap_or_default())
}

/// Deserialize `AssistantMessage.content` (Pi
/// `content: (TextContent | ThinkingContent | ToolCall)[]`, `ai/src/types.ts:385`).
///
/// SESS-027 — READ-TOLERANT, not validating; an `Image` block is ACCEPTED. pi's union is
/// compile-time TS and its read path is `JSON.parse(line) as FileEntry`, skipping only malformed
/// JSON (`core/session-manager.ts:503-511` @v0.83.0), so any session JSONL pi loads, cyrup loads.
pub(super) fn de_assistant_content<'de, D>(deserializer: D) -> Result<Vec<Content>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize as _;
    // Pi's per-role content unions are COMPILE-TIME TS only; its runtime `JSON.parse` accepts any
    // block regardless of role (no schema validation, `ai/src/types.ts:385`). cyrup matches that
    // read tolerance 1:1 — no role-union rejection — so any session JSONL Pi loads, cyrup loads.
    //
    // That tolerance extends to a MISSING or `null` `content`: Pi normalizes it to `[]` rather than
    // dropping the message — `sessionEntryToContextMessages` (`session-manager.ts:383-395`):
    // "Session files are parsed without validation; old versions, forks, or hand-edited files can
    // contain messages with null/missing content", then
    // `if ((role === "user" || role === "assistant" || role === "toolResult") && content == null)
    //  return [{ ...message, content: [] }];` (`==` also catches `undefined`, i.e. an absent key —
    // hence `#[serde(default)]` on the three `content` fields). Without this, cyrup's strict
    // deserializer fails the whole `Message`, the session entry demotes to `Entry::Unknown`, and the
    // turn silently vanishes from LLM context, compaction input and token accounting. The
    // SERIALIZER is unchanged: cyrup, like Pi, always writes the array form back.
    Ok(Option::<Vec<Content>>::deserialize(deserializer)?.unwrap_or_default())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::message::*;

    #[test]
    fn content_serializes_camelcase_tagged() {
        let c = Content::Thinking {
            thinking: "hm".into(),
            thinking_signature: Some("sig".into()),
            redacted: false,
        };
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v["type"], "thinking");
        assert_eq!(v["thinking"], "hm");
        assert_eq!(v["thinkingSignature"], "sig");
    }

    #[test]
    fn thinking_redacted_omitted_when_false_emitted_when_true() {
        // gap 3: Pi `redacted?: boolean` (types.ts:335) is omitted when unset — Pi only ever
        // emits `redacted: true`. A non-redacted block must NOT serialize a `redacted` key.
        let not_redacted = Content::thinking("hm");
        let v = serde_json::to_value(&not_redacted).expect("serialize");
        assert!(v.get("redacted").is_none(), "false must be omitted: {v}");
        // Absent on the wire round-trips back to `false`.
        let back: Content = serde_json::from_value(v).expect("deserialize");
        assert_eq!(back, not_redacted);
        // A redacted block still writes `redacted: true`.
        let redacted = Content::Thinking {
            thinking: "x".into(),
            thinking_signature: Some("sig".into()),
            redacted: true,
        };
        let rv = serde_json::to_value(&redacted).expect("serialize");
        assert_eq!(rv["redacted"], true);
        assert_eq!(
            serde_json::from_value::<Content>(rv).expect("deserialize"),
            redacted
        );
    }

    #[test]
    fn content_tool_call_flattens_single_type_key_no_duplicate() {
        // Req 2 + 4: `Content::ToolCall` delegates to `ToolCall`'s self-tag — exactly ONE
        // `type:"toolCall"` (no duplicate key), fields flattened, byte-1:1 with Pi tool-call content.
        let tc = ToolCall {
            id: "t".into(),
            name: "n".into(),
            arguments: serde_json::Map::new().into(),
            thought_signature: Some("g".into()),
        };
        let c = Content::ToolCall(tc);
        let s = serde_json::to_string(&c).expect("serialize");
        assert_eq!(
            s.matches("\"type\"").count(),
            1,
            "no duplicate type key: {s}"
        );
        assert!(
            s.starts_with("{\"type\":\"toolCall\""),
            "type emitted first: {s}"
        );
        let v = serde_json::to_value(&c).expect("serialize");
        assert_eq!(v["type"], "toolCall");
        assert_eq!(v["id"], "t");
        assert_eq!(v["name"], "n");
        assert_eq!(v["thoughtSignature"], "g");
        // Round-trip (req 4): Pi input (with `type` present) deserializes back to an equal value.
        assert_eq!(
            serde_json::from_value::<Content>(v).expect("deserialize"),
            c
        );
    }

    #[test]
    fn user_content_accepts_bare_string_shorthand() {
        // Pi-interop: a user message whose `content` is a bare JSON string.
        let json = serde_json::json!({ "role": "user", "content": "hello", "timestamp": 7 });
        let m: Message = serde_json::from_value(json).expect("deserialize");
        assert_eq!(
            m,
            Message::User {
                content: vec![Content::text("hello")],
                timestamp: 7
            }
        );
        // The array form still deserializes.
        let json2 = serde_json::json!({
            "role": "user",
            "content": [{ "type": "text", "text": "hi" }],
            "timestamp": 0,
        });
        let m2: Message = serde_json::from_value(json2).expect("deserialize");
        assert_eq!(
            m2,
            Message::User {
                content: vec![Content::text("hi")],
                timestamp: 0
            }
        );
    }

    #[test]
    fn assistant_content_accepts_image_on_deserialize_like_pi() {
        // Pi's runtime is type-erased: `JSON.parse` accepts an image in an assistant turn even
        // though the compile-time TS union forbids it (types.ts:385). cyrup matches that read
        // tolerance 1:1 — a session JSONL Pi loads, cyrup loads.
        let json = serde_json::json!({
            "role": "assistant",
            "content": [{ "type": "image", "data": "x", "mimeType": "image/png" }],
            "provider": "faux", "model": "m", "api": "faux",
            "usage": Usage::default(), "stopReason": "stop", "timestamp": 0,
        });
        let m = serde_json::from_value::<Message>(json).expect("Pi accepts an off-union block");
        match m {
            Message::Assistant(a) => {
                assert!(matches!(a.content.as_slice(), [Content::Image { .. }]))
            }
            other => panic!("expected assistant, got {other:?}"),
        }
    }

    #[test]
    fn user_and_tool_result_content_accept_off_union_blocks_like_pi() {
        // Pi runtime read-tolerance (see above): user/toolResult content is typed Text|Image at
        // compile time but `JSON.parse` accepts any block (types.ts:379,402). cyrup matches 1:1.
        let user = serde_json::json!({
            "role": "user",
            "content": [{ "type": "toolCall", "id": "t", "name": "n", "arguments": {} }],
            "timestamp": 0,
        });
        assert!(serde_json::from_value::<Message>(user).is_ok());
        let tr = serde_json::json!({
            "role": "toolResult", "toolCallId": "t", "toolName": "n",
            "content": [{ "type": "thinking", "thinking": "x" }],
            "isError": false, "timestamp": 0,
        });
        assert!(serde_json::from_value::<Message>(tr).is_ok());
    }
}
