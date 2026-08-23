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
//! The model-facing metadata surface (`description`/`prompt_snippet`/`prompt_guidelines`, R-03-039)
//! lives on that same trait, so the `Arc<dyn Tool>` vtable the registry hands out carries it.
//! Failure is signaled by `Err`, mapped to `isError:true` by the runtime (R-03-038). The only
//! `unsafe` in the crate is the isolated unix process-group code in [`ops::local`].
#![deny(unsafe_code)]

pub mod config;
pub mod details;
pub(crate) mod error;
pub mod isolation;
pub(crate) mod jsnum;
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
    OsSandbox, PermissionPolicy, PolicyDecision, ProtectedFs, ProtectedPaths, Rule, SandboxKind,
    SandboxPolicy, TraversalFs, protected_path_rule,
};
pub use lock::FileMutationLocks;
pub use ops::{
    Access, ArgvOutput, ArgvSpec, Backend, DirEntry, ExecSpec, ExitStatus, FsOps, ImageMime, Meta,
    ProcOps, ShellConfig, Transport, WalkItem, WalkOpts, kill_pid, kill_process_tree,
    kill_tracked_detached_children, terminate_pid, track_detached_child_pid,
    untrack_detached_child_pid,
};
pub use registry::{
    Availability, BUILTIN_NAMES, ToolRegistry, all_tools, coding_tools, read_only_tools,
};
pub use truncate::{Truncated, TruncatedBy, Truncation};

#[cfg(test)]
mod tests;
