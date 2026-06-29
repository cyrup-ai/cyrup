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
    SecurityNotice { message: SECURITY_CAVEAT, source }
}

impl PackageManager {
    pub fn new(store: PackageStore) -> Self {
        Self { store }
    }

    /// Install a package from a source ref (R-09-018). Explicit user action only (R-09-019).
    /// Project scope requires `trusted` (R-09-017).
    pub async fn install(
        &self,
        source: PackageSource,
        scope: InstallScope,
        trusted: bool,
        cancel: CancelToken,
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
            PackageSource::Oci { .. } => return Err(ResourceError::Unsupported),
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
                    ResourceError::Manifest("project package dir unavailable (no project root)".into())
                })?;
                let url = url.clone();
                let ref_name = reff.ref_name().map(str::to_string);
                let commit = tokio::task::spawn_blocking(move || git_clone(&url, &dir, ref_name))
                    .await
                    .map_err(|e| ResourceError::Git(e.to_string()))??;
                Some(commit)
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
            let Some(reg_path) = self.store.registry_path(scope) else { continue };
            let mut reg = lock::load(&reg_path)?;
            if reg.remove(id) {
                lock::save(&reg_path, &reg)?;
                // Best-effort: drop the cloned working tree (not Path installs).
                if let Some(dir) = self.store.package_dir(scope, id)
                    && dir.exists() {
                        let _ = std::fs::remove_dir_all(&dir);
                    }
                removed = true;
            }
        }
        if removed {
            Ok(())
        } else {
            Err(ResourceError::Manifest(format!("package not installed: {id}")))
        }
    }

    /// List installs across both scopes.
    pub fn list(&self) -> Vec<InstalledPackage> {
        let mut out = Vec::new();
        for scope in [InstallScope::Global, InstallScope::Project] {
            if let Some(reg_path) = self.store.registry_path(scope)
                && let Ok(reg) = lock::load(&reg_path) {
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
            let Some(reg_path) = self.store.registry_path(scope) else { continue };
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
            let Some(reg_path) = self.store.registry_path(scope) else { continue };
            let mut reg = lock::load(&reg_path)?;
            if let Some(pkg) = reg.find_mut(id) {
                pkg.disabled.set(&sel, !enabled);
                lock::save(&reg_path, &reg)?;
                return Ok(());
            }
        }
        Err(ResourceError::Manifest(format!("package not installed: {id}")))
    }
}

/// Refresh an installed package. Git: re-clone the source URL (real fetch via `gix`) so the working
/// tree and recorded commit advance with the remote, mirroring Pi `updateGit`; for a bare on-disk
/// source path the local repo is re-copied and re-resolved. If the source is unreachable the update
/// degrades gracefully — the last known commit (or the local clone's HEAD) is retained rather than
/// failing the whole run. Path: no-op.
fn refresh(
    pkg: &mut InstalledPackage,
    scope: InstallScope,
    store: &PackageStore,
) -> Result<(), ResourceError> {
    match &pkg.source {
        PackageSource::Path { .. } => Ok(()),
        PackageSource::Oci { .. } => Err(ResourceError::Unsupported),
        PackageSource::Git { url, reff } => {
            let Some(dir) = store.package_dir(scope, &pkg.id) else { return Ok(()) };
            match git_clone(url, &dir, reff.ref_name().map(str::to_string)) {
                Ok(commit) => pkg.resolved_commit = Some(commit),
                Err(_) => {
                    // Unreachable source: keep going. If a local clone is present, re-read its HEAD.
                    if let Ok(commit) = git_head(&dir) {
                        pkg.resolved_commit = Some(commit);
                    }
                }
            }
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
fn git_clone(
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

    // A fresh clone requires the target dir to be empty/absent; clear any stale tree.
    if dir.exists() {
        let _ = std::fs::remove_dir_all(dir);
    }
    let mut prepare =
        gix::prepare_clone(url, dir).map_err(|e| ResourceError::Git(e.to_string()))?;
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
    let tree = commit.tree().map_err(|e| ResourceError::Git(e.to_string()))?;
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
        let Ok(name) = std::str::from_utf8(entry.filename()) else { continue };
        let path = dst.join(name);
        match entry.mode().kind() {
            EntryKind::Tree => {
                let object =
                    entry.object().map_err(|e| ResourceError::Git(e.to_string()))?;
                let subtree = object
                    .try_into_tree()
                    .map_err(|e| ResourceError::Git(e.to_string()))?;
                materialize_tree(&subtree, &path)?;
            }
            EntryKind::Blob | EntryKind::BlobExecutable | EntryKind::Link => {
                let object =
                    entry.object().map_err(|e| ResourceError::Git(e.to_string()))?;
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
    let commit = repo.head_commit().map_err(|e| ResourceError::Git(e.to_string()))?;
    Ok(commit.id().to_hex().to_string())
}
