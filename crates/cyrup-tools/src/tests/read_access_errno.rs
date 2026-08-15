//! `read`'s readability precheck must propagate the underlying errno, not a fixed string.
//!
//! Pi `coding-agent/src/core/tools/read.ts`:
//!   * `:54`  — `access: (path) => fsAccess(path, constants.R_OK)`
//!   * `:241` — `await ops.access(absolutePath);` (uncaught)
//!   * `:321-324` — the sole `catch` re-`reject`s the original error
//!
//! so the model receives Node's raw errno text, e.g.
//! `ENOENT: no such file or directory, access '/work/missing.txt'` — carrying the errno CODE and
//! the RESOLVED ABSOLUTE path. (The sibling `edit` deliberately wraps instead, edit.ts:326-331,
//! which `edit.rs:194-196` mirrors; `read` is the one that must propagate.)
//!
//! cyrup replaced all of it with `File not found or unreadable: {the raw user-supplied path}`,
//! which collapsed ENOENT/EACCES/ENOTDIR into one string and hid which absolute path was actually
//! probed — the latter mattering because `read.rs` may have selected a macOS filename VARIANT of
//! the requested name.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolUpdate};
use crate::config::ReadOpts;
use crate::ops::local::LocalFs;
use crate::tools::ReadTool;
use std::path::Path;
use std::sync::Arc;

async fn read_err(cwd: &Path, path: &str) -> String {
    let read = ReadTool::new(Arc::new(LocalFs), cwd.to_path_buf(), ReadOpts::default());
    let err = read
        .execute(
            ToolCallId::from("tc-test"),
            serde_json::json!({ "path": path }),
            CancelToken::new(),
            Box::new(|_u: ToolUpdate| {}),
        )
        .await
        .unwrap_err();
    err.to_string()
}

/// A missing file must report ENOENT against the RESOLVED absolute path.
///
/// Asserted on the errno CODE rather than on the OS display text, so it holds on BOTH `access`
/// arms: `errno_name`'s `cfg(not(unix))` half maps `ErrorKind::NotFound` to the same `ENOENT`
/// (error.rs), while the trailing `strerror` prose differs per platform.
#[tokio::test]
async fn missing_file_reports_enoent_and_the_resolved_absolute_path() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let msg = read_err(&cwd, "missing.txt").await;

    let abs = cwd.join("missing.txt");
    assert!(
        msg.contains(&*abs.to_string_lossy()),
        "must name the resolved absolute path {abs:?}, got: {msg}"
    );
    assert!(msg.starts_with("ENOENT: "), "must lead with the ENOENT code token, got: {msg}");
    #[cfg(unix)]
    assert!(msg.contains("No such file or directory"), "must carry the ENOENT errno, got: {msg}");
    assert!(
        !msg.contains("File not found or unreadable"),
        "the cyrup-invented literal must be gone, got: {msg}"
    );
}

/// An EXISTING but unreadable file must report EACCES — a class the model can act on differently
/// from ENOENT. Under the old fixed string these two were indistinguishable.
#[cfg(unix)]
#[tokio::test]
async fn unreadable_file_reports_eacces_distinctly_from_enoent() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let p = cwd.join("secret.txt");
    std::fs::write(&p, "top secret\n").unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A process that bypasses R_OK (root) reads it anyway — same as Pi for root. Skip.
    if std::fs::read(&p).is_ok() {
        let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));
        return;
    }

    let msg = read_err(&cwd, "secret.txt").await;
    let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644));

    assert!(
        msg.contains(&*p.to_string_lossy()),
        "must name the resolved absolute path {p:?}, got: {msg}"
    );
    assert!(msg.contains("Permission denied"), "must carry the EACCES errno, got: {msg}");

    let enoent = read_err(&cwd, "missing.txt").await;
    assert_ne!(
        msg, enoent,
        "EACCES and ENOENT must not collapse to the same model-facing string"
    );
}

/// The path is reported as RESOLVED, not as the raw argument the model typed — the whole point of
/// the errno text for a tool whose `resolve_read_path` may pick a different filename variant.
/// Platform-agnostic: both `access` arms report the absolute path they probed.
#[tokio::test]
async fn nested_relative_path_is_reported_resolved_not_raw() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::create_dir_all(cwd.join("a/b")).unwrap();
    let msg = read_err(&cwd, "a/b/nope.txt").await;
    assert!(
        msg.contains(&*cwd.join("a/b/nope.txt").to_string_lossy()),
        "message must be absolute, got: {msg}"
    );
}

// ---------------------------------------------------------------------------------------------
// The `cfg(not(unix))` half of `LocalFs::access` — shipped (cyrup-tools cross-compiles clean for
// `x86_64-pc-windows-gnu`) and, until now, with zero tests because every test in this file wore a
// blanket `#[cfg(unix)]`. `windows_access_result` is the whole of that arm's decision, factored
// out of the cfg block precisely so it can be asserted from the arm the suite actually runs on.
// ---------------------------------------------------------------------------------------------

use crate::error::errno_code_of;
use crate::ops::local::windows_access_result;
use crate::ops::Access;

/// RED before the fix: the arm returned `error::invalid("{path} is not writable")`, a message with
/// no leading errno token, so `errno_code_of` — `edit.rs`'s port of pi's `"code" in error` test
/// (edit.ts:332) — returned `None` and pi's `Error code: ${error.code}` line silently disappeared
/// on Windows while surviving on macOS/Linux. libuv's `fs__access` denies `W_OK` on a read-only
/// file with `UV_EPERM`, which Node reports as `.code === "EPERM"`.
#[test]
fn windows_readwrite_denial_carries_a_recoverable_errno_code() {
    let err = windows_access_result(Path::new(r"C:\work\ro.txt"), Access::ReadWrite, true, false)
        .expect_err("W_OK on a FILE_ATTRIBUTE_READONLY file is libuv's UV_EPERM");

    assert_eq!(
        errno_code_of(&err),
        Some("EPERM"),
        "edit's `Error code:` line needs a recoverable code on this arm too, got: {err}"
    );
    assert!(err.message.starts_with("EPERM: "), "code must lead the message, got: {err}");
    assert!(
        err.message.contains(r"C:\work\ro.txt"),
        "the probed path must still be named, got: {err}"
    );
    assert!(
        !err.message.contains("is not writable"),
        "the cyrup-invented literal must be gone, got: {err}"
    );
}

/// The rest of libuv's `fs__access` truth table. Both `Ok` rows look like under-checking against
/// the unix arm and are parity: libuv grants `R_OK` for anything that stats (Node documents that
/// `fs.access` does not consult Windows ACLs), and it exempts directories from the read-only test
/// because a directory's `FILE_ATTRIBUTE_READONLY` bit does not mean "not writable" on Windows.
#[test]
fn windows_access_truth_table_matches_libuv_fs_access() {
    let p = Path::new(r"C:\work\thing");

    // W_OK not requested ⇒ granted, read-only bit or not.
    assert!(windows_access_result(p, Access::Exists, true, false).is_ok());
    assert!(windows_access_result(p, Access::Read, true, false).is_ok());
    // Writable file ⇒ granted.
    assert!(windows_access_result(p, Access::ReadWrite, false, false).is_ok());
    // Directory with the read-only attribute ⇒ granted (libuv's explicit exemption).
    assert!(windows_access_result(p, Access::ReadWrite, true, true).is_ok());
    // The one denial.
    assert!(windows_access_result(p, Access::ReadWrite, true, false).is_err());
}
