//! Package manifest resolution (cyrup.toml, package.json `pi` key, auto-discovery), glob entries
//! and override patterns at file and plain-dir granularity (A-09-4, G2, G3, G6).

use std::fs;

use super::fixtures::{cfg, full_theme_json, make_package_tree, run_discover, skill_md, write};
use crate::{
    DiscoveryConfig, InstallScope, InstalledPackage, InstalledPackages, PackageSource,
    PackageStore, PinRef, resolve_manifest,
};

// ===========================================================================
// A-09-4 — package manifest (cyrup.toml) + pi-key + auto-discovery
// ===========================================================================

#[test]
fn a09_4_manifest_resolution_toml_pi_and_autodiscover() {
    // cyrup.toml
    let t1 = tempfile::tempdir().unwrap();
    make_package_tree(t1.path(), true, false);
    let m = resolve_manifest(t1.path()).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::CyrupToml);
    assert_eq!(m.skills.len(), 1);
    assert_eq!(m.prompts.len(), 1);
    assert_eq!(m.themes.len(), 1);
    assert_eq!(m.extensions.len(), 1);

    // package.json `pi` key
    let t2 = tempfile::tempdir().unwrap();
    make_package_tree(t2.path(), true, true);
    let m = resolve_manifest(t2.path()).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::PackageJson);
    assert_eq!(m.themes.len(), 1);

    // auto-discovery (no manifest)
    let t3 = tempfile::tempdir().unwrap();
    make_package_tree(t3.path(), false, false);
    let m = resolve_manifest(t3.path()).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::AutoDiscovered);
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
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("alpha"),
        "package skill surfaces"
    );
    assert!(
        report.registry.prompts.contains("greet"),
        "package prompt surfaces"
    );
    assert!(
        report.registry.themes.contains("midnight"),
        "package theme surfaces"
    );
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy")),
        "package extension crate path handed off to cyrup-ext"
    );
}

#[tokio::test]
async fn installed_global_git_package_resolves_via_package_global_dir() {
    // A Global-scope git-source package's working tree lives at `<package_global_dir>/packages/<id>`
    // — the bin passes `dirs.package_dir` (default `<agent_dir>/packages`) as the store root
    // (subcommands.rs:396), so the tree is one level deeper than a naive `<global_dir>/packages/<id>`
    // guess. Discovery must resolve it via `package_global_dir`, NOT `global_dir` (the loose-resource
    // agent root), else an installed Global git/oci package's resources never surface — the base half
    // of C1 (gap-07 #1 / gap-13 C1). Path installs bypass this (they carry an absolute source path),
    // so only git/oci sources exercise the base; here we replicate the on-disk state without a real
    // clone (discovery only READS the tree, exactly like a09_5_update_skips_pinned seeds git records).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let global = root.join("global");
    let package_dir = global.join("packages"); // the bin's default `dirs.package_dir`.
    fs::create_dir_all(&global).unwrap();

    let source = PackageSource::Git {
        url: "file:///fake/pkg".into(),
        reff: PinRef::Default,
    };
    let id = source.package_id();
    let store = PackageStore::new(package_dir.clone(), None);
    let pkg_tree = store.package_dir(InstallScope::Global, &id).unwrap();
    make_package_tree(&pkg_tree, true, false);

    // `DiscoveryConfig::new(root, &global)` defaults `package_global_dir` to `global/packages`
    // (== the store root above), so a Global package resolves out of the box.
    let mut c = DiscoveryConfig::new(root, &global);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id,
            source,
            scope: InstallScope::Global,
            resolved_commit: Some("deadbeef".into()),
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("alpha"),
        "Global git package skill resolves via package_global_dir"
    );
    assert!(
        report.registry.themes.contains("midnight"),
        "Global git package theme resolves"
    );
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy")),
        "Global git package extension dir collected"
    );

    // Negative control: pointing the base at the loose-resource `global_dir` (agent root) resolves
    // `<global_dir>/packages/<id>` — the WRONG, one-level-too-shallow path — so nothing is found.
    // Proves the resolution genuinely keys off `package_global_dir`, not `global_dir`.
    let mut wrong = c.clone();
    wrong.package_global_dir = global.clone();
    assert!(
        !run_discover(&wrong).await.registry.skills.contains("alpha"),
        "resolving a Global package via global_dir misses its tree — the package_global_dir base matters"
    );
}

// ===========================================================================
// package.json manifest entries are globs (G6)
// ===========================================================================

#[test]
fn manifest_glob_entries_expand_to_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(&dir.join("themes/a.json"), &full_theme_json("a", &[], &[]));
    write(&dir.join("themes/b.json"), &full_theme_json("b", &[], &[]));
    // A glob entry resolves via collectFilesFromManifestEntries (package-manager.ts:2201-2215).
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    assert_eq!(m.kind, crate::package::ManifestKind::PackageJson);
    assert_eq!(m.themes.len(), 2, "glob expands to both theme files");
    assert!(
        m.themes
            .iter()
            .all(|p| p.extension().is_some_and(|e| e == "json"))
    );
}

#[tokio::test]
async fn manifest_glob_themes_surface_in_discovery() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(
        &pkg_dir.join("themes/midnight.json"),
        &full_theme_json("midnight", &[], &[]),
    );
    write(
        &pkg_dir.join("themes/noon.json"),
        &full_theme_json("noon", &[], &[]),
    );
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.themes.contains("midnight"),
        "glob-matched theme file surfaces"
    );
    assert!(
        report.registry.themes.contains("noon"),
        "second glob-matched theme file surfaces"
    );
}

// ===========================================================================
// Manifest override patterns: `!` exclude, `+` force-include, `-` force-exclude (G2)
// ===========================================================================

#[test]
fn manifest_override_exclude_drops_matching_source() {
    // A `!` pattern excludes matching members of the resolved source set instead of being treated
    // as a literal source path that matches nothing (package-manager.ts:2154-2159, 696-717).
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        &dir.join("skills/public/SKILL.md"),
        &skill_md("public", "public skill"),
    );
    write(
        &dir.join("skills/internal/SKILL.md"),
        &skill_md("internal", "internal skill"),
    );
    write(
        &dir.join("skills/secret/SKILL.md"),
        &skill_md("secret", "secret skill"),
    );
    // Source glob enumerates all three; `!skills/internal` excludes one by its SKILL.md parent dir.
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills/*/SKILL.md","!skills/internal"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    let names: Vec<String> = m
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("skills/public/SKILL.md")),
        "public kept: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("skills/secret/SKILL.md")),
        "secret kept: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("skills/internal/SKILL.md")),
        "internal excluded by `!`: {names:?}"
    );
}

#[test]
fn manifest_override_force_include_and_force_exclude() {
    // `!` excludes all, `+` force-includes one back (overriding the exclude), `-` force-excludes
    // another even though it would otherwise be present (package-manager.ts:718-771).
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(&dir.join("skills/a/SKILL.md"), &skill_md("a", "skill a"));
    write(&dir.join("skills/b/SKILL.md"), &skill_md("b", "skill b"));
    write(&dir.join("skills/c/SKILL.md"), &skill_md("c", "skill c"));
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills/*/SKILL.md","!skills/*","+skills/b","-skills/c"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    let names: Vec<String> = m
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("skills/b/SKILL.md")),
        "b force-included: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("skills/a/SKILL.md")),
        "a stays excluded: {names:?}"
    );
    assert!(
        !names.iter().any(|n| n.ends_with("skills/c/SKILL.md")),
        "c force-excluded: {names:?}"
    );
}

#[tokio::test]
async fn manifest_override_patterns_surface_filtered_set_in_discovery() {
    // End-to-end: an installed package whose manifest uses `!` must materialize only the enabled
    // resources in discovery (package-manager.ts:2144-2163 collectManifestFiles enabledByManifest).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(
        &pkg_dir.join("themes/keep.json"),
        &full_theme_json("keep", &[], &[]),
    );
    write(
        &pkg_dir.join("themes/drop.json"),
        &full_theme_json("drop", &[], &[]),
    );
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"themes":["themes/*.json","!themes/drop.json"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.themes.contains("keep"),
        "non-excluded theme surfaces"
    );
    assert!(
        !report.registry.themes.contains("drop"),
        "`!themes/drop.json` excludes the theme from discovery"
    );
}

// ===========================================================================
// G3 — manifest override patterns filter at resolved-file granularity (plain dirs)
// ===========================================================================

#[test]
fn manifest_plain_dir_override_excludes_subdir() {
    // A PLAIN DIRECTORY source entry plus `!skills/internal`: Pi expands `skills/` to its SKILL.md
    // files via collectFilesFromPaths BEFORE applyPatterns (package-manager.ts:2201-2215), so the
    // exclude drops `skills/internal/SKILL.md`. (Previously cyrup applied the `!` to the raw dir
    // entry, which matched nothing.)
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    write(
        &dir.join("skills/public/SKILL.md"),
        &skill_md("public", "public skill"),
    );
    write(
        &dir.join("skills/internal/SKILL.md"),
        &skill_md("internal", "internal skill"),
    );
    write(
        &dir.join("skills/secret/SKILL.md"),
        &skill_md("secret", "secret skill"),
    );
    write(
        &dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills","!skills/internal"]}}"#,
    );
    let m = resolve_manifest(dir).unwrap();
    let names: Vec<String> = m
        .skills
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("skills/public/SKILL.md")),
        "public kept: {names:?}"
    );
    assert!(
        names.iter().any(|n| n.ends_with("skills/secret/SKILL.md")),
        "secret kept: {names:?}"
    );
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("skills/internal/SKILL.md")),
        "internal dropped by `!skills/internal` after dir expansion: {names:?}"
    );
}

#[tokio::test]
async fn manifest_plain_dir_override_surfaces_filtered_skills_in_discovery() {
    // End-to-end: the directory-expanded + override-filtered skills surface in discovery (the
    // excluded one does not).
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let pkg_dir = root.join("pkgtree");
    write(
        &pkg_dir.join("skills/keep/SKILL.md"),
        &skill_md("keepme", "kept skill"),
    );
    write(
        &pkg_dir.join("skills/internal/SKILL.md"),
        &skill_md("dropme", "internal skill"),
    );
    write(
        &pkg_dir.join("package.json"),
        r#"{"name":"pack","pi":{"skills":["skills","!skills/internal"]}}"#,
    );

    let mut c = cfg(root);
    c.installed = InstalledPackages {
        packages: vec![InstalledPackage {
            id: cyrup_core::PackageId::from("path:pkg".to_string()),
            source: PackageSource::Path {
                path: pkg_dir.clone(),
            },
            scope: InstallScope::Global,
            resolved_commit: None,
            installed_at: "0".to_string(),
            disabled: Default::default(),
        }],
    };
    let report = run_discover(&c).await;
    assert!(
        report.registry.skills.contains("keepme"),
        "kept skill surfaces"
    );
    assert!(
        !report.registry.skills.contains("dropme"),
        "excluded skill is filtered out"
    );
}
