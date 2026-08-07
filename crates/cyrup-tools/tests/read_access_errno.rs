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
use cyrup_tools::config::ReadOpts;
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::tools::ReadTool;
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
#[cfg(unix)]
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
#[cfg(unix)]
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
