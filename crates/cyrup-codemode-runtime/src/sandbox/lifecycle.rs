//! The two pieces of shared state that outlive one message: the switch that stops an isolate from
//! outside its thread, and the count of live executions that `close()` waits on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use deno_core::v8::IsolateHandle;
use tokio::sync::Notify;

/// What stops a running script: `terminate_execution` on an isolate in this process, or the death
/// of the sandbox process that holds it.
type Stopper = Box<dyn FnOnce() + Send>;

enum KillState {
    /// Nothing to stop yet.
    Idle,
    /// An isolate or a sandbox process that is running or about to.
    Armed(Stopper),
    /// Stopped: whatever is armed afterwards must not start.
    Killed,
}

/// Stops an isolate from another thread, including one spinning in `while (true) {}`, or kills
/// the process that runs it.
///
/// Upstream sets an interrupt flag the VM polls and then terminates the worker
/// (`host.ts:268-272`, `worker.ts:60`). V8 has one preemption, `terminate_execution`, which must
/// be requested from outside the isolate's thread; it is also what a memory limit uses. A script
/// in a sandbox process is stopped by killing the process, which also stops a native built-in
/// that never looks at `terminate_execution`.
///
/// Arming and killing take the same lock, so a kill that arrives while the isolate is being built
/// is not lost: [`KillSwitch::arm`] then refuses, and the thread exits without running a script.
pub(super) struct KillSwitch {
    state: Mutex<KillState>,
}

impl KillSwitch {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(KillState::Idle),
        }
    }

    /// Hands the isolate's handle over. `false`: already killed, do not start the script.
    pub(super) fn arm(&self, handle: IsolateHandle) -> bool {
        self.arm_with(move || {
            handle.terminate_execution();
        })
    }

    /// Hands over whatever stops the script. `false`: already killed, do not start the script.
    pub(super) fn arm_with(&self, stop: impl FnOnce() + Send + 'static) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match *state {
            KillState::Killed => false,
            KillState::Idle | KillState::Armed(_) => {
                *state = KillState::Armed(Box::new(stop));
                true
            }
        }
    }

    /// Stops the script, now or as soon as it is armed. Idempotent.
    pub(super) fn kill(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let KillState::Armed(stop) = std::mem::replace(&mut *state, KillState::Killed) {
            stop();
        }
    }
}

/// Executions and isolate threads that have not finished (pi `Sandbox.running`, `host.ts:292`,
/// extended to the threads, which upstream awaits in `worker.terminate()`).
#[derive(Default)]
pub(super) struct Running {
    count: AtomicUsize,
    idle: Notify,
}

impl Running {
    /// One more live thing; it ends when the guard drops.
    pub(super) fn enter(self: &Arc<Self>) -> RunningGuard {
        self.count.fetch_add(1, Ordering::SeqCst);
        RunningGuard(Arc::clone(self))
    }

    pub(super) fn live(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    /// Resolves once nothing is live, or after `limit` if something still is. `true`: idle.
    ///
    /// An isolate thread that is stuck inside a native built-in never looks at
    /// `terminate_execution`, and a thread cannot be killed; waiting for it forever would hang
    /// whoever closes the sandbox. After `limit` the thread is left behind (detached): it keeps
    /// counting in [`Running::live`] and ends whenever the built-in returns.
    pub(super) async fn wait_idle(&self, limit: Duration) -> bool {
        tokio::time::timeout(limit, self.until_idle()).await.is_ok()
    }

    async fn until_idle(&self) {
        loop {
            let notified = self.idle.notified();
            tokio::pin!(notified);
            // Registered before the count is read, so a guard that drops in between is not missed.
            notified.as_mut().enable();
            if self.live() == 0 {
                return;
            }
            notified.await;
        }
    }
}

pub(super) struct RunningGuard(Arc<Running>);

impl Drop for RunningGuard {
    fn drop(&mut self) {
        if self.0.count.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}
