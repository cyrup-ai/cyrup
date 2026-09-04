//! Integration test: the BUNDLED `skills/pi-subagents/SKILL.md` (and the seven prompt recipes)
//! actually register with the host (G147).
//!
//! Upstream declares both statically in its package manifest — `"skills": ["./skills"]`,
//! `"prompts": ["./prompts"]` (`pi-subagents/package.json:56-61` @v0.34.0) — and pi's package
//! manager loads them when the extension package is installed. cyrup's subagents extension is a
//! NATIVE built-in with no package.json, so the same declaration travels the extension seam:
//! `resources_discover` (R-09-022).
//!
//! The entry point driven here is the exact pair of calls `cyrup-session-svc`'s session builder
//! makes at startup (`crates/cyrup-session-svc/src/builder.rs:977` then `:1001-1002`):
//!
//! ```text
//! let agg = ext_host.aggregate_resources(&cancel).await;
//! report.registry.extend(&DiscoveredPaths { skill_paths, prompt_paths, theme_paths })
//! ```
//!
//! and the assertion is on the resulting registry's WINNERS — the set that becomes the session's
//! `<available_skills>` block. `bundled_skill_files()` returning the right paths was already
//! proven by its own unit test and proved nothing about reachability.
//!
//! The user action: start any cyrup session with the subagents extension installed (`CYRUP_SUBAGENTS=1`
//! or a `cyrup-subagents` config file). The orchestrator's system prompt then lists the
//! `pi-subagents` skill, and the model opens it with `read`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::extension::{RegistrationMode, SubagentsExtension};
use crate::registration::SubagentExtensionConfig;
use cyrup_core::CancelToken;
use cyrup_ext::{ExtMode, ExtensionHost, HostConfig};
use cyrup_resources::{DiscoveredPaths, ResourceRegistry};

async fn host_at(cwd: &Path) -> Arc<ExtensionHost> {
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: true,
        cwd: cwd.to_path_buf(),
    }));
    host.load_native(Arc::new(SubagentsExtension::with_config_and_cwd(
        SubagentExtensionConfig::default(),
        cwd.to_path_buf(),
    )))
    .await
    .expect("the subagents extension loads");
    host
}

/// THE reachability proof: the aggregate the session builder reads carries the bundled skill, and
/// folding it through the registry exactly as the builder does makes `pi-subagents` a WINNER —
/// i.e. an entry of the orchestrator's `<available_skills>` block.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_bundled_pi_subagents_skill_reaches_the_session_resource_registry() {
    let dir = tempfile::tempdir().unwrap();
    let host = host_at(dir.path()).await;

    let agg = host.aggregate_resources(&CancelToken::new()).await;
    assert!(
        agg.skill_paths
            .iter()
            .any(|p| p.path.ends_with("skills/pi-subagents/SKILL.md")),
        "the extension must contribute its bundled SKILL.md: {:?}",
        agg.skill_paths
    );
    // Every contribution is attributed to this extension (gap-08 #15).
    for contributed in agg.skill_paths.iter().chain(agg.prompt_paths.iter()) {
        assert_eq!(
            contributed.extension.as_str(),
            "subagents",
            "contribution must be attributed to the subagents extension"
        );
    }

    // The builder's own fold, verbatim (`builder.rs:988-1002`).
    let extra = DiscoveredPaths {
        skill_paths: agg
            .skill_paths
            .iter()
            .map(|p| PathBuf::from(&p.path))
            .collect(),
        prompt_paths: agg
            .prompt_paths
            .iter()
            .map(|p| PathBuf::from(&p.path))
            .collect(),
        theme_paths: Vec::new(),
    };
    let registry = ResourceRegistry::default().extend(&extra);

    let skill_names: Vec<&str> = registry.skills.winners().map(|s| s.name.as_str()).collect();
    assert!(
        skill_names.contains(&"pi-subagents"),
        "the bundled operational skill must be a discovered winner: {skill_names:?}"
    );

    // Upstream declares `prompts` in the SAME package.json block, so the recipes register too.
    let prompt_names: Vec<&str> = registry
        .prompts
        .winners()
        .map(|p| p.name.as_str())
        .collect();
    for recipe in [
        "parallel-review",
        "review-loop",
        "gather-context-and-clarify",
    ] {
        assert!(
            prompt_names.contains(&recipe),
            "the bundled recipe {recipe:?} must register as a prompt template: {prompt_names:?}"
        );
    }
}

/// A CHILD-SAFE (fanout) registration contributes NOTHING: it declares no subscriptions at all, so
/// the orchestration skill never reaches a child's prompt through this seam. (The complementary
/// half — stripping it from a prompt a forked child INHERITED — is
/// `prompt_runtime::strip_subagent_orchestration_skill`.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_child_safe_registration_contributes_no_resources() {
    let dir = tempfile::tempdir().unwrap();
    let host = Arc::new(ExtensionHost::new(HostConfig {
        mode: ExtMode::Tui,
        has_ui: false,
        cwd: dir.path().to_path_buf(),
    }));
    host.load_native(Arc::new(SubagentsExtension::with_mode(
        SubagentExtensionConfig::default(),
        dir.path().to_path_buf(),
        RegistrationMode::ChildSafe,
    )))
    .await
    .expect("the child-safe surface loads");

    let agg = host.aggregate_resources(&CancelToken::new()).await;
    assert!(
        agg.skill_paths.is_empty() && agg.prompt_paths.is_empty(),
        "a fanout child must contribute no packaged resources: {agg:?}"
    );
}
