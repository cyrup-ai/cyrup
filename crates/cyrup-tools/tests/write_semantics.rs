//! Pi write semantics for the two file mutators (`write`, `edit`) — TOOL-004.
//!
//! Pi's live coding-agent mutators both funnel through ONE injected op whose default is a single
//! Node call: `defaultWriteOperations.writeFile = (path, content) => fsWriteFile(path, content,
//! "utf-8")` (`write.ts:32-35`) and `defaultEditOperations.writeFile` (`edit.ts:83-87`), where
//! `fsWriteFile` is `writeFile` from `fs/promises` (`write.ts:3`, `edit.ts:4`). With the default
//! `{ mode: 0o666, flag: "w" }`, that is `open(2)` with `O_WRONLY|O_CREAT|O_TRUNC` — no
//! `O_NOFOLLOW`, no `O_EXCL` — followed by `write(2)` and `close(2)`.
//!
//! Everything asserted here is a direct consequence of writing THROUGH the existing inode:
//!   * the creation mode is applied only when `O_CREAT` actually creates the file, so an existing
//!     file's mode is untouched (`0700` stays `0700`, `0600` stays `0600`);
//!   * `open` follows symlinks, so writing `a -> b` truncates `b` and leaves `a` a symlink;
//!   * every hard link to the inode observes the new bytes and the inode number is unchanged;
//!   * `O_WRONLY` on a file the process cannot write fails `EACCES`, where a temp-file+`rename`
//!     would have succeeded (`rename(2)` checks the PARENT directory, not the file).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use cyrup_core::{CancelToken, Tool, ToolCallId, ToolUpdate, ToolUpdateSink};
use cyrup_tools::ops::local::LocalFs;
use cyrup_tools::ops::FsOps;
use cyrup_tools::tools::{EditTool, WriteTool};
use cyrup_tools::FileMutationLocks;
use std::sync::Arc;

fn fs() -> Arc<dyn FsOps> {
    Arc::new(LocalFs)
}

fn locks() -> Arc<FileMutationLocks> {
    Arc::new(FileMutationLocks::new())
}

fn cid() -> ToolCallId {
    ToolCallId::from("tc-write-semantics")
}

fn noop_sink() -> ToolUpdateSink {
    Box::new(|_u: ToolUpdate| {})
}

#[cfg(unix)]
fn mode_of(path: &std::path::Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

#[cfg(unix)]
fn chmod(path: &std::path::Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn inode_of(path: &std::path::Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).unwrap().ino()
}

/// True when the test process is root, for whom the DAC write bit is not enforced.
#[cfg(unix)]
fn running_as_root(probe_dir: &std::path::Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    let probe = probe_dir.join(".uid-probe");
    std::fs::write(&probe, b"x").unwrap();
    let uid = std::fs::metadata(&probe).unwrap().uid();
    std::fs::remove_file(&probe).unwrap();
    uid == 0
}

/// `write` over an existing file keeps that file's mode (Pi: the `0o666` creation mode is applied
/// by `open` ONLY when `O_CREAT` creates the file). A temp-file+rename replaces the inode and
/// silently WIDENS a `0600` secrets file to `0666 & ~umask`.
#[cfg(unix)]
#[tokio::test]
async fn write_preserves_the_existing_file_mode() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let secret = cwd.join("secrets.txt");
    std::fs::write(&secret, "old\n").unwrap();
    chmod(&secret, 0o600);

    let write = WriteTool::new(fs(), locks(), cwd.clone(), Default::default());
    write
        .execute(
            cid(),
            serde_json::json!({ "path": "secrets.txt", "content": "new\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(&secret).unwrap(), "new\n");
    assert_eq!(
        mode_of(&secret),
        0o600,
        "write must not change an existing file's mode (pi writes through the existing inode)"
    );
}

/// `edit` over an executable script keeps the `+x` bits.
#[cfg(unix)]
#[tokio::test]
async fn edit_preserves_the_existing_file_mode() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let script = cwd.join("run.sh");
    std::fs::write(&script, "echo old\n").unwrap();
    chmod(&script, 0o700);

    let edit = EditTool::new(fs(), locks(), cwd.clone(), Default::default());
    edit.execute(
        cid(),
        serde_json::json!({
            "path": "run.sh",
            "edits": [{ "oldText": "old", "newText": "new" }]
        }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();

    assert_eq!(std::fs::read_to_string(&script).unwrap(), "echo new\n");
    assert_eq!(mode_of(&script), 0o700, "edit must not strip the executable bits");
}

/// Writing to a symlink FOLLOWS it: the link stays a link and the target's bytes change.
#[cfg(unix)]
#[tokio::test]
async fn write_follows_a_symlink_instead_of_replacing_it() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let target = cwd.join("target.txt");
    let link = cwd.join("link.txt");
    std::fs::write(&target, "old\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let write = WriteTool::new(fs(), locks(), cwd.clone(), Default::default());
    write
        .execute(
            cid(),
            serde_json::json!({ "path": "link.txt", "content": "new\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    assert!(
        std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
        "write replaced the symlink with a regular file"
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "new\n",
        "write did not follow the symlink to its target"
    );
}

/// `edit` through a symlink likewise writes the target and leaves the link a link.
#[cfg(unix)]
#[tokio::test]
async fn edit_follows_a_symlink_instead_of_replacing_it() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let target = cwd.join("target.txt");
    let link = cwd.join("link.txt");
    std::fs::write(&target, "alpha\n").unwrap();
    std::os::unix::fs::symlink(&target, &link).unwrap();

    let edit = EditTool::new(fs(), locks(), cwd.clone(), Default::default());
    edit.execute(
        cid(),
        serde_json::json!({
            "path": "link.txt",
            "edits": [{ "oldText": "alpha", "newText": "beta" }]
        }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();

    assert!(
        std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
        "edit replaced the symlink with a regular file"
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "beta\n");
}

/// A hard-link set survives a mutation: the inode number is unchanged and the sibling link sees
/// the new bytes.
#[cfg(unix)]
#[tokio::test]
async fn edit_keeps_hard_link_identity() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let a = cwd.join("a.txt");
    let b = cwd.join("b.txt");
    std::fs::write(&a, "alpha\n").unwrap();
    std::fs::hard_link(&a, &b).unwrap();
    let ino_before = inode_of(&a);
    assert_eq!(ino_before, inode_of(&b), "fixture: the two names must share an inode");

    let edit = EditTool::new(fs(), locks(), cwd.clone(), Default::default());
    edit.execute(
        cid(),
        serde_json::json!({
            "path": "a.txt",
            "edits": [{ "oldText": "alpha", "newText": "omega" }]
        }),
        CancelToken::new(),
        noop_sink(),
    )
    .await
    .unwrap();

    assert_eq!(inode_of(&a), ino_before, "edit replaced the inode instead of writing through it");
    assert_eq!(inode_of(&a), inode_of(&b), "edit broke the hard-link set");
    assert_eq!(
        std::fs::read_to_string(&b).unwrap(),
        "omega\n",
        "the sibling hard link did not observe the write"
    );
}

/// The same property for `write`.
#[cfg(unix)]
#[tokio::test]
async fn write_keeps_hard_link_identity() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    let a = cwd.join("a.txt");
    let b = cwd.join("b.txt");
    std::fs::write(&a, "alpha\n").unwrap();
    std::fs::hard_link(&a, &b).unwrap();
    let ino_before = inode_of(&a);

    let write = WriteTool::new(fs(), locks(), cwd.clone(), Default::default());
    write
        .execute(
            cid(),
            serde_json::json!({ "path": "a.txt", "content": "omega\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    assert_eq!(inode_of(&a), ino_before, "write replaced the inode instead of writing through it");
    assert_eq!(std::fs::read_to_string(&b).unwrap(), "omega\n");
}

/// A file the process cannot write is rejected, exactly as `open(O_WRONLY)` fails `EACCES` for pi.
/// `rename(2)` needs write permission on the parent DIRECTORY, not the file, so the temp+rename
/// path silently overwrote a `0444` file.
#[cfg(unix)]
#[tokio::test]
async fn write_to_a_read_only_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    if running_as_root(&cwd) {
        eprintln!("skipping: running as root, the DAC write bit is not enforced");
        return;
    }
    let ro = cwd.join("ro.txt");
    std::fs::write(&ro, "old\n").unwrap();
    chmod(&ro, 0o444);

    let write = WriteTool::new(fs(), locks(), cwd.clone(), Default::default());
    let err = write
        .execute(
            cid(),
            serde_json::json!({ "path": "ro.txt", "content": "new\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .expect_err("writing a read-only file must fail like pi's open(O_WRONLY)");

    assert_eq!(
        std::fs::read_to_string(&ro).unwrap(),
        "old\n",
        "the read-only file was overwritten anyway; error was: {err}"
    );
}

/// Creating a NEW file still works (pi's `O_CREAT`) and parent directories are still created
/// (pi's `ops.mkdir(dirname)`, `write.ts:215`).
#[tokio::test]
async fn write_still_creates_new_files_and_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();

    let write = WriteTool::new(fs(), locks(), cwd.clone(), Default::default());
    write
        .execute(
            cid(),
            serde_json::json!({ "path": "nested/deep/new.txt", "content": "hello" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(cwd.join("nested/deep/new.txt")).unwrap(), "hello");
}

/// Overwriting a LONGER file with shorter content must not leave a tail behind: pi's `O_TRUNC`
/// empties the file at `open`, before a single new byte is written.
#[tokio::test]
async fn write_truncates_at_open_leaving_no_tail() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_path_buf();
    std::fs::write(cwd.join("f.txt"), "AAAAAAAAAAAAAAAAAAAAAAAA\n").unwrap();

    let write = WriteTool::new(fs(), locks(), cwd.clone(), Default::default());
    write
        .execute(
            cid(),
            serde_json::json!({ "path": "f.txt", "content": "B\n" }),
            CancelToken::new(),
            noop_sink(),
        )
        .await
        .unwrap();

    assert_eq!(std::fs::read_to_string(cwd.join("f.txt")).unwrap(), "B\n");
}
