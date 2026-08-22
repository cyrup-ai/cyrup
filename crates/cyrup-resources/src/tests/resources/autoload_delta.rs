//! CFG-010 — `autoload: false` is a DELTA filter, not an include filter, including project-over-
//! global deltas and delta pairing across ssh/https spellings of one repo.

use std::fs;

use super::fixtures::{make_package_tree, skill_md, write};
use crate::{DiscoveryConfig, InstallScope, PackageSource, PackageStore, ResourceScope, discover};
use cyrup_core::CancelToken;

// ===========================================================================
// CFG-010 — `autoload: false` is a DELTA filter, not an include filter
// ===========================================================================

/// Pi's object-form package entry carries `autoload` (settings-manager.ts:79, documented at :73 as
/// "start empty and only apply explicit resource patterns"). `collectPackageResources` branches on
/// it (package-manager.ts:2084-2085) into `applyPackageDeltaFilter` (:2173-2189), which starts from
/// an EMPTY set and returns immediately when the user gave no patterns for that resource type
/// (:2180-2182) — contributing nothing.
///
/// cyrup modelled none of it: `autoload` was not a field, so serde dropped it, and
/// `retain_by_package_filter` saw `patterns == None` for every type and kept the package's ENTIRE
/// manifest. A package the user explicitly opted out of loaded in full.
#[tokio::test]
async fn cfg010_autoload_false_with_no_patterns_contributes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(false),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report.registry.skills.contains("alpha"),
        "autoload=false with no `skills` patterns contributes NO skills"
    );
    assert!(!report.registry.prompts.contains("greet"), "…no prompts");
    assert!(!report.registry.themes.contains("midnight"), "…no themes");
    assert!(
        !report
            .registry
            .ext_crate_paths
            .iter()
            .any(|p| p.ends_with("deploy")),
        "…and no extensions"
    );
}

/// The delta half: under `autoload: false` an explicit pattern list ADDS BACK exactly what it names
/// (`applyAutoloadDisabledPatterns`, package-manager.ts:760-777 — start empty, each pattern sets its
/// matches' enabled flag, later patterns winning). Contrast the ordinary include-filter meaning of
/// the same list, which starts from everything.
#[tokio::test]
async fn cfg010_autoload_false_adds_back_only_the_named_patterns() {
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

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(false),
            skills: Some(vec!["skills/alpha/**".to_string()]),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("alpha"),
        "the named skill is added back"
    );
    assert!(
        !report.registry.skills.contains("beta"),
        "an unnamed sibling stays out"
    );
    assert!(
        !report.registry.prompts.contains("greet"),
        "a resource type with no patterns is still empty — the delta is per-type"
    );

    // A `!`/`-` pattern under autoload=false names something to keep DISABLED, so it adds nothing
    // (`enabled = !pattern.startsWith("-") && !pattern.startsWith("!")`, :766).
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(false),
            skills: Some(vec!["!skills/alpha/**".to_string()]),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        !report.registry.skills.contains("alpha"),
        "a negative pattern under autoload=false adds nothing back"
    );
}

/// `autoload: true` (and the absent case) must leave the ordinary include-filter path untouched —
/// Pi only takes the delta branch on an explicit `=== false` (package-manager.ts:2084).
#[tokio::test]
async fn cfg010_autoload_true_keeps_the_ordinary_include_filter_meaning() {
    let tmp = tempfile::tempdir().unwrap();
    let pkg = tmp.path().join("pkgsrc");
    make_package_tree(&pkg, true, false);
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");
    fs::create_dir_all(&global).unwrap();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;
    cfg.configured_packages = vec![crate::ConfiguredPackage {
        source: pkg.to_string_lossy().into_owned(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter {
            autoload: Some(true),
            ..Default::default()
        },
    }];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(report.registry.skills.contains("alpha"));
    assert!(report.registry.prompts.contains("greet"));
    assert!(report.registry.themes.contains("midnight"));
}

/// CFG-010 (dedupe half) — a PROJECT `autoload: false` entry is a DELTA **over** the global entry
/// for the same package, not a replacement of it.
///
/// Pi's `dedupePackages` says so in its own doc comment — "A project entry with autoload=false is
/// a delta over the global entry, so both are kept (delta first)" (package-manager.ts:1676-1679) —
/// and keeps both at :1691-1696. `resolvePackageSources` then processes the delta first, so under
/// `addResource`'s first-writer-wins map (:2488-2490) the delta OWNS the verdict for every file it
/// names and the global entry fills in everything else. Pi pins exactly this at
/// package-manager.test.ts:1714-1738: `-extensions/foo.ts` stays disabled at project scope while
/// its sibling loads at user scope.
///
/// cyrup dropped the second entry unconditionally, which inverted the feature: the project entry's
/// patterns became the only thing that loaded.
#[tokio::test]
async fn cfg010_project_autoload_false_entry_is_a_delta_over_the_global_entry() {
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
    let source = pkg.to_string_lossy().into_owned();

    let mut cfg = DiscoveryConfig::new(cwd, global);
    cfg.trusted_project = true;

    // --- negative delta: opt ONE skill out of an otherwise fully autoloaded global package ---
    // `-p` matches exactly, and an exact pattern naming a skill DIRECTORY matches its `SKILL.md`
    // (`matchesAnyExactPattern`, package-manager.ts:661-679).
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Project,
            filter: crate::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["-skills/alpha".to_string()]),
                ..Default::default()
            },
        },
        crate::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report.registry.skills.contains("beta"),
        "the global entry still autoloads everything the delta did not name"
    );
    assert!(
        report.registry.prompts.contains("greet"),
        "…including resource types the delta says nothing about"
    );
    assert!(
        !report.registry.skills.contains("alpha"),
        "the project delta's `-` pattern keeps `alpha` off even though the global entry autoloads it"
    );

    // --- positive delta: the named resource loads at PROJECT scope, the rest at GLOBAL scope ---
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: source.clone(),
            scope: InstallScope::Project,
            filter: crate::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["skills/alpha/**".to_string()]),
                ..Default::default()
            },
        },
        crate::ConfiguredPackage {
            source,
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    let alpha = report
        .registry
        .skills
        .get_name("alpha")
        .expect("the delta names `alpha`, and the global entry autoloads it anyway");
    let beta = report
        .registry
        .skills
        .get_name("beta")
        .expect("the global entry contributes the rest of the package");
    assert_eq!(
        alpha.scope,
        ResourceScope::ProjectPackage,
        "a file the delta names is attributed to the PROJECT entry (Pi: scope \"project\")"
    );
    assert_eq!(
        beta.scope,
        ResourceScope::GlobalPackage,
        "everything else is attributed to the global entry (Pi: scope \"user\")"
    );
    // Pi's accumulator is a Map keyed by path, so a delta pair can never double-list a file.
    assert_eq!(
        report
            .registry
            .skills
            .all()
            .iter()
            .filter(|s| s.name == "alpha")
            .count(),
        1,
        "the delta pair must not list the same file twice"
    );
}

/// CFG-010 (dedupe half) — the project delta RESOLVES against the entry it deltas over.
///
/// Pi's `findAutoloadDeltaBase` (package-manager.ts:1301-1313 @v0.83.0) swaps in the user entry's
/// source and scope (`resolvedSource`/`resolvedScope`, :1246-1247) before parsing, so the pair lands
/// on ONE working tree even though the INSTALL PATH is scope-dependent
/// (`getGitInstallPath(parsed, resolvedScope)`, :1289; `getNpmInstallPath(…, resolvedScope)`,
/// :1276). Without the swap the project half looks under `<project>/.cyrup/packages/<id>`, which no
/// install ever wrote, and the delta applies to nothing.
///
/// The pair is a GIT source on purpose. `findAutoloadDeltaBase` matches by `getPackageIdentity`,
/// and that identity is scope-independent ONLY for the npm and git arms
/// (`npm:<name>` / `git:<host>/<path>`, package-manager.ts:1678-1684); the local arm resolves the
/// path against `getBaseDirForScope(scope)` (`:1685-1688`), so one RELATIVE local string is two
/// different packages across scopes and pi finds no delta base at all — the CFG-026 rule that
/// [`crate::package::package_identity`]'s own unit tests pin. A relative local source therefore
/// cannot express this case; it would only assert a pairing pi does not make.
#[tokio::test]
async fn cfg010_project_delta_resolves_against_the_global_entrys_install_location() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");

    let source_str = "https://github.com/acme/shared-pack.git";
    let source = PackageSource::parse(source_str).unwrap();
    let id = source.package_id();
    // Installed under the GLOBAL package store ONLY — `<project>/.cyrup/packages/<id>` does not
    // exist, so any resolution that keeps the project scope lands on nothing.
    let store = PackageStore::new(global.join("packages"), Some(cwd.clone()));
    let pkg = store.package_dir(InstallScope::Global, &id).unwrap();
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let delta = crate::ConfiguredPackage {
        source: source_str.into(),
        scope: InstallScope::Project,
        filter: crate::PackageFilter {
            autoload: Some(false),
            skills: Some(vec!["-skills/alpha".to_string()]),
            ..Default::default()
        },
    };
    let base = crate::ConfiguredPackage {
        source: source_str.into(),
        scope: InstallScope::Global,
        filter: crate::PackageFilter::default(),
    };

    let mut cfg = DiscoveryConfig::new(&cwd, &global);
    cfg.project_root = Some(cwd.clone());
    cfg.trusted_project = true;
    cfg.configured_packages = vec![delta.clone(), base];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .all(|d| !d.message.contains("shared-pack")),
        "the delta must resolve to the global entry's install location, not to a missing project \
         path: {:?}",
        report.diagnostics
    );
    assert!(
        report.registry.skills.contains("beta"),
        "the global entry autoloads the rest of the package"
    );
    assert!(
        !report.registry.skills.contains("alpha"),
        "the delta's `-` pattern reached the same tree and kept `alpha` off"
    );

    // NON-VACUITY CONTROL. Drop the global entry and the same delta has nothing to resolve against,
    // so it stays at project scope and hits the empty project store — i.e. the pass above is the
    // swap doing work, not the project scope happening to point at the same tree.
    let mut alone = cfg.clone();
    alone.configured_packages = vec![delta];
    let report = discover(&alone, CancelToken::new()).await.unwrap();
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.message.contains("shared-pack")),
        "an unpaired project delta resolves against the PROJECT store, which is empty: {:?}",
        report.diagnostics
    );
    assert!(!report.registry.skills.contains("beta"));
}

/// CFG-010 (pairing key) — the delta pair is matched by IDENTITY, not by source string.
///
/// `dedupePackages` keys on `getPackageIdentity(source, entry.scope)`
/// (package-manager.ts:1703 @v0.83.0), and the git arm of that identity is `git:<host>/<path>`
/// (`:1682-1684`) — deliberately spelling-independent, "to normalize SSH and HTTPS" per its own doc
/// comment (`:1669-1674`). So a project delta written `ssh://git@github.com/acme/p.git` pairs with a
/// global entry written `https://github.com/acme/p.git`. (The scp-like `git@github.com:acme/p.git`
/// would NOT: `isLocalPath` lists only `npm:`/`git:`/`github:`/`http:`/`https:`/`ssh:` as non-local
/// (paths.ts:41-55 @v0.83.0), so upstream reads that spelling as a local path, and so does cyrup.)
///
/// A source-STRING key does not pair them, and the failure is not merely a missed subtraction: the
/// two spellings still resolve onto one tree (`findAutoloadDeltaBase` matches by identity), so an
/// unpaired global half is a repeat visit to an already-seen tree and is skipped outright — every
/// resource it autoloads disappears.
#[tokio::test]
async fn cfg010_delta_pairs_across_ssh_and_https_spellings_of_one_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("project");
    fs::create_dir_all(&cwd).unwrap();
    let global = tmp.path().join("agent");

    let https = "https://github.com/acme/shared-pack.git";
    let ssh = "ssh://git@github.com/acme/shared-pack.git";
    let id = PackageSource::parse(https).unwrap().package_id();
    assert_eq!(
        id,
        PackageSource::parse(ssh).unwrap().package_id(),
        "precondition: the two spellings are one package"
    );
    let store = PackageStore::new(global.join("packages"), Some(cwd.clone()));
    let pkg = store.package_dir(InstallScope::Global, &id).unwrap();
    make_package_tree(&pkg, true, false);
    write(
        &pkg.join("skills/beta/SKILL.md"),
        &skill_md("beta", "beta skill"),
    );

    let mut cfg = DiscoveryConfig::new(&cwd, &global);
    cfg.project_root = Some(cwd.clone());
    cfg.trusted_project = true;
    cfg.configured_packages = vec![
        crate::ConfiguredPackage {
            source: ssh.into(),
            scope: InstallScope::Project,
            filter: crate::PackageFilter {
                autoload: Some(false),
                skills: Some(vec!["-skills/alpha".to_string()]),
                ..Default::default()
            },
        },
        crate::ConfiguredPackage {
            source: https.into(),
            scope: InstallScope::Global,
            filter: crate::PackageFilter::default(),
        },
    ];
    let report = discover(&cfg, CancelToken::new()).await.unwrap();
    // The global half must still be walked: `beta` and the package's prompt are what it autoloads,
    // and neither is anything the delta could have contributed (its only pattern is a `-`).
    assert!(
        report.registry.skills.contains("beta"),
        "the global half of the pair must not be skipped as a duplicate tree: {:?}",
        report.diagnostics
    );
    assert!(
        report.registry.prompts.contains("greet"),
        "…including resource types the delta says nothing about"
    );
    assert!(
        !report.registry.skills.contains("alpha"),
        "and the delta the other spelling declared still subtracts `alpha`"
    );
}
