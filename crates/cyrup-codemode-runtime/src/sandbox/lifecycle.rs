//! The two pieces of shared state that outlive one message: the switch that stops an isolate from
//! outside its thread, and the count of live executions that `close()` waits on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use deno_core::v8::IsolateHandle;
use tokio::sync::Notify;

enum KillState {
    /// No isolate yet.
    Idle,
    /// An isolate that is running or about to.
    Armed(IsolateHandle),
    /// Stopped: an isolate that is armed afterwards must not start.
    Killed,
}

/// Stops an isolate from another thread, including one spinning in `while (true) {}`.
///
/// Upstream sets an interrupt flag the VM polls and then terminates the worker
/// (`host.ts:268-272`, `worker.ts:60`). V8 has one preemption, `terminate_execution`, which must
/// be requested from outside the isolate's thread; it is also what a memory limit uses.
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
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match *state {
            KillState::Killed => false,
            KillState::Idle | KillState::Armed(_) => {
                *state = KillState::Armed(handle);
                true
            }
        }
    }

    /// Terminates the isolate's execution, now or as soon as it is armed. Idempotent.
    pub(super) fn kill(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let KillState::Armed(handle) = std::mem::replace(&mut *state, KillState::Killed) {
            handle.terminate_execution();
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

    /// Resolves once nothing is live.
    pub(super) async fn wait_idle(&self) {
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
