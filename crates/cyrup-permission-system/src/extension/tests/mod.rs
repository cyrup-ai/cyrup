//! The `extension` module's own unit tests.
//!
//! These stay here rather than under [`crate::tests`] for module locality, not for isolation:
//! several of them OVERRIDE env — [`crate::extension::INSTALL_ENV_VAR`],
//! `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`, the parent-session anchor — and none of them mutates the
//! process environment to do it. Each takes a THREAD-LOCAL [`crate::envx`] pin, which no other
//! test's thread can observe, so no lock is involved and none is needed.
//!
//! The process-global statics that DO still need serializing are unrelated to the environment and
//! must exist EXACTLY ONCE, shared by every caller: [`crate::runtime_api::test_registry_lock`]
//! guards the `RUNTIME_API` slot, and `watcher::ANCHOR_REGISTER_LOCK` is declared once in the one
//! module that uses it. A per-module copy of either silently destroys the mutual exclusion.
//!
//! The `#![allow]` below is the module-level allow the single inline `mod tests` carried; a lint
//! level on a `mod` item applies to its out-of-line body, so it covers every submodule here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

mod agent_start;
mod config_reload;
mod enabled_switch;
mod env;
mod events;
mod gate;
mod install;
mod support;
mod watcher;
