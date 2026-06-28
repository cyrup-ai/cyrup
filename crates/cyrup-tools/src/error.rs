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

/// Cancellation (R-03-009). Message kept stable so callers/tests can detect it.
pub(crate) fn aborted() -> ToolError {
    ToolError::new("operation aborted")
}

/// Display a path for messages (lossy, never panics).
pub(crate) fn show(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
