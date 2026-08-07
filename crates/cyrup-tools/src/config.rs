//! Per-tool configuration (arch-03 §3.4 `ToolsOptions`).

use crate::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, FIND_MAX_RESULTS, GREP_MAX_MATCHES, LS_MAX_ENTRIES,
};
use std::path::PathBuf;
use std::sync::Arc;

/// The `{command, cwd, env}` an extension may rewrite before `bash` spawns the child
/// (Pi `BashSpawnContext`, bash.ts:150-154). `env` is the set of variable OVERRIDES layered on top
/// of the inherited parent environment (cyrup inherits the parent env by default; Pi materializes
/// the full env with `{...getShellEnv()}`, bash.ts:164). A hook that wants to add/replace a
/// variable pushes/sets it here.
///
/// `env_remove` is the DELETION channel that materialized shape gives Pi for free: `resolveSpawnContext`
/// does five unconditional `delete env.PI_*` (bash.ts:165-170), and a hook can `delete` from the
/// object it receives (docs/extensions.md:2122). Over an inherit-plus-overrides model that has to be
/// explicit, so keys listed here are removed from the child environment BEFORE `env` is applied —
/// meaning a key may legitimately appear in both (delete-then-set, exactly Pi's order).
#[derive(Clone, Debug, Default)]
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

/// The five session-metadata variables `bash` exposes to its child, WITHOUT the vendor prefix
/// (Pi `PI_SESSION_ID` / `PI_SESSION_FILE` / `PI_PROVIDER` / `PI_MODEL` / `PI_REASONING_LEVEL`,
/// bash.ts:165-181, documented at docs/environment-variables.md:19-27).
pub const SESSION_ENV_SUFFIXES: [&str; 5] =
    ["SESSION_ID", "SESSION_FILE", "PROVIDER", "MODEL", "REASONING_LEVEL"];

/// Every fully-qualified key `bash` scrubs from the child environment before repopulating it.
///
/// Pi deletes the five `PI_*` names unconditionally (bash.ts:165-170). cyrup renamed the family to
/// `CYRUP_*`, so it must scrub BOTH: the `CYRUP_*` names it sets itself, and the `PI_*` names,
/// which are exactly the ones Pi deletes and which a pi-flavoured parent (or a script that still
/// reads them) would otherwise see with a stale value. A subagent run is a real re-exec of the
/// `cyrup` binary, so the inheritance path is live.
pub fn session_env_scrub_keys() -> Vec<String> {
    let mut keys = Vec::with_capacity(SESSION_ENV_SUFFIXES.len() * 2);
    for suffix in SESSION_ENV_SUFFIXES {
        keys.push(format!("CYRUP_{suffix}"));
        keys.push(format!("PI_{suffix}"));
    }
    keys
}

/// The live session metadata `bash` publishes to its child (Pi reads the same five values off the
/// per-call `ExtensionContext`, bash.ts:171-181).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionEnvInfo {
    /// `CYRUP_SESSION_ID` (Pi `ctx.sessionManager.getSessionId()`).
    pub session_id: Option<String>,
    /// `CYRUP_SESSION_FILE`; `None` for an ephemeral/in-memory session, which Pi leaves unset
    /// ("unset for ephemeral sessions", docs/environment-variables.md:22).
    pub session_file: Option<PathBuf>,
    /// `CYRUP_PROVIDER` / `CYRUP_MODEL` — Pi sets the pair together, only when a model is selected.
    pub provider: Option<String>,
    pub model: Option<String>,
    /// `CYRUP_REASONING_LEVEL` (`off`/`minimal`/`low`/`medium`/`high`/`xhigh`/`max`).
    pub reasoning_level: Option<String>,
}

/// A shared, mutable handle to [`SessionEnvInfo`].
///
/// The values must be read at SPAWN time, not baked in when the tool is constructed: "The values
/// are resolved when each command starts. Switching models or changing the reasoning level
/// therefore affects the next bash command without restarting Pi"
/// (docs/environment-variables.md:27). Pi gets that for free from its per-call `ExtensionContext`;
/// cyrup's `Tool::execute` has no context argument, so the session layer hands the tool this handle
/// at build time and updates it in place on every `set_model` / `set_thinking_level`.
#[derive(Clone, Debug, Default)]
pub struct SessionEnvHandle(Arc<std::sync::RwLock<SessionEnvInfo>>);

impl SessionEnvHandle {
    pub fn new(info: SessionEnvInfo) -> Self {
        Self(Arc::new(std::sync::RwLock::new(info)))
    }

    /// Snapshot the current metadata (a poisoned lock still yields the last value written).
    pub fn get(&self) -> SessionEnvInfo {
        self.0.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Replace the whole snapshot.
    pub fn set(&self, info: SessionEnvInfo) {
        *self.0.write().unwrap_or_else(|e| e.into_inner()) = info;
    }

    /// Push the active model (Pi's `ctx.model.provider` / `ctx.model.id`).
    pub fn set_model(&self, provider: impl Into<String>, model: impl Into<String>) {
        let mut g = self.0.write().unwrap_or_else(|e| e.into_inner());
        g.provider = Some(provider.into());
        g.model = Some(model.into());
    }

    /// Push the effective reasoning level (Pi's `ctx.thinkingLevel`).
    pub fn set_reasoning_level(&self, level: impl Into<String>) {
        self.0.write().unwrap_or_else(|e| e.into_inner()).reasoning_level = Some(level.into());
    }
}

/// Hook to adjust command, cwd, or env before execution (Pi `BashSpawnHook`, bash.ts:139).
pub type BashSpawnHook = Arc<dyn Fn(BashSpawnContext) -> BashSpawnContext + Send + Sync>;

/// A shared, mutable handle to "can the model that is active RIGHT NOW consume images?".
///
/// Pi answers that question per call, off the `ExtensionContext` it threads into every tool:
/// `getNonVisionImageNote(ctx?.model)` (read.ts:246) over `model.input.includes("image")`
/// (read.ts:87-92). A mid-session `/model` switch therefore changes the very next `read`.
/// cyrup's `Tool::execute` has no context argument — the same gap [`SessionEnvHandle`] closes for
/// `bash` — so the session layer hands `read` this handle at build time and updates it in place on
/// every `set_model`, instead of baking a snapshot into [`ReadOpts::supports_images`].
///
/// The flag is deliberately a plain `bool` and not a modality set: `cyrup-tools` does not depend on
/// `cyrup-provider`, so the caller is the one that evaluates `model.input.contains(&Modality::Image)`.
#[derive(Clone, Debug)]
pub struct ModelVisionHandle(Arc<std::sync::atomic::AtomicBool>);

impl ModelVisionHandle {
    pub fn new(supports_images: bool) -> Self {
        Self(Arc::new(std::sync::atomic::AtomicBool::new(supports_images)))
    }

    /// Read the capability of the currently-selected model.
    pub fn get(&self) -> bool {
        // `Relaxed` is sufficient: this flag carries no happens-before relationship with any other
        // state — a `read` racing an in-flight `/model` switch may legitimately see either model.
        self.0.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Push the capability of a newly-selected model (Pi's `ctx.model` changing under the tool).
    pub fn set(&self, supports_images: bool) {
        self.0.store(supports_images, std::sync::atomic::Ordering::Relaxed);
    }
}

#[derive(Clone, Debug)]
pub struct ReadOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
    /// Static fallback for whether the model can consume images (R-03-012 non-vision fallback).
    /// Consulted ONLY when `model_vision` is `None` — i.e. by embedders and tests that never wire a
    /// session layer. Prefer [`ReadOpts::supports_images_now`] over reading this field directly.
    pub supports_images: bool,
    /// The LIVE capability of the active model, read at execute time; overrides `supports_images`
    /// whenever it is set. `None` (no session layer wired) keeps the static fallback, mirroring how
    /// [`BashOpts::session_env`] treats Pi's `ctx === undefined`. See [`ModelVisionHandle`].
    pub model_vision: Option<ModelVisionHandle>,
    /// Max image bound (both dimensions) before resize.
    pub max_image_dim: u32,
    /// `images.autoResize` (Pi `ReadToolOptions.autoResizeImages`, read.ts:58-60, defaulted at
    /// read.ts:207 with `?? true`). When `false`, `read` skips `resizeImage` entirely and inlines the
    /// NORMALIZED original bytes with no dimension note (image-process.ts's else-branch), so a vision
    /// model sees the full-resolution screenshot.
    ///
    /// Static, not a live handle, on purpose: Pi reads `settingsManager.getImageAutoResize()` once in
    /// `_buildRuntime` and bakes it into the tool definition (agent-session.ts:2553,2564), so a
    /// mid-session toggle only reaches the tool when the runtime is rebuilt. See
    /// [`ModelVisionHandle`] for the cases where Pi genuinely IS per-call.
    pub auto_resize_images: bool,
}

impl ReadOpts {
    /// Resolve image support the way Pi does — from the model active AT CALL TIME (read.ts:246),
    /// not from whatever was selected when the tool was constructed.
    pub fn supports_images_now(&self) -> bool {
        self.model_vision.as_ref().map_or(self.supports_images, ModelVisionHandle::get)
    }
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            supports_images: true,
            model_vision: None,
            max_image_dim: 2000,
            // Pi `options?.autoResizeImages ?? true` (read.ts:207).
            auto_resize_images: true,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct WriteOpts;

#[derive(Clone, Debug, Default)]
pub struct EditOpts;

#[derive(Clone)]
pub struct BashOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
    /// Optional command prefix prepended before the command (R-03-025, arch-07).
    pub command_prefix: Option<String>,
    /// Optional explicit shell path from settings (Pi `shellPath`, bash.ts:152). Resolved per-exec;
    /// a non-existent path yields the `Custom shell path not found: …` error (shell.ts:73).
    pub shell_path: Option<String>,
    /// Managed bin directory prepended to the child `PATH` (Pi `getShellEnv`/`getBinDir`,
    /// shell.ts:122-134). `None` ⇒ inherit the parent `PATH` unchanged.
    pub bin_dir: Option<PathBuf>,
    /// Hook to rewrite `{command, cwd, env}` before the child spawns (Pi `spawnHook`, bash.ts:198).
    pub spawn_hook: Option<BashSpawnHook>,
    /// Expose the current session's metadata to the child as `CYRUP_*` variables (Pi
    /// `exposeSessionEnvironment`, bash.ts:194; `options?.exposeSessionEnvironment ?? true`,
    /// bash.ts:322 — DEFAULT TRUE). Turning it off suppresses the injection AND the prompt
    /// guideline, but never the scrub: Pi deletes the keys before it consults this flag
    /// (bash.ts:165-171).
    pub expose_session_environment: bool,
    /// The live metadata source read at spawn time. `None` (no session layer wired) behaves like
    /// Pi's `ctx === undefined`: nothing is injected, the scrub still happens.
    pub session_env: Option<SessionEnvHandle>,
}

impl Default for BashOpts {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            command_prefix: None,
            shell_path: None,
            bin_dir: None,
            spawn_hook: None,
            // Pi: `options?.exposeSessionEnvironment ?? true` (bash.ts:322).
            expose_session_environment: true,
            session_env: None,
        }
    }
}

impl std::fmt::Debug for BashOpts {
    // Manual: `spawn_hook` is a boxed closure (not `Debug`); render it as a presence marker so
    // `ToolsOptions`/`BashOpts` keep their `Debug` impls.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BashOpts")
            .field("max_lines", &self.max_lines)
            .field("max_bytes", &self.max_bytes)
            .field("command_prefix", &self.command_prefix)
            .field("shell_path", &self.shell_path)
            .field("bin_dir", &self.bin_dir)
            .field("spawn_hook", &self.spawn_hook.as_ref().map(|_| "<hook>"))
            .field("expose_session_environment", &self.expose_session_environment)
            .field("session_env", &self.session_env.as_ref().map(SessionEnvHandle::get))
            .finish()
    }
}

#[derive(Clone, Debug)]
pub struct GrepOpts {
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for GrepOpts {
    fn default() -> Self {
        Self { limit: GREP_MAX_MATCHES, max_bytes: DEFAULT_MAX_BYTES }
    }
}

#[derive(Clone, Debug)]
pub struct FindOpts {
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for FindOpts {
    fn default() -> Self {
        Self { limit: FIND_MAX_RESULTS, max_bytes: DEFAULT_MAX_BYTES }
    }
}

#[derive(Clone, Debug)]
pub struct LsOpts {
    pub limit: usize,
    pub max_bytes: usize,
}

impl Default for LsOpts {
    fn default() -> Self {
        Self { limit: LS_MAX_ENTRIES, max_bytes: DEFAULT_MAX_BYTES }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ToolsOptions {
    pub read: ReadOpts,
    pub write: WriteOpts,
    pub edit: EditOpts,
    pub bash: BashOpts,
    pub grep: GrepOpts,
    pub find: FindOpts,
    pub ls: LsOpts,
}
