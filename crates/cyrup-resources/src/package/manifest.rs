//! Package manifest resolution (arch-09 §4.2, R-09-015/016).
//!
//! Resolution order inside a package tree:
//! 1. `cyrup.toml` (native, preferred)
//! 2. `package.json` `pi` / `cyrup` key (Pi cross-harness compat)
//! 3. auto-discovery of conventional dirs (`extensions/ skills/ prompts/ themes/`).

use std::path::{Path, PathBuf};

use crate::error::ResourceError;

/// Package metadata block.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageMeta {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
}

/// Declared resource paths (relative to the package root).
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManifestResources {
    #[serde(default)]
    pub extensions: Vec<PathBuf>,
    #[serde(default)]
    pub skills: Vec<PathBuf>,
    #[serde(default)]
    pub prompts: Vec<PathBuf>,
    #[serde(default)]
    pub themes: Vec<PathBuf>,
}

/// Native `cyrup.toml` manifest.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CyrupManifest {
    pub package: PackageMeta,
    #[serde(default)]
    pub resources: ManifestResources,
}

/// `package.json` with a `pi`/`cyrup` resource key (Pi compat).
#[derive(Debug, Clone, serde::Deserialize)]
struct PiPackageJson {
    #[serde(default)]
    pi: Option<ManifestResources>,
    #[serde(default)]
    cyrup: Option<ManifestResources>,
}

/// How a package's resources were resolved (for diagnostics).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestKind {
    CyrupToml,
    PackageJson,
    AutoDiscovered,
}

/// Resolved, absolute resource paths for a package plus how they were found.
#[derive(Debug, Clone)]
pub struct ResolvedManifest {
    pub kind: ManifestKind,
    pub extensions: Vec<PathBuf>,
    pub skills: Vec<PathBuf>,
    pub prompts: Vec<PathBuf>,
    pub themes: Vec<PathBuf>,
}

/// Resolve a package tree's manifest into absolute resource paths (R-09-015/016).
pub fn resolve_manifest(dir: &Path) -> Result<ResolvedManifest, ResourceError> {
    let cyrup_toml = dir.join("cyrup.toml");
    if cyrup_toml.is_file() {
        let text = std::fs::read_to_string(&cyrup_toml)?;
        let manifest: CyrupManifest = toml::from_str(&text)?;
        return Ok(absolutize(dir, ManifestKind::CyrupToml, manifest.resources));
    }

    let package_json = dir.join("package.json");
    if package_json.is_file() {
        let text = std::fs::read_to_string(&package_json)?;
        let parsed: PiPackageJson = serde_json::from_str(&text)?;
        if let Some(res) = parsed.pi.or(parsed.cyrup) {
            return Ok(absolutize(dir, ManifestKind::PackageJson, res));
        }
    }

    // Auto-discovery of conventional dirs.
    let mut res = ManifestResources::default();
    for (field, name) in [
        (&mut res.extensions, "extensions"),
        (&mut res.skills, "skills"),
        (&mut res.prompts, "prompts"),
        (&mut res.themes, "themes"),
    ] {
        if dir.join(name).is_dir() {
            field.push(PathBuf::from(name));
        }
    }
    Ok(absolutize(dir, ManifestKind::AutoDiscovered, res))
}

fn absolutize(dir: &Path, kind: ManifestKind, res: ManifestResources) -> ResolvedManifest {
    let join = |paths: Vec<PathBuf>| -> Vec<PathBuf> {
        paths.into_iter().map(|p| if p.is_absolute() { p } else { dir.join(p) }).collect()
    };
    ResolvedManifest {
        kind,
        extensions: join(res.extensions),
        skills: join(res.skills),
        prompts: join(res.prompts),
        themes: join(res.themes),
    }
}
