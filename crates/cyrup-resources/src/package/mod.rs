//! Packages — manifest-declared bundles of skills/prompts/themes/extensions, installed global or
//! project-local, with pinning/update/enable-disable (arch-09 §3.6, R-09-015..021).

pub mod git_url;
pub mod install;
pub mod lock;
pub mod manifest;
pub mod source;
pub mod store;

use std::collections::BTreeSet;

use cyrup_core::PackageId;

pub use git_url::{ParsedGitUrl, has_unsafe_git_install_part, parse_git_url};
pub use manifest::{ManifestKind, ManifestResources, ResolvedManifest, resolve_manifest};
pub use source::{PackageSource, PinRef, package_identity};
pub use store::{PackageStore, migrate_legacy_doubled_packages_root};

use crate::scope::InstallScope;

/// Per-resource enable/disable state within a package (R-09-018).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DisabledSet {
    #[serde(default)]
    pub skills: BTreeSet<String>,
    #[serde(default)]
    pub prompts: BTreeSet<String>,
    #[serde(default)]
    pub themes: BTreeSet<String>,
    #[serde(default)]
    pub extensions: BTreeSet<String>,
}

impl DisabledSet {
    pub fn is_disabled(&self, sel: &ResourceSelector) -> bool {
        match sel {
            ResourceSelector::Skill(n) => self.skills.contains(n),
            ResourceSelector::Prompt(n) => self.prompts.contains(n),
            ResourceSelector::Theme(n) => self.themes.contains(n),
            ResourceSelector::Extension(n) => self.extensions.contains(n),
        }
    }

    fn set(&mut self, sel: &ResourceSelector, disabled: bool) {
        let (set, name) = match sel {
            ResourceSelector::Skill(n) => (&mut self.skills, n),
            ResourceSelector::Prompt(n) => (&mut self.prompts, n),
            ResourceSelector::Theme(n) => (&mut self.themes, n),
            ResourceSelector::Extension(n) => (&mut self.extensions, n),
        };
        if disabled {
            set.insert(name.clone());
        } else {
            set.remove(name);
        }
    }
}

/// Identifies a single resource within a package for enable/disable (R-09-018).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceSelector {
    Skill(String),
    Prompt(String),
    Theme(String),
    Extension(String),
}

/// A persisted install record.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub id: PackageId,
    pub source: PackageSource,
    pub scope: InstallScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_commit: Option<String>,
    /// RFC3339-ish timestamp (epoch-seconds fallback to avoid an extra time dep).
    pub installed_at: String,
    #[serde(default)]
    pub disabled: DisabledSet,
}

/// Per-resource filters declared alongside a settings-declared package source (Pi `PackageFilter`,
/// package-manager.ts:184-190, read off the object form of a `packages` entry).
///
/// Two modes, chosen by [`Self::autoload`] (Pi `collectPackageResources`, package-manager.ts:2079-2092):
///
/// - **include filter** (`autoload` absent or `true`) — start from the package's default resources
///   and narrow. `None` for a type keeps that type's defaults; `Some` carries `applyPatterns`
///   patterns; an explicitly EMPTY list disables the type outright (`applyPackageFilter`, :2147-2171).
/// - **delta** (`autoload == Some(false)`) — start from NOTHING and add back only what the patterns
///   name (`applyPackageDeltaFilter`, :2173-2189). `None` or an empty list for a type therefore
///   contributes nothing at all, which is the whole point of opting a package out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageFilter {
    /// Pi `PackageSource.autoload` (settings-manager.ts:79). Only `Some(false)` is load-bearing.
    pub autoload: Option<bool>,
    pub extensions: Option<Vec<String>>,
    pub skills: Option<Vec<String>>,
    pub prompts: Option<Vec<String>>,
    pub themes: Option<Vec<String>>,
}

impl PackageFilter {
    pub fn is_empty(&self) -> bool {
        self.autoload.is_none()
            && self.extensions.is_none()
            && self.skills.is_none()
            && self.prompts.is_none()
            && self.themes.is_none()
    }

    /// Whether this filter is the `autoload: false` DELTA form (Pi `filter.autoload === false`,
    /// package-manager.ts:2084 — only an explicit `false` takes the branch).
    pub fn is_delta(&self) -> bool {
        self.autoload == Some(false)
    }
}

/// A package **declared in settings** (`settings.json` `packages: [...]`) as opposed to one recorded
/// by `cyrup install` into `packages.json`.
///
/// Pi has no separate install registry: `PackageManager.resolve()` re-reads
/// `projectSettings.packages` + `globalSettings.packages` on EVERY call and resolves each entry to a
/// working tree (installing it on demand), then collects its resources
/// (package-manager.ts:891-901,1224-1283). cyrup keeps both channels: the install registry
/// ([`InstalledPackages`]) and this settings channel, which the session builder feeds into
/// discovery so a declared package is never inert (CFG-003).
///
/// **[CYRUP-DELTA] no on-demand install.** Pi's `resolvePackageSources` will `npm install` / `git
/// clone` a missing source mid-resolve. cyrup does not perform network installs during session
/// assembly: a `Path` source resolves directly, and a `git:`/`npm:`-style source resolves only if it
/// is ALREADY installed at the matching scope. An unresolvable entry is surfaced as a
/// [`crate::ResourceDiagnostic`], never silently dropped and never fatal.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfiguredPackage {
    /// The raw `source` string exactly as written in settings.
    pub source: String,
    /// Which settings layer declared it — Pi's `scope: "project" | "user"` (package-manager.ts:891-898).
    pub scope: InstallScope,
    pub filter: PackageFilter,
}

/// Persisted install registry (one per scope).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackages {
    #[serde(default)]
    pub packages: Vec<InstalledPackage>,
}

impl InstalledPackages {
    pub fn find(&self, id: &PackageId) -> Option<&InstalledPackage> {
        self.packages.iter().find(|p| &p.id == id)
    }

    pub fn find_mut(&mut self, id: &PackageId) -> Option<&mut InstalledPackage> {
        self.packages.iter_mut().find(|p| &p.id == id)
    }

    /// Insert or replace by id.
    pub fn upsert(&mut self, pkg: InstalledPackage) {
        if let Some(slot) = self.find_mut(&pkg.id) {
            *slot = pkg;
        } else {
            self.packages.push(pkg);
        }
    }

    pub fn remove(&mut self, id: &PackageId) -> bool {
        let before = self.packages.len();
        self.packages.retain(|p| &p.id != id);
        self.packages.len() != before
    }
}

/// Which packages a bulk operation targets.
#[derive(Clone, Debug)]
pub enum UpdateTarget {
    One(PackageId),
    /// Skips pinned packages (R-09-020).
    All,
}

/// Outcome of an update run.
#[derive(Clone, Debug, Default)]
pub struct UpdateReport {
    pub updated: Vec<PackageId>,
    pub skipped_pinned: Vec<PackageId>,
    pub failed: Vec<(PackageId, String)>,
}

/// Surfaced to the user at every install (R-09-019).
#[derive(Clone, Debug)]
pub struct SecurityNotice {
    pub message: &'static str,
    pub source: PackageSource,
}

pub const SECURITY_CAVEAT: &str = "Packages run with full system access: extensions execute code \
and skills can instruct the model to run anything. Only install packages you trust.";

/// A current RFC3339-ish timestamp. Falls back to epoch seconds to avoid pulling in a time crate.
pub(crate) fn now_stamp() -> String {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => format!("{}", d.as_secs()),
        Err(_) => "0".to_string(),
    }
}
