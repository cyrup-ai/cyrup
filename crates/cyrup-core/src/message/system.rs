//! The system turn: [`SystemMessage`], the transcript's own prompt and tool-declaration state
//! (PROV-083a).

use super::content::{Content, de_system_content};
use super::sections::Sections;
use crate::tool_def::{ToolDef, ToolReference};

/// System instructions and tool declarations at ONE POINT in the transcript.
///
/// Pi `SystemMessage` — `packages/ai/src/types.ts:491-507` @v0.87.1, ported field for field. The
/// docblock there is the contract, and it is reproduced because it is the whole model:
///
/// > The leading system message is the system prompt. Later system messages change it: `content`
/// > adds instructions from that point on, `sections` replace or remove named prompt sections, and
/// > `toolsAdded`/`toolsRemoved` change the tool set. Replaying every system message in order yields
/// > the current prompt and tools. Providers that accept system messages mid-conversation send each
/// > one in place; other providers rebuild the leading system message from the replayed state.
///
/// The replay itself is `cyrup_provider::utils::transcript`.
///
/// This is a STRUCT with a self-tagging serializer, and [`crate::Message::System`] is a newtype arm
/// delegating to it, exactly as [`crate::AssistantMessage`] and `Message::Assistant` are paired.
/// Upstream is an `interface` in the `Message` union, and a single owner of the key order means the
/// agent-layer wrapper (`cyrup_agent::AgentMessage::System`) reuses these bytes instead of
/// re-deriving them — the divergence risk the hand-written serializers exist to remove.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemMessage {
    /// Pi `content: string | TextContent[]` (`types.ts:493`). On the LEADING message this is the
    /// base prompt; later, additional instructions.
    ///
    /// On READ the bare string, the array, and `null` are all accepted (see [`de_system_content`]).
    /// On WRITE the BARE STRING form is emitted whenever the content is representable as one (empty,
    /// or a single signature-less text block), because that is the only form pi's producers ever
    /// build: `createInitialSystemMessage` writes `content: systemPrompt ?? ""`
    /// (`utils/transcript.ts:19`), `getCurrentSystemMessage` writes `content: content.join("\n\n")`
    /// (`:97`), and `packages/coding-agent/src/core/agent-session.ts:1420`/`:1441` both write a
    /// string. pi's session write path is a bare `JSON.stringify(entry)` with no shape transform, so
    /// that string IS the on-disk byte form.
    ///
    /// This is the MIRROR of [`crate::Message::User`], where every pi producer builds the ARRAY and
    /// cyrup therefore always writes the array.
    #[serde(default, deserialize_with = "de_system_content")]
    pub content: Vec<Content>,
    /// Pi `sections?: Record<string, string | null>` (`types.ts:496-501`) — ORDERED named sections;
    /// a `None` value inside is pi's `null`, "remove this section". Absent when `None`, reproducing
    /// `...(sections.size > 0 ? { sections } : {})` (`utils/transcript.ts:93`): an unset key is
    /// ABSENT on the wire, never `null`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub sections: Option<Sections>,
    /// Pi `toolsAdded?: Tool[]` (`types.ts:504`) — COMPLETE definitions of tools that become
    /// available at this point. Written only when non-empty, reproducing
    /// `...(hasTools ? { toolsAdded: tools } : {})` (`utils/transcript.ts:20`).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools_added: Vec<ToolDef>,
    /// Pi `toolsRemoved?: ToolReference[]` (`types.ts:506`) — tools that stop being available at this
    /// point. Name-only, and absent when empty.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tools_removed: Vec<ToolReference>,
    /// Unix timestamp in milliseconds.
    pub timestamp: i64,
}

/// Where `toolsAdded`/`toolsRemoved` sit relative to `timestamp` on the wire.
///
/// pi writes system messages from two kinds of producer, and they build their object literals in
/// different orders. The key order is therefore a property of where a message came from, not of the
/// type, and a rewrite that picks the wrong one reorders every pi row it touches.
#[derive(Clone, Copy)]
enum ToolKeys {
    /// `role, content, sections?, timestamp, toolsAdded?, toolsRemoved?` — the agent loop's
    /// `withToolChanges` (`packages/agent/src/agent-loop.ts:368-375` @v1.0.0):
    /// `{ ...rest, ...(toolsAdded.length > 0 ? { toolsAdded } : {}), ...(toolsRemoved… ) }`, where
    /// `rest` is the pending message minus its two tool keys and so already ends in `timestamp`.
    /// Every system message that becomes a `message` entry passes through it (the prompt update
    /// `_preparePromptAndToolLoadout` returns, `agent-session.ts:1702`, and the declaration the loop
    /// inserts on its own, `agent-loop.ts:350`), so this is the default.
    AfterTimestamp,
    /// `role, content, sections?, toolsAdded?, timestamp` — a REPLAYED snapshot, which pi builds
    /// with `getCurrentSystemMessage` (`packages/ai/src/utils/transcript.ts:95-100` @v1.0.0). Its one
    /// persisted use is a `compaction` entry's `systemMessage` (`session-manager.ts:1270-1283`, a
    /// spread of that snapshot with `timestamp` overwritten in place). A snapshot has no removals.
    BeforeTimestamp,
}

impl serde::Serialize for SystemMessage {
    /// Self-tagging serializer: emits `role: "system"` FIRST, then pi's field order for a system
    /// message that is a `message` entry — role, content, sections?, timestamp, toolsAdded?,
    /// toolsRemoved? (see [`ToolKeys::AfterTimestamp`] for where that order comes from).
    ///
    /// A pi row written by pi's loop therefore round-trips byte for byte. This is NOT the
    /// `interface SystemMessage` declaration order (`types.ts:492-507`), which an earlier revision of
    /// this serializer followed: pi's session write path is a bare `JSON.stringify(entry)`, so the
    /// order the producer's literal builds is the on-disk order, whatever the interface says.
    ///
    /// The three optional keys are OMITTED when absent rather than written as `null`, reproducing
    /// pi's `...(x ? {k: x} : {})` spreads exactly. The derived `Deserialize` ignores the extra
    /// `role` key on read.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.serialize_with_order(serializer, ToolKeys::AfterTimestamp)
    }
}

impl SystemMessage {
    /// Serialize as a replayed snapshot: role, content, sections?, toolsAdded?, timestamp (see
    /// [`ToolKeys::BeforeTimestamp`]). For the `systemMessage` of a `compaction` entry, which pi
    /// writes in this order and not in the order of a `message` entry. `toolsRemoved` is never
    /// written, because a replayed snapshot has none.
    pub fn serialize_replayed<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.serialize_with_order(serializer, ToolKeys::BeforeTimestamp)
    }

    /// `#[serde(serialize_with = …)]` adapter for an `Option<SystemMessage>` field that holds a
    /// replayed snapshot: [`Self::serialize_replayed`] for `Some`, `null` for `None` (callers pair it
    /// with `skip_serializing_if = "Option::is_none"`, so the `None` arm is not reached in practice).
    pub fn serialize_replayed_opt<S>(
        message: &Option<SystemMessage>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match message {
            Some(m) => m.serialize_replayed(serializer),
            None => serializer.serialize_none(),
        }
    }

    fn serialize_with_order<S>(&self, serializer: S, keys: ToolKeys) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct as _;
        let replayed = matches!(keys, ToolKeys::BeforeTimestamp);
        let removed = !replayed && !self.tools_removed.is_empty();
        let len = 3
            + usize::from(self.sections.is_some())
            + usize::from(!self.tools_added.is_empty())
            + usize::from(removed);
        let mut st = serializer.serialize_struct("SystemMessage", len)?;
        st.serialize_field("role", "system")?;
        match self.content_as_str() {
            Some(text) => st.serialize_field("content", text)?,
            None => st.serialize_field("content", &self.content)?,
        }
        match &self.sections {
            Some(s) => st.serialize_field("sections", s)?,
            None => st.skip_field("sections")?,
        }
        if replayed {
            self.serialize_tools_added(&mut st)?;
            st.serialize_field("timestamp", &self.timestamp)?;
        } else {
            st.serialize_field("timestamp", &self.timestamp)?;
            self.serialize_tools_added(&mut st)?;
            if removed {
                st.serialize_field("toolsRemoved", &self.tools_removed)?;
            } else {
                st.skip_field("toolsRemoved")?;
            }
        }
        st.end()
    }

    fn serialize_tools_added<S>(&self, st: &mut S) -> Result<(), S::Error>
    where
        S: serde::ser::SerializeStruct,
    {
        if self.tools_added.is_empty() {
            st.skip_field("toolsAdded")
        } else {
            st.serialize_field("toolsAdded", &self.tools_added)
        }
    }
}

impl SystemMessage {
    /// The bare-string rendering of `content`, when there is one: `""` for no blocks, and the text
    /// of a single signature-less [`Content::Text`]. `None` means the content needs the array form to
    /// survive (several blocks, a signature, an image, a thinking block).
    ///
    /// Every `SystemMessage` pi builds has content of exactly this shape, so this is the branch that
    /// makes a cyrup-written system entry byte-identical to pi's. See the [`Self::content`] docs.
    fn content_as_str(&self) -> Option<&str> {
        match self.content.as_slice() {
            [] => Some(""),
            [
                Content::Text {
                    text,
                    text_signature: None,
                },
            ] => Some(text.as_ref()),
            _ => None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use super::*;
    use crate::Message;

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("{name} desc"),
            parameters: serde_json::json!({ "type": "object" }),
            constrained_sampling: None,
        }
    }

    /// PROV-083a — the ON-DISK shape. Pins pi's exact key ORDER for a `message` entry (the agent
    /// loop's `withToolChanges`, `packages/agent/src/agent-loop.ts:368-375` @v1.0.0: `timestamp`
    /// BEFORE `toolsAdded`/`toolsRemoved`) and the absent-not-`null` rule for the three optional
    /// keys, then a full serialize → deserialize → serialize byte round trip. cyrup-session persists
    /// this struct into the JSONL, and pi's write path is a bare `JSON.stringify(entry)`, so this
    /// key order IS the parity surface.
    #[test]
    fn system_message_jsonl_key_order_and_absent_not_null() {
        let sections = Sections::from_iter([
            ("tone".to_string(), Some("terse".to_string())),
            ("scope".to_string(), None),
        ]);
        let m = Message::System(SystemMessage {
            content: vec![Content::text("be helpful")],
            sections: Some(sections),
            tools_added: vec![tool("read")],
            tools_removed: vec![ToolReference::new("write")],
            timestamp: 7,
        });
        let first = serde_json::to_string(&m).expect("serialize");

        let at = |k: &str| {
            first
                .find(k)
                .unwrap_or_else(|| panic!("{k} missing in {first}"))
        };
        assert!(at(r#""role""#) < at(r#""content""#));
        assert!(at(r#""content""#) < at(r#""sections""#));
        assert!(at(r#""sections""#) < at(r#""timestamp""#));
        assert!(at(r#""timestamp""#) < at(r#""toolsAdded""#));
        assert!(at(r#""toolsAdded""#) < at(r#""toolsRemoved""#));
        // `role` appears EXACTLY ONCE — the whole reason `Message`'s serializer is hand-written and
        // this arm delegates instead of being internally tagged on top of a self-tagging payload.
        assert_eq!(first.matches(r#""role""#).count(), 1, "{first}");
        // The BARE STRING content form, which is the only form pi's producers build.
        assert!(first.contains(r#""content":"be helpful""#), "{first}");
        // `sections` keeps WIRE order, and a removal is a real JSON `null` inside it.
        assert!(
            first.contains(r#""sections":{"tone":"terse","scope":null}"#),
            "insertion order, not alphabetized: {first}"
        );

        let back: Message = serde_json::from_str(&first).expect("deserialize");
        assert_eq!(back, m, "value round-trips");
        assert_eq!(
            serde_json::to_string(&back).expect("re-serialize"),
            first,
            "bytes round-trip"
        );

        // The three optional keys are ABSENT when unset, never `null` — pi's
        // `...(x ? {k: x} : {})` spreads (`utils/transcript.ts:20`, `:93`, `:98`).
        let minimal = serde_json::to_string(&Message::System(SystemMessage {
            content: Vec::new(),
            sections: None,
            tools_added: Vec::new(),
            tools_removed: Vec::new(),
            timestamp: 0,
        }))
        .expect("serialize");
        assert_eq!(
            minimal, r#"{"role":"system","content":"","timestamp":0}"#,
            "the empty prompt is `content:\"\"`, and nothing else is written"
        );
        let back: Message = serde_json::from_str(&minimal).expect("deserialize");
        assert_eq!(
            serde_json::to_string(&back).expect("re-serialize"),
            minimal,
            "byte-identical"
        );
    }

    /// READ tolerance, matching the sibling arms (SESS-027): the array `content` form, a JSON `null`
    /// content (pi normalizes it to `""` at `coding-agent/src/core/session-manager.ts:444`), and an
    /// absent `content` all load.
    #[test]
    fn system_message_read_tolerance_matches_pis_loader() {
        let null_content: Message =
            serde_json::from_str(r#"{"role":"system","content":null,"timestamp":1}"#)
                .expect("null content loads");
        let Message::System(m) = &null_content else {
            panic!("expected a system message");
        };
        assert!(m.content.is_empty());
        assert_eq!(
            serde_json::to_string(&null_content).expect("serialize"),
            r#"{"role":"system","content":"","timestamp":1}"#,
            "pi's own `content == null` normalization, reproduced on write"
        );

        let absent: Message =
            serde_json::from_str(r#"{"role":"system","timestamp":2}"#).expect("absent content");
        let Message::System(m) = &absent else {
            panic!("expected a system message");
        };
        assert!(m.content.is_empty());

        // The ARRAY form loads and is preserved in VALUE; a single signature-less block re-exports as
        // the bare string, which is the form pi writes and which `contentText` reads identically.
        let array: Message = serde_json::from_str(
            r#"{"role":"system","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}],"timestamp":3}"#,
        )
        .expect("array content");
        let Message::System(m) = &array else {
            panic!("expected a system message");
        };
        assert_eq!(m.content.len(), 2, "both blocks survive");
        // Two blocks cannot collapse to a bare string, so the array form is written back.
        assert!(
            serde_json::to_string(&array)
                .expect("serialize")
                .contains(r#""content":[{"type":"text","text":"a"}"#)
        );
    }

    /// `Sections` is ORDER-PRESERVING with JS `Map` semantics, and serializes as a JSON object in
    /// that order. A `HashMap` would randomize the rendered prompt; a `BTreeMap` would alphabetize
    /// it. Both are silent bugs, which is why the container is hand-written.
    #[test]
    fn sections_preserve_wire_order_and_patch_in_place() {
        let json = r#"{"zulu":"z","alpha":"a","mike":null}"#;
        let s: Sections = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            s.iter().collect::<Vec<_>>(),
            vec![("zulu", Some("z")), ("alpha", Some("a")), ("mike", None)],
            "wire order, NOT alphabetical"
        );
        assert_eq!(
            serde_json::to_string(&s).expect("serialize"),
            json,
            "byte round trip"
        );
        assert_eq!(
            s.values().collect::<Vec<_>>(),
            vec!["z", "a"],
            "nulls skipped"
        );

        // `Map.set` on an existing key keeps its POSITION; a new key appends; `delete` removes.
        let mut s = s;
        s.set("zulu", Some("Z".to_string()));
        s.set("new", Some("n".to_string()));
        assert!(s.remove("alpha"));
        assert!(!s.remove("absent"));
        assert_eq!(
            s.iter().collect::<Vec<_>>(),
            vec![("zulu", Some("Z")), ("mike", None), ("new", Some("n"))]
        );
        assert_eq!(s.get("zulu"), Some(Some("Z")));
        assert_eq!(s.get("mike"), Some(None), "present and null");
        assert_eq!(s.get("alpha"), None, "absent");
        assert_eq!(s.len(), 3);
        assert!(!s.is_empty());
    }

    /// The pre-existing arms are UNTOUCHED by the new variant: an old `toolResult`/`user` line still
    /// round-trips byte-identically, and a `system` line does not deserialize as any of them.
    #[test]
    fn adding_the_system_variant_leaves_the_other_arms_byte_identical() {
        let user = r#"{"role":"user","content":[{"type":"text","text":"hi"}],"timestamp":7}"#;
        let m: Message = serde_json::from_str(user).expect("user parses");
        assert_eq!(serde_json::to_string(&m).expect("serialize"), user);

        let tr = concat!(
            r#"{"role":"toolResult","toolCallId":"tc1","toolName":"read","#,
            r#""content":[{"type":"text","text":"ok"}],"isError":false,"timestamp":7}"#
        );
        let m: Message = serde_json::from_str(tr).expect("toolResult parses");
        assert_eq!(serde_json::to_string(&m).expect("serialize"), tr);
    }
    /// CODE-014 — a system row pi's loop wrote survives a cyrup REWRITE byte for byte.
    ///
    /// The literal is what `JSON.stringify` writes for the object `withToolChanges` returns over a
    /// pending prompt update (`agent-loop.ts:368-375` @v1.0.0): the pending message's own keys
    /// (`role, content, sections, timestamp`) first, then `toolsAdded`, then `toolsRemoved`. The
    /// assertion is on BYTES: `assert_eq!` on parsed JSON passes whatever the order is, and the order
    /// is the only thing a rewrite can change.
    #[test]
    fn a_system_row_written_by_pis_loop_rewrites_byte_for_byte() {
        let row = concat!(
            r#"{"role":"system","content":"","#,
            r#""sections":{"preamble":"You are an expert coding assistant.","cwd":"<cwd>\n/work\n</cwd>"},"#,
            r#""timestamp":1700000000000,"#,
            r#""toolsAdded":[{"name":"read","description":"Read a file","parameters":{"type":"object","properties":{}}}],"#,
            r#""toolsRemoved":[{"name":"old"}]}"#,
        );
        let parsed: Message = serde_json::from_str(row).expect("a pi row loads");
        assert_eq!(
            serde_json::to_string(&parsed).expect("serialize"),
            row,
            "a cyrup rewrite of a pi row must not reorder its keys"
        );

        // The order the previous serializer wrote (`interface SystemMessage`'s declaration order)
        // is a DIFFERENT string for the same value, so the equality above is not vacuous.
        let declaration_order = concat!(
            r#"{"role":"system","content":"","#,
            r#""sections":{"preamble":"You are an expert coding assistant.","cwd":"<cwd>\n/work\n</cwd>"},"#,
            r#""toolsAdded":[{"name":"read","description":"Read a file","parameters":{"type":"object","properties":{}}}],"#,
            r#""toolsRemoved":[{"name":"old"}],"#,
            r#""timestamp":1700000000000}"#,
        );
        let reloaded: Message = serde_json::from_str(declaration_order).expect("also loads");
        assert_eq!(reloaded, parsed, "the two orders are one value");
        assert_ne!(
            serde_json::to_string(&reloaded).expect("serialize"),
            declaration_order
        );
    }

    /// The other order pi writes: a `compaction` entry's `systemMessage` is a replayed snapshot
    /// (`getCurrentSystemMessage`, `utils/transcript.ts:95-100` @v1.0.0), `toolsAdded` BEFORE
    /// `timestamp`, and never `toolsRemoved`.
    #[test]
    fn a_replayed_snapshot_keeps_its_own_key_order() {
        let snapshot = SystemMessage {
            content: Vec::new(),
            sections: Some(Sections::from_iter([(
                "preamble".to_string(),
                Some("p".to_string()),
            )])),
            tools_added: vec![tool("read")],
            tools_removed: vec![ToolReference::new("ignored")],
            timestamp: 9,
        };
        struct Replayed<'a>(&'a SystemMessage);
        impl serde::Serialize for Replayed<'_> {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                self.0.serialize_replayed(s)
            }
        }
        let text = serde_json::to_string(&Replayed(&snapshot)).expect("serialize");
        assert_eq!(
            text,
            concat!(
                r#"{"role":"system","content":"","sections":{"preamble":"p"},"#,
                r#""toolsAdded":[{"name":"read","description":"read desc","parameters":{"type":"object"}}],"#,
                r#""timestamp":9}"#,
            )
        );
        // Both orders load to the same value (minus the removals a snapshot never carries).
        let back: SystemMessage = serde_json::from_str(&text).expect("loads");
        assert_eq!(back.timestamp, 9);
        assert_eq!(back.tools_added.len(), 1);
    }
}
