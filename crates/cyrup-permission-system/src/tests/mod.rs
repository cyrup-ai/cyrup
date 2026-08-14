//! Crate-internal test modules.
//!
//! These were formerly `tests/*.rs` integration binaries. Cargo compiles every file under `tests/`
//! into its OWN binary and runs it as its OWN process; for assertions that touch no process-global
//! state that per-file process buys nothing, so they live here as ordinary `#[cfg(test)]` modules
//! of the library instead.
//!
//! The assertions are unchanged from their integration-test form — only the crate self-reference
//! (`cyrup_permission_system::…` → `crate::…`) was rewritten.
//!
//! # What may NOT move here
//!
//! The bar for this directory is stricter than "no subprocess". `cargo test` runs the whole
//! crate's unit tests as parallel threads in ONE process, so a test that touches the process
//! ENVIRONMENT is only isolated while it owns a `tests/` binary of its own. Two classes must stay
//! under `tests/`:
//!
//! 1. **Tests that MUTATE process env.** `tests/prompt_dedup.rs` and `tests/forwarding_persist.rs`
//!    set `CYRUP_SUBAGENT_CHILD` process-wide (and `prompt_dedup` never restores it), while
//!    [`crate::extension`]'s own `ask_fails_fast_without_ui_subagent_or_yolo` asserts that variable
//!    is ABSENT. No lock fixes that without editing the pre-existing unit test.
//! 2. **Tests that resolve a config path.** [`crate::ext_config::ExtensionConfig::resolve_config_path`]
//!    re-reads `CYRUP_PERMISSION_SYSTEM_CONFIG_PATH` on every load, so anything built through
//!    `PermissionSystemExtension::new` must hold [`crate::ext_config::env_lock`] — see that
//!    function's doc comment and `extension.rs`'s `with_config_env_lock`. Acquiring it means
//!    restructuring each `#[tokio::test]` into the sync-lock-then-`block_on` shape, which is a
//!    change to the test, not a relocation of it.
//!
//! The three modules below construct their `ExtensionConfig` values directly and never resolve a
//! path from the environment, so neither hazard applies to them. (They each ASSERT
//! `FORWARDING_AGENT_DIR_ENV` is unset as a precondition; no test in this crate sets it.)

mod forwarded_prompt_fractional_timeout;
mod forwarding_preserve_location;
mod forwarding_response_path_containment;
