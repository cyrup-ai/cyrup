//! Per-tool configuration (arch-03 §3.4 `ToolsOptions`).

use crate::truncate::{
    DEFAULT_MAX_BYTES, DEFAULT_MAX_LINES, FIND_MAX_RESULTS, GREP_MAX_MATCHES, LS_MAX_ENTRIES,
};
use std::path::PathBuf;
use std::sync::Arc;

/// The `{command, cwd, env}` an extension may rewrite before `bash` spawns the child
/// (Pi `BashSpawnContext`, bash.ts:133-137). `env` is the set of variable OVERRIDES layered on top
/// of the inherited parent environment (cyrup inherits the parent env by default; Pi materializes
/// the full env). A hook that wants to add/replace a variable pushes/sets it here.
#[derive(Clone, Debug)]
pub struct BashSpawnContext {
    pub command: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Hook to adjust command, cwd, or env before execution (Pi `BashSpawnHook`, bash.ts:139).
pub type BashSpawnHook = Arc<dyn Fn(BashSpawnContext) -> BashSpawnContext + Send + Sync>;

#[derive(Clone, Debug)]
pub struct ReadOpts {
    pub max_lines: usize,
    pub max_bytes: usize,
    /// Whether the active model can consume images (R-03-012 non-vision fallback).
    pub supports_images: bool,
    /// Max image bound (both dimensions) before resize.
    pub max_image_dim: u32,
}

impl Default for ReadOpts {
    fn default() -> Self {
        Self {
            max_lines: DEFAULT_MAX_LINES,
            max_bytes: DEFAULT_MAX_BYTES,
            supports_images: true,
            max_image_dim: 2000,
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
    /// Hook to rewrite `{command, cwd, env}` before the child spawns (Pi `spawnHook`, bash.ts:154).
    pub spawn_hook: Option<BashSpawnHook>,
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
