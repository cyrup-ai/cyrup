//! The default local backend over `tokio::fs` / `tokio::process` (arch-03 §3.3, §6.5).
//!
//! `LocalFs` is an indirection over the real filesystem; `LocalProc` runs commands through the
//! detected shell, streams combined stdout+stderr, and kills the whole process tree on
//! cancel/timeout (R-03-024/027). The two `ProcOps` methods intentionally use DIFFERENT
//! escalations, 1:1 with their DIFFERENT real Pi consumers: [`LocalProc::exec`] backs both the
//! `bash` tool (`bash.ts:82-148`'s `createLocalBashOperations`) and the immediate-bash RPC seam
//! (`bash-executor.ts:108`'s `executeBashWithOperations`, which calls the SAME `BashOperations`),
//! and both paths' abort/timeout handlers call `killProcessTree` (`shell.ts:200-225`) — an
//! IMMEDIATE `killpg(SIGKILL)`, no `SIGTERM`, no grace period, ever. [`LocalProc::exec_argv`] backs
//! the WASM `exec` capability grant instead, whose real consumer is `exec.ts:34-63`'s
//! `execCommand`/`killProcess` — a `SIGTERM`-then-grace-then-`SIGKILL` escalation (the group-scoped
//! analog of `cyrup-ext`'s `proc.rs::kill`, single-pid escalation for the non-`setsid`'d `proc`
//! capability). The only `unsafe` in the crate lives here, isolated to the unix process-group calls
//! (`setsid`/`killpg`) with safety comments.

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

/// Local filesystem operations.
#[derive(Default, Clone)]
pub struct LocalFs;

#[async_trait::async_trait]
impl FsOps for LocalFs {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError> {
        tokio::fs::read(path).await.map_err(|e| error::io(&error::show(path), &e))
    }

    async fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .map_err(|e| error::io(&format!("create dir {}", error::show(parent)), &e))?;
            }
        let tmp = match path.file_name() {
            Some(name) => {
                let mut t = name.to_os_string();
                t.push(format!(".cyrup-tmp-{}", unique_suffix()));
                path.with_file_name(t)
            }
            None => return Err(error::invalid(format!("invalid path: {}", error::show(path)))),
        };
        tokio::fs::write(&tmp, bytes)
            .await
            .map_err(|e| error::io(&format!("write {}", error::show(&tmp)), &e))?;
        tokio::fs::rename(&tmp, path).await.map_err(|e| {
            let _ = std::fs::remove_file(&tmp);
            error::io(&format!("rename to {}", error::show(path)), &e)
        })?;
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
                return Err(error::io(&error::show(path), &std::io::Error::last_os_error()));
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            // No portable effective-access syscall here: fall back to metadata existence + the
            // readonly bit (the pre-`access(2)` behavior).
            let meta = tokio::fs::metadata(path)
                .await
                .map_err(|e| error::io(&error::show(path), &e))?;
            if mode == Access::ReadWrite && meta.permissions().readonly() {
                return Err(error::invalid(format!("{} is not writable", error::show(path))));
            }
            Ok(())
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

    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError> {
        let mut rd = tokio::fs::read_dir(path)
            .await
            .map_err(|e| error::io(&error::show(path), &e))?;
        let mut out = Vec::new();
        loop {
            match rd.next_entry().await {
                Ok(Some(entry)) => {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    out.push(DirEntry { name, path: entry.path() });
                }
                Ok(None) => break,
                Err(e) => return Err(error::io(&error::show(path), &e)),
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
/// `spec.args` (no shell, no word-splitting), with the same unix process-group setup as the shell path
/// so the whole tree can be reaped on timeout/cancel.
#[allow(unsafe_code)]
fn build_argv_command(spec: &ArgvSpec) -> std::process::Command {
    let mut std_cmd = std::process::Command::new(&spec.program);
    std_cmd.args(&spec.args);
    std_cmd.current_dir(&spec.cwd);
    for (k, v) in &spec.env {
        std_cmd.env(k, v);
    }
    // Pi uses stdio `["ignore","pipe","pipe"]` (exec.ts:44): stdin closed, stdout+stderr piped.
    std_cmd.stdin(std::process::Stdio::null());
    std_cmd.stdout(std::process::Stdio::piped());
    std_cmd.stderr(std::process::Stdio::piped());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: `setsid` only detaches the child into its own session/process group before exec;
        // it touches no parent memory and is async-signal-safe. This makes the child the group leader
        // (pgid == pid) so the whole tree can be killed via `killpg` on timeout/cancel (R-03-027).
        unsafe {
            std_cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    std_cmd
}

/// Send the GRACEFUL first step of the kill-tree escalation (Pi `killProcess`'s `proc.kill
/// ("SIGTERM")`, `exec.ts:55`) to the child's whole process GROUP (contrast [`terminate_pid`],
/// which signals a single non-group-leader pid for the unrelated `proc` capability). A `setsid`'d
/// tree that traps `SIGTERM` gets real time — up to [`LocalProc::kill_grace`] — to flush state /
/// clean up children before [`send_sigkill_tree`] forces it (R-03-024/027).
///
/// Returns whether a REAL graceful signal was actually sent: `true` on unix when `killpg(2)`
/// succeeds; `false` on non-unix (no portable single-call graceful-signal-a-tree primitive there)
/// AND on unix if the group is already gone (`killpg` fails, e.g. `ESRCH`). Callers must use this
/// to skip the grace-period wait when nothing was sent — exactly the sibling `proc` capability's
/// `ProcCaps::kill`/`cyrup_tools::terminate_pid` fix for the identical bug class (commit `0790ace`:
/// "skip the pointless SIGTERM grace wait on non-unix").
#[allow(unsafe_code)]
fn send_sigterm_tree(child: &tokio::process::Child) -> bool {
    #[cfg(unix)]
    {
        match child.id() {
            // SAFETY: send SIGTERM to the child's process group (created via `setsid`). A negative
            // pid / killpg targets the group; a nonzero return (e.g. `ESRCH`, group already gone)
            // means nothing was actually signaled.
            Some(pid) => unsafe { libc::killpg(pid as libc::pid_t, libc::SIGTERM) == 0 },
            None => false,
        }
    }
    #[cfg(not(unix))]
    {
        // No portable graceful-signal primitive on non-unix without a real `Child` + platform API
        // beyond what this crate depends on — genuinely nothing is sent here; [`send_sigkill_tree`]'s
        // `taskkill /F /T` is the only real termination path on this platform.
        let _ = child;
        false
    }
}

/// Force-kill the child's whole process tree (Pi `killProcess`'s escalation, `exec.ts:57-61`: `if
/// (!proc.killed) proc.kill("SIGKILL")` after the grace period) — R-03-024/027.
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

/// Send SIGTERM to a SINGLE process by pid — NOT a process group (contrast [`send_sigterm_tree`],
/// which targets the whole `setsid` group a shell-spawned tree needs, R-03-027). This is the graceful
/// half of a two-step escalation for a caller that owns exactly one non-group-leader child directly
/// (e.g. cyrup-ext's long-lived `proc` capability, arch-08 §5.2/pi-mcp-adapter-port.md §3.1, which
/// spawns a plain — not `setsid`'d — child, mirroring the real `StdioClientTransport`'s non-detached
/// spawn 1:1). A best-effort no-op on non-unix (no portable single-pid graceful-signal primitive
/// there without holding the `Child` itself, which this pid-only API deliberately doesn't require);
/// [`kill_pid`] is the forceful escalation that DOES work everywhere.
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
/// half of [`terminate_pid`]; works everywhere (unlike the graceful half).
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
                    // Skip the grace-period wait entirely when nothing was actually sent (non-unix,
                    // or the group is already gone) — waiting it out has zero chance of a graceful
                    // exit landing, mirroring `ProcCaps::kill`'s identical fix for this bug class
                    // (`cyrup-ext/src/caps/proc.rs:347-355`, commit `0790ace`).
                    let sigterm_sent = send_sigterm_tree(&child);
                    pending = Some(ExitStatus::Killed);
                    let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };
                    grace.as_mut().reset(tokio::time::Instant::now() + wait);
                }
                _ = &mut timeout_fut, if pending.is_none() => {
                    let sigterm_sent = send_sigterm_tree(&child);
                    pending = Some(ExitStatus::TimedOut);
                    let wait = if sigterm_sent { self.kill_grace } else { Duration::ZERO };
                    grace.as_mut().reset(tokio::time::Instant::now() + wait);
                }
                _ = &mut grace, if pending.is_some() => {
                    // Grace elapsed (or was skipped outright, above) with NO natural exit yet (a
                    // graceful mid-grace exit already
                    // broke the loop via the `child.wait()`/drain arms below, which keep the REAL
                    // code). Force it, but still capture whatever real status `wait()` reports
                    // (Pi: SIGKILL ⇒ `exit(null)` ⇒ `code ?? 0`, matched by `exit_from`'s
                    // no-code-⇒-`Signaled` mapping) rather than discarding it outright.
                    send_sigkill_tree(&mut child);
                    let real = child.wait().await.ok().map(exit_from);
                    break real.unwrap_or_else(|| pending.unwrap_or(ExitStatus::TimedOut));
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
            shell: ShellConfig::detect(),
        }
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

    /// THE regression this fix closes: a well-behaved child that TRAPS SIGTERM and exits itself
    /// with its OWN real, nonzero exit code mid-grace must have that REAL code reported — 1:1 with
    /// Pi's `waitForChildProcess`/`finalize(exitCode)` (`child-process.ts:73-80`), which always
    /// resolves with the actual observed `code`, `killed` bolted on separately (`exec.ts:97`). The
    /// old cyrup behavior collapsed this to a hard-coded `code 0` any time a kill was in flight.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn exec_argv_timeout_preserves_the_real_code_of_a_graceful_sigterm_handler() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let out = proc
            .exec_argv(
                argv("sh", &["-c", "trap 'exit 7' TERM; while true; do sleep 1; done"]),
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
        let started = tokio::time::Instant::now();
        let out = proc
            .exec_argv(
                argv("sh", &["-c", "trap '' TERM; while true; do sleep 1; done"]),
                CancelToken::new(),
                Some(Duration::from_millis(100)),
            )
            .await
            .expect("exec_argv runs");
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
        let cancel = CancelToken::new();
        let started = tokio::time::Instant::now();
        let task = tokio::spawn({
            let cancel = cancel.clone();
            let spec = argv("sh", &["-c", "trap '' TERM; while true; do sleep 1; done"]);
            async move { proc.exec_argv(spec, cancel, None).await }
        });
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        let out = task.await.expect("task joins").expect("exec_argv runs");
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

    /// `send_sigterm_tree`'s `bool` return is what `exec`/`exec_argv`'s select loops now use to skip
    /// the grace-period wait when nothing was actually sent — the SAME bug class as `terminate_pid`
    /// (`0790ace`), just for the whole-process-GROUP kill this file's `exec`/`exec_argv` use. Verified
    /// directly against a REAL `setsid`'d process group: `true` while it's alive, `false` (`ESRCH`)
    /// once it's already reaped — not just documented.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn send_sigterm_tree_reports_true_while_alive_false_once_reaped() {
        let std_cmd = build_argv_command(&argv("sleep", &["30"]));
        let mut child = tokio::process::Command::from(std_cmd).spawn().expect("sleep spawns");

        assert!(send_sigterm_tree(&child), "a live setsid'd group is genuinely signaled");
        // `sleep` doesn't trap SIGTERM — the signal just sent is enough to reap it.
        child.wait().await.expect("the SIGTERM-obeying child dies");

        assert!(
            !send_sigterm_tree(&child),
            "signaling an already-reaped process group must report false (ESRCH), not silently \
             claim success"
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
        let started = tokio::time::Instant::now();
        let out = tokio::time::timeout(
            Duration::from_secs(3),
            proc.exec_argv(
                argv("sh", &["-c", "(sleep 5 &) ; exit 0"]),
                CancelToken::new(),
                None,
            ),
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
    }
}
