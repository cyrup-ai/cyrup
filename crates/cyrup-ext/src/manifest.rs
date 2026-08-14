//! The extension manifest (`extension.json`, arch-08 §4.2 / ADR-0002). JSON — consistent with
//! cyrup-config (JSON-only); there is no `toml` dep in the host. Declares the capabilities a guest
//! requests (granted subject to trust, arch-07/12), its entry point, and the WIT world version.

use crate::error::ExtError;
use std::path::Path;

/// The manifest file name inside an extension directory (arch-08 §4.2). Pi's analog is the
/// `pi` field of `package.json` (`loader.ts:596` @v0.83.0).
pub const MANIFEST_FILE: &str = "extension.json";

/// The on-disk `extension.json` (arch-08 §4.2).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    pub id: String,
    pub version: String,
    /// WIT world compatibility, e.g. `cyrup:ext@0.5` (see [`HOST_WORLD`]).
    pub world: String,
    /// Source entry for a Tier-1 build; absent for a prebuilt `.wasm` package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

/// Capabilities a guest requests (arch-08 §4.2). `net` is never ambient under WASI p2.
///
/// # This is a RESTRICTION the host applies, not a promise the guest keeps (EXT-054)
///
/// Every field defaults to the DENYING value, so a manifest with no `capabilities` block — and the
/// two manifest-synthesis sites in [`crate::loader`] that stand in for a bare `.wasm` artifact —
/// grant nothing at all. The grant crosses into instantiation as **data**
/// ([`crate::ExtensionHost::load_wasm_with_caps`] → `GuestState::with_capabilities`) and is enforced
/// **host-side** in `crates/cyrup-ext/src/host/live.rs`, per ADR-0002's batch-17 instruction
/// ("seeding `GuestState` from the manifest must not introduce a host reference into `GuestState`
/// that the guest can reach — the grant is data, the enforcement is host-side").
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    /// fs grants, e.g. `["read:.", "write:.cyrup/todo"]`. Parsed by
    /// [`Capabilities::parse_fs_grants`]; an empty list denies `ext-fs` outright.
    #[serde(default)]
    pub fs: Vec<String>,
    #[serde(default)]
    pub exec: bool,
    #[serde(default)]
    pub net: bool,
    #[serde(default)]
    pub ui: bool,
}

/// One parsed `capabilities.fs` entry: `"<mode>:<relative-path>"` where `<mode>` is `read` or
/// `write`. A `write` grant implies read on the same subtree (you cannot write what you cannot
/// address), matching the ordinary meaning of the two words in the manifest example
/// `["read:.", "write:.cyrup/todo"]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FsGrant {
    /// The grant's root, RELATIVE to the host's project cwd ([`crate::HostConfig::cwd`]). Never
    /// absolute and never containing `..` — [`Capabilities::parse_fs_grants`] refuses both.
    pub path: std::path::PathBuf,
    /// `true` for `write:` (read+write), `false` for `read:` (read only).
    pub write: bool,
}

impl Capabilities {
    /// The EMPTY grant: nothing at all. Identical to [`Default`], named so the deny-by-default
    /// intent is legible at the call sites that synthesize a manifest (`loader.rs`).
    pub fn none() -> Self {
        Self::default()
    }

    /// The grant the host applies to a component it loads ITSELF, with no `extension.json` to read
    /// — [`crate::ExtensionHost::load_wasm`]. The interactive capabilities are granted because the
    /// CALLER is the host and has already made the decision the manifest otherwise expresses; `fs`
    /// stays EMPTY because `ext-fs` has no root to resolve against without a declared grant, which
    /// is exactly what it had before EXT-054/EXT-055 (`FsCaps::default()`). Every DISCOVERED
    /// extension goes through [`crate::ExtensionHost::load_discovered`] and is capped by its own
    /// manifest instead.
    pub fn host_granted() -> Self {
        Self { fs: Vec::new(), exec: true, net: true, ui: true }
    }

    /// Parse [`Self::fs`] into typed grants, refusing anything that would escape the project root.
    ///
    /// A malformed entry is an ERROR, not a silently-dropped grant: a typo in the manifest that
    /// quietly widened or narrowed the sandbox is the failure mode EXT-054 is about. The load fails
    /// and the operator sees the offending string.
    pub fn parse_fs_grants(&self) -> Result<Vec<FsGrant>, ExtError> {
        self.fs
            .iter()
            .map(|raw| {
                let (mode, rel) = raw.split_once(':').ok_or_else(|| {
                    ExtError::Capability(format!(
                        "fs grant `{raw}` is not `read:<path>` or `write:<path>`"
                    ))
                })?;
                let write = match mode.trim() {
                    "read" => false,
                    "write" => true,
                    other => {
                        return Err(ExtError::Capability(format!(
                            "fs grant `{raw}` has unknown mode `{other}` (expected `read` or `write`)"
                        )));
                    }
                };
                let path = std::path::PathBuf::from(rel.trim());
                if path.is_absolute()
                    || path.components().any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(ExtError::Capability(format!(
                        "fs grant `{raw}` must be a relative path inside the project (no `..`, no absolute path)"
                    )));
                }
                Ok(FsGrant { path, write })
            })
            .collect()
    }
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
/// - 0.4 → 0.5 (ADR-0002's single batched bump; the "anything needing a new export is a member"
///   rule): the payload-parity batch. EXPORT ADDITIONS — `events.on-before-provider-headers`
///   (EXT-009), `events.on-session-info-changed` (EXT-011), `events.prepare-arguments` (EXT-023).
///   EXPORT RE-SIGNINGS — `on-tool-exec-update`/`on-tool-exec-end` gained `name` (and `args-json`
///   on the update, EXT-014); `on-session-start`/`on-session-shutdown`/`on-session-before-switch`/
///   `on-session-before-fork` gained their discriminating fields (EXT-015);
///   `on-resources-discover` gained `cwd`+`reason` (EXT-016); `on-project-trust` gained `cwd`
///   (EXT-043); `on-model-select`/`on-thinking-level-select` gained their sibling fields
///   (EXT-042); and `types.hook-outcome`'s `block` arm became a `block-result` record carrying
///   `terminate` (EXT-049), which re-signs EVERY hook export at once. IMPORT RE-SIGNING —
///   `session.set-label`'s `label` became `option<string>` so it can CLEAR (EXT-046). The
///   `tool-descriptor` record gained `prepare-arguments` + `render-shell` (EXT-023 / EXT-024),
///   which is a re-signing of the `registration.register-tool` import. (Additive IMPORTS in the
///   same batch, which would not have required a bump on their own: `ctx-state.get-cwd`
///   (EXT-044), `ctx-state.is-run-cancelled` + `models.scoped-models` (EXT-045),
///   `bus.unsubscribe` (EXT-050), `provider-stream.on-payload`/`on-response` (EXT-052).)
pub const HOST_WORLD: &str = "cyrup:ext@0.5";

impl ExtensionManifest {
    /// Parse from JSON bytes.
    pub fn from_json(bytes: &[u8]) -> Result<Self, ExtError> {
        serde_json::from_slice(bytes).map_err(ExtError::from)
    }

    /// Load and parse `extension.json` from an extension directory.
    ///
    /// The `Err` is not the last word on the directory: [`crate::loader::discover`] falls back to
    /// the manifest-less "bare `.wasm`" rule, matching Pi's `readPiManifest` -> `null` ->
    /// `index.ts` fall-through (`loader.ts:568-579`, `:594-624` @v0.83.0). It does NOT swallow the
    /// error while doing so — [`crate::loader::discover_with_diagnostics`] reports the fallback and
    /// its two consequences (a different id, an empty grant), which is the same principle
    /// [`Capabilities::parse_fs_grants`] states below: a malformed declaration is never a silent
    /// change of sandbox.
    pub fn load(dir: &Path) -> Result<Self, ExtError> {
        let path = dir.join(MANIFEST_FILE);
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
