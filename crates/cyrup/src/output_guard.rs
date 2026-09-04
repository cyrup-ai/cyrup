//! Stdout takeover guard (Pi `core/output-guard.ts`).
//!
//! Pi's `takeOverStdout()` (output-guard.ts:45-70) monkey-patches `process.stdout.write` for the
//! duration of a non-interactive run so that *any* incidental `console.log` — a migration notice, a
//! cross-project session hint, a stray library diagnostic — is redirected to the real **stderr** and
//! can never land in the middle of the machine-readable PRINT/JSON/RPC stream on stdout. Only
//! `writeRawStdout()` (output-guard.ts:85, using the captured original handle) reaches true stdout; it
//! is the sole path the protocol writers use (print-mode.ts:106/115/141, rpc-mode.ts:60).
//!
//! Rust has no supported way to hot-swap the global `std::io::stdout()` the `println!` macro targets
//! (and an fd-level `dup2` redirect would require `unsafe`, which this crate forbids). Pi's takeover is
//! itself a *user-space* swap of its own `process.stdout.write` — it does not redirect the OS file
//! descriptor either (which is exactly why `package-manager.ts:2497` has to redirect a spawned
//! subprocess's stdout to fd 2 separately). So the faithful user-space port is: a process-global
//! takeover flag that the bin's own incidental-stdout write sites route through
//! ([`emit_stray_line`]), while the protocol path stays on real stdout.
//!
//! In cyrup the protocol path is already disciplined: the PRINT/JSON/RPC writers take an injected
//! `Write` sink which `main` binds to `std::io::stdout()` (main.rs, the one-shot/RPC arms). That
//! injected sink is the analog of Pi's `writeRawStdout` — it always reaches true stdout. This module
//! supplies the *other* half Pi's swap provided for free: routing the incidental writes off that
//! stream while the guard is active.

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};

/// pi's `writeRawStdout`/`flushRawStdout` (`core/output-guard.ts:85-108` @v0.84.1) — the OTHER half
/// of this module upstream, and the one that carries the `EAGAIN`/`EWOULDBLOCK`/`ENOBUFS` retry
/// loop (`:20-43`). TOOL-037.
///
/// It lives in `cyrup-modes` rather than here, and the direction of the dependency is why: the
/// consumers are the protocol writers (`json.rs`, `print.rs`), and `cyrup` depends on
/// `cyrup-modes`, not the reverse. Re-exported so this module still names pi's full surface and a
/// reader looking for `writeRawStdout` in the file that ports `output-guard.ts` finds it.
pub use cyrup_modes::{RAW_STDOUT_RETRY_DELAY_MS, flush_raw_stdout, write_raw_stdout};

/// Process-global takeover state — the analog of Pi's module-level `stdoutTakeoverState`
/// (output-guard.ts:7). `false` (not taken over) until `main` installs the guard for a
/// non-interactive run, mirroring Pi's default.
static STDOUT_TAKEN_OVER: AtomicBool = AtomicBool::new(false);

/// Install the stdout guard (Pi `takeOverStdout`, output-guard.ts:45): from now on every
/// [`emit_stray_line`] is rerouted to stderr instead of stdout, keeping the PRINT/JSON/RPC protocol
/// stream on stdout pristine. Idempotent, like Pi's early-return-if-already-taken-over.
pub fn take_over_stdout() {
    STDOUT_TAKEN_OVER.store(true, Ordering::SeqCst);
}

/// Remove the stdout guard (Pi `restoreStdout`, output-guard.ts:72): subsequent [`emit_stray_line`]
/// calls go back to stdout. Called at run teardown, mirroring Pi's `finally { restoreStdout() }`
/// (main.ts:848).
pub fn restore_stdout() {
    STDOUT_TAKEN_OVER.store(false, Ordering::SeqCst);
}

/// Whether the stdout guard is currently installed (Pi `isStdoutTakenOver`, output-guard.ts:81).
pub fn is_stdout_taken_over() -> bool {
    STDOUT_TAKEN_OVER.load(Ordering::SeqCst)
}

/// Write `text` verbatim to the stream a stray stdout write should land on: **stderr** while the
/// guard is installed (Pi's swapped `process.stdout.write` → `rawStderrWrite`, output-guard.ts:54-63),
/// otherwise real **stdout** — exactly where a bare `process.stdout.write`/`console.log` would have
/// gone. Best-effort and never panics on a write/flush error (Pi's `writeRawStdout` retries/ignores
/// `EAGAIN`/`ENOBUFS`; here a failed incidental write is simply dropped rather than aborting the run).
fn write_guarded(text: &str) {
    if is_stdout_taken_over() {
        let mut w = io::stderr().lock();
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
    } else {
        let mut w = io::stdout().lock();
        let _ = w.write_all(text.as_bytes());
        let _ = w.flush();
    }
}

/// Emit incidental text (no trailing newline) — e.g. an inline confirmation prompt — through the
/// stdout guard, the way readline's `rl.question` writes its prompt to `process.stdout` under Pi's
/// takeover swap (main.ts:191-203). Under the guard it lands on stderr so it cannot corrupt the
/// PRINT/JSON/RPC stream on stdout.
pub fn emit_stray(text: &str) {
    write_guarded(text);
}

/// Emit an incidental *line* (Pi `console.log`) through the stdout guard: the text plus a trailing
/// newline. A startup migration notice, a cross-project session-resolution hint. While the guard is
/// installed it is rerouted to stderr so it cannot corrupt a `--mode json`/`--mode rpc`/PRINT stream;
/// otherwise (interactive / plain-metadata commands, where Pi never takes over) it goes to stdout
/// exactly like the original `println!`.
pub fn emit_stray_line(line: &str) {
    write_guarded(&format!("{line}\n"));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    // These tests mutate the process-global takeover flag, so they must not run concurrently with one
    // another. They are grouped into a single `#[test]` and always restore the flag to its `false`
    // default (matching every other test's expectation) before returning.
    #[test]
    fn takeover_flag_roundtrips_like_pi() {
        // Default matches Pi's `stdoutTakeoverState === undefined` (output-guard.ts:7,81).
        assert!(!is_stdout_taken_over());

        take_over_stdout();
        assert!(is_stdout_taken_over());

        // Idempotent, like Pi's early return when already taken over (output-guard.ts:46-48).
        take_over_stdout();
        assert!(is_stdout_taken_over());

        // The stray emitters must not panic in either state (they route to stderr while taken over).
        emit_stray_line("takeover-routing-smoke");
        emit_stray("takeover-prompt-smoke ");

        restore_stdout();
        assert!(!is_stdout_taken_over());
        emit_stray_line("restored-routing-smoke");
        emit_stray("restored-prompt-smoke ");
    }
}
