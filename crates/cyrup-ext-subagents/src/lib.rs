//! cyrup-ext-subagents — SubAgents: OS-subprocess delegation extension (arch-SA;
//! conformance: `spec/extensions/subagents/00-functionality.md`, func-SA).
//!
//! Ports `pi-subagents` onto cyrup as a single `cyrup_ext::native::NativeExtension`. The
//! mandatory mechanism (func-SA §1.1, arch-SA §1.1) is binding and non-negotiable: a subagent
//! run is ALWAYS a genuine OS subprocess re-exec of the `cyrup` binary, resolved via
//! `std::env::current_exe()` and spawned via `tokio::process::Command`; communication with a
//! running child is one-directional NDJSON over stdout, never an in-process object graph;
//! cancellation is real SIGINT->SIGTERM->SIGKILL OS-signal escalation, never a cooperative
//! in-process token standing in for process termination; fork-context is a direct, synchronous
//! `cyrup-session` library call (never routed through `cyrup-ext`'s capability system); and
//! background execution is a second, detached OS-process hop. This crate has ZERO dependency on
//! `cyrup-agent` — the subagent's own turn loop runs entirely inside the spawned child's own
//! `cyrup` binary invocation, never inside this process's in-process agent runtime.
//!
//! No-panic policy (arch-00 §8) is enforced crate-wide via `[workspace.lints]`; this crate-level
//! `#![deny(...)]` mirrors `cyrup-ext`'s own explicit restatement of that convention. Tests may
//! `#[allow(...)]` at the test-module level where unwrap/expect/indexing is acceptable.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![forbid(unsafe_code)]

pub mod artifacts;
pub mod background;
pub mod discovery;
pub mod error;
pub mod exec;
pub mod extension;
pub mod fork_context;
/// The crate's single port of pi's `shared/formatters.ts` (`formatTokens`,
/// `formatModelThinking`, the run-mode label) — see [`formatters`].
pub mod formatters;
pub mod jsonl;
pub mod missions;
pub mod native_supervisor;
/// The crate's single port of pi's `shared/utils.ts` path helpers: the `CYRUP_HOME` -> `HOME` ->
/// tempdir home ladder, `getAgentDir()` and `getProjectConfigDir()`. See [`paths`].
pub mod paths;
pub mod prompt_runtime;
pub mod registration;
pub mod runner;
pub mod spawn;
/// The crate's single epoch-millisecond clock (`Date.now()`). See [`time`].
pub mod time;
pub mod tui;
pub mod watchdog;

/// Crate-internal test modules relocated out of `tests/` (see [`tests`]'s own module doc):
/// in-process tests that never needed a separate Cargo integration-test binary.
#[cfg(test)]
mod tests;

// P-5 (cyrup-permission-system-port.md §4 / reconciliation §2): re-export the `background::control`
// file-channel primitives + the R-SA-P1 parent-session anchor const at the crate root so the
// permission companion's child→parent ask-forwarding spool (its own P-4 build) consumes them
// cross-crate as BUILDING BLOCKS (not as the subagents interrupt channel — permission replicates
// pi's own nonce-bound request/response spool on top of these). These are already `pub` at
// `background::control::*`/`exec::*`; the flat re-export is the ergonomic P-5 surface the port doc
// names. This crate WRITES the anchor into every spawn env (`build_attempt_spawn_plan`,
// `background::spawn_detached`) and permission reads it back in the child.
//
// PERM-001 adds the one flow in the other direction: pi's orchestrator publishes the anchor into
// its OWN `process.env` at `SessionStart` (`pi-subagents/src/extension/index.ts:716` @v0.43.0,
// deleted at `:619`) so that even a DETACHED descendant inherits it, and this crate cannot do that
// (`#![forbid(unsafe_code)]` vs. 2024-edition `unsafe std::env::set_var`). The
// [`background::parent_anchor`] register stands in for that `process.env` slot, and the anchor's
// sole consumer — `cyrup-permission-system`, in its PARENT role — is what publishes into it. These
// two re-exports are that publish/clear pair.
pub use background::control::{
    CONTROL_INBOX_POLL_INTERVAL, validate_contains_root, validate_safe_token, watch_control_inbox,
};
pub use background::parent_anchor::{clear_parent_session_anchor, publish_parent_session_anchor};
pub use exec::{AGENT_NAME_ENV_VAR, PARENT_SESSION_ENV_VAR};

// SUBA-084 — pi `src/api/agents.ts:2,12` @v0.64.0 re-exports the runtime agent registration
// surface (`registerAgent` + its `RuntimeAgentDefinition`/`RuntimeAgentRegistration` types) at
// the package's public API root. The entry point itself is a method
// (`extension::SubagentsExtension::register_agent` / `SubagentExecutor::register_agent`) because
// the registry is owned per extension instance rather than process-global; the types an embedder
// needs to call it are re-exported here so it never has to spell the module path.
pub use discovery::runtime_registry::{
    RuntimeAgentDefinition, RuntimeAgentRegistration, RuntimeAgentRegistry, RuntimeThinking,
};
