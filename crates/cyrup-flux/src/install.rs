//! Materialise the embedded bundle into a directory the resource loader can scan (FLUX-001) — the
//! port of `code_puppy_core_plugins/flux_bootstrap/installer.py` @v0.0.40.
//!
//! Upstream's design goals (`installer.py:9-20`), each kept here:
//!
//! * **Idempotent** — re-running with the same bundled content is a no-op (`:193-199`).
//! * **Non-destructive** — a file the user hand-edited is backed up to `<name>.bak` (or
//!   `.bak.N`) before the fresh copy lands (`:113-127`, `:211-214`); a pre-existing file the
//!   installer never wrote is preserved in place forever (`:201-209`). "User edited it" versus
//!   "we wrote it last time" is told by a manifest of the SHA-256 hashes installed
//!   (`.flux_bootstrap_manifest.json`, `:139-149`).
//! * **Version-gated** — the tree is walked only when the stored marker
//!   (`.flux_bootstrap_version`, `:152-167`) differs from the current one, so steady-state startup
//!   is one small file read. cyrup's marker is [`bundle_marker`], version PLUS content
//!   fingerprint (see [`crate::bundle::bundle_fingerprint`] for why).
//! * **Fails closed, never fatal** — every error is an `Err` the caller turns into a notice, never
//!   a panic (`:19-20`; `register_callbacks.py:67-68`).
//! * Best-effort **cross-process lock** on `.flux_bootstrap.lock` (`:42-44`, `:244-256`): a
//!   concurrent first-launch install skips rather than races.
//!
//! Every write is a temp file + atomic rename (`:83-91` `_atomic_write_text`, `:94-110`
//! `_write_bytes_preserving_mode`), so a crash mid-install leaves the previous file intact and,
//! because the marker is written LAST (`:220-221`), re-runs the pass next time.
//!
//! Layout differs from upstream by one level: code-puppy copies `bundled/commands/…` to
//! `<config_dir>/commands/…` because its command loader scans the config dir root; cyrup
//! contributes the tree through `ResourcesDiscover` `promptPaths`/`skillPaths` instead
//! (`crates/cyrup-flux/src/extension.rs`), so the payload lands under an extension-owned root
//! (`<agent_dir>/flux/resources/{prompts,skills}`, see [`crate::resources::BundledRoot`]) that no
//! other scanner walks — no double registration, and the dot-file markers never sit in a scanned
//! directory. Mode-bit preservation (`:94-110`) has no counterpart: the bundle is markdown only,
//! and embedded bytes carry no mode.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use fs4::{FileExt, TryLockError};

use crate::bundle::{BundledFile, bundle_fingerprint, bundled_files, sha256_hex};

/// `installer.py:44` — the best-effort cross-process lock file.
pub const LOCK_NAME: &str = ".flux_bootstrap.lock";
/// `installer.py:51` — the marker recording the bundle installed on the last clean pass.
pub const VERSION_MARKER_NAME: &str = ".flux_bootstrap_version";
/// `installer.py:52` — the manifest of SHA-256 hashes the installer wrote.
pub const MANIFEST_NAME: &str = ".flux_bootstrap_manifest.json";

/// The value stored in the version marker: `<crate version>+<bundle fingerprint>`. Upstream stores
/// the code-puppy `__version__` alone (`register_callbacks.py:38-44`); the fingerprint suffix is
/// cyrup's addition (rationale at [`crate::bundle::bundle_fingerprint`]).
#[must_use]
pub fn bundle_marker() -> String {
    format!("{}+{}", env!("CARGO_PKG_VERSION"), bundle_fingerprint())
}

/// What one install pass did — `installer.py:55-72` `InstallReport`. Entries are bundle-relative
/// paths (`backed_up` holds the backup's own relative path, `:214`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstallReport {
    pub installed: Vec<String>,
    pub updated: Vec<String>,
    pub backed_up: Vec<String>,
    pub skipped: Vec<String>,
}

impl InstallReport {
    /// `installer.py:64-66`.
    #[must_use]
    pub fn changed(&self) -> bool {
        !(self.installed.is_empty() && self.updated.is_empty() && self.backed_up.is_empty())
    }

    /// `installer.py:68-72`, verbatim wording.
    #[must_use]
    pub fn summary(&self) -> String {
        format!(
            "{} new, {} updated, {} backed up, {} unchanged",
            self.installed.len(),
            self.updated.len(),
            self.backed_up.len(),
            self.skipped.len()
        )
    }
}

/// The outcome of [`ensure_installed`]. Upstream encodes the first as an early `return` in the
/// caller (`register_callbacks.py:53-54`) and the last as an EMPTY report (`installer.py:253`),
/// indistinguishable from a pass that found nothing to do; naming them lets the caller decide what
/// to tell the user without inspecting a report for emptiness.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    /// The marker already names this bundle; the tree was not walked.
    UpToDate,
    /// A pass ran to completion and the marker now names this bundle.
    Installed(InstallReport),
    /// Another process holds the install lock — it is installing; nothing was written.
    SkippedLocked,
}

/// The per-file decision of `installer.py:186-218`, as a domain enum over the three hashes that
/// determine it. Pure: the shell ([`install_pass`]) reads the filesystem and applies the verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileAction {
    /// No file at `dest` — write it and claim it (`:186-191`).
    Install,
    /// `dest` already holds exactly our payload — leave it, re-claim it (`:193-199`).
    Unchanged,
    /// `dest` differs and is NOT in the manifest — a user-owned file with our name; preserve it in
    /// place forever, never back up, overwrite or claim it (`:207-209`).
    PreserveForeign,
    /// `dest` is ours (in the manifest) and untouched since we wrote it — overwrite freely
    /// (`:211`, hash matches, falls through to `:216-218`).
    Overwrite,
    /// `dest` is ours and the user hand-edited it — keep theirs as a unique `.bak`, then
    /// overwrite (`:211-218`).
    BackupThenOverwrite,
}

/// Decide what to do with one destination file.
///
/// `current` is the SHA-256 of what is on disk (`None` when absent), `recorded` the hash the
/// manifest remembers writing there (`None` when the installer never claimed this path), `payload`
/// the hash of the bytes we ship.
#[must_use]
pub fn decide(current: Option<&str>, recorded: Option<&str>, payload: &str) -> FileAction {
    match (current, recorded) {
        (None, _) => FileAction::Install,
        (Some(cur), _) if cur == payload => FileAction::Unchanged,
        (Some(_), None) => FileAction::PreserveForeign,
        (Some(cur), Some(rec)) if cur == rec => FileAction::Overwrite,
        (Some(_), Some(_)) => FileAction::BackupThenOverwrite,
    }
}

/// `installer.py:152-158` — the marker recorded on the last successful install, `None` when absent
/// or blank.
#[must_use]
pub fn read_installed_marker(root: &Path) -> Option<String> {
    let text = fs::read_to_string(root.join(VERSION_MARKER_NAME)).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// `installer.py:165-167` — true on a fresh install (no marker) or when the bundle changed.
#[must_use]
pub fn needs_install(root: &Path, marker: &str) -> bool {
    read_installed_marker(root).as_deref() != Some(marker)
}

/// `installer.py:83-91` `_atomic_write_text` / `:94-110` — temp file beside `path`, then an
/// atomic rename over it (`os.replace`; `fs::rename` replaces an existing destination file on
/// every supported platform).
fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = tmp_sibling(path);
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

/// `<path>.tmp` beside `path` (`installer.py:89`, `:104`).
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    name.push(".tmp");
    path.with_file_name(name)
}

/// `installer.py:113-127` `_unique_backup_path` — `<name>.bak`, else the first free `.bak.N`.
#[must_use]
pub fn unique_backup_path(dest: &Path) -> PathBuf {
    let base = dest
        .file_name()
        .map(std::ffi::OsStr::to_os_string)
        .unwrap_or_default();
    let mut candidate = base.clone();
    candidate.push(".bak");
    let mut backup = dest.with_file_name(&candidate);
    let mut i = 1u32;
    while backup.exists() {
        let mut numbered = base.clone();
        numbered.push(format!(".bak.{i}"));
        backup = dest.with_file_name(numbered);
        i = i.saturating_add(1);
    }
    backup
}

/// `installer.py:139-144` — a missing or unparsable manifest is an empty one.
fn load_manifest(root: &Path) -> BTreeMap<String, String> {
    fs::read(root.join(MANIFEST_NAME))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// `installer.py:147-149` — sorted keys, two-space indent (a `BTreeMap` through
/// `to_string_pretty` produces exactly that).
fn save_manifest(root: &Path, manifest: &BTreeMap<String, String>) -> io::Result<()> {
    let text = serde_json::to_string_pretty(manifest).map_err(io::Error::other)?;
    atomic_write(&root.join(MANIFEST_NAME), text.as_bytes())
}

/// `installer.py:170-222` `_install_pass` — copy `files` into `root`, unlocked. Separated from
/// [`ensure_installed`] so the lock wrapper does not deepen the copy logic (`:173-174`), and so a
/// test can drive a pass over a chosen bundle.
pub fn install_pass(root: &Path, files: &[BundledFile], marker: &str) -> io::Result<InstallReport> {
    let mut report = InstallReport::default();
    let manifest = load_manifest(root);
    let mut new_manifest = BTreeMap::new();

    for file in files {
        let rel = file.rel;
        let dest = root.join(rel);
        let payload_hash = sha256_hex(file.bytes);
        let current_hash = match fs::read(&dest) {
            Ok(bytes) => Some(sha256_hex(&bytes)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => None,
            Err(e) => return Err(e),
        };
        match decide(
            current_hash.as_deref(),
            manifest.get(rel).map(String::as_str),
            &payload_hash,
        ) {
            FileAction::Install => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                atomic_write(&dest, file.bytes)?;
                report.installed.push(rel.to_string());
                new_manifest.insert(rel.to_string(), payload_hash);
            }
            FileAction::Unchanged => {
                report.skipped.push(rel.to_string());
                new_manifest.insert(rel.to_string(), payload_hash);
            }
            FileAction::PreserveForeign => {
                report.skipped.push(rel.to_string());
            }
            FileAction::Overwrite => {
                atomic_write(&dest, file.bytes)?;
                report.updated.push(rel.to_string());
                new_manifest.insert(rel.to_string(), payload_hash);
            }
            FileAction::BackupThenOverwrite => {
                let backup = unique_backup_path(&dest);
                fs::copy(&dest, &backup)?;
                let backup_rel = backup
                    .strip_prefix(root)
                    .map_or_else(|_| backup.clone(), Path::to_path_buf)
                    .to_string_lossy()
                    .replace('\\', "/");
                report.backed_up.push(backup_rel);
                atomic_write(&dest, file.bytes)?;
                report.updated.push(rel.to_string());
                new_manifest.insert(rel.to_string(), payload_hash);
            }
        }
    }

    save_manifest(root, &new_manifest)?;
    atomic_write(&root.join(VERSION_MARKER_NAME), marker.as_bytes())?;
    Ok(report)
}

/// `installer.py:225-256` `install_bundled_commands` plus the `needs_install` gate its caller
/// applies (`register_callbacks.py:53-54`): make the embedded bundle present under `root`.
///
/// Creates `root`, returns [`InstallOutcome::UpToDate`] without walking when the marker already
/// names this bundle, otherwise takes a non-blocking exclusive lock on [`LOCK_NAME`] — held by
/// another process means [`InstallOutcome::SkippedLocked`] — and runs one [`install_pass`]. The
/// lock releases when the `File` drops (`:256` `os.close` releases the flock).
pub fn ensure_installed(root: &Path) -> io::Result<InstallOutcome> {
    let marker = bundle_marker();
    fs::create_dir_all(root)?;
    if !needs_install(root, &marker) {
        return Ok(InstallOutcome::UpToDate);
    }
    let lock: File = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(root.join(LOCK_NAME))?;
    // `fs4` 1.1 spells the exclusive non-blocking attempt `try_lock` (`LOCK_EX | LOCK_NB`,
    // `installer.py:250`); the fully-qualified form is the one `cyrup-config/src/lock.rs` uses.
    match FileExt::try_lock(&lock) {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(InstallOutcome::SkippedLocked),
        Err(TryLockError::Error(e)) => return Err(e),
    }
    let report = install_pass(root, bundled_files(), &marker);
    let _ = FileExt::unlock(&lock);
    report.map(InstallOutcome::Installed)
}
