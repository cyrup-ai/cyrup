//! Error helpers (arch-03 §8).
//!
//! The failure type itself is `cyrup_core::ToolError` (a flat `{message}` struct) — re-exported by
//! the crate root so built-ins and callers refer to a single type. These helpers build the
//! model-observed messages with a consistent vocabulary. The agent runtime maps any `Err` to an
//! `isError:true` tool result (R-03-038); a normal `Ok` is never an error.

use cyrup_core::ToolError;
use std::path::Path;

/// Generic invalid-input / bad-params error.
pub(crate) fn invalid(msg: impl Into<String>) -> ToolError {
    ToolError::new(msg)
}

/// Missing file / directory.
pub(crate) fn not_found(msg: impl Into<String>) -> ToolError {
    ToolError::new(msg)
}

/// Wrap a std::io error with context.
pub(crate) fn io(context: &str, e: &std::io::Error) -> ToolError {
    ToolError::new(format!("{context}: {e}"))
}

/// Cancellation (R-03-009). Pi throws `new Error("Operation aborted")` (capital O) on every
/// non-bash tool's abort path — read.ts:226, edit.ts:319, ls.ts:115/119, write.ts:209, grep.ts:158,
/// find.ts — so the model-observed literal must match exactly. (bash uses Pi's distinct
/// `"Command aborted"`, matched at bash.rs via the `Killed` arm — it never routes through here.)
pub(crate) fn aborted() -> ToolError {
    ToolError::new("Operation aborted")
}

/// Policy / isolation denial (arch-12 R-12-006/R-03-006). Surfaced as a normal `Err`, which the
/// runtime maps to an `isError:true` tool result (R-03-038) — never a panic. Message kept stable so
/// callers/tests can detect a blocked operation.
pub(crate) fn denied(msg: impl Into<String>) -> ToolError {
    ToolError::new(msg)
}

/// Display a path for messages (lossy, never panics).
pub(crate) fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
