//! Registry assembly: `resources_discover` contribution merge, `/reload` re-discovery, missing
//! CLI-path diagnostics, and settings override patterns over loose resources (A-09-7, A-09-8, G4, G8).

use super::fixtures::{cfg, run_discover, skill_md, write};
use crate::{DiscoveryConfig, ResourceHandle, ResourceOverrides, ResourceScope};

// ===========================================================================
// A-09-7 — resources_discover contribution merged
// ===========================================================================

#[tokio::test]
async fn a09_7_resources_discover_contribution_merged() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let contributed = root.join("ext/skills/contrib/SKILL.md");
    write(&contributed, &skill_md("contrib", "from an extension"));

    let mut c = cfg(root);
    c.extra.skill_paths = vec![contributed.parent().unwrap().to_path_buf()];
    let report = run_discover(&c).await;
    let skill = report
        .registry
        .skills
        .get_name("contrib")
        .expect("contributed skill available");
    assert_eq!(
        skill.scope,
        ResourceScope::Discovered,
        "enters at Discovered scope (R-09-022)"
    );
}

// ===========================================================================
// A-09-8 — /reload re-discovers
// ===========================================================================

#[tokio::test]
async fn a09_8_reload_rediscovers_new_resources() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut c = cfg(root);
    c.trusted_project = true;

    let first = run_discover(&c).await;
    let handle = ResourceHandle::new(first.registry);
    assert!(!handle.load().skills.contains("late"));

    // Add a new skill, then re-run discovery and atomically swap (R-09-023).
    write(
        &root.join(".cyrup/skills/late/SKILL.md"),
        &skill_md("late", "added at runtime"),
    );
    let second = run_discover(&c).await;
    handle.store(std::sync::Arc::new(second.registry));
    assert!(
        handle.load().skills.contains("late"),
        "/reload re-discovers new skill"
    );
}

// ===========================================================================
// Explicit CLI path that does not exist → diagnostic (G8)
// ===========================================================================

#[tokio::test]
async fn missing_cli_skill_path_emits_warning() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let mut c = cfg(root);
    c.cli.skills = vec![root.join("does/not/exist")];
    let report = run_discover(&c).await;
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("skill path does not exist")),
        "a missing --skill path warns instead of scanning silently (skills.ts:458)"
    );
}

// ===========================================================================
// G4 — settings override patterns filter auto-discovered loose resources
// ===========================================================================

#[tokio::test]
async fn settings_override_filters_loose_global_skills() {
    // Pi filters auto-discovered loose resources against the settings skills/prompts/themes arrays
    // via isEnabledByOverrides (package-manager.ts:700-717, 2271-2304). A `!skills/drop` override
    // disables one discovered global skill while leaving the other (and a `-` force-exclude works).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global");
    write(
        &global.join("skills/keep/SKILL.md"),
        &skill_md("keepskill", "kept"),
    );
    write(
        &global.join("skills/drop/SKILL.md"),
        &skill_md("dropskill", "dropped"),
    );

    // Baseline: both discovered with no overrides.
    let baseline = run_discover(&DiscoveryConfig::new(root, &global)).await;
    assert!(
        baseline.registry.skills.contains("keepskill")
            && baseline.registry.skills.contains("dropskill"),
        "both loose skills discovered without overrides"
    );

    // With a `!skills/drop` settings override, only `keep` survives.
    let mut c = DiscoveryConfig::new(root, &global);
    c.global_overrides = ResourceOverrides {
        skills: vec!["!skills/drop".to_string()],
        ..Default::default()
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("keepskill"),
        "keep retained under override"
    );
    assert!(
        !report.registry.skills.contains("dropskill"),
        "drop excluded by settings `!skills/drop` override"
    );
}
