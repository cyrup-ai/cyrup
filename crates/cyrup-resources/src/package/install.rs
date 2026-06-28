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
                if let Some(dir) = self.store.package_dir(scope, id) {
                    if dir.exists() {
                        let _ = std::fs::remove_dir_all(&dir);
                    }
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
            if let Some(reg_path) = self.store.registry_path(scope) {
                if let Ok(reg) = lock::load(&reg_path) {
                    out.extend(reg.packages);
                }
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

/// Best-effort refresh of an installed package. Git: re-resolve HEAD from the local clone (network
/// fetch is fixture-gated / deferred, §12). Path: no-op.
fn refresh(
    pkg: &mut InstalledPackage,
    scope: InstallScope,
    store: &PackageStore,
) -> Result<(), ResourceError> {
    match &pkg.source {
        PackageSource::Path { .. } => Ok(()),
        PackageSource::Oci { .. } => Err(ResourceError::Unsupported),
        PackageSource::Git { .. } => {
            if let Some(dir) = store.package_dir(scope, &pkg.id) {
                if let Ok(commit) = git_head(&dir) {
                    pkg.resolved_commit = Some(commit);
                }
            }
            Ok(())
        }
    }
}

/// Materialize a git package working tree at `dir` and return the resolved HEAD commit (hex).
///
/// **Channel note (§7.6, §12):** network fetch over the wire is fixture-gated — it needs gix's
/// `blocking-network-client` feature, which is off by default to keep print-mode builds lean. The
/// supported, deterministic path is a **local git repo** (a `file://` URL or a filesystem path to a
/// real repo, e.g. a local fixture): its working tree is copied into the store and HEAD is read
/// with `gix` (no network). `_ref_name` checkout is deferred (§12); the cloned HEAD is recorded.
fn git_clone(
    url: &str,
    dir: &std::path::Path,
    _ref_name: Option<String>,
) -> Result<String, ResourceError> {
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let local = url.strip_prefix("file://").unwrap_or(url);
    let src = std::path::Path::new(local);
    if src.is_dir() {
        copy_tree(src, dir)?;
        return git_head(dir);
    }
    Err(ResourceError::Git(format!(
        "network git fetch is fixture-gated (build gix with `blocking-network-client`); \
         use a local repo path/file:// URL: {url}"
    )))
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
