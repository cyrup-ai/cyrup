//! CFG-035 — `.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md` discovery, tier selection and trust gating.

use std::fs;

use crate::{DiscoveryConfig, discover};
use cyrup_core::CancelToken;

// ===========================================================================
// CFG-035 — `.cyrup/SYSTEM.md` / `APPEND_SYSTEM.md` discovery
// ===========================================================================

/// CFG-035: `discoverSystemPromptFile` (`resource-loader.ts:1022-1034` @v0.83.0) — the project file
/// wins ONLY when the project is trusted; otherwise the global file is used; otherwise nothing.
///
/// Before this landed, `grep -rn 'SYSTEM\.md' crates/` found the two filenames ONLY as trust-gate
/// MARKERS (`cyrup-config/src/trust.rs:194`, `:203-204`) — cyrup prompted the user to trust a
/// project *because of* a file it then never read.
#[test]
fn cfg035_system_prompt_file_is_discovered_project_first_under_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup")).unwrap();
    fs::create_dir_all(&agent).unwrap();

    // Nothing on disk → `None` (resource-loader.ts:1033).
    assert_eq!(crate::discover_system_prompt_file(&cwd, &agent, true), None);

    // Global only → the global file, regardless of trust (`:1028-1031` is NOT trust-gated).
    let global = agent.join("SYSTEM.md");
    fs::write(&global, "global").unwrap();
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, false),
        Some(global.clone())
    );
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, true),
        Some(global.clone())
    );

    // Project file present: it wins when trusted (`:1023-1026`) and is INVISIBLE when not.
    let project = cwd.join(".cyrup/SYSTEM.md");
    fs::write(&project, "project").unwrap();
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, true),
        Some(project.clone())
    );
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, false),
        Some(global),
        "an untrusted project falls through to the global file, not to None"
    );

    // Trusted, project file present, no global file.
    fs::remove_file(agent.join("SYSTEM.md")).unwrap();
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, true),
        Some(project)
    );
    assert_eq!(
        crate::discover_system_prompt_file(&cwd, &agent, false),
        None
    );
}

/// CFG-035: `discoverAppendSystemPromptFile` (`resource-loader.ts:1036-1048` @v0.83.0) is the same
/// two-tier pair over `APPEND_SYSTEM.md`, and picks exactly ONE file — the project one SHADOWS the
/// global one. `cyrup-session/src/prompt/overrides.rs:15-16` documents accumulation of both tiers;
/// upstream does not accumulate.
#[test]
fn cfg035_append_system_prompt_file_picks_exactly_one_tier() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let agent = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup")).unwrap();
    fs::create_dir_all(&agent).unwrap();
    let project = cwd.join(".cyrup/APPEND_SYSTEM.md");
    let global = agent.join("APPEND_SYSTEM.md");
    fs::write(&project, "project").unwrap();
    fs::write(&global, "global").unwrap();

    assert_eq!(
        crate::discover_append_system_prompt_file(&cwd, &agent, true),
        Some(project),
        "trusted: the project file shadows the global one — they never accumulate"
    );
    assert_eq!(
        crate::discover_append_system_prompt_file(&cwd, &agent, false),
        Some(global)
    );
    // The SYSTEM.md pair is independent of the APPEND_SYSTEM.md pair.
    assert_eq!(crate::discover_system_prompt_file(&cwd, &agent, true), None);
}

/// CFG-035: the discovery rides out on `DiscoveryReport`, off the same `cwd` / `global_dir` /
/// `trusted_project` the registry was built from — Pi computes both inside the same `reload()`
/// (`resource-loader.ts:525`, `:531-535` @v0.83.0). This is the field
/// `cyrup-session-svc/src/builder.rs` must consume as the FALLBACK for `custom_prompt` /
/// `append_system_prompt` (the CLI flags take precedence, per Pi's `??`).
#[tokio::test]
async fn cfg035_discovery_report_carries_the_discovered_prompt_overrides() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let global = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup")).unwrap();
    fs::create_dir_all(&global).unwrap();
    fs::write(cwd.join(".cyrup/SYSTEM.md"), "project system").unwrap();
    fs::write(global.join("APPEND_SYSTEM.md"), "global append").unwrap();

    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.trusted_project = true;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert_eq!(
        report.system_prompt_file,
        Some(cwd.join(".cyrup/SYSTEM.md"))
    );
    assert_eq!(
        report.append_system_prompt_file,
        Some(global.join("APPEND_SYSTEM.md"))
    );

    cfg.trusted_project = false;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert_eq!(
        report.system_prompt_file, None,
        "an untrusted project's SYSTEM.md must not reach the prompt"
    );
    assert_eq!(
        report.append_system_prompt_file,
        Some(global.join("APPEND_SYSTEM.md")),
        "the global tier is not trust-gated"
    );
}
