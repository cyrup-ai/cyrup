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
pub use source::{PackageSource, PinRef};
pub use store::PackageStore;

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
