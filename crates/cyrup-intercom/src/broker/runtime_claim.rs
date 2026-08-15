//! The broker's **runtime claim** — a 1:1 port of `pi-intercom` **v0.9.2**
//! `broker/runtime-claim.ts:1-21` (`assertNoLiveBroker`).
//!
//! Called from the `IntercomBroker` constructor at **v0.9.2** `broker/broker.ts:231`, in a very
//! specific slot: immediately AFTER `ensureIntercomRuntimeDir(INTERCOM_DIR)` (`broker.ts:230`) and
//! immediately BEFORE the stale-socket `unlinkSync(LISTEN_TARGET)` (`broker.ts:233-238`). That
//! ordering is the whole point. Without the claim, a second broker starting against a runtime dir
//! that already has a *live* incumbent unlinks the incumbent's socket out from under it and binds
//! its own. The incumbent keeps its already-accepted connections (an unlinked Unix socket inode
//! stays alive for everyone already attached) but becomes permanently unreachable to new clients,
//! so the two brokers silently partition the session graph: every session that registered before
//! the theft can still talk to its peers on the old broker and is invisible to every session that
//! registers after it. `assertNoLiveBroker` turns that silent split-brain into a loud refusal.
//!
//! The file is new since the ported v0.7.0 baseline (`broker/runtime-claim.ts` does not exist in
//! `git ls-tree v0.7.0 broker/`; it arrived with upstream `db22c07`), which is why cyrup's
//! `broker::run` unlinked unconditionally.
//!
//! **Stale sockets must stay reclaimable.** A broker killed with SIGKILL never reaches
//! `shutdown_broker`, so it leaves both `broker.sock` and `broker.pid` behind. If the claim
//! refused on the mere *presence* of a pid file, intercom would deadlock until a human deleted it.
//! The claim is therefore a *liveness* probe, not a file-existence probe: everything short of "a
//! process with that pid is demonstrably still there" yields the runtime to the newcomer.
//!
//! ## Mechanism differences from the TypeScript (Rust is not JavaScript)
//!
//! * Upstream signals refusal by `throw`ing out of a constructor; cyrup returns
//!   `Err(io::Error)` from [`assert_no_live_broker`] so `broker::run`'s existing `io::Result`
//!   entrypoint propagates it to `main` (`src/bin/cyrup-intercom-broker.rs`) as a non-zero exit —
//!   the same observable outcome as an uncaught throw: the process dies before binding anything.
//! * Upstream's `existsSync` pre-check (`runtime-claim.ts:4`) and its `try { readFileSync } catch
//!   { return; }` (`:7-11`) are two spellings of one rule — an unreadable pid file yields the
//!   runtime — so they collapse into a single `read_to_string(..).is_err()` arm here.
//! * `Number.parseInt(text, 10)` + `Number.isSafeInteger` (`:8-12`) are ported explicitly rather
//!   than delegated to `str::parse::<i32>()`, because the two disagree on exactly the inputs a
//!   corrupted pid file produces: `parseInt` accepts a digit *prefix* (`"123abc"` → `123`, a pid
//!   upstream goes on to probe) and rejects a value outside `±(2^53-1)`. See [`parse_pid`].
//! * `process.kill(pid, 0)` (`:15`) becomes `nix::sys::signal::kill(pid, None)`. Upstream only
//!   swallows `ESRCH` (`:17`) and re-throws every other errno (`:18`); this port keeps that
//!   asymmetry, which matters for `EPERM` — a pid owned by another user IS a live process, and
//!   treating it as dead would resurrect exactly the socket theft the claim exists to prevent.
//!   (Note this is deliberately stricter than [`crate::transport::spawn`]'s `pid_alive`, whose
//!   upstream — `spawn.ts` `isBrokerRunning` — wraps the same call in a bare `catch { return
//!   false; }` and so does treat `EPERM` as dead.)

use std::path::Path;

/// `Number.MAX_SAFE_INTEGER` (`2^53 - 1`) — the bound `Number.isSafeInteger` enforces at
/// **v0.9.2** `broker/runtime-claim.ts:12`.
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Refuse to start when `pid_path` names a **live** broker process
/// (`assertNoLiveBroker`, **v0.9.2** `broker/runtime-claim.ts:3-21`).
///
/// Yields the runtime (`Ok(())`) when the pid file is absent, unreadable, unparseable, non-positive,
/// outside the safe-integer range, or names a process that no longer exists (`ESRCH`) — i.e. every
/// flavour of *stale*, so a crashed broker's leftovers never wedge intercom.
///
/// # Errors
/// Returns [`std::io::ErrorKind::AddrInUse`] when the recorded pid is still alive, or when its
/// liveness cannot be established as *absence* (any errno other than `ESRCH`, e.g. `EPERM` for a
/// process owned by another user — upstream re-throws those at `runtime-claim.ts:18`).
pub fn assert_no_live_broker(pid_path: &Path) -> std::io::Result<()> {
    // `if (!existsSync(pidPath)) return;` (`runtime-claim.ts:4`) + `catch { return; }` (`:9-11`).
    let Ok(raw) = std::fs::read_to_string(pid_path) else {
        return Ok(());
    };
    // `Number.parseInt(...trim(), 10)` (`:8`) + `if (!Number.isSafeInteger(pid) || pid <= 0) return;`
    // (`:12`).
    let Some(pid) = parse_pid(raw.trim()) else {
        return Ok(());
    };

    // Node's `process.kill` validates `pid === (pid | 0)` and throws a `TypeError` for a safe
    // integer that is not an int32 — a throw that is not `ESRCH`, so `runtime-claim.ts:16-18`
    // re-throws it and startup is refused. Same verdict here: a pid we cannot even hand to
    // `kill(2)` is not evidence that the incumbent is gone.
    let Ok(pid) = i32::try_from(pid) else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("Refusing to replace live intercom broker process {pid}: not a valid process id"),
        ));
    };

    match probe_pid(pid) {
        #[cfg(unix)]
        PidProbe::Gone => Ok(()),
        // `throw new Error(\`Refusing to replace live intercom broker process ${pid}\`)` (`:20`).
        #[cfg(unix)]
        PidProbe::Live => Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("Refusing to replace live intercom broker process {pid}"),
        )),
        #[cfg(unix)]
        PidProbe::Undetermined(errno) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("Refusing to replace live intercom broker process {pid}: {errno}"),
        )),
        // Same limb as `Undetermined`: not proven absent ⇒ not reclaimable. The message names the
        // recovery (delete the pid file) because, unlike `EPERM`, this verdict does not clear on
        // its own.
        #[cfg(not(unix))]
        PidProbe::Unsupported => Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!(
                "Refusing to replace intercom broker process {pid}: process liveness cannot be \
                 probed on this platform. If that process is gone, delete {}.",
                pid_path.display()
            ),
        )),
    }
}

/// The outcome of a `process.kill(pid, 0)` liveness probe, keeping upstream's three-way split
/// (**v0.9.2** `broker/runtime-claim.ts:14-19`) instead of collapsing to a bool: only `ESRCH`
/// proves *absence*, and every other errno is re-thrown rather than read as "dead".
enum PidProbe {
    /// The signal was accepted — the process exists.
    #[cfg(unix)]
    Live,
    /// `ESRCH`: no such process (`runtime-claim.ts:17`).
    #[cfg(unix)]
    Gone,
    /// Any other errno — upstream re-throws (`runtime-claim.ts:18`). `EPERM` is the realistic one:
    /// the process exists, we just may not signal it.
    #[cfg(unix)]
    Undetermined(nix::errno::Errno),
    /// No liveness probe exists on this platform at all — see [`probe_pid`]'s `CYRUP-DELTA`.
    #[cfg(not(unix))]
    Unsupported,
}

/// `process.kill(pid, 0)` (**v0.9.2** `broker/runtime-claim.ts:15`).
///
/// # [CYRUP-DELTA] — the non-Unix arm cannot probe, and therefore must not answer `Gone`
///
/// Upstream symbol: `process.kill(pid, 0)` at **v0.9.2** `broker/runtime-claim.ts:15`. Node
/// implements `process.kill` on Windows too (`OpenProcess`), so upstream gets a real three-way
/// answer on every platform. Rust's std has no portable equivalent and `nix` is POSIX-only, so a
/// faithful Windows probe would mean a new `windows-sys` dependency — a workspace-level decision
/// this crate cannot make on its own.
///
/// What the arm must NOT do is guess. It previously returned [`PidProbe::Gone`] for every pid,
/// justified by "the broker is unix-only, nothing can reach this" — a justification that expires
/// the moment the broker listens on a Windows named pipe (see `broker::listener`), at which point
/// every starting `cyrup` would declare a live incumbent dead, unlink its endpoint, and silently
/// partition the session graph: exactly the split-brain this whole module exists to prevent.
///
/// So the non-Unix arm takes the same limb a live-but-unsignalable pid takes — upstream re-throws
/// every outcome that is not `ESRCH` (`:18`), and "no probe exists" is certainly not `ESRCH`. This
/// matches the sibling `check_pid_liveness` in
/// `cyrup-ext-subagents/src/background/reconcile.rs`, which reports `Liveness::Unknown` rather than
/// `Dead` on non-Unix for the same reason. The cost is that a pid file left by a SIGKILLed Windows
/// broker wedges intercom until it is deleted; the error text says so. A loud, one-command wedge is
/// the correct trade against a silent fabric split.
fn probe_pid(pid: i32) -> PidProbe {
    #[cfg(unix)]
    {
        match nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None) {
            Ok(()) => PidProbe::Live,
            Err(nix::errno::Errno::ESRCH) => PidProbe::Gone,
            Err(errno) => PidProbe::Undetermined(errno),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        PidProbe::Unsupported
    }
}

/// `Number.parseInt(text, 10)` followed by `Number.isSafeInteger(pid) && pid > 0`
/// (**v0.9.2** `broker/runtime-claim.ts:8,12`), fused into one step: `Some(pid)` exactly for the
/// values upstream goes on to probe, `None` for every value it returns early on.
///
/// Ported explicitly because `str::parse::<i32>()` is not `parseInt`:
/// * `parseInt` consumes an optional sign then a **prefix** of ASCII digits and ignores the rest,
///   so `"4321 (crashed)"` is the pid `4321` upstream probes — `str::parse` would reject it and
///   this port would wrongly reclaim a live broker's socket.
/// * `parseInt` yields `NaN` when no digits lead, and `isSafeInteger` then rejects it.
/// * `isSafeInteger` also rejects `|value| > 2^53 - 1`, which `str::parse::<i64>()` would accept.
fn parse_pid(text: &str) -> Option<u64> {
    // `parseInt`'s optional leading sign. A negative value cannot survive `pid <= 0` (`:12`), and
    // `"-0"` is `-0`, which is `<= 0` too — so either way a `-` prefix yields the runtime.
    let digits_start = match text.strip_prefix('-') {
        Some(_) => return None,
        None => text.strip_prefix('+').unwrap_or(text),
    };
    let digits: &str = match digits_start.find(|c: char| !c.is_ascii_digit()) {
        Some(end) => digits_start.get(..end)?,
        None => digits_start,
    };
    // No leading digits → `NaN` → `!Number.isSafeInteger(NaN)` → return (`:12`).
    let significant = digits.trim_start_matches('0');
    // All zeros (or empty) → `0` or `NaN`, both caught by `:12`.
    if significant.is_empty() {
        return None;
    }
    // 17+ significant digits is `>= 10^16 > 2^53 - 1`, so it can never be a safe integer — checked
    // before `parse` so an absurdly long pid file cannot overflow `u64` into a wrong answer.
    if significant.len() > 16 {
        return None;
    }
    let value: u64 = significant.parse().ok()?;
    if value > MAX_SAFE_INTEGER { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
    use super::*;

    /// Mirrors upstream `v0.9.2 broker/runtime-claim.test.ts:8-20` ("broker startup refuses to
    /// replace a live broker PID"): the test process's own pid is by definition live.
    /// The exact refusal text is upstream's (`runtime-claim.ts:20`) and is asserted on the Unix arm,
    /// where `kill(pid, 0)` returns `Ok` and the verdict is genuinely `Live`. The cross-platform
    /// half of the claim ("a live pid is never replaced") is
    /// `a_live_pid_is_refused_on_every_platform`.
    #[cfg(unix)]
    #[test]
    fn a_live_pid_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");
        std::fs::write(&pid_path, format!("{}\n", std::process::id())).unwrap();

        let err = assert_no_live_broker(&pid_path).expect_err("a live broker must not be replaced");
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
        assert_eq!(
            err.to_string(),
            format!("Refusing to replace live intercom broker process {}", std::process::id())
        );
    }

    /// Mirrors upstream `v0.9.2 broker/runtime-claim.test.ts:22-34` ("broker startup tolerates
    /// absent, invalid, and stale PID files") — the reclaim half of the contract, which is what
    /// keeps a SIGKILLed broker from wedging intercom until a human deletes a file.
    #[test]
    fn absent_invalid_and_stale_pid_files_yield_the_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");

        // Absent (`runtime-claim.test.ts:26`).
        assert!(assert_no_live_broker(&pid_path).is_ok());

        // Invalid (`:27-28`): `parseInt("invalid", 10)` is `NaN`.
        std::fs::write(&pid_path, "invalid\n").unwrap();
        assert!(assert_no_live_broker(&pid_path).is_ok());

        // Stale (`:29-30`): upstream's own choice of an unallocatable pid. Reclaim requires a probe
        // that can PROVE absence (`ESRCH`), which only the Unix arm can do — see
        // `probe_pid`'s CYRUP-DELTA and `a_pid_that_cannot_be_probed_is_never_reclaimed` below.
        #[cfg(unix)]
        {
            std::fs::write(&pid_path, "2147483647\n").unwrap();
            assert!(assert_no_live_broker(&pid_path).is_ok());
        }
    }

    /// Both cfg arms of [`probe_pid`], asserted through the public entrypoint, because the two arms
    /// answered the SAME question with OPPOSITE defaults: the Unix arm distinguishes
    /// `Live`/`Gone`/`Undetermined`, while the non-Unix arm used to answer `Gone` for every pid —
    /// live or not.
    ///
    /// That was latent only for as long as the crate refused to compile for Windows at all. With the
    /// broker's listener now gated rather than the whole crate (`broker::listener`), a `Gone`
    /// default would make every starting `cyrup` on Windows declare a live incumbent dead, unlink
    /// its endpoint and split the intercom fabric — silently.
    ///
    /// The rule under test is upstream's, not an invention: `runtime-claim.ts:16-18` swallows only
    /// `ESRCH` and re-throws every other outcome. "This platform has no probe" is not `ESRCH`.
    #[test]
    fn a_pid_that_cannot_be_probed_is_never_reclaimed() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");
        // Upstream's own stale-pid fixture (`runtime-claim.test.ts:29`): unallocatable, so on Unix
        // `kill(2)` answers ESRCH and this is the reclaimable case.
        std::fs::write(&pid_path, "2147483647\n").unwrap();

        #[cfg(unix)]
        {
            // The probe can prove absence, so the runtime is yielded — pi's behaviour verbatim.
            assert!(
                assert_no_live_broker(&pid_path).is_ok(),
                "ESRCH is the one outcome upstream reads as absence"
            );
        }
        #[cfg(not(unix))]
        {
            // No probe exists, so absence is unproven and the claim must refuse rather than guess.
            let err = assert_no_live_broker(&pid_path)
                .expect_err("an unprobeable pid must not be treated as dead");
            assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
            let text = err.to_string();
            assert!(text.contains("cannot be probed"), "{text}");
            // The wedge has to be recoverable, so the message names the file to delete.
            assert!(text.contains(&pid_path.display().to_string()), "{text}");
        }
    }

    /// MIRROR (both arms): a live pid is refused on every platform. On Unix that is `kill` → `Ok`;
    /// on non-Unix it is the unprobeable limb. Either way the incumbent is never replaced, which is
    /// the property `assert_no_live_broker` exists for.
    #[test]
    fn a_live_pid_is_refused_on_every_platform() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");
        std::fs::write(&pid_path, format!("{}\n", std::process::id())).unwrap();

        let err = assert_no_live_broker(&pid_path).expect_err("a live broker must not be replaced");
        assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);
        assert!(err.to_string().contains(&std::process::id().to_string()), "{err}");
    }

    /// The other early-return arms of `runtime-claim.ts:12`, which upstream's own suite does not
    /// enumerate: a non-positive pid and an out-of-safe-range pid both yield the runtime.
    #[test]
    fn non_positive_and_unsafe_pids_yield_the_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");

        for raw in ["0\n", "-1\n", "-0\n", "  \n", "9007199254740992\n", "99999999999999999999\n"] {
            std::fs::write(&pid_path, raw).unwrap();
            assert!(
                assert_no_live_broker(&pid_path).is_ok(),
                "a pid file of {raw:?} is not evidence of a live broker"
            );
        }
    }

    /// MIRROR CASE for the `parseInt`-vs-`str::parse` divergence: a pid file whose digits are
    /// followed by garbage still names a live process upstream, so tolerance must NOT extend to it.
    /// `str::parse::<i32>()` would have rejected the whole string and reclaimed a live broker.
    #[test]
    fn a_live_pid_with_a_trailing_garbage_suffix_is_still_refused() {
        let dir = tempfile::tempdir().unwrap();
        let pid_path = dir.path().join("broker.pid");
        std::fs::write(&pid_path, format!("{} (crashed?)\n", std::process::id())).unwrap();

        let err = assert_no_live_broker(&pid_path).expect_err("parseInt reads the digit prefix");
        assert!(err.to_string().contains(&std::process::id().to_string()), "{err}");
    }

    #[test]
    fn parse_pid_matches_parse_int_plus_is_safe_integer() {
        assert_eq!(parse_pid("123"), Some(123));
        assert_eq!(parse_pid("+123"), Some(123));
        assert_eq!(parse_pid("0123"), Some(123));
        assert_eq!(parse_pid("123abc"), Some(123));
        assert_eq!(parse_pid("2147483647"), Some(2_147_483_647));
        assert_eq!(parse_pid(&MAX_SAFE_INTEGER.to_string()), Some(MAX_SAFE_INTEGER));

        assert_eq!(parse_pid(""), None);
        assert_eq!(parse_pid("abc"), None);
        assert_eq!(parse_pid("abc123"), None, "parseInt only reads a digit PREFIX");
        assert_eq!(parse_pid("0"), None);
        assert_eq!(parse_pid("-5"), None);
        assert_eq!(parse_pid("9007199254740992"), None, "2^53 is not a safe integer");
        assert_eq!(parse_pid("12345678901234567890"), None);
    }
}
