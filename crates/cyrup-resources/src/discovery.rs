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
use crate::package::manifest::{ManifestResourceType, resolve_local_entries};
use crate::package::store::installed_dir;
use crate::package::{
    ConfiguredPackage, DisabledSet, InstalledPackage, InstalledPackages, PackageFilter,
    ResourceSelector, resolve_manifest,
};
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
        Self {
            by_key: HashMap::new(),
            all: Vec::new(),
        }
    }
}

impl<T: Named + Clone> ResourceSet<T> {
    /// Build from all candidates, applying Pi's precedence: candidates are ordered by
    /// [`ResourceScope::precedence_rank`] ascending (**lower rank wins**), ties broken by insertion
    /// order, then the **first** occurrence of each name wins — a 1:1 port of Pi's
    /// `resolved.sort((a,b) => rank(a) - rank(b))` followed by first-wins dedup
    /// (package-manager.ts:2474-2482, `resourcePrecedenceRank` 184-188). A stable sort preserves
    /// insertion order within a rank, so e.g. a project-scope package (rank 4) inserted before a
    /// global-scope package (also rank 4) wins the tie. The explicit `--skill` CLI tier sits at
    /// rank 5 (below every package), reproducing Pi's append-after-sort order in which a same-name
    /// package wins over an appended `additionalSkillPaths` entry (resource-loader.ts:421).
    pub fn build(all: Vec<T>) -> Self {
        // `sort_by_key` is a *stable* sort: equal ranks keep their original (insertion) order, so
        // first-wins over this ordering reproduces Pi's project-package-first rank-4 tie.
        let mut ordered: Vec<&T> = all.iter().collect();
        ordered.sort_by_key(|c| c.scope().precedence_rank());
        let mut by_key: HashMap<ResourceKey, T> = HashMap::new();
        for c in ordered {
            by_key.entry(c.key().clone()).or_insert_with(|| c.clone());
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

/// Settings-level override-pattern lists that selectively enable/disable *auto-discovered loose*
/// resources (1:1 with Pi's `globalSettings`/`projectSettings` `skills`/`prompts`/`themes` arrays,
/// applied via `isEnabledByOverrides`/`addAutoDiscoveredResources`, package-manager.ts:700-717,
/// 2241-2304). Each list holds `!`/`+`/`-` override patterns (plain/glob includes are ignored here);
/// an empty list disables nothing. Populated by cyrup-config; defaults to empty (no filtering).
#[derive(Default, Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResourceOverrides {
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub prompts: Vec<String>,
    #[serde(default)]
    pub themes: Vec<String>,
    /// The settings `extensions` array. Pi's `RESOURCE_TYPES` is
    /// `["extensions","skills","prompts","themes"]` (package-manager.ts:194) and `resolve()` runs the
    /// SAME `resolveLocalEntries` pass over all four (package-manager.ts:905-931), so a plain path
    /// here is a positive listing that LOADS an extension root — not merely a filter (CFG-004).
    #[serde(default)]
    pub extensions: Vec<String>,
}

/// Everything discovery needs, passed in (keeps `cyrup-resources` core-only).
#[derive(Clone, Debug)]
pub struct DiscoveryConfig {
    pub cwd: PathBuf,
    pub global_dir: PathBuf,
    pub global_agents_dir: PathBuf,
    /// The `PackageStore` global root — the value the `install` subcommand passes as the store's
    /// `global_dir` when it records an install (`PackageStore::new(dirs.package_dir, …)`, cyrup
    /// subcommands.rs:396; Pi `dirs.package_dir`, env.rs:156-160). Used to resolve installed
    /// **Global**-scope package working trees, i.e. `<package_global_dir>/packages/<id>`. Kept
    /// DISTINCT from `global_dir` (which roots the loose global resources at `<global_dir>/skills`,
    /// `/prompts`, `/themes`) because the bin passes its `package_dir` — not `agent_dir` — as the
    /// store root, so a Global package's tree lives one level deeper than a naive
    /// `<global_dir>/packages/<id>` guess. Defaults to `<global_dir>/packages` (the bin's own default
    /// for `package_dir`), so callers that don't set a custom `--package-dir`/`CYRUP_PACKAGE_DIR`
    /// resolve installed Global packages correctly with no extra wiring.
    pub package_global_dir: PathBuf,
    /// The user-tier cross-tool `.agents` base dir (Pi `getHomeDir()/.agents`,
    /// package-manager.ts:2286,217). When `Some`, `<user_agents_dir>/skills` is loaded as a
    /// USER/global-scope skill source AND excluded from the project `.agents/skills` ancestor walk so
    /// it is not double-counted (Pi `userAgentsSkillsDir` + the `.filter(... !== userAgentsSkillsDir)`
    /// dedup, package-manager.ts:2286-2289,2377-2389). `None` keeps the legacy
    /// `global_agents_dir/skills` behavior (no user-tier `~/.agents/skills`).
    pub user_agents_dir: Option<PathBuf>,
    pub project_root: Option<PathBuf>,
    /// DI-11 trust decision from cyrup-config (R-09-003/008/012).
    pub trusted_project: bool,
    pub enable_skills: bool,
    pub enable_prompts: bool,
    pub enable_themes: bool,
    pub cli: CliResourcePaths,
    pub installed: InstalledPackages,
    /// Packages DECLARED in `settings.json` (`packages: [...]`), per settings layer. Pi's ONLY
    /// package channel — `PackageManager.resolve()` re-reads
    /// `projectSettings.packages`/`globalSettings.packages` on every call and resolves each entry to
    /// a working tree (package-manager.ts:891-901). Resolved BEFORE [`Self::installed`]; a duplicate
    /// working tree recorded in the install registry is skipped (CFG-003).
    pub configured_packages: Vec<ConfiguredPackage>,
    /// From `resources_discover` (R-09-022).
    pub extra: DiscoveredPaths,
    /// Top-level per-resource enable/disable state.
    pub disabled: DisabledSet,
    /// Settings override patterns for global (user-scope) auto-discovered loose resources
    /// (package-manager.ts:700-717, 2271-2278).
    pub global_overrides: ResourceOverrides,
    /// Settings override patterns for project-scope auto-discovered loose resources.
    pub project_overrides: ResourceOverrides,
}

impl DiscoveryConfig {
    /// A config with all kinds enabled, `global_agents_dir` derived as `<global>/agents`, no
    /// project, untrusted. Callers (cyrup-config) override fields as needed.
    pub fn new(cwd: impl Into<PathBuf>, global_dir: impl Into<PathBuf>) -> Self {
        let global_dir = global_dir.into();
        let global_agents_dir = global_dir.join("agents");
        // The bin's default `dirs.package_dir` is `<agent_dir>/packages` (env.rs:156-160); with
        // `global_dir == agent_dir` (the session-svc builder's `DiscoveryConfig::new` call) this
        // default matches the store root the `install` subcommand writes to, so a Global package
        // installed with no custom `--package-dir` resolves correctly out of the box.
        let package_global_dir = global_dir.join("packages");
        Self {
            cwd: cwd.into(),
            global_dir,
            global_agents_dir,
            package_global_dir,
            user_agents_dir: None,
            project_root: None,
            trusted_project: false,
            enable_skills: true,
            enable_prompts: true,
            enable_themes: true,
            cli: CliResourcePaths::default(),
            installed: InstalledPackages::default(),
            configured_packages: Vec::new(),
            extra: DiscoveredPaths::default(),
            disabled: DisabledSet::default(),
            global_overrides: ResourceOverrides::default(),
            project_overrides: ResourceOverrides::default(),
        }
    }
}

/// One package working tree queued for resource collection, from either the settings channel
/// ([`ConfiguredPackage`], CFG-003) or the install registry ([`InstalledPackage`]).
struct PackageTree {
    dir: PathBuf,
    id: cyrup_core::PackageId,
    tier: ResourceScope,
    disabled: DisabledSet,
    filter: PackageFilter,
    /// Set only on the USER/global half of a `dedupePackages` DELTA PAIR — the filter of the
    /// project-scope `autoload: false` entry that deltas over this one. See
    /// [`subtract_delta_shadow`].
    delta_shadow: Option<PackageFilter>,
}

/// Borrowed view of a [`PackageTree`] used inside the collection loop.
struct PackageTreeRef<'a> {
    id: &'a cyrup_core::PackageId,
    disabled: &'a DisabledSet,
    filter: &'a PackageFilter,
    delta_shadow: Option<&'a PackageFilter>,
}

/// Apply a settings-declared package's per-type filter to a collected buffer, in whichever of Pi's
/// two modes the entry selected (`collectPackageResources`, package-manager.ts:2079-2092).
///
/// `delta` is the entry's `autoload: false` (`filter.autoload === false`, :2084 — only an explicit
/// `false`, so [`PackageFilter::is_delta`] is the exact test):
///
/// - `delta == false` — INCLUDE filter (`applyPackageFilter`, :2147-2171). `None` keeps everything
///   (the package's own manifest already selected it); an explicitly EMPTY list disables the whole
///   resource type (:2156-2162); otherwise `applyPatterns` narrows, relative to the package root.
/// - `delta == true` — DELTA (`applyPackageDeltaFilter`, :2173-2189). The buffer starts EMPTY and
///   only what the patterns name is added back. With no patterns for this type Pi returns straight
///   away having added nothing (:2180-2182), so an entry that is just
///   `{"source": …, "autoload": false}` contributes zero resources — the point of opting out.
fn retain_by_package_filter<T>(
    buf: &mut Vec<T>,
    path_of: impl Fn(&T) -> PathBuf,
    package_root: &Path,
    patterns: Option<&[String]>,
    delta: bool,
) {
    if delta {
        let patterns = patterns.unwrap_or_default();
        if patterns.is_empty() {
            buf.clear();
            return;
        }
        let all: Vec<PathBuf> = buf.iter().map(&path_of).collect();
        let enabled = crate::package::manifest::apply_autoload_disabled_patterns(
            package_root,
            &all,
            patterns,
        );
        buf.retain(|item| enabled.contains(&path_of(item)));
        return;
    }
    let Some(patterns) = patterns else {
        return;
    };
    if patterns.is_empty() {
        buf.clear();
        return;
    }
    let all: Vec<PathBuf> = buf.iter().map(&path_of).collect();
    let enabled = crate::package::manifest::apply_settings_patterns(package_root, &all, patterns);
    buf.retain(|item| enabled.contains(&path_of(item)));
}

/// Drop from a base entry's buffer everything the project-scope `autoload: false` entry that deltas
/// over it NAMED — the second half of Pi's `dedupePackages` delta pair.
///
/// Pi keeps both entries and resolves the delta FIRST (`dedupePackages`, package-manager.ts:1691-1696,
/// then `resolvePackageSources`, :1229), writing into an accumulator whose `addResource` is
/// first-writer-wins (`if (!map.has(path))`, :2488-2490). So the delta entry claims the slot for
/// every path its patterns name — whether it enabled or disabled it — and the base entry only fills
/// in the paths left over. cyrup's buffers carry live resources instead of Pi's `enabled` flag, so
/// the same net set is reached by SUBTRACTION: a path the delta enabled was already contributed by
/// the delta tree (at project scope, which also wins the shared rank-4 tie), and a path it disabled
/// must not reappear via the base.
///
/// `None`/empty patterns for a resource type name nothing, so the base contributes that type in
/// full — matching `applyPackageDeltaFilter`'s early return (:2180-2182).
fn subtract_delta_shadow<T>(
    buf: &mut Vec<T>,
    path_of: impl Fn(&T) -> PathBuf,
    package_root: &Path,
    patterns: Option<&[String]>,
) {
    let Some(patterns) = patterns.filter(|p| !p.is_empty()) else {
        return;
    };
    let all: Vec<PathBuf> = buf.iter().map(&path_of).collect();
    let named = crate::package::manifest::autoload_delta_verdicts(package_root, &all, patterns);
    buf.retain(|item| {
        let path = path_of(item);
        !named.iter().any(|(named_path, _)| named_path == &path)
    });
}

/// Resolve a settings-declared package entry to its on-disk working tree.
///
/// Pi's `resolvePackageSources` (package-manager.ts:1224-1283) resolves a `local` source against the
/// scope base dir (`getBaseDirForScope`, 2055-2064: `<cwd>/.cyrup` for project, the agent dir for
/// user) and *installs* an npm/git source that is missing. **[CYRUP-DELTA]**: cyrup performs no
/// network install during session assembly — a non-local source resolves only through the existing
/// install registry paths, and anything unresolvable becomes a loud [`ResourceDiagnostic`] instead of
/// a silent drop or a failed session (constraint: malformed/missing declarations fail loudly + safely).
///
/// `all` is the full declared set, needed for Pi's `findAutoloadDeltaBase`
/// (package-manager.ts:1285-1299): a project-scope `autoload: false` entry RESOLVES against the
/// user-scope entry it deltas over (`resolvedSource`/`resolvedScope`, :1232-1234) so the pair lands
/// on one working tree even where the two scopes resolve the same source string differently — a
/// relative local path, or an npm install root. Its `tier` still comes from its own scope, exactly
/// like Pi's `metadata` (`{ source: sourceStr, scope, … }`, :1235).
fn resolve_configured_package(
    declared: &ConfiguredPackage,
    all: &[ConfiguredPackage],
    cfg: &DiscoveryConfig,
) -> Result<PackageTree, Box<ResourceDiagnostic>> {
    let tier = declared.scope.package_resource_scope();
    let delta_base = if declared.scope == InstallScope::Project && declared.filter.is_delta() {
        all.iter()
            .find(|e| e.scope == InstallScope::Global && e.source.trim() == declared.source.trim())
    } else {
        None
    };
    let resolve_scope = delta_base.map_or(declared.scope, |e| e.scope);
    let resolve_source = delta_base.map_or(declared.source.as_str(), |e| e.source.as_str());
    let base = match resolve_scope {
        InstallScope::Project => cfg.cwd.join(".cyrup"),
        InstallScope::Global => cfg.global_dir.clone(),
    };
    let source = crate::package::PackageSource::parse(resolve_source.trim()).map_err(|e| {
        Box::new(ResourceDiagnostic::error(
            ResourceKind::Package,
            &base,
            format!(
                "settings `packages` entry {:?} is not a usable package source: {e}",
                declared.source
            ),
        ))
    })?;
    let id = source.package_id();
    let dir = match &source {
        // A local path resolves against the scope base dir, exactly like Pi's
        // `resolveLocalExtensionSource` (package-manager.ts:1301-1327).
        crate::package::PackageSource::Path { path } => {
            // `resolvePathFromBase` normalizes before testing absoluteness (package-manager.ts:
            // 2069-2071 → paths.ts:57-85 @v0.83.0), so `"packages": ["~/pack"]` resolves under the
            // home dir instead of producing `<base>/~/pack` and the misleading
            // "not installed — run `cyrup install`" diagnostic below (CFG-025).
            let normalized =
                PathBuf::from(cyrup_config::paths::normalize_path(&path.to_string_lossy()));
            if normalized.is_absolute() {
                normalized
            } else {
                base.join(normalized)
            }
        }
        // git/oci: use the tree a previous `cyrup install` materialized, if any.
        _ => installed_dir(
            &source,
            resolve_scope,
            &id,
            &cfg.package_global_dir,
            cfg.project_root.as_deref(),
        )
        .ok_or_else(|| {
            Box::new(ResourceDiagnostic::error(
                ResourceKind::Package,
                &base,
                format!(
                    "package {:?} is declared in settings but its install location could not be \
                     resolved",
                    declared.source
                ),
            ))
        })?,
    };
    if !dir.is_dir() {
        return Err(Box::new(ResourceDiagnostic::error(
            ResourceKind::Package,
            &dir,
            format!(
                "package {:?} is declared in settings but is not installed at this path — run \
                 `cyrup install {}`",
                declared.source, declared.source
            ),
        )));
    }
    Ok(PackageTree {
        dir,
        id,
        tier,
        disabled: DisabledSet::default(),
        filter: declared.filter.clone(),
        delta_shadow: None,
    })
}

/// Whether an auto-discovered loose resource file is enabled by a settings override list. The match
/// base is the parent of the conventional `skills`/`prompts`/`themes` directory (`root.parent()`), so
/// a settings pattern like `!skills/internal` filters by the conventional-relative path, matching
/// Pi's `projectBaseDir`/`globalBaseDir`-relative matching (package-manager.ts:2271-2304).
fn override_enabled(path: &Path, patterns: &[String], root: &Path) -> bool {
    if patterns.is_empty() {
        return true;
    }
    let base = root.parent().unwrap_or(root);
    crate::package::manifest::is_enabled_by_overrides(base, path, patterns)
}

/// Port of Pi's `findGitRepoRoot` (package-manager.ts:426-438): walk up from `start_dir`, returning
/// the first ancestor (inclusive) that contains a `.git` entry, or `None` at the filesystem root.
/// Existence is tested with [`Path::exists`] exactly like Pi's `existsSync(join(dir, ".git"))`, so a
/// `.git` directory and a git-worktree `.git` file both qualify.
fn find_git_repo_root(start_dir: &Path) -> Option<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Some(dir);
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => return None,
        }
    }
}

/// Port of Pi's `collectAncestorAgentsSkillDirs` (package-manager.ts:440-459): from `start_dir`,
/// collect `<dir>/.agents/skills` at every ancestor, stopping at (and including) the git repo root
/// if one exists, otherwise at the filesystem root. `start_dir` is taken as already-absolute (Pi
/// `resolve`s it; `cfg.cwd` is the absolute analog), so no extra lexical normalization is applied.
fn collect_ancestor_agents_skill_dirs(start_dir: &Path) -> Vec<PathBuf> {
    let git_repo_root = find_git_repo_root(start_dir);
    let mut skill_dirs = Vec::new();
    let mut dir = start_dir.to_path_buf();
    loop {
        skill_dirs.push(dir.join(".agents").join("skills"));
        if git_repo_root.as_ref() == Some(&dir) {
            break;
        }
        match dir.parent() {
            Some(parent) if parent != dir => dir = parent.to_path_buf(),
            _ => break,
        }
    }
    skill_dirs
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

impl ResourceRegistry {
    /// Merge extension-contributed skill/prompt/theme paths into this snapshot, returning a NEW
    /// snapshot (the registry is immutable; callers atomically swap the result in). 1:1 with Pi's
    /// `ResourceLoader.extendResources` (resource-loader.ts:293) as driven by
    /// `extendResourcesFromExtensions` (agent-session.ts:2112-2135): each contributed file is loaded
    /// at the [`ResourceScope::Discovered`] tier (Pi's `scope:"temporary"` extension resources, rank
    /// `6`) and folded back through [`ResourceSet::build`], so a same-name user/package/CLI resource
    /// still wins (first-wins precedence). Existing candidates are preserved; `ext_crate_paths` are
    /// carried over unchanged. Parse failures / missing paths are dropped (Pi logs a warning and
    /// skips), never panicking.
    pub fn extend(&self, extra: &DiscoveredPaths) -> ResourceRegistry {
        let mut skills: Vec<Skill> = self.skills.all().to_vec();
        let mut prompts: Vec<PromptTemplate> = self.prompts.all().to_vec();
        let mut themes: Vec<Theme> = self.themes.all().to_vec();
        // Warnings/diagnostics are surfaced by the primary discovery pass; the extend-merge mirrors
        // Pi's loader, which records them internally rather than returning them to the caller.
        let mut warnings: Vec<ResourceWarning> = Vec::new();
        let mut diagnostics: Vec<ResourceDiagnostic> = Vec::new();
        for p in &extra.skill_paths {
            add_skill_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                &mut skills,
                &mut warnings,
                &mut diagnostics,
            );
        }
        for p in &extra.prompt_paths {
            add_prompt_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                &mut prompts,
                &mut warnings,
            );
        }
        for p in &extra.theme_paths {
            add_theme_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                &mut themes,
                &mut warnings,
            );
        }
        let _ = (warnings, diagnostics);
        ResourceRegistry {
            skills: ResourceSet::build(skills),
            prompts: ResourceSet::build(prompts),
            themes: ResourceSet::build(themes),
            ext_crate_paths: self.ext_crate_paths.clone(),
        }
    }
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
        Some(joined) => joined.map_err(|e| {
            ResourceError::Core(cyrup_core::CoreError::Io(std::io::Error::other(
                e.to_string(),
            )))
        })?,
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
    // Settings override patterns (Pi `globalSettings.{skills,prompts,themes}`) selectively
    // enable/disable these auto-discovered loose resources (package-manager.ts:2271-2304).
    if cfg.enable_skills {
        // The user-tier cross-tool `~/.agents/skills` (Pi `userAgentsSkillsDir`,
        // package-manager.ts:2286,2377-2389) is loaded as a USER/global-scope source when the
        // session-svc builder plumbs `user_agents_dir = $HOME/.agents`.
        let mut roots = vec![
            cfg.global_dir.join("skills"),
            cfg.global_agents_dir.join("skills"),
        ];
        if let Some(user_agents) = &cfg.user_agents_dir {
            roots.push(user_agents.join("skills"));
        }
        for root in roots {
            let mut buf = Vec::new();
            scan_skill_root(
                &root,
                ResourceScope::Global,
                &root,
                &mut buf,
                &mut warnings,
                &mut diagnostics,
            );
            buf.retain(|s| override_enabled(&s.skill_md, &cfg.global_overrides.skills, &root));
            skills.extend(buf);
        }
    }
    if cfg.enable_prompts {
        let root = cfg.global_dir.join("prompts");
        let mut buf = Vec::new();
        scan_prompt_root(&root, ResourceScope::Global, &root, &mut buf, &mut warnings);
        buf.retain(|p| override_enabled(&p.path, &cfg.global_overrides.prompts, &root));
        prompts.extend(buf);
    }
    if cfg.enable_themes {
        let root = cfg.global_dir.join("themes");
        let mut buf = Vec::new();
        scan_theme_root(&root, ResourceScope::Global, &root, &mut buf, &mut warnings);
        buf.retain(|t| {
            t.origin_path
                .as_deref()
                .is_none_or(|p| override_enabled(p, &cfg.global_overrides.themes, &root))
        });
        themes.extend(buf);
    }

    // --- settings positive listings (Pi `resolveLocalEntries`, package-manager.ts:906-928) ---
    // Plain paths listed in the project/global settings `skills`/`prompts`/`themes` arrays are
    // loaded as `source:"local"` entries, which Pi ranks *above* auto-discovered files of the same
    // scope (`resourcePrecedenceRank` 184-188). Project entries are trust-gated and resolve relative
    // to `<cwd>/.cyrup` (Pi `projectBaseDir = join(cwd, CONFIG_DIR)`, 900); global entries resolve
    // relative to the global config dir (Pi `globalBaseDir = agentDir`, 899).
    if cfg.trusted_project {
        let project_base = cfg.cwd.join(".cyrup");
        add_local_entries(
            &project_base,
            &cfg.project_overrides,
            ResourceScope::ProjectSettings,
            cfg,
            &mut skills,
            &mut prompts,
            &mut themes,
            &mut ext_paths,
            &mut warnings,
            &mut diagnostics,
        );
    }
    add_local_entries(
        &cfg.global_dir,
        &cfg.global_overrides,
        ResourceScope::GlobalSettings,
        cfg,
        &mut skills,
        &mut prompts,
        &mut themes,
        &mut ext_paths,
        &mut warnings,
        &mut diagnostics,
    );

    // --- packages: settings-declared (CFG-003) + the install registry (R-09-015/016/017/018) ---
    // All packages share precedence rank 4 (Pi `resourcePrecedenceRank`, package-manager.ts:185).
    // Pi pushes project-scope packages before global ones (allPackages, 887-893), so under that
    // shared rank a project-local package wins a same-name tie with a global one. Stable-order both
    // channels project-first to reproduce that (config order is preserved within a scope).
    //
    // Pi's ONLY channel is the settings one — `resolve()` re-reads
    // `projectSettings.packages`/`globalSettings.packages` on every call (891-901) — so the
    // settings-declared trees are resolved FIRST and an install-registry record for the same working
    // tree is skipped as a duplicate.
    let mut trees: Vec<PackageTree> = Vec::new();
    let mut ordered_cfg: Vec<&ConfiguredPackage> = cfg.configured_packages.iter().collect();
    ordered_cfg.sort_by_key(|p| match p.scope {
        InstallScope::Project => 0u8,
        InstallScope::Global => 1u8,
    });
    // Project-scope `autoload: false` entries that ACTUALLY resolved, in declaration order. The
    // global entry each one deltas over carries it as its `delta_shadow` (Pi's `dedupePackages`
    // keeps both halves of the pair, package-manager.ts:1691-1696). Recorded from the admitted
    // trees, not from the raw settings, so an untrusted or unresolvable project entry cannot reach
    // in and suppress resources from the global package it names.
    let mut project_deltas: Vec<(String, PackageFilter)> = Vec::new();
    for declared in ordered_cfg {
        if declared.scope == InstallScope::Project && !cfg.trusted_project {
            continue; // fail-closed trust gate (Pi `assertProjectTrustedForScope`, 2055-2058)
        }
        match resolve_configured_package(declared, &cfg.configured_packages, cfg) {
            Ok(mut tree) => {
                match declared.scope {
                    InstallScope::Project if declared.filter.is_delta() => project_deltas
                        .push((declared.source.trim().to_string(), declared.filter.clone())),
                    InstallScope::Global => {
                        tree.delta_shadow = project_deltas
                            .iter()
                            .find(|(source, _)| source == declared.source.trim())
                            .map(|(_, filter)| filter.clone());
                    }
                    InstallScope::Project => {}
                }
                trees.push(tree);
            }
            Err(diag) => diagnostics.push(*diag),
        }
    }
    let mut ordered_pkgs: Vec<&InstalledPackage> = cfg.installed.packages.iter().collect();
    ordered_pkgs.sort_by_key(|p| match p.scope {
        InstallScope::Project => 0u8,
        InstallScope::Global => 1u8,
    });
    for pkg in ordered_pkgs {
        if pkg.scope == InstallScope::Project && !cfg.trusted_project {
            continue; // fail-closed trust gate
        }
        let Some(dir) = installed_dir(
            &pkg.source,
            pkg.scope,
            &pkg.id,
            // The package-store global root the bin wrote to (`dirs.package_dir`), NOT `global_dir`
            // (the loose-resource agent root) — see the `package_global_dir` field docs.
            &cfg.package_global_dir,
            cfg.project_root.as_deref(),
        ) else {
            continue;
        };
        trees.push(PackageTree {
            dir,
            id: pkg.id.clone(),
            tier: pkg.scope.package_resource_scope(),
            disabled: pkg.disabled.clone(),
            filter: PackageFilter::default(),
            delta_shadow: None,
        });
    }
    let mut seen_trees: Vec<PathBuf> = Vec::new();
    for tree in trees {
        let PackageTree {
            dir,
            id,
            tier,
            disabled,
            filter,
            delta_shadow,
        } = tree;
        // The base half of a delta pair intentionally revisits a tree the project delta already
        // walked — Pi processes both entries (`dedupePackages`, package-manager.ts:1691-1696).
        // Every other repeat is a genuine duplicate (the install registry re-declaring a settings
        // package) and is skipped.
        if seen_trees.contains(&dir) && delta_shadow.is_none() {
            continue;
        }
        seen_trees.push(dir.clone());
        let pkg = PackageTreeRef {
            id: &id,
            disabled: &disabled,
            filter: &filter,
            delta_shadow: delta_shadow.as_ref(),
        };
        let manifest = match resolve_manifest(&dir) {
            Ok(m) => m,
            Err(e) => {
                warnings.push(ResourceWarning::new(
                    ResourceKind::Package,
                    dir,
                    e.to_string(),
                ));
                continue;
            }
        };
        let origin = ResourceOrigin::Package {
            id: pkg.id.clone(),
            scope: tier,
        };
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
                    !pkg.disabled
                        .is_disabled(&ResourceSelector::Skill(s.name.clone()))
                });
                retain_by_package_filter(
                    &mut buf,
                    |s| s.skill_md.clone(),
                    &dir,
                    pkg.filter.skills.as_deref(),
                    pkg.filter.is_delta(),
                );
                subtract_delta_shadow(
                    &mut buf,
                    |s| s.skill_md.clone(),
                    &dir,
                    pkg.delta_shadow.and_then(|f| f.skills.as_deref()),
                );
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
                    !pkg.disabled
                        .is_disabled(&ResourceSelector::Prompt(p.key.as_str().to_string()))
                });
                retain_by_package_filter(
                    &mut buf,
                    |p| p.path.clone(),
                    &dir,
                    pkg.filter.prompts.as_deref(),
                    pkg.filter.is_delta(),
                );
                subtract_delta_shadow(
                    &mut buf,
                    |p| p.path.clone(),
                    &dir,
                    pkg.delta_shadow.and_then(|f| f.prompts.as_deref()),
                );
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
                    !pkg.disabled
                        .is_disabled(&ResourceSelector::Theme(t.data.name.clone()))
                });
                retain_by_package_filter(
                    &mut buf,
                    |t| t.origin_path.clone().unwrap_or_default(),
                    &dir,
                    pkg.filter.themes.as_deref(),
                    pkg.filter.is_delta(),
                );
                subtract_delta_shadow(
                    &mut buf,
                    |t| t.origin_path.clone().unwrap_or_default(),
                    &dir,
                    pkg.delta_shadow.and_then(|f| f.themes.as_deref()),
                );
                for t in &mut buf {
                    t.origin = origin.clone();
                }
                themes.extend(buf);
            }
        }
        let mut ext_buf: Vec<PathBuf> = Vec::new();
        for ext in &manifest.extensions {
            let name = ext
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            if !pkg.disabled.is_disabled(&ResourceSelector::Extension(name)) {
                ext_buf.push(ext.clone());
            }
        }
        retain_by_package_filter(
            &mut ext_buf,
            Clone::clone,
            &dir,
            pkg.filter.extensions.as_deref(),
            pkg.filter.is_delta(),
        );
        subtract_delta_shadow(
            &mut ext_buf,
            Clone::clone,
            &dir,
            pkg.delta_shadow.and_then(|f| f.extensions.as_deref()),
        );
        for e in ext_buf {
            if !ext_paths.contains(&e) {
                ext_paths.push(e);
            }
        }

        // A package tree with no manifest AND none of the conventional resource dirs is a BARE
        // EXTENSION directory: `collectPackageResources` returns false (package-manager.ts:2126-
        // 2140 @v0.83.0 — `hasAnyDir` stays false), and `resolveLocalExtensionSource` then does
        // `this.addResource(accumulator.extensions, resolved, metadata, true)` (`:1338-1341`),
        // registering the directory itself. cyrup pushed only `manifest.extensions` entries, so
        // `"packages": ["./my-ext"]` on a manifest-less extension loaded nothing (CFG-027).
        if manifest.kind == crate::package::manifest::ManifestKind::AutoDiscovered
            && manifest.extensions.is_empty()
            && manifest.skills.is_empty()
            && manifest.prompts.is_empty()
            && manifest.themes.is_empty()
            && manifest.agents.is_empty()
            && !ext_paths.contains(&dir)
        {
            ext_paths.push(dir.clone());
        }
    }

    // --- project loose resources (trust-gated) (R-09-002/003/008/012) ---
    // Pi loads loose project resources from the cwd, gated on TRUST alone — it has no `project_root`
    // precondition. Gating on `project_root.is_some()` left this block DEAD in production (the
    // session-svc builder sets `trusted_project` but never `project_root`), so cyrup discovered ZERO
    // project skills/prompts/themes live. `.cyrup/{skills,prompts,themes}` are read at `cwd`
    // (skills.ts:432 `resolve(resolvedCwd, CONFIG_DIR_NAME, …)`; resource-loader.ts:764-768); the
    // `.agents/skills` tree is walked cwd→git-root (`collectAncestorAgentsSkillDirs`,
    // package-manager.ts:440-459). The whole block already keys off `cfg.cwd`, never `project_root`.
    if cfg.trusted_project {
        let base = &cfg.cwd;
        {
            if cfg.enable_skills {
                // `.cyrup/skills` (Pi `.pi/skills`) is read at `cwd` only — `projectBaseDir =
                // join(cwd, CONFIG_DIR_NAME)` (package-manager.ts:900), no ancestor walk.
                {
                    let root = base.join(".cyrup/skills");
                    let mut buf = Vec::new();
                    scan_skill_root(
                        &root,
                        ResourceScope::Project,
                        &root,
                        &mut buf,
                        &mut warnings,
                        &mut diagnostics,
                    );
                    buf.retain(|s| {
                        override_enabled(&s.skill_md, &cfg.project_overrides.skills, &root)
                    });
                    skills.extend(buf);
                }
                // `.agents/skills` is walked up **every ancestor** from `cwd` to the git repo root
                // (or the filesystem root if there is none) — Pi `collectAncestorAgentsSkillDirs`
                // (package-manager.ts:440-459) feeding `projectAgentsSkillDirs`
                // (package-manager.ts:2286-2290), each loaded with its own `.agents` baseDir
                // (2326-2342). The user-tier skills dir is filtered out so it is not double-counted,
                // 1:1 with Pi's `.filter((dir) => resolve(dir) !== resolve(userAgentsSkillsDir))` (2289).
                //
                // USER-TIER DIR — DOCUMENTED DEFERRAL (residual #6). Pi's `userAgentsSkillsDir =
                // join(getHomeDir(), ".agents", "skills")` is literally `$HOME/.agents/skills`
                // (package-manager.ts:2286; `getHomeDir` = `process.env.HOME || homedir()`, :217) — the
                // user tier of the SAME cross-tool `.agents/skills` convention walked above. cyrup
                // instead uses `global_agents_dir/skills` = `agent_dir/agents/skills`
                // (= `~/.cyrup/agent/agents/skills` by default; DiscoveryConfig::new + session-svc
                // builder.rs:481). Pi-faithful is `~/.agents/skills`, and cyrup-config/trust.rs:173
                // ALREADY uses `home.join(".agents")` for the matching trust-walk exclusion — so this
                // is a genuine divergence (not a deliberate relocation), but the complete fix is
                // out-of-scope here: it needs `$HOME/.agents/skills` plumbed from cyrup-config into
                // `DiscoveryConfig` (a new field) and set by cyrup-session-svc — both outside the
                // editable crate set for this pass. Tracked in spec/gap-analysis/00-residual-ledger.md #6.
                // Dedup the ancestor walk against the user-tier `~/.agents/skills` (Pi
                // `.filter((dir) => resolve(dir) !== resolve(userAgentsSkillsDir))`,
                // package-manager.ts:2289) when plumbed; otherwise fall back to the legacy
                // `global_agents_dir/skills` exclusion.
                let user_agents_skills = cfg.user_agents_dir.as_ref().map_or_else(
                    || cfg.global_agents_dir.join("skills"),
                    |d| d.join("skills"),
                );
                for root in collect_ancestor_agents_skill_dirs(&cfg.cwd) {
                    if root == user_agents_skills {
                        continue;
                    }
                    let mut buf = Vec::new();
                    scan_skill_root(
                        &root,
                        ResourceScope::Project,
                        &root,
                        &mut buf,
                        &mut warnings,
                        &mut diagnostics,
                    );
                    buf.retain(|s| {
                        override_enabled(&s.skill_md, &cfg.project_overrides.skills, &root)
                    });
                    skills.extend(buf);
                }
            }
            if cfg.enable_prompts {
                let root = base.join(".cyrup/prompts");
                let mut buf = Vec::new();
                scan_prompt_root(
                    &root,
                    ResourceScope::Project,
                    &root,
                    &mut buf,
                    &mut warnings,
                );
                buf.retain(|p| override_enabled(&p.path, &cfg.project_overrides.prompts, &root));
                prompts.extend(buf);
            }
            if cfg.enable_themes {
                let root = base.join(".cyrup/themes");
                let mut buf = Vec::new();
                scan_theme_root(
                    &root,
                    ResourceScope::Project,
                    &root,
                    &mut buf,
                    &mut warnings,
                );
                buf.retain(|t| {
                    t.origin_path
                        .as_deref()
                        .is_none_or(|p| override_enabled(p, &cfg.project_overrides.themes, &root))
                });
                themes.extend(buf);
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
            add_prompt_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                &mut prompts,
                &mut warnings,
            );
        }
    }
    if cfg.enable_themes {
        for p in &cfg.extra.theme_paths {
            add_theme_path(
                p,
                ResourceScope::Discovered,
                ResourceOrigin::Builtin,
                &mut themes,
                &mut warnings,
            );
        }
    }

    // --- explicit CLI flags (Pi `source:"cli", scope:"temporary"`; Pi appends `additionalSkillPaths`
    // after the entire sorted accumulator, resource-loader.ts:421, so under first-wins they lose to a
    // same-name resource of any rank, including a rank-4 package — modeled as rank 5)
    // (R-09-002/008/012) ---
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
    emit_collisions(
        &registry.skills,
        ResourceKind::Skill,
        |s| s.skill_md.clone(),
        &mut diagnostics,
    );
    emit_collisions(
        &registry.prompts,
        ResourceKind::Prompt,
        |p| p.path.clone(),
        &mut diagnostics,
    );
    emit_collisions(
        &registry.themes,
        ResourceKind::Theme,
        |t| t.origin_path.clone().unwrap_or_default(),
        &mut diagnostics,
    );

    Ok(DiscoveryReport {
        registry,
        warnings,
        diagnostics,
    })
}

/// Emit a `collision` diagnostic for every shadowed same-name candidate in `set`.
fn emit_collisions<T: Named + Clone>(
    set: &ResourceSet<T>,
    kind: ResourceKind,
    path_of: impl Fn(&T) -> PathBuf,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let mut seen: std::collections::HashSet<(ResourceKey, PathBuf)> =
        std::collections::HashSet::new();
    // Resolve symlinks before comparing so a duplicate reached via a symlink collapses onto the
    // real file rather than surfacing as a spurious collision (skills.ts:403-408 `canonicalizePath`
    // + `realPathSet`). Falls back to the raw path when the file cannot be canonicalized.
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    for candidate in set.all() {
        let Some(winner) = set.get(candidate.key()) else {
            continue;
        };
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
    scan_skill_dir(
        root,
        scope,
        origin_root,
        true,
        Vec::new(),
        out,
        warnings,
        diagnostics,
    );
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
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
    children.sort();

    // Accumulate `.gitignore`/`.ignore`/`.fdignore` rules from this directory, prefixed relative to
    // the scan root, then build a matcher for the rules seen so far (skills.ts:16,47-65,188-189).
    collect_ignore_patterns(dir, origin_root, &mut ignore_patterns);
    let matcher = build_ignore(origin_root, &ignore_patterns);
    let is_ignored = |path: &Path, is_dir: bool| -> bool {
        let Ok(rel) = path.strip_prefix(origin_root) else {
            return false;
        };
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
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
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
        let Ok(content) = std::fs::read_to_string(dir.join(filename)) else {
            continue;
        };
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
    Some(if negated {
        format!("!{prefixed}")
    } else {
        prefixed
    })
}

/// Build a gitignore matcher rooted at the scan root from the accumulated prefixed patterns.
fn build_ignore(root: &Path, patterns: &[String]) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    for pattern in patterns {
        let _ = builder.add_line(None, pattern);
    }
    builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

/// Load the plain-path positive listings from a settings `skills`/`prompts`/`themes` array at the
/// settings tier (`scope`), via [`resolve_local_entries`] (Pi `resolveLocalEntries`,
/// package-manager.ts:2218-2239). The pattern subset of each array has already selected the enabled
/// files; everything returned here is loaded. Resource-kind enable flags are honored.
#[allow(clippy::too_many_arguments)]
fn add_local_entries(
    base: &Path,
    overrides: &ResourceOverrides,
    scope: ResourceScope,
    cfg: &DiscoveryConfig,
    skills: &mut Vec<Skill>,
    prompts: &mut Vec<PromptTemplate>,
    themes: &mut Vec<Theme>,
    ext_paths: &mut Vec<PathBuf>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    // `extensions` is the FIRST entry of Pi's `RESOURCE_TYPES` (package-manager.ts:194) and goes
    // through the very same `resolveLocalEntries` pass (:905-931). A settings-declared extension
    // root is therefore LOADED, not just pattern-filtered (CFG-004).
    for e in resolve_local_entries(
        base,
        &overrides.extensions,
        ManifestResourceType::Extensions,
    ) {
        if !ext_paths.contains(&e) {
            ext_paths.push(e);
        }
    }
    if cfg.enable_skills {
        for md in resolve_local_entries(base, &overrides.skills, ManifestResourceType::Skills) {
            let root = md.parent().unwrap_or(&md).to_path_buf();
            load_one_skill(&md, scope, &root, skills, warnings, diagnostics);
        }
    }
    if cfg.enable_prompts {
        for p in resolve_local_entries(base, &overrides.prompts, ManifestResourceType::Prompts) {
            let origin = ResourceOrigin::LooseFile {
                scope,
                root: p.parent().unwrap_or(&p).to_path_buf(),
            };
            match PromptTemplate::load(&p, scope, origin) {
                Ok(t) => prompts.push(t),
                Err(e) => {
                    warnings.push(ResourceWarning::new(ResourceKind::Prompt, p, e.to_string()))
                }
            }
        }
    }
    if cfg.enable_themes {
        for t in resolve_local_entries(base, &overrides.themes, ManifestResourceType::Themes) {
            let origin = ResourceOrigin::LooseFile {
                scope,
                root: t.parent().unwrap_or(&t).to_path_buf(),
            };
            match Theme::load(&t, scope, origin) {
                Ok(th) => themes.push(th),
                Err(e) => {
                    warnings.push(ResourceWarning::new(ResourceKind::Theme, t, e.to_string()))
                }
            }
        }
    }
}

fn load_one_skill(
    md: &Path,
    scope: ResourceScope,
    origin_root: &Path,
    out: &mut Vec<Skill>,
    warnings: &mut Vec<ResourceWarning>,
    diagnostics: &mut Vec<ResourceDiagnostic>,
) {
    let origin = ResourceOrigin::LooseFile {
        scope,
        root: origin_root.to_path_buf(),
    };
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
        let origin = ResourceOrigin::LooseFile {
            scope,
            root: origin_root.to_path_buf(),
        };
        match PromptTemplate::load(&md, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(
                ResourceKind::Prompt,
                md,
                e.to_string(),
            )),
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
        let origin = ResourceOrigin::LooseFile {
            scope,
            root: origin_root.to_path_buf(),
        };
        match Theme::load(&json, scope, origin) {
            Ok(t) => out.push(t),
            Err(e) => warnings.push(ResourceWarning::new(
                ResourceKind::Theme,
                json,
                e.to_string(),
            )),
        }
    }
}

/// Direct file children of `dir` with the given extension, sorted. Non-recursive.
fn direct_children(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
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
            warnings.push(ResourceWarning::new(
                ResourceKind::Prompt,
                path,
                e.to_string(),
            ));
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
        Err(e) => warnings.push(ResourceWarning::new(
            ResourceKind::Theme,
            path,
            e.to_string(),
        )),
    }
}
