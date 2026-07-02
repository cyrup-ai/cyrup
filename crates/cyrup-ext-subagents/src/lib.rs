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

pub mod background;
pub mod discovery;
pub mod error;
pub mod exec;
pub mod extension;
pub mod fork_context;
pub mod registration;
pub mod spawn;
pub mod tui;
