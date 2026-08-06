//! Extension discovery + loading orchestration (arch-08 §6.2; Pi `loader.ts`
//! `discoverAndLoadExtensions`/`loadExtensions`). Scans the three roots — project-local
//! `<cwd>/.cyrup/extensions/`, global `<agentDir>/extensions/`, and explicitly-configured paths —
//! de-duplicates by canonical path, parses each `extension.json` manifest, applies the pre/post-trust
//! split (R-08-002), and collects a [`LoadExtensionsResult`] of loaded ids + per-path errors (the
//! analog of Pi's `LoadExtensionsResult.errors`). The actual component instantiation is performed by
//! [`crate::facade::ExtensionHost`] (feature-gated on `wasm-host`); discovery itself needs no wasm.

use crate::error::ExtError;
use crate::manifest::{ExtensionManifest, HOST_WORLD};
use cyrup_core::ExtensionId;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The project config dir (matches cyrup-config: `<cwd>/.cyrup`).
pub const PROJECT_CONFIG_DIR: &str = ".cyrup";
/// The extensions subdirectory under a root.
pub const EXTENSIONS_SUBDIR: &str = "extensions";

/// Where a discovered extension came from — drives the trust split (R-08-002).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtOrigin {
    /// Project-local (`<cwd>/.cyrup/extensions/`): loaded only AFTER the project is trusted (post-trust).
    Project,
    /// Global (`<agentDir>/extensions/`): pre-trust (always eligible).
    Global,
    /// Explicitly configured (CLI `--extension` etc.): pre-trust.
    Configured,
}

impl ExtOrigin {
    /// Pre-trust origins (global + configured/CLI) load regardless of project trust; the project
    /// origin is post-trust (R-08-002).
    pub fn is_pre_trust(&self) -> bool {
        !matches!(self, ExtOrigin::Project)
    }
}

/// A single discovered extension: its directory, its manifest, the prebuilt `.wasm` artifact (if
/// present), and its origin.
#[derive(Clone, Debug)]
pub struct DiscoveredExtension {
    pub dir: PathBuf,
    pub manifest: ExtensionManifest,
    /// A prebuilt component artifact in the dir, if any; otherwise the manifest `entry` drives a
    /// Tier-1 `cargo build` (R-08-031).
    pub wasm: Option<PathBuf>,
    pub origin: ExtOrigin,
}

impl DiscoveredExtension {
    /// The extension id (manifest `id`).
    pub fn id(&self) -> ExtensionId {
        ExtensionId::from(self.manifest.id.as_str())
    }

    /// Whether this extension is eligible to load given the project trust decision (R-08-002).
    pub fn is_trusted(&self, project_trusted: bool) -> bool {
        self.origin.is_pre_trust() || project_trusted
    }
}

/// The three discovery roots (Pi `discoverAndLoadExtensions` inputs).
#[derive(Clone, Debug, Default)]
pub struct DiscoveryRoots {
    /// The project working dir; `<cwd>/.cyrup/extensions/` is scanned (post-trust).
    pub project_cwd: Option<PathBuf>,
    /// The agent (global) dir; `<agentDir>/extensions/` is scanned (pre-trust).
    pub agent_dir: Option<PathBuf>,
    /// Explicitly-configured paths (pre-trust); each may be an extension dir or a dir of extensions.
    pub configured: Vec<PathBuf>,
}

/// A per-path load failure (Pi `LoadExtensionsResult.errors[]`).
#[derive(Clone, Debug)]
pub struct LoadError {
    pub path: PathBuf,
    pub error: String,
    /// Whether this failure is one Pi would also have recorded — i.e. a genuine load fault, which
    /// Pi's bin turns into `Failed to load extension "<path>": <err>` and `process.exit(1)`
    /// (main.ts:735-738, :843-849).
    ///
    /// `false` for [`crate::ExtError::Untrusted`] ONLY: cyrup applies the project-trust gate
    /// *inside* the load (`load_discovered`, R-ARCH-EXT-017) and records the skip in this same
    /// vector, whereas Pi filters untrusted project resources out **before** `loadExtensions` runs,
    /// so an untrusted project-local extension never reaches Pi's `errors[]` at all. Treating it as
    /// a load failure would make merely opening an untrusted project a fatal startup — the exact
    /// opposite of the trust gate's intent. It is still reported in the `[Extension issues]` panel.
    pub fatal: bool,
}

/// The aggregate result of a discover+load pass (Pi `LoadExtensionsResult`).
#[derive(Clone, Debug, Default)]
pub struct LoadExtensionsResult {
    pub loaded: Vec<ExtensionId>,
    pub errors: Vec<LoadError>,
}

impl LoadExtensionsResult {
    pub fn is_empty(&self) -> bool {
        self.loaded.is_empty() && self.errors.is_empty()
    }
}

/// Discover extensions across the three roots, de-duplicated by canonical directory path
/// (Pi `discoverAndLoadExtensions`). Project root first, then global, then configured — first
/// occurrence of a path wins (load-order determinism, R-08-004).
pub fn discover(roots: &DiscoveryRoots) -> Vec<DiscoveredExtension> {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<DiscoveredExtension> = Vec::new();

    if let Some(cwd) = &roots.project_cwd {
        let dir = cwd.join(PROJECT_CONFIG_DIR).join(EXTENSIONS_SUBDIR);
        scan_dir(&dir, ExtOrigin::Project, &mut seen, &mut out);
    }
    if let Some(agent) = &roots.agent_dir {
        let dir = agent.join(EXTENSIONS_SUBDIR);
        scan_dir(&dir, ExtOrigin::Global, &mut seen, &mut out);
    }
    for p in &roots.configured {
        // A configured path may be a single extension dir, or a directory of extensions.
        if is_extension_dir(p) {
            push_dir(p, ExtOrigin::Configured, &mut seen, &mut out);
        } else {
            scan_dir(p, ExtOrigin::Configured, &mut seen, &mut out);
        }
    }
    out
}

/// True iff `dir` directly holds an extension (an `extension.json` or a `*.wasm` component).
fn is_extension_dir(dir: &Path) -> bool {
    dir.join("extension.json").is_file() || first_wasm(dir).is_some()
}

/// Find the first `*.wasm` artifact in `dir` (a prebuilt component).
fn first_wasm(dir: &Path) -> Option<PathBuf> {
    let rd = std::fs::read_dir(dir).ok()?;
    let mut wasms: Vec<PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "wasm").unwrap_or(false))
        .collect();
    wasms.sort();
    wasms.into_iter().next()
}

/// Scan one root directory for extension subdirectories (one level, no recursion — Pi rule).
fn scan_dir(
    dir: &Path,
    origin: ExtOrigin,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<DiscoveredExtension>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort(); // deterministic order (R-08-004)
    for path in entries {
        if path.is_dir() && is_extension_dir(&path) {
            push_dir(&path, origin, seen, out);
        }
    }
}

/// Parse the manifest for one extension dir and push it (deduplicated). A missing/invalid manifest
/// is skipped during discovery; the per-path error surfaces at load time so callers can report it.
fn push_dir(
    dir: &Path,
    origin: ExtOrigin,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<DiscoveredExtension>,
) {
    let key = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !seen.insert(key) {
        return; // de-dup (Pi `seen` set)
    }
    let wasm = first_wasm(dir);
    let manifest = match ExtensionManifest::load(dir) {
        Ok(m) => m,
        Err(_) => {
            // Synthesize a minimal manifest for a bare prebuilt `.wasm` (Pi's "direct file" rule):
            // id from the artifact/dir stem, host world. No manifest + no wasm => skip.
            let Some(w) = &wasm else { return };
            let id = w
                .file_stem()
                .and_then(|s| s.to_str())
                .or_else(|| dir.file_name().and_then(|s| s.to_str()))
                .unwrap_or("extension")
                .to_string();
            ExtensionManifest {
                id,
                version: "0.0.0".into(),
                world: HOST_WORLD.into(),
                entry: None,
                capabilities: Default::default(),
            }
        }
    };
    out.push(DiscoveredExtension { dir: dir.to_path_buf(), manifest, wasm, origin });
}

/// Resolve the component bytes for a discovered extension: a prebuilt `.wasm` is read directly; an
/// absent artifact with a manifest `entry` is built via the Tier-1 `cargo build` loop (R-08-031).
pub fn resolve_component_bytes(disc: &DiscoveredExtension) -> Result<Vec<u8>, ExtError> {
    if let Some(w) = &disc.wasm {
        return std::fs::read(w).map_err(ExtError::from);
    }
    match &disc.manifest.entry {
        Some(entry) => {
            let crate_dir = disc.dir.join(entry);
            crate::build::build_component(&crate_dir)
        }
        None => Err(ExtError::Component(format!(
            "extension `{}` has no prebuilt .wasm and no manifest `entry` to build",
            disc.manifest.id
        ))),
    }
}
