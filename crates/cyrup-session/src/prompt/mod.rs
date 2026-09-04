//! System-prompt & context assembly (arch-06; conformance: func-06, R-06-001..017).
//!
//! This module is the Rust port of Pi's `buildSystemPrompt` + the context-loading slice of
//! `resource-loader.ts`. It is separate from [`crate::context`] (arch-04 session-context building):
//! this layer assembles the **system prompt** string sent to the model and discovers the
//! **project context files** (`AGENTS.md`/`CLAUDE.md`) + **skill pointers** that feed it.
//!
//! Pipeline (arch-06 §2):
//! ```text
//! caller gathers ToolPromptContribution + SkillPointer + ContextSnapshot
//!   -> SystemPromptBuilder::build(&PromptInputs) -> String
//!   -> apply_before_agent_start(prompt, &inputs, hooks) -> final prompt   (R-06-014)
//! ```
//!
//! - Pure assembly: [`SystemPromptBuilder`] / [`PromptInputs`] / [`DocsPointers`] (no I/O).
//! - Blocking discovery: [`ContextFileLoader`] (run via `spawn_blocking`).
//! - Session cache: [`ContextStore`] / [`ContextSnapshot`] (`arc-swap`, read-once-per-session).
//! - Override result: [`ResolvedOverride`] (CLI > project > global, resolved upstream).
//! - Hook seam: [`BeforeAgentStartInput`] / [`BeforeAgentStartOutput`] / [`apply_before_agent_start`].

pub mod builder;
pub mod cache;
pub mod context_files;
pub mod hook;
pub mod overrides;
pub mod skills_inject;
pub mod tool_prompts;

pub use builder::{DEFAULT_SELECTED_TOOLS, DocsPointers, PromptInputs, SystemPromptBuilder};
pub use cache::{ContextError, ContextSnapshot, ContextStore};
pub use context_files::{
    ContextDiagnostic, ContextFile, ContextFileLoader, ContextScope, TrustQuery,
};
pub use hook::{
    BeforeAgentStartHook, BeforeAgentStartInput, BeforeAgentStartOutput, apply_before_agent_start,
};
pub use overrides::ResolvedOverride;
pub use tool_prompts::ToolPromptContribution;

// Re-export the prompt-facing skill pointer projection (defined in cyrup-resources).
pub use cyrup_resources::SkillPointer;

#[cfg(test)]
mod tests;
