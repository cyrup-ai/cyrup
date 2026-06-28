//! Unified resource discovery + loader (arch-09 §3.2, §6.1, R-09-022..024).
//!
//! `discover()` fans across roots (built-in, global, global-agents, project walk, installed
//! packages, CLI flags, `resources_discover` contributions), parses each kind, and merges into a
//! [`ResourceRegistry`] with deterministic same-name precedence (R-09-024).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cyrup_core::CancelToken;

use crate::error::{ResourceError, ResourceKind, ResourceWarning};
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
    let mut ext_paths: Vec<PathBuf> = Vec::new();

    // --- built-in themes (R-09-011) ---
    if cfg.enable_themes {
        themes.extend(builtin_themes());
    }

    // --- global loose resources (R-09-002/008/012) ---
    if cfg.enable_skills {
        for root in [cfg.global_dir.join("skills"), cfg.global_agents_dir.join("skills")] {
            scan_skill_root(&root, ResourceScope::Global, &root, &mut skills, &mut warnings);
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
                scan_skill_root(sdir, tier, sdir, &mut buf, &mut warnings);
                buf.retain(|s| {
                    !pkg.disabled.is_disabled(&ResourceSelector::Skill(s.front.name.clone()))
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
                scan_prompt_root(pdir, tier, pdir, &mut buf, &mut warnings);
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
                scan_theme_root(tdir, tier, tdir, &mut buf, &mut warnings);
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
    if let Some(project_root) = &cfg.project_root {
        if cfg.trusted_project {
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
    }

    // --- resources_discover contributions (R-09-022) ---
    if cfg.enable_skills {
        for p in &cfg.extra.skill_paths {
            add_skill_path(p, ResourceScope::Discovered, ResourceOrigin::Builtin, &mut skills, &mut warnings);
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
    Ok(DiscoveryReport { registry, warnings })
}

fn name_disabled(set: &std::collections::BTreeSet<String>, key: &ResourceKey) -> bool {
    set.iter().any(|n| &ResourceKey::normalize(n) == key)
}

/// Directory walk that ignores `.gitignore`/hidden filters (resource dirs are explicit).
fn walk_files(root: &Path, max_depth: usize, want: impl Fn(&Path) -> bool) -> Vec<PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let walker = ignore::WalkBuilder::new(root)
        .hidden(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .ignore(false)
        .parents(false)
        .max_depth(Some(max_depth))
        .build();
    for entry in walker.flatten() {
        let p = entry.path();
        if p.is_file() && want(p) {
            out.push(p.to_path_buf());
        }
    }
    out.sort();
    out
}

fn scan_skill_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
) {
    for md in walk_files(root, 6, |p| p.file_name().is_some_and(|n| n == "SKILL.md")) {
        let origin = ResourceOrigin::LooseFile { scope, root: origin_root.to_path_buf() };
        match Skill::load(&md, scope, origin) {
            Ok(s) => out.push(s),
            Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Skill, md, e.to_string())),
        }
    }
}

fn scan_prompt_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<PromptTemplate>,
    warnings: &mut Vec<ResourceWarning>,
) {
    for md in walk_files(root, 4, |p| p.extension().is_some_and(|e| e == "md")) {
        let origin = ResourceOrigin::LooseFile { scope, root: origin_root.to_path_buf() };
        match PromptTemplate::load(&md, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Prompt, md, e.to_string())),
        }
    }
}

fn scan_theme_root(
    root: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Theme>,
    warnings: &mut Vec<ResourceWarning>,
) {
    for json in walk_files(root, 4, |p| p.extension().is_some_and(|e| e == "json")) {
        let origin = ResourceOrigin::LooseFile { scope, root: origin_root.to_path_buf() };
        match Theme::load(&json, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(ResourceKind::Theme, json, e.to_string())),
        }
    }
}

/// Add a skill from an explicit path: a `SKILL.md` file, a skill dir, or a skills root.
fn add_skill_path(
    path: &Path,
    scope: ResourceScope,
    origin: ResourceOrigin,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
) {
    let md = if path.is_file() {
        path.to_path_buf()
    } else if path.join("SKILL.md").is_file() {
        path.join("SKILL.md")
    } else {
        // Treat as a root containing multiple skills.
        scan_skill_root(path, scope, path, out, warnings);
        return;
    };
    match Skill::load(&md, scope, origin) {
        Ok(s) => out.push(s),
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
