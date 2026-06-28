//! `packages.json` registry persistence (arch-09 §4.1). Atomic write (temp + rename).

use std::path::Path;

use crate::error::ResourceError;
use crate::package::InstalledPackages;

/// Load an `InstalledPackages` registry from `path`. A missing file is an empty registry.
pub fn load(path: &Path) -> Result<InstalledPackages, ResourceError> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let reg: InstalledPackages = serde_json::from_str(&text)?;
            Ok(reg)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(InstalledPackages::default()),
        Err(e) => Err(ResourceError::Io(e)),
    }
}

/// Persist `reg` to `path` atomically (write temp, rename). Creates parent dirs.
pub fn save(path: &Path, reg: &InstalledPackages) -> Result<(), ResourceError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(reg)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json.as_bytes())?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}
