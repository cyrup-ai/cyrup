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
//! unix process-group calls (`setsid`/`killpg`, [`build_command`]/[`send_sigkill_tree`], used ONLY
//! by [`LocalProc::exec`]) and the single-pid `kill(2)` calls ([`terminate_pid`]/[`kill_pid`]) with
//! safety comments.

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

    /// An ALREADY-cancelled token must never spawn a process at all — Pi's real
    /// `createLocalBashOperations.exec` checks `signal?.aborted` and throws BEFORE calling `spawn()`
    /// (`bash.ts:86-88`), ahead of even the cwd-exists check. Proven here the same way the sibling
    /// SIGKILL tests prove immediacy: a marker file the child would create if it ever ran must stay
    /// absent, and the call must return near-instantly (no real process start/teardown latency).
    #[tokio::test]
    async fn exec_pre_cancelled_never_spawns() {
        let proc = LocalProc::with_kill_grace(ShellConfig::detect(), Duration::from_secs(5));
        let marker = std::env::temp_dir().join(format!("cyrup-exec-precancel-{}", unique_suffix()));
        let cancel = CancelToken::new();
        cancel.cancel();
        let started = tokio::time::Instant::now();
        let status = proc
            .exec(exec_spec(&format!("touch {}", marker.display())), cancel, None, &mut |_data: &[u8]| {})
            .await
            .expect("a pre-cancelled exec resolves Ok, not Err");
        assert_eq!(status, ExitStatus::Killed, "pre-cancelled reports the same reason as mid-run cancel");
        assert!(
            started.elapsed() < Duration::from_millis(200),
            "must short-circuit before spawning, not pay real process start/teardown latency, got {:?}",
            started.elapsed()
        );
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
            let out = proc
                .exec_argv(
                    argv("sh", &["-c", &script]),
                    CancelToken::new(),
                    Some(Duration::from_millis(15)),
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
                 killed (ground truth file was empty)"
            );
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
