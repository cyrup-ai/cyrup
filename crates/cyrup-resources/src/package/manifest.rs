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

/// Which resource family a manifest entry list belongs to, controlling how a *directory* source
/// entry expands to resource files (Pi `collectResourceFiles`, package-manager.ts:633-639).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestResourceType {
    Skills,
    Prompts,
    Themes,
    /// Extensions resolve to crate directories (not files); no file-level expansion ([CYRUP-DELTA],
    /// npm `.ts`/`.js` channel dropped, R-09-021).
    Extensions,
}

fn absolutize(dir: &Path, kind: ManifestKind, res: ManifestResources) -> ResolvedManifest {
    ResolvedManifest {
        kind,
        extensions: resolve_entries(dir, res.extensions, ManifestResourceType::Extensions),
        skills: resolve_entries(dir, res.skills, ManifestResourceType::Skills),
        prompts: resolve_entries(dir, res.prompts, ManifestResourceType::Prompts),
        themes: resolve_entries(dir, res.themes, ManifestResourceType::Themes),
    }
}

/// Resolve manifest entries to absolute paths, honoring Pi's **override-pattern** model
/// (package-manager.ts:2144-2216, 696-735).
///
/// A manifest resource list mixes *source entries* and *override patterns* (`collectManifestFiles`):
/// - non-override entries (`isOverridePattern` false, package-manager.ts:274) are resolved into the
///   candidate set — glob-expanded if they contain `*`/`?` (`hasGlobPattern`,
///   `collectFilesFromManifestEntries`, 2201-2215), or joined literally otherwise;
/// - override entries (`!`/`+`/`-`) are then applied via [`apply_patterns`] to selectively
///   enable/disable members of that candidate set (`applyPatterns`, 718-771).
///
/// The result is the set of *enabled* paths (Pi: `Array.from(enabledByManifest)`), so an entry like
/// `!skills/internal/**` correctly drops the matching sources instead of being treated as a literal
/// path that matches nothing.
fn resolve_entries(dir: &Path, entries: Vec<PathBuf>, rtype: ManifestResourceType) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut overrides: Vec<String> = Vec::new();
    for entry in entries {
        let s = entry.to_string_lossy().to_string();
        if is_override_pattern(&s) {
            overrides.push(s);
        } else {
            sources.push(entry);
        }
    }

    let mut all: Vec<PathBuf> = Vec::new();
    for entry in sources {
        let s = entry.to_string_lossy();
        if s.contains('*') || s.contains('?') {
            expand_glob(dir, &s, &mut all);
        } else if entry.is_absolute() {
            all.push(entry);
        } else {
            all.push(dir.join(entry));
        }
    }

    if overrides.is_empty() {
        return all;
    }

    // Override patterns present: match Pi's collect-then-filter order. Pi resolves every source
    // entry — including a *plain directory* — into concrete resource files via
    // `collectFilesFromPaths`/`collectResourceFiles` *before* `applyPatterns`
    // (package-manager.ts:2201-2215, 2148-2156), so a pattern like `!skills/internal` filters the
    // expanded `SKILL.md` files rather than a raw directory entry that matches nothing. Extensions
    // keep directory granularity ([CYRUP-DELTA], no file channel).
    let candidates = match rtype {
        ManifestResourceType::Extensions => all,
        _ => collect_files_from_paths(&all, rtype),
    };
    apply_patterns(dir, &candidates, &overrides)
}

/// Resolve a set of source paths to concrete resource files (1:1 with Pi `collectFilesFromPaths`,
/// package-manager.ts:2407-2424): a file is kept as-is; a directory is expanded to its resource
/// files via [`collect_resource_files`]. Non-existent paths are skipped.
fn collect_files_from_paths(paths: &[PathBuf], rtype: ManifestResourceType) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    for p in paths {
        if p.is_file() {
            out.push(p.clone());
        } else if p.is_dir() {
            collect_resource_files(p, rtype, &mut out);
        }
    }
    out
}

/// Expand a directory into its resource files (1:1 with Pi `collectResourceFiles`,
/// package-manager.ts:633-639): skills via the `SKILL.md`-then-`.md` walk (`collectSkillEntries`
/// "pi", 347-415); prompts/themes via a recursive extension filter (`collectFiles`, 295-343).
fn collect_resource_files(dir: &Path, rtype: ManifestResourceType, out: &mut Vec<PathBuf>) {
    match rtype {
        ManifestResourceType::Skills => collect_skill_files(dir, true, out),
        ManifestResourceType::Prompts => collect_files_with_ext(dir, "md", out),
        ManifestResourceType::Themes => collect_files_with_ext(dir, "json", out),
        ManifestResourceType::Extensions => out.push(dir.to_path_buf()),
    }
}

/// Port of Pi `collectSkillEntries(dir, "pi")` (package-manager.ts:347-415): a directory containing
/// `SKILL.md` yields just that file and stops; otherwise direct `.md` children at the scan root are
/// skills and every subdirectory is recursed. Dot-entries and `node_modules` are skipped.
fn collect_skill_files(dir: &Path, root_level: bool, out: &mut Vec<PathBuf>) {
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() {
        out.push(skill_md);
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for path in children {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_skill_files(&path, false, out);
        } else if root_level && path.is_file() && path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Recursively collect files with the given extension (1:1 with Pi `collectFiles`,
/// package-manager.ts:295-343). Dot-entries and `node_modules` are skipped.
fn collect_files_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();
    for path in children {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if path.is_dir() {
            collect_files_with_ext(&path, ext, out);
        } else if path.is_file() && path.extension().is_some_and(|e| e == ext) {
            out.push(path);
        }
    }
}

/// An entry that selectively enables/disables resources rather than naming a source path
/// (`isOverridePattern`, package-manager.ts:274-276): a leading `!` (exclude glob), `+`
/// (force-include exact), or `-` (force-exclude exact).
fn is_override_pattern(s: &str) -> bool {
    s.starts_with('!') || s.starts_with('+') || s.starts_with('-')
}

/// Apply override patterns to a candidate set, returning the enabled members (`applyPatterns`,
/// package-manager.ts:718-771). The manifest layer only ever passes override patterns here (plain
/// includes have already been resolved into `all`), so there are no plain include patterns:
/// the result starts as the full candidate set, then plain excludes (`!`, glob) are removed,
/// force-includes (`+`, exact) are added back from the candidate set overriding excludes, and
/// force-excludes (`-`, exact) are removed last overriding everything.
fn apply_patterns(base: &Path, all: &[PathBuf], patterns: &[String]) -> Vec<PathBuf> {
    let mut excludes: Vec<String> = Vec::new();
    let mut force_includes: Vec<String> = Vec::new();
    let mut force_excludes: Vec<String> = Vec::new();
    for p in patterns {
        if let Some(rest) = p.strip_prefix('+') {
            force_includes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('-') {
            force_excludes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('!') {
            excludes.push(rest.to_string());
        }
    }

    // Step 1+2: result = all candidates, minus any matching a plain exclude (glob).
    let mut result: Vec<PathBuf> = if excludes.is_empty() {
        all.to_vec()
    } else {
        all.iter()
            .filter(|p| !matches_any_pattern(base, p, &excludes))
            .cloned()
            .collect()
    };

    // Step 3: force-include — add back from the candidate set (exact), overriding excludes.
    if !force_includes.is_empty() {
        for p in all {
            if !result.contains(p) && matches_any_exact(base, p, &force_includes) {
                result.push(p.clone());
            }
        }
    }

    // Step 4: force-exclude — remove even if included or force-included (exact).
    if !force_excludes.is_empty() {
        result.retain(|p| !matches_any_exact(base, p, &force_excludes));
    }

    result
}

/// An entry in a settings `skills`/`prompts`/`themes` array is a *pattern* (vs a plain source path)
/// if it starts with `!`/`+`/`-` or contains a glob `*`/`?` (1:1 with Pi `isPattern`,
/// package-manager.ts:270-272 / `splitPatterns` 282-293).
fn is_pattern(s: &str) -> bool {
    s.starts_with('!')
        || s.starts_with('+')
        || s.starts_with('-')
        || s.contains('*')
        || s.contains('?')
}

/// Resolve the **plain** (positive-listing) entries of a settings `skills`/`prompts`/`themes` array
/// into the set of *enabled* resource files (1:1 with Pi `resolveLocalEntries`,
/// package-manager.ts:2218-2239). Each non-pattern entry is resolved relative to `base`
/// (`resolvePathFromBase`, trimmed), expanded to concrete resource files
/// (`collectFilesFromPaths`), then the pattern subset (`!`/`+`/`-`/glob) selects which stay enabled
/// (`applyPatterns`). These files are loaded at the settings tier (`source:"local"`), which Pi
/// ranks *above* auto-discovered files of the same scope (`resourcePrecedenceRank` 184-188).
pub(crate) fn resolve_local_entries(
    base: &Path,
    entries: &[String],
    rtype: ManifestResourceType,
) -> Vec<PathBuf> {
    if entries.is_empty() {
        return Vec::new();
    }
    let mut plain: Vec<PathBuf> = Vec::new();
    let mut patterns: Vec<String> = Vec::new();
    for entry in entries {
        if is_pattern(entry) {
            patterns.push(entry.clone());
        } else {
            // `resolvePathFromBase` with `trim: true`: a trimmed, base-relative (or absolute) path.
            let trimmed = entry.trim();
            let p = PathBuf::from(trimmed);
            plain.push(if p.is_absolute() {
                p
            } else {
                base.join(trimmed)
            });
        }
    }
    let all = collect_files_from_paths(&plain, rtype);
    apply_patterns_full(base, &all, &patterns)
}

/// Full `applyPatterns` including the leading **include** step (package-manager.ts:727-773), used
/// for settings local entries where a glob entry acts as a positive include-filter. (The manifest
/// variant [`apply_patterns`] omits the include step because manifest source entries are already
/// resolved into the candidate set.) Order: includes (or all) → excludes (`!`) → force-includes
/// (`+`) → force-excludes (`-`).
fn apply_patterns_full(base: &Path, all: &[PathBuf], patterns: &[String]) -> Vec<PathBuf> {
    let mut includes: Vec<String> = Vec::new();
    let mut excludes: Vec<String> = Vec::new();
    let mut force_includes: Vec<String> = Vec::new();
    let mut force_excludes: Vec<String> = Vec::new();
    for p in patterns {
        if let Some(rest) = p.strip_prefix('+') {
            force_includes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('-') {
            force_excludes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('!') {
            excludes.push(rest.to_string());
        } else {
            includes.push(p.clone());
        }
    }

    // Step 1: apply includes (or keep all when there are none).
    let mut result: Vec<PathBuf> = if includes.is_empty() {
        all.to_vec()
    } else {
        all.iter()
            .filter(|p| matches_any_pattern(base, p, &includes))
            .cloned()
            .collect()
    };
    // Step 2: drop plain excludes (glob).
    if !excludes.is_empty() {
        result.retain(|p| !matches_any_pattern(base, p, &excludes));
    }
    // Step 3: force-include exact (add back from the candidate set, overriding excludes).
    if !force_includes.is_empty() {
        for p in all {
            if !result.contains(p) && matches_any_exact(base, p, &force_includes) {
                result.push(p.clone());
            }
        }
    }
    // Step 4: force-exclude exact (remove last, overriding everything).
    if !force_excludes.is_empty() {
        result.retain(|p| !matches_any_exact(base, p, &force_excludes));
    }
    result
}

/// Decide whether an auto-discovered loose resource at `path` is enabled by a settings override list
/// (1:1 with Pi `isEnabledByOverrides`, package-manager.ts:700-717). Only `!`/`+`/`-` entries are
/// considered (`getOverridePatterns`, 696-698): a `!` glob exclude disables, a `+` exact
/// force-include re-enables (overriding excludes), and a `-` exact force-exclude disables last
/// (overriding force-includes). With no override entries everything stays enabled.
pub(crate) fn is_enabled_by_overrides(base: &Path, path: &Path, patterns: &[String]) -> bool {
    let mut excludes: Vec<String> = Vec::new();
    let mut force_includes: Vec<String> = Vec::new();
    let mut force_excludes: Vec<String> = Vec::new();
    for p in patterns {
        if let Some(rest) = p.strip_prefix('+') {
            force_includes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('-') {
            force_excludes.push(rest.to_string());
        } else if let Some(rest) = p.strip_prefix('!') {
            excludes.push(rest.to_string());
        }
    }

    let mut enabled = true;
    if !excludes.is_empty() && matches_any_pattern(base, path, &excludes) {
        enabled = false;
    }
    if !force_includes.is_empty() && matches_any_exact(base, path, &force_includes) {
        enabled = true;
    }
    if !force_excludes.is_empty() && matches_any_exact(base, path, &force_excludes) {
        enabled = false;
    }
    enabled
}

/// Posix-normalized path string (Pi `toPosixPath`, package-manager.ts:212-214).
fn to_posix(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Posix-normalized path of `path` relative to `base` (Pi `relative` + `toPosixPath`).
fn rel_posix(base: &Path, path: &Path) -> String {
    to_posix(path.strip_prefix(base).unwrap_or(path))
}

/// Glob-match `text` against `pattern` (minimatch). A pattern that fails to compile matches nothing.
fn glob_match(pattern: &str, text: &str) -> bool {
    globset::Glob::new(pattern)
        .map(|g| g.compile_matcher().is_match(text))
        .unwrap_or(false)
}

/// Strip a leading `./` / `.\` and posix-normalize (Pi `normalizeExactPattern`,
/// package-manager.ts:671-674).
fn normalize_exact(pattern: &str) -> String {
    let stripped = pattern
        .strip_prefix("./")
        .or_else(|| pattern.strip_prefix(".\\"))
        .unwrap_or(pattern);
    stripped.replace('\\', "/")
}

/// True if `path` glob-matches any pattern (`matchesAnyPattern`, package-manager.ts:643-669):
/// tested against the base-relative posix path, the basename, and the absolute posix path; for a
/// `SKILL.md` file the parent directory's relative path, basename, and absolute path are also
/// tested.
fn matches_any_pattern(base: &Path, path: &Path, patterns: &[String]) -> bool {
    let rel = rel_posix(base, path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let abs = to_posix(path);
    let is_skill = name == "SKILL.md";
    let parent = if is_skill { path.parent() } else { None };
    let parent_rel = parent.map(|p| rel_posix(base, p));
    let parent_name = parent
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().to_string());
    let parent_abs = parent.map(to_posix);

    patterns.iter().any(|pat| {
        let np = pat.replace('\\', "/");
        if glob_match(&np, &rel) || glob_match(&np, &name) || glob_match(&np, &abs) {
            return true;
        }
        if !is_skill {
            return false;
        }
        parent_rel.as_deref().is_some_and(|p| glob_match(&np, p))
            || parent_name.as_deref().is_some_and(|p| glob_match(&np, p))
            || parent_abs.as_deref().is_some_and(|p| glob_match(&np, p))
    })
}

/// True if `path` exactly equals any pattern (`matchesAnyExactPattern`,
/// package-manager.ts:676-693): the base-relative posix path or absolute posix path equals the
/// normalized pattern; for a `SKILL.md` file the parent directory's relative or absolute path may
/// also match.
fn matches_any_exact(base: &Path, path: &Path, patterns: &[String]) -> bool {
    let rel = rel_posix(base, path);
    let abs = to_posix(path);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_skill = name == "SKILL.md";
    let parent = if is_skill { path.parent() } else { None };
    let parent_rel = parent.map(|p| rel_posix(base, p));
    let parent_abs = parent.map(to_posix);

    patterns.iter().any(|pat| {
        let np = normalize_exact(pat);
        if np == rel || np == abs {
            return true;
        }
        if !is_skill {
            return false;
        }
        parent_rel.as_deref() == Some(np.as_str()) || parent_abs.as_deref() == Some(np.as_str())
    })
}

/// Expand a glob `pattern` (relative to `dir`) into absolute matches over the package tree. A
/// leading `./` is stripped (Pi `normalizeExactPattern`). `*`/`?` do not cross `/`; `**` does
/// (globset default), matching Pi's glob semantics. Both files and directories match (`nodir:false`).
fn expand_glob(dir: &Path, pattern: &str, out: &mut Vec<PathBuf>) {
    let normalized = pattern.strip_prefix("./").unwrap_or(pattern);
    let Ok(glob) = globset::Glob::new(normalized) else {
        return;
    };
    let matcher = glob.compile_matcher();
    let mut matches: Vec<PathBuf> = Vec::new();
    walk_tree(dir, dir, &matcher, &mut matches);
    matches.sort();
    out.extend(matches);
}

/// Recursively collect every path under `root` whose root-relative path matches `matcher`.
/// Dot-directories and `node_modules` are skipped (consistent with the skill/resource walk).
fn walk_tree(root: &Path, dir: &Path, matcher: &globset::GlobMatcher, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
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
