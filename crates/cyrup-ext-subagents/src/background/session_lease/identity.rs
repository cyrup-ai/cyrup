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

    /// The probe that wrote the token: what precedes its first `:` (`linux`, `start`, `runtime`),
    /// or the empty string for a token without one. Two tokens are comparable only when their
    /// schemes agree ([`check_pid_identity_with`]).
    #[must_use]
    pub fn scheme(&self) -> &str {
        self.0.split_once(':').map_or("", |(scheme, _)| scheme)
    }
}

impl std::fmt::Display for ProcessStartIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// pi `getProcessStartIdentity` (`:68-82`), widened off Linux: the start identity of `pid`, or
/// `None` when this platform cannot read one for it.
///
/// On Linux it is [`linux_start_identity`], upstream's `/proc` read, unchanged. Everywhere else
/// upstream answers `undefined`, so a pid that a later process inherited read alive for ever; there
/// this answers [`portable_start_identity`] instead (\[CYRUP-DELTA], PERM-045(b)). The two tokens
/// carry different schemes (`linux:` and `start:`), and [`check_pid_identity_with`] compares only
/// tokens of one scheme, so a record one probe wrote is never judged by the other.
///
/// `None` NEVER means "gone": every caller reads it as "this rung cannot speak", which is the only
/// safe direction — an absent identity that read as death would kill live runs.
///
/// [`crate::background::async_retention::lock`]'s retention lock consumes this same function; it
/// is deliberately not a second copy, because two `/proc` parsers that drift produce two
/// incompatible answers about the same pid.
#[must_use]
pub fn process_start_identity(pid: u32) -> Option<ProcessStartIdentity> {
    if cfg!(target_os = "linux") {
        linux_start_identity(pid)
    } else {
        portable_start_identity(pid)
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
/// `None` wherever `/proc` cannot answer for `pid` (every platform but Linux, a procless container).
#[must_use]
pub fn linux_start_identity(pid: u32) -> Option<ProcessStartIdentity> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_command = stat.get(stat.rfind(')')? + 1..)?;
    let start_ticks = after_command.split_whitespace().nth(19)?;
    Some(ProcessStartIdentity::from_token(format!(
        "linux:{start_ticks}"
    )))
}

/// \[CYRUP-DELTA] (PERM-045(b)) The start identity of `pid` where `/proc` is not there to read:
/// `start:<seconds since the epoch>`, the process's start time as the OS reports it (`proc_pidinfo`
/// on macOS, `GetProcessTimes` on Windows, `/proc` again on Linux, through `sysinfo`).
///
/// The production probe off Linux only ([`process_start_identity`]); on Linux it is compiled and
/// tested, and not used, because the `linux:` token is upstream's and is finer.
///
/// Its grain is one second: a pid that is handed to a new process within the second its first
/// holder started reads as the same process, which is the safe direction (alive), and no worse than
/// upstream, which cannot tell at all. `None` for pid 0 (a process `kill(2)` cannot name; macOS's
/// `kernel_task` and Windows's idle process hold it), for a pid the OS does not report, and for a
/// start time of 0 (not reported).
#[must_use]
pub fn portable_start_identity(pid: u32) -> Option<ProcessStartIdentity> {
    if pid == 0 {
        return None;
    }
    let pid = sysinfo::Pid::from_u32(pid);
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        false,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    let started = system.process(pid)?.start_time();
    (started > 0).then(|| ProcessStartIdentity::from_token(format!("start:{started}")))
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
/// On Linux [`process_start_identity`] always answers for a live pid, and off Linux
/// [`portable_start_identity`] does (PERM-045(b)), so this is the path of a platform where neither
/// can read this very process (a procless container). There, the value is WRITTEN into
/// `owner.json` but never compared: a later reader's probe answers `None` or a token of another
/// scheme, and [`check_pid_identity_with`] compares only tokens of one scheme, so
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

/// The owner stamp a cross-process record carries (pid, start identity, hostname and, where the
/// writer recorded one, its PID namespace), borrowed for [`owner_demonstrably_gone`].
#[derive(Clone, Copy, Debug)]
pub struct OwnerStamp<'a> {
    /// The writer's pid, as its own PID namespace numbers it.
    pub pid: u32,
    /// The writer's [`ProcessStartIdentity`], when one was recorded.
    pub start_identity: Option<&'a ProcessStartIdentity>,
    /// The writer's hostname.
    pub hostname: &'a str,
    /// The writer's PID namespace
    /// ([`current_pid_namespace_scope`](crate::background::reconcile::current_pid_namespace_scope)),
    /// when it recorded one. The session lease's `owner.json` records none (upstream's has none).
    pub pid_namespace_scope: Option<&'a str>,
}

/// Whether the process an [`OwnerStamp`] names is demonstrably gone, as seen from a reader on
/// `hostname` in the PID namespace `pid_namespace_scope`: the first two rungs of the session
/// lease's ladder ([`demonstrably_stale`](super::demonstrably_stale)), which is built on this, and
/// the verdict the permission system's `no-ui` marker is judged by. One implementation, so the two
/// records can never disagree about the same writer.
///
/// Positive evidence only; every rung that cannot see the writer's pid table answers `false`:
///
/// 1. **another host ⇒ not gone** (pi `session-lease.ts:175`): this machine's pid table says
///    nothing about another machine's.
/// 2. **another PID namespace ⇒ not gone** (pi `currentPidNamespaceScope`, SUBA-159, the rule
///    `active-async-capacity.ts:256` and `await-async-run.ts:21-22` apply): on one host, `kill(2)` and
///    `/proc` see only this namespace, so the writer's pid is either invisible (`ESRCH`) or someone
///    else's. A stamp without a namespace (the lease's, an older record) is probed as before.
/// 3. **the pid is demonstrably gone** ([`process_demonstrably_gone`]).
#[must_use]
pub fn owner_demonstrably_gone(
    owner: &OwnerStamp<'_>,
    hostname: &str,
    pid_namespace_scope: Option<&str>,
    liveness: fn(u32) -> Liveness,
    start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
) -> bool {
    if owner.hostname != hostname {
        return false;
    }
    if crate::background::reconcile::pid_namespace_scope_differs(
        owner.pid_namespace_scope,
        pid_namespace_scope,
    ) {
        return false;
    }
    process_demonstrably_gone(owner.pid, owner.start_identity, liveness, start_identity_of)
}
