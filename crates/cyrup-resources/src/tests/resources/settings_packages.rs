//! Settings-declared packages and local entries — filters, install attempts and their failure
//! diagnostics, trust gating, missing-path skips (CFG-003, CFG-004, CFG-027).

use std::fs;

use super::fixtures::{make_local_git_repo, make_package_tree, skill_md, write};
use crate::{
    DiagnosticType, DiscoveryConfig, InstallScope, PackageSource, PackageStore, PinRef,
    ResourceOverrides, discover,
};
use cyrup_core::CancelToken;

// ===========================================================================
// CFG-003 / CFG-004 — settings-declared packages + settings-declared local entries
// ===========================================================================

/// A package declared in settings (never installed) contributes its resources, and the object-form
/// per-type include filter (Pi `applyPackageFilter`, package-manager.ts:2147-2171) is honored.
#[tokio::test]
async fn cfg003_settings_declared_package_is_discovered_with_its_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    // No filter: everything the manifest declares loads.
    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(report.registry.skills.contains("alpha"));
    assert!(report.registry.skills.contains("beta"));
    assert!(report.registry.prompts.contains("greet"));
    assert!(report.registry.themes.contains("midnight"));
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy"))
    );

    // `skills: ["skills/alpha/**"]` selects alpha only; `themes: []` disables themes entirely.
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            skills: Some(vec!["skills/alpha/**".to_string()]),
            themes: Some(Vec::new()),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "the pattern keeps alpha"
    );
    assert!(
        !report.registry.skills.contains("beta"),
        "the pattern drops beta"
    );
    assert!(
        !report.registry.themes.contains("midnight"),
        "an explicitly EMPTY filter list disables the whole resource type"
    );
}

/// A settings-declared package that cyrup would have to INSTALL to resolve, on a pass whose
/// install gate is CLOSED, is a LOUD diagnostic — never a silent drop and never a failed discovery
/// pass.
///
/// The gate is [`DiscoveryConfig::install_missing_packages`], `false` here because this config is
/// left at its default — pi's `resolve(async () => "skip")` caller (cli/startup-ui.ts:73 @v0.83.0).
/// With it OPEN the package is cloned instead; that arm is
/// `cfg003_a_declared_git_package_is_cloned_when_the_install_gate_is_open`.
///
/// The source must be a git one. Pi installs an uninstalled npm/git source on demand
/// (`resolvePackageSources`, package-manager.ts:1287-1291 → `installMissing` `:1260-1271`
/// @v0.83.0), and where `installMissing` answers `false` pi `continue`s SILENTLY (`:1290`), so the
/// diagnostic is cyrup's documented `[CYRUP-DELTA]` for exactly that arm. A missing LOCAL path is a different
/// upstream path entirely and is silent — see
/// `cfg027_a_missing_local_package_path_is_a_silent_skip`.
///
/// The source string must carry a PROTOCOL. Pi's `parseGitUrl` bails at
/// `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) return null;` (git.ts:177-179
/// @v0.83.0), so a bare `github:org/pkg` shorthand — even though `isLocalPath` rejects it
/// (paths.ts:41-55) — falls out of `parseSource`'s git branch and lands on its terminal
/// `return { type: "local", path: source }` (package-manager.ts:1449-1459). Pi therefore treats
/// `github:org/pkg` as a LOCAL path and is SILENT about it, and so is cyrup; only a
/// protocol-qualified URL reaches the install arm this delta replaces.
#[tokio::test]
async fn cfg003_uninstallable_settings_declared_package_is_an_error_diagnostic() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: "https://github.com/org/absent-package".into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.message.contains("absent-package"))
        .expect("an uninstalled declared package must be reported");
    assert_eq!(d.diagnostic_type, DiagnosticType::Error);
    assert_eq!(d.resource_type, crate::ResourceKind::Package);
    // …and the gate held: nothing was fetched, so the message is the "run `cyrup install`" one and
    // not an install failure.
    // The remedy text names the source, so the literal is "run `cyrup install <source>`" — matching
    // on a closing backtick right after `install` can never fire.
    assert!(
        d.message.contains("run `cyrup install "),
        "a closed gate must not have attempted an install: {}",
        d.message
    );
    assert!(
        !d.message.contains("could not be installed"),
        "…and specifically must not report an install FAILURE: {}",
        d.message
    );
}

/// CFG-003 — a settings-declared GIT package with no working tree is CLONED during discovery when
/// the install gate is open, and its resources load in the same pass.
///
/// This is pi's session path verbatim: `ResourceLoader.reload()` calls `packageManager.resolve()`
/// with **no** `onMissing` (resource-loader.ts:403 @v0.83.0, and again at `:549` for
/// `loadCurrentExtensionSet`), so the git arm's `if (!existsSync(installedPath)) { const installed =
/// await installMissing(); if (!installed) continue; }` (package-manager.ts:1287-1291) reaches
/// `installMissing` (`:1260-1271`) with no callback to consult, which installs unconditionally
/// unless `isOfflineModeEnabled()` (`:42-46`). `installParsedSource` (`:1347-1356`) → `installGit`
/// (`:1820-1852`) then does `ensureGitIgnore(gitRoot)` (`:1831-1834`) and clones (`:1837`), and the
/// tree is walked by `collectPackageResources` (`:1296`) in that same `resolve()` call.
///
/// **Red before the fix:** `resolve_configured_package` resolved a git source ONLY through an
/// already-materialized `cyrup install` tree, so this exact config produced the
/// "…is not installed at this path — run `cyrup install`" diagnostic and zero skills. Both the
/// `skills.contains("alpha")` assertion and the `dir.is_dir()` one failed.
///
/// **Hermeticity, and why this test asserts a FAILED clone rather than a successful one.** The only
/// source spellings that reach the git arm are the ones `isLocalPath` calls non-local
/// (`npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:`, paths.ts:41-55 @v0.83.0) — a `file://` URL is
/// LOCAL to pi and to cyrup alike (`source.rs::is_local_path`), so it can never arrive here as a
/// git source no matter how the fixture is written. A settings string therefore cannot name a local
/// repository, and a successful clone through THIS entry point would need a real remote. `localhost`
/// keeps it on the loopback: `git:localhost/acme/pack` parses to
/// `Git { url: "https://localhost/acme/pack" }` (`parse_generic_git_url`'s bare `host/path` arm,
/// which accepts a dotted host or `localhost`) and the clone fails against a port nobody is serving.
/// The successful-clone half is `cfg003_install_declared_git_package_materializes_the_tree`.
/// (The one environmental assumption is that nothing serves a git repo on `localhost:443`; any
/// other outcome there is still a clone FAILURE, so the assertion holds either way. `git://` cannot
/// be used instead: `parseGitUrl` strips a leading `git:` before parsing (git.ts:172-179 @v0.83.0,
/// ported at `git_url.rs`), so `git://host/p` loses its scheme on both sides.)
///
/// **Red before the fix** on the message: with no install arm at all, a declared git package with
/// no tree produced "…is not installed at this path — run `cyrup install`" and nothing was ever
/// attempted. The assertion that the diagnostic says the INSTALL failed is what pins that the arm
/// is now entered.
#[tokio::test]
async fn cfg003_an_open_gate_attempts_the_install_and_reports_its_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    // A loose skill that must survive the failed package: pi throws out of `resolve()` here
    // (`installGit`'s `throw error`, package-manager.ts:1849 @v0.83.0) and loses the whole build;
    // cyrup's stated `[CYRUP-DELTA]` is to report and continue.
    write(
        &global.join("skills/solo/SKILL.md"),
        &skill_md("solo", "solo skill"),
    );

    let source = "git:localhost/acme/pack";
    let parsed = PackageSource::parse(source).unwrap();
    assert!(
        matches!(&parsed, PackageSource::Git { url, .. } if url == "https://localhost/acme/pack"),
        "precondition: the entry must reach the GIT arm, not the local one: {parsed:?}"
    );

    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.project_root = Some(cwd.clone());
    cfg.trusted_project = true;
    cfg.install_missing_packages = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: source.into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];

    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let d = report
        .diagnostics
        .iter()
        .find(|d| d.message.contains("acme/pack"))
        .expect("the declared package must be reported");
    assert_eq!(d.diagnostic_type, DiagnosticType::Error);
    assert!(
        d.message.contains("could not be installed"),
        "the gate was open, so an install was ATTEMPTED — the old \"run `cyrup install`\" text \
         means the arm was never entered: {}",
        d.message
    );
    assert!(
        report.registry.skills.contains("solo"),
        "a failed package install must not take the rest of the resource set down"
    );
    // `git_clone` stages and only renames on success — pi's `rmSync(targetDir, …)` guarantee
    // (`:1847`) arrived at differently. Nothing is left at the target path.
    let store = PackageStore::new(cfg.package_global_dir.clone(), Some(cwd.clone()));
    assert!(
        !store
            .package_dir(InstallScope::Global, &parsed.package_id())
            .unwrap()
            .exists(),
        "a failed clone must leave no partial tree"
    );
}

/// CFG-003, the clone half: [`crate::discovery::install_declared_git_package`] materializes the
/// working tree at exactly the path discovery resolves, prepares the install root first, and writes
/// no registry row.
///
/// Ported from `installGit`'s fresh-clone path (package-manager.ts:1820-1852 @v0.83.0):
/// `getGitInstallRoot` + `ensureGitIgnore` (`:1831-1834`) then the clone (`:1837`). The tree lands
/// under `PackageStore::package_dir`, which is what [`crate::package::store::installed_dir`] — the
/// resolver's git arm — returns, so the fall-through in `resolve_configured_package` walks the tree
/// it just created within the same pass, matching pi's `collectPackageResources(installedPath, …)`
/// immediately after `installMissing` (`:1293-1296`).
///
/// **COVERAGE, NOT PROOF-BY-RED:** this function is new, so no version of this test could fail
/// before the change — there was nothing to call. The red-before assertion for CFG-003 lives in
/// `cfg003_an_open_gate_attempts_the_install_and_reports_its_failure`, which drives the same arm
/// through the public `discover` entry point. Hermetic: the "remote" is a local repo behind a
/// `file://` URL — a real gix clone, no network — which is a spelling only reachable by
/// constructing the source directly, exactly as this crate's other clone tests do.
#[tokio::test]
async fn cfg003_install_declared_git_package_materializes_the_tree() {
    let Some((_repo_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping CFG-003 clone test: `git` CLI not available");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let source = PackageSource::Git {
        url: format!("file://{}", repo_dir.display()),
        reff: PinRef::Default,
    };
    let mut cfg = DiscoveryConfig::new(cwd.clone(), global.clone());
    cfg.project_root = Some(cwd.clone());
    cfg.install_missing_packages = true;

    let store = PackageStore::new(cfg.package_global_dir.clone(), Some(cwd.clone()));
    let dir = store
        .package_dir(InstallScope::Global, &source.package_id())
        .unwrap();
    assert!(!dir.exists(), "precondition: nothing is installed yet");

    crate::discovery::install_declared_git_package(&source, InstallScope::Global, &dir, &cfg)
        .expect("clone over file:// transport");

    assert!(
        dir.join("skills/alpha/SKILL.md").exists(),
        "the working tree is materialized at the path discovery resolves: {}",
        dir.display()
    );
    // The install ROOT is prepared BEFORE the clone (`:1831-1834`); at project scope that root sits
    // inside the user's own repository, which is what CFG-037 is about.
    assert_eq!(
        fs::read_to_string(
            store
                .packages_root(InstallScope::Global)
                .unwrap()
                .join(".gitignore")
        )
        .unwrap(),
        "*\n!.gitignore\n"
    );
    // No registry row is invented: `packages.json` records what `cyrup install` did, and the user
    // never installed this one. pi has no registry here at all — the declaration IS the record.
    assert!(
        !store.registry_path(InstallScope::Global).unwrap().exists(),
        "auto-install must not write an install-registry row"
    );

    // …and the tree is now the ordinary already-installed case: discovery walks it and does not
    // touch the remote again (pi's `existsSync(installedPath)` short-circuit, `:1288`). Proven with
    // the remote deleted first, so a second clone attempt could not silently succeed.
    fs::remove_dir_all(&repo_dir).unwrap();
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        // The same tree, named the way a settings entry names an ALREADY-installed local tree.
        source: dir.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "the cloned tree loads its resources: {:?}",
        report.diagnostics
    );
    assert!(report.registry.prompts.contains("greet"));
}

/// CFG-027: a settings-declared LOCAL package path that does not exist contributes nothing and
/// reports nothing — Pi's `resolveLocalExtensionSource` opens with
/// `if (!existsSync(resolved)) return;` (package-manager.ts:1324-1326 @v0.83.0), reached before any
/// diagnostic could be produced. cyrup routed it into the git arm's "not installed at this path —
/// run `cyrup install`" error, which is doubly wrong: `cyrup install` cannot materialize a path the
/// user typed, and Pi is silent here.
///
/// A bare `github:org/pkg` shorthand takes the SAME silent arm: `isLocalPath` rejects it
/// (paths.ts:41-55 @v0.83.0) but `parseGitUrl` bails on
/// `if (!hasGitPrefix && !/^(https?|ssh|git):\/\//i.test(url)) return null;` (git.ts:177-179), and
/// `parseSource` falls through to `return { type: "local", path: source }`
/// (package-manager.ts:1449-1459). Only a protocol-qualified url is a git source — see
/// `cfg003_uninstallable_settings_declared_package_is_an_error_diagnostic`.
#[tokio::test]
async fn cfg027_a_missing_local_package_path_is_a_silent_skip() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    // A real sibling package, so the pass is proven to still be running after the skip.
    let present = global.join("present-pack");
    make_package_tree(&present, true, false);

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: "./absent-package".into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
        // Not a git source in Pi — no protocol, so `parseGitUrl` returns null and `parseSource`
        // files it as `{ type: "local" }`. Silent, exactly like the `./` form above.
        crate::ConfiguredPackage {
            source: "github:org/absent-shorthand".into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
        crate::ConfiguredPackage {
            source: "present-pack".into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("absent-shorthand")),
        "a `github:` shorthand is a LOCAL path in Pi and must be silent too: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("absent-package")),
        "a missing LOCAL path must be silent: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
    assert!(
        !report
            .warnings
            .iter()
            .any(|w| w.reason.contains("absent-package")),
        "…including as a warning"
    );
    assert!(
        report.registry.skills.contains("alpha"),
        "the sibling entry must still resolve"
    );
}

/// CFG-027: a settings-declared LOCAL package path that is a FILE registers as an extension
/// directly — Pi's `resolveLocalExtensionSource` `:1330-1334` @v0.83.0 sets
/// `metadata.baseDir = dirname(resolved)` and calls
/// `addResource(accumulator.extensions, resolved, metadata, true)` without going anywhere near
/// `collectPackageResources`. cyrup demanded a directory and dropped the entry with a "not installed
/// at this path" error.
#[tokio::test]
async fn cfg027_a_local_package_entry_that_is_a_file_registers_as_an_extension() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    let ext_file = global.join("solo-ext.wasm");
    fs::write(&ext_file, b"\0asm").unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: "solo-ext.wasm".into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("solo-ext.wasm")),
        "a FILE entry is the extension: {:?}",
        report.registry.ext_crate_paths
    );
    assert!(
        report.diagnostics.is_empty(),
        "and it is not an error: {:?}",
        report
            .diagnostics
            .iter()
            .map(|d| d.message.clone())
            .collect::<Vec<_>>()
    );
}

/// CFG-004: a PROJECT-scope settings-declared package is trust-gated (fail closed), exactly like the
/// project-installed tier.
#[tokio::test]
async fn cfg003_project_scope_declared_package_is_trust_gated() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();
    // Declared relative to the project base dir (Pi `join(cwd, CONFIG_DIR_NAME)`, :2058).
    let pkg = cwd.join(".cyrup/local-pack");
    make_package_tree(&pkg, true, false);

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: "local-pack".into(),
        scope: InstallScope::Project,
        filter: crate::PackageFilter::default(),
    }];
    cfg.trusted_project = false;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report.registry.skills.contains("alpha"),
        "untrusted project must load nothing"
    );

    cfg.trusted_project = true;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "a trusted project resolves the declared package against <cwd>/.cyrup"
    );
}

/// CFG-004: a plain path in the settings `extensions` array is LOADED as an extension root — Pi runs
/// `resolveLocalEntries` over `RESOURCE_TYPES`, whose first member is `"extensions"`
/// (package-manager.ts:194, :905-931).
#[tokio::test]
async fn cfg004_settings_declared_extension_entries_are_loaded_not_just_filtered() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    let global = tmp.path().join("agent");
    fs::create_dir_all(cwd.join(".cyrup/exts/proj-ext")).unwrap();
    fs::create_dir_all(global.join("exts/glob-ext")).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.global_overrides = ResourceOverrides {
        extensions: vec!["exts/glob-ext".to_string()],
        ..Default::default()
    };
    cfg.project_overrides = ResourceOverrides {
        extensions: vec!["exts/proj-ext".to_string()],
        ..Default::default()
    };
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let roots = &report.registry.ext_crate_paths;
    assert!(roots.iter().any(|p| p.ends_with("glob-ext")), "{roots:?}");
    assert!(roots.iter().any(|p| p.ends_with("proj-ext")), "{roots:?}");

    // Untrusted project: the project entry must not load; the global one still does.
    cfg.trusted_project = false;
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let roots = &report.registry.ext_crate_paths;
    assert!(roots.iter().any(|p| p.ends_with("glob-ext")), "{roots:?}");
    assert!(!roots.iter().any(|p| p.ends_with("proj-ext")), "{roots:?}");
}
