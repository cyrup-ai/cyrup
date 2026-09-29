//! 1:1 port of `packages/ai/src/utils/transcript.ts` @v0.87.1 (PROV-083a).
//!
//! A transcript carries its own system instructions and tool declarations: the LEADING
//! [`Message::System`] is the system prompt and the initial tool set, and every later one is a
//! DELTA — `content` adds instructions from that point on, `sections` replace or remove named
//! prompt sections, and `tools_added`/`tools_removed` change the tool set
//! (`packages/ai/src/types.ts:483-507`).
//!
//! Replaying those deltas is what this module does. [`normalize_context`] folds the shorthand
//! [`Context::system_prompt`]/[`Context::tools`] into a leading system message and is the ONLY
//! public constructor of a [`TranscriptContext`]; [`get_current_tools`] and
//! [`get_current_system_message`] resolve the state after every delta; [`resolve_transcript`] and
//! [`resolve_transcript_tools`] decide how much of that state a given provider can be handed in
//! place versus rebuilt at the head.

use crate::context::{Context, ToolDef, ToolReference};
use crate::utils::text::{content_text_default, get_system_message_text};
use cyrup_core::{Content, Message, Sections, SystemMessage};

/// Pi `TranscriptContext` (`packages/ai/src/types.ts:625-634`): the NORMALIZED request context
/// passed to providers and API implementations. The prompt and tool declarations are carried by the
/// transcript's system messages, not by sibling fields.
///
/// Upstream brands this type with a `unique symbol` so that *"Only `normalizeContext()` produces
/// this type, so a raw `Context` cannot reach provider code by accident"* (`types.ts:628-631`). The
/// Rust analogue of that brand is this newtype's PRIVATE field: outside this module nothing can
/// build one, so [`normalize_context`] is the only way in. Inside this module the three functions
/// that upstream writes as `{ messages } as TranscriptContext`
/// (`normalizeContext` :33, `collapseSystemMessages` :112, and `resolveTranscript` via it) construct
/// it directly — the same set of producers the brand admits.
///
/// ```compile_fail
/// // A raw message list cannot be promoted to a normalized transcript: the field is private, so
/// // `normalize_context` (or a `collapse`/`resolve` of one it produced) is the only way in.
/// let _ = cyrup_provider::context::TranscriptContext {
///     messages: Vec::new(),
/// };
/// ```
///
/// ```
/// // The door that IS open.
/// let ctx = cyrup_provider::utils::transcript::normalize_context(&Default::default());
/// assert!(ctx.messages().is_empty());
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranscriptContext {
    messages: Vec<Message>,
}

impl TranscriptContext {
    /// The transcript, prompt and tool declarations included.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Consume the wrapper for the message list.
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }
}

/// `createInitialSystemMessage` (`transcript.ts:11-24`): build the leading system message for a
/// prompt and tool set. Returns `None` when BOTH are empty, so an empty transcript stays empty.
pub fn create_initial_system_message(
    system_prompt: Option<&str>,
    tools: &[ToolDef],
) -> Option<SystemMessage> {
    let has_system_prompt = system_prompt.is_some_and(|p| !p.is_empty());
    let has_tools = !tools.is_empty();
    if !has_system_prompt && !has_tools {
        return None;
    }
    Some(SystemMessage {
        // `content: systemPrompt ?? ""` (:19). The empty prompt is the empty block list, which
        // serializes back out as `""` — see `de_system_content`.
        content: match system_prompt.filter(|p| !p.is_empty()) {
            Some(p) => vec![Content::text(p)],
            None => Vec::new(),
        },
        sections: None,
        // `...(hasTools ? { toolsAdded: tools } : {})` (:20).
        tools_added: tools.to_vec(),
        tools_removed: Vec::new(),
        timestamp: 0,
    })
}

/// `normalizeContext` (`transcript.ts:31-35`): fold [`Context::system_prompt`] and
/// [`Context::tools`] into a leading system message.
///
/// This is the ONLY public entry point that produces a [`TranscriptContext`]; every provider-facing
/// function in this module expects the result.
pub fn normalize_context(context: &Context) -> TranscriptContext {
    let initial = create_initial_system_message(context.system_prompt.as_deref(), &context.tools);
    let messages = match initial {
        Some(head) => {
            let mut out = Vec::with_capacity(context.messages.len() + 1);
            out.push(Message::System(head));
            out.extend(context.messages.iter().cloned());
            out
        }
        None => context.messages.clone(),
    };
    TranscriptContext { messages }
}

/// `isSystemMessage` (`transcript.ts:46-48`) as a narrowing borrow: `Some` exactly when the message
/// is a [`Message::System`].
pub fn as_system_message(message: &Message) -> Option<&SystemMessage> {
    match message {
        Message::System(m) => Some(m),
        _ => None,
    }
}

/// `getInitialSystemMessage` (`transcript.ts:51-54`): the leading system message, if the transcript
/// starts with one.
pub fn get_initial_system_message(messages: &[Message]) -> Option<&SystemMessage> {
    messages.first().and_then(as_system_message)
}

/// `withoutInitialSystemMessage` (`transcript.ts:57-59`): drop the leading system message for APIs
/// that carry the prompt OUTSIDE the message list.
pub fn without_initial_system_message(messages: &[Message]) -> &[Message] {
    match get_initial_system_message(messages) {
        Some(_) => messages.get(1..).unwrap_or(&[]),
        None => messages,
    }
}

/// `getCurrentTools` (`transcript.ts:62-70`): the tools available after applying every transcript
/// delta in order.
///
/// Upstream's accumulator is a JS `Map`, so REMOVE-then-ADD runs per message and the result is in
/// INSERTION order, with a re-added name keeping its original position. [`OrderedTools`] below is
/// that `Map`.
pub fn get_current_tools(messages: &[Message]) -> Vec<ToolDef> {
    let mut tools = OrderedTools::default();
    for message in messages {
        let Some(system) = as_system_message(message) else {
            continue;
        };
        for tool in &system.tools_removed {
            tools.remove(&tool.name);
        }
        for tool in &system.tools_added {
            tools.set(tool.clone());
        }
    }
    tools.into_values()
}

/// `getCurrentSystemMessage` (`transcript.ts:77-101`): replay every system message into ONE leading
/// system message holding the current prompt and tools.
///
/// Later `content` is APPENDED to the base prompt (joined `"\n\n"`), `sections` are patched by name
/// with a `null` deleting, the FIRST `timestamp` wins (`timestamp ??= message.timestamp`, :81), and
/// tools resolve through [`get_current_tools`]. Returns `None` when there was no system message at
/// all AND no tool survived — upstream's `timestamp === undefined && tools.length === 0` (:94).
pub fn get_current_system_message(messages: &[Message]) -> Option<SystemMessage> {
    let mut content: Vec<String> = Vec::new();
    let mut sections = Sections::new();
    let mut timestamp: Option<i64> = None;
    for message in messages {
        let Some(system) = as_system_message(message) else {
            continue;
        };
        timestamp = timestamp.or(Some(system.timestamp));
        let text = content_text_default(&system.content);
        if !text.is_empty() {
            content.push(text);
        }
        if let Some(patch) = &system.sections {
            for (name, value) in patch.iter() {
                match value {
                    None => {
                        sections.remove(name);
                    }
                    Some(v) => sections.set(name, Some(v.to_owned())),
                }
            }
        }
    }
    let tools = get_current_tools(messages);
    if timestamp.is_none() && tools.is_empty() {
        return None;
    }
    let joined = content.join("\n\n");
    Some(SystemMessage {
        content: if joined.is_empty() {
            Vec::new()
        } else {
            vec![Content::text(joined)]
        },
        // `...(sections.size > 0 ? { sections: ... } : {})` (:93).
        sections: if sections.is_empty() {
            None
        } else {
            Some(sections)
        },
        // `...(tools.length > 0 ? { toolsAdded: tools } : {})` (:98) — the empty vec IS absent.
        tools_added: tools,
        tools_removed: Vec::new(),
        timestamp: timestamp.unwrap_or(0),
    })
}

/// `getCurrentSystemPrompt` (`transcript.ts:104-107`): the current system prompt TEXT after
/// replaying every system message.
pub fn get_current_system_prompt(messages: &[Message]) -> String {
    get_current_system_message(messages)
        .as_ref()
        .map(get_system_message_text)
        .unwrap_or_default()
}

/// `collapseSystemMessages` (`transcript.ts:114-118`): rebuild the transcript for APIs WITHOUT
/// mid-conversation system messages — the replayed system message leads, and every later system
/// message is dropped.
pub fn collapse_system_messages(context: &TranscriptContext) -> TranscriptContext {
    let head = get_current_system_message(&context.messages);
    let mut messages: Vec<Message> = Vec::with_capacity(context.messages.len());
    if let Some(head) = head {
        messages.push(Message::System(head));
    }
    messages.extend(
        context
            .messages
            .iter()
            .filter(|m| as_system_message(m).is_none())
            .cloned(),
    );
    TranscriptContext { messages }
}

/// `resolveTranscript` (`transcript.ts:121-126`): keep later system messages IN PLACE when the model
/// accepts them; otherwise collapse them.
pub fn resolve_transcript(
    context: &TranscriptContext,
    supports_mid_convo_system_messages: Option<bool>,
) -> TranscriptContext {
    if supports_mid_convo_system_messages.unwrap_or(false) {
        context.clone()
    } else {
        collapse_system_messages(context)
    }
}

/// `toToolDeclaration` (`transcript.ts:129-137`): strip executable and display-only fields from a
/// tool before transcript comparison or persistence.
///
/// Upstream's body is a four-key object literal whose `parameters` goes through
/// `JSON.parse(JSON.stringify(...))`. Both halves of that are no-ops for a [`ToolDef`]: it has
/// exactly those four fields and nothing executable or display-only to drop, and its `parameters` is
/// already a `serde_json::Value` — there are no typebox symbol keys and no `undefined` fields for a
/// JSON round-trip to strip. The function exists because it is part of the module's surface and
/// because [`get_tool_state_changes`] maps it over its output, exactly as upstream does.
pub fn to_tool_declaration(tool: &ToolDef) -> ToolDef {
    tool.clone()
}

/// `declarationsEqual` (`transcript.ts:148-150`): whether two tools declare the SAME interface to
/// the model.
///
/// Upstream compares `JSON.stringify(toToolDeclaration(left))` against the same for `right`,
/// because a string comparison of two literals built in the same key order is exact and avoids a
/// deep-equal dependency in a browser-safe package (`:141-147`). In Rust the structural comparison
/// IS that comparison: `ToolDef`'s four fields are compared in order, and `parameters` is a
/// `serde_json::Value` whose object representation is a `BTreeMap` — key order is not representable,
/// so two schemas cannot serialize to different strings while comparing structurally equal. The
/// canonical-serialization step upstream needs to reach exactness is already cyrup's data model.
pub fn declarations_equal(left: &ToolDef, right: &ToolDef) -> bool {
    to_tool_declaration(left) == to_tool_declaration(right)
}

/// Pi `ToolStateChanges` (`transcript.ts:152-155`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolStateChanges {
    pub tools_added: Vec<ToolDef>,
    pub tools_removed: Vec<ToolReference>,
}

/// `getToolStateChanges` (`transcript.ts:158-175`): compare two COMPLETE tool states. A changed
/// definition is a removal followed by an addition — it appears in both lists.
pub fn get_tool_state_changes(previous: &[ToolDef], current: &[ToolDef]) -> ToolStateChanges {
    let find = |list: &[ToolDef], name: &str| -> Option<ToolDef> {
        list.iter().find(|t| t.name == name).cloned()
    };
    ToolStateChanges {
        tools_added: current
            .iter()
            .filter(|tool| match find(previous, &tool.name) {
                None => true,
                Some(previous_tool) => !declarations_equal(&previous_tool, tool),
            })
            .map(to_tool_declaration)
            .collect(),
        tools_removed: previous
            .iter()
            .filter(|tool| match find(current, &tool.name) {
                None => true,
                Some(current_tool) => !declarations_equal(tool, &current_tool),
            })
            .map(ToolReference::from)
            .collect(),
    }
}

/// `getDeclaredTools` (`transcript.ts:178-186`): every definition referenced by transcript tool
/// state, in FIRST-DECLARATION order.
///
/// Unlike [`get_current_tools`] this ignores removals, and a re-declaration REPLACES the definition
/// while keeping the first position (upstream's `Map.set`).
pub fn get_declared_tools(messages: &[Message]) -> Vec<ToolDef> {
    let mut definitions = OrderedTools::default();
    for message in messages {
        let Some(system) = as_system_message(message) else {
            continue;
        };
        for tool in &system.tools_added {
            definitions.set(tool.clone());
        }
    }
    definitions.into_values()
}

/// `hasToolRedefinitions` (`transcript.ts:192-203`): whether a tool NAME was declared twice with
/// DIFFERENT definitions. Transports that reference tools by name (Anthropic
/// `tool_addition`/`tool_removal`) cannot express that.
pub fn has_tool_redefinitions(messages: &[Message]) -> bool {
    let mut declared: Vec<ToolDef> = Vec::new();
    for message in messages {
        let Some(system) = as_system_message(message) else {
            continue;
        };
        for tool in &system.tools_added {
            match declared.iter_mut().find(|t| t.name == tool.name) {
                Some(slot) => {
                    if !declarations_equal(slot, tool) {
                        return true;
                    }
                    *slot = tool.clone();
                }
                None => declared.push(tool.clone()),
            }
        }
    }
    false
}

/// `hasNonAdditiveToolChanges` (`transcript.ts:206-217`): whether tool history contains a REMOVAL or
/// a same-name REDECLARATION that an addition-only transport cannot replay.
///
/// Note the asymmetry with [`has_tool_redefinitions`]: this one trips on ANY repeated name, even one
/// re-declared identically, because an addition-only transport would send the declaration twice.
pub fn has_non_additive_tool_changes(messages: &[Message]) -> bool {
    let mut declared: Vec<&str> = Vec::new();
    for message in messages {
        let Some(system) = as_system_message(message) else {
            continue;
        };
        if !system.tools_removed.is_empty() {
            return true;
        }
        for tool in &system.tools_added {
            if declared.contains(&tool.name.as_str()) {
                return true;
            }
            declared.push(&tool.name);
        }
    }
    false
}

/// Pi `TranscriptTools` (`transcript.ts:219-228`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TranscriptTools {
    /// Tools sent in the TOP-LEVEL request field.
    pub request_tools: Vec<ToolDef>,
    /// Whether later system messages carry their own `tools_added` as IN-PLACE additions. When
    /// `false`, `request_tools` already holds the complete current tool set.
    pub anchors_additions: bool,
}

/// `resolveTranscriptTools` (`transcript.ts:236-244`): split tool declarations between the top-level
/// request field and in-place additions.
///
/// A transport that can anchor additions at a system message keeps the INITIAL tools at the top and
/// loads later ones where they appear; that only works when no tool was removed or re-declared, so
/// everything else sends the current tool list.
pub fn resolve_transcript_tools(
    messages: &[Message],
    supports_tool_additions: bool,
) -> TranscriptTools {
    let anchors_additions = supports_tool_additions && !has_non_additive_tool_changes(messages);
    TranscriptTools {
        request_tools: if anchors_additions {
            get_initial_system_message(messages)
                .map(|system| system.tools_added.clone())
                .unwrap_or_default()
        } else {
            get_current_tools(messages)
        },
        anchors_additions,
    }
}

/// The insertion-ordered `Map<string, Tool>` upstream's replay helpers accumulate into
/// (`transcript.ts:63`, `:180`, `:194`).
///
/// A `HashMap` would make the resolved tool ORDER — which reaches the wire as the request's `tools`
/// array — nondeterministic, and a `BTreeMap` would silently alphabetize it. Neither matches
/// `Map.values()`, so the ordered container is spelled out: `set` keeps an existing name in its
/// original POSITION and replaces its definition, a new name appends, and `remove` drops it.
#[derive(Default)]
struct OrderedTools(Vec<ToolDef>);

impl OrderedTools {
    fn set(&mut self, tool: ToolDef) {
        match self.0.iter_mut().find(|t| t.name == tool.name) {
            Some(slot) => *slot = tool,
            None => self.0.push(tool),
        }
    }

    fn remove(&mut self, name: &str) {
        if let Some(i) = self.0.iter().position(|t| t.name == name) {
            self.0.remove(i);
        }
    }

    fn into_values(self) -> Vec<ToolDef> {
        self.0
    }
}
