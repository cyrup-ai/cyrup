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
pub mod write;

pub use bash::{BASH_CONFIG, ShellTool, ShellToolConfig};
pub use edit::EditTool;
pub use find::FindTool;
pub use grep::GrepTool;
pub use ls::LsTool;
pub use powershell::POWERSHELL_CONFIG;
pub use read::ReadTool;
pub use write::WriteTool;
