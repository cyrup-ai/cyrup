//! CODE-014 — keep the transcript's prompt equal to the session's.
//!
//! pi never sends a prompt of its own. The prompt is the `sections` of the system messages in the
//! transcript, and before each run and each turn the session compares what the transcript replays
//! with what the prompt should be now, and appends one system message holding the difference
//! (`_preparePromptAndToolLoadout`, `core/agent-session.ts:1689-1705` @v1.0.0). A session's first
//! prompt writes the whole prompt, because the transcript replays nothing yet; a prompt that did not
//! change writes nothing; a prompt that changed writes only the sections that changed.
//!
//! The two places pi makes that comparison are the two places this one is made:
//! [`AgentSession::assemble_run_messages`] before a run's first request (`agent-session.ts:2058`)
//! and `PolicyHooks::prepare_next_turn` before every later one (`:887`).

use cyrup_agent::AgentMessage;
use cyrup_core::{Sections, SystemMessage};
use cyrup_session::prompt::diff_system_prompt_sections;

use super::AgentSession;

/// The prompt sections a transcript replays: every system message's `sections`, applied in order,
/// a `null` deleting the section (pi `getCurrentSystemMessage(messages)?.sections ?? {}`,
/// `packages/ai/src/utils/transcript.ts:77-101` @v1.0.0). A section keeps the position it first had.
pub(crate) fn replayed_sections<'a>(
    transcript: impl IntoIterator<Item = &'a AgentMessage>,
) -> Sections {
    let mut replayed = Sections::new();
    for message in transcript {
        let AgentMessage::System(system) = message else {
            continue;
        };
        let Some(patch) = &system.sections else {
            continue;
        };
        for (name, value) in patch.iter() {
            match value {
                Some(text) => replayed.set(name, Some(text.to_owned())),
                None => {
                    replayed.remove(name);
                }
            }
        }
    }
    replayed
}

impl AgentSession {
    /// The system message that brings the transcript's prompt up to date, or `None` when it already
    /// is (pi `_preparePromptAndToolLoadout`'s return, `agent-session.ts:1702`).
    ///
    /// It carries `sections` and nothing else: the tool declarations ride in on the same message
    /// when the loop reconciles them, which is pi's `declareToolChanges` over a pending system
    /// message (`agent-loop.ts:333`).
    ///
    /// This changes what the model is sent, not only what the file holds: the model reads the
    /// replay of these rows and no prompt besides.
    pub(crate) fn prompt_update<'a>(
        &self,
        transcript: impl IntoIterator<Item = &'a AgentMessage>,
    ) -> Option<SystemMessage> {
        let previous = replayed_sections(transcript);
        let current = Self::lock(&self.base_prompt).sections().clone();
        let patch = diff_system_prompt_sections(&previous, &current)?;
        Some(SystemMessage {
            sections: Some(patch),
            timestamp: super::now_ms(),
            ..SystemMessage::default()
        })
    }
}
