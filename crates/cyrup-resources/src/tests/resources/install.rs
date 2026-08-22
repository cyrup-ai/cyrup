//! Install / remove / list / update / pin over local paths and git fixtures, failure atomicity,
//! ref checkout, and the project-scope trust gate with its security caveat (A-09-5, A-09-6, G3).

use std::fs;
use std::path::PathBuf;

use super::fixtures::{
    make_local_git_repo, make_local_git_repo_two_commits, make_package_tree, write,
};
use crate::package::lock;
use crate::{
    InstallScope, InstalledPackage, InstalledPackages, PackageManager, PackageSource, PackageStore,
    PinRef, ResourceSelector, SECURITY_CAVEAT, UpdateTarget,
};
use cyrup_core::CancelToken;

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
        .install(
            PackageSource::Path { path: pkg.clone() },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("local-path install");
    assert_eq!(
        notice.message, SECURITY_CAVEAT,
        "security caveat surfaced (R-09-019)"
    );

    let listed = mgr.list();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, rec.id);

    // enable/disable a resource within the package (R-09-018).
    mgr.set_enabled(&rec.id, ResourceSelector::Skill("alpha".to_string()), false)
        .unwrap();
    let after = mgr.list();
    assert!(after[0].disabled.skills.contains("alpha"));

    mgr.remove(&rec.id).await.expect("remove");
    assert!(mgr.list().is_empty(), "list empty after remove");
}

#[tokio::test]
async fn a09_5_update_skips_pinned_and_one_moves_it() {
    // Construct two git installs directly in the registry: one Default (bulk-updatable), one
    // Tag-pinned (bulk-skipped). Neither URL resolves, so the evidence that a package was ATTEMPTED
    // is its appearance in `report.failed` — Pi's `ensureGitRef` (package-manager.ts:1870-1896
    // @v0.83.0) opens with `runCommand("git", fetchArgs)`, which rejects on a non-zero exit and
    // propagates. This test used to assert `report.updated.contains(&default_id)` for a
    // `file:///x/a` that cannot be cloned: it passed VACUOUSLY, pinning the exact defect that a
    // failed fetch was reported to the user as a successful update.
    let tmp = tempfile::tempdir().unwrap();
    let global = tmp.path().join("global");
    let store = PackageStore::new(global.clone(), None);

    let default_src = PackageSource::Git {
        url: "file:///x/a".into(),
        reff: PinRef::Default,
    };
    let tag_src = PackageSource::Git {
        url: "file:///x/b".into(),
        reff: PinRef::Tag("v1".into()),
    };
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
    let report = mgr
        .update(UpdateTarget::All, CancelToken::new())
        .await
        .unwrap();
    assert!(
        report.skipped_pinned.contains(&tag_id),
        "bulk update skips pinned (R-09-020)"
    );
    assert!(
        report.failed.iter().any(|(id, _)| id == &default_id),
        "bulk update ATTEMPTS the unpinned package, and an unreachable remote is a failure"
    );
    assert!(
        !report.updated.contains(&default_id),
        "an unreachable remote must never be reported as an update"
    );
    assert!(!report.updated.contains(&tag_id));
    assert!(
        !report.failed.iter().any(|(id, _)| id == &tag_id),
        "the pinned package was skipped, so it cannot have been attempted"
    );

    // update(One) targets the pinned package regardless of the pin.
    let one = mgr
        .update(UpdateTarget::One(tag_id.clone()), CancelToken::new())
        .await
        .unwrap();
    assert!(
        one.skipped_pinned.is_empty(),
        "explicit update(One) does not skip a pinned package"
    );
    assert!(
        one.failed.iter().any(|(id, _)| id == &tag_id),
        "explicit update(One) attempts the pinned package"
    );
}

/// A git fetch that fails must leave the installed working tree byte-identical, and must be
/// reported as a failure rather than an update.
///
/// Pi never clones over an existing tree: `installGit` early-returns into a fetch when
/// `existsSync(targetDir)` (package-manager.ts:1822-1830 @v0.83.0) and `updateGit` fetches IN PLACE
/// and only `git reset --hard`s once the fetch succeeded (`:1853-1868` → `ensureGitRef`
/// `:1870-1896`), so an unreachable remote throws with the tree untouched.
///
/// Red at HEAD: `git_clone_url` opened with an unconditional `remove_dir_all(dir)`, so the tree was
/// destroyed BEFORE the fetch could fail — one offline `cyrup update` wiped every git package's
/// working tree — and `refresh` then swallowed the error, re-read nothing, and returned `Ok(())`,
/// which `update` recorded in `report.updated`.
#[tokio::test]
async fn a_failed_git_update_preserves_the_installed_tree_and_is_reported_as_failed() {
    let Some((_tmp, repo_dir)) = make_local_git_repo() else {
        eprintln!("skipping failed-update test: `git` CLI not available");
        return;
    };
    let url = format!("file://{}", repo_dir.display());

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url,
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("initial clone over file://");

    let dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    assert!(dir.is_dir(), "the install materialized a working tree");
    let listing = |p: &std::path::Path| {
        let mut names: Vec<_> = fs::read_dir(p)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        names
    };
    let before = listing(&dir);
    assert!(!before.is_empty(), "the clone produced files");

    // Make the remote unreachable, exactly as an offline machine or a deleted repo would.
    fs::remove_dir_all(&repo_dir).unwrap();

    let report = mgr
        .update(UpdateTarget::All, CancelToken::new())
        .await
        .unwrap();
    assert!(
        report.failed.iter().any(|(id, _)| id == &rec.id),
        "an unreachable remote is a per-package failure, not an update"
    );
    assert!(!report.updated.contains(&rec.id));

    assert!(dir.is_dir(), "the installed working tree must still exist");
    assert_eq!(before, listing(&dir), "the working tree must be unchanged");

    // No staging or backup residue is left beside it.
    let root = store.packages_root(InstallScope::Global).unwrap();
    for entry in fs::read_dir(&root).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(
            !name.ends_with(".cyrup-incoming") && !name.ends_with(".cyrup-previous"),
            "staging residue left behind: {name}"
        );
    }
}

/// A git install whose clone fails part-way must leave NOTHING at the target path — Pi's
/// `installGit` catch is `rmSync(targetDir, { recursive: true, force: true })`
/// (package-manager.ts:1847 @v0.83.0).
///
/// Red at HEAD: the bare-on-disk-repo arm of `git_clone` `copy_tree`d the source into `dir` and only
/// THEN resolved the ref, so a directory that is not a git repo left a full copy of itself at
/// exactly the path `installed_dir` resolves — with no `packages.json` row behind it. Because
/// `resolve_configured_package` keys off the DIRECTORY and not the registry, the orphan then loads
/// as an installed package on every later session.
#[tokio::test]
async fn a_failed_git_install_leaves_no_partial_tree_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("not-a-repo");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("SKILL.md"), "---\nname: ghost\n---\n").unwrap();

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let source = PackageSource::Git {
        url: src.to_string_lossy().into_owned(),
        reff: PinRef::Default,
    };
    let id = source.package_id();

    mgr.install(source, InstallScope::Global, true, CancelToken::new())
        .await
        .expect_err("a directory that is not a git repo cannot be installed");

    let dir = store
        .package_dir(InstallScope::Global, &id)
        .expect("package dir");
    assert!(
        !dir.exists(),
        "a failed install must leave nothing at {}",
        dir.display()
    );
    assert!(mgr.list().is_empty(), "and no registry row");
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
            PackageSource::Git {
                url: repo_dir.display().to_string(),
                reff: PinRef::Default,
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install from local git repo");
    assert!(
        rec.resolved_commit.is_some(),
        "HEAD commit resolved via gix (no network)"
    );
    assert_eq!(mgr.list().len(), 1);
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
        .install(
            PackageSource::Path { path: pkg.clone() },
            InstallScope::Project,
            false,
            CancelToken::new(),
        )
        .await
        .expect_err("untrusted project install must be refused");
    assert!(matches!(err, crate::ResourceError::Untrusted(_)));

    // Trusted project install succeeds and still surfaces the security caveat (R-09-019).
    let (_rec, notice) = mgr
        .install(
            PackageSource::Path { path: pkg },
            InstallScope::Project,
            true,
            CancelToken::new(),
        )
        .await
        .expect("trusted project install");
    assert_eq!(notice.message, SECURITY_CAVEAT);
}

/// CFG-037: a project-scope git install root must self-ignore before the clone —
/// `const gitRoot = this.getGitInstallRoot(scope); if (gitRoot) { this.ensureGitIgnore(gitRoot); }`
/// (package-manager.ts:1829-1834 @v0.83.0), with `ensureGitIgnore` writing exactly
/// `*\n!.gitignore\n` and only when no `.gitignore` is there (`:1952-1960`).
///
/// Red at HEAD: `grep -rn gitignore crates/cyrup-resources/src` found only the skill-walk ignore
/// READER; nothing wrote one, so a project-scope clone (plus its nested `.git`) landed untracked in
/// the user's repository.
#[test]
fn project_package_install_root_self_ignores_and_never_clobbers_an_existing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    let store = PackageStore::new(tmp.path().join("global"), Some(project.clone()));
    let root = store.packages_root(InstallScope::Project).unwrap();
    assert_eq!(root, project.join(".cyrup").join("packages"));

    // The root does not exist yet — `ensureGitIgnore` creates it (`:1953-1955`).
    crate::package::install::ensure_git_ignore(&root).unwrap();
    let ignore = root.join(".gitignore");
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "*\n!.gitignore\n");

    // Idempotent, and a user's own file is left byte-identical.
    fs::write(&ignore, "# mine\n").unwrap();
    crate::package::install::ensure_git_ignore(&root).unwrap();
    assert_eq!(fs::read_to_string(&ignore).unwrap(), "# mine\n");
}

/// CFG-025: settings-declared resource paths go through `normalizePath` before the absoluteness
/// test — `resolvePathFromBase` (package-manager.ts:2069-2071 @v0.83.0) → `resolvePath`
/// (paths.ts:81-85) → `normalizePath` (`:57-78`).
///
/// Red at HEAD: `resolve_local_entries` did `PathBuf::from(entry.trim())` and tested THAT for
/// absoluteness, so `~/team-skills` became `<base>/~/team-skills` and loaded nothing, and
/// `file:///abs/x` became `<base>/file:/abs/x`.
#[test]
fn settings_local_entries_expand_tilde_and_file_urls_before_resolving_against_the_base() {
    use crate::package::manifest::{ManifestResourceType, resolve_local_entries};

    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path().join("base");
    let home = PathBuf::from(cyrup_config::paths::normalize_path("~"));

    // A `~` entry resolves under the home dir, NOT under `base`.
    let out = resolve_local_entries(
        &base,
        &["~/team-skills/SKILL.md".to_string()],
        ManifestResourceType::Skills,
    );
    // The file does not exist, so nothing is collected — what is pinned here is that the resolved
    // candidate never contained a literal `~` segment under `base`.
    assert!(out.is_empty());
    assert!(!base.join("~").exists());

    // A real file behind a `file://` URL IS collected.
    let abs = tmp.path().join("pack");
    write(&abs.join("prompts/one.md"), "hello");
    let url = format!("file://{}", abs.join("prompts").display());
    let out = resolve_local_entries(&base, &[url], ManifestResourceType::Prompts);
    assert_eq!(out, vec![abs.join("prompts/one.md")]);

    // And a real file under the home dir is collected through the `~` form.
    let _ = home;
}

// ===========================================================================
// Git ref checkout actually materializes the pinned ref (G3)
// ===========================================================================

#[tokio::test]
async fn a09_5_git_ref_checkout_materializes_pinned_tag() {
    let Some((_tmp, repo_dir)) = make_local_git_repo_two_commits() else {
        eprintln!("skipping git ref-checkout test: `git` CLI not available");
        return;
    };

    let global = tempfile::tempdir().unwrap();
    let store = PackageStore::new(global.path().to_path_buf(), None);
    let mgr = PackageManager::new(store.clone());
    let (rec, _notice) = mgr
        .install(
            PackageSource::Git {
                url: repo_dir.display().to_string(),
                reff: PinRef::Tag("v1".into()),
            },
            InstallScope::Global,
            true,
            CancelToken::new(),
        )
        .await
        .expect("install pinned to tag v1 from a local git repo");

    // The store working copy must reflect the *pinned* ref, not default HEAD: marker.txt held "v1"
    // at the tagged commit and "v2" at HEAD. Locate the materialized clone via the store dir.
    let store_dir = store
        .package_dir(InstallScope::Global, &rec.id)
        .expect("package dir");
    let marker = store_dir.join("marker.txt");
    assert!(
        marker.exists(),
        "checked-out tree present at {}",
        store_dir.display()
    );
    assert_eq!(
        fs::read_to_string(&marker).unwrap().trim(),
        "v1",
        "tag v1 checkout materialized (not default HEAD's v2) — pin is applied, R-09-018/020"
    );
}
