//! Env-driven opt-in proof for the SubAgents extension's install gate — specifically the
//! `CYRUP_SUBAGENTS` branch of [`cyrup_ext_subagents::extension::is_installed`]. This cannot live in a
//! `src/` `#[cfg(test)]` module: this crate is `#![forbid(unsafe_code)]` and Rust 2024 requires
//! `unsafe` for `std::env::set_var`/`remove_var`. A `tests/*.rs` file is its own compilation unit, not
//! subject to the library crate's `forbid` attribute — the exact precedent
//! `background_runner_main_integration.rs` / `companions_*_proof.rs` already establish for this crate's
//! env-mutating proofs.
//!
//! Requirement (b) of the opt-in fix: with `CYRUP_SUBAGENTS=1` a plain TOP-LEVEL session attaches the
//! FULL orchestrator surface (default OFF flips ON); clearing it — with no `subagents/config.json`
//! present anywhere — attaches NOTHING again. This mirrors `cyrup_intercom` / `cyrup_permission_system`
//! opt-in semantics exactly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_ext::EventKind;
use cyrup_ext::native::InitApi;
use cyrup_ext_subagents::extension::{INSTALL_ENV_VAR, is_installed, subagent_extension_for_env};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

/// Serializes the process-global env mutation this file performs (a tokio mutex so the guard is held
/// across the async `init` await), matching this crate's established env-mutation test convention.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const CHILD_ENV: &str = "CYRUP_SUBAGENT_CHILD";
const FANOUT_CHILD_ENV: &str = "CYRUP_SUBAGENT_FANOUT_CHILD";

#[tokio::test]
async fn cyrup_subagents_env_opts_a_top_level_session_into_the_full_surface() {
    let _guard = ENV_LOCK.lock().await;

    // Empty dirs: NO on-disk config anywhere, so the ONLY install signal under test is the env var.
    let agent_dir = tempfile::tempdir().expect("agent dir");
    let cwd = tempfile::tempdir().expect("cwd");

    // SAFETY: scoped, mutex-serialized env mutation (Rust 2024 requires `unsafe` for set/remove_var);
    // a `tests/*.rs` file is a separate compilation unit not bound by the library's
    // `#![forbid(unsafe_code)]`. Force TOP-LEVEL by clearing the two child flags (so this process
    // resolves to `Full`, not `ChildSafe`), then opt in via `CYRUP_SUBAGENTS=1`.
    unsafe {
        std::env::remove_var(CHILD_ENV);
        std::env::remove_var(FANOUT_CHILD_ENV);
        std::env::set_var(INSTALL_ENV_VAR, "1");
    }

    // (b) `CYRUP_SUBAGENTS=1` marks a config-less installation as installed...
    assert!(
        is_installed(agent_dir.path(), cwd.path()),
        "CYRUP_SUBAGENTS=1 marks a config-less installation as installed"
    );
    // ...and a top-level session attaches the FULL orchestrator surface (SessionStart housekeeping,
    // which a `ChildSafe` surface never installs — so this asserts Full specifically).
    let ext = subagent_extension_for_env(
        agent_dir.path(),
        SubagentExtensionConfig::default(),
        cwd.path().to_path_buf(),
    )
    .expect("an opted-in top-level session attaches the subagents extension");
    let mut api = InitApi::new();
    ext.init(&mut api).await.expect("full init succeeds");
    assert!(
        api.subscriptions().contains(EventKind::SessionStart),
        "the attached surface is the FULL orchestrator (installs the SessionStart housekeeping)"
    );

    // Default OFF: clear the opt-in and, with no config file anywhere, the SAME top-level session
    // attaches NOTHING.
    // SAFETY: as above.
    unsafe {
        std::env::remove_var(INSTALL_ENV_VAR);
    }
    assert!(
        !is_installed(agent_dir.path(), cwd.path()),
        "with no env and no config file, a config-less installation is NOT installed (default OFF)"
    );
    assert!(
        subagent_extension_for_env(
            agent_dir.path(),
            SubagentExtensionConfig::default(),
            cwd.path().to_path_buf(),
        )
        .is_none(),
        "a top-level session that has not opted in attaches nothing"
    );
}
