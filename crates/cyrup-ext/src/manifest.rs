//! The extension manifest (`extension.json`, arch-08 §4.2 / ADR-0002). JSON — consistent with
//! cyrup-config (JSON-only); there is no `toml` dep in the host. Declares the capabilities a guest
//! requests (granted subject to trust, arch-07/12), its entry point, and the WIT world version.

use crate::error::ExtError;
use std::path::Path;

/// The manifest file name inside an extension directory (arch-08 §4.2).
///
/// EXT-070 — this used to call itself "Pi's analog … the `pi` field of `package.json`
/// (`loader.ts:596`)", and BOTH halves of that sentence were wrong.
///
/// (a) The two files are **disjoint**: they share zero keys. `extension.json` is
/// `{id, version, world, entry, capabilities{fs, exec, net, ui}}` — identity, WIT world version and
/// sandbox grant — and is 100% cyrup-original as a schema, because pi has no per-extension manifest
/// and no capability model at all (a pi extension is a `.ts` file with ambient Node authority). It
/// exists here because of the WASM sandbox (ADR-0002). pi's `interface PiManifest` is
/// `{extensions?, skills?, prompts?, themes?}` — resource PATHS — declared at
/// `pi/packages/coding-agent/src/core/extensions/loader.ts:561-566` @v0.83.0 and read off the `pi`
/// key at `:572-573`.
///
/// (b) Those four pi keys **are** fully ported — just not here. They live in another crate, as
/// `ManifestResources` in `crates/cyrup-resources/src/package/manifest.rs`, which also carries the
/// two cyrup-originals bolted onto them (a fifth `agents` key, and acceptance of a `cyrup`
/// package.json key beside `pi`, with `pi` winning on collision — EXT-063). A reader who finds only
/// this file concludes pi's manifest is unported; the enumeration that filed EXT-070 did exactly
/// that until the second crate turned up.
///
/// (`loader.ts:596` is `const packageJsonPath = path.join(dir, "package.json");` — a path join, not
/// the field read the old citation claimed.)
pub const MANIFEST_FILE: &str = "extension.json";

/// The on-disk `extension.json` (arch-08 §4.2).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    pub id: String,
    pub version: String,
    /// WIT world compatibility, e.g. `cyrup:ext@0.10` (see [`HOST_WORLD`], which is the value a
    /// manifest written today should carry — this example rotted two bumps behind it once already).
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
        Self {
            fs: Vec::new(),
            exec: true,
            net: true,
            ui: true,
        }
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
/// - 0.5 → 0.6 (the same one-batched-bump rule): the `ctx.ui` surface batch. IMPORT RE-SIGNINGS —
///   `ui.set-widget` became pi's three arguments `(key, content-json: option<string>,
///   opts-json)` (EXT-047; upstream `setWidget(key, content, options?)`, `types.ts:170-175`
///   @v0.83.0), and `ui.theme-list` widened from a json array of NAMES to `{name, path}` rows
///   (EXT-021; upstream `getAllThemes(): {name, path}[]`, `types.ts:269`). Both are the
///   fails-to-LINK kind of import change, hence the bump. (Additive IMPORTS in the same batch,
///   which would not have required a bump on their own: `ui.set-working-message` (`types.ts:151`),
///   `ui.set-working-visible` (`:154`), `ui.set-working-indicator` (`:164`),
///   `ui.set-hidden-thinking-label` (`:167`), `ui.theme-get-by-name` (`:272`).) EXPORT ADDITION —
///   `transform-markdown` (EXT-019; pi `MarkdownTransformer`, `extensions/types.ts:1153`
///   @v0.84.1), which would have required the bump on its own, plus its declaring import
///   `registration.register-markdown-transformer` (`:1292`).
/// - 0.6 → 0.7: IMPORT RE-SIGNING — `types.tool-descriptor` gained `constrained-sampling`, which
///   re-signs `registration.register-tool` (PROV-011 / EXT-024; pi
///   `ToolDefinition.constrainedSampling?: false | ConstrainedSamplingConfig`,
///   `extensions/types.ts:463` @v0.83.0, copied onto the runtime tool at
///   `core/tools/tool-definition-wrapper.ts:14`). A 0.6 guest calls `register-tool` with the
///   nine-field record the 0.7 host no longer accepts, so it fails to LINK — the same failure
///   mode as a stale export, hence the bump.
/// - 0.7 → 0.8: the ext-rpc surface-enumeration batch, both directions again. EXPORT RENAMES —
///   `events.on-tool-exec-{start,update,end}` became `on-tool-execution-*`, the only three exports
///   in the 33-event mapping whose names were not a mechanical kebab-case of pi's
///   (`tool_execution_start`/`_update`/`_end`, `extensions/types.ts:1223-1225` @v0.83.0, EXT-069);
///   and `events.on-session-tree`'s parameter became `event-json` (EXT-068 — the payload is pi's
///   four-field `SessionTreeEvent` leaf transition, `types.ts:646-652`, not a tree dump). IMPORT
///   MOVE — `add-autocomplete-provider` left `interface registration` for `interface ui`, which is
///   what puts it behind this file's own `capabilities.ui` grant (EXT-065; upstream declares it
///   inside `ExtensionUIContext`, `types.ts:225`). A 0.7 guest imports it from the old interface
///   and fails to LINK. IMPORT ADDITION — `ui.theme-get-json` (EXT-066), which would not have
///   required a bump on its own — and `world.wit`'s header used to claim it DID, which EXT-061
///   corrected in favour of this entry.
/// - 0.8, still 0.8: IMPORT ADDITION — `ctx-state.get-system-prompt-options` (EXT-061; pi
///   `ctx.getSystemPromptOptions(): BuildSystemPromptOptions` on `ExtensionCommandContext`,
///   `extensions/types.ts:355` @v0.83.0), the last unaccounted-for member of pi's extension API
///   surface. Deliberately NOT a bump, and the reasoning is worth keeping because it was worked out
///   the wrong way round first: [`ExtensionManifest::check_world`] passes when the GUEST's minor is
///   `>=` the host's, so the version gate defends one direction only — an OLD guest against a NEW
///   host. An added import cannot fail that direction at all (the host merely offers an import the
///   guest never calls). The direction it COULD fail — a guest built against a newer world than the
///   host it runs on — is accepted by the gate whatever the numbers are, because a bump raises the
///   host's FLOOR, not its ceiling. So bumping here would refuse every already-built 0.8 guest and
///   prevent nothing. The ABI fingerprint (`build/abi.rs`), not the version, is what stops a STALE
///   cached artifact being served across this edit.
/// - 0.8 -> 0.9: EXPORT RE-SIGNING — `events.render-call` and `events.render-result` each gained a
///   third parameter, `opts-json`, carrying the `(options, theme)` half of upstream's renderer
///   signature (EXT-006; `MessageRenderer` `extensions/types.ts:1213-1217` @v0.84.4,
///   `EntryRenderer` `:1219-1223`, `ToolDefinition.renderCall`/`renderResult` `:491-498`). This is
///   the direction the gate DOES defend: a 0.8 guest exports the two-parameter shape, so without
///   the bump it would pass [`ExtensionManifest::check_world`] and then die inside wasmtime on an
///   opaque link error — the `f777e44` failure mode, verbatim. Nothing else moved in this batch.
/// - 0.9 -> 0.10: EXPORT ADDITION — `events.bash-operations-exec` (DRIFT-004), the guest half of
///   pi's `UserBashEventResult.operations` (`extensions/types.ts:1136-1142` @v0.84.4, the field at
///   `:1139`; the `BashOperations` interface at `core/tools/bash.ts:63-81`, its `exec` signature at
///   `:71-80`). A new export is the same fails-to-LINK direction as a re-signed one — a 0.9 guest
///   exports nothing under that name — so it takes the bump on its own. Its declaring import
///   `registration.register-bash-operations` and the `host-bash` interface carrying pi's two
///   closure-shaped `exec` options (`emit-bash-output` for `onData`, `is-bash-cancelled` for
///   `signal`) are ADDITIVE imports and would not have required a bump on their own.
pub const HOST_WORLD: &str = "cyrup:ext@0.10";

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
