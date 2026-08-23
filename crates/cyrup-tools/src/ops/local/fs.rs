//! [`LocalFs`] — the default [`FsOps`] backend over `tokio::fs`.
//!
//! An indirection over the real filesystem: every built-in tool reaches the disk through this
//! trait object rather than `std::fs`, which is what lets isolation (arch-12) and remote backends
//! re-target them without touching tool logic.

use crate::error;
use crate::ops::{Access, DirEntry, FsOps, Meta, WalkItem, WalkOpts};
use cyrup_core::{EventStream, ToolError};
use ignore::WalkBuilder;
use std::path::Path;
use tokio::io::AsyncWriteExt;

/// The win32 half of [`FsOps::access`], factored out of the `cfg(not(unix))` arm so the decision
/// that SHIPS to Windows is compiled and unit-tested on every host — see
/// `crates/cyrup-tools/src/tests/read_access_errno.rs`. The arm itself cannot be exercised here,
/// but this predicate is the whole of its behaviour.
///
/// Pi issues ONE call on every platform (`fsAccess(path, R_OK)` at read.ts:60, `fsAccess(path,
/// R_OK | W_OK)` at edit.ts:97), so parity for this arm is defined by libuv's `fs__access`
/// (`uv/src/win/fs.c`), which Node's `fs.access` runs on win32:
///
///   * it calls `GetFileAttributesW` and fails with the *stat* error when the path is absent;
///   * otherwise access is granted unless **W_OK was requested** AND the file carries
///     `FILE_ATTRIBUTE_READONLY` AND it is **not a directory** (directories cannot be read-only on
///     Windows, so libuv exempts them explicitly);
///   * the denial is `UV_EPERM`, not `EACCES`.
///
/// Two consequences worth stating, because both look like bugs against the unix arm and are not.
/// `R_OK` NEVER fails for a path that exists: libuv does not consult ACLs, which Node documents
/// ("the `fs.access()` function … does not check the ACL and therefore may report that a path is
/// accessible even if the ACL restricts the user"). So the coarse, stat-only shape of this arm is
/// parity with pi, not a shortcut — an unreadable-but-present file passes upstream too. And the
/// `Exists` mode reduces to the same stat, exactly as `F_OK` does for libuv.
/// The denial itself is `UV_EPERM`, surfaced by Node as an error whose `.code` is `EPERM`, so it
/// travels through [`error::io_errno_code`] — the same `CODE: context: display` shape the unix arm
/// builds with [`error::io_errno`]. `edit.rs`'s `errno_code_of` therefore recovers a code on BOTH
/// arms and Pi's `Error code: ${error.code}` line (edit.ts:332-333) survives on Windows. The
/// previous `error::invalid("{path} is not writable")` carried no code token at all.
#[cfg_attr(unix, allow(dead_code))]
pub(crate) fn windows_access_result(
    path: &Path,
    mode: Access,
    readonly: bool,
    is_dir: bool,
) -> Result<(), ToolError> {
    if mode == Access::ReadWrite && readonly && !is_dir {
        return Err(error::io_errno_code(
            "EPERM",
            &error::show(path),
            &std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        ));
    }
    Ok(())
}

/// Local filesystem operations.
#[derive(Default, Clone)]
pub struct LocalFs;

#[async_trait::async_trait]
impl FsOps for LocalFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        tokio::fs::read(path)
            .await
            .map_err(|e| error::io(&error::show(path), &e))
    }

    /// A real `std::fs::File`, so `grep`'s search pulls the file through `grep_searcher`'s rolling
    /// buffer instead of allocating it whole — the property Pi gets for free by running the search
    /// in a separate ripgrep process (grep.ts:226). The `open(2)` itself runs on the blocking pool;
    /// the reads happen inside the caller's `spawn_blocking`, per [`FsOps::read_stream`].
    async fn read_stream(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        let owned = path.to_path_buf();
        let file = tokio::task::spawn_blocking(move || std::fs::File::open(&owned))
            .await
            .map_err(|e| error::invalid(format!("read_stream: {e}")))?
            .map_err(|e| error::io(&error::show(path), &e))?;
        Ok(Box::new(file))
    }

    /// 1:1 with Pi's `fsWriteFile(path, content, "utf-8")` (write.ts:33 / edit.ts:85):
    /// `O_WRONLY|O_CREAT|O_TRUNC` with creation mode `0o666` (umask applies), write, close.
    ///
    /// [CYRUP-DELTA] The parent-directory creation is Pi's SEPARATE `ops.mkdir(dirname(path),
    /// {recursive:true})` step, which `write` runs immediately before its `writeFile`
    /// (write.ts:32-35, :215-218). It is folded in here rather than exposed as a second trait
    /// method so the protected-path decorator still gets exactly one chance to deny BEFORE any
    /// directory is created. `edit` reaches this after its own `access(R_OK|W_OK)` precheck has
    /// already proven the file exists, so the `create_dir_all` is a no-op on that path.
    ///
    /// This deliberately does NOT write a temp file and rename it into place. Doing so replaces the
    /// target inode and so silently drops the file's mode (a `0600` secrets file becomes
    /// `0666 & ~umask`), its ownership, its hard-link set, and its identity as a symlink; it also
    /// lets a write succeed on a read-only file, because `rename(2)` checks the parent directory
    /// rather than the file. See [`FsOps::write_in_place`] for the durability trade this accepts.
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| error::io(&format!("create dir {}", error::show(parent)), &e))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await
            .map_err(|e| error::io(&format!("write {}", error::show(path)), &e))?;
        file.write_all(bytes)
            .await
            .map_err(|e| error::io(&format!("write {}", error::show(path)), &e))?;
        // `tokio::fs::File` buffers; flush pushes the bytes to the OS. Node's `writeFile` likewise
        // only loops `write(2)` and closes the fd — there is no `fsync` on either side.
        file.flush()
            .await
            .map_err(|e| error::io(&format!("write {}", error::show(path)), &e))?;
        Ok(())
    }

    #[allow(unsafe_code)]
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError> {
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            // Pi prechecks EFFECTIVE access via Node's `fs.access`: `read` uses `R_OK`
            // (read.ts:54) and `edit` uses `R_OK | W_OK` (edit.ts:86). Mirror that with the
            // `access(2)` syscall so the precheck reflects the caller's effective permissions
            // (uid/gid/ACL), not merely the coarse `permissions().readonly()` bit which can pass
            // for a file the process cannot actually read/write across owners.
            let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_err(|_| error::invalid(format!("invalid path: {}", error::show(path))))?;
            let amode = match mode {
                Access::Exists => libc::F_OK,
                Access::Read => libc::R_OK,
                Access::ReadWrite => libc::R_OK | libc::W_OK,
            };
            // SAFETY: `access(2)` only reads the NUL-terminated path buffer we own; it performs no
            // writes and touches no parent memory. Returns 0 on success, or -1 with `errno` set.
            let rc = unsafe { libc::access(c_path.as_ptr(), amode) };
            if rc != 0 {
                // `io_errno`, not `io`: Pi's `edit` reports `Error code: ${error.code}`
                // (edit.ts:332-333) off the caught Node error object, and `ToolError` is flat, so
                // the errno NAME has to ride in the message for `edit.rs` to recover it. `read`
                // propagates this string verbatim (read.ts:241 uncaught) and Node's own raw text
                // leads with the same code, so the prefix moves `read` toward Pi as well.
                return Err(error::io_errno(
                    &error::show(path),
                    &std::io::Error::last_os_error(),
                ));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            // Pi's precheck is the SAME one call on every platform — `fsAccess(path, R_OK)`
            // (read.ts:60) / `fsAccess(path, R_OK | W_OK)` (edit.ts:97) — so what this arm has to
            // reproduce is what libuv's `fs__access` does on win32, not what `access(2)` does.
            // See [`windows_access_result`] for the decision, its parity argument, and the tests
            // that cover it on every host.
            let meta = tokio::fs::metadata(path)
                .await
                .map_err(|e| error::io_errno(&error::show(path), &e))?;
            windows_access_result(path, mode, meta.permissions().readonly(), meta.is_dir())
        }
    }

    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError> {
        let meta = tokio::fs::metadata(path)
            .await
            .map_err(|e| error::io(&error::show(path), &e))?;
        let canonical = tokio::fs::canonicalize(path)
            .await
            .unwrap_or_else(|_| path.to_path_buf());
        Ok(Meta {
            is_dir: meta.is_dir(),
            is_file: meta.is_file(),
            len: meta.len(),
            canonical,
        })
    }

    /// `io_errno` rather than `io` so `ls`'s `Cannot read directory: ${e.message}` wrapper
    /// (ls.ts:150-152) renders a Node-shaped body leading with the errno code, which is what
    /// `e.message` is on the upstream side.
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        let mut rd = tokio::fs::read_dir(path)
            .await
            .map_err(|e| error::io_errno(&error::show(path), &e))?;
        let mut out = Vec::new();
        loop {
            match rd.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    out.push(DirEntry {
                        name,
                        path: entry.path(),
                    });
                }
                Ok(None) => break,
                Err(e) => return Err(error::io_errno(&error::show(path), &e)),
            }
        }
        Ok(out)
    }

    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>> {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<WalkItem, ToolError>>(256);
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let walker = WalkBuilder::new(&root)
                .hidden(!opts.include_hidden)
                .git_ignore(true)
                .git_exclude(true)
                // Pi runs `rg`/`fd` which honor the user's global gitignore (`~/.gitignore`,
                // arch-03:404). Mirror that with `git_global(true)`.
                .git_global(true)
                // `require_git(false)` (fd's `--no-require-git`) honors `.gitignore` even outside a
                // repo; `require_git(true)` is fd/ripgrep's default nested-repo-boundary behavior.
                // The caller sets this per search path (find.ts:226-240): `false` outside a repo,
                // `true` inside one. See `WalkOpts::require_git`.
                .require_git(opts.require_git)
                .parents(true)
                .build();
            for result in walker {
                let item = match result {
                    Ok(entry) => {
                        let path = entry.path().to_path_buf();
                        let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                        Ok(WalkItem { path, is_dir })
                    }
                    Err(e) => Err(ToolError::new(format!("walk: {e}"))),
                };
                if tx.blocking_send(item).is_err() {
                    break;
                }
            }
        });
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}
