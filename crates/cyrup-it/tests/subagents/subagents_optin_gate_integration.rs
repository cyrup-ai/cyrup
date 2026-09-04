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

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use cyrup_ext::EventKind;
use cyrup_ext::native::InitApi;
use cyrup_ext_subagents::extension::{is_installed_with, subagent_extension_for};
use cyrup_ext_subagents::registration::SubagentExtensionConfig;

#[tokio::test]
async fn cyrup_subagents_env_opts_a_top_level_session_into_the_full_surface() {
    // Empty dirs: NO on-disk config anywhere, so the ONLY install signal under test is the env var.
    let agent_dir = tempfile::tempdir().expect("agent dir");
    let cwd = tempfile::tempdir().expect("cwd");

    // The install signal is INJECTED, not exported: `is_installed_with` takes the same lookup the
    // production path uses, so this still proves the `CYRUP_SUBAGENTS` branch — while the
    // top-level-vs-child decision is stated as arguments rather than by clearing two more
    // process-global flags. Nothing here races a concurrent reader of the environment.
    let opted_in = |_: &str| Some("1".to_string());
    let not_opted_in = |_: &str| None;

    // (b) `CYRUP_SUBAGENTS=1` marks a config-less installation as installed...
    assert!(
        is_installed_with(&opted_in, agent_dir.path(), cwd.path()),
        "CYRUP_SUBAGENTS=1 marks a config-less installation as installed"
    );
    // ...and a top-level session attaches the FULL orchestrator surface (SessionStart housekeeping,
    // which a `ChildSafe` surface never installs — so this asserts Full specifically).
    let ext = subagent_extension_for(
        SubagentExtensionConfig::default(),
        cwd.path().to_path_buf(),
        /* child */ false,
        /* fanout_authorized */ false,
        /* installed */ true,
    )
    .expect("an opted-in top-level session attaches the subagents extension");
    let mut api = InitApi::new();
    ext.init(&mut api).await.expect("full init succeeds");
    assert!(
        api.subscriptions().contains(EventKind::SessionStart),
        "the attached surface is the FULL orchestrator (installs the SessionStart housekeeping)"
    );

    // Default OFF: without the opt-in, and with no config file anywhere, the SAME top-level
    // session attaches NOTHING.
    assert!(
        !is_installed_with(&not_opted_in, agent_dir.path(), cwd.path()),
        "with no env and no config file, a config-less installation is NOT installed (default OFF)"
    );
    assert!(
        subagent_extension_for(
            SubagentExtensionConfig::default(),
            cwd.path().to_path_buf(),
            /* child */ false,
            /* fanout_authorized */ false,
            /* installed */ false,
        )
        .is_none(),
        "a top-level session that has not opted in attaches nothing"
    );
}
