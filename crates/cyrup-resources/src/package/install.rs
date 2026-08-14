//! Package install / remove / list / update / enable-disable (arch-09 §3.6, §6.4).

use cyrup_core::{CancelToken, PackageId};

use crate::error::ResourceError;
use crate::package::lock;
use crate::package::source::PackageSource;
use crate::package::store::PackageStore;
use crate::package::{
    DisabledSet, InstalledPackage, ResourceSelector, SECURITY_CAVEAT, SecurityNotice, UpdateReport,
    UpdateTarget, now_stamp,
};
use crate::scope::InstallScope;

/// Manages package installs across global + project scopes.
#[derive(Clone, Debug)]
pub struct PackageManager {
    store: PackageStore,
}

/// The security caveat shown for every install (R-09-019).
pub fn security_notice_for(source: PackageSource) -> SecurityNotice {
    SecurityNotice {
        message: SECURITY_CAVEAT,
        source,
    }
}

/// Make a package install root self-ignoring (Pi `ensureGitIgnore`, package-manager.ts:1952-1960
/// @v0.83.0): create the directory when absent, then write `*\n!.gitignore\n` — byte for byte —
/// only when no `.gitignore` is already there, so a user's own file is never clobbered.
pub(crate) fn ensure_git_ignore(dir: &std::path::Path) -> Result<(), ResourceError> {
    if !dir.exists() {
        std::fs::create_dir_all(dir).map_err(|e| {
            ResourceError::Manifest(format!(
                "could not create package install root {}: {e}",
                dir.display()
            ))
        })?;
    }
    let ignore_path = dir.join(".gitignore");
    if !ignore_path.exists() {
        std::fs::write(&ignore_path, "*\n!.gitignore\n").map_err(|e| {
            ResourceError::Manifest(format!("could not write {}: {e}", ignore_path.display()))
        })?;
    }
    Ok(())
}

impl PackageManager {
    pub fn new(store: PackageStore) -> Self {
        Self { store }
    }

    /// Install a package from a source ref (R-09-018). Explicit user action only (R-09-019).
    /// Project scope requires `trusted` (R-09-017).
    ///
    /// The whole install — trust gate, install-root `.gitignore`, clone, `packages.json` upsert —
    /// runs inside ONE `spawn_blocking`, so it is a single unit that always settles.
    ///
    /// **Mechanism note (JS → Rust).** Pi's `installGit` (package-manager.ts:1820-1852 @v0.83.0)
    /// is an `async` function, and a JS async function cannot be torn down part-way: whatever it
    /// starts it finishes or throws, and its `catch` (`:1846-1851`) removes the partial tree. A
    /// Rust future is droppable at every `.await`, and this function used to have one *between*
    /// the clone and the registry write. Dropping there — a cancelled `cyrup install`, a Ctrl-C,
    /// a `select!` losing the race — does **not** stop the clone (`spawn_blocking` tasks are never
    /// aborted, the `JoinHandle` merely detaches), so a complete working tree landed at exactly
    /// the path [`crate::package::store::installed_dir`] resolves while `packages.json` never
    /// learned about it. `resolve_configured_package` keys off the DIRECTORY, not the registry
    /// (`discovery.rs`, the `installed_dir` arm), so the orphan then loads as a real installed
    /// package on every later session. There is no `.await` inside the unit any more.
    pub async fn install(
        &self,
        source: PackageSource,
        scope: InstallScope,
        trusted: bool,
        cancel: CancelToken,
    ) -> Result<(InstalledPackage, SecurityNotice), ResourceError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || this.install_blocking(source, scope, trusted, &cancel))
            .await
            .map_err(|e| ResourceError::Git(e.to_string()))?
    }

    /// The body of [`install`](Self::install), on a blocking thread. See that method for why the
    /// clone and the registry write must not be separated by an `.await`.
    fn install_blocking(
        &self,
        source: PackageSource,
        scope: InstallScope,
        trusted: bool,
        cancel: &CancelToken,
    ) -> Result<(InstalledPackage, SecurityNotice), ResourceError> {
        if scope == InstallScope::Project && !trusted {
            let p = match &source {
                PackageSource::Path { path } => path.clone(),
                PackageSource::Git { url, .. } => url.into(),
                PackageSource::Oci { reference } => reference.into(),
            };
            return Err(ResourceError::Untrusted(p));
        }
        if cancel.is_cancelled() {
            return Err(ResourceError::Cancelled);
        }

        let notice = security_notice_for(source.clone());
        let id = source.package_id();

        let resolved_commit = match &source {
            PackageSource::Oci { .. } => return Err(ResourceError::UnsupportedOci),
            PackageSource::Path { path } => {
                if !path.exists() {
                    return Err(ResourceError::Manifest(format!(
                        "local package path does not exist: {}",
                        path.display()
                    )));
                }
                None
            }
            PackageSource::Git { url, reff } => {
                let dir = self.store.package_dir(scope, &id).ok_or_else(|| {
                    ResourceError::Manifest(
                        "project package dir unavailable (no project root)".into(),
                    )
                })?;
                // `installGit` prepares the install ROOT before cloning — `const gitRoot =
                // this.getGitInstallRoot(scope); if (gitRoot) { this.ensureGitIgnore(gitRoot); }`
                // (package-manager.ts:1829-1834 @v0.83.0). At project scope that root is inside the
                // user's repository, so without it the clone (and its nested `.git`) shows up in
                // `git status` (CFG-037).
                if let Some(root) = self.store.packages_root(scope) {
                    ensure_git_ignore(&root)?;
                }
                let ref_name = reff.ref_name().map(str::to_string);
                // A failed clone must leave NOTHING at `dir` — Pi's `installGit` catch is
                // `rmSync(targetDir, { recursive: true, force: true })` (package-manager.ts:1847
                // @v0.83.0). [`git_clone`] guarantees that by staging, so the `?` here is the
                // whole of Pi's `throw error` (`:1849`).
                Some(git_clone(url, &dir, ref_name)?)
            }
        };

        let rec = InstalledPackage {
            id: id.clone(),
            source,
            scope,
            resolved_commit,
            installed_at: now_stamp(),
            disabled: DisabledSet::default(),
        };

        let reg_path = self
            .store
            .registry_path(scope)
            .ok_or_else(|| ResourceError::Manifest("no registry path for scope".into()))?;
        let mut reg = lock::load(&reg_path)?;
        reg.upsert(rec.clone());
        lock::save(&reg_path, &reg)?;

        Ok((rec, notice))
    }

    /// Remove a package from whichever scope holds it.
    pub async fn remove(&self, id: &PackageId) -> Result<(), ResourceError> {
        let mut removed = false;
        for scope in [InstallScope::Global, InstallScope::Project] {
            let Some(reg_path) = self.store.registry_path(scope) else {
                continue;
            };
            let mut reg = lock::load(&reg_path)?;
            if reg.remove(id) {
                lock::save(&reg_path, &reg)?;
                // Best-effort: drop the cloned working tree (not Path installs).
                if let Some(dir) = self.store.package_dir(scope, id)
                    && dir.exists()
                {
                    let _ = std::fs::remove_dir_all(&dir);
                }
                removed = true;
            }
        }
        if removed {
            Ok(())
        } else {
            Err(ResourceError::Manifest(format!(
                "package not installed: {id}"
            )))
        }
    }

    /// List installs across both scopes.
    pub fn list(&self) -> Vec<InstalledPackage> {
        let mut out = Vec::new();
        for scope in [InstallScope::Global, InstallScope::Project] {
            if let Some(reg_path) = self.store.registry_path(scope)
                && let Ok(reg) = lock::load(&reg_path)
            {
                out.extend(reg.packages);
            }
        }
        out
    }

    /// Update one or all packages. `All` skips pinned (R-09-020); `One` updates regardless.
    pub async fn update(
        &self,
        target: UpdateTarget,
        cancel: CancelToken,
    ) -> Result<UpdateReport, ResourceError> {
        let mut report = UpdateReport::default();
        for scope in [InstallScope::Global, InstallScope::Project] {
            let Some(reg_path) = self.store.registry_path(scope) else {
                continue;
            };
            let mut reg = lock::load(&reg_path)?;
            let mut dirty = false;
            for pkg in &mut reg.packages {
                if cancel.is_cancelled() {
                    return Err(ResourceError::Cancelled);
                }
                match &target {
                    UpdateTarget::One(id) if &pkg.id != id => continue,
                    UpdateTarget::All if pkg.source.pin().is_pinned() => {
                        report.skipped_pinned.push(pkg.id.clone());
                        continue;
                    }
                    _ => {}
                }
                match refresh(pkg, scope, &self.store) {
                    Ok(()) => {
                        pkg.installed_at = now_stamp();
                        dirty = true;
                        report.updated.push(pkg.id.clone());
                    }
                    Err(e) => report.failed.push((pkg.id.clone(), e.to_string())),
                }
            }
            if dirty {
                lock::save(&reg_path, &reg)?;
            }
        }
        Ok(report)
    }

    /// Enable/disable a single resource within a package (R-09-018).
    pub fn set_enabled(
        &self,
        id: &PackageId,
        sel: ResourceSelector,
        enabled: bool,
    ) -> Result<(), ResourceError> {
        for scope in [InstallScope::Global, InstallScope::Project] {
            let Some(reg_path) = self.store.registry_path(scope) else {
                continue;
            };
            let mut reg = lock::load(&reg_path)?;
            if let Some(pkg) = reg.find_mut(id) {
                pkg.disabled.set(&sel, !enabled);
                lock::save(&reg_path, &reg)?;
                return Ok(());
            }
        }
        Err(ResourceError::Manifest(format!(
            "package not installed: {id}"
        )))
    }
}

/// Refresh an installed package. Git: re-clone the source URL (real fetch via `gix`) so the working
/// tree and recorded commit advance with the remote, mirroring Pi `updateGit`; for a bare on-disk
/// source path the local repo is re-copied and re-resolved. Path: no-op.
///
/// An unreachable source is a **failure of that package**, not a silent success. Pi's `updateGit`
/// (package-manager.ts:1853-1868 @v0.83.0) delegates to `ensureGitRef` (`:1870-1896`), whose very
/// first statement is `await this.runCommand("git", fetchArgs, …)`; `runCommand` rejects on a
/// non-zero exit, so the rejection propagates out of `update()` and the caller is told. cyrup used
/// to swallow the error, re-read the local `HEAD` and return `Ok(())` — which `PackageManager::update`
/// then recorded in `report.updated`, telling the user a package had been updated when the remote
/// had never been contacted. The error goes into `report.failed` now; the installed working tree is
/// untouched either way, because [`git_clone`] stages.
fn refresh(
    pkg: &mut InstalledPackage,
    scope: InstallScope,
    store: &PackageStore,
) -> Result<(), ResourceError> {
    match &pkg.source {
        PackageSource::Path { .. } => Ok(()),
        PackageSource::Oci { .. } => Err(ResourceError::UnsupportedOci),
        PackageSource::Git { url, reff } => {
            let Some(dir) = store.package_dir(scope, &pkg.id) else {
                return Ok(());
            };
            pkg.resolved_commit = Some(git_clone(url, &dir, reff.ref_name().map(str::to_string))?);
            Ok(())
        }
    }
}

/// Materialize a git package working tree at `dir` and return the resolved commit (hex).
///
/// Two transports (§7.6, utils/git.ts):
/// - A bare **on-disk repo directory** (a filesystem path, no URL scheme) is copied into the store
///   and the requested ref checked out from its object database — fully offline/deterministic.
/// - A **URL** (`file://`, `https://`, `http://`, `ssh://`, `git://`) is fetched with `gix`'s real
///   clone machinery (`blocking-network-client`), so remote installs work, not just local fixtures.
///
/// When `ref_name` is set (a branch/tag/commit pin, utils/git.ts:6-19), the named ref is resolved
/// and its tree materialized over the working copy so the pin is actually applied (R-09-018/020) —
/// the recorded commit is the ref's commit, not default HEAD. When it is absent, the cloned HEAD is
/// used.
///
/// **`dir` is only ever touched on success.** The clone runs into a staging sibling and is renamed
/// into place at the end; on any failure the staging tree is deleted and `dir` is byte-identical to
/// what it was. That is Pi's guarantee, arrived at differently: Pi never clones over an existing
/// tree at all — `installGit` early-returns into a fetch when `existsSync(targetDir)`
/// (package-manager.ts:1822-1830 @v0.83.0) and `updateGit` fetches **in place** and only then
/// `git reset --hard`s (`:1853-1868` → `ensureGitRef` `:1870-1896`), so a fetch that fails throws
/// with the installed tree untouched. cyrup has no in-place fetch path and re-clones instead, and
/// the previous shape wiped `dir` *first* (`git_clone_url`'s opening `remove_dir_all`), so a single
/// unreachable remote — an offline `cyrup update`, an expired token, a deleted branch — DELETED the
/// installed working tree of every git package it touched, while [`refresh`] went on to report
/// those packages as updated.
fn git_clone(
    url: &str,
    dir: &std::path::Path,
    ref_name: Option<String>,
) -> Result<String, ResourceError> {
    let staging = sibling_dir(dir, ".cyrup-incoming");
    let _ = std::fs::remove_dir_all(&staging);
    match git_clone_into(url, &staging, ref_name) {
        Ok(commit) => match promote_clone(&staging, dir) {
            Ok(()) => Ok(commit),
            Err(e) => {
                let _ = std::fs::remove_dir_all(&staging);
                Err(e)
            }
        },
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// `<dir>` with `suffix` appended to its final component, in the same parent directory — so the
/// rename in [`promote_clone`] stays on one filesystem.
fn sibling_dir(dir: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "package".to_string());
    match dir.parent() {
        Some(parent) => parent.join(format!("{name}{suffix}")),
        None => std::path::PathBuf::from(format!("{name}{suffix}")),
    }
}

/// Move a completed staging clone onto `dir`, replacing any previous tree. The previous tree is
/// moved aside first and only deleted once the new one is in place, so a failure mid-swap restores
/// it rather than leaving the package missing.
fn promote_clone(staging: &std::path::Path, dir: &std::path::Path) -> Result<(), ResourceError> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !dir.exists() {
        return std::fs::rename(staging, dir).map_err(ResourceError::Io);
    }
    let backup = sibling_dir(dir, ".cyrup-previous");
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(dir, &backup).map_err(ResourceError::Io)?;
    match std::fs::rename(staging, dir) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
            Ok(())
        }
        Err(e) => {
            // Put the working tree back before surfacing the failure.
            let _ = std::fs::rename(&backup, dir);
            Err(ResourceError::Io(e))
        }
    }
}

/// [`git_clone`]'s body, writing into `dir` directly. Callers other than [`git_clone`] must not use
/// it: it is the half that can leave a partial tree behind.
fn git_clone_into(
    url: &str,
    dir: &std::path::Path,
    ref_name: Option<String>,
) -> Result<String, ResourceError> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // A bare on-disk path (no URL scheme) is copied directly — deterministic and offline.
    if !url.contains("://") {
        let src = std::path::Path::new(url);
        if src.is_dir() {
            copy_tree(src, dir)?;
            return match ref_name {
                Some(reff) => checkout_ref(dir, &reff),
                None => git_head(dir),
            };
        }
        return Err(ResourceError::Git(format!(
            "not a git repo directory and not a URL: {url}"
        )));
    }
    // A scheme URL (file://, https://, http://, ssh://, git://) — real gix clone.
    git_clone_url(url, dir, ref_name)
}

/// Clone a git URL into `dir` with `gix` (real network/file transport via `blocking-network-client`)
/// and return the resolved commit (hex). Mirrors Pi's `git clone` step in install/updateGit.
fn git_clone_url(
    url: &str,
    dir: &std::path::Path,
    ref_name: Option<String>,
) -> Result<String, ResourceError> {
    use std::sync::atomic::AtomicBool;

    // `dir` is the staging path [`git_clone`] just cleared, so it is empty/absent by construction.
    // The old unconditional `remove_dir_all` lived here and is what made a failed fetch destructive.
    let prepare = gix::prepare_clone(url, dir).map_err(|e| ResourceError::Git(e.to_string()))?;
    // `prepare_clone` creates a FRESH repo whose config does NOT include the user's
    // ~/.gitconfig (credential helper, SSH command). Inject that auth config now — between
    // prepare_clone and fetch — so a PRIVATE https/ssh install authenticates with the user's
    // configured credentials exactly as Pi (system `git clone`) would. Best-effort: a no-op
    // when nothing relevant is configured, harmless for public/`file://` clones.
    let mut prepare = configure_clone(prepare);
    if let Some(reff) = ref_name.as_deref() {
        prepare = prepare
            .with_ref_name(Some(reff))
            .map_err(|e| ResourceError::Git(e.to_string()))?;
    }
    let interrupt = AtomicBool::default();
    let (mut checkout, _) = prepare
        .fetch_then_checkout(gix::progress::Discard, &interrupt)
        .map_err(|e| ResourceError::Git(e.to_string()))?;
    let (repo, _) = checkout
        .main_worktree(gix::progress::Discard, &interrupt)
        .map_err(|e| ResourceError::Git(e.to_string()))?;

    // Resolve the recorded commit: the pinned ref if given, else the checked-out HEAD.
    let commit_hex = match ref_name.as_deref() {
        Some(reff) => repo
            .rev_parse_single(reff)
            .map_err(|e| ResourceError::Git(format!("ref `{reff}` not found: {e}")))?
            .object()
            .map_err(|e| ResourceError::Git(e.to_string()))?
            .try_into_commit()
            .map_err(|e| ResourceError::Git(e.to_string()))?
            .id()
            .to_hex()
            .to_string(),
        None => git_head_repo(&repo)?,
    };
    Ok(commit_hex)
}

// ============================================================================
// Git auth / credential injection for remote clones
// ----------------------------------------------------------------------------
// Ported 1:1 from the proven cyrup-ai gix facade in
// kodegen-workspace/packages/kodegen-tools-git/src/operations/auth.rs
// (`GitConfig::read` + `to_gix_overrides` + `configure_clone`). `gix::prepare_clone`
// creates a fresh repo whose config does NOT include the user's ~/.gitconfig (credential
// helpers, ssh settings); we read the user's *effective* config via the `git` binary
// (so includes / system+global+local all resolve, mirroring Pi's reliance on system git)
// and feed it back as in-memory config overrides. gix 0.85's
// `PrepareFetch::with_in_memory_config_overrides` is API-identical to 0.75's, so the port
// is a straight copy with no behavioral change.
// ============================================================================

/// Cached, read-once user git auth config (mirrors kodegen's `GIT_CONFIG` `OnceLock`).
static GIT_AUTH_CONFIG: std::sync::OnceLock<GitAuthConfig> = std::sync::OnceLock::new();

/// User git configuration relevant to authenticating a clone (kodegen `GitConfig`).
#[derive(Debug, Clone, Default)]
struct GitAuthConfig {
    /// `core.sshCommand` — custom SSH program/flags for ssh:// and git@ remotes.
    ssh_command: Option<String>,
    /// `ssh.variant` — SSH program variant gix uses to shape its arguments.
    ssh_variant: Option<String>,
    /// `credential.helper` — credential helper that supplies https usernames/passwords/tokens.
    credential_helper: Option<String>,
}

impl GitAuthConfig {
    /// Read auth-relevant git config via the `git` binary (kodegen `GitConfig::read`).
    fn read() -> Self {
        let mut config = GitAuthConfig::default();
        if !git_available() {
            return config;
        }
        config.ssh_command = git_config_get("core.sshCommand");
        config.ssh_variant = git_config_get("ssh.variant");
        config.credential_helper = git_config_get("credential.helper");
        config
    }

    /// Render as gix in-memory override strings `key=value` (kodegen `to_gix_overrides`).
    fn to_gix_overrides(&self) -> Vec<String> {
        let mut overrides = Vec::new();
        if let Some(v) = &self.ssh_command {
            overrides.push(format!("core.sshCommand={v}"));
        }
        if let Some(v) = &self.ssh_variant {
            overrides.push(format!("ssh.variant={v}"));
        }
        // credential.helper drives https auth; gix invokes the helper during fetch.
        if let Some(v) = &self.credential_helper {
            overrides.push(format!("credential.helper={v}"));
        }
        overrides
    }
}

/// Cached accessor for the user's git auth config (kodegen `get_config`).
fn get_auth_config() -> &'static GitAuthConfig {
    GIT_AUTH_CONFIG.get_or_init(GitAuthConfig::read)
}

/// Whether a `git` binary is callable (kodegen `git_available`).
fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Read a single effective git config value (kodegen `git_config_get`).
fn git_config_get(key: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["config", "--get", key])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Inject the user's git auth config into a fresh `PrepareFetch` (kodegen `configure_clone`).
///
/// Best-effort: when the user has nothing relevant configured the override list is empty and
/// `prepare` is returned untouched, so public and `file://` clones are unaffected.
fn configure_clone(prepare: gix::clone::PrepareFetch) -> gix::clone::PrepareFetch {
    let overrides = get_auth_config().to_gix_overrides();
    if overrides.is_empty() {
        prepare
    } else {
        prepare.with_in_memory_config_overrides(overrides)
    }
}

/// Resolve `reff` (branch/tag/commit) in the local clone at `dir`, materialize its tree over the
/// working copy, and return the resolved commit (hex). No network — object-database access only.
fn checkout_ref(dir: &std::path::Path, reff: &str) -> Result<String, ResourceError> {
    let repo = gix::open(dir).map_err(|e| ResourceError::Git(e.to_string()))?;
    let id = repo
        .rev_parse_single(reff)
        .map_err(|e| ResourceError::Git(format!("ref `{reff}` not found: {e}")))?;
    let commit = id
        .object()
        .map_err(|e| ResourceError::Git(e.to_string()))?
        .try_into_commit()
        .map_err(|e| ResourceError::Git(e.to_string()))?;
    let commit_hex = commit.id().to_hex().to_string();
    let tree = commit
        .tree()
        .map_err(|e| ResourceError::Git(e.to_string()))?;
    materialize_tree(&tree, dir)?;
    Ok(commit_hex)
}

/// Recursively write a git tree's blobs to `dst`, overwriting the working copy so it matches the
/// checked-out ref. Submodule (`Commit`) entries and non-UTF-8 names are skipped; symlinks are
/// written as regular files carrying the link target (sufficient for resource materialization).
fn materialize_tree(tree: &gix::Tree<'_>, dst: &std::path::Path) -> Result<(), ResourceError> {
    use gix::object::tree::EntryKind;
    std::fs::create_dir_all(dst)?;
    for entry in tree.iter() {
        let entry = entry.map_err(|e| ResourceError::Git(e.to_string()))?;
        let Ok(name) = std::str::from_utf8(entry.filename()) else {
            continue;
        };
        let path = dst.join(name);
        match entry.mode().kind() {
            EntryKind::Tree => {
                let object = entry
                    .object()
                    .map_err(|e| ResourceError::Git(e.to_string()))?;
                let subtree = object
                    .try_into_tree()
                    .map_err(|e| ResourceError::Git(e.to_string()))?;
                materialize_tree(&subtree, &path)?;
            }
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                let object = entry
                    .object()
                    .map_err(|e| ResourceError::Git(e.to_string()))?;
                std::fs::write(&path, &object.into_blob().data)?;
            }
            EntryKind::Commit => {}
        }
    }
    Ok(())
}

/// Recursively copy a directory tree (used to materialize a local git repo into the store).
fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> Result<(), ResourceError> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn git_head(dir: &std::path::Path) -> Result<String, ResourceError> {
    let repo = gix::open(dir).map_err(|e| ResourceError::Git(e.to_string()))?;
    git_head_repo(&repo)
}

fn git_head_repo(repo: &gix::Repository) -> Result<String, ResourceError> {
    let commit = repo
        .head_commit()
        .map_err(|e| ResourceError::Git(e.to_string()))?;
    Ok(commit.id().to_hex().to_string())
}
