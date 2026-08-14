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

/// Artifact quadruple writer + housekeeping sweep (T6) — Rust port of pi `shared/artifacts.ts`. See
/// [`artifacts`] for the four-file `<runId>_<agent>[_i]_{input.md,output.md,.jsonl,_meta.json}`
/// layout and the 24h-throttled 7-day cleanup contract.
pub mod artifacts;
pub mod background;
pub mod discovery;
pub mod error;
pub mod exec;
pub mod extension;
pub mod fork_context;
/// The shared, size-capped append-only JSONL primitive (R-SA-136/146) used by both
/// [`spawn::SpawnedChild`]'s child-output tee and [`background::RunPaths::events`]'s async-run
/// event log. See [`jsonl`] for the full contract.
/// The NATIVE supervisor channel (`pi-subagents/src/intercom/native-supervisor-channel.ts`): the
/// broker-free, file-backed child↔supervisor request/reply channel upstream introduced in `3ac0ef5`
/// ("Make supervisor coordination native") when it deleted the companion-recommendation surface.
pub mod native_supervisor;

pub mod jsonl;
/// The durable MISSION subsystem — a 1:1 port of `pi-subagents/src/missions/` (6 files @v0.43.0):
/// mission records, their store, the launch binding that ties a mission to a real run, the six
/// `mission.*` tool actions, per-mission workflow state, and the turn-end goal-continuation driver.
/// See [`missions`] for the full file-by-file correspondence table.
pub mod missions;
pub mod prompt_runtime;
pub mod registration;
pub mod spawn;
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
    validate_contains_root, validate_safe_token, watch_control_inbox, CONTROL_INBOX_POLL_INTERVAL,
};
pub use background::parent_anchor::{clear_parent_session_anchor, publish_parent_session_anchor};
pub use exec::{AGENT_NAME_ENV_VAR, PARENT_SESSION_ENV_VAR};
