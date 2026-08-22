//! Process primitives shared across the agent module: poison-tolerant locking, wall-clock
//! stamping, and panic-payload recovery.

use std::sync::Mutex;

/// Lock a `std::sync::Mutex` ignoring poisoning (no panic on a poisoned lock; arch-00 no-panic).
pub(super) fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Wall-clock milliseconds since the Unix epoch — the Rust analogue of Pi `Date.now()`
/// (agent.ts:383,504; agent-loop.ts:741). Used to stamp prompt user messages, tool-result messages,
/// and the synthetic failure message so the value reaches the `convert_to_llm` wire payload exactly
/// as Pi's does. Never panics: a clock before the epoch degrades to `0`.
pub(super) fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Recover a human-readable message from a caught panic payload (Pi
/// `error instanceof Error ? error.message : String(error)`, agent.ts:505). A `panic!`/`unwrap`
/// payload is typically a `&str` or `String`, which we downcast to recover the real text; any other
/// payload type falls back to a generic label.
pub(super) fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "run task failed".to_string()
    }
}
