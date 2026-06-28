//! Cross-process advisory file locks + atomic writes with owner-only permissions
//! (arch-07 §5, R-07-014/015).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use fs4::FileExt;

use crate::error::ConfigError;

/// An RAII cross-process exclusive lock, taken on a sidecar `<path>.lock` file so the lock survives
/// atomic `rename` over the target (the lock inode is never replaced).
pub struct FileLock {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
}

impl FileLock {
    pub fn acquire(target: &Path) -> Result<Self, ConfigError> {
        if let Some(parent) = target.parent() {
            ensure_dir(parent)?;
        }
        let lock_path = lock_path_for(target);
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        FileExt::lock(&file).map_err(|_| ConfigError::Lock { path: lock_path.clone() })?;
        Ok(Self { file, path: lock_path })
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

fn lock_path_for(target: &Path) -> PathBuf {
    let mut name = target.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(".lock");
    match target.parent() {
        Some(p) => p.join(name),
        None => PathBuf::from(name),
    }
}

/// Create a directory (and parents) with owner-only (0700) permissions on unix.
pub fn ensure_dir(dir: &Path) -> Result<(), ConfigError> {
    if dir.as_os_str().is_empty() || dir.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(dir, perms)?;
    }
    Ok(())
}

/// Atomically write `bytes` to `path` via temp-file + rename. When `secret`, the file is created
/// with 0600 permissions and its parent dir as 0700 (R-07-014).
pub fn write_atomic(path: &Path, bytes: &[u8], secret: bool) -> Result<(), ConfigError> {
    let parent = path.parent().filter(|p| !p.as_os_str().is_empty());
    if let Some(parent) = parent {
        ensure_dir(parent)?;
    }
    let file_name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    let mut tmp_name = file_name;
    tmp_name.push(format!(".tmp.{}", std::process::id()));
    let tmp_path = match parent {
        Some(p) => p.join(&tmp_name),
        None => PathBuf::from(&tmp_name),
    };

    {
        let mut opts = OpenOptions::new();
        opts.create(true).write(true).truncate(true);
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp_path)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        #[cfg(unix)]
        if secret {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
    }

    std::fs::rename(&tmp_path, path)?;
    Ok(())
}
