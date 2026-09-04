//! Extension discovery + loading orchestration (arch-08 §6.2; Pi `loader.ts`
//! `discoverAndLoadExtensions`/`loadExtensions`). Scans the three roots — project-local
//! `<cwd>/.cyrup/extensions/`, global `<agentDir>/extensions/`, and explicitly-configured paths —
//! de-duplicates by canonical path, parses each `extension.json` manifest, applies the pre/post-trust
//! split (R-08-002), and collects a [`LoadExtensionsResult`] of loaded ids + per-path errors (the
//! analog of Pi's `LoadExtensionsResult.errors`). The actual component instantiation is performed by
//! [`crate::facade::ExtensionHost`] (feature-gated on `wasm-host`); discovery itself needs no wasm.

use crate::error::ExtError;
use crate::manifest::{ExtensionManifest, HOST_WORLD, MANIFEST_FILE};
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
    /// Extension paths a settings `extensions` array turned off with a `-pattern` — the negative
    /// half of the same arrays the `cyrup config` editor writes (Pi `toggleTopLevelResource`,
    /// `modes/interactive/components/config-selector.ts:532-578`, `arrayKey === "extensions"`).
    ///
    /// Each entry is the candidate path this scanner would otherwise accept: a subdirectory of a
    /// discovery root, or a bare `*.wasm` artifact sitting in one. **The pattern match itself is not
    /// done here** — it lives in `cyrup-resources`
    /// (`discovery::scan_loose_extension_root` + `package::manifest::is_enabled_by_overrides`),
    /// which owns the settings-array semantics for all four resource kinds; this crate only honours
    /// the resolved verdict, so the extension `cyrup config` disabled is not loaded on the next run.
    ///
    /// [`Default`] is empty, i.e. today's behaviour for every caller that does not resolve settings.
    pub disabled: Vec<PathBuf>,
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
    /// `false` for the two records Pi's `errors[]` does not contain at all:
    ///
    /// 1. [`crate::ExtError::Untrusted`]. cyrup applies the project-trust gate *inside* the load
    ///    (`load_discovered`, R-ARCH-EXT-017) and records the skip in this same vector, whereas Pi
    ///    filters untrusted project resources out **before** `loadExtensions` runs, so an untrusted
    ///    project-local extension never reaches Pi's `errors[]`. Treating it as a load failure would
    ///    make merely opening an untrusted project a fatal startup — the exact opposite of the trust
    ///    gate's intent.
    /// 2. An unparseable `extension.json` (see [`discover_with_diagnostics`]). Pi's manifest reader
    ///    `readPiManifest` swallows a malformed `package.json` outright — `catch { return null }`,
    ///    `loader.ts:568-579` @v0.83.0 — and `resolveExtensionEntries` (`loader.ts:594-624`) then
    ///    falls through to the `index.ts`/`index.js` convention, so the directory still loads and
    ///    startup still continues. Making it fatal here would abort a startup Pi completes.
    ///
    /// Both are still reported in the `[Extension issues]` panel.
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
///
/// Discovery diagnostics are dropped; see [`discover_with_diagnostics`] to keep them.
pub fn discover(roots: &DiscoveryRoots) -> Vec<DiscoveredExtension> {
    discover_with_diagnostics(roots).0
}

/// [`discover`], additionally returning the non-fatal diagnostics discovery produced — today, one
/// per directory whose `extension.json` exists but could not be read or parsed.
///
/// # Why this exists (the silent-fallback hole)
///
/// [`push_dir`] falls back to the manifest-less "bare `.wasm`" rule when [`ExtensionManifest::load`]
/// fails, which is Pi's own shape: `readPiManifest` returns `null` on a malformed `package.json`
/// (`loader.ts:568-579` @v0.83.0) and `resolveExtensionEntries` then falls through to the
/// `index.ts`/`index.js` convention (`loader.ts:594-624`), or returns `null` and the subdirectory
/// contributes nothing (`discoverExtensionsInDir`, `loader.ts:636-668`).
///
/// Pi can afford that silence: its manifest is a *pointer list* (`pi.extensions`), so falling back
/// to `index.ts` yields the same extension, at the same path-derived identity, with the same (total)
/// privileges. cyrup's `extension.json` also carries the **id** and the **capability grant**, so the
/// same fallback silently produces a DIFFERENT extension — id from the artifact stem — holding
/// [`crate::Capabilities::none`]. The author whose manifest has a trailing comma gets a nameless,
/// powerless extension and no message, which also contradicts `manifest.rs`'s own stated rule that a
/// malformed entry is an error rather than a silently-dropped grant.
///
/// So the LOAD OUTCOME stays Pi's (fall back, keep going, do not abort startup — hence
/// [`LoadError::fatal`] `false`) and only the message is added, because Pi has no capability model
/// to report on. A directory with NO `extension.json` at all is the plain manifest-less convention
/// and produces no diagnostic, exactly as a directory with no `package.json` does in Pi.
pub fn discover_with_diagnostics(
    roots: &DiscoveryRoots,
) -> (Vec<DiscoveredExtension>, Vec<LoadError>) {
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut out: Vec<DiscoveredExtension> = Vec::new();
    let mut diags: Vec<LoadError> = Vec::new();
    // Canonicalized with the SAME fallible-then-verbatim rule the `seen` dedup key uses
    // (`push_dir`/`push_file`), so the two compare apples to apples.
    let disabled: HashSet<PathBuf> = roots.disabled.iter().map(|p| dedup_key(p)).collect();

    if let Some(cwd) = &roots.project_cwd {
        let dir = cwd.join(PROJECT_CONFIG_DIR).join(EXTENSIONS_SUBDIR);
        scan_dir(
            &dir,
            ExtOrigin::Project,
            &disabled,
            &mut seen,
            &mut out,
            &mut diags,
        );
    }
    if let Some(agent) = &roots.agent_dir {
        let dir = agent.join(EXTENSIONS_SUBDIR);
        scan_dir(
            &dir,
            ExtOrigin::Global,
            &disabled,
            &mut seen,
            &mut out,
            &mut diags,
        );
    }
    for p in &roots.configured {
        // A configured path may be a bare prebuilt `.wasm` artifact, a single extension dir, or a
        // directory of extensions. Pi takes a non-directory configured path verbatim
        // (`loader.ts:704-717` — the `addPaths([resolved])` fall-through after the `isDirectory()`
        // branch), so a directly-named artifact must not be dropped.
        if is_component_file(p) {
            push_file(p, ExtOrigin::Configured, &mut seen, &mut out);
        } else if is_extension_dir(p) {
            push_dir(p, ExtOrigin::Configured, &mut seen, &mut out, &mut diags);
        } else {
            // EXT-033, the DIAGNOSTIC half. A CONFIGURED path — a `-e <path>` the user typed — that
            // resolves to nothing used to fall into `scan_dir`, whose first statement is
            // `let Ok(rd) = read_dir(dir) else { return };`: a silent return producing neither a
            // `loaded` entry nor an `errors` entry, so a typo'd `-e` was indistinguishable from a
            // correct one and the author's only symptom was that their tools and commands were
            // absent. This is also the documented escape hatch under `--no-extensions`, i.e. the
            // path a user is most likely to reach for.
            //
            // pi guards the same three shapes and surfaces the miss: `fs.existsSync(resolved) &&
            // fs.statSync(resolved).isDirectory()` decides the directory branch and anything else
            // falls through to `addPaths([resolved])`
            // (`pi/packages/coding-agent/src/core/extensions/loader.ts:704-717` @v0.83.0), which
            // then reports the failure as a per-path `LoadExtensionsResult.errors` entry.
            //
            // Non-fatal, matching pi: a bad `-e` does not abort startup, it is reported. The two
            // DISCOVERY roots (project / global) are deliberately NOT guarded this way — a missing
            // `.cyrup/extensions` directory is the ordinary case, not a user error, and pi says
            // nothing about it either.
            let before = out.len();
            scan_dir(
                p,
                ExtOrigin::Configured,
                &disabled,
                &mut seen,
                &mut out,
                &mut diags,
            );
            if out.len() == before {
                diags.push(LoadError {
                    path: p.clone(),
                    fatal: false,
                    error: if p.exists() {
                        "configured extension path is neither a `.wasm` component, an extension \
                         directory (one holding `extension.json` or a `*.wasm`), nor a directory \
                         containing any"
                            .into()
                    } else {
                        "configured extension path does not exist".into()
                    },
                });
            }
        }
    }
    (out, diags)
}

/// True iff `dir` directly holds an extension (an `extension.json` or a `*.wasm` component).
fn is_extension_dir(dir: &Path) -> bool {
    dir.join(MANIFEST_FILE).is_file() || first_wasm(dir).is_some()
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

/// True iff `path` is a bare prebuilt component artifact (a plain `*.wasm` file). `is_file()`
/// follows symlinks, so a symlinked artifact counts — matching Pi's `entry.isFile() ||
/// entry.isSymbolicLink()` (loader.ts:649-650).
fn is_component_file(path: &Path) -> bool {
    path.extension().map(|x| x == "wasm").unwrap_or(false) && path.is_file()
}

/// Scan one root directory for extensions (one level, no recursion — Pi rule). Two entry shapes are
/// accepted, mirroring Pi's `discoverExtensionsInDir` (loader.ts:628-666): rule 1 "Direct files" —
/// a bare artifact sitting straight in the root (`extensions/mytool.wasm`, the analog of Pi's
/// `extensions/*.ts`) — and rules 2/3, a subdirectory holding a manifest or an artifact.
fn scan_dir(
    dir: &Path,
    origin: ExtOrigin,
    disabled: &HashSet<PathBuf>,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<DiscoveredExtension>,
    diags: &mut Vec<LoadError>,
) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort(); // deterministic order (R-08-004)
    for path in entries {
        // A settings `-pattern` disable (`DiscoveryRoots::disabled`) removes the candidate before
        // any manifest is read, so a disabled extension contributes neither an instance nor a
        // diagnostic — the same silence a not-present one produces.
        if disabled.contains(&dedup_key(&path)) {
            continue;
        }
        if path.is_dir() {
            if is_extension_dir(&path) {
                push_dir(&path, origin, seen, out, diags);
            }
        } else if is_component_file(&path) {
            push_file(&path, origin, seen, out);
        }
    }
}

/// The identity one discovered path is compared by: its canonical form, falling back to the path
/// verbatim when it cannot be canonicalized (a broken symlink, a missing parent). Shared by the
/// `seen` de-dup and the `disabled` filter so both key the same way.
fn dedup_key(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Push a bare prebuilt `.wasm` artifact that lives directly in a discovery root (Pi's "direct
/// file" rule). There is no `extension.json` to read beside it — the root is shared with every
/// other entry — so the manifest is synthesized exactly as [`push_dir`] does for a manifest-less
/// extension dir: id from the artifact stem, host world, no `entry` (nothing to build; the artifact
/// IS the component). The EXT-028 caveat there applies here too — claiming [`HOST_WORLD`] makes
/// `check_world` a tautology, so a stale artifact surfaces as a wasmtime link error instead.
///
/// `dir` is the containing root, used only to resolve a manifest `entry` for a Tier-1 build, which
/// this path never has. The dedup key is therefore the ARTIFACT path, not the root, so several bare
/// artifacts in one root are distinct extensions rather than collapsing into one.
fn push_file(
    file: &Path,
    origin: ExtOrigin,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<DiscoveredExtension>,
) {
    let key = dedup_key(file);
    if !seen.insert(key) {
        return; // de-dup (Pi `seen` set)
    }
    let id = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("extension")
        .to_string();
    let manifest = ExtensionManifest {
        id,
        version: "0.0.0".into(),
        world: HOST_WORLD.into(),
        entry: None,
        // EXT-054, deny-by-default: a bare artifact ships no `extension.json`, so it DECLARED
        // nothing and is granted nothing. Synthesizing a permissive grant here would reopen the
        // bypass from the other end — the artifact whose capabilities nobody can read is exactly
        // the one that must not receive `exec`/`net`/`ui`/`fs`. Ship an `extension.json` to ask.
        capabilities: crate::manifest::Capabilities::none(),
    };
    out.push(DiscoveredExtension {
        dir: file.parent().map(Path::to_path_buf).unwrap_or_default(),
        manifest,
        wasm: Some(file.to_path_buf()),
        origin,
    });
}

/// Parse the manifest for one extension dir and push it (deduplicated).
///
/// A dir with NO `extension.json` takes the manifest-less "bare `.wasm`" rule silently — that is
/// Pi's `index.ts` convention (`loader.ts:594-624`) and needs no comment. A dir whose
/// `extension.json` EXISTS but does not read/parse takes the same fallback (Pi's `readPiManifest`
/// `catch { return null }`, `loader.ts:568-579`) and additionally records a non-fatal diagnostic in
/// `diags`, because unlike Pi's pointer-list manifest cyrup's carries the id and the capability
/// grant — see [`discover_with_diagnostics`] for the full parity argument.
fn push_dir(
    dir: &Path,
    origin: ExtOrigin,
    seen: &mut HashSet<PathBuf>,
    out: &mut Vec<DiscoveredExtension>,
    diags: &mut Vec<LoadError>,
) {
    let key = dedup_key(dir);
    if !seen.insert(key) {
        return; // de-dup (Pi `seen` set)
    }
    let wasm = first_wasm(dir);
    let manifest_path = dir.join(MANIFEST_FILE);
    let manifest =
        match ExtensionManifest::load(dir) {
            Ok(m) => m,
            Err(e) => {
                // Synthesize a minimal manifest for a bare prebuilt `.wasm` (Pi's "direct file" rule):
                // id from the artifact/dir stem, host world. No manifest + no wasm => skip.
                //
                // EXT-028 caveat: claiming [`HOST_WORLD`] makes `check_world` a tautology for this path,
                // so a prebuilt artifact built against an older world is NOT caught by the version gate
                // and still surfaces as a wasmtime link error at instantiation. There is nothing to
                // check — the bytes carry no declared world — and refusing every manifest-less `.wasm`
                // would drop Pi's direct-file rule. Ship an `extension.json` to get the typed error.
                let id = wasm
                    .as_deref()
                    .and_then(|w| w.file_stem())
                    .and_then(|s| s.to_str())
                    .or_else(|| dir.file_name().and_then(|s| s.to_str()))
                    .unwrap_or("extension")
                    .to_string();
                // The manifest is only "absent" if the file is not there; an existing-but-broken one is
                // the operator-visible case. `is_file()` also catches an unreadable file (permissions),
                // which is just as invisible to its author as a syntax error.
                if manifest_path.is_file() {
                    diags.push(LoadError {
                    path: dir.to_path_buf(),
                    // Pi keeps loading and does not abort startup on a malformed manifest.
                    fatal: false,
                    error: match &wasm {
                        Some(w) => format!(
                            "{MANIFEST_FILE} could not be read: {e}; falling back to the \
                             manifest-less rule — `{}` loads as extension `{id}` with NO declared \
                             capabilities",
                            w.file_name().and_then(|s| s.to_str()).unwrap_or("<artifact>"),
                        ),
                        None => format!(
                            "{MANIFEST_FILE} could not be read: {e}; the directory has no prebuilt \
                             .wasm to fall back to and was skipped"
                        ),
                    },
                });
                }
                if wasm.is_none() {
                    return;
                }
                ExtensionManifest {
                    id,
                    version: "0.0.0".into(),
                    world: HOST_WORLD.into(),
                    entry: None,
                    // EXT-054, deny-by-default — see `push_file`'s note: no manifest, no grant.
                    capabilities: crate::manifest::Capabilities::none(),
                }
            }
        };
    out.push(DiscoveredExtension {
        dir: dir.to_path_buf(),
        manifest,
        wasm,
        origin,
    });
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
