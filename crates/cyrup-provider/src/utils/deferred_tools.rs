//! Split the active tool list into an *immediate* prefix and *transcript-loaded* definitions
//! (1:1 port of Pi `packages/ai/src/utils/deferred-tools.ts:8-38` `splitDeferredTools`).
//!
//! DRIFT-001, message-anchored tool loading. A [`Message::ToolResult`] carries
//! `addedToolNames`: the tools that became available *because of* that result. Providers with
//! native deferred loading do not send those definitions in the request prefix — they send them
//! marked `defer_loading` and *anchor* them at the transcript position where they appeared, so
//! the prompt prefix stays cache-stable across the turn that introduced them.
//!
//! This module owns only the PARTITION. The two renderings live in their apis:
//! `anthropic-messages` emits `tool_reference` blocks, `openai-responses` emits a synthetic
//! `tool_search_call`/`tool_search_output` pair. Two rules that look like they belong here do
//! NOT (verified against Pi):
//!
//! - **No safety valve.** Pi's "if every tool is deferred, promote them all back" rule lives in
//!   the Anthropic caller only (`anthropic-messages.ts:955-959`). `openai-responses.ts:301`
//!   deliberately omits it and ships a body with no `tools` key at all, so hoisting the valve in
//!   here would silently break Responses parity.
//! - **Dedupe is NOT gated by `enabled`.** Pi collapses the tool list by normalized name before
//!   the early return (`deferred-tools.ts:13-15`), so a disabled model still gets a deduped list.
//!   With the identity normalizer that is a no-op on any well-formed tool list; it only bites
//!   under the Anthropic OAuth normalizer (`toClaudeCodeName`), which merges case-variants.

use crate::context::ToolDef;
use cyrup_core::{Content, Message};
use std::collections::{HashMap, HashSet};

/// The result of [`split_deferred_tools`] (Pi `{ immediate: Tool[]; deferred: Map<string, Tool> }`).
///
/// `deferred` is a `Vec` of `(normalized_name, tool)` rather than a `HashMap` because Pi's `Map`
/// is **insertion-ordered** and its iteration order reaches the wire: `[...deferred.values()]`
/// becomes the tail of `params.tools`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolPlacement {
    /// Tools sent in the request prefix, in first-declaration order.
    pub immediate: Vec<ToolDef>,
    /// Tools anchored at their `addedToolNames` marker, keyed by NORMALIZED name.
    pub deferred: Vec<(String, ToolDef)>,
}

impl ToolPlacement {
    /// The deferred tool definitions in insertion order (Pi `[...toolPlacement.deferred.values()]`).
    ///
    /// The matching NAME set is deliberately not offered here: the Anthropic caller must build it
    /// AFTER its safety valve has run, from whatever is still deferred at that point.
    pub fn deferred_tools(&self) -> Vec<ToolDef> {
        self.deferred.iter().map(|(_, t)| t.clone()).collect()
    }
}

/// Split `tools` into prefix and transcript-loaded definitions (Pi `splitDeferredTools`,
/// deferred-tools.ts:8-38).
///
/// Pi takes the whole `Context`; cyrup takes `messages` and `tools` separately because the
/// Anthropic caller must split over the **transformed** message list
/// (`anthropic-messages.ts:949-953` passes `{ ...context, messages: transformedMessages }`) while
/// `Context.messages` stays untouched, and the Responses caller passes the raw list
/// (`openai-responses.ts:267`).
///
/// Algorithm, in order:
/// 1. Collapse `tools` into an insertion-ordered map keyed by `normalize_name(tool.name)` —
///    **last value wins, first position is kept**, exactly like `Map.prototype.set`.
/// 2. If `!enabled`, return every unique tool as immediate. (Step 1 still applied.)
/// 3. Walk `messages` IN ORDER: an assistant `toolCall` block marks its name *used*; a tool
///    result's `addedToolNames` entry is deferred only if that name has not been used **at or
///    before** that point. A tool the model already called cannot be hidden from the prefix.
/// 4. Partition the unique map by that name set.
pub fn split_deferred_tools(
    messages: &[Message],
    tools: &[ToolDef],
    enabled: bool,
    normalize_name: &dyn Fn(&str) -> String,
) -> ToolPlacement {
    // (1) `const uniqueTools = new Map<string, Tool>()` — insertion-ordered, last value wins.
    let mut unique: Vec<(String, ToolDef)> = Vec::with_capacity(tools.len());
    let mut index: HashMap<String, usize> = HashMap::with_capacity(tools.len());
    for tool in tools {
        let key = normalize_name(&tool.name);
        match index.get(&key) {
            Some(&at) => {
                if let Some(slot) = unique.get_mut(at) {
                    slot.1 = tool.clone();
                }
            }
            None => {
                index.insert(key.clone(), unique.len());
                unique.push((key, tool.clone()));
            }
        }
    }

    // (2) `if (!enabled) return { immediate: [...uniqueTools.values()], deferred: new Map() }`.
    if !enabled {
        return ToolPlacement {
            immediate: unique.into_iter().map(|(_, t)| t).collect(),
            deferred: Vec::new(),
        };
    }

    // (3) In-order accumulation of used vs. deferred names.
    let mut deferred_names: HashSet<String> = HashSet::new();
    let mut used_names: HashSet<String> = HashSet::new();
    for message in messages {
        match message {
            Message::Assistant(am) => {
                for block in &am.content {
                    if let Content::ToolCall(tc) = block {
                        used_names.insert(normalize_name(&tc.name));
                    }
                }
            }
            Message::ToolResult {
                added_tool_names, ..
            } => {
                for name in added_tool_names {
                    let normalized = normalize_name(name);
                    if !used_names.contains(&normalized) {
                        deferred_names.insert(normalized);
                    }
                }
            }
            Message::User { .. } => {}
        }
    }

    // (4) Partition, preserving the unique map's insertion order on both sides.
    let mut immediate: Vec<ToolDef> = Vec::new();
    let mut deferred: Vec<(String, ToolDef)> = Vec::new();
    for (name, tool) in unique {
        if deferred_names.contains(&name) {
            deferred.push((name, tool));
        } else {
            immediate.push(tool);
        }
    }
    ToolPlacement {
        immediate,
        deferred,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use cyrup_core::{AssistantMessage, StopReason, ToolCall, ToolCallId, Usage};
    use serde_json::json;

    fn tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: format!("The {name} tool"),
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
        }
    }

    fn tool_with(name: &str, description: &str) -> ToolDef {
        ToolDef {
            name: name.to_string(),
            description: description.to_string(),
            parameters: json!({ "type": "object", "properties": {}, "required": [] }),
        }
    }

    fn assistant_call(name: &str) -> Message {
        Message::Assistant(AssistantMessage {
            content: vec![Content::ToolCall(ToolCall {
                id: ToolCallId::from("call_1"),
                name: name.to_string(),
                arguments: serde_json::Map::new(),
                thought_signature: None,
            })],
            provider: "anthropic".into(),
            model: "claude-opus-4-6".to_string(),
            api: "anthropic-messages".into(),
            response_model: None,
            response_id: None,
            diagnostics: None,
            usage: Usage::default(),
            stop_reason: StopReason::ToolUse,
            deferred: None,
            error_message: None,
            raw_stop_reason: None,
            timestamp: 2,
        })
    }

    fn tool_result(added: &[&str]) -> Message {
        Message::ToolResult {
            tool_call_id: ToolCallId::from("call_1"),
            tool_name: "base_tool".to_string(),
            content: vec![Content::text("done")],
            is_error: false,
            details: None,
            usage: None,
            added_tool_names: added.iter().map(|s| s.to_string()).collect(),
            timestamp: 3,
        }
    }

    fn identity(name: &str) -> String {
        name.to_string()
    }

    #[test]
    fn defers_a_tool_introduced_by_its_marker() {
        let messages = vec![assistant_call("base_tool"), tool_result(&["late_tool"])];
        let tools = vec![tool("base_tool"), tool("late_tool")];
        let placement = split_deferred_tools(&messages, &tools, true, &identity);
        assert_eq!(
            placement
                .immediate
                .iter()
                .map(|t| t.name.as_str())
                .collect::<Vec<_>>(),
            ["base_tool"]
        );
        assert_eq!(
            placement
                .deferred
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>(),
            ["late_tool"]
        );
    }

    #[test]
    fn keeps_a_tool_immediate_when_used_before_its_marker() {
        // Pi: "keeps a tool immediate when it was used before its marker".
        let messages = vec![assistant_call("late_tool"), tool_result(&["late_tool"])];
        let tools = vec![tool("base_tool"), tool("late_tool")];
        let placement = split_deferred_tools(&messages, &tools, true, &identity);
        assert!(placement.deferred.is_empty());
        assert_eq!(placement.immediate.len(), 2);
    }

    #[test]
    fn a_later_use_does_not_un_defer_an_earlier_marker() {
        // The suppression is positional: "used AT OR BEFORE that point". A call that comes
        // AFTER the marker leaves the tool deferred.
        let messages = vec![tool_result(&["late_tool"]), assistant_call("late_tool")];
        let tools = vec![tool("base_tool"), tool("late_tool")];
        let placement = split_deferred_tools(&messages, &tools, true, &identity);
        assert_eq!(placement.deferred.len(), 1);
    }

    #[test]
    fn does_not_resurrect_a_marked_tool_missing_from_tools() {
        let messages = vec![assistant_call("base_tool"), tool_result(&["late_tool"])];
        let tools = vec![tool("base_tool")];
        let placement = split_deferred_tools(&messages, &tools, true, &identity);
        assert_eq!(placement.immediate.len(), 1);
        assert!(placement.deferred.is_empty());
    }

    #[test]
    fn disabled_returns_everything_immediate_but_still_dedupes() {
        // Pi deferred-tools.ts:13-15 — the unique-map collapse happens BEFORE the `!enabled`
        // early return, so dedupe is NOT gated by the flag.
        let messages = vec![assistant_call("base_tool"), tool_result(&["late_tool"])];
        let tools = vec![
            tool("read"),
            tool_with("Read", "Canonical definition"),
            tool("late_tool"),
        ];
        let placement = split_deferred_tools(&messages, &tools, false, &|n| n.to_lowercase());
        assert!(placement.deferred.is_empty());
        assert_eq!(
            placement
                .immediate
                .iter()
                .map(|t| (t.name.as_str(), t.description.as_str()))
                .collect::<Vec<_>>(),
            [
                ("Read", "Canonical definition"),
                ("late_tool", "The late_tool tool")
            ]
        );
    }

    #[test]
    fn dedupe_keeps_first_position_and_last_value() {
        // JS `Map.set` semantics, and they are observable on the wire because the deferred map's
        // iteration order becomes the tail of `params.tools`.
        let tools = vec![
            tool("read"),
            tool("zzz"),
            tool_with("Read", "Canonical definition"),
        ];
        let placement = split_deferred_tools(&[], &tools, true, &|n| n.to_lowercase());
        assert_eq!(
            placement
                .immediate
                .iter()
                .map(|t| (t.name.as_str(), t.description.as_str()))
                .collect::<Vec<_>>(),
            [("Read", "Canonical definition"), ("zzz", "The zzz tool")]
        );
    }

    #[test]
    fn normalizes_names_on_both_sides_of_the_marker_check() {
        // Pi "normalizes OAuth names before checking prior tool usage": the call is `Read`, the
        // marker is `read`; under the canonicalizing normalizer they are the same tool.
        let messages = vec![assistant_call("Read"), tool_result(&["read"])];
        let tools = vec![tool("base_tool"), tool("read")];
        let placement = split_deferred_tools(&messages, &tools, true, &|n| n.to_lowercase());
        assert!(placement.deferred.is_empty());
    }

    #[test]
    fn no_safety_valve_in_the_splitter() {
        // The "everything deferred" case is left ALONE here; only the Anthropic caller promotes.
        let messages = vec![assistant_call("base_tool"), tool_result(&["late_tool"])];
        let tools = vec![tool("late_tool")];
        let placement = split_deferred_tools(&messages, &tools, true, &identity);
        assert!(placement.immediate.is_empty());
        assert_eq!(placement.deferred.len(), 1);
    }
}
