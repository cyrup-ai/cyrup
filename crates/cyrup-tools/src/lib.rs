//! cyrup-tools — built-in tools and the operations seam (arch-03; conformance: func-03).
//!
//! `read`/`write`/`edit`/`bash` + `grep`/`find`/`ls`, the shared truncation model, and the
//! canonical `FsOps`/`ProcOps` (`Backend`) backend seam consumed by isolation (arch-12).
//!
//! Scaffold stub. The `Tool` trait + its `ToolError` are defined in `cyrup-core` (arch-00 §3.4);
//! built-in tools will implement that trait here.

/// The tool failure type is owned by `cyrup-core` (arch-03 §8). Re-exported so built-in tools and
/// callers refer to a single type.
pub use cyrup_core::ToolError;
