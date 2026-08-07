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
    /// WIT world compatibility, e.g. `cyrup:ext@0.3` (see [`HOST_WORLD`]).
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

/// The world version this host implements. Kept in lockstep with the `package cyrup:ext@…` line of
/// BOTH `world.wit` copies — `crates/cyrup-ext/tests/wit_world_sync.rs` enforces that tie.
///
/// # The bump rule (EXT-028)
///
/// **ANY change to an EXPORT — added, removed, or RE-SIGNED — bumps the MINOR.** An export change is
/// breaking for an already-built guest: it either does not have the function at all, or has it at
/// the old signature. Either way [`crate::host::LiveExtension`] fails at instantiation with a raw
/// wasmtime link error rather than a typed [`ExtError::WorldVersion`], because
/// `bindings::Extension::instantiate_async` resolves the world's exports eagerly.
/// [`ExtensionManifest::check_world`] is minor-aware for exactly that reason, and it only helps if
/// this constant actually moves.
///
/// ADDED imports are additive from the guest's point of view and need no bump on their own. A
/// RE-SIGNED or REMOVED import does: the guest's own import list is baked into its component, so it
/// asks the host for a function the host no longer has and fails to link identically to a stale
/// export.
///
/// History:
/// - 0.1 → 0.2 (SEAM-005): ADDED the `events.on-agent-settled` export. (The `ctx-state` /
///   `control.abort` / `control.shutdown` additions in the same batch were IMPORTS.)
/// - 0.2 → 0.3 (EXT-028, for `f777e44`): RE-SIGNED the `events.on-tool-result` export, which gained
///   a trailing `usage-json: option<string>` (Pi `ToolResultEventBase.usage`, types.ts:919-921).
///   `f777e44` changed `world.wit` without touching this constant, so a pre-`f777e44` guest still
///   declaring `cyrup:ext@0.2` passed the gate and then died inside wasmtime. This bump is the fix.
/// - 0.3 → 0.4: RE-SIGNED the `control.compact` IMPORT, which gained `opts-json: string` (Pi
///   `ctx.compact(options?: CompactOptions)`, types.ts:296-300,344). An import re-signing is
///   normally the guest's problem alone — but a guest built against 0.3 imports `compact` at the
///   ZERO-argument signature, which the 0.4 host no longer provides, so it fails to LINK exactly
///   the way a stale export does. (The `ctx-state.get-mode`/`has-ui` additions in the same batch
///   were purely additive imports and would not have required a bump on their own.)
pub const HOST_WORLD: &str = "cyrup:ext@0.4";

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

    /// Check world-version compatibility (arch-08 §4.1): the MAJOR must match AND the guest's MINOR
    /// must be at least the host's. Both mismatches are surfaced as a typed error, never a trap.
    ///
    /// The MINOR rule is what makes an EXPORT change safe to ship. The host's world changes by
    /// adding guest exports (`events.on-agent-settled`, SEAM-005) and by RE-SIGNING them
    /// (`events.on-tool-result` gaining `usage-json`, EXT-028); a guest built against an older MINOR
    /// implements neither shape, so instantiation would fail deep inside wasmtime with an opaque
    /// missing-export link error. Comparing MINOR here turns that into
    /// [`ExtError::WorldVersion`] at manifest-check time, before any bytes are instantiated. A guest
    /// with a HIGHER minor is accepted: it may want imports this host lacks, and that failure is
    /// specific and reportable, whereas the reverse is not.
    pub fn check_world(&self, host: &str) -> Result<(), ExtError> {
        // `cyrup:ext@MAJOR.MINOR[.PATCH]` -> (MAJOR, MINOR); a missing/garbled MINOR reads as 0.
        let parts = |w: &str| -> Option<(String, u32)> {
            let after_at = w.split('@').nth(1)?;
            let mut it = after_at.split('.');
            let major = it.next()?.to_string();
            let minor = it.next().and_then(|m| m.parse::<u32>().ok()).unwrap_or(0);
            Some((major, minor))
        };
        match (parts(&self.world), parts(host)) {
            (Some((gmaj, gmin)), Some((hmaj, hmin))) if gmaj == hmaj && gmin >= hmin => Ok(()),
            _ => Err(ExtError::WorldVersion {
                found: self.world.clone(),
                required: host.to_string(),
            }),
        }
    }
}
