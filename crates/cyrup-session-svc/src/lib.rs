//! cyrup-session-svc — the `AgentSession` facade (arch-11; the single integration seam).
//!
//! Wires provider + agent + tools + session + config + resources + ext into the one surface every
//! front-end (`cyrup-tui`/`cyrup-modes`/`cyrup-sdk`) and embedder consumes (func-11 R-11-023).
//!
//! ## The seam
//! - [`SessionBuilder`] (+ [`SessionConfig`]/[`SessionTarget`]) assembles a run from the real
//!   subsystems: resolve settings + trust + auth + model; discover resources; build the tool
//!   registry with isolation decorators + permission policy; open/create the session tree and wire
//!   compaction; assemble the system prompt via `SystemPromptBuilder` + `ContextStore`; build the
//!   extension host with native built-ins and attach BOTH ext seams (`EventSubscriber` + `Hooks`)
//!   to the agent; resolve the provider into the agent loop.
//! - [`AgentSession`] is the runtime surface: `prompt` (→ an [`EventStream`] of
//!   [`AgentSessionEvent`]), `steer`/`follow_up`, `abort`, `compact`, `branch`/`fork`, `set_model`,
//!   `wait_for_idle` — with durable persistence across every turn.
//! - [`SessionServiceError`] aggregates the subsystem errors.
#![forbid(unsafe_code)]

mod attribution;
mod bash;
mod builder;
mod command;
mod compact;
mod error;
mod event;
pub mod export;
mod factory;
mod guest_providers;
mod host_services;
mod hooks;
mod provider_swap;
mod runtime;
mod services;
mod session;
mod state;
mod subscriber;
mod tools;

pub use attribution::merge_provider_attribution_headers;
// `BashChunkSink` is exported alongside them because it is the `on_chunk` parameter type of the
// public `execute_bash`/`execute_bash_with_user_event`: a front-end that streams a `!`/`!!` run's
// output (the interactive TUI does) has to name it to build the sink.
pub use bash::{BashChunkSink, BashOptions, BashResult};
pub use builder::{
    extension_discovery_roots, ExtensionFlagValue, NoTools, SessionBuilder, SessionConfig,
    SessionTarget,
};
pub use command::{SessionCommand, SessionCommandOutput};
pub use error::SessionServiceError;
pub use export::session_jsonl_to_html;
pub use event::{
    AgentSessionEvent, DeliverAs, InputSource, PromptAccepted, PromptOptions, StreamingBehavior,
    SummarizationRetrySource, UserInput,
};
/// The streaming delta carried on [`AgentSessionEvent::MessageUpdate`]. Re-exported because it is
/// already part of this crate's public surface (the variant's `assistant_message_event` payload) —
/// without it a consumer cannot match on the seam's own event without taking a direct
/// `cyrup-provider` dependency. `cyrup-modes`' wire projection is the first such consumer.
pub use cyrup_provider::StreamEvent;
pub use factory::SessionFactory;
pub use guest_providers::GuestProviderRegistry;
pub use cyrup_ext::NotifyKind;
pub use host_services::{
    ControlSink, InjectMessage, InjectSink, LiveHostServices, OverlayRequest, OverlaySink, UiEffect,
    UiEffectSink, UiKind, UiReply, UiRequest, UiSink,
};
pub use provider_swap::{ProviderResolver, ProviderSwap};
pub use runtime::{
    AgentSessionRuntime, NewSessionOptions, RuntimeActions, RuntimeDiagnostic, RuntimeForkResult,
    SwitchResult, SwitchSessionOptions,
};
pub use services::{AgentSessionServices, ExtensionLoadDiagnostic, StartupDiagnostics};
pub use session::{
    AgentSession, ForkAnchor, ForkOutcome, ForkPosition, ModelCycleResult, NavigateTreeOptions,
    NavigateTreeOutcome, ScopedModel, SessionDagKind, SessionDagNode,
};
pub use state::{
    CompactionResult, ContextUsage, SessionStateView, SessionStats, StatsContextUsage,
    StatsTokens,
};
pub use tools::ToolInfo;
// The compaction `reason` carried by `AgentSessionEvent::Compaction{Start,End}` (so a front-end can
// branch its status copy on manual/threshold/overflow without depending on `cyrup-session` directly).
pub use cyrup_session::compaction::CompactionReason;

// Load-bearing re-exports so embedders need not depend on every subsystem directly.
pub use cyrup_core::EventStream;
pub use cyrup_config::AppMode;
/// Re-exported so the TUI `/settings` selector can name the write scope for [`AgentSession::persist_setting`]
/// without a direct `cyrup-config` dependency (the additive L6↔L5 settings-write seam).
pub use cyrup_config::SettingsScope;
/// Re-exported so the TUI `/trust` selector can name the persisted decision type for
/// [`AgentSession::write_project_trust`] without a direct `cyrup-config` dependency.
pub use cyrup_config::trust::{TrustDecision, TrustEntry, TrustOption};
/// Re-exported so the TUI `/settings` selector can read the merged config + the default-trust enum
/// for its grid rows without a direct `cyrup-config` dependency.
pub use cyrup_config::{DefaultProjectTrust, EffectiveSettings, EnvVars};
/// Re-exported because it is the argument type of [`SessionBuilder::cli_settings`] — an embedder (or
/// a front-end test) cannot name that seam's input without a direct `cyrup-config` dependency.
pub use cyrup_config::Settings;
/// Re-exported so the TUI `/resume` selector can name the session-list rows
/// [`AgentSession::list_sessions`] returns without a direct `cyrup-session` dependency.
pub use cyrup_session::listing::SessionInfo;
/// Re-exported so a front-end can walk the ROLE-tagged context
/// [`AgentSession::raw_context_messages`] returns — Pi's `AgentMessage` union
/// (`messages.ts:26-77`), whose `bashExecution`/`custom`/`branchSummary`/`compactionSummary` arms
/// [`cyrup_core::Message`] cannot represent — without a direct `cyrup-session` dependency. This is
/// what lets the TUI replay a resumed session into its per-role components instead of the
/// LLM-flattened `user` text (interactive-mode.ts:3506-3516).
pub use cyrup_session::agent_message;
/// Re-exported so the CLI bin can resolve `--session`/`--fork` partial-UUID + global-cross-project
/// references (Pi `resolveSessionPath`, main.ts:163-189) without a direct `cyrup-session` dependency:
/// [`list_in_dir`] lists a cwd's sessions and [`list_all`] scans every project under the root.
pub use cyrup_session::layout::{SessionLayout, SessionsRoot};
pub use cyrup_session::listing::{list_all, list_in_dir};
pub use cyrup_tools::{Availability, PermissionPolicy};
/// Re-exported so front-ends (`cyrup-modes` RPC `set_steering_mode`/`set_follow_up_mode`) can name
/// the queue-drain mode [`AgentSession::set_steering_mode`] takes without a direct `cyrup-agent` dep.
pub use cyrup_agent::QueueMode;
/// Re-exported so an embedder can name the custom-transport seam types
/// ([`SessionBuilder::stream_fn`]/[`SessionBuilder::key_resolver`], Pi `AgentOptions.streamFn`) and
/// the built-in proxy transport ([`ProxyStreamFn`]) without a direct `cyrup-agent` dependency.
pub use cyrup_agent::{ApiKeyResolver, ProxyStreamFn, ProxyStreamOptions, StreamFn};
/// Re-exported so an embedder can name the synthetic-resource override closures' element types
/// ([`SessionBuilder::skills_override`]/[`SessionBuilder::context_files_override`]) without a direct
/// `cyrup-resources`/`cyrup-session` dependency.
pub use cyrup_resources::SkillPointer;
pub use cyrup_session::prompt::{ContextFile, ContextScope};
/// Re-exported so front-ends can name the thinking level [`AgentSession::set_thinking_level`] takes
/// and the entry id the RPC `fork` targets without a direct `cyrup-core` dependency.
pub use cyrup_core::{Content, EntryId, ModelThinkingLevel};
