//! cyrup-tools — built-in tools and the operations seam (arch-03; conformance: func-03).
//!
//! Ships the deliberately minimal default tool set — `read`, `write`, `edit`, `bash`, `grep`,
//! `find`, `ls` (DI-1) — as native Rust, plus the cross-cutting infrastructure they share: the
//! two-limit truncation model ([`truncate`]), path handling ([`path`]), per-file mutation locking
//! ([`lock`]), the canonical [`FsOps`]/[`ProcOps`] operations seam ([`ops`]) consumed by isolation
//! (arch-12), the bash streaming engine with process-tree kill ([`output`]), in-process
//! gitignore-aware search/glob (`grep`/`find`), and the [`ToolRegistry`] with extension override
//! and availability controls.
//!
//! The `Tool` trait and `ToolError` are reused from `cyrup-core` (arch-00 §3.4) — never redefined.
//! Failure is signaled by `Err`, mapped to `isError:true` by the runtime (R-03-038). The only
//! `unsafe` in the crate is the isolated unix process-group code in [`ops::local`].
#![deny(unsafe_code)]

pub mod config;
pub mod details;
pub(crate) mod error;
pub mod isolation;
pub mod lock;
pub mod ops;
pub mod output;
pub mod path;
pub mod registry;
pub mod tools;
pub mod truncate;

/// The tool failure type is owned by `cyrup-core` (arch-03 §8). Re-exported so built-in tools and
/// callers refer to a single type.
pub use cyrup_core::ToolError;

pub use config::{
    BashOpts, BashSpawnContext, BashSpawnHook, EditOpts, FindOpts, GrepOpts, LsOpts, ReadOpts,
    ToolsOptions, WriteOpts,
};
pub use isolation::{
    protected_path_rule, OsSandbox, PermissionPolicy, PolicyDecision, ProtectedFs, ProtectedPaths,
    Rule, SandboxKind, SandboxPolicy, TraversalFs,
};
pub use lock::FileMutationLocks;
pub use ops::{
    Access, ArgvOutput, ArgvSpec, Backend, DirEntry, ExecSpec, ExitStatus, FsOps, ImageMime, Meta,
    ProcOps, ShellConfig, Transport, WalkItem, WalkOpts,
};
pub use registry::{
    all_tools, coding_tools, read_only_tools, Availability, ToolRegistry, BUILTIN_NAMES,
};
pub use truncate::{Truncated, Truncation, TruncatedBy};

/// Optional metadata surface for tools (R-03-039). Built-ins implement it; the `prompt_guidelines`
/// MUST name the tool. Distinct from the runtime-facing `cyrup_core::Tool` trait.
pub trait ToolMeta {
    fn label(&self) -> &str
    where
        Self: cyrup_core::Tool,
    {
        self.name()
    }
    fn description(&self) -> &str;
    /// One-line snippet injected into the system prompt (arch-06).
    fn prompt_snippet(&self) -> Option<&str> {
        None
    }
    /// Tool-specific guidelines (each MUST name the tool).
    fn prompt_guidelines(&self) -> &[&str] {
        &[]
    }
}
