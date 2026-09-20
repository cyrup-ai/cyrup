//! The process-terminal artifact's on-disk shapes — pi `src/shared/types.ts:629-713` and
//! `src/runs/background/process-terminal.ts:15-31` @v0.68.0.
//!
//! Every discriminated union upstream expresses with a TypeScript literal-tagged type is a Rust
//! enum here, and every key keeps its camelCase spelling, so the JSON on disk is unchanged.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::background::RunId;
use crate::background::session_lease::{CanonicalSessionId, LeaseToken};

use super::id::RunnerProcessInstanceId;

/// The literal `version: 1` every process-terminal record carries (pi `:15`, `:692`).
///
/// A unit type rather than a `u32`, on
/// [`CapacityOwnerVersion`](crate::background::active_async_capacity::CapacityOwnerVersion)'s
/// pattern: a record written by a build that bumped the version fails to parse rather than being
/// silently read under the wrong schema.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcessTerminalVersion;

impl ProcessTerminalVersion {
    /// The only value this type represents.
    pub const VALUE: u32 = 1;
}

impl serde::Serialize for ProcessTerminalVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for ProcessTerminalVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported process-terminal record version {raw} (this build reads version {})",
                Self::VALUE
            )))
        }
    }
}

/// pi `ProcessTerminalState` (`types.ts:632`) — the four words a proof's `state` can be.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessTerminalState {
    /// Written at launch, before the runner has been authorized to touch any child session.
    Pending,
    /// The runner's close was observed and every writer it declared is accounted for.
    Observed,
    /// The close could not be proven; [`ProcessTerminalReason`] says which rung refused.
    Unknown,
    /// The run ended before a child was ever started.
    NotStarted,
}

impl ProcessTerminalState {
    /// The kebab-case word this state writes as — the one the `debug.run` dump renders and the
    /// one the capacity verdict interpolates into `process-terminal proof is <state>`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Observed => "observed",
            Self::Unknown => "unknown",
            Self::NotStarted => "not-started",
        }
    }
}

/// pi `ProcessTerminalReason` (`types.ts:633-643`) — all TEN variants, in upstream's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessTerminalReason {
    /// No observer was available to watch the runner close.
    ObserverUnavailable,
    /// `process-terminal-candidate.json` is absent — the runner never reached its own candidate
    /// write (`process-terminal.ts:258`).
    RunnerCandidateMissing,
    /// The candidate belongs to another run or another runner instance (`:259`).
    RunnerInstanceMismatch,
    /// The declared writers and the recorded writers disagree, or both are empty (`:277`).
    WriterCloseUnverified,
    /// A pi-writer's process TREE was not observed torn down (`:279`).
    ProcessTreeUnverified,
    /// The canonical session file's lease directory exists but cannot be read (`:274`).
    CanonicalSessionUnavailable,
    /// The canonical session file is still leased by a live owner (`:274`).
    CanonicalSessionLeaseActive,
    /// A revival lease was held and its release was never acknowledged (`:276`).
    CanonicalSessionReleaseUnverified,
    /// The proof could not be read, validated or persisted (`:186`, `:197`, `:309`).
    ProofWriteFailed,
    /// A repair pass replaced a stale proof.
    StaleRepair,
}

impl ProcessTerminalReason {
    /// The kebab-case word this reason writes as.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ObserverUnavailable => "observer-unavailable",
            Self::RunnerCandidateMissing => "runner-candidate-missing",
            Self::RunnerInstanceMismatch => "runner-instance-mismatch",
            Self::WriterCloseUnverified => "writer-close-unverified",
            Self::ProcessTreeUnverified => "process-tree-unverified",
            Self::CanonicalSessionUnavailable => "canonical-session-unavailable",
            Self::CanonicalSessionLeaseActive => "canonical-session-lease-active",
            Self::CanonicalSessionReleaseUnverified => "canonical-session-release-unverified",
            Self::ProofWriteFailed => "proof-write-failed",
            Self::StaleRepair => "stale-repair",
        }
    }
}

/// pi `ProcessTerminalBase.resumeDisposition` (`types.ts:698`) — whether the run this proof
/// describes can be revived from its session file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ResumeDisposition {
    /// A terminal-but-revivable run whose session file is still on disk (`process-terminal.ts:147`).
    Resumable,
    /// A STOPPED run: deliberately ended, never revived (`:145`).
    NonResumable,
    /// Any other state, or a missing session file (`:146-147`).
    Unavailable,
}

/// pi `ProcessTreeTerminal`'s unknown-arm reason (`types.ts:668`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessTreeUnknownReason {
    /// The platform has no process-group teardown to verify.
    UnsupportedPlatform,
    /// The teardown signal itself failed.
    SignalFailed,
    /// The signal landed but the teardown could not be verified.
    VerificationFailed,
}

/// pi `ProcessTreeTerminal` (`types.ts:653-670`) — ALL THREE arms.
///
/// # `[CYRUP-DELTA]` — the type has three arms but upstream's own validator accepts two
///
/// `validProcessInstance` (`process-terminal.ts:46-53`) accepts an observed process tree ONLY when
/// `mechanism === "posix-process-group"`; an observed `windows-taskkill` tree — a shape upstream's
/// own type declares at `types.ts:660-665` — falls through to `:54`, fails the
/// `state === "unknown"` test, and is refused. That is an inconsistency in upstream, not in this
/// port. **The TYPE is ported with all three arms** (it is the on-disk format, and a record
/// written by a Windows build must round-trip) and **the VALIDATOR is ported exactly as upstream
/// wrote it** ([`valid_process_instance`](super::valid_process_instance)), so a reader who spots
/// the asymmetry does not "fix" it into a divergence from upstream's accept set.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "RawProcessTree", into = "RawProcessTree")]
pub enum ProcessTreeTerminal {
    /// `{state:"observed", mechanism:"posix-process-group", processGroupId, verifiedAt}`.
    ObservedProcessGroup {
        /// The process group that was verified gone.
        process_group_id: u32,
        /// Epoch milliseconds of the verification.
        verified_at: i64,
    },
    /// `{state:"observed", mechanism:"windows-taskkill", pid, verifiedAt}`.
    ObservedTaskkill {
        /// The root pid `taskkill /T` was run against.
        pid: u32,
        /// Epoch milliseconds of the verification.
        verified_at: i64,
    },
    /// `{state:"unknown", reason, diagnostic?}`.
    Unknown {
        /// Which of the three unknown reasons applies.
        reason: ProcessTreeUnknownReason,
        /// A free-text diagnostic, when there is one.
        diagnostic: Option<String>,
    },
}

impl ProcessTreeTerminal {
    /// `true` for either observed arm — pi's `writer.processTree.state !== "observed"` test
    /// (`process-terminal.ts:279`), spelled once.
    #[must_use]
    pub fn is_observed(&self) -> bool {
        !matches!(self, Self::Unknown { .. })
    }
}

/// The flat JSON shape [`ProcessTreeTerminal`] reads and writes — upstream's three object
/// literals, with the keys they actually carry.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProcessTree {
    state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    mechanism: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    process_group_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verified_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<ProcessTreeUnknownReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<String>,
}

impl From<ProcessTreeTerminal> for RawProcessTree {
    fn from(value: ProcessTreeTerminal) -> Self {
        match value {
            ProcessTreeTerminal::ObservedProcessGroup {
                process_group_id,
                verified_at,
            } => Self {
                state: "observed".to_string(),
                mechanism: Some("posix-process-group".to_string()),
                process_group_id: Some(process_group_id),
                pid: None,
                verified_at: Some(verified_at),
                reason: None,
                diagnostic: None,
            },
            ProcessTreeTerminal::ObservedTaskkill { pid, verified_at } => Self {
                state: "observed".to_string(),
                mechanism: Some("windows-taskkill".to_string()),
                process_group_id: None,
                pid: Some(pid),
                verified_at: Some(verified_at),
                reason: None,
                diagnostic: None,
            },
            ProcessTreeTerminal::Unknown { reason, diagnostic } => Self {
                state: "unknown".to_string(),
                mechanism: None,
                process_group_id: None,
                pid: None,
                verified_at: None,
                reason: Some(reason),
                diagnostic,
            },
        }
    }
}

impl TryFrom<RawProcessTree> for ProcessTreeTerminal {
    type Error = String;

    fn try_from(raw: RawProcessTree) -> Result<Self, Self::Error> {
        match (raw.state.as_str(), raw.mechanism.as_deref()) {
            ("observed", Some("posix-process-group")) => Ok(Self::ObservedProcessGroup {
                process_group_id: raw
                    .process_group_id
                    .ok_or("observed posix-process-group tree is missing processGroupId")?,
                verified_at: raw
                    .verified_at
                    .ok_or("observed process tree is missing verifiedAt")?,
            }),
            ("observed", Some("windows-taskkill")) => Ok(Self::ObservedTaskkill {
                pid: raw
                    .pid
                    .ok_or("observed windows-taskkill tree is missing pid")?,
                verified_at: raw
                    .verified_at
                    .ok_or("observed process tree is missing verifiedAt")?,
            }),
            ("unknown", _) => Ok(Self::Unknown {
                reason: raw.reason.ok_or("unknown process tree is missing reason")?,
                diagnostic: raw.diagnostic,
            }),
            (state, mechanism) => Err(format!(
                "unsupported process tree state '{state}' with mechanism '{}'",
                mechanism.unwrap_or("<none>")
            )),
        }
    }
}

/// pi `ProcessInstanceExit` (`types.ts:682`) — the union of `RunnerProcessInstanceExit`
/// (`:645-651`) and `PiWriterProcessInstanceExit` (`:672-680`).
///
/// `process-terminal.ts` imports the UNION, and it is the union the candidate's `writers` map
/// holds: every value in it is a pi-writer record, every value in an observed proof's `instances`
/// is one runner record followed by the flattened writers.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum ProcessInstanceExit {
    /// pi `RunnerProcessInstanceExit` (`:645-651`) — the RUNNER's own close. Carries no
    /// `attempt`, and upstream's validator refuses one that does (`process-terminal.ts:44`).
    Runner {
        /// The runner instance this exit belongs to.
        process_instance_id: RunnerProcessInstanceId,
        /// Epoch milliseconds at which the close was observed.
        close_observed_at: i64,
        /// The process's exit code, when it exited rather than being signalled.
        exit_code: Option<i32>,
        /// The signal name that ended it, when one did.
        signal: Option<String>,
    },
    /// pi `PiWriterProcessInstanceExit` (`:672-680`) — one attempt of one step's writer process.
    PiWriter {
        /// The writer instance this exit belongs to.
        process_instance_id: String,
        /// Which attempt of the step this was (`:675`).
        attempt: u32,
        /// Epoch milliseconds at which the close was observed.
        close_observed_at: i64,
        /// The process's exit code, when it exited rather than being signalled.
        exit_code: Option<i32>,
        /// The signal name that ended it, when one did.
        signal: Option<String>,
        /// Whether the writer's whole process TREE was verified torn down (`:679`).
        process_tree: ProcessTreeTerminal,
    },
}

/// pi `CanonicalSessionTerminal` (`types.ts:684-689`) — the observed proof's statement about the
/// session file the run held.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CanonicalSessionTerminal {
    /// The sha256 of the session file's realpath (`:685`).
    pub canonical_session_id: CanonicalSessionId,
    /// `released` when this run held a revival lease and gave it back, `not-held` when it never
    /// held one (`:686`, written at `process-terminal.ts:155`).
    pub lease_disposition: LeaseDisposition,
    /// Always `true` (`:687`): the block is only built when the lease reads FREE
    /// (`process-terminal.ts:151`), so its presence IS the statement.
    pub free_at_observation: bool,
    /// Present, and `true`, only when a revival lease was held and its release was acknowledged
    /// (`:688`, `process-terminal.ts:157`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_session_lease_released: Option<bool>,
}

/// pi `CanonicalSessionTerminal.leaseDisposition` (`types.ts:686`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LeaseDisposition {
    /// A revival lease was held for this run and has been released.
    Released,
    /// No revival lease was ever held.
    NotHeld,
}

/// pi `ProcessTerminalBase` (`types.ts:691-699`) — the fields every arm of
/// [`ProcessTerminal`] carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessTerminalBase {
    /// Always `1`.
    pub version: ProcessTerminalVersion,
    /// The run this proof belongs to (`:693`) — one half of what a reader matches on.
    pub run_id: RunId,
    /// Present only on a PER-STEP overlay proof (`:694`, written by `stepProcessTerminalProof`).
    pub child_index: Option<usize>,
    /// The runner instance this proof belongs to (`:695`) — the other half of the match.
    pub runner_process_instance_id: RunnerProcessInstanceId,
    /// Whether the run this proof describes can be revived (`:698`).
    pub resume_disposition: Option<ResumeDisposition>,
}

impl ProcessTerminalBase {
    /// The minimal base a freshly minted proof carries: no child index, no resume disposition.
    #[must_use]
    pub fn new(run_id: RunId, runner_process_instance_id: RunnerProcessInstanceId) -> Self {
        Self {
            version: ProcessTerminalVersion,
            run_id,
            child_index: None,
            runner_process_instance_id,
            resume_disposition: None,
        }
    }
}

/// pi `ProcessTerminal` (`types.ts:701-713`) — the FOUR-arm discriminated union that is the whole
/// artifact.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "RawProcessTerminal", into = "RawProcessTerminal")]
pub enum ProcessTerminal {
    /// `state: "pending"` — written at launch and left in place by a runner that never reached
    /// its close. A proof that stays `pending` forever IS the crash signal.
    Pending {
        /// The shared base.
        base: ProcessTerminalBase,
    },
    /// `state: "not-started"` — the run ended before any child started.
    NotStarted {
        /// The shared base.
        base: ProcessTerminalBase,
    },
    /// `state: "observed"` — the positive proof, and the only state that releases a capacity slot.
    Observed {
        /// The shared base.
        base: ProcessTerminalBase,
        /// Epoch milliseconds of the close observation (`:705`).
        observed_at: i64,
        /// The runner instance, followed by every writer instance (`:706`).
        instances: Vec<ProcessInstanceExit>,
        /// The session-file statement, when one could be made (`:707`).
        canonical_session: Option<CanonicalSessionTerminal>,
    },
    /// `state: "unknown"` — a rung of the ladder refused, and [`Self::reason`] says which.
    Unknown {
        /// The shared base.
        base: ProcessTerminalBase,
        /// Which rung refused (`:711`).
        reason: ProcessTerminalReason,
        /// The error sentence that produced the refusal, when there was one (`:712`).
        diagnostic: Option<String>,
    },
}

impl ProcessTerminal {
    /// The arm's own `state` word.
    #[must_use]
    pub fn state(&self) -> ProcessTerminalState {
        match self {
            Self::Pending { .. } => ProcessTerminalState::Pending,
            Self::NotStarted { .. } => ProcessTerminalState::NotStarted,
            Self::Observed { .. } => ProcessTerminalState::Observed,
            Self::Unknown { .. } => ProcessTerminalState::Unknown,
        }
    }

    /// The shared base, whichever arm this is.
    #[must_use]
    pub fn base(&self) -> &ProcessTerminalBase {
        match self {
            Self::Pending { base }
            | Self::NotStarted { base }
            | Self::Observed { base, .. }
            | Self::Unknown { base, .. } => base,
        }
    }

    /// The run this proof claims to belong to.
    #[must_use]
    pub fn run_id(&self) -> &RunId {
        &self.base().run_id
    }

    /// The runner instance this proof claims to belong to.
    #[must_use]
    pub fn runner_process_instance_id(&self) -> &RunnerProcessInstanceId {
        &self.base().runner_process_instance_id
    }

    /// The refusal reason, for the `unknown` arm only.
    #[must_use]
    pub fn reason(&self) -> Option<ProcessTerminalReason> {
        match self {
            Self::Unknown { reason, .. } => Some(*reason),
            _ => None,
        }
    }
}

/// The flat JSON object [`ProcessTerminal`] reads and writes — upstream's
/// `ProcessTerminalBase & { … }` intersection, spelled once.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawProcessTerminal {
    version: ProcessTerminalVersion,
    state: ProcessTerminalState,
    run_id: RunId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    child_index: Option<usize>,
    runner_process_instance_id: RunnerProcessInstanceId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    observed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    instances: Option<Vec<ProcessInstanceExit>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<ProcessTerminalReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    diagnostic: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resume_disposition: Option<ResumeDisposition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    canonical_session: Option<CanonicalSessionTerminal>,
}

impl From<ProcessTerminal> for RawProcessTerminal {
    fn from(value: ProcessTerminal) -> Self {
        let state = value.state();
        let (observed_at, instances, reason, diagnostic, canonical_session) = match &value {
            ProcessTerminal::Pending { .. } | ProcessTerminal::NotStarted { .. } => {
                (None, None, None, None, None)
            }
            ProcessTerminal::Observed {
                observed_at,
                instances,
                canonical_session,
                ..
            } => (
                Some(*observed_at),
                Some(instances.clone()),
                None,
                None,
                canonical_session.clone(),
            ),
            ProcessTerminal::Unknown {
                reason, diagnostic, ..
            } => (None, None, Some(*reason), diagnostic.clone(), None),
        };
        let base = value.base();
        Self {
            version: base.version,
            state,
            run_id: base.run_id.clone(),
            child_index: base.child_index,
            runner_process_instance_id: base.runner_process_instance_id.clone(),
            observed_at,
            instances,
            reason,
            diagnostic,
            resume_disposition: base.resume_disposition,
            canonical_session,
        }
    }
}

impl TryFrom<RawProcessTerminal> for ProcessTerminal {
    type Error = String;

    fn try_from(raw: RawProcessTerminal) -> Result<Self, Self::Error> {
        let base = ProcessTerminalBase {
            version: raw.version,
            run_id: raw.run_id,
            child_index: raw.child_index,
            runner_process_instance_id: raw.runner_process_instance_id,
            resume_disposition: raw.resume_disposition,
        };
        Ok(match raw.state {
            ProcessTerminalState::Pending => Self::Pending { base },
            ProcessTerminalState::NotStarted => Self::NotStarted { base },
            ProcessTerminalState::Observed => Self::Observed {
                base,
                observed_at: raw
                    .observed_at
                    .ok_or("observed process-terminal proof is missing observedAt")?,
                instances: raw
                    .instances
                    .ok_or("observed process-terminal proof is missing instances")?,
                canonical_session: raw.canonical_session,
            },
            ProcessTerminalState::Unknown => Self::Unknown {
                base,
                reason: raw
                    .reason
                    .ok_or("unknown process-terminal proof is missing reason")?,
                diagnostic: raw.diagnostic,
            },
        })
    }
}

/// pi `ProcessTerminalCandidate` (`process-terminal.ts:15-24`) — the record the RUNNER writes
/// stating what it expects to have to prove, and the input
/// [`finalize_process_terminal`](super::finalize_process_terminal) reads back.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessTerminalCandidate {
    /// Always `1`.
    pub version: ProcessTerminalVersion,
    /// The run this candidate belongs to (`:17`).
    pub run_id: RunId,
    /// The runner instance that wrote it (`:18`).
    pub runner_process_instance_id: RunnerProcessInstanceId,
    /// Writer exits, keyed by STEP INDEX as a decimal string (`:19`) — upstream's
    /// `Record<string, ProcessInstanceExit[]>`. A [`BTreeMap`] rather than a
    /// [`HashMap`](std::collections::HashMap) so the JSON key order is deterministic across
    /// writes of the same content.
    pub writers: BTreeMap<String, Vec<ProcessInstanceExit>>,
    /// How many writer exits each step index is EXPECTED to produce (`:20`). Absent on a
    /// candidate written by [`initialize_process_terminal`](super::initialize_process_terminal),
    /// which is what makes a runner that died at startup distinguishable from one that finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_writers: Option<BTreeMap<String, u32>>,
    /// The canonical session file this run held, when it held one (`:21`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_file: Option<PathBuf>,
    /// The revival lease token this run acquired, when it acquired one (`:22`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revival_lease_token: Option<LeaseToken>,
    /// Whether the lease release was ACKNOWLEDGED (`:23`), stamped by
    /// [`mark_process_terminal_candidate_lease_release`](super::mark_process_terminal_candidate_lease_release).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revival_lease_release_acknowledged: Option<bool>,
}

/// pi `RunnerCloseObservation` (`process-terminal.ts:26-31`) — what the observer of the runner's
/// close hands to the ladder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunnerCloseObservation {
    /// The runner instance that closed (`:27`).
    pub process_instance_id: RunnerProcessInstanceId,
    /// Epoch milliseconds of the observation (`:28`).
    pub close_observed_at: i64,
    /// The exit code, when the process exited (`:29`).
    pub exit_code: Option<i32>,
    /// The signal name, when the process was signalled (`:30`).
    pub signal: Option<String>,
}

/// One step's writer-process ledger, as the RUNNER accumulates it while the step runs.
///
/// `[CYRUP-DELTA]` — upstream has no such accumulator because it has nothing to accumulate: pi
/// `subagent-runner.ts:5168-5190` writes `writers[i] = []` / `expectedWriters[i] = 0` for every
/// step, and its own comment at `:5169` gives the reason — *"Children run inside this process, so
/// no step has writer processes to prove terminal."* cyrup's steps spawn REAL OS children
/// ([`crate::spawn::SpawnedChild`]), so this type is the pair of numbers
/// `finalizeProcessTerminal`'s `inconsistentWriters` test (`process-terminal.ts:271-272`) compares:
///
/// * [`Self::launched`] counts every child this step actually started — it becomes
///   `expectedWriters[i]`, and it is incremented at the SPAWN, before anything can be observed
///   about the child;
/// * [`Self::exits`] holds one [`ProcessInstanceExit::PiWriter`] per child whose close WAS
///   observed — it becomes `writers[i]`.
///
/// The two are counted at different moments on purpose. A child that was launched and whose close
/// was never observed leaves `launched > exits.len()`, which is exactly the
/// [`ProcessTerminalReason::WriterCloseUnverified`] verdict — a rung that is vacuous upstream and
/// live here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WriterProcessLedger {
    /// How many writer child processes this step LAUNCHED (one per model-fallback / startup-retry
    /// attempt that reached a real `spawn`). Becomes `expectedWriters[<step index>]`.
    pub launched: u32,
    /// One record per launched child whose close was observed. Becomes
    /// `writers[<step index>]`.
    pub exits: Vec<ProcessInstanceExit>,
}

impl WriterProcessLedger {
    /// Record that this step launched one more writer child — the `expectedWriters` half, counted
    /// at the spawn so a child that never reports a close still raises the count.
    pub fn record_launch(&mut self) {
        self.launched = self.launched.saturating_add(1);
    }

    /// Record one observed writer close — the `writers` half.
    pub fn record_exit(&mut self, exit: ProcessInstanceExit) {
        self.exits.push(exit);
    }
}
