//! The sandbox process: what runs in the process [`super::process`] starts.
//!
//! The host binary re-executes itself with [`super::HOST_SUBCOMMAND`] and calls [`run_sandbox_process`].
//! The process reads one [`ToProcess::Start`] frame from its stdin, runs that script in an isolate
//! on a thread of its own with the same code the in-process sandbox runs
//! ([`isolate::run_thread`]), and relays the isolate's messages to its stdout and the supervisor's
//! replies from its stdin. Nothing else is written to stdout: it is the supervisor's pipe.
//!
//! The process holds nothing the supervisor needs, so it dies freely. It ends when the script
//! settles, when its stdin closes (the supervisor is gone or finished), when the supervisor kills
//! it (a deadline, a cancel, `close()`), or when the engine aborts, which no script can prevent:
//! one allocation the heap cannot satisfy is a fatal out-of-memory in V8 (measured: `new
//! Array(2 ** 27).fill(0)` under a 256 MiB heap limit). The supervisor reports each of these as a
//! failed script and carries on.

use std::ffi::c_char;
use std::io::{self, BufReader};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use deno_core::v8;
use tokio::sync::mpsc::{WeakUnboundedSender, unbounded_channel};

use super::execution::ISOLATE_THREAD_STACK_BYTES;
use super::isolate;
use super::lifecycle::{KillSwitch, Running};
use super::protocol::{HostReply, IsolateInit, WorkerMessage};
use super::wire::{FromProcess, ToProcess, read_frame, write_frame};

/// Address space and memory a sandbox process needs beyond the script's own budget: the engine, the
/// binary's data, thread stacks and the allocator's arenas. Measured on a debug build of the `cyrup`
/// binary: an idle sandbox process held 325 MB of writable memory (`VmData`) and 78 MB resident; a
/// release build needs less.
const PROCESS_BASELINE_BYTES: usize = 768 * 1024 * 1024;

/// How long the process waits for its writer to put the out-of-memory report on the pipe before it
/// lets the engine abort.
const FATAL_OOM_FLUSH: Duration = Duration::from_secs(2);

/// The process's way to report a fatal out-of-memory from the engine's handler, which runs on the
/// isolate thread while the allocation that failed is still pending.
struct FatalOomReport {
    /// Weak, so that this static does not keep the writer's channel open after the isolate ends.
    to_host: WeakUnboundedSender<WorkerMessage>,
    /// Signalled by the writer once the report is on the pipe.
    written: Mutex<mpsc::Receiver<()>>,
}

static FATAL_OOM: OnceLock<FatalOomReport> = OnceLock::new();

/// Whether this process reports a fatal out-of-memory itself (only a sandbox process does; in the
/// host's own process the engine aborts, as it always has).
pub(super) fn reports_fatal_out_of_memory() -> bool {
    FATAL_OOM.get().is_some()
}

/// The engine's out-of-memory handler. V8 aborts the process when it returns; the supervisor reads
/// the abort as a crash unless the report got there first. `extern "C"` because V8 calls it through
/// its C ABI.
pub(super) extern "C" fn on_fatal_out_of_memory(
    _location: *const c_char,
    _details: &v8::OomDetails,
) {
    let Some(report) = FATAL_OOM.get() else {
        return;
    };
    let Some(to_host) = report.to_host.upgrade() else {
        return;
    };
    if to_host.send(WorkerMessage::OutOfMemory).is_err() {
        return;
    }
    let written = report
        .written
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let _ = written.recv_timeout(FATAL_OOM_FLUSH);
}

/// Runs the sandbox process to its end and returns its exit code. Blocks; call it from a thread
/// that is not inside a tokio runtime context's async task (the isolate has its own runtime).
#[must_use]
pub fn run_sandbox_process() -> i32 {
    let mut input = BufReader::new(io::stdin());
    let mut scratch = Vec::new();
    let init = match read_frame::<ToProcess>(&mut input, &mut scratch) {
        Ok(Some(ToProcess::Start(init))) => init,
        // The supervisor went away before it said what to run, or said something else.
        Ok(Some(ToProcess::Reply(_)) | None) => return 0,
        Err(error) => {
            eprintln!("codemode sandbox: unreadable start frame: {error}");
            return 70;
        }
    };
    limit_resources(&init);

    let (to_host, mut messages) = unbounded_channel::<WorkerMessage>();
    let (reply_tx, from_host) = unbounded_channel::<HostReply>();
    let (written_tx, written_rx) = mpsc::channel::<()>();
    // Ignored when already set: this function runs once per process.
    let _ = FATAL_OOM.set(FatalOomReport {
        to_host: to_host.downgrade(),
        written: Mutex::new(written_rx),
    });

    let writer = std::thread::Builder::new()
        .name(String::from("codemode-sandbox-writer"))
        .spawn(move || {
            let mut out = io::stdout().lock();
            while let Some(message) = messages.blocking_recv() {
                let fatal = matches!(message, WorkerMessage::OutOfMemory);
                if write_frame(&mut out, &FromProcess::from(message)).is_err() {
                    // The supervisor is gone.
                    std::process::exit(0);
                }
                if fatal {
                    let _ = written_tx.send(());
                }
            }
        });
    let Ok(writer) = writer else {
        return 70;
    };

    // A closed stdin is the supervisor saying it no longer wants the process.
    let reader = std::thread::Builder::new()
        .name(String::from("codemode-sandbox-reader"))
        .spawn(move || {
            while let Ok(Some(ToProcess::Reply(reply))) =
                read_frame::<ToProcess>(&mut input, &mut scratch)
            {
                // A reply the isolate no longer wants (the script ended while a call it did not
                // await was in flight) is dropped. It must not end the process: the writer may
                // still be putting the script's end on the pipe, and only `run_sandbox_process`
                // knows when it is done.
                let _ = reply_tx.send(reply);
            }
            std::process::exit(0);
        });
    if reader.is_err() {
        return 70;
    }

    let running = Arc::new(Running::default());
    let isolate = std::thread::Builder::new()
        .name(String::from("codemode-isolate"))
        .stack_size(ISOLATE_THREAD_STACK_BYTES)
        .spawn({
            let guard = running.enter();
            move || {
                isolate::run_thread(init, to_host, from_host, Arc::new(KillSwitch::new()), guard)
            }
        });
    let Ok(isolate) = isolate else {
        return 70;
    };
    let _ = isolate.join();
    // The isolate's sender went with its thread, so the writer drains what is queued and ends.
    let _ = writer.join();
    0
}

/// Caps what the process can take from the machine, whatever the script does or the engine fails
/// to count: no core file (a crashed engine would otherwise write its whole address space to
/// disk), and a ceiling on writable memory that the heap limit, the `ArrayBuffer` budget and the
/// engine's own needs fit under. Past it an allocation fails inside the process, which reports an
/// out-of-memory or crashes; the host's memory is never asked.
///
/// Best effort: a platform without the limit (`RLIMIT_DATA` is not enforced on macOS, and Windows
/// has no `setrlimit`) runs without it, still in a process of its own.
#[cfg(unix)]
pub(super) fn limit_resources(init: &IsolateInit) {
    use nix::libc::rlim_t;
    use nix::sys::resource::{Resource, setrlimit};

    let _ = setrlimit(Resource::RLIMIT_CORE, 0, 0);
    let ceiling = process_ceiling(init.memory_limit);
    let ceiling = rlim_t::try_from(ceiling).unwrap_or(rlim_t::MAX);
    let _ = setrlimit(Resource::RLIMIT_DATA, ceiling, ceiling);
}

#[cfg(not(unix))]
pub(super) fn limit_resources(_init: &IsolateInit) {}

/// The writable-memory ceiling for a script with `memory_limit` bytes of heap: the heap (with the
/// headroom the near-limit callback grants), the same number again for memory behind `ArrayBuffer`s
/// (the prelude's budget), and [`PROCESS_BASELINE_BYTES`].
pub(super) fn process_ceiling(memory_limit: Option<usize>) -> usize {
    let (heap, external) = isolate::memory_budgets(memory_limit);
    heap.saturating_add(external)
        .saturating_add(PROCESS_BASELINE_BYTES)
}
