//! Conformance tests for cyrup-resources (A-09-1..10, func-09).
//!
//! Tempdir fixtures only; no network. The git-clone path is exercised against a LOCAL git repo
//! created in-test (skipped gracefully if the `git` CLI is unavailable); local-path install +
//! manifest parsing + pin/update are exercised unconditionally.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cyrup_core::CancelToken;
use cyrup_resources::package::lock;
use cyrup_resources::{
    DiscoveryConfig, InstallScope, InstalledPackage, InstalledPackages, PackageManager,
    PackageSource, PackageStore, PinRef, PlaceholderArgs, ResourceHandle, ResourceScope,
    ResourceSelector, SECURITY_CAVEAT, Skill, Theme, ThemeWatcher, UpdateTarget, builtin_themes,
    discover, resolve_manifest,
};

// ---------------------------------------------------------------------------
// fixture helpers
// ---------------------------------------------------------------------------

fn write(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn skill_md(name: &str, description: &str) -> String {
    format!("---\nname: {name}\ndescription: {description}\n---\n# {name}\n\nDo the thing.\n")
}

/// Discovery config rooted at a temp dir, project untrusted by default.
fn cfg(root: &Path) -> DiscoveryConfig {
    let global = root.join("global");
    fs::create_dir_all(&global).unwrap();
    let mut c = DiscoveryConfig::new(root, &global);
    c.project_root = Some(root.to_path_buf());
    c.cwd = root.to_path_buf();
    c
}

async fn run_discover(c: &DiscoveryConfig) -> cyrup_resources::DiscoveryReport {
    discover(c, CancelToken::new()).await.unwrap()
}

// ===========================================================================
// A-09-1 / A-09-10 — SKILL.md discovery, pointer, /skill:name, trust gate, standard format
// ===========================================================================

#[tokio::test]
async fn a09_1_skill_discovery_pointer_command_and_trust_gate() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join(".cyrup/skills/foo/SKILL.md"), &skill_md("foo", "use when frobbing"));

    // Untrusted project: the project-scoped skill is hidden (R-09-003, fail-closed).
    let mut c = cfg(root);
    c.trusted_project = false;
    let untrusted = run_discover(&c).await;
    assert!(!untrusted.registry.skills.contains("foo"), "untrusted project must hide skills");

    // Trusted project: the skill surfaces with a pointer + /skill:foo command.
    c.trusted_project = true;
    let report = run_discover(&c).await;
    let skill = report.registry.skills.get_name("foo").expect("foo skill discovered");
    assert_eq!(skill.scope, ResourceScope::Project);
    assert_eq!(skill.command(), "/skill:foo");

    let pointer = skill.pointer();
    assert_eq!(pointer.name, "foo");
    assert_eq!(pointer.description.as_deref(), Some("use when frobbing"));
    assert!(pointer.path.ends_with("SKILL.md"), "pointer points at SKILL.md for on-demand read");
}

#[tokio::test]
async fn a09_10_agent_skills_standard_format_loads_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    // A SKILL.md authored to the Agent Skills standard (extra unmodelled keys + allowed-tools).
    let md = "---\n\
name: pdf-processing\n\
description: Use this skill when working with PDF files\n\
allowed-tools:\n  - read\n  - bash\n\
license: MIT\n\
---\n\
# PDF Processing\n\nSteps to process a PDF.\n";
    let p = tmp.path().join("skills/pdf/SKILL.md");
    write(&p, md);

    let skill = Skill::load(&p, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
        .expect("standard SKILL.md loads unchanged");
    assert_eq!(skill.front.name, "pdf-processing");
    assert_eq!(skill.front.allowed_tools, vec!["read".to_string(), "bash".to_string()]);
    assert!(skill.front.extra.contains_key("license"), "unmodelled keys round-trip via extra");

    // Body is lazily read (R-09-026) and excludes the front-matter.
    let body = skill.read_body().await.unwrap();
    assert!(body.contains("Steps to process a PDF."));
    assert!(!body.contains("pdf-processing"), "front-matter excluded from body");
}

// ===========================================================================
// A-09-2 — prompt template /name expansion, placeholders, disable
// ===========================================================================

#[tokio::test]
async fn a09_2_prompt_expansion_placeholders_and_disable() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(
        &root.join("global/prompts/review.md"),
        "Please review with focus {{focus}} and ignore {{skip}}.",
    );

    let mut c = cfg(root);
    let report = run_discover(&c).await;
    let tmpl = report.registry.prompts.get_name("review").expect("/review template discovered");
    assert!(tmpl.placeholders.contains(&"focus".to_string()));
    assert!(tmpl.placeholders.contains(&"skip".to_string()));

    // Expansion substitutes provided args; unknown placeholders stay literal + reported.
    let args: PlaceholderArgs = [("focus", "perf")].into_iter().collect();
    let exp = tmpl.expand(&args);
    assert_eq!(exp.text, "Please review with focus perf and ignore {{skip}}.");
    assert_eq!(exp.unresolved, vec!["skip".to_string()]);

    // --no-prompt-templates disables discovery (R-09-010).
    c.enable_prompts = false;
    let disabled = run_discover(&c).await;
    assert!(!disabled.registry.prompts.contains("review"), "--no-prompt-templates disables it");
}

// ===========================================================================
// A-09-3 — themes: built-in dark/light, hot-reload, runtime switch
// ===========================================================================

#[tokio::test]
async fn a09_3_builtin_dark_and_light_present() {
    let builtins = builtin_themes();
    assert!(builtins.iter().any(|t| t.data.name == "dark"), "built-in dark exists (R-09-011)");
    assert!(builtins.iter().any(|t| t.data.name == "light"), "built-in light exists (R-09-011)");

    let tmp = tempfile::tempdir().unwrap();
    let c = cfg(tmp.path());
    let report = run_discover(&c).await;
    assert!(report.registry.themes.contains("dark"));
    assert!(report.registry.themes.contains("light"));
}

#[tokio::test]
async fn a09_3_theme_disable_flag() {
    let tmp = tempfile::tempdir().unwrap();
    let mut c = cfg(tmp.path());
    c.enable_themes = false;
    let report = run_discover(&c).await;
    assert!(!report.registry.themes.contains("dark"), "--no-themes drops built-ins too");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a09_3_theme_hot_reload_and_runtime_switch() {
    let tmp = tempfile::tempdir().unwrap();
    let active = tmp.path().join("active.json");
    write(&active, r##"{"name":"mine","vars":{"bg":"#000000"},"colors":{"background":"$bg"}}"##);

    let theme = Theme::load(&active, ResourceScope::Cli, cyrup_resources::ResourceOrigin::Builtin)
        .unwrap();
    let watcher = ThemeWatcher::spawn(
        std::sync::Arc::new(theme.data.clone()),
        active.clone(),
        CancelToken::new(),
    )
    .expect("theme watcher spawns");
    let mut rx = watcher.subscribe();
    assert_eq!(rx.borrow_and_update().name, "mine");

    // Mutate the active theme file; the watcher must publish the new theme (R-09-013).
    tokio::time::sleep(Duration::from_millis(120)).await;
    write(&active, r##"{"name":"mine","vars":{"bg":"#ffffff"},"colors":{"background":"$bg"}}"##);

    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("hot-reload fired before timeout")
        .expect("watch channel open");
    assert_eq!(rx.borrow().vars.get("bg").map(String::as_str), Some("#ffffff"));

    // Runtime switch to a different theme file (R-09-014).
    let other = tmp.path().join("other.json");
    write(&other, r##"{"name":"other","vars":{},"colors":{"foreground":"#abcdef"}}"##);
    watcher.retarget(other).expect("retarget to a new active theme");
    tokio::time::timeout(Duration::from_secs(5), rx.changed())
        .await
        .expect("retarget published before timeout")
        .expect("watch channel open");
    assert_eq!(rx.borrow().name, "other");
}

#[test]
fn theme_resolve_var_indirection_and_bad_hex() {
    let theme = Theme::parse(
        r##"{"name":"t","vars":{"bg":"#112233"},"colors":{"background":"$bg","bad":"nothex","blank":""}}"##,
        None,
        ResourceScope::Builtin,
        cyrup_resources::ResourceOrigin::Builtin,
    )
    .unwrap();
    let resolved = theme.resolve();
    assert_eq!(
        resolved.roles.get("background"),
        Some(&cyrup_resources::ColorSpec::Rgb { r: 0x11, g: 0x22, b: 0x33 })
    );
    assert_eq!(resolved.roles.get("bad"), Some(&cyrup_resources::ColorSpec::Inherit));
    assert_eq!(resolved.roles.get("blank"), Some(&cyrup_resources::ColorSpec::Inherit));
}

// ===========================================================================
// A-09-4 — package manifest (cyrup.toml) + pi-key + auto-discovery
// ===========================================================================

fn make_package_tree(dir: &Path, with_manifest: bool, pi_key: bool) {
    write(&dir.join("skills/alpha/SKILL.md"), &skill_md("alpha", "alpha skill"));
    write(&dir.join("prompts/greet.md"), "Hello {{who}}");
    write(&dir.join("themes/midnight.json"), r#"{"name":"midnight","vars":{},"colors":{}}"#);
    fs::create_dir_all(dir.join("extensions/deploy")).unwrap();
    if with_manifest && !pi_key {
        write(
            &dir.join("cyrup.toml"),
            "[package]\nname = \"pack\"\nversion = \"0.1.0\"\n\n\
             [resources]\nextensions = [\"./extensions/deploy\"]\nskills = [\"./skills\"]\n\
             prompts = [\"./prompts\"]\nthemes = [\"./themes\"]\n",
        );
    } else if with_manifest && pi_key {
        write(
            &dir.join("package.json"),
            r#"{"name":"pack","keywords":["pi-package"],"pi":{"extensions":["./extensions/deploy"],"skills":["./skills"],"prompts":["./prompts"],"themes":["./themes"]}}"#,
        );
    }
}

#[test]
fn a09_4_manifest_resolution_toml_pi_and_autodiscover() {
    // cyrup.toml
    let t1 = tempfile::tempdir().unwrap();
    make_package_tree(t1.path(), true, false);
    let m = resolve_manifest(t1.path()).unwrap();
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::CyrupToml);
    assert_eq!(m.skills.len(), 1);
    assert_eq!(m.prompts.len(), 1);
    assert_eq!(m.themes.len(), 1);
    assert_eq!(m.extensions.len(), 1);

    // package.json `pi` key
    let t2 = tempfile::tempdir().unwrap();
    make_package_tree(t2.path(), true, true);
    let m = resolve_manifest(t2.path()).unwrap();
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::PackageJson);
    assert_eq!(m.themes.len(), 1);

    // auto-discovery (no manifest)
    let t3 = tempfile::tempdir().unwrap();
    make_package_tree(t3.path(), false, false);
    let m = resolve_manifest(t3.path()).unwrap();
    assert_eq!(m.kind, cyrup_resources::package::ManifestKind::AutoDiscovered);
    assert_eq!(m.skills, vec![t3.path().join("skills")]);
    assert_eq!(m.extensions, vec![t3.path().join("extensions")]);
}

#[tokio::test]
async fn a09_4_installed_package_resources_surface_in_discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    make_package_tree(&pkg_dir, true, false);

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path { path: pkg_dir.clone() },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(report.registry.skills.contains("alpha"), "package skill surfaces");
    assert!(report.registry.prompts.contains("greet"), "package prompt surfaces");
    assert!(report.registry.themes.contains("midnight"), "package theme surfaces");
    assert!(
        report.registry.ext_crate_paths.iter().any(|p| p.ends_with("deploy")),
        "package extension crate path handed off to cyrup-ext"
    );
}

// ===========================================================================
// A-09-5 — install / remove / list / update / pin (local path + git fixture)
// ===========================================================================

#[tokio::test]
async fn a09_5_local_path_install_list_remove() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let pkg = tmp.path().join("mypkg");
    make_package_tree(&pkg, true, false);

    let mgr = PackageManager::new(PackageStore::new(global, None));
    let (rec, notice) = mgr
        .install(PackageSource::Path { path: pkg.clone() }, InstallScope::Global, true, CancelToken::new())
        .await
        .expect("local-path install");
    assert_eq!(notice.message, SECURITY_CAVEAT, "security caveat surfaced (R-09-019)");

    let listed = mgr.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, rec.id);

    // enable/disable a resource within the package (R-09-018).
    mgr.set_enabled(&rec.id, ResourceSelector::Skill("alpha".to_string()), false).unwrap();
    let after = mgr.list();
    assert!(after[0].disabled.skills.contains("alpha"));

    mgr.remove(&rec.id).await.expect("remove");
    assert!(mgr.list().is_empty(), "list empty after remove");
}

#[tokio::test]
async fn a09_5_update_skips_pinned_and_one_moves_it() {
    // Construct two git installs directly in the registry: one Default (bulk-updatable), one
    // Tag-pinned (bulk-skipped). No network: refresh re-reads HEAD from a (possibly absent) local
    // clone and degrades gracefully.
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let store = PackageStore::new(global.clone(), None);

    let default_src = PackageSource::Git { url: "file:///x/a".into(), reff: PinRef::Default };
    let tag_src = PackageSource::Git { url: "file:///x/b".into(), reff: PinRef::Tag("v1".into()) };
    let default_id = default_src.package_id();
    let tag_id = tag_src.package_id();
    assert!(tag_src.pin().is_pinned());
    assert!(!default_src.pin().is_pinned());

    let reg = InstalledPackages {
        packages: vec![
            InstalledPackage {
                id: default_id.clone(),
                source: default_src,
                scope: InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            },
            InstalledPackage {
                id: tag_id.clone(),
                source: tag_src,
                scope: InstallScope::Global,
                resolved_commit: None,
                installed_at: "0".to_string(),
                disabled: Default::default(),
            },
        ],
    };
    let reg_path = store.registry_path(InstallScope::Global).unwrap();
    lock::save(&reg_path, &reg).unwrap();

    let mgr = PackageManager::new(store);
    let report = mgr.update(UpdateTarget::All, CancelToken::new()).await.unwrap();
    assert!(report.skipped_pinned.contains(&tag_id), "bulk update skips pinned (R-09-020)");
    assert!(report.updated.contains(&default_id), "bulk update moves unpinned");
    assert!(!report.updated.contains(&tag_id));

    // update(One) moves the pinned package regardless.
    let one = mgr.update(UpdateTarget::One(tag_id.clone()), CancelToken::new()).await.unwrap();
    assert!(one.updated.contains(&tag_id), "explicit update(One) moves a pinned package");
}

#[tokio::test]
async fn a09_5_git_local_fixture_install() {
    let Some(repo) = make_local_git_repo() else {
        // git CLI unavailable: git-clone is fixture-gated (§7.6); the local-path + registry-driven
        // update paths above already cover install/list/remove/update/pin.
        eprintln!("skipping git-fixture install: `git` CLI not available");
        return;
    };
    let (_tmp, repo_dir) = repo;

    let global = tempfile::tempdir().unwrap();
    let mgr = PackageManager::new(PackageStore::new(global.path().to_path_buf(), None));
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git { url: repo_dir.display().to_string(), reff: PinRef::Default },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install from local git repo");
    assert!(rec.resolved_commit.is_some(), "HEAD commit resolved via gix (no network)");
    assert_eq!(mgr.list().len(), 1);
}

/// Create a real local git repo with one commit. Returns the tempdir (kept alive) + repo path, or
/// None if the `git` CLI is unavailable.
fn make_local_git_repo() -> Option<(tempfile::TempDir, PathBuf)> {
    use std::process::Command;
    let tmp = tempfile::tempdir().ok()?;
    let dir = tmp.path().to_path_buf();
    make_package_tree(&dir, true, false);
    let git = |args: &[&str]| -> bool {
        Command::new("git")
            .current_dir(&dir)
            .args(args)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    };
    if !git(&["init", "-q"]) {
        return None;
    }
    git(&["config", "user.email", "t@t"]);
    git(&["config", "user.name", "t"]);
    if !git(&["add", "-A"]) {
        return None;
    }
    if !git(&["commit", "-q", "-m", "init"]) {
        return None;
    }
    Some((tmp, dir))
}

// ===========================================================================
// A-09-6 — project-local scope trust gate + security caveat
// ===========================================================================

#[tokio::test]
async fn a09_6_project_install_trust_gated_with_security_notice() {
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let project = tmp.path().join("proj");
    let pkg = tmp.path().join("pkg");
    make_package_tree(&pkg, true, false);
    let mgr = PackageManager::new(PackageStore::new(global, Some(project)));

    // Untrusted project install is refused (R-09-017).
    let err = mgr
        .install(PackageSource::Path { path: pkg.clone() }, InstallScope::Project, false, CancelToken::new())
        .await
        .expect_err("untrusted project install must be refused");
    assert!(matches!(err, cyrup_resources::ResourceError::Untrusted(_)));

    // Trusted project install succeeds and still surfaces the security caveat (R-09-019).
    let (_rec, notice) = mgr
        .install(PackageSource::Path { path: pkg }, InstallScope::Project, true, CancelToken::new())
        .await
        .expect("trusted project install");
    assert_eq!(notice.message, SECURITY_CAVEAT);
}

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
    let skill = report.registry.skills.get_name("contrib").expect("contributed skill available");
    assert_eq!(skill.scope, ResourceScope::Discovered, "enters at Discovered scope (R-09-022)");
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
    write(&root.join(".cyrup/skills/late/SKILL.md"), &skill_md("late", "added at runtime"));
    let second = run_discover(&c).await;
    handle.store(std::sync::Arc::new(second.registry));
    assert!(handle.load().skills.contains("late"), "/reload re-discovers new skill");
}

// ===========================================================================
// A-09-9 — same-name precedence (built-in -> global -> project, later wins)
// ===========================================================================

#[tokio::test]
async fn a09_9_same_name_precedence_project_beats_global() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write(&root.join("global/skills/dup/SKILL.md"), &skill_md("dup", "global one"));
    write(&root.join(".cyrup/skills/dup/SKILL.md"), &skill_md("dup", "project one"));

    let mut c = cfg(root);
    c.trusted_project = true;
    let report = run_discover(&c).await;

    let winner = report.registry.skills.get_name("dup").expect("dup present");
    assert_eq!(winner.scope, ResourceScope::Project, "project wins over global (R-09-024)");
    assert_eq!(winner.front.description.as_deref(), Some("project one"));

    // Both candidates retained for diagnostics.
    let count = report.registry.skills.all().iter().filter(|s| s.key.as_str() == "dup").count();
    assert_eq!(count, 2, "shadowed candidate retained in `all`");
}

#[tokio::test]
async fn a09_9_theme_precedence_global_overrides_builtin() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    // A loose global theme named `dark` overrides the built-in `dark`.
    write(
        &root.join("global/themes/dark.json"),
        r##"{"name":"dark","vars":{"bg":"#010101"},"colors":{"background":"$bg"}}"##,
    );
    let c = cfg(root);
    let report = run_discover(&c).await;
    let dark = report.registry.themes.get_name("dark").expect("dark present");
    assert_eq!(dark.scope, ResourceScope::Global, "global theme overrides built-in");
}
