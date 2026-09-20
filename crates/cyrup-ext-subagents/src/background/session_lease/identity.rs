//! [`ProcessStartIdentity`] — pi `getProcessStartIdentity`'s return value
//! (`runs/shared/session-lease.ts:68-82` @v0.68.0) — its probe, and
//! [`process_demonstrably_gone`] (`:162-172`), the predicate the staleness ladder is built on.

use crate::background::reconcile::{Liveness, check_pid_identity_with};

/// A process's START identity: the one fact about a pid that a RECYCLED pid cannot reproduce.
///
/// On Linux the value is `linux:<startTicks>`, read from `/proc/<pid>/stat` field 20
/// (`starttime`), counted AFTER the last `)` so a command name containing spaces or parentheses
/// cannot shift the field index (pi `:72-75`). Upstream answers `undefined` on every other
/// platform and on any read or parse failure.
///
/// # Why the type exists at all
///
/// `kill(pid, 0)` — [`check_pid_liveness`](crate::background::reconcile::check_pid_liveness) — can
/// only answer "some process holds this pid". A lease owner that died and whose pid was handed to
/// an unrelated process therefore reads ALIVE forever, and the lease is never reclaimable. The
/// start identity is what turns "alive" into "alive, and it is still the SAME process": pi
/// `processDemonstrablyGone` (`:162-172`) declares an owner gone when the pid is dead OR the pid
/// is alive under a DIFFERENT start identity.
///
/// On the wire it is a plain string (upstream's `processStartIdentity?: string`), so the on-disk
/// owner record is unchanged.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct ProcessStartIdentity(String);

impl ProcessStartIdentity {
    /// Wraps an already-known identity token — one parsed back out of an `owner.json`, or one a
    /// test injects. Non-validating by design: the token's grammar is a platform detail
    /// (`linux:…`, and upstream's `runtime:…` fallback), and the only operation performed on it
    /// anywhere is EQUALITY against another token read from the same machine.
    #[must_use]
    pub fn from_token(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// Borrows the token.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ProcessStartIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// pi `getProcessStartIdentity` (`:68-82`) — the ONE `/proc/<pid>/stat` parse in this crate.
///
/// Field 22 of `/proc/<pid>/stat` (1-based; index 19 of the fields AFTER the parenthesised
/// command, which is how upstream counts it at `:75`) is `starttime`, the number of clock ticks
/// after boot at which the process started. A pid plus its `starttime` is unique for the life of
/// a boot, which is exactly what makes "the pid is alive but under a different start identity"
/// mean "the pid was recycled".
///
/// The command field is skipped from the LAST `)` rather than the first, because a process name
/// may itself contain parentheses — that is why upstream uses `lastIndexOf` and why this uses
/// [`str::rfind`].
///
/// `None` on any platform or any pid this cannot answer for. `None` NEVER means "gone": every
/// caller reads it as "this rung cannot speak", which is the only safe direction — an absent
/// identity that read as death would kill live runs.
///
/// [`crate::background::async_retention::lock`]'s retention lock consumes this same function; it
/// is deliberately not a second copy, because two `/proc` parsers that drift produce two
/// incompatible answers about the same pid.
#[must_use]
pub fn process_start_identity(pid: u32) -> Option<ProcessStartIdentity> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_command = stat.get(stat.rfind(')')? + 1..)?;
    let start_ticks = after_command.split_whitespace().nth(19)?;
    Some(ProcessStartIdentity::from_token(format!(
        "linux:{start_ticks}"
    )))
}

/// pi's third rung (`:210-212`) — `pid === process.pid ? \`runtime:${…uptime…}\` : undefined`.
///
/// Upstream derives the value from `process.uptime()`, which Rust's std does not expose on any
/// platform. The value's contract is narrower than "the exec time", and this satisfies it: it must
/// be CONSTANT for the life of this process and must not be reproducible by a later process that
/// inherits this pid. A timestamp captured once, on the first call, is both — the
/// [`std::sync::OnceLock`] is what makes it once, and a recycled pid belongs to a process that
/// started later and therefore captures a later value.
///
/// # When this rung is reached, and when it is compared
///
/// On Linux [`process_start_identity`] always answers for a live pid, so this is the non-Linux and
/// procless-container path only. There, the value is WRITTEN into `owner.json` but never compared:
/// a later reader asks [`process_start_identity`] about the owner's pid, gets `None`, and
/// [`process_demonstrably_gone`] stops at its second rung. Recording it anyway is upstream's
/// behaviour and is what keeps the field present for an operator reading the file, rather than
/// silently absent on half the platforms this crate builds for.
#[must_use]
pub fn runtime_start_identity() -> ProcessStartIdentity {
    static CAPTURED: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    let ms = *CAPTURED.get_or_init(crate::time::now_epoch_millis);
    ProcessStartIdentity::from_token(format!("runtime:{ms}"))
}

/// pi `processDemonstrablyGone` (`:162-172`) — the predicate every rung of the staleness ladder
/// is built out of.
///
/// `true` only on POSITIVE evidence of absence, in exactly upstream's two shapes:
///
/// 1. the pid is CONFIRMED dead ([`Liveness::Dead`], upstream's `alive === false`, `:168`); or
/// 2. the pid is confirmed ALIVE, an identity was recorded for it, and the identity observed right
///    now DIFFERS (`:169-171`) — the pid was recycled.
///
/// Everything else is `false`: [`Liveness::Unknown`] (upstream's `alive === undefined`, an
/// `EPERM`-class probe under sandboxing), an owner with no recorded identity, and an identity the
/// current platform cannot re-read. Each of those is an inconclusive check, and treating one as
/// death would steal a lease from a live runner — the exact hazard the lease exists to prevent.
///
/// Both ambient inputs are injected, as upstream injects them (`:165`), so a test can present a
/// dead owner or a recycled pid without owning one. The ladder itself is
/// [`check_pid_identity_with`]: this function is that verdict compared against
/// [`Liveness::Dead`], never a second copy of it.
#[must_use]
pub fn process_demonstrably_gone(
    pid: u32,
    start_identity: Option<&ProcessStartIdentity>,
    liveness: fn(u32) -> Liveness,
    start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
) -> bool {
    check_pid_identity_with(pid, start_identity, liveness, start_identity_of) == Liveness::Dead
}
