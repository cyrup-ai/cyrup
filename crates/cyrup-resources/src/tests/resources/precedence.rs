//! Resource precedence-rank model — 1:1 with Pi `resourcePrecedenceRank`
//! (package-manager.ts:172-188) — and the same-name first-wins dedup it drives (A-09-9, G1).

use std::path::Path;

use super::fixtures::{cfg, full_theme_json, run_discover, skill_md, write};
use crate::{
    InstallScope, InstalledPackage, InstalledPackages, PackageSource, ResourceOverrides,
    ResourceScope,
};

// ===========================================================================
// A-09-9 — same-name precedence (built-in -> global -> project, later wins)
// ===========================================================================

#[tokio::test]
async fn a09_9_same_name_precedence_project_beats_global() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global one"),
    );
    write(
        &root.join(".cyrup/skills/dup/SKILL.md"),
        &skill_md("dup", "project one"),
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    let report = run_discover(&c).await;

    let winner = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        winner.scope,
        ResourceScope::Project,
        "project wins over global (R-09-024)"
    );
    assert_eq!(winner.front.description.as_deref(), Some("project one"));

    // Both candidates retained for diagnostics.
    let count = report
        .registry
        .skills
        .all()
        .iter()
        .filter(|s| s.key.as_str() == "dup")
        .count();
    assert_eq!(count, 2, "shadowed candidate retained in `all`");
}

#[tokio::test]
async fn a09_9_theme_precedence_global_overrides_builtin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A loose global theme named `dark` overrides the built-in `dark`.
    write(
        &root.join("global/themes/dark.json"),
        &full_theme_json("dark", &[("bg", "#010101")], &[("background", "$bg")]),
    );
    let c = cfg(root);
    let report = run_discover(&c).await;
    let dark = report
        .registry
        .themes
        .get_name("dark")
        .expect("dark present");
    assert_eq!(
        dark.scope,
        ResourceScope::Global,
        "global theme overrides built-in"
    );
}

// ===========================================================================
// G1 — resource precedence-rank model (1:1 with Pi `resourcePrecedenceRank`,
// package-manager.ts:172-188): lower rank wins under first-wins same-name dedup.
//   0 project+settings  1 project+auto  2 user+settings  3 user+auto  4 ANY package.
//   The explicit --skill/--prompt-template/--theme CLI tier is appended AFTER the sorted
//   accumulator (resource-loader.ts:421/436/455), so it loses to every package — modeled rank 5.
//   cyrup-only tiers: Discovered 6, Builtin 7.
// ===========================================================================

/// An installed package backed by a path tree containing a single named skill.
fn pkg_with_skill(
    dir: &Path,
    id: &str,
    scope: InstallScope,
    name: &str,
    desc: &str,
) -> InstalledPackage {
    write(
        &dir.join(format!("skills/{name}/SKILL.md")),
        &skill_md(name, desc),
    );
    InstalledPackage {
        id: cyrup_core::PackageId::from(id.to_string()),
        source: PackageSource::Path {
            path: dir.to_path_buf(),
        },
        scope,
        resolved_commit: None,
        installed_at: "0".to_string(),
        disabled: Default::default(),
    }
}

#[test]
fn g1_precedence_rank_matches_pi() {
    // package-manager.ts:184-188 — exact rank table; lower wins.
    assert_eq!(ResourceScope::ProjectSettings.precedence_rank(), 0);
    assert_eq!(ResourceScope::Project.precedence_rank(), 1);
    assert_eq!(ResourceScope::GlobalSettings.precedence_rank(), 2);
    assert_eq!(ResourceScope::Global.precedence_rank(), 3);
    assert_eq!(
        ResourceScope::ProjectPackage.precedence_rank(),
        4,
        "all packages rank 4"
    );
    assert_eq!(ResourceScope::GlobalPackage.precedence_rank(), 4);
    assert_eq!(
        ResourceScope::Cli.precedence_rank(),
        5,
        "CLI appended after the sorted accumulator (resource-loader.ts:421), so it loses to packages"
    );
    assert_eq!(
        ResourceScope::Discovered.precedence_rank(),
        6,
        "[CYRUP-DELTA] below CLI + packages"
    );
    assert_eq!(
        ResourceScope::Builtin.precedence_rank(),
        7,
        "[CYRUP-DELTA] lowest"
    );
}

#[tokio::test]
async fn g1_cli_loses_to_project_resource() {
    // Pi appends the explicit `--skill` paths after the sorted accumulator (resource-loader.ts:421),
    // so under first-wins a project resource (rank 1) wins the same-name collision over the CLI path
    // (rank 5) (package-manager.ts:184-188).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join(".cyrup/skills/dup/SKILL.md"),
        &skill_md("dup", "project one"),
    );
    let cli = root.join("cli/dup/SKILL.md");
    write(&cli, &skill_md("dup", "cli one"));

    let mut c = cfg(root);
    c.trusted_project = true;
    c.cli.skills = vec![cli];
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::Project,
        "project (rank 1) beats CLI (rank 5)"
    );
    assert_eq!(w.front.description.as_deref(), Some("project one"));
}

#[tokio::test]
async fn g1_cli_loses_to_global_auto_and_to_package() {
    // Pi appends the explicit `--skill` paths (`additionalSkillPaths`) AFTER the entire sorted
    // accumulator — `mergePaths([...cliEnabledSkills, ...enabledSkills], additionalSkillPaths)`
    // (resource-loader.ts:421) — so under first-wins the CLI path loses to a same-name resource of
    // ANY rank. Here a global auto-discovered loose file (rank 3) wins over the CLI path (rank 5).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global auto"),
    );
    let cli = root.join("cli/dup/SKILL.md");
    write(&cli, &skill_md("dup", "cli one"));

    let mut c = cfg(root);
    c.cli.skills = vec![cli.clone()];
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::Global,
        "global auto (rank 3) beats CLI (rank 5)"
    );
    assert_eq!(w.front.description.as_deref(), Some("global auto"));

    // Now collide the CLI path with a *package* (rank 4): the package wins, because Pi appends the
    // CLI path after the rank-4 package in `enabledSkills` (resource-loader.ts:421). CLI loses.
    let pkgdir = root.join("pkg");
    let pkg = pkg_with_skill(&pkgdir, "path:p", InstallScope::Global, "only", "pkg one");
    let clip = root.join("cli2/only/SKILL.md");
    write(&clip, &skill_md("only", "cli loses"));
    let mut c2 = cfg(root);
    c2.installed = InstalledPackages {
        packages: vec![pkg],
    };
    c2.cli.skills = vec![clip];
    let r2 = run_discover(&c2).await;
    let w2 = r2.registry.skills.get_name("only").expect("only present");
    assert_eq!(
        w2.scope,
        ResourceScope::GlobalPackage,
        "package (rank 4) beats CLI (rank 5)"
    );
    assert_eq!(w2.front.description.as_deref(), Some("pkg one"));
}

#[tokio::test]
async fn g1_global_loose_beats_project_package() {
    // Inversion (c): Pi puts ALL packages at rank 4, below user-auto (rank 3), so a global loose
    // file beats a project-local package on a same-name collision (package-manager.ts:185).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global loose"),
    );
    let pkgdir = root.join("ppkg");
    let pkg = pkg_with_skill(
        &pkgdir,
        "path:pp",
        InstallScope::Project,
        "dup",
        "project package",
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    c.installed = InstalledPackages {
        packages: vec![pkg],
    };
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::Global,
        "global loose (rank 3) beats project package (rank 4)"
    );
    assert_eq!(w.front.description.as_deref(), Some("global loose"));
}

#[tokio::test]
async fn g1_project_package_beats_global_package() {
    // Both packages rank 4; Pi inserts project-scope packages first (allPackages, 887-893), so the
    // project-local package wins the same-rank tie.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let gdir = root.join("gpkg");
    let pdir = root.join("ppkg");
    // Register global first to prove the project-first re-ordering (not input order) decides.
    let gpkg = pkg_with_skill(
        &gdir,
        "path:g",
        InstallScope::Global,
        "dup",
        "global package",
    );
    let ppkg = pkg_with_skill(
        &pdir,
        "path:p",
        InstallScope::Project,
        "dup",
        "project package",
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    c.installed = InstalledPackages {
        packages: vec![gpkg, ppkg],
    };
    let report = run_discover(&c).await;
    let w = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        w.scope,
        ResourceScope::ProjectPackage,
        "project package wins the rank-4 tie"
    );
    assert_eq!(w.front.description.as_deref(), Some("project package"));
}

#[tokio::test]
async fn g1_settings_entry_outranks_auto_in_same_scope() {
    // Inversion (b): a settings-listed plain entry (source:"local") outranks an auto-discovered
    // file of the same scope (package-manager.ts:184-188, resolveLocalEntries 2218-2239). Global:
    // rank 2 beats rank 3; project: rank 0 beats rank 1.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // Global: auto-discovered under global/skills, settings entry under global/extra.
    write(
        &root.join("global/skills/dup/SKILL.md"),
        &skill_md("dup", "global auto"),
    );
    write(
        &root.join("global/extra/dup/SKILL.md"),
        &skill_md("dup", "global settings"),
    );
    // Project: auto under .cyrup/skills, settings entry under .cyrup/extra (base = <cwd>/.cyrup).
    write(
        &root.join(".cyrup/skills/pdup/SKILL.md"),
        &skill_md("pdup", "project auto"),
    );
    write(
        &root.join(".cyrup/extra/pdup/SKILL.md"),
        &skill_md("pdup", "project settings"),
    );

    let mut c = cfg(root);
    c.trusted_project = true;
    c.global_overrides = ResourceOverrides {
        skills: vec!["extra/dup".to_string()],
        ..Default::default()
    };
    c.project_overrides = ResourceOverrides {
        skills: vec!["extra/pdup".to_string()],
        ..Default::default()
    };
    let report = run_discover(&c).await;

    let g = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(
        g.scope,
        ResourceScope::GlobalSettings,
        "global settings (rank 2) beats auto (rank 3)"
    );
    assert_eq!(g.front.description.as_deref(), Some("global settings"));

    let p = report
        .registry
        .skills
        .get_name("pdup")
        .expect("pdup present");
    assert_eq!(
        p.scope,
        ResourceScope::ProjectSettings,
        "project settings (rank 0) beats auto (rank 1)"
    );
    assert_eq!(p.front.description.as_deref(), Some("project settings"));
}

#[tokio::test]
async fn g1_settings_entry_loads_prompt_and_theme_positive_listings() {
    // resolveLocalEntries loads plain entries that are NOT in a conventional dir, across all three
    // resource families; a `!`-pattern entry in the same array still only filters (never loads).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/extra/hello.md"), "Say hi to {{who}}");
    write(
        &root.join("global/extra/cool.json"),
        &full_theme_json("cool", &[], &[]),
    );
    // A second prompt that the `!` pattern must keep filtering out of *auto-discovery*.
    write(&root.join("global/prompts/dropme.md"), "drop");

    let mut c = cfg(root);
    c.global_overrides = ResourceOverrides {
        prompts: vec![
            "extra/hello.md".to_string(),
            "!prompts/dropme.md".to_string(),
        ],
        themes: vec!["extra/cool.json".to_string()],
        ..Default::default()
    };
    let report = run_discover(&c).await;
    let pr = report
        .registry
        .prompts
        .get_name("hello")
        .expect("settings prompt loaded");
    assert_eq!(pr.scope, ResourceScope::GlobalSettings);
    let th = report
        .registry
        .themes
        .get_name("cool")
        .expect("settings theme loaded");
    assert_eq!(th.scope, ResourceScope::GlobalSettings);
    assert!(
        !report.registry.prompts.contains("dropme"),
        "`!` pattern still filters auto-discovery"
    );
}

#[tokio::test]
async fn g1_project_settings_entry_trust_gated() {
    // Project settings positive listings are trust-gated (fail-closed) like project loose files.
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join(".cyrup/extra/secret/SKILL.md"),
        &skill_md("secret", "project settings skill"),
    );

    let mut c = cfg(root);
    c.project_overrides = ResourceOverrides {
        skills: vec!["extra/secret".to_string()],
        ..Default::default()
    };

    c.trusted_project = false;
    assert!(
        !run_discover(&c).await.registry.skills.contains("secret"),
        "untrusted hides settings entry"
    );

    c.trusted_project = true;
    assert!(
        run_discover(&c).await.registry.skills.contains("secret"),
        "trusted surfaces settings entry"
    );
}
