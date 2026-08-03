//! cyrup-intercom — out-of-band supervisor coordination companion (a 1:1 source port of
//! `pi-intercom` v0.6.0).
//!
//! A native `cyrup_ext::native::NativeExtension` plus a hidden broker **process** (the
//! `cyrup __intercom-broker` subcommand). Every local session connects to one shared broker over a
//! Unix-domain socket with 4-byte-big-endian length-prefixed JSON framing; a subagent child and its
//! supervisor are just two ordinary broker clients whose ask/answer routes child→broker→supervisor
//! by session identity (never over the child's stdio, never via a file drop). See
//! `spec/extensions/cyrup-intercom-port.md`.
//!
//! No-panic policy (arch-00 §8) is enforced crate-wide via `[workspace.lints]`; this crate-level
//! `#![deny(...)]` restates it. Tests `#[allow(...)]` at the test-module level.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
#![forbid(unsafe_code)]

pub mod broker;
pub mod config;
pub mod connect;
pub mod error;
pub mod extension;
pub mod identity;
pub mod inbound;
pub mod paths;
pub mod relay;
pub mod reply_tracker;
pub mod seams;
pub mod session_state;
pub mod tools;
pub mod transport;
pub mod ui;

pub use error::{IntercomError, Result};
pub use extension::{IntercomExtension, intercom_extension_for_env, intercom_extension_for_env_concrete};
