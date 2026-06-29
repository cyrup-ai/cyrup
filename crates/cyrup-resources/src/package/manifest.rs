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
    ResolvedManifest {
        kind,
        extensions: resolve_entries(dir, res.extensions),
        skills: resolve_entries(dir, res.skills),
        prompts: resolve_entries(dir, res.prompts),
        themes: resolve_entries(dir, res.themes),
    }
}

/// Resolve manifest entries to absolute paths, glob-expanding any entry containing `*`/`?` against
/// the package root (Pi `collectFilesFromManifestEntries`, package-manager.ts:2201-2215, which
/// `globSync`s glob entries and `resolve`s plain ones). A plain entry resolves literally; a glob
/// entry expands to every path under the package tree whose root-relative path matches.
fn resolve_entries(dir: &Path, entries: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let s = entry.to_string_lossy();
        if s.contains('*') || s.contains('?') {
            expand_glob(dir, &s, &mut out);
        } else if entry.is_absolute() {
            out.push(entry);
        } else {
            out.push(dir.join(entry));
        }
    }
    out
}

/// Expand a glob `pattern` (relative to `dir`) into absolute matches over the package tree. A
/// leading `./` is stripped (Pi `normalizeExactPattern`). `*`/`?` do not cross `/`; `**` does
/// (globset default), matching Pi's glob semantics. Both files and directories match (`nodir:false`).
fn expand_glob(dir: &Path, pattern: &str, out: &mut Vec<PathBuf>) {
    let normalized = pattern.strip_prefix("./").unwrap_or(pattern);
    let Ok(glob) = globset::Glob::new(normalized) else { return };
    let matcher = glob.compile_matcher();
    let mut matches: Vec<PathBuf> = Vec::new();
    walk_tree(dir, dir, &matcher, &mut matches);
    matches.sort();
    out.extend(matches);
}

/// Recursively collect every path under `root` whose root-relative path matches `matcher`.
/// Dot-directories and `node_modules` are skipped (consistent with the skill/resource walk).
fn walk_tree(
    root: &Path,
    dir: &Path,
    matcher: &globset::GlobMatcher,
    out: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root)
            && matcher.is_match(rel)
        {
            out.push(path.clone());
        }
        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            walk_tree(root, &path, matcher, out);
        }
    }
}
