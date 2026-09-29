//! The eight built-in tools (DI-1). Each implements `cyrup_core::Tool`, including its model-facing
//! metadata (`description`/`prompt_snippet`/`prompt_guidelines`) verbatim from Pi.
//!
//! `bash` and `powershell` are ONE type — [`bash::ShellTool`], Pi's `createShellToolDefinition`
//! (bash.ts:338-517) — instantiated from two [`bash::ShellToolConfig`] values. The engine lives in
//! [`bash`] and [`powershell`] holds only its config, mirroring upstream's own file split.

pub mod bash;
pub mod edit;
pub mod edit_diff;
pub mod find;
mod globmatch;
pub mod grep;
pub mod ls;
pub mod powershell;
pub mod read;
mod rgconfig;
pub(crate) use rgconfig::config_path_from_env as rg_config_path_from_env;
pub mod write;

pub use bash::{BASH_CONFIG, ShellTool, ShellToolConfig};
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use powershell::POWERSHELL_CONFIG;
pub use read::ReadTool;
pub use write::WriteTool;

/// Pi's literal `constrainedSampling: { type: "json_schema", strict: "prefer" }`, declared
/// UNCONDITIONALLY by the coding built-ins: `core/tools/read.ts:80`, the shared shell definition
/// `bash.ts:243` (so `powershell` inherits it from the same line), `edit.ts:156` and `write.ts:57`
/// @v0.87.1. It was `constrainedSampling: getExperimentalToolSampling()` at v0.84.2-v0.85.1;
/// CHANGELOG 0.86.0 — *"Enabled strict-prefer JSON-schema sampling by default for built-in read,
/// bash, powershell, edit, and write tools, without requiring PI_EXPERIMENTAL"* — dropped the flag,
/// and `core/experimental.ts` @v0.87.1 no longer exports `getExperimentalToolSampling` at all.
/// `grep`/`find`/`ls` carry no `constrainedSampling` key upstream and so keep the trait default.
///
/// The value is ONE shared `static`, `cyrup_core::prefer_strict_tool_sampling` — see that
/// accessor and its `static` in `cyrup-core/src/constrained_sampling.rs`, which carries the
/// `[CYRUP-DELTA, mechanism only]` for holding a single shared value where pi writes four inline
/// object literals.
pub(crate) fn prefer_strict_tool_sampling() -> Option<&'static cyrup_core::ConstrainedSampling> {
    Some(cyrup_core::prefer_strict_tool_sampling())
}
