//! `powershell` — Pi's second built-in shell tool (`core/tools/powershell.ts`).
//!
//! There is no execution logic here and there must never be any. Everything except the values below
//! is [`super::bash::ShellTool`], exactly as upstream's `createPowerShellToolDefinition` is
//! `createShellToolDefinition` with a different `ShellToolConfig` (powershell.ts:49-57) and
//! `powershell.ts` imports its entire engine from `bash.ts`.

use super::bash::{ShellTool, ShellToolConfig};
use crate::config::{BashOpts, PowerShellOpts};
use crate::ops::{ProcOps, ShellConfig};
use cyrup_core::ToolError;
use std::path::PathBuf;
use std::sync::Arc;

/// `UTF8_OUTPUT_PREFIX` (powershell.ts:16) — verbatim, INCLUDING the trailing newline that
/// separates it from the model's command.
const UTF8_OUTPUT_PREFIX: &str =
    "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";

/// The `shellPath` setting names a BASH. Pi's `createLocalPowerShellOperations()` takes no options
/// at all (powershell.ts:32-33) and `PowerShellToolOptions` omits `shellPath` (powershell.ts:29-30),
/// so this resolver drops the argument rather than letting a bash path steer PowerShell.
fn resolve_powershell_ignoring_shell_path(
    _shell_path: Option<&str>,
) -> Result<ShellConfig, ToolError> {
    ShellConfig::resolve_powershell()
}

/// Pi's `powershellToolConfig` (powershell.ts:39-47).
pub static POWERSHELL_CONFIG: ShellToolConfig = ShellToolConfig {
    name: "powershell",
    label: "powershell",
    shell_name: "PowerShell",
    // v0.84.3 `bashSchema` (bash.ts:43) — the tag `powershell` exists at. See the
    // `command_description` CYRUP-DELTA on `ShellToolConfig`.
    command_description: "Shell command to execute",
    // powershell.ts:19.
    prompt_snippet: "Execute PowerShell commands",
    // powershell.ts:20, with the same `PI_*` → `CYRUP_*` divergence the bash guideline documents:
    // this sentence names the variables THIS tool injects into its own child, and cyrup injects
    // `CYRUP_*` while scrubbing `PI_*` unconditionally.
    prompt_guidelines: &[
        "You can inspect CYRUP_* environment variables for current model and session details.",
    ],
    temp_file_prefix: "cyrup-powershell",
    command_preamble: Some(UTF8_OUTPUT_PREFIX),
    resolve_shell: resolve_powershell_ignoring_shell_path,
};

impl ShellTool {
    /// Pi's `createPowerShellToolDefinition` (powershell.ts:49-57).
    pub fn powershell(proc: Arc<dyn ProcOps>, cwd: PathBuf, opts: PowerShellOpts) -> Self {
        Self::new(&POWERSHELL_CONFIG, proc, cwd, BashOpts::from(opts))
    }
}
