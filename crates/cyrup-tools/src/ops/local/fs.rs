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

/// Render an [`ignore::Error`] the way ripgrep does — `{path}: {io error}`, with the path
/// stated **exactly once**.
///
/// `Display` cannot be used directly. `Error::from_walkdir` stores
/// `Error::Io(io::Error::from(walkdir_err))` (ignore 0.4.26 `src/lib.rs:296-301`), and
/// walkdir 2.5.0's `From<Error> for io::Error` is `io::Error::new(kind, walk_err)`
/// (`walkdir-2.5.0/src/error.rs:253-261`) — a CUSTOM io error whose own `Display` re-states
/// the path as `"IO error for operation on {path}: {err}"` (`:224-229`). Printed under
/// `WithPath` (`lib.rs:333-335`) that yields the path twice. walkdir 2.3.x returned the
/// inner `io::Error` unchanged, which is why `rg` 14.1.0 prints the clean form and a
/// straight `to_string()` here does not.
///
/// `Error::io_error()` is NOT the escape hatch: it unwraps only `ignore`'s own
/// `Partial`/`WithLineNumber`/`WithPath`/`WithDepth` nesting and then returns the
/// `Error::Io` payload verbatim (`lib.rs:205-222`), i.e. walkdir's wrapper. And there is
/// no `Error::path()` accessor at 0.4.26 — the path is only reachable by matching the
/// public `WithPath` variant (`lib.rs:78-84`), which is what this does.
///
/// Every arm reproduces the corresponding arm of `impl Display for Error`
/// (`lib.rs:322-359`) unchanged except for the io leaf, so `Loop` (which carries no
/// `WithPath` — `lib.rs:286-295`) keeps ripgrep's un-prefixed
/// `"File system loop found: … points to an ancestor …"`.
/// One walked entry, converted for the stream — shared VERBATIM by the serial and the parallel
/// branch of [`LocalFs::walk`] so the two cannot drift in what they yield.
///
/// A per-entry `Err` is a NON-FATAL event on this stream and the walk CONTINUES past it:
/// `ignore::Walk::next` leaves its iterator intact after yielding one (ignore 0.4.26
/// `src/walk.rs:1124-1126`), and on the parallel branch the visitor returns `WalkState::Continue`
/// for it just as it does for an `Ok`. Consumers must never read one as end-of-stream.
///
/// The message is [`walk_error_message`]'s rendering of the `ignore::Error`, i.e. ripgrep's own
/// `{path}: {io error}` with the path stated once, e.g.
/// `/srv/locked: Permission denied (os error 13)`. It is NOT `e.to_string()`: at ignore 0.4.26 +
/// walkdir 2.5.0 `Display` states the path TWICE — see [`walk_error_message`]. No prefix is added
/// here because the two consumers of this seam want different things from the same value: `find`
/// emulates fd, which discards it (fd 10.5.0 `src/walk.rs:227-231`, `:500-505`), and `grep`
/// emulates ripgrep, which reports it as `rg: {path}: {io error}` on stderr. A `walk: ` prefix
/// matched neither.
fn to_walk_item(result: Result<ignore::DirEntry, ignore::Error>) -> Result<WalkItem, ToolError> {
    match result {
        Ok(entry) => {
            let path = entry.path().to_path_buf();
            // One `file_type()` read feeds both flags. `ignore` 0.4.26 derives a traversed entry's
            // type from `std::fs::DirEntry::file_type` (`walk.rs:322-333`, `:353-367`), which is
            // `lstat` semantics: a symlink reports itself as a symlink, so it is neither a dir nor
            // a file and both flags are false — exactly the entry ripgrep declines to search.
            // `None` occurs only for the synthetic stdin entry (`walk.rs:67-78`), which a
            // path-rooted walker never yields, so `unwrap_or(false)` mirrors ripgrep's own
            // `file_type().map_or(false, |ft| ft.is_file())`: unknown type is not a regular file.
            let ty = entry.file_type();
            let is_dir = ty.map(|t| t.is_dir()).unwrap_or(false);
            let is_file = ty.map(|t| t.is_file()).unwrap_or(false);
            Ok(WalkItem {
                path,
                is_dir,
                is_file,
            })
        }
        Err(e) => Err(ToolError::new(walk_error_message(&e))),
    }
}

pub(crate) fn walk_error_message(err: &ignore::Error) -> String {
    match err {
        // The one variant carrying a path. Recurse, so the tail is the PEELED io text
        // rather than walkdir's restatement of this same path.
        ignore::Error::WithPath { path, err } => {
            format!("{}: {}", path.display(), walk_error_message(err))
        }
        // Transparent in `Display` (`lib.rs:336`); stay transparent.
        ignore::Error::WithDepth { err, .. } => walk_error_message(err),
        // `Display` is `line {n}: {err}` (`lib.rs:330-332`).
        ignore::Error::WithLineNumber { line, err } => {
            format!("line {line}: {}", walk_error_message(err))
        }
        // `Display` joins with `\n` (`lib.rs:325-329`).
        ignore::Error::Partial(errs) => errs
            .iter()
            .map(walk_error_message)
            .collect::<Vec<_>>()
            .join("\n"),
        ignore::Error::Io(io) => io_error_message(io),
        // `Loop`, `Glob`, `UnrecognizedFileType`, `InvalidDefinition`: no nested io error,
        // no doubled path — their own `Display` is already what ripgrep prints.
        other => other.to_string(),
    }
}

/// Peel walkdir's wrapper off an [`std::io::Error`] so it prints as the OS did.
///
/// `io::Error::get_ref` is `Some` only for the custom repr, where it hands back the boxed
/// payload — here the `walkdir::Error`. walkdir's `source()` is the ORIGINAL `io::Error`
/// (`walkdir-2.5.0/src/error.rs:212-217`), which renders as
/// `Permission denied (os error 13)`.
///
/// Both hops are required. An `io::Error` straight from `std::fs` is the OS repr, so
/// `get_ref()` is `None`; `ignore`'s handful of `io::Error::new(kind, "<literal>")` sites
/// (`walk.rs:175-179`, `:377`, `:427`) DO have a payload but its `source()` is `None`.
/// Both therefore fall through to their own `Display`, unchanged. walkdir's error is the
/// only payload on this seam that carries a `source()`.
fn io_error_message(err: &std::io::Error) -> String {
    if let Some(payload) = err.get_ref()
        && let Some(original) = std::error::Error::source(payload)
    {
        return original.to_string();
    }
    err.to_string()
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

    /// One `spawn_blocking` for the whole batch instead of one per path — see
    /// [`FsOps::read_streams`] for why this override exists and what it is worth.
    ///
    /// `open(2)` itself is cheap (~1.5µs here); it is entering the blocking pool that is not. The
    /// opens run back-to-back on a single pool thread, so the batch costs one round-trip in total.
    async fn read_streams(
        &self,
        paths: &[std::path::PathBuf],
    ) -> Vec<Result<Box<dyn std::io::Read + Send>, ToolError>> {
        let owned: Vec<std::path::PathBuf> = paths.to_vec();
        let opened = tokio::task::spawn_blocking(move || {
            owned
                .into_iter()
                .map(|path| {
                    std::fs::File::open(&path)
                        .map(|f| Box::new(f) as Box<dyn std::io::Read + Send>)
                        .map_err(|e| error::io(&error::show(&path), &e))
                })
                .collect::<Vec<_>>()
        })
        .await;
        match opened {
            Ok(v) => v,
            // The pool itself failed, so nothing was opened. Every path reports it, keeping the
            // one-result-per-path contract the caller indexes against.
            Err(e) => paths
                .iter()
                .map(|_| Err(error::invalid(format!("read_streams: {e}"))))
                .collect(),
        }
    }

    /// 1:1 with Pi's `fsWriteFile(path, content, "utf-8")` (write.ts:39 / edit.ts:107):
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
        // `io_errno`, not `io`, on every edge below. Pi's write ops are raw Node calls whose
        // rejections propagate uncaught out of `execute` (write.ts:221/225, edit.ts:371) and reach
        // the model as `error.message` verbatim (agent-loop.ts:701-707), and a Node `SystemError`
        // message ALWAYS leads with the libuv errno name — `EACCES: permission denied, open '/x'`,
        // `ENOSPC: no space left on device, write`. `ToolError` is flat, so the code has to ride as
        // the leading token of the message; that is precisely what `error::io_errno` builds, and it
        // is the same `CODE: context: display` shape `access`/`read_dir`/`lock` already emit. The
        // context is the SYSCALL Node names for each edge (`mkdir`, `open`, `write`), so the model
        // can tell a parent-creation failure from an open failure from a short write, which the
        // single shared `write {path}` context could not.
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| error::io_errno(&format!("mkdir {}", error::show(parent)), &e))?;
        }
        let mut file = tokio::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .await
            .map_err(|e| error::io_errno(&format!("open {}", error::show(path)), &e))?;
        file.write_all(bytes)
            .await
            .map_err(|e| error::io_errno(&format!("write {}", error::show(path)), &e))?;
        // `tokio::fs::File` buffers; flush pushes the bytes to the OS. Node's `writeFile` likewise
        // only loops `write(2)` and closes the fd — there is no `fsync` on either side.
        file.flush()
            .await
            .map_err(|e| error::io_errno(&format!("write {}", error::show(path)), &e))?;
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
            // fd builds its walker in two phases — the chained knobs, then the tool-specific
            // ignore SOURCES (fd 10.5.0 `src/walk.rs:352-386`) — and so must this. `add_ignore`
            // returns `Option<ignore::Error>`, not `&mut WalkBuilder` (ignore 0.4.26
            // `walk.rs:718`), so the chain cannot stay a single expression terminating in
            // `.build()`.
            let mut builder = WalkBuilder::new(&root);
            builder
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
                // Also what makes the custom ignore filename below apply in ANCESTORS of the
                // search root: `Ignore::add_parents` runs `add_child_path` per parent, which
                // compiles the custom-ignore matcher too (ignore 0.4.26 `dir.rs:182-248`,
                // `:286-292`). fd computes this same `true` (`read_parent_ignore &&
                // (read_fdignore || read_vcsignore)`).
                //
                // `--no-ignore` implies `--no-ignore-parent` (ripgrep `defs.rs:4262-4266` sets all
                // five `no_ignore_*` flags), so parent traversal is switched off with it.
                //
                // Honestly labelled: this is currently BELT AND BRACES, not load-bearing, and no
                // test can distinguish it. Whenever `no_ignore` is set, every source parent
                // traversal could contribute is switched off elsewhere in this same chain —
                // `.ignore` and the gitignore family below, the custom file at the registration
                // gate further down. Their POSITION is irrelevant: every `WalkBuilder` setter is a
                // plain field assignment on `ig_builder` (ignore 0.4.33 `walk.rs:896-899`,
                // `:820-826`) and nothing is read until `build()`, so these calls commute.
                //
                // The one source that survives `--no-ignore` is `--ignore-file` (ripgrep
                // `defs.rs:4251-4252`: it "does not imply \flag{no-ignore-files}"), and those are
                // explicit paths that parent traversal never supplied. It is set because it is what
                // the flag MEANS, so that adding a future parent-sensitive source cannot silently
                // reintroduce the bug.
                .parents(!opts.no_ignore);

            // The `$RIPGREP_CONFIG_PATH` knobs. Each is a no-op at its default, so this block
            // changes nothing for a caller that did not ask for it.
            //
            // `-u`/`--no-ignore` is the wider of the two ignore switches, and it is wider because
            // it reaches FOUR mechanisms — not because any one of them covers more:
            //
            //   `.ignore(false)`                     `.ignore` files, and ONLY those
            //   the three `git_*` switches           the gitignore family
            //   the registration gate further down   the custom ignore file (`.rgignore`)
            //   `.parents(!opts.no_ignore)` above    ignore files in ANCESTOR directories
            //
            // `ignore(false)` covering the custom file is the misconception this comment used to
            // state, and it is why `--no-ignore` once did nothing at all in any tree holding a
            // `.rgignore`. In ignore 0.4.33 the two matchers are built independently: the custom
            // one is gated ONLY on whether the filename list is empty (`dir.rs:360-364` — no
            // `opts.ignore` in the condition), while `opts.ignore` gates the `.ignore` matcher
            // alone (`dir.rs:391-401`). See the registration gate below for why that has to be a
            // skip rather than an undo.
            //
            // `--no-ignore-vcs` is the narrower one and takes only the git switches, so `.ignore`,
            // the custom file and parent traversal all keep applying. That asymmetry is the whole
            // difference between the two flags and is why they are not folded together.
            if opts.no_ignore {
                builder
                    .ignore(false)
                    .git_ignore(false)
                    .git_exclude(false)
                    .git_global(false);
            } else if opts.no_ignore_vcs {
                builder
                    .git_ignore(false)
                    .git_exclude(false)
                    .git_global(false);
            }
            builder
                .max_depth(opts.max_depth)
                .follow_links(opts.follow_links)
                .same_file_system(opts.one_file_system)
                .max_filesize(opts.max_filesize);
            // `Path::cmp` is the byte-wise component order `--sort=path` produces — the only
            // ordering stable across runs and platforms — and `--sortr=path` is exactly its
            // reverse, so the two share one comparator with the arguments swapped.
            match opts.sort_by_path {
                Some(crate::ops::PathSort::Ascending) => {
                    builder.sort_by_file_path(std::path::Path::cmp);
                }
                Some(crate::ops::PathSort::Descending) => {
                    builder.sort_by_file_path(|a, b| b.cmp(a));
                }
                None => {}
            }

            // `.fdignore` for fd / `.rgignore` for ripgrep. Both are inert until registered;
            // neither tool reads the other's. Custom ignore files outrank `.ignore` and EVERY
            // gitignore source (ignore 0.4.26 `dir.rs:580-585`:
            // `m_custom_ignore.or(m_ignore).or(m_gi).or(m_gi_exclude).or(m_global)
            // .or(m_explicit)`), so a `!keep.txt` negation in `.fdignore` re-includes a path a
            // `.gitignore` excluded — same as fd.
            //
            // Gated on `!opts.no_ignore`, and it MUST be a skip rather than an undo: `WalkBuilder`
            // exposes only `add_custom_ignore_filename`, with no setter that clears the list, so a
            // filename registered here can never be taken back.
            //
            // This is the half of `--no-ignore` that `ignore(false)` does not cover. `ignore(false)`
            // drops `.ignore`; the custom file is a SEPARATE source, and by the precedence chain
            // quoted just above it is the STRONGEST one — so leaving it registered meant
            // `--no-ignore` changed nothing at all in any tree containing a `.rgignore`. ripgrep
            // gates it the same way:
            // `if !self.no_ignore_dot { builder.add_custom_ignore_filename(".rgignore") }`
            // (`hiargs.rs:897-899`), and `--no-ignore` implies `--no-ignore-dot`.
            //
            // `--no-ignore-vcs` deliberately does NOT reach here: it implies neither
            // `no_ignore_dot` nor `no_ignore_parent`, so `.ignore`, the custom file and parent
            // traversal all keep applying for it. The two switches are asymmetric on purpose.
            if let Some(name) = opts.flavor.custom_ignore_filename()
                && !opts.no_ignore
            {
                builder.add_custom_ignore_filename(name);
            }

            // fd's GLOBAL ignore file (fd 10.5.0 `src/walk.rs:371-386`). Registered via
            // `add_ignore`, which lands in `explicit_ignores` — the LOWEST precedence source,
            // below the global gitignore (`dir.rs:585`), exactly as it is for fd. ripgrep has no
            // global ignore file, so this is gated on the fd flavor alone.
            if opts.flavor.reads_fd_global_ignore()
                && let Some(global) = crate::path::fd_global_ignore_file()
            {
                // fd prints a warning for a malformed pattern and KEEPS WALKING, with the rules
                // that did parse still in force (`ignore::Error::Partial`). pi buffers fd's stderr
                // but only surfaces it when fd exits non-zero AND produced no output
                // (find.ts:284-310), so that warning is invisible upstream on a successful run.
                // `walk` has no warning channel; dropping the error reproduces both halves.
                drop(builder.add_ignore(&global));
            }

            // `--ignore-file`. `add_ignore` lands in `explicit_ignores`, the LOWEST-precedence
            // source (ignore `dir.rs:585`) — which is where ripgrep puts it too, so a `.gitignore`
            // still outranks it. A file that does not parse is dropped rather than made fatal,
            // matching how the fd global ignore above is handled.
            for path in &opts.ignore_files {
                drop(builder.add_ignore(path));
            }

            // The caller's directory prune. `filter_entry` is the ONLY skip handle `ignore`
            // exposes, and it is honoured identically by `Walk` and `WalkParallel` (ignore 0.4.26
            // `walk.rs:973-979`, cloned into both at `walk.rs:616,640,1423`), which is what lets
            // the serial and parallel branches below share one prune and never drift in which
            // paths they visit. The predicate must be total: it is consulted for FILES too, and a
            // `false` there would drop the file.
            if let Some(prune) = opts.prune.clone() {
                builder.filter_entry(move |e| {
                    // Depth 0 is the search root and is never prunable. `Walk::skip_entry`
                    // short-circuits it (`walk.rs:1057-1059`) and `WalkParallel` pushes roots
                    // before any filter runs (`walk.rs:1355-1400`), so this guard is
                    // belt-and-braces on both — and load-bearing for any future walker that is
                    // neither.
                    if e.depth() == 0 {
                        return true;
                    }
                    // `DirEntry::is_dir` is `pub(crate)` (`walk.rs:102`), so the type is read
                    // through `file_type()` exactly as `to_walk_item` does.
                    if e.file_type().is_some_and(|t| t.is_dir()) {
                        return !prune.prunes(e.path());
                    }
                    true
                });
            }

            // `--sort=path` / `--sortr=path` force the SERIAL walk. `WalkBuilder::sort_by_file_path`
            // "is not used in the parallel iterator" (ignore 0.4.26 `walk.rs:899`) and
            // `WalkParallel` has no sort field at all (`walk.rs:1314-1325`), so a parallel walk
            // would silently drop the ordering — and with a match cap in force the ordering
            // decides WHICH matches the caller gets (`ops/mod.rs` `PathSort`). ripgrep resolves it
            // the same way: "sorting results currently always forces ripgrep to abandon
            // parallelism and run in a single thread" (`rg --help`, rg 15.1.0).
            //
            // Both branches share every setting above, including the `filter_entry` prune, which
            // `Walk` and `WalkParallel` honour identically — so the two differ in ORDER and
            // THREADS only, never in which paths they visit.
            if opts.sort_by_path.is_some() {
                for result in builder.build() {
                    if tx.blocking_send(to_walk_item(result)).is_err() {
                        break;
                    }
                }
                return;
            }

            // Threads default to `available_parallelism().min(12)` (`walk.rs:1434-1440`) — the
            // same heuristic ripgrep uses — so `.threads()` is deliberately not called.
            // `visit()` joins every worker under `std::thread::scope` before returning
            // (`walk.rs:1406-1430`): no thread outlives this `spawn_blocking` task.
            builder.build_parallel().run(|| {
                let tx = tx.clone();
                Box::new(move |result| {
                    // `blocking_send` — these are `std::thread::scope` threads with no tokio
                    // runtime context, so the bounded channel keeps applying back-pressure
                    // instead of panicking. A closed receiver means the consumer hit its cap or
                    // was cancelled, so `Quit` (not `Continue`) stops every remaining thread at
                    // once; a dropped receiver also makes an already-parked `blocking_send`
                    // return `Err` rather than hang.
                    //
                    // Nothing in here can panic: no `unwrap`, no indexing. `WalkParallel::visit`
                    // joins with `handle.join().unwrap()`, so a panicking visitor would surface
                    // as a panic in this blocking task.
                    match tx.blocking_send(to_walk_item(result)) {
                        Ok(()) => ignore::WalkState::Continue,
                        Err(_) => ignore::WalkState::Quit,
                    }
                })
            });
        });
        Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
    }
}
