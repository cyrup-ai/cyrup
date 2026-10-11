//! The physical file a `settings.json` path writes to, and the mode-keeping atomic save onto it
//! (CFG-112) — the ONE implementation shared by [`super::FileSettingsStore`] and every
//! `cyrup-ext-subagents` settings writer (`discovery/settings_write.rs`), so that all of them lock
//! the SAME sidecar, `<physical target>.lock`, for a symlinked `settings.json` as for a plain one.
//!
//! Before CFG-112 each side had its own answer: the subagents writers resolved the target
//! (SUBA-171) and locked `<target>.lock`, while `FileSettingsStore::with_lock` locked
//! `<link>.lock` and renamed its temp over the link itself. For a symlinked `settings.json` the
//! two took different sidecars (no exclusion between a `/config` toggle and an agent-override
//! save, so one could lose the other's write), and the main writer's save replaced the link with a
//! plain file.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;

/// pi-subagents `resolveSettingsWriteTarget` (`src/shared/settings-file-lease.ts:6-30`
/// @ad11b7ab):
///
/// ```text
/// export function resolveSettingsWriteTarget(filePath: string): string {
///     let targetPath = filePath;
///     for (;;) {
///         try {
///             return fs.realpathSync.native(targetPath);
///         } catch (error) {
///             if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error;
///             // A trailing separator requires a directory; it cannot name a new settings file.
///             if (targetPath.endsWith("/") || targetPath.endsWith(path.sep)) throw error;
///         }
///         // A missing target is allowed only when its physical parent already exists.
///         const parentPath = fs.realpathSync.native(path.dirname(targetPath));
///         const unresolvedPath = path.join(parentPath, path.basename(targetPath));
///         let linkText: string;
///         try {
///             linkText = fs.readlinkSync(unresolvedPath);
///         } catch (error) {
///             if (!(error instanceof Error && "code" in error && error.code === "ENOENT")) throw error;
///             return unresolvedPath;
///         }
///         // Keep link text intact so the filesystem follows directory links before "..".
///         targetPath = path.isAbsolute(linkText) ? linkText : `${parentPath}${path.sep}${linkText}`;
///     }
/// }
/// ```
///
/// The physical file a settings path writes to, following file and directory links, including a
/// dangling file link (whose link text names the file to create). A missing target is allowed only
/// when its physical parent exists; a path ending in a separator cannot name a new settings file
/// and rethrows the `ENOENT`. An atomic rename onto the result lands on the link's TARGET and the
/// link stays in place.
///
/// # Errors
///
/// The `io::Error` of the failing `canonicalize`/`read_link` (an `ENOENT` for a trailing
/// separator or a missing physical parent), or `InvalidInput` for a path that names no file.
pub fn resolve_settings_write_target(file_path: &Path) -> std::io::Result<PathBuf> {
    let mut target = file_path.to_path_buf();
    loop {
        match std::fs::canonicalize(&target) {
            Ok(resolved) => return Ok(resolved),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            Err(e) => {
                let text = target.as_os_str().to_string_lossy();
                if text.ends_with('/') || text.ends_with(std::path::MAIN_SEPARATOR) {
                    return Err(e);
                }
            }
        }
        let parent = match target.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        let parent_path = std::fs::canonicalize(parent)?;
        let Some(base) = target.file_name() else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("settings path '{}' names no file", target.display()),
            ));
        };
        let unresolved = parent_path.join(base);
        let link_text = match std::fs::read_link(&unresolved) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(unresolved),
            Err(e) => return Err(e),
        };
        // Keep the link text intact (no normalization) so the filesystem follows directory links
        // before any `..`, as upstream's string join does.
        target = if link_text.is_absolute() {
            link_text
        } else {
            parent_path.join(link_text)
        };
    }
}

/// Save `bytes` to the PHYSICAL settings file `target` ([`resolve_settings_write_target`])
/// atomically, keeping an existing file's mode and refusing a file the caller may not write.
///
/// Port of pi-subagents' settings writer (`writeSettingsFile`, `src/agents/agents.ts:950-978`
/// @ad11b7ab): an existing file's `stat().mode & 0o7777` is checked for write access (pi
/// `accessSync(W_OK)`) and carried onto the temp before the rename, so a `0600` file stays `0600`
/// and a read-only file is refused instead of replaced; a new file gets the umask default. pi's
/// main `FileSettingsStorage` writes with `writeFileSync(path, next, "utf-8")`
/// (`core/settings-manager.ts:243` @f1b2e77f5), which goes through a link, keeps the mode and
/// fails with `EACCES` on a read-only file — the same three observable outcomes; the rename only
/// adds that an interrupted save leaves the previous file readable.
///
/// # Errors
///
/// [`ConfigError::Io`] naming `target` for the stat, the write-access probe, the temp write or the
/// rename.
pub fn write_settings_target_atomic(target: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let io = |source: std::io::Error| ConfigError::Io {
        path: target.to_path_buf(),
        source,
    };
    let existing_mode = match std::fs::metadata(target) {
        Ok(meta) => Some(file_mode(&meta)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(io(e)),
    };
    if existing_mode.is_some() {
        // pi `fs.accessSync(targetPath, W_OK)`. An open-for-write that neither truncates nor
        // creates: refused exactly when `access(2)` with `W_OK` would be for a regular file.
        std::fs::OpenOptions::new()
            .write(true)
            .open(target)
            .map_err(io)?;
    }
    crate::lock::write_atomic_with_mode(target, bytes, existing_mode)
}

/// `stat().mode & 0o7777` on unix; `0` elsewhere, where `write_atomic_with_mode` ignores it.
#[cfg(unix)]
fn file_mode(meta: &std::fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o7777
}

#[cfg(not(unix))]
fn file_mode(_meta: &std::fs::Metadata) -> u32 {
    0
}
