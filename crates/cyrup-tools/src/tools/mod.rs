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
/// [CYRUP-DELTA, mechanism only] The value is held as a `static` in this crate because
/// [`cyrup_core::Tool::constrained_sampling`] hands out a reference, so it cannot be built per call.
/// Upstream's literal is an inline object on each tool definition; the declaration the model sees is
/// identical either way.
///
/// **NOT a delta — residual drift, tracked:** `cyrup_core::experimental_tool_sampling` /
/// `experimental_tool_sampling_from` (`cyrup-core/src/constrained_sampling.rs:100-123`, re-exported
/// at `cyrup-core/src/lib.rs:27`) were this declaration's flag-gated ancestor, the Rust counterpart
/// of pi's `getExperimentalToolSampling`. Upstream DELETED that function — `git grep -n
/// getExperimentalToolSampling v0.87.1` returns nothing and `core/experimental.ts` @v0.87.1 exports
/// only `areExperimentalFeaturesEnabled` — and after TOOL-046 nothing in this workspace calls
/// cyrup's copy either (`grep -rn experimental_tool_sampling --include=*.rs crates/` finds only that
/// module, its own unit tests, the `lib.rs` re-export and doc mentions). It is dead production API
/// whose `OnceLock` env latch is no longer reachable from any tool, and the faithful port is to
/// remove it together with the stale "the four coding built-ins declare it — as of pi v0.84.2"
/// module header above it. That edit is OUT OF SCOPE for the pass that landed this line, which owned
/// only `cyrup-tools`, `cyrup-core/src/tool.rs` and `cyrup-ext/src/wrapper.rs`; it is recorded here
/// rather than done silently, and the workspace does not publish, so no external caller depends on
/// the symbol.
pub(crate) fn prefer_strict_tool_sampling() -> Option<&'static cyrup_core::ConstrainedSampling> {
    static PREFER_STRICT_TOOL_SAMPLING: cyrup_core::ConstrainedSampling =
        cyrup_core::ConstrainedSampling::Config(
            cyrup_core::ConstrainedSamplingConfig::JsonSchema {
                strict: cyrup_core::StrictSampling::Prefer,
            },
        );
    Some(&PREFER_STRICT_TOOL_SAMPLING)
}
