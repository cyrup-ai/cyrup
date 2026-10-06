//! Declaring tool loadout changes to the model — the functional core of pi's `declareToolChanges`
//! (`packages/agent/src/agent-loop.ts:327-376` @v1.0.1) and the request projection that goes with
//! it.
//!
//! `context.tools` is what the runtime can execute; the transcript's system messages declare what
//! the model may call. Before each request the difference between the two becomes `toolsAdded` and
//! `toolsRemoved` on a system message. When a pending system message exists, its tool fields are
//! treated as intent and replaced with the delta between the committed transcript and the
//! executable set, so replaying the transcript always yields exactly the executable set. Otherwise
//! a new system message is inserted before the first non-system pending message.
//!
//! The declarations are PERSISTED, because the system message is emitted as an ordinary message
//! (`message_start`/`message_end`) and the session writes it to the transcript. That is the whole
//! mechanism by which a loadout survives a resume, a `/tree` navigation and a compaction.

use crate::event::AgentMessage;
use cyrup_core::{Message, SystemMessage, ToolDef};
use cyrup_provider::utils::transcript::ToolStateChanges;
use cyrup_provider::{get_current_tools, get_tool_state_changes};
use std::sync::Arc;

/// Pi `declareToolChanges` (`agent-loop.ts:333-363` @v1.0.1): the `pending` messages with the tool
/// declarations the model is missing.
///
/// `committed` is the loop's working transcript, `declared` the executable set's declarations
/// (hidden declarations included: they are recorded and projected out of requests, never left
/// undeclared). `timestamp` stamps a system message this function has to create.
pub(crate) fn declare_tool_changes(
    committed: &[Arc<AgentMessage>],
    declared: &[ToolDef],
    pending: Vec<AgentMessage>,
    timestamp: i64,
) -> Vec<AgentMessage> {
    // `pendingMessages[systemIndex]`: the LAST pending system message, if any (`:335-340`).
    let system_index = pending
        .iter()
        .rposition(|m| matches!(m, AgentMessage::System(_)));
    let pending_system = system_index.and_then(|i| match pending.get(i) {
        Some(AgentMessage::System(s)) => Some(s.clone()),
        _ => None,
    });

    // The baseline: the pending messages with that message's own tool fields treated as intent
    // and set aside (`withToolChanges(pending, NO_CHANGES)`, `:343-346`).
    let baseline: Vec<AgentMessage> = pending
        .iter()
        .enumerate()
        .map(
            |(index, message)| match (message, pending_system.as_ref()) {
                (AgentMessage::System(_), Some(system)) if Some(index) == system_index => {
                    AgentMessage::System(with_tool_changes(system, ToolStateChanges::default()))
                }
                _ => message.clone(),
            },
        )
        .collect();

    let replayed: Vec<Message> = committed
        .iter()
        .map(|m| m.as_ref())
        .chain(baseline.iter())
        .filter_map(|m| match m {
            AgentMessage::System(s) => Some(Message::System(s.clone())),
            _ => None,
        })
        .collect();
    let changes = get_tool_state_changes(&get_current_tools(&replayed), declared);
    let unchanged = changes.tools_added.is_empty() && changes.tools_removed.is_empty();

    if let (Some(index), Some(system)) = (system_index, pending_system) {
        // Keep the caller's message object when it already declares no tool changes (`:354-355`).
        if unchanged && system.tools_added.is_empty() && system.tools_removed.is_empty() {
            return pending;
        }
        return baseline
            .into_iter()
            .enumerate()
            .map(|(i, message)| {
                if i == index {
                    AgentMessage::System(with_tool_changes(&system, changes.clone()))
                } else {
                    message
                }
            })
            .collect();
    }
    if unchanged {
        return pending;
    }
    let update = with_tool_changes(
        &SystemMessage {
            timestamp,
            ..SystemMessage::default()
        },
        changes,
    );
    // Before the first non-system pending message, or at the end when there is none (`:359-362`).
    let insert_at = pending
        .iter()
        .position(|m| !matches!(m, AgentMessage::System(_)))
        .unwrap_or(pending.len());
    let mut out = pending;
    out.insert(insert_at, AgentMessage::System(update));
    out
}

/// Pi `withToolChanges` (`agent-loop.ts:368-375` @v1.0.1): `message` with its tool fields replaced
/// by `changes`; empty lists omit the field.
fn with_tool_changes(message: &SystemMessage, changes: ToolStateChanges) -> SystemMessage {
    SystemMessage {
        tools_added: changes.tools_added,
        tools_removed: changes.tools_removed,
        ..message.clone()
    }
}

/// The request's view of a transcript: system messages without their tool declarations.
///
/// pi derives a request's tools from the transcript replay, minus the declarations a
/// `prepareLoadout` hook hides (`_installHiddenDeclarationsProjection`, `agent-session.ts:1721-1738`
/// @v1.0.1). cyrup's request carries those tools as `Context::tools` — the loadout's ADVERTISED set,
/// which is that same replay minus the same hidden names, by construction of
/// [`declare_tool_changes`] — so the declarations inside the transcript are not repeated in the
/// message list: a provider that replays them would declare every tool twice, and a hidden one
/// once. A system message left with nothing in it is dropped (pi keeps it as an empty message,
/// which no provider renders as anything).
pub(crate) fn without_tool_declarations(messages: Vec<Message>) -> Vec<Message> {
    messages
        .into_iter()
        .filter_map(|message| match message {
            Message::System(mut system) => {
                system.tools_added.clear();
                system.tools_removed.clear();
                let empty = system.content.is_empty()
                    && system.sections.as_ref().is_none_or(|s| s.is_empty());
                (!empty).then_some(Message::System(system))
            }
            other => Some(other),
        })
        .collect()
}

#[cfg(test)]
#[path = "declare_tests.rs"]
mod tests;
