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
        // The run's options when its `before_agent_start` handlers set some, the base otherwise
        // (pi diffs `buildSystemPromptSections(result.systemPromptOptions)` at run start and the
        // refreshed run options at every turn boundary, `agent-session.ts:2105`, `:915` @v1.1.0;
        // EXT-084). A forced prompt changes no section: the transcript keeps the structured ones.
        let run = Self::lock(&self.run_prompt_options).clone();
        let current = match run {
            Some(options) => {
                let docs = Self::lock(&self.base_prompt).docs().clone();
                crate::tools::BuiltPrompt::from_options(options, docs)
                    .sections()
                    .clone()
            }
            None => Self::lock(&self.base_prompt).sections().clone(),
        };
        let patch = diff_system_prompt_sections(&previous, &current)?;
        Some(SystemMessage {
            sections: Some(patch),
            timestamp: super::now_ms(),
            ..SystemMessage::default()
        })
    }
}

impl AgentSession {
    /// pi `_preparePromptAndToolLoadout`'s two stamps (`core/agent-session.ts:1742-1744` @v1.1.0):
    /// the options list the tools the loadout now has, and hide the declarations requests leave
    /// out, so the prompt describes exactly the tools the request carries.
    pub(crate) fn stamp_live_loadout(
        &self,
        options: &mut cyrup_session::prompt::SystemPromptOptions,
    ) {
        let dynamic = Self::lock(&self.dynamic_tools);
        options.selected_tools = dynamic.active_names();
        options.hidden_tools = dynamic.hidden_names();
    }

    /// The turn-boundary refresh of the run's options (pi `_installAgentNextTurnRefresh`,
    /// `core/agent-session.ts:908-917` @v1.1.0; EXT-084): the run's options — or the base's,
    /// when the run has none — with the live active tools, and every tool's snippet and
    /// guidelines (the base's, then the run's over them, so a tool registered mid-run is
    /// described). pi stores the result as the run's options "to keep session.systemPrompt and
    /// ctx.getSystemPrompt() in step with what the provider sees".
    pub(crate) fn refresh_run_prompt_options(&self) {
        let base = Self::lock(&self.base_prompt).options().clone();
        let run = Self::lock(&self.run_prompt_options).clone();
        let mut options = run.unwrap_or_else(|| base.clone());
        let mut snippets = base.tool_snippets;
        snippets.extend(std::mem::take(&mut options.tool_snippets));
        options.tool_snippets = snippets;
        let mut guidelines = base.tool_guidelines;
        guidelines.extend(std::mem::take(&mut options.tool_guidelines));
        options.tool_guidelines = guidelines;
        self.stamp_live_loadout(&mut options);
        *Self::lock(&self.run_prompt_options) = Some(options);
        self.sync_prompt_mirror();
    }

    /// Keep the prompt `ctx.getSystemPrompt()` reads equal to [`Self::effective_system_prompt`]
    /// (pi binds it to `this.systemPrompt`, `buildSystemPrompt(this._runSystemPromptOptions ??
    /// this._baseSystemPromptOptions)`, `core/agent-session.ts:1470-1472`, `:3465` @v1.1.0).
    pub(crate) fn sync_prompt_mirror(&self) {
        self.services.host_services.update_prompt_state(
            Some(self.effective_system_prompt()),
            self.services.settings.project_trusted(),
        );
    }
}
