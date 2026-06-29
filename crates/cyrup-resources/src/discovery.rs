//! Unified resource discovery + loader (arch-09 §3.2, §6.1, R-09-022..024).
//!
//! `discover()` fans across roots (built-in, global, global-agents, project walk, installed
//! packages, CLI flags, `resources_discover` contributions), parses each kind, and merges into a
//! [`ResourceRegistry`] with deterministic same-name precedence (R-09-024).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cyrup_core::CancelToken;

use crate::error::{ResourceDiagnostic, ResourceError, ResourceKind, ResourceWarning};
use crate::key::ResourceKey;
use crate::package::store::installed_dir;
use crate::package::{DisabledSet, InstalledPackages, ResourceSelector, resolve_manifest};
use crate::prompt::PromptTemplate;
use crate::scope::{InstallScope, ResourceOrigin, ResourceScope};
use crate::skill::Skill;
use crate::theme::{Theme, builtin_themes};

/// Anything keyed + scoped that can live in a [`ResourceSet`].
pub trait Named {
    fn key(&self) -> &ResourceKey;
    fn scope(&self) -> ResourceScope;
}

/// Name-keyed winner-takes set with the full candidate list retained for diagnostics.
#[derive(Debug, Clone)]
pub struct ResourceSet<T: Named + Clone> {
    by_key: HashMap<ResourceKey, T>,
    all: Vec<T>,
}

impl<T: Named + Clone> Default for ResourceSet<T> {
    fn default() -> Self {
        Self { by_key: HashMap::new(), all: Vec::new() }
    }
}

impl<T: Named + Clone> ResourceSet<T> {
    /// Build from all candidates, applying precedence: higher [`ResourceScope`] wins a same-name
    /// collision; within a scope, insertion (path) order decides (R-09-024).
    pub fn build(mut all: Vec<T>) -> Self {
        // Stable sort ascending by scope so higher scopes are inserted last and overwrite.
        all.sort_by_key(|c| c.scope());
        let mut by_key = HashMap::new();
        for c in &all {
            by_key.insert(c.key().clone(), c.clone());
        }
        Self { by_key, all }
    }

    /// The winning resource for a normalized name.
    pub fn get(&self, key: &ResourceKey) -> Option<&T> {
        self.by_key.get(key)
    }

    /// Lookup by raw (un-normalized) name.
    pub fn get_name(&self, name: &str) -> Option<&T> {
        self.by_key.get(&ResourceKey::normalize(name))
    }

    pub fn contains(&self, name: &str) -> bool {
        self.by_key.contains_key(&ResourceKey::normalize(name))
    }

    /// Iterate the winning resources (order unspecified).
    pub fn winners(&self) -> impl Iterator<Item = &T> {
        self.by_key.values()
    }

    /// Every candidate, including shadowed ones (for `list`/diagnostics).
    pub fn all(&self) -> &[T] {
        &self.all
    }

    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// Explicit `--skill` / `--prompt-template` / `--theme` paths.
#[derive(Clone, Debug, Default)]
pub struct CliResourcePaths {
    pub skills: Vec<PathBuf>,
    pub prompts: Vec<PathBuf>,
    pub themes: Vec<PathBuf>,
}

/// Paths contributed by extensions via `resources_discover` (R-09-022). Shape mirrors Pi's record.
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredPaths {
    #[serde(default)]
    pub skill_paths: Vec<PathBuf>,
    #[serde(default)]
    pub prompt_paths: Vec<PathBuf>,
    #[serde(default)]
    pub theme_paths: Vec<PathBuf>,
}

/// Everything discovery needs, passed in (keeps `cyrup-resources` core-only).
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub cwd: PathBuf,
    pub global_dir: PathBuf,
    pub global_agents_dir: PathBuf,
    pub project_root: Option<PathBuf>,
    /// DI-11 trust decision from cyrup-config (R-09-003/008/012).
    pub trusted_project: bool,
    pub enable_skills: bool,
    pub enable_prompts: bool,
    pub enable_themes: bool,
    pub cli: CliResourcePaths,
    pub installed: InstalledPackages,
    /// From `resources_discover` (R-09-022).
    pub extra: DiscoveredPaths,
    /// Top-level per-resource enable/disable state.
    pub disabled: DisabledSet,
}

impl DiscoveryConfig {
    /// A config with all kinds enabled, `global_agents_dir` derived as `<global>/agents`, no
    /// project, untrusted. Callers (cyrup-config) override fields as needed.
    pub fn new(cwd: impl Into<PathBuf>, global_dir: impl Into<PathBuf>) -> Self {
        let global_dir = global_dir.into();
        let global_agents_dir = global_dir.join("agents");
        Self {
            cwd: cwd.into(),
            global_dir,
            global_agents_dir,
            project_root: None,
            trusted_project: false,
            enable_skills: true,
            enable_prompts: true,
            enable_themes: true,
            cli: CliResourcePaths::default(),
            installed: InstalledPackages::default(),
            extra: DiscoveredPaths::default(),
            disabled: DisabledSet::default(),
        }
    }
}

/// The immutable snapshot the rest of the app reads (swapped atomically on `/reload`).
#[derive(Debug, Clone, Default)]
pub struct ResourceRegistry {
    pub skills: ResourceSet<Skill>,
    pub prompts: ResourceSet<PromptTemplate>,
    pub themes: ResourceSet<Theme>,
    /// Extension crate dirs found in packages — handed to cyrup-ext to build.
    pub ext_crate_paths: Vec<PathBuf>,
}

/// The result of a discovery pass.
#[derive(Debug)]
pub struct DiscoveryReport {
    pub registry: ResourceRegistry,
    pub warnings: Vec<ResourceWarning>,
    /// Structured diagnostics (warning | error | collision), 1:1 with Pi's `ResourceDiagnostic`
    /// surface (skills.ts; resource-loader.ts:8). Surfaced at startup and on `/reload`.
    pub diagnostics: Vec<ResourceDiagnostic>,
}

/// One-shot discovery. Blocking fs work runs on `spawn_blocking`; cancellation aborts the wait.
pub async fn discover(
    cfg: &DiscoveryConfig,
    cancel: CancelToken,
) -> Result<DiscoveryReport, ResourceError> {
    if cancel.is_cancelled() {
        return Err(ResourceError::Cancelled);
    }
    let cfg = cfg.clone();
    let join = tokio::task::spawn_blocking(move || discover_blocking(&cfg));
    match cancel.run_until_cancelled(join).await {
        Some(joined) => joined.map_err(|e| ResourceError::Core(cyrup_core::CoreError::Io(
            std::io::Error::other(e.to_string()),
        )))?,
        None => Err(ResourceError::Cancelled),
    }
}

fn discover_blocking(cfg: &DiscoveryConfig) -> Result<DiscoveryReport, ResourceError> {
    let mut skills: Vec<Skill> = Vec::new();
    let mut prompts: Vec<PromptTemplate> = Vec::new();
    let mut themes: Vec<Theme> = Vec::new();
    let mut warnings: Vec<ResourceWarning> = Vec::new();
    let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
    let mut ext_paths: Vec<PathBuf> = Vec::new();

    // --- built-in themes (R-09-011) ---
    if cfg.enable_themes {
        themes.extend(builtin_themes());
    }

    // --- global loose resources (R-09-002/008/012) ---
    if cfg.enable_skills {
        for root in [cfg.global_dir.join("skills"), cfg.global_agents_dir.join("skills")] {
            scan_skill_root(
                &root,
                ResourceScope::Global,
                &root,
                &mut skills,
                &mut warnings,
                &mut diagnostics,
            );
        }
    }
    if cfg.enable_prompts {
        let root = cfg.global_dir.join("prompts");
        scan_prompt_root(&root, ResourceScope::Global, &root, &mut prompts, &mut warnings);
    }
    if cfg.enable_themes {
        let root = cfg.global_dir.join("themes");
        scan_theme_root(&root, ResourceScope::Global, &root, &mut themes, &mut warnings);
    }

    // --- installed packages (R-09-015/016/017/018) ---
    for pkg in &cfg.installed.packages {
        if pkg.scope == InstallScope::Project && !cfg.trusted_project {
            continue; // fail-closed trust gate
        }
        let tier = pkg.scope.package_resource_scope();
        let Some(dir) = installed_dir(
            &pkg.source,
            pkg.scope,
            &pkg.id,
            &cfg.global_dir,
            cfg.project_root.as_deref(),
        ) else {
            continue;
        };
        let manifest = match resolve_manifest(&dir) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(ResourceWarning::new(ResourceKind::Package, dir, e.to_string()));
                continue;
            }
        };
        let origin = ResourceOrigin::Package { id: pkg.id.clone(), scope: tier };
        if cfg.enable_skills {
            for sdir in &manifest.skills {
                let mut buf = Vec::new();
                // A manifest entry may glob-resolve to a single `SKILL.md`/`.md` file as well as a
                // directory root (package-manager.ts collectFilesFromManifestEntries).
                if sdir.is_file() {
                    let root = sdir.parent().unwrap_or(sdir);
                    load_one_skill(sdir, tier, root, &mut buf, &mut warnings, &mut diagnostics);
                } else {
                    scan_skill_root(sdir, tier, sdir, &mut buf, &mut warnings, &mut diagnostics);
                }
                buf.retain(|s| {
                    !pkg.disabled.is_disabled(&ResourceSelector::Skill(s.name.clone()))
                });
                for s in &mut buf {
                    s.origin = origin.clone();
                }
                skills.extend(buf);
            }
        }
        if cfg.enable_prompts {
            for pdir in &manifest.prompts {
                let mut buf = Vec::new();
                if pdir.is_file() {
                    let po = ResourceOrigin::LooseFile {
                        scope: tier,
                        root: pdir.parent().unwrap_or(pdir).to_path_buf(),
                    };
                    match PromptTemplate::load(pdir, tier, po) {
                        Ok(t) => buf.push(t),
                        Err(e) => warnings.push(ResourceWarning::new(
                            ResourceKind::Prompt,
                            pdir,
                            e.to_string(),
                        )),
                    }
                } else {
                    scan_prompt_root(pdir, tier, pdir, &mut buf, &mut warnings);
                }
                buf.retain(|p| {
                    !pkg.disabled.is_disabled(&ResourceSelector::Prompt(p.key.as_str().to_string()))
                });
                for p in &mut buf {
                    p.origin = origin.clone();
                }
                prompts.extend(buf);
            }
        }
        if cfg.enable_themes {
            for tdir in &manifest.themes {
                let mut buf = Vec::new();
                if tdir.is_file() {
                    let to = ResourceOrigin::LooseFile {
                        scope: tier,
                        root: tdir.parent().unwrap_or(tdir).to_path_buf(),
                    };
                    match Theme::load(tdir, tier, to) {
                        Ok(t) => buf.push(t),
                        Err(e) => warnings.push(ResourceWarning::new(
                            ResourceKind::Theme,
                            tdir,
                            e.to_string(),
                        )),
                    }
                } else {
                    scan_theme_root(tdir, tier, tdir, &mut buf, &mut warnings);
                }
                buf.retain(|t| {
                    !pkg.disabled.is_disabled(&ResourceSelector::Theme(t.data.name.clone()))
                });
                for t in &mut buf {
                    t.origin = origin.clone();
                }
                themes.extend(buf);
            }
        }
        for ext in &manifest.extensions {
            let name = ext.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if !pkg.disabled.is_disabled(&ResourceSelector::Extension(name)) {
                ext_paths.push(ext.clone());
            }
        }
    }

    // --- project loose resources (trust-gated, walked cwd->root) (R-09-002/003/008/012) ---
    if let Some(project_root) = &cfg.project_root
        && cfg.trusted_project {
            for base in walk_up(&cfg.cwd, project_root) {
                if cfg.enable_skills {
                    for sub in [".cyrup/skills", ".agents/skills"] {
                        let root = base.join(sub);
                        scan_skill_root(
                            &root,
                            ResourceScope::Project,
                            &root,
                            &mut skills,
                            &mut warnings,
                            &mut diagnostics,
                        );
                    }
                }
                if cfg.enable_prompts {
                    let root = base.join(".cyrup/prompts");
                    scan_prompt_root(
                        &root,
                        ResourceScope::Project,
                        &root,
                        &mut prompts,
                        &mut warnings,
                    );
                }
                if cfg.enable_themes {
                    let root = base.join(".cyrup/themes");
                    scan_theme_root(
                        &root,
                        ResourceScope::Project,
                        &root,
                        &mut themes,
                        &mut warnings,
                    );
                }
            }
        }

    // --- resources_discover contributions (R-09-022) ---
    if cfg.enable_skills {
        for p in &cfg.extra.skill_paths {
            add_skill_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                &mut skills,
                &mut warnings,
                &mut diagnostics,
            );
        }
    }
    if cfg.enable_prompts {
        for p in &cfg.extra.prompt_paths {
            add_prompt_path(p, ResourceScope::Discovered, ResourceOrigin::Builtin, &mut prompts, &mut warnings);
        }
    }
    if cfg.enable_themes {
        for p in &cfg.extra.theme_paths {
            add_theme_path(p, ResourceScope::Discovered, ResourceOrigin::Builtin, &mut themes, &mut warnings);
        }
    }

    // --- explicit CLI flags (highest precedence) (R-09-002/008/012) ---
    if cfg.enable_skills {
        for p in &cfg.cli.skills {
            add_skill_path(
                p,
                ResourceScope::Cli,
                ResourceOrigin::Cli { path: p.clone() },
                &mut skills,
                &mut warnings,
                &mut diagnostics,
            );
        }
    }
    if cfg.enable_prompts {
        for p in &cfg.cli.prompts {
            add_prompt_path(
                p,
                ResourceScope::Cli,
                ResourceOrigin::Cli { path: p.clone() },
                &mut prompts,
                &mut warnings,
            );
        }
    }
    if cfg.enable_themes {
        for p in &cfg.cli.themes {
            add_theme_path(
                p,
                ResourceScope::Cli,
                ResourceOrigin::Cli { path: p.clone() },
                &mut themes,
                &mut warnings,
            );
        }
    }

    // --- top-level enable/disable filter (R-09-018) ---
    let dis = &cfg.disabled;
    skills.retain(|s| !name_disabled(&dis.skills, &s.key));
    prompts.retain(|p| !name_disabled(&dis.prompts, &p.key));
    themes.retain(|t| !name_disabled(&dis.themes, &t.key));

    let registry = ResourceRegistry {
        skills: ResourceSet::build(skills),
        prompts: ResourceSet::build(prompts),
        themes: ResourceSet::build(themes),
        ext_crate_paths: ext_paths,
    };

    // Same-name collision diagnostics (skills.ts:410-427; resource-loader.ts:913-964): each
    // shadowed candidate yields a `collision` diagnostic carrying winner/loser paths.
    emit_collisions(&registry.skills, ResourceKind::Skill, |s| s.skill_md.clone(), &mut diagnostics);
    emit_collisions(&registry.prompts, ResourceKind::Prompt, |p| p.path.clone(), &mut diagnostics);
    emit_collisions(
        &registry.themes,
        ResourceKind::Theme,
        |t| t.origin_path.clone().unwrap_or_default(),
        &mut diagnostics,
    );

    Ok(DiscoveryReport { registry, warnings, diagnostics })
}

/// Emit a `collision` diagnostic for every shadowed same-name candidate in `set`.
fn emit_collisions<T: Named + Clone>(
    set: &ResourceSet<T>,
    kind: ResourceKind,
    path_of: impl Fn(&T) -> PathBuf,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let mut seen: std::collections::HashSet<(ResourceKey, PathBuf)> = std::collections::HashSet::new();
    // Resolve symlinks before comparing so a duplicate reached via a symlink collapses onto the
    // real file rather than surfacing as a spurious collision (skills.ts:403-408 `canonicalizePath`
    // + `realPathSet`). Falls back to the raw path when the file cannot be canonicalized.
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    for candidate in set.all() {
        let Some(winner) = set.get(candidate.key()) else { continue };
        let winner_path = path_of(winner);
        let loser_path = path_of(candidate);
        let winner_canon = canon(&winner_path);
        let loser_canon = canon(&loser_path);
        if loser_canon == winner_canon {
            continue; // the winner itself, or the same file reached via a symlink
        }
        if !seen.insert((candidate.key().clone(), loser_canon)) {
            continue; // dedup symlinked duplicates
        }
        diagnostics.push(ResourceDiagnostic::collision(
            kind,
            candidate.key().as_str(),
            winner_path,
            loser_path,
        ));
    }
}

fn name_disabled(set: &std::collections::BTreeSet<String>, key: &ResourceKey) -> bool {
    set.iter().any(|n| &ResourceKey::normalize(n) == key)
}

/// Skill discovery rules (skills.ts:150-272), ported 1:1:
/// - if a directory contains `SKILL.md`, treat it as a skill root and do not recurse further;
/// - otherwise load direct `.md` children of the root as skills;
/// - recurse into subdirectories to find `SKILL.md` (loose `.md` children only count at the root).
///
/// `node_modules` and dot-directories are skipped (skills.ts:223,229-231).
fn scan_skill_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    scan_skill_dir(root, scope, origin_root, true, Vec::new(), out, warnings, diagnostics);
}

#[allow(clippy::too_many_arguments)]
fn scan_skill_dir(
    dir: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    include_root_files: bool,
    mut ignore_patterns: Vec<String>,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    // Accumulate `.gitignore`/`.ignore`/`.fdignore` rules from this directory, prefixed relative to
    // the scan root, then build a matcher for the rules seen so far (skills.ts:16,47-65,188-189).
    collect_ignore_patterns(dir, origin_root, &mut ignore_patterns);
    let matcher = build_ignore(origin_root, &ignore_patterns);
    let is_ignored = |path: &Path, is_dir: bool| -> bool {
        let Ok(rel) = path.strip_prefix(origin_root) else { return false };
        let rel = to_posix(rel);
        if rel.is_empty() {
            return false;
        }
        matcher.matched(&rel, is_dir).is_ignore()
    };

    // First pass: a `SKILL.md` makes this a single skill root — load it and stop (skills.ts:194-220).
    let skill_md = dir.join("SKILL.md");
    if skill_md.is_file() && !is_ignored(&skill_md, false) {
        load_one_skill(&skill_md, scope, origin_root, out, warnings, diagnostics);
        return;
    }

    // Second pass: direct `.md` children + recurse subdirs (skills.ts:221-269).
    for path in children {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        let is_dir = path.is_dir();
        if is_ignored(&path, is_dir) {
            continue;
        }
        if is_dir {
            scan_skill_dir(
                &path,
                scope,
                origin_root,
                false,
                ignore_patterns.clone(),
                out,
                warnings,
                diagnostics,
            );
        } else if include_root_files
            && path.is_file()
            && path.extension().is_some_and(|e| e == "md")
        {
            load_one_skill(&path, scope, origin_root, out, warnings, diagnostics);
        }
    }
}

/// Convert a path to a forward-slash string (skills.ts `toPosixPath`).
fn to_posix(p: &Path) -> String {
    p.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect::<Vec<_>>()
        .join("/")
}

/// Read this directory's ignore files and append their patterns, prefixed by the directory's path
/// relative to the scan root, so a nested ignore file scopes to its own subtree (skills.ts:47-65).
fn collect_ignore_patterns(dir: &Path, root: &Path, patterns: &mut Vec<String>) {
    let prefix = match dir.strip_prefix(root) {
        Ok(rel) if rel.as_os_str().is_empty() => String::new(),
        Ok(rel) => format!("{}/", to_posix(rel)),
        Err(_) => String::new(),
    };
    for filename in [".gitignore", ".ignore", ".fdignore"] {
        let Ok(content) = std::fs::read_to_string(dir.join(filename)) else { continue };
        for line in content.lines() {
            if let Some(pattern) = prefix_ignore_pattern(line, &prefix) {
                patterns.push(pattern);
            }
        }
    }
}

/// Prefix a single ignore pattern with the subdirectory prefix, preserving `!` negation and
/// stripping a leading `/` (1:1 with skills.ts `prefixIgnorePattern`, lines 24-45).
fn prefix_ignore_pattern(line: &str, prefix: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') && !trimmed.starts_with("\\#") {
        return None;
    }
    let mut pattern = line;
    let mut negated = false;
    if let Some(rest) = pattern.strip_prefix('!') {
        negated = true;
        pattern = rest;
    } else if let Some(rest) = pattern.strip_prefix("\\!") {
        pattern = rest;
    }
    if let Some(rest) = pattern.strip_prefix('/') {
        pattern = rest;
    }
    let prefixed = if prefix.is_empty() {
        pattern.to_string()
    } else {
        format!("{prefix}{pattern}")
    };
    Some(if negated { format!("!{prefixed}") } else { prefixed })
}

/// Build a gitignore matcher rooted at the scan root from the accumulated prefixed patterns.
fn build_ignore(root: &Path, patterns: &[String]) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for pattern in patterns {
        let _ = builder.add_line(None, pattern);
    }
    builder.build().unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

fn load_one_skill(
    md: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let origin = ResourceOrigin::LooseFile { scope, root: origin_root.to_path_buf() };
    match Skill::load_with_diagnostics(md, scope, origin) {
        Ok((skill, diags)) => {
            diagnostics.extend(diags);
            if let Some(s) = skill {
                out.push(s);
            }
        }
        Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Skill, md, e.to_string())),
    }
}

/// Prompt dirs are scanned **non-recursively** — only direct `.md` children (prompt-templates.ts:137-174).
fn scan_prompt_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    for md in direct_children(root, "md") {
        let origin = ResourceOrigin::LooseFile { scope, root: origin_root.to_path_buf() };
        match PromptTemplate::load(&md, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Prompt, md, e.to_string())),
        }
    }
}

/// Theme dirs are scanned **non-recursively** — only direct `.json` children (resource-loader.ts:853-881).
fn scan_theme_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Theme>,
    warnings: &mut Vec<ResourceWarning>,
) {
    for json in direct_children(root, "json") {
        let origin = ResourceOrigin::LooseFile { scope, root: origin_root.to_path_buf() };
        match Theme::load(&json, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Theme, json, e.to_string())),
        }
    }
}

/// Direct file children of `dir` with the given extension, sorted. Non-recursive.
fn direct_children(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == ext))
        .collect();
    out.sort();
    out
}

/// Add a skill from an explicit path: a `SKILL.md` file, a skill dir, or a skills root.
fn add_skill_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    // An explicitly configured path that does not exist is a diagnostic, not a silent empty scan
    // (skills.ts:457-459: `{ type: "warning", message: "skill path does not exist" }`).
    if !path.exists() {
        diagnostics.push(ResourceDiagnostic::warning(
            ResourceKind::Skill,
            path,
            "skill path does not exist",
        ));
        return;
    }
    let md = if path.is_file() {
        path.to_path_buf()
    } else if path.join("SKILL.md").is_file() {
        path.join("SKILL.md")
    } else {
        // Treat as a root containing multiple skills.
        scan_skill_root(path, scope, path, out, warnings, diagnostics);
        return;
    };
    match Skill::load_with_diagnostics(&md, scope, origin) {
        Ok((skill, diags)) => {
            diagnostics.extend(diags);
            if let Some(s) = skill {
                out.push(s);
            }
        }
        Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Skill, md, e.to_string())),
    }
}

fn add_prompt_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    if !path.exists() {
        warnings.push(ResourceWarning::new(
            ResourceKind::Prompt,
            path,
            "prompt path does not exist",
        ));
        return;
    }
    if path.is_dir() {
        scan_prompt_root(path, scope, path, out, warnings);
        return;
    }
    match PromptTemplate::load(path, scope, origin) {
        Ok(t) => out.push(t),
        Err(e) => {
            warnings.push(ResourceWarning::new(ResourceKind::Prompt, path, e.to_string()));
        }
    }
}

fn add_theme_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<Theme>,
    warnings: &mut Vec<ResourceWarning>,
) {
    if !path.exists() {
        warnings.push(ResourceWarning::new(
            ResourceKind::Theme,
            path,
            "theme path does not exist",
        ));
        return;
    }
    if path.is_dir() {
        scan_theme_root(path, scope, path, out, warnings);
        return;
    }
    match Theme::load(path, scope, origin) {
        Ok(t) => out.push(t),
        Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Theme, path, e.to_string())),
    }
}

/// Dirs from `cwd` up to and including `project_root` (Agent Skills standard ascending walk).
fn walk_up(cwd: &Path, project_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut cur = Some(cwd.to_path_buf());
    while let Some(c) = cur {
        dirs.push(c.clone());
        if c == project_root {
            break;
        }
        cur = c.parent().map(Path::to_path_buf);
    }
    if !dirs.iter().any(|d| d == project_root) {
        dirs.push(project_root.to_path_buf());
    }
    dirs
}
