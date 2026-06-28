//! Scope, origin, and install-scope (arch-09 §3.1).
//!
//! [`ResourceScope`] is an ascending priority: a higher value wins a same-name collision
//! (R-09-024 — built-in -> global -> project -> discovered -> cli, later wins). Packages rank
//! just *below* loose files of the same locality so a user's loose override beats a package.

use std::path::PathBuf;

use cyrup_core::{ExtensionId, PackageId};

/// Where a resource came from. Ascending priority = later wins (R-09-024).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceScope {
    /// Compiled-in (themes: `dark`, `light` — R-09-011).
    Builtin = 0,
    /// Package installed at global scope.
    GlobalPackage = 1,
    /// Global config dir (loose files).
    Global = 2,
    /// Package installed project-local (trust-gated).
    ProjectPackage = 3,
    /// `.cyrup/*` and `.agents/skills` (trust-gated).
    Project = 4,
    /// Contributed via `resources_discover` (R-09-022).
    Discovered = 5,
    /// Explicit `--skill` / `--prompt-template` / `--theme` (highest).
    Cli = 6,
}

/// Provenance detail kept for diagnostics / `list`.
#[derive(Clone, Debug)]
pub enum ResourceOrigin {
    Builtin,
    LooseFile { scope: ResourceScope, root: PathBuf },
    Package { id: PackageId, scope: ResourceScope },
    Cli { path: PathBuf },
    Extension { ext: ExtensionId },
}

/// Install destination for a package (R-09-017). Project-local installs are trust-gated.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallScope {
    Global,
    Project,
}

impl InstallScope {
    /// The `ResourceScope` tier a package's resources enter at.
    pub fn package_resource_scope(self) -> ResourceScope {
        match self {
            InstallScope::Global => ResourceScope::GlobalPackage,
            InstallScope::Project => ResourceScope::ProjectPackage,
        }
    }
}
