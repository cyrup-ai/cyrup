//! The supervisor's end of a sandbox process (see [`super::child`] for the other end).
//!
//! [`start`] stands in for the isolate thread of the in-process sandbox: it starts the process,
//! and two threads relay between its pipes and the same channels the supervisor already uses, so
//! [`super::execution`] cannot tell the two apart. The supervisor's [`KillSwitch`] holds the
//! means to kill the process, which is how a deadline, a cancel or `close()` stops a script, and
//! a process that ends without having said how the script ended is reported as a crash.

use std::ffi::OsString;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use super::lifecycle::{KillSwitch, RunningGuard};
use super::protocol::{HostReply, IsolateInit, WorkerMessage};
use super::wire::{FromProcess, ToProcess, read_frame, write_frame};

/// The argument the host binary is started with to become a sandbox process. The binary must call
/// [`run_sandbox_process`](super::run_sandbox_process) when it sees it, before it does anything
/// else with its standard streams.
pub const HOST_SUBCOMMAND: &str = "__codemode-sandbox";

/// How long a process that has closed its output gets to exit before it is killed.
const EXIT_GRACE: Duration = Duration::from_secs(2);

/// How a sandbox process is started: the program, its arguments and the only environment it gets.
///
/// The process runs a model's script, so it starts with an empty environment (API keys and the
/// like stay in the host) plus what is added here and the library path variables a dynamically
/// linked binary needs.
#[derive(Clone, Debug)]
pub struct HostCommand {
    program: PathBuf,
    args: Vec<OsString>,
    env: Vec<(OsString, OsString)>,
    /// What the process sees as its own name, when that is not `program`.
    arg0: Option<OsString>,
}

/// The kernel's name for the image a process is running (Linux). Executing it starts exactly the
/// running binary, also after its file was removed or replaced.
#[cfg(target_os = "linux")]
const RUNNING_IMAGE: &str = "/proc/self/exe";

/// What a removed `/proc/self/exe` target reads as in `readlink`.
#[cfg(target_os = "linux")]
const DELETED_SUFFIX: &str = " (deleted)";

/// Appended where a sandbox process could not start or died before it said anything, and the cause
/// may be that the binary it is started from is no longer the one this session is running.
const UPGRADE_HINT: &str =
    "If cyrup was upgraded or reinstalled while this session was open, restart cyrup";

impl HostCommand {
    #[must_use]
    pub fn new<I, A>(program: impl Into<PathBuf>, args: I) -> Self
    where
        I: IntoIterator<Item = A>,
        A: Into<OsString>,
    {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            env: Vec::new(),
            arg0: None,
        }
    }

    /// The running executable, started with [`HOST_SUBCOMMAND`].
    ///
    /// # Errors
    ///
    /// The operating system could not say which executable is running.
    pub fn current_exe() -> io::Result<Self> {
        Self::running_executable([HOST_SUBCOMMAND])
    }

    /// The running executable, started with `args`.
    ///
    /// [CYRUP-DELTA] Upstream's sandbox is a worker thread and has no binary to start. Here every
    /// script starts one, long after the session did, and a path captured at startup names whatever
    /// is there by then: after cyrup was upgraded or reinstalled mid-session (a package manager, an
    /// install script, `cargo install`) the old file is gone and every script failed to start, or a
    /// newer cyrup was started to speak the old host's pipe protocol. On Linux the process is
    /// started from `/proc/self/exe` instead, which is the image this process runs, wherever its
    /// file has gone. Elsewhere the path is kept and the failures that follow say to restart.
    ///
    /// # Errors
    ///
    /// The operating system could not say which executable is running.
    pub(crate) fn running_executable<I, A>(args: I) -> io::Result<Self>
    where
        I: IntoIterator<Item = A>,
        A: Into<OsString>,
    {
        let path = std::env::current_exe()?;
        #[cfg(target_os = "linux")]
        if std::path::Path::new(RUNNING_IMAGE).exists() {
            let mut command = Self::new(RUNNING_IMAGE, args);
            command.arg0 = display_name(&path);
            return Ok(command);
        }
        Ok(Self::new(path, args))
    }

    /// Adds an environment variable for the process.
    #[must_use]
    pub fn with_env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    fn spawn(&self) -> io::Result<Child> {
        let mut command = Command::new(&self.program);
        #[cfg(unix)]
        if let Some(name) = &self.arg0 {
            std::os::unix::process::CommandExt::arg0(&mut command, name);
        }
        command
            .args(&self.args)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // The engine writes its fatal errors here; the host's terminal is not the place.
            .stderr(Stdio::null());
        for key in ["LD_LIBRARY_PATH", "DYLD_LIBRARY_PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
        command.spawn().map_err(|error| self.explain(error))
    }

    /// A program that is not there is, for the executable cyrup started from, an upgrade or a
    /// reinstall that happened while the session was open (see [`Self::running_executable`]).
    fn explain(&self, error: io::Error) -> io::Error {
        if error.kind() != io::ErrorKind::NotFound {
            return error;
        }
        io::Error::new(
            error.kind(),
            format!(
                "{error}: {} does not exist. {UPGRADE_HINT}.",
                self.program.display()
            ),
        )
    }
}

/// The name a process started from [`RUNNING_IMAGE`] presents as its own: that of the file it was
/// started from, as `ps` and the process's own argument list should show.
#[cfg(target_os = "linux")]
fn display_name(path: &std::path::Path) -> Option<OsString> {
    let name = path.file_name()?.to_string_lossy();
    Some(OsString::from(
        name.strip_suffix(DELETED_SUFFIX).unwrap_or(&name),
    ))
}

/// The child, shared by the thread that waits for it and the kill switch.
type SharedChild = Arc<Mutex<Child>>;

fn lock(child: &SharedChild) -> std::sync::MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

fn kill(child: &SharedChild) {
    // An error is a process that already ended.
    let _ = lock(child).kill();
}

/// Waits for the process to end, killing it if it does not within `grace`. Never holds the lock
/// while it sleeps, so the kill switch is never kept waiting.
fn reap(child: &SharedChild, grace: Duration) -> Option<ExitStatus> {
    let deadline = Instant::now() + grace;
    loop {
        {
            let mut child = lock(child);
            match child.try_wait() {
                Ok(Some(status)) => return Some(status),
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    return child.wait().ok();
                }
                Ok(None) => {}
                Err(_) => return None,
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Starts the sandbox process for one script and the threads that connect it to the supervisor.
///
/// `guard` stays alive until the process has been reaped, so `close()` does not return while one
/// is running.
///
/// # Errors
///
/// The process or one of its threads could not be started.
pub(super) fn start(
    command: &HostCommand,
    init: IsolateInit,
    to_host: UnboundedSender<WorkerMessage>,
    from_host: UnboundedReceiver<HostReply>,
    kill_switch: &KillSwitch,
    guard: RunningGuard,
) -> io::Result<()> {
    let mut process = command.spawn()?;
    let (Some(stdin), Some(stdout)) = (process.stdin.take(), process.stdout.take()) else {
        let _ = process.kill();
        let _ = process.wait();
        return Err(io::Error::other("the sandbox process has no pipes"));
    };
    let child: SharedChild = Arc::new(Mutex::new(process));
    let stopper = Arc::clone(&child);
    if !kill_switch.arm_with(move || kill(&stopper)) {
        // Stopped before it started.
        kill(&child);
        let _ = reap(&child, EXIT_GRACE);
        return Ok(());
    }
    let started = std::thread::Builder::new()
        .name(String::from("codemode-sandbox-writer"))
        .spawn(move || write_to_process(stdin, init, from_host))
        .and_then(|_| {
            let child = Arc::clone(&child);
            std::thread::Builder::new()
                .name(String::from("codemode-sandbox-reader"))
                .spawn(move || read_from_process(stdout, &child, &to_host, guard))
                .map(drop)
        });
    if started.is_err() {
        // The writer, if it started, ends when the process dies and its pipe breaks.
        kill_switch.kill();
    }
    started
}

/// The start frame, then each tool outcome the supervisor sends, until the supervisor is done.
/// Dropping the pipe is the process's signal to end.
fn write_to_process(
    mut stdin: ChildStdin,
    init: IsolateInit,
    mut from_host: UnboundedReceiver<HostReply>,
) {
    if write_frame(&mut stdin, &ToProcess::Start(init)).is_err() {
        return;
    }
    while let Some(reply) = from_host.blocking_recv() {
        if write_frame(&mut stdin, &ToProcess::Reply(reply)).is_err() {
            return;
        }
    }
}

/// Relays the process's messages until the script settles or the process ends, then reaps it.
fn read_from_process(
    stdout: ChildStdout,
    child: &SharedChild,
    to_host: &UnboundedSender<WorkerMessage>,
    guard: RunningGuard,
) {
    // Dropped after the process is reaped.
    let _guard = guard;
    let mut input = BufReader::with_capacity(64 * 1024, stdout);
    let mut scratch = Vec::new();
    let mut heard = false;
    let ending = loop {
        match read_frame::<FromProcess>(&mut input, &mut scratch) {
            Ok(Some(frame)) => {
                heard = true;
                let message = WorkerMessage::from(frame);
                let settled = matches!(
                    message,
                    WorkerMessage::Done(_)
                        | WorkerMessage::Crash(_)
                        | WorkerMessage::OutOfMemory
                        | WorkerMessage::ActiveLimit
                );
                if to_host.send(message).is_err() || settled {
                    // Settled, or nobody is listening any more.
                    break None;
                }
            }
            // The process closed its output without having settled the script.
            Ok(None) => break Some(None),
            Err(error) => break Some(Some(error)),
        }
    };
    match ending {
        None => kill(child),
        Some(unreadable) => {
            let message = match unreadable {
                Some(error) => {
                    kill(child);
                    let _ = reap(child, EXIT_GRACE);
                    format!("The sandbox process sent an unreadable message: {error}")
                }
                None => {
                    describe_exit_in(reap(child, EXIT_GRACE), heard, host_address_space_limit())
                }
            };
            let _ = to_host.send(WorkerMessage::Crash(message));
        }
    }
    let _ = reap(child, EXIT_GRACE);
}

/// What the supervisor tells the model when the process ended without saying how the script did.
/// `heard` is whether it sent anything first.
///
/// [CYRUP-DELTA] A process that returns an exit code before it has said a word did not run a
/// script that crashed the engine (that dies by a signal, or reports its out-of-memory): it is a
/// binary that did not understand the host, such as a newer cyrup installed over the one this
/// session started from, so the message says what to do.
fn describe_exit(status: Option<ExitStatus>, heard: bool) -> String {
    let mut restart = false;
    let cause = match status {
        None => String::from("it could not be waited for"),
        Some(status) => match signal_of(status) {
            Some(signal) => format!("it was terminated by {signal}"),
            None => match status.code() {
                Some(code) => {
                    restart = !heard;
                    format!("it exited with status {code}")
                }
                None => String::from("it ended abnormally"),
            },
        },
    };
    let message = format!("The sandbox process crashed before the script settled: {cause}.");
    if restart {
        format!("{message} {UPGRADE_HINT}.")
    } else {
        message
    }
}

/// [`describe_exit`], plus what an operator needs when the process died by a signal before it said
/// a word in a host whose address space is limited (`address_space_limit`, in bytes).
///
/// [CYRUP-DELTA] V8 reserves a large range of address space when the isolate starts, and a
/// process limited with `ulimit -v` / `RLIMIT_AS` dies on a trap (measured: `SIGTRAP`, at limits
/// of 16, 20, 24 and 32 GiB; scripts ran at 40 GiB and above). The message used to be the bare
/// signal, which says nothing about the limit that caused it.
pub(super) fn describe_exit_in(
    status: Option<ExitStatus>,
    heard: bool,
    address_space_limit: Option<u64>,
) -> String {
    let message = describe_exit(status, heard);
    let signalled = status.is_some_and(|status| signal_of(status).is_some());
    match address_space_limit {
        Some(limit) if signalled && !heard => format!(
            "{message} The address space of cyrup is limited to {} (`ulimit -v`), and the script \
             sandbox needs more than 32 GiB of it: raise the limit to run scripts.",
            gib(limit)
        ),
        _ => message,
    }
}

/// `bytes` as GiB for a message, one decimal at most.
pub(super) fn gib(bytes: u64) -> String {
    let tenths = bytes.saturating_mul(10) / (1024 * 1024 * 1024);
    if tenths.is_multiple_of(10) {
        format!("{} GiB", tenths / 10)
    } else {
        format!("{}.{} GiB", tenths / 10, tenths % 10)
    }
}

/// The soft `RLIMIT_AS` of this process in bytes, when one is set. Linux only: that is where the
/// limit was measured, and `RLIMIT_AS` is not enforced the same way elsewhere.
#[cfg(target_os = "linux")]
fn host_address_space_limit() -> Option<u64> {
    use nix::sys::resource::{RLIM_INFINITY, Resource, getrlimit};

    let (soft, _hard) = getrlimit(Resource::RLIMIT_AS).ok()?;
    (soft != RLIM_INFINITY).then_some(soft)
}

#[cfg(not(target_os = "linux"))]
fn host_address_space_limit() -> Option<u64> {
    None
}

#[cfg(unix)]
fn signal_of(status: ExitStatus) -> Option<String> {
    use std::os::unix::process::ExitStatusExt;

    let number = status.signal()?;
    let name = match number {
        4 => "SIGILL",
        5 => "SIGTRAP",
        6 => "SIGABRT",
        9 => "SIGKILL",
        11 => "SIGSEGV",
        _ => "",
    };
    Some(if name.is_empty() {
        format!("signal {number}")
    } else {
        format!("signal {number} ({name})")
    })
}

#[cfg(not(unix))]
fn signal_of(_status: ExitStatus) -> Option<String> {
    None
}
