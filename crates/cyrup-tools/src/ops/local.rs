//! The default local backend over `tokio::fs` / `tokio::process` (arch-03 §3.3, §6.5).
//!
//! `LocalFs` is an indirection over the real filesystem; `LocalProc` runs commands through the
//! detected shell, streams combined stdout+stderr, and kills on cancel/timeout. The two `ProcOps`
//! methods intentionally use DIFFERENT escalations, 1:1 with their DIFFERENT real Pi consumers:
//! [`LocalProc::exec`] backs both the `bash` tool (`bash.ts:97-99`'s `createLocalBashOperations`,
//! which spawns with `detached: process.platform !== "win32"`) and the immediate-bash RPC seam
//! (`bash-executor.ts:108`'s `executeBashWithOperations`, which calls the SAME `BashOperations`),
//! and both paths' abort/timeout handlers call `killProcessTree` (`shell.ts:200-225`) — an
//! IMMEDIATE `killpg(SIGKILL)` (negated pid, whole process GROUP), no `SIGTERM`, no grace period,
//! ever. [`LocalProc::exec_argv`] backs the WASM `exec` capability grant instead, whose real
//! consumer is `exec.ts:34-63`'s `execCommand`/`killProcess` — spawned with `shell: false` and NO
//! `detached` option, and killed via a bare `proc.kill("SIGTERM")`/`proc.kill("SIGKILL")` (Node's
//! `ChildProcess.kill()` always signals only `this.pid`, never a negated/group pid, regardless of
//! `detached`). So `exec_argv`'s escalation is a `SIGTERM`-then-grace-then-`SIGKILL`
//! **single-pid** signal — the SAME single-pid mechanism `cyrup-ext`'s `proc.rs::kill` already uses
//! for the unrelated long-lived `proc` capability ([`terminate_pid`]/[`kill_pid`], reused directly
//! here) — NOT a process-group kill. The only `unsafe` in the crate lives here, isolated to the
//! unix process-group calls (`setsid`/`killpg`, [`build_command`]/[`send_sigkill_tree`]/
//! [`kill_process_tree`], used ONLY by [`LocalProc::exec`] and its shutdown drain) and the
//! single-pid `kill(2)` calls ([`terminate_pid`]/[`kill_pid`]) with safety comments.
//!
//! [`LocalProc::exec`] additionally enrolls each spawned shell in the process-global
//! `TRACKED_DETACHED_CHILD_PIDS` registry, so a shutdown signal can `killpg` every detached bash
//! child still running BEFORE any teardown runs — Pi's `trackedDetachedChildPids` /
//! `killTrackedDetachedChildren` (`utils/shell.ts:180-195` @v0.83.0), drained as the first statement
//! of all three of its signal handlers. See [`kill_tracked_detached_children`] (SEAM-S03).

use super::{
    Access, ArgvOutput, ArgvSpec, DirEntry, ExecSpec, ExitStatus, FsOps, Meta, ProcOps, ShellConfig,
    Transport, WalkItem, WalkOpts,
};
use crate::error;
use cyrup_core::{CancelToken, EventStream, ToolError};
use ignore::WalkBuilder;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

/// Process-unique-ish suffix for temp files (no rng dependency).
pub(crate) fn unique_suffix() -> String {
    let pid = std::process::id();
    let n = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{pid:x}-{nanos:x}-{n:x}")
}

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
        tokio::fs::read(path).await.map_err(|e| error::io(&error::show(path), &e))
    }

    /// A real `std::fs::File`, so `grep`'s search pulls the file through `grep_searcher`'s rolling
    /// buffer instead of allocating it whole — the property Pi gets for free by running the search
    /// in a separate ripgrep process (grep.ts:226). The `open(2)` itself runs on the blocking pool;
    /// the reads happen inside the caller's `spawn_blocking`, per [`FsOps::read_stream`].
    async fn read_stream(
        &self,
        path: &Path,
    ) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
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
            && !parent.as_os_str().is_empty() {
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
                return Err(error::io_errno(&error::show(path), &std::io::Error::last_os_error()));
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
        let canonical = tokio::fs::canonicalize(path).await.unwrap_or_else(|_| path.to_path_buf());
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
                    out.push(DirEntry { name, path: entry.path() });
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

/// How long [`LocalProc::exec_argv`] waits after `SIGTERM` before escalating to `SIGKILL` — Pi's
/// exact `killProcess` timing (`exec.ts:56-61`: `setTimeout(..., 5000)`). Mirrors
/// `cyrup-ext::caps::proc::DEFAULT_KILL_GRACE`. NOT used by [`LocalProc::exec`] (the `bash`
/// tool/immediate-bash backend), which mirrors `killProcessTree`'s immediate, graceless `SIGKILL`
/// instead (`shell.ts:200-225`) — see the module doc comment.
const DEFAULT_KILL_GRACE: Duration = Duration::from_secs(5);

/// How long [`LocalProc::exec`]/[`LocalProc::exec_argv`] wait, once the child process ITSELF has
/// exited, for its inherited stdout/stderr pipe(s) to fall idle before giving up on reading them —
/// Pi's exact `EXIT_STDIO_GRACE_MS` (`child-process.ts:16`), armed by `waitForChildProcess`
/// (`child-process.ts:49-137`). A short-lived command that backgrounds a descendant (`sh -c "(sleep
/// 5 &); exit 0"`) leaves that descendant holding our stdout/stderr pipe open long after the
/// process we spawned has exited, so waiting on EOF alone hangs forever — the exact class of hang
/// Pi's grace timer exists to close (earendil-works/pi#5303). The timer is armed the instant the
/// child exits and re-armed on every subsequent data chunk, so an actively-writing descendant keeps
/// us reading, while a quiet inherited handle releases us after this much idle time.
const EXIT_STDIO_GRACE: Duration = Duration::from_millis(100);

/// Local process operations.
pub struct LocalProc {
    shell: ShellConfig,
    /// SIGTERM→SIGKILL grace period; overridable ONLY for tests ([`Self::with_kill_grace`]) so the
    /// escalation path is exercisable without a real test waiting 5+ real seconds — production
    /// always gets Pi's real 5s via [`Self::new`].
    kill_grace: Duration,
}

impl LocalProc {
    pub fn new(shell: ShellConfig) -> Self {
        Self::with_kill_grace(shell, DEFAULT_KILL_GRACE)
    }

    /// Build with a caller-supplied SIGTERM→SIGKILL grace period (tests only).
    pub fn with_kill_grace(shell: ShellConfig, kill_grace: Duration) -> Self {
        Self { shell, kill_grace }
    }
}

/// Build the OS command for an [`ExecSpec`], installing the unix process-group setup.
#[allow(unsafe_code)]
fn build_command(spec: &ExecSpec) -> std::process::Command {
    let mut std_cmd = std::process::Command::new(&spec.shell.program);
    std_cmd.args(&spec.shell.args);
    if spec.shell.transport == Transport::Argv {
        std_cmd.arg(&spec.command);
    }
    std_cmd.current_dir(&spec.cwd);
    // Removals FIRST, then the overrides — Pi deletes the session keys and only then repopulates
    // them (bash.ts:165-181), so a key in both lists ends up set, not unset.
    for k in &spec.env_remove {
        std_cmd.env_remove(k);
    }
    for (k, v) in &spec.env {
        std_cmd.env(k, v);
    }
    std_cmd.stdout(std::process::Stdio::piped());
    std_cmd.stderr(std::process::Stdio::piped());
    if spec.shell.transport == Transport::Stdin {
        std_cmd.stdin(std::process::Stdio::piped());
    } else {
        std_cmd.stdin(std::process::Stdio::null());
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` only detaches the child into its own session/process group before exec;
        // it touches no parent memory and is async-signal-safe. This makes the child the group
        // leader (pgid == pid) so the whole tree can be killed via `killpg` (R-03-027).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    std_cmd
}

/// Build the OS command for an [`ArgvSpec`] — a DIRECT argv (shell:false) exec (Pi `execCommand`
/// spawn with `shell:false`, exec.ts:41-45): the program IS `spec.program`, its args are the literal
/// `spec.args` (no shell, no word-splitting). Unlike [`build_command`] (the `bash`-tool/shell path,
/// whose real consumer `bash.ts:97-99` passes `detached: true`), this deliberately does NOT
/// `setsid` the child — Pi's real `execCommand` (`exec.ts:41-45`) never sets `detached` either, so
/// the spawned process stays in the caller's own process group and [`LocalProc::exec_argv`]'s
/// escalation targets it by single pid only ([`terminate_pid`]/[`kill_pid`]), never `killpg`.
fn build_argv_command(spec: &ArgvSpec) -> std::process::Command {
    let mut std_cmd = std::process::Command::new(&spec.program);
    std_cmd.args(&spec.args);
    // An empty `cwd` is skipped rather than passed to `current_dir` — matching Node's real
    // `child_process.spawn`, which treats a falsy `cwd` as "no override" and inherits the parent's
    // own ambient cwd (verified live: Node `spawn("pwd",[],{cwd:""})` exits 0, printing the ambient
    // cwd), the exact real-consumer behavior `execCommand` (`exec.ts:41-45`) relies on. Unlike Node,
    // `std::process::Command::current_dir("")` hard-fails the spawn (verified live: `Os { code: 2,
    // kind: NotFound, .. }`) — this callers-owned defensive check (this crate has no upstream
    // knowledge of WHY `spec.cwd` might be empty; `cyrup-session-svc::host_services::exec` already
    // folds a guest-supplied empty `cwd` back to the session cwd before building an `ArgvSpec`, so
    // this is defense in depth for any other/future caller of `exec_argv`) keeps that same graceful
    // degrade rather than erroring on a `PathBuf::new()`.
    if !spec.cwd.as_os_str().is_empty() {
        std_cmd.current_dir(&spec.cwd);
    }
    for (k, v) in &spec.env {
        std_cmd.env(k, v);
    }
    // Pi uses stdio `["ignore","pipe","pipe"]` (exec.ts:44): stdin closed, stdout+stderr piped.
    std_cmd.stdin(std::process::Stdio::null());
    std_cmd.stdout(std::process::Stdio::piped());
    std_cmd.stderr(std::process::Stdio::piped());
    // Deliberately NO `setsid`/process-group setup here — Pi's real `execCommand` spawn
    // (`exec.ts:41-45`) never sets `detached`, so the child stays in the caller's own process
    // group and must be signaled by single pid only, never `killpg` (see the doc comment above and
    // the module doc comment).
    std_cmd
}

/// Process-global registry of the detached bash children that are currently running — a literal
/// port of Pi's `const trackedDetachedChildPids = new Set<number>()`
/// (`packages/coding-agent/src/utils/shell.ts:180` @v0.83.0), whose own comment states the purpose:
/// "Detached child processes must be tracked so they can be killed on parent shutdown signals
/// (SIGHUP/SIGTERM)."
///
/// Filled at [`LocalProc::exec`]'s spawn and emptied when that exec finishes, mirroring Pi's two
/// call sites — `if (child.pid) trackDetachedChildPid(child.pid);` right after the
/// `detached: process.platform !== "win32"` spawn (`core/tools/bash.ts:108`) and
/// `if (child.pid) untrackDetachedChildPid(child.pid);` as the FIRST statement of that spawn's
/// `finally` (`bash.ts:142`). [`LocalProc::exec_argv`] deliberately does NOT participate: its real
/// consumer `execCommand` (`exec.ts:41-45`) passes no `detached` and Pi never tracks it.
///
/// Process-global on purpose, exactly as Pi's module-level `Set` is: it must survive session
/// replacement (`/new`, `/fork`, `switchSession`), which the per-session `session_cancel` route
/// [`send_sigkill_tree`] hangs off does not — that scoping difference is half of what SEAM-S03
/// records.
static TRACKED_DETACHED_CHILD_PIDS: std::sync::Mutex<std::collections::BTreeSet<u32>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

/// Lock the registry, ignoring poisoning.
///
/// A panic while the set is held cannot corrupt it (a `BTreeSet<u32>` has no invariant a partial
/// mutation can break) and this runs on the shutdown path, where refusing to kill orphans because
/// some unrelated task panicked is strictly worse than proceeding. Pi has no lock at all — JS is
/// single-threaded — so there is no upstream behaviour to mirror here, only a Rust obligation.
fn tracked_detached_child_pids()
-> std::sync::MutexGuard<'static, std::collections::BTreeSet<u32>> {
    TRACKED_DETACHED_CHILD_PIDS.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Pi `trackDetachedChildPid` (`utils/shell.ts:182-184` @v0.83.0), called at the bash spawn
/// (`bash.ts:108`).
pub fn track_detached_child_pid(pid: u32) {
    tracked_detached_child_pids().insert(pid);
}

/// Pi `untrackDetachedChildPid` (`utils/shell.ts:186-188` @v0.83.0), called from the bash spawn's
/// `finally` (`bash.ts:142`).
pub fn untrack_detached_child_pid(pid: u32) {
    tracked_detached_child_pids().remove(&pid);
}

/// Kill every still-running detached bash child and empty the registry — Pi
/// `killTrackedDetachedChildren` (`utils/shell.ts:190-195` @v0.83.0).
///
/// This is the FIRST statement of all three of Pi's signal handlers (`modes/print-mode.ts:55`,
/// `modes/rpc/rpc-mode.ts:373`, `modes/interactive/interactive-mode.ts:3663`) and of interactive's
/// two emergency paths (`emergencyTerminalExit` at `:3605`, the `uncaughtException` handler at
/// `:3631`), all @v0.83.0. It is synchronous and total: by the time anything can re-enter the
/// handler, the groups are already signalled.
///
/// CYRUP-DELTA — order of drain vs. kill. Pi loops the live `Set` and clears it AFTERWARDS
/// (`for (const pid of trackedDetachedChildPids) killProcessTree(pid); trackedDetachedChildPids
/// .clear();`, `shell.ts:191-194`). This takes the set out of the lock FIRST and kills without
/// holding it. Two Rust-only obligations force that: another thread's [`KillTreeOnDrop`] may be
/// blocked on `untrack_detached_child_pid` while this runs, so holding the lock across a
/// syscall-per-pid loop makes that thread wait on a shutdown path; and a re-entrant call would
/// deadlock a `std::sync::Mutex` where Pi's re-entered handler is merely a nested call over a
/// single-threaded `Set`. The observable result is identical — every pid present at entry is
/// killed, and none of them is present at exit.
pub fn kill_tracked_detached_children() {
    drain_and_kill(&TRACKED_DETACHED_CHILD_PIDS);
}

/// The body of [`kill_tracked_detached_children`], parameterised over the registry.
///
/// Split out ONLY so the drain can be tested against a registry the test owns. Calling the real
/// drain from a test would kill every detached child tracked by whatever else is running in the
/// same process — harmless under the nextest gate (one process per test), a cross-test SIGKILL
/// under a threaded `cargo test`, and this project's rule is not to introduce a flake.
fn drain_and_kill(registry: &std::sync::Mutex<std::collections::BTreeSet<u32>>) {
    let mut guard = registry.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let pids = std::mem::take(&mut *guard);
    drop(guard);
    for pid in pids {
        kill_process_tree(pid);
    }
}

/// Pi `killProcessTree` (`utils/shell.ts:200-225` @v0.83.0) addressed by PID ALONE, for the
/// [`kill_tracked_detached_children`] drain — which, unlike [`send_sigkill_tree`], holds no
/// `tokio::process::Child` for the pids it is killing.
///
/// Ports Pi's fallback, which [`send_sigkill_tree`] expresses differently: upstream tries
/// `process.kill(-pid, "SIGKILL")` and, if that THROWS, falls back to `process.kill(pid,
/// "SIGKILL")` (`shell.ts:214-224`) — a group kill can fail with `ESRCH` when the pid is not a
/// group leader, i.e. when the `setsid` never took effect. `send_sigkill_tree` reaches the same
/// place via its unconditional `child.start_kill()`; here there is no `Child`, so the fallback is
/// spelled out.
#[allow(unsafe_code)]
pub fn kill_process_tree(pid: u32) {
    #[cfg(unix)]
    {
        // SAFETY: `killpg(2)` and `kill(2)` read two integer arguments and touch no memory. A
        // failure (`ESRCH` — group or process already gone) is the expected benign outcome.
        let killed_group = unsafe { libc::killpg(pid as libc::pid_t, libc::SIGKILL) } == 0;
        if !killed_group {
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        // Pi's win32 arm is a fire-and-forget `spawn("taskkill", ["/F","/T","/PID", …], {stdio:
        // "ignore", detached: true, windowsHide: true})` (`shell.ts:203-212`) — NOT a blocking
        // wait, which matters because this runs inside a signal handler.
        let _ = std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn();
    }
}

/// Force-kill [`LocalProc::exec`]'s (the `bash`-tool/shell path's) child's whole process tree —
/// Pi's real `killProcessTree` (`shell.ts:200-225`: `process.kill(-pid, "SIGKILL")`), the ONLY step
/// of that escalation (no `SIGTERM`, no grace period, ever — see the module doc comment) —
/// R-03-024/027. NOT used by [`LocalProc::exec_argv`], which is single-pid
/// ([`terminate_pid`]/[`kill_pid`]) and DOES have a graceful `SIGTERM`-then-grace leg first; see the
/// module doc comment for why the two methods diverge.
#[allow(unsafe_code)]
fn send_sigkill_tree(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    {
        if let Some(pid) = child.id() {
            // SAFETY: send SIGKILL to the child's process group (created via `setsid`). A negative
            // pid / killpg targets the group; harmless if the group is already gone (ESRCH).
            unsafe {
                libc::killpg(pid as libc::pid_t, libc::SIGKILL);
            }
        }
    }
    #[cfg(not(unix))]
    {
        if let Some(pid) = child.id() {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .output();
        }
    }
    let _ = child.start_kill();
}

/// RAII group-kill for [`LocalProc::exec`], armed at spawn and disarmed only once the child has
/// been reaped on the normal path.
///
/// **This closes a JS→Rust mechanism gap, not a missing feature.** Pi's abort and timeout handlers
/// hang off an `async` function that ALWAYS settles: `bash.ts:111-121` registers `onAbort` →
/// `killProcessTree` (`shell.ts:200-225`, `process.kill(-pid, "SIGKILL")`) and the same handler runs
/// on the timeout leg, so upstream cannot reach a state where the shell's process GROUP outlives the
/// call. A Rust future has no such guarantee — it can be dropped at ANY `.await`: a cancelled
/// `tokio::spawn`, a `tokio::time::timeout`, an unwinding panic, or runtime teardown all abandon the
/// `select!` loop below without running a single one of its `send_sigkill_tree` arms.
///
/// `tokio::process`'s own `kill_on_drop(true)` is NOT a substitute and must not be mistaken for one:
/// it SIGKILLs the SINGLE direct child, so every grandchild the `setsid` group contains survives as
/// an orphan still holding this process's stdio pipes. That survival is already on the record as an
/// unfixed consequence in `docs/gap-analysis/12-upstream-drift-pi-core.md` (the `DRIFT-043`
/// rejection note: "grandchildren do survive — single-pid kill, not killpg"); this type is what
/// makes the drop path do what every non-drop path already does.
///
/// The pid cannot be recycled underneath the `killpg`: the guard is disarmed only AFTER the loop has
/// observed `child.wait()`, and until then [`LocalProc::exec`] still owns the un-reaped `Child`, so
/// the pid — and therefore the process-group id, which `setsid` made equal to it — remains ours.
/// Declared AFTER `child` in `exec` so Rust's reverse-declaration drop order runs this guard while
/// that ownership still holds.
/// It also owns the registry membership from [`TRACKED_DETACHED_CHILD_PIDS`], and that half is
/// deliberately NOT affected by [`Self::disarm`]. Pi untracks in a `finally` (`bash.ts:142`), which
/// runs on the normal return, the abort throw and the timeout throw alike; the Rust equivalent of
/// "runs no matter how we leave" is `Drop`, not a statement placed after the `select!` loop. Putting
/// the untrack on the success path instead would leak the pid PERMANENTLY whenever the future is
/// dropped mid-flight — the same class of gap this guard's `killpg` half already closes — and a
/// leaked pid is worse than a merely-forgotten one: the next
/// [`kill_tracked_detached_children`] would `killpg` a pid this process no longer owns and that the
/// kernel may since have recycled onto an unrelated process group.
#[cfg(unix)]
struct KillTreeOnDrop {
    pgid: Option<u32>,
    tracked: Option<u32>,
}

#[cfg(unix)]
impl KillTreeOnDrop {
    fn arm(pid: Option<u32>) -> Self {
        // Pi `bash.ts:108`: `if (child.pid) trackDetachedChildPid(child.pid);` — the `if` is why
        // this is keyed off the `Option` rather than a placeholder pid.
        if let Some(pid) = pid {
            track_detached_child_pid(pid);
        }
        Self { pgid: pid, tracked: pid }
    }

    /// The child has been reaped by the normal path; the group must NOT be signalled on drop.
    ///
    /// Registry membership is untouched here on purpose — see the type's doc comment. `Drop` still
    /// runs immediately afterwards (the guard is a local in [`LocalProc::exec`]), so the untrack is
    /// not deferred by disarming, only made unconditional.
    fn disarm(&mut self) {
        self.pgid = None;
    }
}

#[cfg(unix)]
#[allow(unsafe_code)]
impl Drop for KillTreeOnDrop {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            // SAFETY: identical to `send_sigkill_tree` — `killpg(2)` reads two integers and touches
            // no memory. `ESRCH` (group already gone) is the expected benign outcome and is ignored.
            unsafe {
                libc::killpg(pgid as libc::pid_t, libc::SIGKILL);
            }
        }
        // Pi's `finally` (`bash.ts:142`), unconditional on how this exec ended.
        if let Some(pid) = self.tracked {
            untrack_detached_child_pid(pid);
        }
    }
}

/// Windows has no process-group primitive that matches `setsid`; [`build_command`] installs none
/// there either, so the guard degrades to `kill_on_drop`'s single-pid behaviour — the same shape
/// the non-unix arm of [`send_sigkill_tree`] already documents.
///
/// The registry half is NOT degraded: Pi tracks on every platform (`bash.ts:108` is outside any
/// platform check, and its `killProcessTree` has a `taskkill /F /T` arm — `shell.ts:203-212` — that
/// kills a tree without needing a process group).
#[cfg(not(unix))]
struct KillTreeOnDrop {
    tracked: Option<u32>,
}

#[cfg(not(unix))]
impl KillTreeOnDrop {
    fn arm(pid: Option<u32>) -> Self {
        if let Some(pid) = pid {
            track_detached_child_pid(pid);
        }
        Self { tracked: pid }
    }
    fn disarm(&mut self) {}
}

#[cfg(not(unix))]
impl Drop for KillTreeOnDrop {
    fn drop(&mut self) {
        if let Some(pid) = self.tracked {
            untrack_detached_child_pid(pid);
        }
    }
}

/// Send SIGTERM to a SINGLE process by pid — NOT a process group (contrast [`send_sigkill_tree`],
/// which targets the whole `setsid` group [`LocalProc::exec`]'s shell-spawned tree needs,
/// R-03-027). This is the graceful half of a two-step escalation for a caller that owns exactly one
/// non-group-leader child directly: TWO real consumers share this exact mechanism — cyrup-ext's
/// long-lived `proc` capability (arch-08 §5.2/pi-mcp-adapter-port.md §3.1, which spawns a plain —
/// not `setsid`'d — child, mirroring the real `StdioClientTransport`'s non-detached spawn 1:1), and
/// [`LocalProc::exec_argv`] (the WASM `exec` capability grant, whose real consumer `exec.ts:34-63`'s
/// `execCommand`/`killProcess` never sets `detached` and signals via a bare, un-negated
/// `proc.kill("SIGTERM")`). A best-effort no-op on non-unix (no portable single-pid graceful-signal
/// primitive there without holding the `Child` itself, which this pid-only API deliberately doesn't
/// require); [`kill_pid`] is the forceful escalation that DOES work everywhere.
///
/// Returns whether a REAL graceful signal was actually sent: `Ok(true)` on unix (the `kill(2)` call
/// succeeded); `Ok(false)` on non-unix, where nothing was sent at all. Callers MUST skip any
/// post-call grace-period wait when this returns `Ok(false)` — waiting for a reaction to a signal
/// that was never sent only pays a needless delay before the (always-effective) forceful escalation,
/// with zero chance of the child ever reacting.
#[allow(unsafe_code)]
pub fn terminate_pid(pid: u32) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        // SAFETY: `kill(2)` only reads its two integer args (pid, signal); it touches no memory. A
        // non-zero return is an `errno` (e.g. `ESRCH` if the pid is already gone), surfaced as an
        // `io::Error`, never a panic.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if rc == 0 { Ok(true) } else { Err(std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Ok(false)
    }
}

/// Force-kill a SINGLE process by pid (SIGKILL / non-unix `taskkill /F /PID`, no `/T` — this
/// targets exactly the one pid, never a subtree; contrast [`send_sigkill_tree`]). The escalation
/// half of [`terminate_pid`]; works everywhere (unlike the graceful half). Shared by cyrup-ext's
/// `proc` capability and [`LocalProc::exec_argv`] — see [`terminate_pid`]'s doc comment.
#[allow(unsafe_code)]
pub fn kill_pid(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: same as `terminate_pid` — `kill(2)` reads two integers, touches no memory.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
        if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    {
        std::process::Command::new("taskkill").args(["/F", "/PID", &pid.to_string()]).output()?;
        Ok(())
    }
}

fn exit_from(status: std::process::ExitStatus) -> ExitStatus {
    match status.code() {
        Some(code) => ExitStatus::Exited(code),
        // No exit code ⇒ died to a signal we did not send (the cancel/timeout branches break with
        // their own statuses before reaching here). Pi maps this to `exitCode: null` ⇒ success.
        None => ExitStatus::Signaled,
    }
}

/// Read one chunk; `None` on EOF/error (or never resolves when the reader is absent).
async fn read_chunk<R: tokio::io::AsyncRead + Unpin>(reader: &mut Option<R>) -> Option<Vec<u8>> {
    match reader {
        Some(r) => {
            let mut buf = [0u8; 8192];
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => None,
                Ok(n) => Some(buf.get(..n).unwrap_or(&[]).to_vec()),
            }
        }
        None => std::future::pending().await,
    }
}

#[async_trait::async_trait]
impl ProcOps for LocalProc {
    async fn exec(
        &self,
        mut spec: ExecSpec,
        cancel: CancelToken,
        timeout: Option<Duration>,
        on_data: &mut (dyn for<'a> FnMut(&'a [u8]) + Send),
    ) -> Result<ExitStatus, ToolError> {
        if spec.shell.program.as_os_str().is_empty() {
            spec.shell = self.shell.clone();
        }
        // Pi checks `signal?.aborted` before EVER spawning (bash.ts:86-88: `if (signal?.aborted) {
        // throw new Error("aborted"); }`, ahead of even the cwd check below) — an already-cancelled
        // token must guarantee zero process execution, not just a kill-after-spawn race. Report it
        // as `Ok(Killed)`, the SAME outcome the mid-run cancel branch below reports (bash.ts's outer
        // catch maps BOTH the pre-spawn and the post-spawn `Error("aborted")` to the identical
        // `"Command aborted"` text, bash.ts:407-411) — every caller (`BashTool::execute`,
        // `bash.rs:315`; `run_bash`, `cyrup-session-svc/src/bash.rs:58`) already renders `Killed`
        // correctly, so this needs no new wiring.
        if cancel.is_cancelled() {
            return Ok(ExitStatus::Killed);
        }
        // Pi checks the cwd exists before spawning (bash.ts:70-74) so the model gets an actionable
        // message instead of a raw spawn error.
        if tokio::fs::metadata(&spec.cwd).await.is_err() {
            return Err(error::invalid(format!(
                "Working directory does not exist: {}\nCannot execute bash commands.",
                error::show(&spec.cwd)
            )));
        }
        let stdin_command =
            if spec.shell.transport == Transport::Stdin { Some(spec.command.clone()) } else { None };

        let std_cmd = build_command(&spec);
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            error::io(&format!("spawn {}", error::show(&spec.shell.program)), &e)
        })?;
        // Declared AFTER `child` on purpose: locals drop in reverse declaration order, so an
        // abandoned future runs this `killpg` while `child` is still un-reaped and the pid is still
        // ours. See [`KillTreeOnDrop`] for why `kill_on_drop` alone leaves the group behind.
        let mut kill_guard = KillTreeOnDrop::arm(child.id());

        if let Some(command) = stdin_command
            && let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(command.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();

        let timeout_fut = async {
            match timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(timeout_fut);

        // Immediate, graceless SIGKILL-the-whole-tree escalation — Pi's real `killProcessTree`
        // (`shell.ts:200-225`: `process.kill(-pid, "SIGKILL")`), called unconditionally by BOTH real
        // consumers of this method on abort/timeout: the `bash` tool's `onAbort`/timeout handler
        // (`bash.ts:111-121`) and the immediate-bash RPC seam (`bash-executor.ts:108` calling the
        // SAME `BashOperations.exec`). Unlike [`Self::exec_argv`] (which backs the WASM `exec`
        // capability's DIFFERENT real consumer, `exec.ts:52-63`'s `killProcess`), there is no
        // `SIGTERM`, no grace window, and no escalation step here — `pending` only records which
        // trigger (cancel vs timeout) asked for termination so the eventual `ExitStatus` reports the
        // right reason.
        let mut pending: Option<ExitStatus> = None;

        // Idle-grace fallback (Pi `waitForChildProcess`, `child-process.ts:49-137`): once the child
        // process itself exits, if its stdio isn't already fully drained (a backgrounded descendant
        // may still hold the pipe open), arm a timer that finalizes after `EXIT_STDIO_GRACE` of
        // silence — re-armed on every subsequent chunk — instead of waiting on EOF forever.
        let mut exit_status: Option<ExitStatus> = None;
        let mut idle_armed = false;
        let idle_grace = tokio::time::sleep(EXIT_STDIO_GRACE);
        tokio::pin!(idle_grace);

        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if pending.is_none() => {
                    send_sigkill_tree(&mut child);
                    pending = Some(ExitStatus::Killed);
                }
                _ = &mut timeout_fut, if pending.is_none() => {
                    send_sigkill_tree(&mut child);
                    pending = Some(ExitStatus::TimedOut);
                }
                _ = &mut idle_grace, if idle_armed => {
                    // The child exited and its inherited stdio has been quiet for
                    // `EXIT_STDIO_GRACE` — a still-open pipe held by a backgrounded descendant is
                    // not coming back with more output soon enough to justify hanging forever.
                    break match pending {
                        Some(reason) => reason,
                        None => exit_status.unwrap_or(ExitStatus::Exited(-1)),
                    };
                }
                chunk = read_chunk(&mut stdout) => {
                    match chunk {
                        Some(data) => {
                            on_data(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stdout = None;
                            if let Some(done) = exit_status
                                && stderr.is_none() {
                                    break pending.unwrap_or(done);
                                }
                        }
                    }
                }
                chunk = read_chunk(&mut stderr) => {
                    match chunk {
                        Some(data) => {
                            on_data(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stderr = None;
                            if let Some(done) = exit_status
                                && stdout.is_none() {
                                    break pending.unwrap_or(done);
                                }
                        }
                    }
                }
                s = child.wait(), if exit_status.is_none() => {
                    let mapped = match s {
                        Ok(st) => exit_from(st),
                        Err(_) => ExitStatus::Exited(-1),
                    };
                    exit_status = Some(mapped);
                    if stdout.is_none() && stderr.is_none() {
                        break pending.unwrap_or(mapped);
                    }
                    idle_armed = true;
                    idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                }
            }
        };
        // Every break above is reached only after `child.wait()` has been observed (`exit_status`
        // is `Some` on the EOF breaks, and `idle_armed` — the only gate on the idle-grace break —
        // is set in the `child.wait()` arm itself), so the child is reaped here and the group must
        // not be signalled again.
        kill_guard.disarm();
        Ok(status)
    }

    async fn exec_argv(
        &self,
        spec: ArgvSpec,
        cancel: CancelToken,
        timeout: Option<Duration>,
    ) -> Result<ArgvOutput, ToolError> {
        let std_cmd = build_argv_command(&spec);
        let mut cmd = tokio::process::Command::from(std_cmd);
        cmd.kill_on_drop(true);
        // Pi `execCommand` never rejects on a bad command — its caller maps a spawn failure to
        // `{code:1}` (exec.ts:99-105). Here we surface the spawn error and let the grant layer
        // (`LiveHostServices::exec`) apply that Pi mapping.
        let mut child =
            cmd.spawn().map_err(|e| error::io(&format!("spawn {}", spec.program), &e))?;

        let mut stdout = child.stdout.take();
        let mut stderr = child.stderr.take();
        // Buffer each stream separately (Pi accumulates `stdout`/`stderr` strings, exec.ts:47-48,81-87).
        let mut out_buf: Vec<u8> = Vec::new();
        let mut err_buf: Vec<u8> = Vec::new();

        let timeout_fut = async {
            match timeout {
                Some(d) => tokio::time::sleep(d).await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(timeout_fut);

        // SIGTERM-then-grace-then-SIGKILL escalation (Pi `killProcess`, `exec.ts:52-63`) — see
        // `LocalProc::exec`'s identical loop above for the full rationale.
        let mut pending: Option<ExitStatus> = None;
        let grace = tokio::time::sleep(self.kill_grace);
        tokio::pin!(grace);
        // Guards the grace arm below from re-firing on every subsequent loop iteration: a
        // `tokio::time::Sleep` that has already elapsed keeps reporting `Ready` on every re-poll
        // until it is `.reset()` to a future deadline, and (unlike the cancel/timeout arms above)
        // the grace arm below deliberately does NOT reset `grace` — it needs to fire exactly once.
        let mut sigkill_sent = false;

        // Idle-grace fallback (Pi `waitForChildProcess`, `child-process.ts:49-137`) — see
        // `LocalProc::exec`'s identical block above for the full rationale.
        let mut exit_status: Option<ExitStatus> = None;
        let mut idle_armed = false;
        let idle_grace = tokio::time::sleep(EXIT_STDIO_GRACE);
        tokio::pin!(idle_grace);

        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled(), if pending.is_none() => {
                    // SINGLE-PID SIGTERM (Pi's bare `proc.kill("SIGTERM")`, `exec.ts:55` — NEVER a
                    // negated/group pid; `exec.ts`'s spawn never sets `detached`, so there IS no
                    // group to target here, unlike `LocalProc::exec`'s group `send_sigkill_tree`).
                    // Skip the grace-period wait entirely when nothing was actually sent (non-unix,
                    // or the pid is already gone) — waiting it out has zero chance of a graceful
                    // exit landing, mirroring `ProcCaps::kill`'s identical fix for this bug class
                    // (`cyrup-ext/src/caps/proc.rs:748-751`, commit `0790ace`) — indeed the SAME
                    // `terminate_pid` call, reused directly rather than merely mirrored.
                    let sigterm_sent = child.id().is_some_and(|pid| terminate_pid(pid).unwrap_or(false));
                    pending = Some(ExitStatus::Killed);
                    let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };
                    grace.as_mut().reset(tokio::time::Instant::now() + wait);
                }
                _ = &mut timeout_fut, if pending.is_none() => {
                    let sigterm_sent = child.id().is_some_and(|pid| terminate_pid(pid).unwrap_or(false));
                    pending = Some(ExitStatus::TimedOut);
                    let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };
                    grace.as_mut().reset(tokio::time::Instant::now() + wait);
                }
                _ = &mut grace, if pending.is_some() && !sigkill_sent => {
                    // Grace elapsed (or was skipped outright, above) with NO natural exit yet (a
                    // graceful mid-grace exit already broke the loop via the `child.wait()`/drain
                    // arms below, which keep the REAL code). Force it — but, unlike a `break` here,
                    // KEEP DRAINING: the persistent `child.wait()` arm below still captures the REAL
                    // post-SIGKILL status (Pi: SIGKILL ⇒ `exit(null)` ⇒ `code ?? 0`, matched by
                    // `exit_from`'s no-code-⇒-`Signaled` mapping), and the `read_chunk` arms keep
                    // pumping whatever bytes the child already wrote into the pipe before it died.
                    // Breaking immediately here (the previous behavior) silently dropped exactly
                    // that trailing output — bytes already sitting in the kernel pipe buffer at the
                    // instant of SIGKILL are still readable via the still-open read end even after
                    // the writer is gone, but only if something keeps calling `read_chunk` — mirrors
                    // `LocalProc::exec`'s cancel/timeout arms above, which never `break` either.
                    // SINGLE-PID SIGKILL (Pi's bare `proc.kill("SIGKILL")`, `exec.ts:59` — same
                    // never-a-group-pid rationale as the SIGTERM arms above).
                    sigkill_sent = true;
                    if let Some(pid) = child.id() {
                        let _ = kill_pid(pid);
                    }
                }
                _ = &mut idle_grace, if idle_armed => {
                    // `idle_armed` is only set after `child.wait()` already captured the real
                    // status below, so `exit_status` is always `Some` here — Pi's `killed` never
                    // masks the real code (`child-process.ts:73-80`).
                    break exit_status.unwrap_or(ExitStatus::Exited(-1));
                }
                chunk = read_chunk(&mut stdout) => {
                    match chunk {
                        Some(data) => {
                            out_buf.extend_from_slice(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stdout = None;
                            if let Some(done) = exit_status
                                && stderr.is_none() {
                                    break done;
                                }
                        }
                    }
                }
                chunk = read_chunk(&mut stderr) => {
                    match chunk {
                        Some(data) => {
                            err_buf.extend_from_slice(&data);
                            if idle_armed {
                                idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                            }
                        }
                        None => {
                            stderr = None;
                            if let Some(done) = exit_status
                                && stdout.is_none() {
                                    break done;
                                }
                        }
                    }
                }
                s = child.wait(), if exit_status.is_none() => {
                    let mapped = match s {
                        Ok(st) => exit_from(st),
                        Err(_) => ExitStatus::Exited(-1),
                    };
                    exit_status = Some(mapped);
                    if stdout.is_none() && stderr.is_none() {
                        break mapped;
                    }
                    idle_armed = true;
                    idle_grace.as_mut().reset(tokio::time::Instant::now() + EXIT_STDIO_GRACE);
                }
            }
        };
        // `killed` (Pi's `killed` local, exec.ts:49) is set the instant a SIGTERM/SIGKILL
        // escalation is INITIATED — orthogonal to `status`, which always carries the REAL observed
        // code/signal outcome above (see `ArgvOutput` doc comment).
        Ok(ArgvOutput { status, stdout: out_buf, stderr: err_buf, killed: pending.is_some() })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use crate::ops::ArgvSpec;

    // Every caller builds an `sh`-based spec and so lives under `#[cfg(unix)]`; the helper itself
    // is portable. Silenced rather than gated so it stays available to any future Windows test.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn argv(program: &str, args: &[&str]) -> ArgvSpec {
        ArgvSpec {
            program: program.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            cwd: std::env::temp_dir(),
            env: Vec::new(),
        }
    }

    fn exec_spec(command: &str) -> ExecSpec {
        ExecSpec {
            command: command.to_string(),
            cwd: std::env::temp_dir(),
            env: Vec::new(),
            env_remove: Vec::new(),
            shell: ShellConfig::detect(),
        }
    }

    /// A reapable stand-in for the `while true; do sleep 1; done` body these `exec_argv` fixtures
    /// need, and the second half of the crate's `LEAK-FAIL` story (TOOL-042's residual).
    ///
    /// ## Why a fixture that leaves a one-second `sleep` behind turns an UNRELATED test red
    ///
    /// `exec_argv` deliberately kills by SINGLE PID and never `killpg` — pi's `execCommand` /
    /// `killProcess` (`exec.ts:34-63` @v0.83.0) spawns without `detached` and calls a bare,
    /// un-negated `proc.kill(...)`, so a grandchild of the spawned `sh` is upstream-correct to
    /// survive. `exec_argv_kill_signals_only_the_single_pid_never_the_process_group` exists to
    /// prove exactly that. The three fixtures below therefore leave a live `sleep 1` behind by
    /// DESIGN — and until this helper landed, none of them reaped it, so the process outlived the
    /// whole test binary by up to a second.
    ///
    /// That survivor is what converts nextest's per-test pipe accounting into a red on a test that
    /// spawns nothing at all. macOS has no `pipe2(2)`, so Rust's `anon_pipe` is `pipe(2)` followed
    /// by a separate `ioctl(FIOCLEX)` — the two are NOT atomic. **nextest** creates the stdout and
    /// stderr pipes for every test process in its own address space, from a thread pool, while
    /// concurrently spawning other test processes; a spawn landing inside another pipe's
    /// pre-`FIOCLEX` window inherits that pipe's WRITE end at some fd above 2, where no `dup2` in
    /// `build_argv_command` touches it. The test process that inherited it passes it on to every
    /// child it forks. When the inheriting test exits the stray fd goes with it — but a surviving
    /// GRANDCHILD keeps it, and the victim named by `LEAK-FAIL` is whichever test that pipe
    /// belonged to, which is why the victim is arbitrary and is usually a test with no spawn in it.
    /// Measured at HEAD before this fix: 16 leaks over ~120 runs of the spawn-dense subset, naming
    /// nine different victims including `read_variant_probe_uses_existence_not_readability`,
    /// `edit_rechecks_cancellation_after_the_write_lands` and the pure source-scan
    /// `shell_probe_loops_reap_on_the_error_arm_not_just_the_deadline` — none of which fork.
    ///
    /// cyrup owns neither half of the race (it is std's non-atomic CLOEXEC inside nextest's
    /// process), but it owns the AMPLIFIER: a stray fd is only observable for as long as some
    /// process holds it, and `leak-timeout` is 500 ms (`.config/nextest.toml:42`). A grandchild
    /// reaped before its fixture returns closes the window; a `sleep 1` left running does not.
    ///
    /// ## The shape
    ///
    /// [`Self::reapable_sleep_loop`] keeps the loop semantics the fixtures assert on — a shell that
    /// stays alive across signals with a real forked descendant — and adds only `echo $!` into a
    /// marker file, the same record-and-reap pattern
    /// `a_dropped_exec_future_kills_the_whole_process_group_not_just_the_direct_child` and
    /// `exec_argv_does_not_hang_on_a_backgrounded_descendant_holding_the_pipe_open` already use.
    #[cfg(unix)]
    struct SleeperMarker {
        path: std::path::PathBuf,
    }

    #[cfg(unix)]
    impl SleeperMarker {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("cyrup-sleeper-{tag}-{}.pid", std::process::id()));
            let _ = std::fs::remove_file(&path);
            Self { path }
        }

        /// `<prefix>while true; do sleep 1 & echo $! > MARKER; wait; done` — behaviourally the same
        /// forever-loop-over-a-forked-`sleep` as the literal it replaces (`wait` with no argument
        /// waits for the one background job, and is interrupted by a trapped signal exactly as the
        /// foreground `sleep` was), with the current descendant's pid recorded so [`Self::reap`]
        /// can kill it.
        fn reapable_sleep_loop(&self, prefix: &str) -> String {
            format!(
                "{prefix}while true; do sleep 1 & echo $! > {}; wait; done",
                self.path.display()
            )
        }

        /// SIGKILL the recorded descendant and WAIT for it to actually be gone, if it got as far as
        /// recording itself. Best-effort by construction: the marker is absent when the shell was
        /// killed before its first iteration, which is the case where there is nothing to reap.
        ///
        /// The bounded wait is the load-bearing half. `kill(2)` only QUEUES the signal; the fds the
        /// process holds are released when it is torn down, which is what the leak window cares
        /// about — returning the instant `kill` returns would leave exactly the race this helper
        /// exists to close. Bounded rather than unbounded so a reap that cannot complete degrades to
        /// today's behaviour instead of hanging the test.
        fn reap(&self) {
            if let Ok(text) = std::fs::read_to_string(&self.path)
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                let _ = kill_pid(pid);
                let deadline = std::time::Instant::now() + Duration::from_millis(500);
                while pid_exists(pid) && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
            let _ = std::fs::remove_file(&self.path);
        }
    }

    /// Liveness probe that does NOT perturb what it measures: `kill(pid, 0)` performs the
    /// permission/existence check and delivers nothing. `terminate_pid` cannot be used here — its
    /// `SIGTERM` would kill a `sleep` that the assertion needs to observe as still alive.
    #[cfg(unix)]
    #[allow(unsafe_code)]
    fn pid_exists(pid: u32) -> bool {
        // SAFETY: `kill(2)` with signal 0 reads its two integer arguments and touches no memory.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    /// THE JS→Rust mechanism gap [`KillTreeOnDrop`] closes: pi's abort/timeout handling hangs off an
    /// `async` function that always settles (`bash.ts:111-121` → `killProcessTree`,
    /// `shell.ts:200-225`), so the shell's process GROUP can never outlive the call. A Rust future
    /// can be dropped at any `.await` — here by `tokio::time::timeout`, but equally by a cancelled
    /// `tokio::spawn`, an unwinding panic, or runtime teardown — and every `send_sigkill_tree` arm
    /// in `exec`'s `select!` is then simply never reached.
    ///
    /// RED before the guard: `kill_on_drop(true)` SIGKILLs the direct `setsid` shell ONLY, so the
    /// backgrounded `sleep 30` in its process group survives the drop for its full 30s (recorded as
    /// an unfixed consequence in `12-upstream-drift-pi-core.md`'s `DRIFT-043` rejection note —
    /// "grandchildren do survive — single-pid kill, not killpg"). GREEN after: the group is
    /// `killpg`'d on the drop path exactly as it is on every non-drop path.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_exec_future_kills_the_whole_process_group_not_just_the_direct_child() {
        let proc = LocalProc::new(ShellConfig::detect());
        let marker = std::env::temp_dir()
            .join(format!("cyrup-exec-dropguard-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // The shell backgrounds a descendant in its own (`setsid`) process group and then blocks, so
        // the future is still mid-`select!` when the timeout below drops it.
        let spec = exec_spec(&format!("sleep 30 & echo $! > {}; wait", marker.display()));

        let elapsed = tokio::time::timeout(
            Duration::from_millis(500),
            proc.exec(spec, CancelToken::new(), None, &mut |_data: &[u8]| {}),
        )
        .await;
        assert!(
            elapsed.is_err(),
            "fixture: the command must still be running when the timeout DROPS the future — \
             otherwise this test observes a normal return, not the drop path"
        );

        let descendant: u32 = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .expect("fixture: the backgrounded descendant must have recorded its pid");
        let _ = std::fs::remove_file(&marker);

        // A `killpg`'d process is a zombie until its (now-dead) parent's reaper collects it, and
        // `kill(pid, 0)` succeeds on a zombie, so poll rather than sampling once. A survivor would
        // stay observable for the full 30s, so this bound discriminates by a wide margin.
        let mut gone = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if !pid_exists(descendant) {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        // Cleanup BEFORE the assertion so a failing run does not itself leak the `sleep 30` it is
        // complaining about.
        let _ = kill_pid(descendant);
        assert!(
            gone,
            "dropping the `exec` future must `killpg` the whole `setsid` group — the backgrounded \
             descendant (pid {descendant}) outlived the drop, which is `kill_on_drop`'s single-pid \
             behaviour, not pi's `killProcessTree`"
        );
    }

    /// `LocalProc::exec` (the `bash` tool / immediate-bash backend) must SIGKILL a SIGTERM-ignoring
    /// tree IMMEDIATELY on timeout — Pi's real `killProcessTree` (`shell.ts:200-225`), called
    /// directly by `bash.ts:118-121`'s timeout handler with NO intervening `SIGTERM`/grace step.
    /// Configuring an intentionally huge `kill_grace` (5s, the SAME value `exec_argv` actually
    /// waits out) and still finishing in well under a second proves `exec` never consults
    /// `kill_grace` at all — the exact regression this fix closes (it previously reused
    /// `exec_argv`'s `SIGTERM`-then-5s-grace-then-`SIGKILL` escalation, giving a SIGTERM-ignoring
    /// child up to 5s of extra unsupervised runtime Pi's bash tool never grants).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_timeout_sigkills_a_sigterm_ignoring_child_immediately_no_grace() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let started = tokio::time::Instant::now();
        let status = proc
            .exec(
                exec_spec("trap '' TERM; while true; do sleep 1; done"),
                CancelToken::new(),
                Some(Duration::from_millis(100)),
                &mut |_data: &[u8]| {},
            )
            .await
            .expect("exec runs");
        assert_eq!(
            status,
            ExitStatus::TimedOut,
            "the timeout reason is reported even though the tree never gracefully exited"
        );
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a SIGTERM-ignoring tree must still die within ~100ms of the timeout via immediate \
             SIGKILL — no 5s grace wait like `exec_argv`'s `killProcess` escalation — got {:?}",
            started.elapsed()
        );
    }

    /// The same immediate-SIGKILL behavior on the `cancel` path (Pi `bash.ts:111-113`'s `onAbort`,
    /// which also calls `killProcessTree` directly with no grace step).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_cancel_sigkills_a_sigterm_ignoring_child_immediately_no_grace() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let cancel = CancelToken::new();
        let started = tokio::time::Instant::now();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let spec = exec_spec("trap '' TERM; while true; do sleep 1; done");
            async move { proc.exec(spec, cancel, None, &mut |_data: &[u8]| {}).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        let status = task.await.expect("task joins").expect("exec runs");
        assert_eq!(status, ExitStatus::Killed, "the cancel reason is reported");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a SIGTERM-ignoring tree must still die within ~100ms of cancel via immediate SIGKILL, \
             got {:?}",
            started.elapsed()
        );
    }

    /// An ALREADY-cancelled token must never spawn a process at all — Pi's real
    /// `createLocalBashOperations.exec` checks `signal?.aborted` and throws BEFORE calling `spawn()`
    /// (`bash.ts:86-88`), ahead of even the cwd-exists check.
    ///
    /// TOOL-030: proven WITHOUT any wall-clock bound. The cwd is deliberately a path that does not
    /// exist, which makes the short-circuit's position observable rather than merely fast: the
    /// cancel check at `LocalProc::exec` sits strictly BEFORE the `Working directory does not
    /// exist` guard, which itself sits before `spawn()`. So
    ///   * short-circuit present ⇒ `Ok(ExitStatus::Killed)` (this assertion),
    ///   * short-circuit removed ⇒ `Err("Working directory does not exist: …")`, and
    ///   * short-circuit moved after `spawn()` ⇒ still `Err`, since the spawn itself fails.
    /// No ordering other than Pi's can produce `Ok(Killed)` here. The marker check is kept as a
    /// belt-and-braces witness (it is NOT sufficient on its own — a child that spawned and was
    /// killed before `touch` completed also leaves it absent).
    #[tokio::test]
    async fn exec_pre_cancelled_never_spawns() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let marker = std::env::temp_dir().join(format!("cyrup-exec-precancel-{}", unique_suffix()));
        let missing_cwd =
            std::env::temp_dir().join(format!("cyrup-exec-precancel-cwd-{}", unique_suffix()));
        assert!(!missing_cwd.exists(), "the sentinel cwd must not exist");
        let cancel = CancelToken::new();
        cancel.cancel();
        let spec = ExecSpec {
            cwd: missing_cwd,
            ..exec_spec(&format!("touch {}", marker.display()))
        };
        let status = proc
            .exec(spec, cancel, None, &mut |_data: &[u8]| {})
            .await
            .expect(
                "a pre-cancelled exec resolves Ok(Killed) — reaching the cwd-exists guard or \
                 `spawn()` at all would have produced Err",
            );
        assert_eq!(status, ExitStatus::Killed, "pre-cancelled reports the same reason as mid-run cancel");
        assert!(
            !marker.exists(),
            "the shell command must NEVER have run — an already-cancelled token guarantees zero \
             process execution, matching Pi's pre-spawn `signal?.aborted` check"
        );
    }

    /// A normal (SIGTERM-obeying) child dies well within the grace period on timeout — no SIGKILL
    /// escalation needed. Guards against a regression that makes EVERY timeout/cancel wait out the
    /// full grace period regardless of whether the tree already died (mirrors
    /// `cyrup_ext::caps::proc::kill_terminates_a_real_running_child_and_the_os_process_is_gone`).
    ///
    /// `sleep` does not trap SIGTERM, so it dies to the RAW signal (no exit code) — Pi's own
    /// `code ?? 0` null-coalescing case (`exec.ts:97`) — which `exit_from` reports as `Signaled`;
    /// `killed` is still `true` because a termination WAS initiated (orthogonal to `status`, see
    /// `ArgvOutput`'s doc comment). This must NOT collapse to the bare `TimedOut` reason tag — that
    /// was the bug (a real terminal status discarded whenever `pending` was `Some`).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_timeout_kills_a_normal_child_well_within_grace() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let started = tokio::time::Instant::now();
        let out = proc
            .exec_argv(argv("sleep", &["30"]), CancelToken::new(), Some(Duration::from_millis(200)))
            .await
            .expect("exec_argv runs");
        assert_eq!(
            out.status,
            ExitStatus::Signaled,
            "the REAL observed status (died to the raw signal) is reported, not the bare TimedOut tag"
        );
        assert!(out.killed, "a timeout-initiated kill is still `killed`, independent of `status`");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "a SIGTERM-obeying child (`sleep`) must die well within the 5s grace period, got {:?}",
            started.elapsed()
        );
    }

    /// L4 round-12 finding #3: an [`ArgvSpec`] with an EMPTY `cwd` must NOT hard-fail the spawn —
    /// `build_argv_command` skips `current_dir` entirely for an empty path, matching Node's real
    /// `child_process.spawn`, which treats a falsy `cwd` as "no override" and inherits the parent's
    /// own ambient cwd (verified live: Node `spawn("pwd",[],{cwd:""})` exits 0), unlike
    /// `std::process::Command::current_dir("")`, which hard-fails with `Os { code: 2, kind:
    /// NotFound, .. }` (also verified live). Proven by actually running `pwd` with `cwd:
    /// PathBuf::new()` and reading its REAL stdout: it must equal THIS TEST PROCESS's own ambient
    /// cwd (Rust's `Command` default when `.current_dir()` is never called at all).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_with_an_empty_cwd_inherits_the_ambient_cwd_instead_of_hard_failing() {
        let proc = LocalProc::new(ShellConfig::detect());
        let spec = ArgvSpec {
            program: "pwd".to_string(),
            args: Vec::new(),
            cwd: std::path::PathBuf::new(),
            env: Vec::new(),
        };
        let out = proc
            .exec_argv(spec, CancelToken::new(), None)
            .await
            .expect("exec_argv must not hard-fail on an empty cwd");
        assert_eq!(out.status, ExitStatus::Exited(0), "pwd must run and exit cleanly");
        let printed =
            std::fs::canonicalize(String::from_utf8_lossy(&out.stdout).trim_end()).unwrap_or_default();
        let ambient = std::env::current_dir().expect("this test process has a cwd");
        assert_eq!(
            printed,
            std::fs::canonicalize(&ambient).unwrap_or(ambient),
            "an empty cwd must inherit the ambient process cwd, not error or default to something else"
        );
    }

    /// THE regression this fix closes: a well-behaved child that TRAPS SIGTERM and exits itself
    /// with its OWN real, nonzero exit code mid-grace must have that REAL code reported — 1:1 with
    /// Pi's `waitForChildProcess`/`finalize(exitCode)` (`child-process.ts:73-80`), which always
    /// resolves with the actual observed `code`, `killed` bolted on separately (`exec.ts:97`). The
    /// old cyrup behavior collapsed this to a hard-coded `code 0` any time a kill was in flight.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_timeout_preserves_the_real_code_of_a_graceful_sigterm_handler() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let sleeper = SleeperMarker::new("gracefulterm");
        let out = proc
            .exec_argv(
                argv("sh", &["-c", &sleeper.reapable_sleep_loop("trap 'exit 7' TERM; ")]),
                CancelToken::new(),
                Some(Duration::from_millis(200)),
            )
            .await
            .expect("exec_argv runs");
        assert_eq!(
            out.status,
            ExitStatus::Exited(7),
            "the child's OWN real exit code from its SIGTERM handler must survive, not be \
             discarded to 0 because a kill was in flight"
        );
        assert!(out.killed, "a timeout-initiated kill is still `killed`, independent of `status`");
        sleeper.reap();
    }

    /// The FORCED SIGKILL escalation, exercised deterministically (mirrors
    /// `cyrup_ext::caps::proc::kill_escalates_to_sigkill_when_the_child_ignores_sigterm`): a
    /// process-group leader that traps SIGTERM and loops forever ignores the graceful signal
    /// outright, so `exec_argv`'s timeout branch MUST wait out the (test-shortened) grace period
    /// and then SIGKILL the whole group — which cannot be ignored — to actually terminate it.
    /// Proves the escalation is real, not just documented (closes L4 §2.4).
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_timeout_escalates_to_sigkill_when_the_child_ignores_sigterm() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(150));
        let sleeper = SleeperMarker::new("argvtimeoutkill");
        let started = tokio::time::Instant::now();
        let out = proc
            .exec_argv(
                argv("sh", &["-c", &sleeper.reapable_sleep_loop("trap '' TERM; ")]),
                CancelToken::new(),
                Some(Duration::from_millis(100)),
            )
            .await
            .expect("exec_argv runs");
        sleeper.reap();
        assert_eq!(
            out.status,
            ExitStatus::Signaled,
            "a forced SIGKILL reports the real (signal, no code) status, not the bare TimedOut tag"
        );
        assert!(out.killed, "a timeout-initiated kill is still `killed`");
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the 100ms timeout + 150ms grace period was genuinely waited out before escalating to \
             SIGKILL, got {:?}",
            started.elapsed()
        );
    }

    /// The same escalation on the `cancel` path (not just `timeout`): an abort mid-run SIGTERMs
    /// first, and only SIGKILLs the SIGTERM-ignoring group after the grace period elapses.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_cancel_escalates_to_sigkill_when_the_child_ignores_sigterm() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(150));
        let sleeper = SleeperMarker::new("argvcancelkill");
        let cancel = CancelToken::new();
        let started = tokio::time::Instant::now();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let spec = argv("sh", &["-c", &sleeper.reapable_sleep_loop("trap '' TERM; ")]);
            async move { proc.exec_argv(spec, cancel, None).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        let out = task.await.expect("task joins").expect("exec_argv runs");
        sleeper.reap();
        assert_eq!(
            out.status,
            ExitStatus::Signaled,
            "a forced SIGKILL reports the real (signal, no code) status, not the bare Killed tag"
        );
        assert!(out.killed, "a cancel-initiated kill is still `killed`");
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "the grace period was genuinely waited out before escalating to SIGKILL, got {:?}",
            started.elapsed()
        );
    }

    /// THE regression this fix closes: bytes already sitting in the kernel pipe buffer at the
    /// instant the grace-elapsed arm forces a SIGKILL must NOT be silently dropped. The old code
    /// sent SIGKILL, `child.wait()`ed, and `break`ed immediately — never re-polling `read_chunk` —
    /// so whatever the child had already written but this loop hadn't yet drained was lost.
    ///
    /// Ground-truth harness: a SIGTERM-ignoring child appends an increasing counter to an
    /// independent file via an fd opened ONCE (`exec 3>>`, so the loop spins as fast as the shell
    /// can manage rather than being disk-syscall-bound), written BEFORE the matching stdout
    /// `printf` each iteration with no `sleep`, so the file's last line is always >= whatever made
    /// it to stdout. With the fix, `read_chunk` keeps draining until TRUE EOF (the kernel only
    /// signals EOF once every byte written before the writer's fd closed has been delivered to the
    /// reader) — so captured stdout can lag the ground truth by AT MOST the single in-flight
    /// iteration straddling the SIGKILL instant, never by a whole buffered chunk's worth. Repeated
    /// several times since the exact SIGKILL timing relative to the child's write cadence is
    /// inherently racy — verified live: with this exact script reverted to the pre-fix
    /// (immediate-`break`) behavior, this test failed deterministically on trial 0 across 3
    /// separate runs (deficits of 2-3 lines each); with the fix, 5 separate runs (40 trials total)
    /// were all clean.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_forced_sigkill_does_not_drop_buffered_stdout_already_sitting_in_the_pipe() {
        for trial in 0..8u32 {
            let gt_path = std::env::temp_dir()
                .join(format!("cyrup-exec-argv-gt-{}-{trial}.txt", std::process::id()));
            let _ = std::fs::remove_file(&gt_path);
            let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(15));
            // fd 3 is opened ONCE (`exec 3>>`) rather than re-opened every iteration (a fresh
            // `>>` open/write/close per line is disk-syscall-bound and slow enough that the async
            // reader never falls behind) — this lets the loop spin as fast as the shell can manage
            // (bounded only by `printf`/arithmetic), maximizing how many iterations land inside the
            // short grace window and thus the odds of catching the exact SIGKILL race.
            let script = format!(
                "exec 3>>{}; trap '' TERM; i=0; while true; do printf '%s\\n' \"$i\" >&3; \
                 printf '%s\\n' \"$i\"; i=$((i+1)); done",
                gt_path.display()
            );
            // TOOL-030/TOOL-020: the RUN window (250ms) is decoupled from the KILL GRACE (15ms,
            // configured above). The grace is what this test exercises — SIGTERM is trapped, so
            // the forced-SIGKILL escalation still fires 15ms after the timeout — while the run
            // window only has to guarantee the child completed at least one loop iteration before
            // being killed. At 15ms that guarantee was a scheduling gamble (fork + exec of
            // `/bin/sh` plus one iteration inside roughly 30ms); at 250ms it holds by construction
            // on any host that can start a process at all, with the SIGKILL race the test is
            // actually about completely unchanged.
            let out = proc
                .exec_argv(
                    argv("sh", &["-c", &script]),
                    CancelToken::new(),
                    Some(Duration::from_millis(250)),
                )
                .await
                .expect("exec_argv runs");
            assert!(out.killed, "trial {trial}: a SIGTERM-ignoring child must be force-killed");

            let ground_truth = std::fs::read_to_string(&gt_path).unwrap_or_default();
            let _ = std::fs::remove_file(&gt_path);
            let gt_last: i64 =
                ground_truth.lines().next_back().and_then(|l| l.parse().ok()).unwrap_or(-1);
            let captured = String::from_utf8_lossy(&out.stdout);
            let stdout_last: i64 =
                captured.lines().next_back().and_then(|l| l.parse().ok()).unwrap_or(-1);

            assert!(
                gt_last >= 0,
                "trial {trial}: the child must have run at least one loop iteration before being \
                 killed (ground truth file was empty) — with a 250ms run window this is a real \
                 failure, not the scheduling race the old 15ms window made it"
            );
            // TOOL-020 claimed this bound "assumes the host `ShellConfig::detect()` shell flushes
            // stdout once per iteration". That half is REFUTED at HEAD: `exec_argv` runs the
            // program it is handed, and this call hands it `argv("sh", …)` literally — the
            // `ShellConfig::detect()` passed to `with_kill_grace` is only consulted by `exec`, not
            // by `exec_argv`. The dependence is on `/bin/sh`'s builtin `printf`, which flushes per
            // command, and is identical on every POSIX host.
            assert!(
                gt_last - stdout_last <= 1,
                "trial {trial}: captured stdout (last line {stdout_last}) lagged the ground-truth \
                 file (last line {gt_last}) by more than the one single in-flight iteration the \
                 SIGKILL can legitimately straddle — buffered pipe bytes were dropped at the \
                 forced-SIGKILL boundary"
            );
        }
    }

    /// `terminate_pid`'s `bool` return is the signal callers (`cyrup_ext::caps::proc::ProcCaps::kill`)
    /// rely on to decide whether to wait out a grace period at all — `Ok(true)` on unix means a REAL
    /// `SIGTERM` was sent (so waiting for a reaction is meaningful), verified here by actually
    /// terminating a real spawned child and confirming it dies within the standard grace window.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn terminate_pid_reports_true_and_the_real_process_dies() {
        let mut child = tokio::process::Command::new("sleep")
            .arg("30")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("sleep spawns");
        let pid = child.id().expect("spawned child has a pid");

        let sent = terminate_pid(pid).expect("SIGTERM send succeeds");
        assert!(sent, "unix terminate_pid must report a real signal was sent");

        let status = tokio::time::timeout(Duration::from_secs(2), child.wait())
            .await
            .expect("the SIGTERM-obeying child dies within the grace window")
            .expect("wait succeeds");
        assert!(!status.success(), "a SIGTERM-terminated child does not exit successfully");
    }

    /// L4 round-17 finding #1: `exec_argv`'s kill escalation MUST signal only the single spawned
    /// pid — Pi's real `execCommand`/`killProcess` (`exec.ts:34-63`) spawns with no `detached`
    /// option and kills via a bare, un-negated `proc.kill("SIGTERM"/"SIGKILL")`, which Node always
    /// delivers to `this.pid` alone, never a process group. Proven by actually spawning a SIBLING
    /// process in the exact same process group as the `exec_argv`-spawned command (both inherit
    /// THIS TEST's own group, since [`build_argv_command`] deliberately does not `setsid`), letting
    /// `exec_argv`'s timeout escalate all the way to `SIGKILL`, and confirming the sibling survived.
    /// The regression this guards against (`killpg` targeting the whole group) would have killed
    /// this sibling as collateral damage — and, worse, in production would signal the WASM guest
    /// engine's own ambient process group, since a real `exec_argv` caller is never `setsid`'d either.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_kill_signals_only_the_single_pid_never_the_process_group() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_millis(150));
        let marker = std::env::temp_dir()
            .join(format!("cyrup-exec-argv-singlepid-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // The outer `sh` is what `exec_argv` directly spawns and kills; it backgrounds a SIBLING
        // `sleep 30` (via `&`, no `setsid`) that inherits the SAME process group and writes its own
        // pid to `marker` before the outer shell blocks on `wait`.
        let script = format!("sleep 30 & echo $! > {}; wait", marker.display());
        let out = proc
            .exec_argv(argv("sh", &["-c", &script]), CancelToken::new(), Some(Duration::from_millis(100)))
            .await
            .expect("exec_argv runs");
        assert!(out.killed, "the timeout must have initiated a kill");

        let sibling_pid: u32 = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Ok(s) = std::fs::read_to_string(&marker)
                    && let Ok(pid) = s.trim().parse()
                {
                    return pid;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the background sibling must have written its pid before the outer shell died");
        let _ = std::fs::remove_file(&marker);

        // `terminate_pid` doubles as the liveness probe AND cleanup here: `Ok(true)` means the
        // sibling was genuinely still alive (proving `exec_argv`'s kill never reached it) and also
        // terminates it so the test doesn't leak a real `sleep 30` process; `Err` (`ESRCH`) would
        // mean it was already dead — exactly what the group-kill regression would cause.
        let sibling_was_alive = terminate_pid(sibling_pid).unwrap_or(false);
        assert!(
            sibling_was_alive,
            "a same-process-group sibling of the exec_argv-spawned command must survive its \
             SIGTERM/SIGKILL escalation — exec_argv's kill must target only the single spawned \
             pid, mirroring Pi's real execCommand/killProcess (exec.ts:34-63), never `killpg`"
        );
    }

    /// Reproduces the exact hang class Pi's `EXIT_STDIO_GRACE_MS` idle timer exists to close
    /// (`waitForChildProcess`, `child-process.ts:49-137`, earendil-works/pi#5303): the spawned
    /// command backgrounds a descendant (`sleep 5 &`) that inherits our stdout pipe and then exits
    /// itself immediately. Without an idle-grace fallback, `child.wait()` never runs (gated on both
    /// streams reaching EOF) and the still-open pipe never reaches EOF either — an unconditional
    /// hang. With the fix, the loop must finalize within `EXIT_STDIO_GRACE` of the parent's own
    /// exit, well under the backgrounded descendant's 5s lifetime.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_does_not_hang_on_a_backgrounded_descendant_holding_the_pipe_open() {
        let proc = LocalProc::new(ShellConfig::detect());
        // The descendant records its own pid so this fixture can REAP it. The pipe-holding shape it
        // exists to prove is unchanged — `sleep` is still backgrounded out of a subshell that exits
        // immediately, still inherits the exec stdout/stderr pipes, and is still alive for the whole
        // of the assertion window below — but it no longer survives the test process by ~4.9s. A
        // fixture that deliberately orphans a process is the exact "spawns and does not reap" shape
        // the surrounding suite is being audited for, and under `cargo nextest run` a survivor with
        // inherited handles is what turns into a `LEAK-FAIL`.
        let marker = std::env::temp_dir()
            .join(format!("cyrup-exec-argv-idlegrace-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        let script = format!("( sleep 5 & echo $! > {} ) ; exit 0", marker.display());
        let started = tokio::time::Instant::now();
        let out = tokio::time::timeout(
            Duration::from_secs(3),
            proc.exec_argv(argv("sh", &["-c", &script]), CancelToken::new(), None),
        )
        .await
        .expect("exec_argv must not hang past the idle-grace fallback")
        .expect("exec_argv runs");
        assert_eq!(out.status, ExitStatus::Exited(0), "the parent's own clean exit is reported");
        assert!(!out.killed, "a natural exit with no cancel/timeout is never `killed`");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "must finalize within EXIT_STDIO_GRACE of the parent's exit, not wait on the \
             backgrounded descendant's pipe, got {:?}",
            started.elapsed()
        );

        // `terminate_pid` doubles as the liveness PROOF and the cleanup: `Ok(true)` means the
        // descendant was genuinely still alive at this point — i.e. `exec_argv` really did finalize
        // while the pipe was still held open, which is the whole premise of the timing assertion
        // above — and the same call is what stops it outliving this process.
        let descendant = std::fs::read_to_string(&marker)
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .expect("the backgrounded descendant must have written its pid");
        let _ = std::fs::remove_file(&marker);
        assert!(
            terminate_pid(descendant).unwrap_or(false),
            "the backgrounded descendant must still be alive here — otherwise the idle-grace \
             fallback was never the thing that let `exec_argv` return"
        );
    }

    /// Is `pid` currently in the process-global detached-child registry?
    #[cfg(unix)]
    fn is_tracked(pid: u32) -> bool {
        tracked_detached_child_pids().contains(&pid)
    }

    /// Poll until `pid` is gone, up to `deadline`. A `killpg`'d process is a zombie until reaped and
    /// `kill(pid, 0)` succeeds on a zombie, so a single sample would be racy.
    #[cfg(unix)]
    async fn wait_gone(pid: u32, within: Duration) -> bool {
        let deadline = std::time::Instant::now() + within;
        while std::time::Instant::now() < deadline {
            if !pid_exists(pid) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        false
    }

    /// SEAM-S03, the registry half: `LocalProc::exec` must enroll its `setsid` shell for the whole
    /// time that shell is running and remove it when the exec ends — Pi's
    /// `if (child.pid) trackDetachedChildPid(child.pid);` at the spawn (`core/tools/bash.ts:108`
    /// @v0.83.0) and the matching `untrackDetachedChildPid` in that spawn's `finally` (`:142`).
    ///
    /// The membership is asserted PRESENT first, from inside the `on_data` callback while the child
    /// is provably alive (it has just written its own `$$` and is now blocked in `sleep`). Without
    /// that half the absence assertion afterwards would pass just as well against a registry that
    /// was never written to at all.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_tracks_its_detached_shell_for_exactly_as_long_as_it_runs() {
        let proc = LocalProc::new(ShellConfig::detect());
        let cancel = CancelToken::new();
        let mut child_pid: Option<u32> = None;
        let mut tracked_while_running = false;
        {
            // `$$` in the spawned shell IS the direct child's pid — the same value
            // `KillTreeOnDrop::arm` was handed — and `setsid` made it the group id too.
            let stopper = cancel.clone();
            let status = proc
                .exec(
                    exec_spec("echo $$; sleep 30"),
                    cancel.clone(),
                    None,
                    &mut |data: &[u8]| {
                        if child_pid.is_none()
                            && let Ok(pid) = String::from_utf8_lossy(data).trim().parse::<u32>()
                        {
                            child_pid = Some(pid);
                            tracked_while_running = is_tracked(pid);
                            stopper.cancel();
                        }
                    },
                )
                .await
                .expect("exec runs");
            assert_eq!(
                status,
                ExitStatus::Killed,
                "fixture: the callback cancels, so this must be the cancel path"
            );
        }

        let pid = child_pid.expect("fixture: the shell must have reported its own pid");
        assert!(
            tracked_while_running,
            "a running detached bash child must be in the registry Pi's signal handlers drain \
             (pid {pid}) — otherwise `killTrackedDetachedChildren` has nothing to kill"
        );
        assert!(
            !is_tracked(pid),
            "the finished exec must have left the registry (Pi's `finally` untrack, bash.ts:142) — \
             a retained pid {pid} is worse than a forgotten one, since the next drain would \
             `killpg` a group this process no longer owns"
        );
    }

    /// The JS→Rust guarantee gap on the UNTRACK side, and why it lives in `Drop`.
    ///
    /// Pi's untrack sits in a `finally` (`core/tools/bash.ts:142` @v0.83.0), so it runs on the
    /// normal return, the `aborted` throw and the `timeout:` throw alike — an `async` function
    /// always settles. A Rust future does not: dropping `exec` mid-`select!` (here via
    /// `tokio::time::timeout`, equally a cancelled `tokio::spawn`, a panic, or runtime teardown)
    /// skips everything written after the loop. An untrack placed on the success path would
    /// therefore leak this pid for the life of the process, and the next
    /// `kill_tracked_detached_children` would `killpg` a pid the kernel may have recycled onto an
    /// unrelated group.
    ///
    /// RED if the untrack moves next to `kill_guard.disarm()`; GREEN with it in `Drop`.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dropped_exec_future_untracks_its_pid_instead_of_leaking_it() {
        let proc = LocalProc::new(ShellConfig::detect());
        let mut child_pid: Option<u32> = None;
        let mut tracked_while_running = false;
        {
            let elapsed = tokio::time::timeout(
                Duration::from_millis(500),
                proc.exec(
                    exec_spec("echo $$; sleep 30"),
                    CancelToken::new(),
                    None,
                    &mut |data: &[u8]| {
                        if child_pid.is_none()
                            && let Ok(pid) = String::from_utf8_lossy(data).trim().parse::<u32>()
                        {
                            child_pid = Some(pid);
                            tracked_while_running = is_tracked(pid);
                        }
                    },
                ),
            )
            .await;
            assert!(
                elapsed.is_err(),
                "fixture: the command must still be running when the timeout DROPS the future — \
                 otherwise this observes a normal return, not the drop path"
            );
        }

        let pid = child_pid.expect("fixture: the shell must have reported its own pid");
        assert!(
            tracked_while_running,
            "fixture: the pid must have been in the registry before the drop, or the absence \
             assertion below is vacuous"
        );
        assert!(
            !is_tracked(pid),
            "an ABANDONED exec must still untrack pid {pid} — the untrack is Pi's `finally` \
             (bash.ts:142) and its only faithful Rust home is `Drop`, not a statement after the \
             `select!` loop that a dropped future never reaches"
        );
    }

    /// SEAM-S03, the drain half: `killTrackedDetachedChildren` (`utils/shell.ts:190-195` @v0.83.0)
    /// must `killProcessTree` every registered pid — on unix `process.kill(-pid, "SIGKILL")`
    /// (`:214`), the whole process GROUP, not just the leader — and empty the registry afterwards
    /// (`:194`).
    ///
    /// The discriminating assertion is the GRANDCHILD: a single-pid kill would leave the
    /// backgrounded `sleep 30` running for its full 30s, which is exactly the orphan SEAM-S03 is
    /// about. Its liveness is asserted BEFORE the drain so a fixture that never started cannot pass
    /// this vacuously.
    ///
    /// Runs against a registry this test owns rather than the process-global one — see
    /// [`drain_and_kill`] for why.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_drain_sigkills_each_registered_group_and_empties_the_registry() {
        let marker =
            std::env::temp_dir().join(format!("cyrup-drain-{}-{}.pid", std::process::id(), 1));
        let _ = std::fs::remove_file(&marker);
        // Same fixture shape as the drop-guard test: a `setsid` leader that backgrounds a
        // descendant into its own group and then blocks.
        let spec = exec_spec(&format!("sleep 30 & echo $! > {}; wait", marker.display()));
        let mut cmd = tokio::process::Command::from(build_command(&spec));
        cmd.kill_on_drop(true);
        let mut leader_child = cmd.spawn().expect("fixture: the shell must spawn");
        let leader = leader_child.id().expect("fixture: the shell must have a pid");
        // `build_command` only appends the command to argv under `Transport::Argv`; the WSL-legacy
        // `bash -s` config `try_detect` can return instead expects it on stdin (`shell.rs:52`), and
        // without this the shell would block on an open pipe and never start the fixture.
        if spec.shell.transport == Transport::Stdin
            && let Some(mut stdin) = leader_child.stdin.take()
        {
            let _ = stdin.write_all(spec.command.as_bytes()).await;
            let _ = stdin.shutdown().await;
        }

        let mut descendant = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Some(pid) = std::fs::read_to_string(&marker)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
            {
                descendant = Some(pid);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let _ = std::fs::remove_file(&marker);
        let descendant = descendant.expect("fixture: the descendant must have recorded its pid");
        assert!(
            pid_exists(descendant),
            "fixture: the backgrounded descendant (pid {descendant}) must be alive before the \
             drain, or its absence afterwards proves nothing"
        );

        let registry = std::sync::Mutex::new(std::collections::BTreeSet::from([leader]));
        drain_and_kill(&registry);

        assert!(
            registry.lock().map(|set| set.is_empty()).unwrap_or(false),
            "the drain must empty the registry (Pi's `trackedDetachedChildPids.clear()`, \
             shell.ts:194), so a second delivery does not re-signal recycled pids"
        );
        let group_died = wait_gone(descendant, Duration::from_secs(3)).await;
        // Clean up before asserting, so a failing run does not itself leak the `sleep 30`.
        let _ = kill_pid(descendant);
        let _ = leader_child.start_kill();
        assert!(
            group_died,
            "the drain must `killpg` the whole group: the backgrounded descendant (pid \
             {descendant}) outlived it, which is a single-pid kill of the leader ({leader}), not \
             Pi's `killProcessTree`"
        );
    }
}
