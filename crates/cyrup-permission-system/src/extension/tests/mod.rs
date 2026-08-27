//! The `extension` module's own unit tests.
//!
//! These stay here rather than under [`crate::tests`] (whose doc states the bar for that
//! directory): several of them MUTATE process env — [`crate::extension::INSTALL_ENV_VAR`],
//! `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH`, the parent-session anchor — and are safe only because
//! they hold shared locks while they do it. Those locks must exist EXACTLY ONCE and be shared by
//! every caller: [`crate::ext_config::env_lock`] and [`crate::runtime_api::test_registry_lock`]
//! are crate-level statics, and `watcher::ANCHOR_REGISTER_LOCK` is declared once in the one module
//! that uses it. A per-module copy of any of them silently destroys the mutual exclusion.
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
mod project_trust;
mod support;
mod watcher;
