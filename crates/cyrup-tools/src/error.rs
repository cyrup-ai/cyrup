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

/// Node's `error.code` for a filesystem `io::Error` — the libuv errno NAME (`ENOENT`, `EACCES`,
/// …) that Pi interpolates as `` `Error code: ${error.code}` `` (edit.ts:332-333).
///
/// Rust's `io::Error` carries the raw platform errno and a Display string, but no code name, and
/// [`ToolError`] is a flat `{message}` struct with nowhere to put a side channel. So the name is
/// derived here and travels as the LEADING token of the message built by [`io_errno`], which is
/// also the shape Node's own `Error.message` uses (`ENOENT: no such file or directory, access
/// '/x'`). [`errno_code_of`] recovers it, and is the Rust analogue of Pi's `"code" in error` test.
///
/// The set below is exactly the errno list Node's `uv_err_name` can produce for `access(2)` /
/// `open(2)` / `scandir(3)` on the paths these tools take; anything else falls through to `None`,
/// which drives Pi's `String(error)` branch (edit.ts:333) rather than inventing a code.
pub(crate) fn errno_name(e: &std::io::Error) -> Option<&'static str> {
    #[cfg(unix)]
    {
        let code = e.raw_os_error()?;
        let name = match code {
            libc::ENOENT => "ENOENT",
            libc::EACCES => "EACCES",
            libc::EPERM => "EPERM",
            libc::EROFS => "EROFS",
            libc::EISDIR => "EISDIR",
            libc::ENOTDIR => "ENOTDIR",
            libc::ELOOP => "ELOOP",
            libc::ENAMETOOLONG => "ENAMETOOLONG",
            libc::ENOTEMPTY => "ENOTEMPTY",
            libc::EEXIST => "EEXIST",
            libc::EMFILE => "EMFILE",
            libc::ENFILE => "ENFILE",
            libc::ENOSPC => "ENOSPC",
            libc::EIO => "EIO",
            libc::EBUSY => "EBUSY",
            libc::EINVAL => "EINVAL",
            _ => return None,
        };
        Some(name)
    }
    #[cfg(not(unix))]
    {
        // Windows has no errno; map the portable kinds Node's libuv shim reports for the same
        // syscalls. `raw_os_error` there is a Win32 code, not an errno, so go through `kind()`.
        match e.kind() {
            std::io::ErrorKind::NotFound => Some("ENOENT"),
            std::io::ErrorKind::PermissionDenied => Some("EACCES"),
            std::io::ErrorKind::AlreadyExists => Some("EEXIST"),
            _ => None,
        }
    }
}

/// [`io`] with Node's errno code prepended, so the code survives the flattening into
/// [`ToolError`]'s single `message` field. See [`errno_name`].
pub(crate) fn io_errno(context: &str, e: &std::io::Error) -> ToolError {
    match errno_name(e) {
        Some(code) => ToolError::new(format!("{code}: {context}: {e}")),
        None => io(context, e),
    }
}

/// [`io_errno`] for a branch where the platform hands back no `io::Error` whose code we can
/// derive — the code is supplied by the caller instead. The wire shape is IDENTICAL to
/// [`io_errno`]'s (`CODE: context: display`), so [`errno_code_of`] recovers it the same way and
/// `edit` still renders Pi's `Error code: ${error.code}` line (edit.ts:332-333).
///
/// Used by `LocalFs::access`'s `cfg(not(unix))` arm, where libuv fixes the code (`UV_EPERM`)
/// rather than reporting an errno — see `crate::ops::local::windows_access_result`.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn io_errno_code(code: &str, context: &str, e: &std::io::Error) -> ToolError {
    ToolError::new(format!("{code}: {context}: {e}"))
}

/// Recover the errno code from a message built by [`io_errno`] — Pi's `"code" in error` test
/// (edit.ts:332). `None` means "this error object has no `code`", which is Pi's `String(error)`
/// branch.
pub(crate) fn errno_code_of(err: &ToolError) -> Option<&str> {
    let (head, _rest) = err.message.split_once(": ")?;
    // Guard against a path or free text that happens to contain `": "`: a code is a short,
    // all-uppercase-ASCII `E…` token, exactly as `uv_err_name` produces.
    let is_code = head.len() >= 2
        && head.len() <= 16
        && head.starts_with('E')
        && head.bytes().all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    is_code.then_some(head)
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
