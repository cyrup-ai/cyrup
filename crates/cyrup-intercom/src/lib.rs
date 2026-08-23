//! cyrup-intercom — out-of-band supervisor coordination companion (a 1:1 source port of
//! `pi-intercom` v0.9.2, with the v0.9.3/v0.10.x deltas listed in
//! `docs/gap-analysis/11-cyrup-intercom.md` ported item-by-item).
//!
//! A native `cyrup_ext::native::NativeExtension` plus a hidden broker **process** (the
//! `cyrup __intercom-broker` subcommand). Every local session connects to one shared broker over a
//! Unix-domain socket with 4-byte-big-endian length-prefixed JSON framing; a subagent child and its
//! supervisor are just two ordinary broker clients whose ask/answer routes child→broker→supervisor
//! by session identity (never over the child's stdio, never via a file drop). See
//! `spec/extensions/cyrup-intercom-port.md`.
//!
//! No-panic policy (arch-00 §8). The first four lints below are denied workspace-wide via
//! `[workspace.lints]` and restated here; `unreachable`/`todo`/`unimplemented` are denied at
//! this crate root only, because promoting them to the workspace table would harden eight
//! other crates at once. Tests `#[allow(...)]` at the test-module level.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]
#![forbid(unsafe_code)]

pub mod broker;
pub mod config;
pub mod connect;
pub mod cwd;
pub mod error;
pub mod extension;
pub mod format_context;
pub mod identity;
pub mod inbound;
pub mod paths;
pub mod project_target;
pub mod relay;
pub mod resources;
pub mod reply_tracker;
pub mod seams;
pub mod session_state;
pub mod tools;
pub mod transport;
pub mod ui;

#[cfg(test)]
mod tests;

pub use error::{IntercomError, Result};
pub use extension::{IntercomExtension, intercom_extension_for_env, intercom_extension_for_env_concrete};
