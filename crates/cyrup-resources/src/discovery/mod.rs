//! Unified resource discovery + loader (arch-09 §3.2, §6.1, R-09-022..024).
//!
//! `discover()` fans across roots (built-in, global, global-agents, project walk, installed
//! packages, CLI flags, `resources_discover` contributions), parses each kind, and merges into a
//! [`ResourceRegistry`] with deterministic same-name precedence (R-09-024).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cyrup_core::CancelToken;

use crate::error::{ResourceDiagnostic, ResourceError, ResourceWarning};
use crate::key::ResourceKey;
use crate::package::{ConfiguredPackage, DisabledSet, InstalledPackages};
use crate::prompt::PromptTemplate;
use crate::scope::{ResourceOrigin, ResourceScope};
use crate::skill::Skill;
use crate::theme::Theme;

mod blocking;
mod packages;
mod scan;

use blocking::discover_blocking;
use scan::{add_prompt_path, add_skill_path, add_theme_path};

#[cfg(test)]
pub(crate) use packages::install_declared_git_package;
pub use packages::scope_base_dir;

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
    /// **Global**-scope package working trees, i.e. `<package_global_dir>/<id>` (CFG-054 — this was
    /// `<package_global_dir>/packages/<id>` until [`crate::package::PackageStore::packages_root`] stopped doubling
    /// the segment). Kept DISTINCT from `global_dir` (which roots the loose global resources at
    /// `<global_dir>/skills`, `/prompts`, `/themes`) because the bin passes its `package_dir` — not
    /// `agent_dir` — as the store root. Defaults to `<global_dir>/packages` (the bin's own default
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
    /// Whether a settings-declared GIT package whose working tree is not materialized may be cloned
    /// during this discovery pass (CFG-003).
    ///
    /// This one flag carries BOTH halves of upstream's gate, because upstream has exactly two
    /// callers and they sit at the two ends of it. `resolvePackageSources`' `installMissing`
    /// (package-manager.ts:1260-1271 @v0.83.0) installs UNCONDITIONALLY unless
    /// `isOfflineModeEnabled()` (`:42-46`, `PI_OFFLINE`) or an optional `onMissing(source)` answers
    /// `"skip"`/`"error"`. The session path — `ResourceLoader.reload()` and
    /// `loadCurrentExtensionSet()` — calls `packageManager.resolve()` with **no** `onMissing`
    /// (resource-loader.ts:403, :549 @v0.83.0), i.e. install; the startup-theme pass calls
    /// `packageManager.resolve(async () => "skip")` (cli/startup-ui.ts:73 @v0.83.0), i.e. do not.
    ///
    /// So `true` is pi's resource-loader behaviour and `false` is pi's startup-ui behaviour, and
    /// `false` is the default: a caller that has not thought about the network gets the pass that
    /// touches no network, and the bin turns it on for the session build unless
    /// `--offline`/`CYRUP_OFFLINE`/`PI_OFFLINE` is set (`SessionConfig::install_missing_packages`).
    /// npm and OCI sources are unaffected — they never reach this arm (R-09-021, CFG-009).
    pub install_missing_packages: bool,
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
            // Pi's `resolve(async () => "skip")` caller, not its `resolve()` one — see the field
            // docs. The bin flips it for the session build.
            install_missing_packages: false,
            configured_packages: Vec::new(),
            extra: DiscoveredPaths::default(),
            disabled: DisabledSet::default(),
            global_overrides: ResourceOverrides::default(),
            project_overrides: ResourceOverrides::default(),
        }
    }
}

/// One auto-discovered ("loose") extension sitting directly under a conventional extensions root —
/// `<agent_dir>/extensions` or `<cwd>/.cyrup/extensions`. Pi's `collectAutoExtensionEntries`
/// (package-manager.ts:587-630, dispatched from `collectResourceFiles` :650) produces the same tier:
/// the entries a settings `extensions` array's `+`/`-` patterns filter, as opposed to the
/// plain-path positive listings that land in [`ResourceRegistry::ext_crate_paths`].
///
/// Unlike skills/prompts/themes, a disabled entry is **kept** here with `enabled: false` rather than
/// dropped, because the disabled set is itself load-bearing: `cyrup-session-svc` hands it to
/// `cyrup_ext::DiscoveryRoots::disabled` so the extension loader honours the same `-pattern` the
/// `cyrup config` editor writes.
#[derive(Debug, Clone)]
pub struct LooseExtension {
    /// The extension itself — a subdirectory of `root`, or a bare `*.wasm` artifact sitting in it.
    pub path: PathBuf,
    /// [`ResourceScope::Global`] or [`ResourceScope::Project`] — which settings layer's
    /// `extensions` array governs it, and which config-editor group it renders under.
    pub scope: ResourceScope,
    /// The conventional root it was found in (`…/extensions`). Its PARENT is the base the settings
    /// pattern is relative to, exactly as discovery's own `override_enabled` computes it.
    pub root: PathBuf,
    /// Whether the governing settings `extensions` array leaves it enabled.
    pub enabled: bool,
}

/// The immutable snapshot the rest of the app reads (swapped atomically on `/reload`).
#[derive(Debug, Clone, Default)]
pub struct ResourceRegistry {
    pub skills: ResourceSet<Skill>,
    pub prompts: ResourceSet<PromptTemplate>,
    pub themes: ResourceSet<Theme>,
    /// Extension crate dirs found in packages — handed to cyrup-ext to build.
    pub ext_crate_paths: Vec<PathBuf>,
    /// Auto-discovered loose extensions under the two conventional roots, each tagged with the
    /// settings verdict — see [`LooseExtension`].
    pub loose_extensions: Vec<LooseExtension>,
}

impl ResourceRegistry {
    /// Merge extension-contributed skill/prompt/theme paths into this snapshot, returning a NEW
    /// snapshot (the registry is immutable; callers atomically swap the result in). 1:1 with Pi's
    /// `ResourceLoader.extendResources` (resource-loader.ts:293) as driven by
    /// `extendResourcesFromExtensions` (agent-session.ts:2112-2135): each contributed file is loaded
    /// at the [`ResourceScope::Discovered`] tier (Pi's `scope:"temporary"` extension resources, rank
    /// `6`) and folded back through [`ResourceSet::build`], so a same-name user/package/CLI resource
    /// still wins (first-wins precedence). Existing candidates are preserved; `ext_crate_paths` and
    /// `loose_extensions` are carried over unchanged. Parse failures / missing paths are dropped
    /// (Pi logs a warning and skips), never panicking.
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
            loose_extensions: self.loose_extensions.clone(),
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
    /// The discovered project/global `SYSTEM.md`, or `None` — [`discover_system_prompt_file`].
    ///
    /// Pi computes this inside `reload()` (`resource-loader.ts:525` @v0.83.0) and it is consumed
    /// **only when the CLI gave no `--system-prompt`**: `this.systemPromptSource ??
    /// this.discoverSystemPromptFile()`. The caller therefore treats this as the *fallback* for
    /// `custom_prompt`, never as an override of it (CFG-035).
    pub system_prompt_file: Option<PathBuf>,
    /// The discovered project/global `APPEND_SYSTEM.md`, or `None` —
    /// [`discover_append_system_prompt_file`].
    ///
    /// Pi's `reload()` (`resource-loader.ts:531-535` @v0.83.0) uses it as the SOLE entry of
    /// `appendSources` when `this.appendSystemPromptSource` is unset — an explicit
    /// `--append-system-prompt` REPLACES it rather than accumulating with it (CFG-035).
    pub append_system_prompt_file: Option<PathBuf>,
}

/// The project-or-global `SYSTEM.md` that overrides the built-in system prompt.
///
/// Port of `ResourceLoader.discoverSystemPromptFile` (`resource-loader.ts:1022-1034` @v0.83.0,
/// unchanged at v0.84.1):
///
/// 1. `<cwd>/.cyrup/SYSTEM.md` when the project is trusted **and** the file exists (`:1023-1026`);
/// 2. otherwise `<agent_dir>/SYSTEM.md` when it exists (`:1028-1031`);
/// 3. otherwise `None` (`:1033`).
///
/// The trust gate is on the PROJECT candidate only — an untrusted project falls through to the
/// global file rather than to nothing, and a trusted project without the file does the same.
/// cyrup already ported the trust gate that names this file
/// (`cyrup_config::trust::has_trust_requiring_resources`) without ever porting the read, so the
/// user was answering a security question about a file cyrup would never open.
#[must_use]
pub fn discover_system_prompt_file(
    cwd: &Path,
    agent_dir: &Path,
    project_trusted: bool,
) -> Option<PathBuf> {
    discover_prompt_override(cwd, agent_dir, project_trusted, "SYSTEM.md")
}

/// The manifest file name an extension directory is recognised by
/// (`cyrup_ext::manifest::MANIFEST_FILE`, restated — see [`scan_loose_extension_root`]).
const EXTENSION_MANIFEST_FILE: &str = "extension.json";

/// True iff `dir` directly holds an extension: an `extension.json`, or a prebuilt `*.wasm`
/// component. Restatement of `cyrup_ext::loader::is_extension_dir`
/// (`crates/cyrup-ext/src/loader.rs:208-210`) — see [`scan_loose_extension_root`] for why it is
/// restated rather than imported.
fn is_extension_dir(dir: &Path) -> bool {
    if dir.join(EXTENSION_MANIFEST_FILE).is_file() {
        return true;
    }
    std::fs::read_dir(dir).is_ok_and(|rd| {
        rd.filter_map(Result::ok).any(|e| {
            let p = e.path();
            p.extension().is_some_and(|x| x == "wasm") && p.is_file()
        })
    })
}

/// Enumerate the loose extensions directly under one conventional `…/extensions` root, tagging each
/// with the verdict `patterns` (the governing settings-layer `extensions` array) gives it.
///
/// Port of Pi's `collectAutoExtensionEntries` (`package-manager.ts:587-630`, dispatched from
/// `collectResourceFiles` :650): **one level, no recursion**, and two accepted entry shapes — a
/// subdirectory that itself holds an extension, or a bare artifact sitting straight in the root
/// (Pi's `*.ts`/`*.js`, cyrup's `*.wasm`).
///
/// The two shapes must agree exactly with what the extension loader will accept —
/// `cyrup_ext::loader::scan_dir` (`crates/cyrup-ext/src/loader.rs:237-256`) — or a row would appear
/// in `cyrup config` that nothing loads, or vice-versa. The predicate is nevertheless **restated**
/// here rather than imported: `cyrup-resources` does not depend on `cyrup-ext` (and must not, since
/// `cyrup-ext` sits above it and the edge would close a cycle).
///
/// A disabled entry is retained with `enabled: false` instead of being dropped — the caller needs
/// the negative half too (see [`LooseExtension`]). That is the one deliberate difference from the
/// `buf.retain(override_enabled(…))` pass the other three loose kinds get in `blocking.rs`.
#[must_use]
pub fn scan_loose_extension_root(
    root: &Path,
    scope: ResourceScope,
    patterns: &[String],
) -> Vec<LooseExtension> {
    let Ok(rd) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            if p.is_dir() {
                is_extension_dir(p)
            } else {
                p.extension().is_some_and(|x| x == "wasm") && p.is_file()
            }
        })
        .collect();
    // Deterministic order, matching `scan_dir`'s `entries.sort()` (loader.rs:246, R-08-004).
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let enabled = packages::override_enabled(&path, patterns, root);
            LooseExtension {
                path,
                scope,
                root: root.to_path_buf(),
                enabled,
            }
        })
        .collect()
}

/// The project-or-global `APPEND_SYSTEM.md` appended to the system prompt.
///
/// Port of `ResourceLoader.discoverAppendSystemPromptFile` (`resource-loader.ts:1036-1048`
/// @v0.83.0) — the identical two-tier, trust-gated pair as [`discover_system_prompt_file`].
///
/// Pi picks exactly **one** file: the project one shadows the global one, they never accumulate.
#[must_use]
pub fn discover_append_system_prompt_file(
    cwd: &Path,
    agent_dir: &Path,
    project_trusted: bool,
) -> Option<PathBuf> {
    discover_prompt_override(cwd, agent_dir, project_trusted, "APPEND_SYSTEM.md")
}

/// The shared body of Pi's two byte-identical discover functions (`resource-loader.ts:1022-1048`).
fn discover_prompt_override(
    cwd: &Path,
    agent_dir: &Path,
    project_trusted: bool,
    filename: &str,
) -> Option<PathBuf> {
    // `join(this.cwd, CONFIG_DIR_NAME, filename)` — CONFIG_DIR_NAME is `.pi` upstream, `.cyrup` here.
    let project_path = cwd.join(".cyrup").join(filename);
    if project_trusted && project_path.exists() {
        return Some(project_path);
    }
    let global_path = agent_dir.join(filename);
    if global_path.exists() {
        return Some(global_path);
    }
    None
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
    // CFG-003: the blocking pass can now clone a declared git package, so it needs the token
    // itself. `run_until_cancelled` below only stops WAITING — a `spawn_blocking` task is never
    // aborted — so without this a cancelled build would keep fetching every remaining package.
    let inner_cancel = cancel.clone();
    let join = tokio::task::spawn_blocking(move || discover_blocking(&cfg, &inner_cancel));
    match cancel.run_until_cancelled(join).await {
        Some(joined) => joined.map_err(|e| {
            ResourceError::Core(cyrup_core::CoreError::Io(std::io::Error::other(
                e.to_string(),
            )))
        })?,
        None => Err(ResourceError::Cancelled),
    }
}
