//! Package-tree resolution for [`discover`](super::discover) — the settings-declared
//! ([`ConfiguredPackage`], CFG-003) and install-registry channels, their per-type filters, and the
//! loose-resource override/ancestor-walk helpers the collectors in [`super::blocking`] share.

use std::path::{Path, PathBuf};

use cyrup_core::CancelToken;

use crate::error::{ResourceDiagnostic, ResourceError, ResourceKind};
use crate::package::store::installed_dir;
use crate::package::{ConfiguredPackage, DisabledSet, PackageFilter};
use crate::scope::{InstallScope, ResourceScope};

use super::DiscoveryConfig;

/// One package working tree queued for resource collection, from either the settings channel
/// ([`ConfiguredPackage`], CFG-003) or the install registry ([`InstalledPackage`]).
pub(super) struct PackageTree {
    pub(super) dir: PathBuf,
    pub(super) id: cyrup_core::PackageId,
    pub(super) tier: ResourceScope,
    pub(super) disabled: DisabledSet,
    pub(super) filter: PackageFilter,
    /// Set only on the USER/global half of a `dedupePackages` DELTA PAIR — the filter of the
    /// project-scope `autoload: false` entry that deltas over this one. See
    /// [`subtract_delta_shadow`].
    pub(super) delta_shadow: Option<PackageFilter>,
}

/// Borrowed view of a [`PackageTree`] used inside the collection loop.
pub(super) struct PackageTreeRef<'a> {
    pub(super) id: &'a cyrup_core::PackageId,
    pub(super) disabled: &'a DisabledSet,
    pub(super) filter: &'a PackageFilter,
    pub(super) delta_shadow: Option<&'a PackageFilter>,
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
pub(super) fn retain_by_package_filter<T>(
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
pub(super) fn subtract_delta_shadow<T>(
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

/// What a settings-declared package entry resolved to.
///
/// Pi's `resolveLocalExtensionSource` (package-manager.ts:1316-1345 @v0.83.0) has THREE outcomes for
/// a local source and only one of them is a working tree, so [`resolve_configured_package`] cannot
/// return a bare [`PackageTree`]:
///
/// * `:1324-1326` — the resolved path does not exist ⇒ **silent return**, no diagnostic;
/// * `:1330-1334` — the resolved path is a FILE ⇒ it *is* the extension, registered directly with
///   `metadata.baseDir = dirname(resolved)`;
/// * `:1335-1341` — the resolved path is a directory ⇒ the working-tree walk (with the bare-directory
///   fallback at `:1338-1340`).
pub(super) enum ConfiguredPackageResolution {
    /// A directory to walk for resources — Pi `:1335-1341`.
    Tree(Box<PackageTree>),
    /// A single file that is itself an extension — Pi `:1330-1334`.
    ExtensionFile(PathBuf),
    /// Nothing to do, and nothing to say about it — Pi `:1324-1326`.
    Skip,
}

/// Pi `getBaseDirForScope` (package-manager.ts:2071-2080 @v0.83.0): `join(cwd, CONFIG_DIR_NAME)` for
/// a project entry (`.pi` upstream, `.cyrup` here), the agent dir for a user entry. The base a
/// settings-declared LOCAL source resolves against, both for its working tree and for its identity.
pub fn scope_base_dir(cwd: &Path, global_dir: &Path, scope: InstallScope) -> PathBuf {
    match scope {
        InstallScope::Project => cwd.join(".cyrup"),
        InstallScope::Global => global_dir.to_path_buf(),
    }
}

/// Materialize a settings-declared git package that has no working tree yet — Pi `installGit`'s
/// fresh-clone path (package-manager.ts:1831-1837 @v0.83.0), reached from the git arm of
/// `resolvePackageSources` (`:1287-1291`) through `installMissing` (`:1260-1271`) and
/// `installParsedSource` (`:1347-1356`).
///
/// The install ROOT is prepared before the clone, exactly as upstream orders it: `getGitInstallRoot`
/// then `ensureGitIgnore` (`:1831-1834`) — at project scope that root is inside the user's own
/// repository, so without it the clone shows up in `git status` (CFG-037). [`git_clone`] then
/// stages and renames, so a failure leaves nothing behind at `dir`.
///
/// **No registry row is written.** pi has no install registry: a settings-declared package IS the
/// declaration, re-read from `settings.json` on every `resolve()` (package-manager.ts:891-901), and
/// its tree is found by deriving the path from the source (`getGitInstallPath`, `:2031-2040`).
/// cyrup's `packages.json` records what `cyrup install` did, on purpose — writing a row here would
/// invent a package the user never installed and make `cyrup remove` fight the settings file.
/// Discovery resolves this tree by the same path derivation ([`installed_dir`]), so the row is not
/// needed to load it.
pub(crate) fn install_declared_git_package(
    source: &crate::package::PackageSource,
    scope: InstallScope,
    dir: &Path,
    cfg: &DiscoveryConfig,
) -> Result<(), ResourceError> {
    let crate::package::PackageSource::Git { url, reff } = source else {
        // npm is rejected by `PackageSource::parse` before reaching here (CFG-009) and OCI has no
        // fetcher (R-09-021); neither is a `git clone`.
        return Err(ResourceError::UnsupportedOci);
    };
    let store =
        crate::package::PackageStore::new(cfg.package_global_dir.clone(), cfg.project_root.clone());
    if let Some(root) = store.packages_root(scope) {
        crate::package::install::ensure_git_ignore(&root)?;
    }
    crate::package::install::git_clone(url, dir, reff.ref_name().map(str::to_string))?;
    Ok(())
}

/// Resolve a settings-declared package entry to its on-disk working tree.
///
/// Pi's `resolvePackageSources` (package-manager.ts:1240-1298 @v0.83.0) resolves a `local` source
/// against the scope base dir (`getBaseDirForScope`, `:2071-2088`: `<cwd>/.cyrup` for project, the
/// agent dir for user) and *installs* an npm/git source that is missing.
///
/// **CFG-003 — the git install arm is now ported.** When the working tree is absent, the git arm
/// (`:1287-1292`) calls `installMissing` (`:1260-1271`), which installs unless
/// `isOfflineModeEnabled()` (`:42-46`) or an `onMissing` callback declines; that reaches
/// `installParsedSource` (`:1347-1356`) → `installGit` (`:1820-1852`), whose fresh-clone path is
/// `ensureGitIgnore(gitRoot)` (`:1831-1834`, ported as [`crate::package::install::ensure_git_ignore`])
/// followed by the clone (`:1837`, ported as [`crate::package::install::git_clone`]). Whether this
/// pass may install at all is [`DiscoveryConfig::install_missing_packages`], which carries pi's
/// offline gate and its `onMissing` "skip" answer in one flag — see that field.
///
/// **[CYRUP-DELTA]**, on the two arms that remain: (a) when the install is not permitted pi simply
/// `continue`s (`:1290`, silent) and cyrup emits a loud [`ResourceDiagnostic`] naming the package,
/// and (b) when the install FAILS pi throws out of `resolve()` and takes the session build down
/// (`installGit`'s `throw error`, `:1849`) while cyrup reports the error as a diagnostic and carries
/// on with the rest of the resource set (constraint: malformed/missing declarations fail loudly +
/// safely). npm and OCI sources never reach the install arm at all (R-09-021, CFG-009).
///
/// `all` is the full declared set, needed for Pi's `findAutoloadDeltaBase`
/// (package-manager.ts:1285-1299): a project-scope `autoload: false` entry RESOLVES against the
/// user-scope entry it deltas over (`resolvedSource`/`resolvedScope`, :1232-1234) so the pair lands
/// on one working tree even where the two scopes resolve the same source string differently — a
/// relative local path, or an npm install root. Its `tier` still comes from its own scope, exactly
/// like Pi's `metadata` (`{ source: sourceStr, scope, … }`, :1235).
pub(super) fn resolve_configured_package(
    declared: &ConfiguredPackage,
    all: &[ConfiguredPackage],
    cfg: &DiscoveryConfig,
    cancel: &CancelToken,
) -> Result<ConfiguredPackageResolution, Box<ResourceDiagnostic>> {
    let tier = declared.scope.package_resource_scope();
    // Pi `findAutoloadDeltaBase` (package-manager.ts:1301-1313 @v0.83.0) pairs the two entries by
    // `getPackageIdentity`, and computes each side against ITS OWN scope base (`…, scope)` at :1307
    // vs `…, "user")` at :1311). A raw source-string comparison paired `"./pack"` in both scopes,
    // which pi does NOT: the project one is `local:<cwd>/.cyrup/pack`, the global one is
    // `local:<agent_dir>/pack` — two different trees, so there is no delta base (CFG-026).
    let delta_base = if declared.scope == InstallScope::Project && declared.filter.is_delta() {
        let identity = crate::package::package_identity(
            &declared.source,
            &scope_base_dir(&cfg.cwd, &cfg.global_dir, InstallScope::Project),
        );
        let global_base = scope_base_dir(&cfg.cwd, &cfg.global_dir, InstallScope::Global);
        all.iter().find(|e| {
            e.scope == InstallScope::Global
                && crate::package::package_identity(&e.source, &global_base) == identity
        })
    } else {
        None
    };
    let resolve_scope = delta_base.map_or(declared.scope, |e| e.scope);
    let resolve_source = delta_base.map_or(declared.source.as_str(), |e| e.source.as_str());
    let base = scope_base_dir(&cfg.cwd, &cfg.global_dir, resolve_scope);
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
        // `resolveLocalExtensionSource` (package-manager.ts:1316-1345 @v0.83.0), reached from
        // `resolvePackageSources` at `:1254-1257` for every `parsed.type === "local"` entry.
        crate::package::PackageSource::Path { path } => {
            // `resolvePathFromBase` normalizes before testing absoluteness (package-manager.ts:
            // 2069-2071 → paths.ts:57-85 @v0.83.0), so `"packages": ["~/pack"]` resolves under the
            // home dir instead of producing `<base>/~/pack` and the misleading
            // "not installed — run `cyrup install`" diagnostic below (CFG-025).
            let normalized =
                PathBuf::from(cyrup_config::paths::normalize_path(&path.to_string_lossy()));
            let resolved = if normalized.is_absolute() {
                normalized
            } else {
                base.join(normalized)
            };
            // package-manager.ts:1329 `statSync(resolved)` inside a `try` whose `catch` is a bare
            // `return` (`:1343-1345`), preceded by `if (!existsSync(resolved)) return;`
            // (`:1324-1326`). BOTH are SILENT: a local path that is absent, or that cannot be
            // stat'ed (a broken symlink, a permission error), contributes nothing and says nothing.
            // The "run `cyrup install`" diagnostic below is cyrup's no-network DELTA and belongs
            // only to the git/oci arm — Pi installs those, and cannot install a local path (CFG-027).
            let Ok(meta) = std::fs::metadata(&resolved) else {
                return Ok(ConfiguredPackageResolution::Skip);
            };
            // package-manager.ts:1330-1334 — a local entry that is a FILE *is* the extension:
            // `metadata.baseDir = dirname(resolved)` then `addResource(accumulator.extensions,
            // resolved, metadata, true)`. It is never walked for skills/prompts/themes.
            if meta.is_file() {
                return Ok(ConfiguredPackageResolution::ExtensionFile(resolved));
            }
            // `:1335` gates the walk on `stats.isDirectory()`; anything else (a fifo, a socket)
            // falls out of both branches and returns silently.
            if !meta.is_dir() {
                return Ok(ConfiguredPackageResolution::Skip);
            }
            resolved
        }
        // git/oci: the derived working-tree path — Pi `getGitInstallPath` (package-manager.ts:
        // 2031-2040 @v0.83.0). It is where a previous `cyrup install` materialized the tree, and
        // where the CFG-003 auto-install below puts one when it is absent.
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
    // Only reachable for a git/oci source now: the local arm above has already returned for every
    // non-directory outcome, Pi-silently.
    //
    // CFG-003 — pi's `if (!existsSync(installedPath)) { const installed = await installMissing();
    // if (!installed) continue; }` (package-manager.ts:1287-1291 @v0.83.0). The clone runs on this
    // thread: `discover_blocking` is already inside `spawn_blocking` and [`git_clone`] is
    // synchronous, so there is no future to drop mid-install and no `.await` between the clone and
    // the tree being read. Cancellation is checked BEFORE starting one — an aborted session build
    // must not begin a fetch, and a token that fires mid-clone cannot abort the blocking task, so
    // the check has to sit ahead of the work rather than behind it.
    if !dir.is_dir() {
        if !cfg.install_missing_packages {
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
        if cancel.is_cancelled() {
            return Ok(ConfiguredPackageResolution::Skip);
        }
        if let Err(e) = install_declared_git_package(&source, resolve_scope, &dir, cfg) {
            return Err(Box::new(ResourceDiagnostic::error(
                ResourceKind::Package,
                &dir,
                format!(
                    "package {:?} is declared in settings and could not be installed: {e}",
                    declared.source
                ),
            )));
        }
    }
    Ok(ConfiguredPackageResolution::Tree(Box::new(PackageTree {
        dir,
        id,
        tier,
        disabled: DisabledSet::default(),
        filter: declared.filter.clone(),
        delta_shadow: None,
    })))
}

/// Whether an auto-discovered loose resource file is enabled by a settings override list. The match
/// base is the parent of the conventional `skills`/`prompts`/`themes` directory (`root.parent()`), so
/// a settings pattern like `!skills/internal` filters by the conventional-relative path, matching
/// Pi's `projectBaseDir`/`globalBaseDir`-relative matching (package-manager.ts:2271-2304).
pub(super) fn override_enabled(path: &Path, patterns: &[String], root: &Path) -> bool {
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
pub(super) fn collect_ancestor_agents_skill_dirs(start_dir: &Path) -> Vec<PathBuf> {
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
