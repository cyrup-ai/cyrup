//! `SessionCommand` — the verb surface every adapter shares (arch-11 §2.1, the `command.rs`
//! module). The central invariant of the subsystem is that no mode reaches behaviour that does not
//! flow through `SessionCommand`/[`AgentSession`]; this enum makes that seam explicit so the
//! RPC/print/json adapters route through one place instead of calling facade methods ad hoc.
//!
//! Each verb maps 1:1 onto an [`AgentSession`] method; [`AgentSession::execute`] dispatches them and
//! returns a [`SessionCommandOutput`]. Streaming verbs (`prompt`) are intentionally NOT here — they
//! return an [`crate::EventStream`] and are driven directly; this surface is the request/response
//! control plane (Pi `rpc-mode.ts` command table).

use std::path::PathBuf;

use cyrup_core::EntryId;

use crate::error::SessionServiceError;
use crate::event::{PromptAccepted, UserInput};
use crate::session::{
    AgentSession, ForkAnchor, ForkOutcome, ForkPosition, NavigateTreeOptions, NavigateTreeOutcome,
};
use crate::state::{CompactionResult, ContextUsage, SessionStateView, SessionStats};

/// The request/response control verbs the seam exposes (arch-11 §2.1).
#[derive(Clone, Debug)]
pub enum SessionCommand {
    /// Submit a prompt, resolving to the preflight acceptance (the run is observed via `subscribe`).
    Prompt(UserInput),
    Steer(UserInput),
    FollowUp(UserInput),
    Abort,
    ClearQueue,
    Compact { instructions: Option<String> },
    AbortCompaction,
    AbortBranchSummary,
    SetModel { pattern: String },
    CycleModel { forward: bool },
    SetAutoRetry { enabled: bool },
    AbortRetry,
    SetAutoCompaction { enabled: bool },
    Bash { command: String, exclude_from_context: bool },
    AbortBash,
    GetActiveTools,
    GetAllTools,
    SetActiveTools { names: Vec<String> },
    SetThinkingLevel { level: cyrup_core::ModelThinkingLevel },
    CycleThinkingLevel,
    SetSteeringMode { mode: cyrup_agent::QueueMode },
    SetFollowUpMode { mode: cyrup_agent::QueueMode },
    Branch { entry: EntryId },
    /// Unified `/tree` navigation (`navigateTree`): navigate to `target`, optionally summarizing the
    /// abandoned branch, returning the re-editable text + appended summary entry.
    NavigateTree { target: EntryId, options: NavigateTreeOptions },
    Fork { entry: EntryId, position: ForkPosition },
    /// Clone the session at an entry (or current leaf) into a new file without switching (`clone_at`).
    CloneAt { entry: Option<EntryId> },
    SetSessionName { name: String },
    GetState,
    GetSessionStats,
    GetContextUsage,
    GetForkMessages,
    ExportJsonl { path: Option<PathBuf> },
    GetLastAssistantText,
}

/// The response of a [`SessionCommand`] (the non-streaming control plane).
#[derive(Clone, Debug)]
pub enum SessionCommandOutput {
    Accepted(PromptAccepted),
    Unit,
    Compacted(Option<CompactionResult>),
    /// The new thinking level after a set/cycle (`None` when the model does not support thinking).
    ThinkingLevel(Option<cyrup_core::ModelThinkingLevel>),
    /// The new model id after a cycle (`None` when there was only one candidate).
    Model(Option<String>),
    State(Box<SessionStateView>),
    Stats(SessionStats),
    ContextUsage(ContextUsage),
    ForkMessages(Vec<ForkAnchor>),
    Fork(ForkOutcome),
    Text(Option<String>),
    /// The result of an immediate bash execution.
    Bash(crate::BashResult),
    /// Active tool names (`get_active_tools`).
    ToolNames(Vec<String>),
    /// All enable-able tool definitions (`get_all_tools`).
    Tools(Vec<crate::ToolInfo>),
    /// The result of a `/tree` navigation (`navigate_tree`).
    TreeNavigation(NavigateTreeOutcome),
}

impl AgentSession {
    /// Execute one [`SessionCommand`] through the single seam (arch-11 §2.1). Every adapter routes
    /// here so behaviour cannot diverge per front-end.
    pub async fn execute(
        &self,
        command: SessionCommand,
    ) -> Result<SessionCommandOutput, SessionServiceError> {
        use SessionCommand as C;
        use SessionCommandOutput as O;
        Ok(match command {
            C::Prompt(input) => O::Accepted(self.prompt_accepted(input).await?),
            C::Steer(input) => O::Accepted(self.steer(input).await?),
            C::FollowUp(input) => O::Accepted(self.follow_up(input).await?),
            C::Abort => {
                self.abort();
                O::Unit
            }
            C::ClearQueue => {
                self.clear_queue().await;
                O::Unit
            }
            C::Compact { instructions } => O::Compacted(self.compact(instructions).await?),
            C::AbortCompaction => {
                self.abort_compaction();
                O::Unit
            }
            C::AbortBranchSummary => {
                self.abort_branch_summary();
                O::Unit
            }
            C::SetModel { pattern } => {
                self.set_model(&pattern).await?;
                O::Unit
            }
            C::CycleModel { forward } => {
                O::Model(self.cycle_model(forward).await?.map(|r| r.model.id.to_string()))
            }
            C::SetAutoRetry { enabled } => {
                self.set_auto_retry_enabled(enabled);
                O::Unit
            }
            C::AbortRetry => {
                self.abort_retry();
                O::Unit
            }
            C::SetAutoCompaction { enabled } => {
                self.set_auto_compaction_enabled(enabled);
                O::Unit
            }
            C::Bash { command, exclude_from_context } => O::Bash(
                self.execute_bash(
                    &command,
                    crate::BashOptions { exclude_from_context },
                    None,
                )
                .await,
            ),
            C::AbortBash => {
                self.abort_bash();
                O::Unit
            }
            C::GetActiveTools => O::ToolNames(self.active_tool_names()),
            C::GetAllTools => O::Tools(self.all_tools()),
            C::SetActiveTools { names } => {
                self.set_active_tools_by_name(&names).await;
                O::Unit
            }
            C::SetThinkingLevel { level } => {
                O::ThinkingLevel(Some(self.set_thinking_level(level).await?))
            }
            C::CycleThinkingLevel => O::ThinkingLevel(self.cycle_thinking_level().await?),
            C::SetSteeringMode { mode } => {
                self.set_steering_mode(mode);
                O::Unit
            }
            C::SetFollowUpMode { mode } => {
                self.set_follow_up_mode(mode);
                O::Unit
            }
            C::Branch { entry } => {
                self.branch(entry).await?;
                O::Unit
            }
            C::NavigateTree { target, options } => {
                O::TreeNavigation(self.navigate_tree(target, options).await?)
            }
            C::Fork { entry, position } => O::Fork(self.fork_at_entry(&entry, position).await?),
            C::CloneAt { entry } => O::Text(Some(self.clone_at(entry).await?.to_string())),
            C::SetSessionName { name } => {
                self.set_session_name(&name).await?;
                O::Unit
            }
            C::GetState => O::State(Box::new(self.state_view().await)),
            C::GetSessionStats => O::Stats(self.session_stats().await),
            C::GetContextUsage => O::ContextUsage(self.context_usage().await),
            C::GetForkMessages => O::ForkMessages(self.user_messages_for_forking().await),
            C::ExportJsonl { path } => O::Text(self.export_to_jsonl(path.as_deref()).await?),
            C::GetLastAssistantText => O::Text(self.last_assistant_text().await),
        })
    }
}
