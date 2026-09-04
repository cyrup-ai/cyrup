//! The startup package-update check, end to end against REAL git repositories — the port of Pi's
//! `DefaultPackageManager.checkForAvailableUpdates` / `gitHasAvailableUpdate`
//! (`pi/packages/coding-agent/src/core/package-manager.ts:1175-1238`, `:1521-1554`), fired at
//! startup by `InteractiveMode.run` (`interactive-mode.ts:850-856`).
//!
//! # What was broken
//!
//! cyrup had no update-availability check of any kind, and `NetworkPolicy::allow_update_check()` —
//! the gate that exists precisely to govern one — had ZERO production callers.
//!
//! These tests build actual git repos with the `git` binary (which is what the feature drives, so
//! stubbing it would test nothing) and drive `cyrup::update_check` over them. No network is touched:
//! `origin` is a local path, so `git ls-remote origin` resolves entirely on disk.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::path::{Path, PathBuf};
use std::process::Command;

use cyrup_config::policy::NetworkPolicy;
use cyrup_resources::{InstallScope, PackageSource, PackageStore};

/// A policy that permits the check — what a plain `cyrup` launch resolves to.
const ONLINE: NetworkPolicy = NetworkPolicy {
    offline: false,
    update_check: true,
    install_telemetry: false,
    analytics: false,
};

fn git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        // Identity + hook/signing suppression so the test never depends on the runner's ~/.gitconfig.
        .env("GIT_AUTHOR_NAME", "cyrup test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "cyrup test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .output()
        .unwrap_or_else(|e| panic!("spawning `git {}`: {e}", args.join(" ")));
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// A temp dir that deletes itself when dropped, derefing to [`Path`] so it is still used
/// directly as one. The guard MUST stay bound for the whole test.
struct Scratch(tempfile::TempDir);

impl std::ops::Deref for Scratch {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.0.path()
    }
}

/// A scratch root, unique per call, so a crashed previous run cannot poison this one and
/// nothing is left under `/tmp` afterwards.
fn scratch(name: &str) -> Scratch {
    Scratch(
        tempfile::Builder::new()
            .prefix(&format!("cyrup-pkg-update-{name}-"))
            .tempdir()
            .unwrap(),
    )
}

/// An `origin` repo with one commit, plus a clone of it. Returns `(origin, clone)`.
fn origin_and_clone(root: &Path) -> (PathBuf, PathBuf) {
    let origin = root.join("origin");
    std::fs::create_dir_all(&origin).unwrap();
    git(&origin, &["init", "--initial-branch=main", "--quiet"]);
    std::fs::write(origin.join("README.md"), "v1\n").unwrap();
    git(&origin, &["add", "README.md"]);
    git(&origin, &["commit", "-m", "one", "--quiet"]);

    let clone = root.join("clone");
    git(
        root,
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            clone.to_str().unwrap(),
        ],
    );
    (origin, clone)
}

/// THE mechanism: an install tree level with its remote reports NO update; the same tree reports one
/// the moment the remote gains a commit. Upstream `gitHasAvailableUpdate` (`:1521-1536`).
#[tokio::test]
async fn a_clone_reports_an_update_only_once_its_remote_moves() {
    let root = scratch("moves");
    let (origin, clone) = origin_and_clone(&root);

    // MIRROR: in sync ⇒ nothing to report. This half stays true regardless of how the comparison is
    // implemented, so it is what proves the positive case below is not simply "always true".
    assert!(
        !cyrup::update_check::git_has_available_update(&clone).await,
        "an up-to-date clone claimed an update was available"
    );

    std::fs::write(origin.join("README.md"), "v2\n").unwrap();
    git(&origin, &["add", "README.md"]);
    git(&origin, &["commit", "-m", "two", "--quiet"]);

    assert!(
        cyrup::update_check::git_has_available_update(&clone).await,
        "a clone whose remote advanced reported no update"
    );
}

/// The whole check over the installed-package registry: the out-of-date package is reported by
/// upstream's `displayName` (`${parsed.host}/${parsed.path}`, `:1228`).
#[tokio::test]
async fn the_registry_walk_reports_the_out_of_date_package_by_display_name() {
    let root = scratch("registry");
    let (origin, _) = origin_and_clone(&root);

    // The store the binary builds (`PackageStore::new(dirs.package_dir, Some(dirs.cwd))`).
    let store = PackageStore::new(root.join("agent-packages"), Some(root.join("project")));
    let source = PackageSource::Git {
        url: "https://github.com/nicobailon/pi-intercom".to_string(),
        reff: Default::default(),
    };
    let id = source.package_id();
    let install_dir = store.package_dir(InstallScope::Global, &id).unwrap();
    std::fs::create_dir_all(install_dir.parent().unwrap()).unwrap();
    // Install = a clone of `origin` at the path the store dictates.
    git(
        &root,
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            install_dir.to_str().unwrap(),
        ],
    );

    let registry_path = store.registry_path(InstallScope::Global).unwrap();
    std::fs::create_dir_all(registry_path.parent().unwrap()).unwrap();
    std::fs::write(
        &registry_path,
        serde_json::json!({
            "packages": [{
                "id": id.as_str(),
                "source": { "kind": "git", "url": "https://github.com/nicobailon/pi-intercom",
                            "reff": { "ref": "default" } },
                "scope": "global",
                "installedAt": "0",
            }]
        })
        .to_string(),
    )
    .unwrap();

    // MIRROR: level with its remote ⇒ an EMPTY report even though the package is registered and its
    // tree exists. Without this, "reports one entry" could just mean "reports every package".
    assert!(
        cyrup::update_check::check_for_available_updates(store.clone(), ONLINE)
            .await
            .is_empty(),
        "an up-to-date install produced a notification"
    );

    std::fs::write(origin.join("README.md"), "v2\n").unwrap();
    git(&origin, &["add", "README.md"]);
    git(&origin, &["commit", "-m", "two", "--quiet"]);

    let updates = cyrup::update_check::check_for_available_updates(store, ONLINE).await;
    assert_eq!(
        updates
            .iter()
            .map(|u| u.display_name.as_str())
            .collect::<Vec<_>>(),
        vec!["github.com/nicobailon/pi-intercom"],
        "the out-of-date package was not reported under Pi's displayName"
    );
}

/// The gate: `--offline` / `CYRUP_OFFLINE` suppresses the check entirely, even with an out-of-date
/// package sitting right there (upstream's `if (isOfflineModeEnabled()) return []`, `:1176`).
#[tokio::test]
async fn an_offline_policy_suppresses_the_check_over_a_real_stale_install() {
    let root = scratch("offline");
    let (origin, _) = origin_and_clone(&root);

    let store = PackageStore::new(root.join("agent-packages"), Some(root.join("project")));
    let source = PackageSource::Git {
        url: "https://github.com/nicobailon/pi-intercom".to_string(),
        reff: Default::default(),
    };
    let id = source.package_id();
    let install_dir = store.package_dir(InstallScope::Global, &id).unwrap();
    std::fs::create_dir_all(install_dir.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            install_dir.to_str().unwrap(),
        ],
    );
    let registry_path = store.registry_path(InstallScope::Global).unwrap();
    std::fs::create_dir_all(registry_path.parent().unwrap()).unwrap();
    std::fs::write(
        &registry_path,
        serde_json::json!({
            "packages": [{
                "id": id.as_str(),
                "source": { "kind": "git", "url": "https://github.com/nicobailon/pi-intercom",
                            "reff": { "ref": "default" } },
                "scope": "global",
                "installedAt": "0",
            }]
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(origin.join("README.md"), "v2\n").unwrap();
    git(&origin, &["add", "README.md"]);
    git(&origin, &["commit", "-m", "two", "--quiet"]);

    // Same tree, same staleness — only the policy differs.
    assert_eq!(
        cyrup::update_check::check_for_available_updates(store.clone(), ONLINE)
            .await
            .len(),
        1,
        "precondition: the package really is out of date"
    );
    let offline = NetworkPolicy {
        offline: true,
        ..ONLINE
    };
    assert!(
        cyrup::update_check::check_for_available_updates(store, offline)
            .await
            .is_empty(),
        "an offline launch still probed the remote"
    );
}

/// A PINNED source is never reported — upstream skips `parsed.pinned` outright (`:1198`), because a
/// tag/commit pin means "do not move me" and `update --extensions` would skip it anyway (R-09-020).
#[tokio::test]
async fn a_pinned_package_is_never_reported() {
    let root = scratch("pinned");
    let (origin, _) = origin_and_clone(&root);

    let store = PackageStore::new(root.join("agent-packages"), Some(root.join("project")));
    let source = PackageSource::Git {
        url: "https://github.com/nicobailon/pi-intercom".to_string(),
        reff: cyrup_resources::PinRef::Tag("v1.0.0".to_string()),
    };
    let id = source.package_id();
    let install_dir = store.package_dir(InstallScope::Global, &id).unwrap();
    std::fs::create_dir_all(install_dir.parent().unwrap()).unwrap();
    git(
        &root,
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            install_dir.to_str().unwrap(),
        ],
    );
    let registry_path = store.registry_path(InstallScope::Global).unwrap();
    std::fs::create_dir_all(registry_path.parent().unwrap()).unwrap();
    std::fs::write(
        &registry_path,
        serde_json::json!({
            "packages": [{
                "id": id.as_str(),
                "source": { "kind": "git", "url": "https://github.com/nicobailon/pi-intercom",
                            "reff": { "ref": "tag", "value": "v1.0.0" } },
                "scope": "global",
                "installedAt": "0",
            }]
        })
        .to_string(),
    )
    .unwrap();
    // The remote moves, but the pin means the user asked not to be told.
    std::fs::write(origin.join("README.md"), "v2\n").unwrap();
    git(&origin, &["add", "README.md"]);
    git(&origin, &["commit", "-m", "two", "--quiet"]);

    assert!(
        cyrup::update_check::check_for_available_updates(store, ONLINE)
            .await
            .is_empty(),
        "a pinned package was offered an update"
    );
}
