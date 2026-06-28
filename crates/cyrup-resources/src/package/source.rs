//! Package source + pin model (arch-09 §3.6, §6.4, R-09-020/021).

use std::path::PathBuf;

use cyrup_core::PackageId;

/// Where a package is fetched from. Git is the primary channel; local path for dev; OCI deferred.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum PackageSource {
    /// Primary channel (§7.6).
    Git {
        url: String,
        #[serde(default)]
        reff: PinRef,
    },
    /// Local dev install (no copy; referenced in place).
    Path { path: PathBuf },
    /// Deferred (R-09-021 candidate).
    Oci { reference: String },
}

/// Which git ref a package tracks. Tag/Commit are *pinned* and skipped by bulk update (R-09-020).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", tag = "ref", content = "value")]
pub enum PinRef {
    /// Default-branch HEAD — eligible for bulk update.
    #[default]
    Default,
    /// Tracked branch — eligible for bulk update.
    Branch(String),
    /// PINNED — skipped by bulk update (R-09-020).
    Tag(String),
    /// PINNED.
    Commit(String),
}

impl PinRef {
    /// True for Tag/Commit (R-09-020): bulk `update(All)` skips these.
    pub fn is_pinned(&self) -> bool {
        matches!(self, PinRef::Tag(_) | PinRef::Commit(_))
    }

    /// The git ref name to resolve/checkout, if any (branch/tag/commit).
    pub fn ref_name(&self) -> Option<&str> {
        match self {
            PinRef::Default => None,
            PinRef::Branch(s) | PinRef::Tag(s) | PinRef::Commit(s) => Some(s),
        }
    }
}

impl PackageSource {
    /// The pin ref, if this source carries one (only Git does).
    pub fn pin(&self) -> PinRef {
        match self {
            PackageSource::Git { reff, .. } => reff.clone(),
            _ => PinRef::Default,
        }
    }

    /// Stable identity of an install (used to upsert the registry record).
    pub fn package_id(&self) -> PackageId {
        match self {
            PackageSource::Git { url, .. } => PackageId::from(format!("git:{}", normalize_git(url))),
            PackageSource::Path { path } => {
                let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
                PackageId::from(format!("path:{}", abs.display()))
            }
            PackageSource::Oci { reference } => PackageId::from(format!("oci:{reference}")),
        }
    }
}

/// Normalize a git URL into a stable `host/user/repo` form (strip scheme, `.git`, trailing `/`).
fn normalize_git(url: &str) -> String {
    let mut s = url.trim();
    for prefix in ["https://", "http://", "ssh://", "git://", "file://"] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest;
            break;
        }
    }
    // scp-like `git@host:user/repo`
    let s = s.strip_prefix("git@").unwrap_or(s);
    let s = s.replace(':', "/");
    let s = s.trim_end_matches('/');
    s.strip_suffix(".git").unwrap_or(s).to_string()
}

/// A filesystem-safe directory name derived from a [`PackageId`].
pub fn id_dir_name(id: &PackageId) -> String {
    id.as_str()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' })
        .collect()
}
