//! The default local backend over `tokio::fs` / `tokio::process` (arch-03 §3.3, §6.5).
//!
//! `LocalFs` is an indirection over the real filesystem; `LocalProc` runs commands through the
//! detected shell, streams combined stdout+stderr, and kills the whole process tree on
//! cancel/timeout (R-03-024/027). The only `unsafe` in the crate lives here, isolated to the unix
//! process-group calls (`setsid`/`killpg`) with safety comments.

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

/// Local process operations.
pub struct LocalProc {
    shell: ShellConfig,
}

impl LocalProc {
    pub fn new(shell: ShellConfig) -> Self {
        Self { shell }
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

/// Kill the child's whole process tree (R-03-024/027).
#[allow(unsafe_code)]
fn kill_tree(child: &mut tokio::process::Child) {
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

/// Send SIGTERM to a SINGLE process by pid — NOT a process group (contrast [`kill_tree`], which
/// targets the whole `setsid` group a shell-spawned tree needs, R-03-027). This is the graceful
/// half of a two-step escalation for a caller that owns exactly one non-group-leader child directly
/// (e.g. cyrup-ext's long-lived `proc` capability, arch-08 §5.2/pi-mcp-adapter-port.md §3.1, which
/// spawns a plain — not `setsid`'d — child, mirroring Pi's own non-detached `StdioClientTransport`
/// spawn 1:1). A best-effort no-op on non-unix (no portable single-pid graceful-signal primitive
/// there without holding the `Child` itself, which this pid-only API deliberately doesn't require);
/// [`kill_pid`] is the forceful escalation that DOES work everywhere.
#[allow(unsafe_code)]
pub fn terminate_pid(pid: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        // SAFETY: `kill(2)` only reads its two integer args (pid, signal); it touches no memory. A
        // non-zero return is an `errno` (e.g. `ESRCH` if the pid is already gone), surfaced as an
        // `io::Error`, never a panic.
        let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGTERM) };
        if rc == 0 { Ok(()) } else { Err(std::io::Error::last_os_error()) }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        Ok(())
    }
}

/// Force-kill a SINGLE process by pid (SIGKILL / non-unix `taskkill /F /PID`, no `/T` — this
/// targets exactly the one pid, never a subtree; contrast [`kill_tree`]). The escalation half of
/// [`terminate_pid`]; works everywhere (unlike the graceful half).
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

        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    kill_tree(&mut child);
                    let _ = child.wait().await;
                    break ExitStatus::Killed;
                }
                _ = &mut timeout_fut => {
                    kill_tree(&mut child);
                    let _ = child.wait().await;
                    break ExitStatus::TimedOut;
                }
                chunk = read_chunk(&mut stdout) => {
                    match chunk {
                        Some(data) => on_data(&data),
                        None => stdout = None,
                    }
                }
                chunk = read_chunk(&mut stderr) => {
                    match chunk {
                        Some(data) => on_data(&data),
                        None => stderr = None,
                    }
                }
                s = child.wait(), if stdout.is_none() && stderr.is_none() => {
                    break match s {
                        Ok(st) => exit_from(st),
                        Err(_) => ExitStatus::Exited(-1),
                    };
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

        let status = loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    kill_tree(&mut child);
                    let _ = child.wait().await;
                    break ExitStatus::Killed;
                }
                _ = &mut timeout_fut => {
                    kill_tree(&mut child);
                    let _ = child.wait().await;
                    break ExitStatus::TimedOut;
                }
                chunk = read_chunk(&mut stdout) => {
                    match chunk {
                        Some(data) => out_buf.extend_from_slice(&data),
                        None => stdout = None,
                    }
                }
                chunk = read_chunk(&mut stderr) => {
                    match chunk {
                        Some(data) => err_buf.extend_from_slice(&data),
                        None => stderr = None,
                    }
                }
                s = child.wait(), if stdout.is_none() && stderr.is_none() => {
                    break match s {
                        Ok(st) => exit_from(st),
                        Err(_) => ExitStatus::Exited(-1),
                    };
                }
            }
        };
        Ok(ArgvOutput { status, stdout: out_buf, stderr: err_buf })
    }
}
