//! On-disk layout for installed packages (arch-09 §4.1).

use std::path::{Path, PathBuf};

use cyrup_core::PackageId;

use crate::package::source::id_dir_name;
use crate::scope::InstallScope;

/// Resolves package working-tree dirs and `packages.json` paths per scope.
#[derive(Clone, Debug)]
pub struct PackageStore {
    pub global_dir: PathBuf,
    pub project_root: Option<PathBuf>,
}

impl PackageStore {
    pub fn new(global_dir: PathBuf, project_root: Option<PathBuf>) -> Self {
        Self {
            global_dir,
            project_root,
        }
    }

    /// The packages root dir for a scope: `global_dir` itself (which IS the package dir —
    /// `<agent_dir>/packages` by default, or whatever `--package-dir`/`CYRUP_PACKAGE_DIR`/
    /// `PI_PACKAGE_DIR` resolved to) or `<project>/.cyrup/packages`.
    ///
    /// **CFG-054.** The Global arm used to be `global_dir.join("packages")`. Every caller passes
    /// `dirs.package_dir`, which already ends in `packages` (`cyrup-config/src/env.rs:191-196`), so
    /// a cloned package's working tree landed at `<agent_dir>/packages/packages/<id>` while
    /// [`Self::registry_path`] — which never doubled — sat one level up at
    /// `<agent_dir>/packages/packages.json`. The two scopes disagreed in shape (`<cwd>/.cyrup/
    /// packages/<id>` does not double), and the path a user must open to inspect, patch or delete an
    /// installed package was one no document would naturally state. pi has no two-level join at all:
    /// its git working trees are `join(this.agentDir, "git")` (`package-manager.ts:2050` @v0.83.0)
    /// and its npm root `join(this.agentDir, "npm")` (`:1970`), both directly under the agent dir.
    ///
    /// Dropping the join (rather than re-rooting the store at `agent_dir`) is what keeps the
    /// `CYRUP_PACKAGE_DIR` override meaningful: that variable resolves the package root itself, so
    /// the root is exactly where it points. Trees written by an older build are moved up by
    /// [`migrate_legacy_doubled_packages_root`].
    pub fn packages_root(&self, scope: InstallScope) -> Option<PathBuf> {
        match scope {
            InstallScope::Global => Some(self.global_dir.clone()),
            InstallScope::Project => self
                .project_root
                .as_ref()
                .map(|r| r.join(".cyrup").join("packages")),
        }
    }

    /// The working-tree dir for a package id in a scope.
    pub fn package_dir(&self, scope: InstallScope, id: &PackageId) -> Option<PathBuf> {
        self.packages_root(scope)
            .map(|root| root.join(id_dir_name(id)))
    }

    /// The `packages.json` registry path for a scope.
    pub fn registry_path(&self, scope: InstallScope) -> Option<PathBuf> {
        match scope {
            InstallScope::Global => Some(self.global_dir.join("packages.json")),
            InstallScope::Project => self
                .project_root
                .as_ref()
                .map(|r| r.join(".cyrup").join("packages.json")),
        }
    }
}

/// Move Global-scope package working trees written by a pre-CFG-054 build up out of the doubled
/// `<package_dir>/packages/` segment into `<package_dir>/`, and drop the emptied directory.
///
/// Best-effort and idempotent, in the shape of every other startup migration
/// (`crates/cyrup/src/migrations.rs`): a name that already exists at the destination is left where
/// it is rather than clobbering the newer tree, and any I/O failure simply leaves that entry behind.
/// Returns the number of entries moved so a caller can report, and `0` when there is nothing to do —
/// which is the case for every fresh install, so the common startup pays one `read_dir` on a
/// missing path.
///
/// Without this, upgrading orphans every installed git package: the registry row survives (its path
/// never doubled) while discovery — which resolves the tree through [`PackageStore::package_dir`] —
/// looks at the new location and finds nothing, so the package's skills, prompts and themes silently
/// stop loading.
pub fn migrate_legacy_doubled_packages_root(global_dir: &Path) -> usize {
    let legacy = global_dir.join("packages");
    let Ok(entries) = std::fs::read_dir(&legacy) else {
        return 0;
    };
    let mut moved = 0usize;
    for entry in entries.flatten() {
        let target = global_dir.join(entry.file_name());
        if target.exists() {
            continue;
        }
        if std::fs::rename(entry.path(), &target).is_ok() {
            moved += 1;
        }
    }
    // Only succeeds once the directory is empty, which is exactly the condition to remove it under.
    let _ = std::fs::remove_dir(&legacy);
    moved
}

/// Compute the on-disk dir for an installed package given global/project roots — used by discovery
/// (which has no `PackageStore`). Path sources reference their path directly.
pub fn installed_dir(
    source: &crate::package::source::PackageSource,
    scope: InstallScope,
    id: &PackageId,
    global_dir: &Path,
    project_root: Option<&Path>,
) -> Option<PathBuf> {
    use crate::package::source::PackageSource;
    if let PackageSource::Path { path } = source {
        return Some(path.clone());
    }
    let store = PackageStore::new(
        global_dir.to_path_buf(),
        project_root.map(Path::to_path_buf),
    );
    store.package_dir(scope, id)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    /// CFG-054 — a Global package's working tree and the registry that records it must sit at the
    /// same level. They did not: the tree was `<package_dir>/packages/<id>` while `packages.json`
    /// was `<package_dir>/packages.json`, so the path a user opens to inspect or delete an installed
    /// package was a level deeper than anything would state, and the two scopes disagreed in shape.
    #[test]
    fn a_global_package_tree_and_its_registry_sit_at_the_same_level() {
        // `dirs.package_dir`'s default (`cyrup-config/src/env.rs:191-196`) — the value every caller
        // passes as the store root.
        let package_dir = Path::new("/home/u/.cyrup/agent/packages");
        let store = PackageStore::new(package_dir.to_path_buf(), Some("/repo".into()));
        let id = PackageId::from("git:github.com/acme/pack");

        let tree = store.package_dir(InstallScope::Global, &id).unwrap();
        let registry = store.registry_path(InstallScope::Global).unwrap();
        assert_eq!(tree.parent(), registry.parent(), "tree {tree:?} vs registry {registry:?}");
        assert_eq!(tree, package_dir.join("git-github.com-acme-pack"));
        assert_eq!(registry, package_dir.join("packages.json"));
        assert!(
            !tree.to_string_lossy().contains("packages/packages"),
            "the doubled segment is back: {tree:?}"
        );
        // The `CYRUP_PACKAGE_DIR` override still resolves the root itself, not a child of it.
        let custom = PackageStore::new("/opt/cyrup-packages".into(), None);
        assert_eq!(
            custom.package_dir(InstallScope::Global, &id).unwrap(),
            Path::new("/opt/cyrup-packages/git-github.com-acme-pack")
        );
        // Project scope is unchanged: `<cwd>/.cyrup/packages/<id>` beside `<cwd>/.cyrup/packages.json`.
        assert_eq!(
            store.package_dir(InstallScope::Project, &id).unwrap(),
            Path::new("/repo/.cyrup/packages/git-github.com-acme-pack")
        );
    }

    /// …and a tree written by a pre-CFG-054 build is moved up, so upgrading does not orphan every
    /// installed git package (the registry row survives the change; the tree is what moves).
    #[test]
    fn the_legacy_doubled_root_is_migrated_once_and_never_clobbers() {
        let tmp = tempfile::tempdir().unwrap();
        let package_dir = tmp.path().join("packages");
        let legacy = package_dir.join("packages");
        std::fs::create_dir_all(legacy.join("git-github.com-acme-pack")).unwrap();
        std::fs::write(legacy.join("git-github.com-acme-pack/SKILL.md"), "x").unwrap();
        std::fs::write(legacy.join(".gitignore"), "*\n!.gitignore\n").unwrap();
        // A row the registry knows about, at the level that never doubled.
        std::fs::write(package_dir.join("packages.json"), "{}").unwrap();

        assert_eq!(migrate_legacy_doubled_packages_root(&package_dir), 2);
        let moved = package_dir.join("git-github.com-acme-pack");
        assert_eq!(std::fs::read_to_string(moved.join("SKILL.md")).unwrap(), "x");
        assert!(package_dir.join(".gitignore").exists());
        assert!(!legacy.exists(), "the emptied legacy root is removed");
        assert!(package_dir.join("packages.json").exists(), "the registry is untouched");

        // Idempotent, and a name already present at the destination is left alone rather than
        // overwriting the newer tree.
        assert_eq!(migrate_legacy_doubled_packages_root(&package_dir), 0);
        std::fs::create_dir_all(legacy.join("git-github.com-acme-pack")).unwrap();
        std::fs::write(legacy.join("git-github.com-acme-pack/SKILL.md"), "stale").unwrap();
        assert_eq!(migrate_legacy_doubled_packages_root(&package_dir), 0);
        assert_eq!(std::fs::read_to_string(moved.join("SKILL.md")).unwrap(), "x");
    }
}
