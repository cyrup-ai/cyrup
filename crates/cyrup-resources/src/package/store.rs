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

    /// The packages root dir for a scope (`<global>/packages` or `<project>/.cyrup/packages`).
    pub fn packages_root(&self, scope: InstallScope) -> Option<PathBuf> {
        match scope {
            InstallScope::Global => Some(self.global_dir.join("packages")),
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
