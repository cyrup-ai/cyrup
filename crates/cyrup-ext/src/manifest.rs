//! The extension manifest (`extension.json`, arch-08 §4.2 / ADR-0002). JSON — consistent with
//! cyrup-config (JSON-only); there is no `toml` dep in the host. Declares the capabilities a guest
//! requests (granted subject to trust, arch-07/12), its entry point, and the WIT world version.

use crate::error::ExtError;
use std::path::Path;

/// The on-disk `extension.json` (arch-08 §4.2).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    pub id: String,
    pub version: String,
    /// WIT world compatibility, e.g. `cyrup:ext@0.1`.
    pub world: String,
    /// Source entry for a Tier-1 build; absent for a prebuilt `.wasm` package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// Capabilities a guest requests (arch-08 §4.2). `net` is never ambient under WASI p2.
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// fs grants, e.g. `["read:.", "write:.cyrup/todo"]`.
    #[serde(default)]
    pub fs: Vec<String>,
    #[serde(default)]
    pub exec: bool,
    #[serde(default)]
    pub net: bool,
    #[serde(default)]
    pub ui: bool,
}

/// The world version this host implements.
pub const HOST_WORLD: &str = "cyrup:ext@0.1";

impl ExtensionManifest {
    /// Parse from JSON bytes.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ExtError> {
        serde_json::from_slice(bytes).map_err(ExtError::from)
    }

    /// Load and parse `extension.json` from an extension directory.
    pub fn load(dir: &Path) -> Result<Self, ExtError> {
        let path = dir.join("extension.json");
        let bytes = std::fs::read(&path)?;
        Self::from_json(&bytes)
    }

    /// Check world-version compatibility (arch-08 §4.1): same MAJOR is required; a MAJOR mismatch
    /// is surfaced as a typed error, never a trap.
    pub fn check_world(&self, host: &str) -> Result<(), ExtError> {
        let major = |w: &str| -> Option<String> {
            // `cyrup:ext@MAJOR.MINOR` -> "MAJOR"
            let after_at = w.split('@').nth(1)?;
            let major = after_at.split('.').next()?;
            Some(major.to_string())
        };
        match (major(&self.world), major(host)) {
            (Some(a), Some(b)) if a == b => Ok(()),
            _ => Err(ExtError::WorldVersion {
                found: self.world.clone(),
                required: host.to_string(),
            }),
        }
    }
}
