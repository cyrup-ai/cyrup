//! The session-lease record and its STRICT validator — pi `runs/shared/session-lease.ts:10-45`,
//! `:121-144` @v0.68.0.

use std::path::PathBuf;

use super::identity::ProcessStartIdentity;

/// pi `SessionLeaseOwner.token` (`:19`) — the per-acquisition token that names one contender.
///
/// It is the value the candidate's `revivalLeaseToken` carries (`process-terminal.ts:105`), the
/// value `markProcessTerminalCandidateLeaseRelease` matches on (`:136`), and the value the
/// per-owner stale tombstone is named after (`session-lease.ts:278-288`) — three places where a
/// bare `String` would be transposable with a run id or a session id.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct LeaseToken(String);

impl LeaseToken {
    /// Wraps an already-known token, without minting entropy.
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

impl std::fmt::Display for LeaseToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// pi `canonicalSessionId` (`:100-104`) — the lowercase sha256 hex of a session file's REALPATH.
///
/// The lease directory's own name, and the value the observed proof's `canonicalSession` block
/// publishes (`process-terminal.ts:154`). A newtype because it is a 64-character hex string that
/// sits beside a run id and an instance id in the same records.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct CanonicalSessionId(String);

impl CanonicalSessionId {
    /// Wraps an already-computed digest. Computing one is
    /// [`canonical_session_id`](super::canonical_session_id)'s job — this exists for a value read
    /// back off disk.
    #[must_use]
    pub fn from_token(token: impl Into<String>) -> Self {
        Self(token.into())
    }

    /// Borrows the digest.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CanonicalSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The lease owner's WRITER — pi's `writerState` + `writerPid` + `writerProcessStartIdentity`
/// triple (`:27-29`), expressed as one sum type.
///
/// # The two cross-field rules are expressed BY TYPE, not by a validator
///
/// Upstream enforces them with two explicit refusals in `parseOwner` (`:141-142`): `running`
/// without a `writerPid` is rejected, and a non-`running` state carrying either writer field is
/// rejected. Here [`LeaseWriter::Running`] CARRIES its pid and the other two arms cannot hold one,
/// so neither invalid shape is constructible in Rust at all. [`parse_owner`] still performs both
/// refusals, because the shapes ARE constructible in JSON and a hand-edited or truncated
/// `owner.json` must read as `unreadable` rather than as a lease that can never be reclaimed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeaseWriter {
    /// pi `"none"` (`:27`) — no writer process has been dispatched for this lease.
    None,
    /// pi `"spawning"` (`:27`) — a writer has been dispatched but has not yet reported a pid.
    /// **This arm is never stale** (`:177`): it is the unobservable window, and treating it as
    /// gone would steal the lease out from under a child that is mid-fork.
    Spawning,
    /// pi `"running"` (`:27`) — a writer process is live and its pid is recorded.
    Running {
        /// pi `writerPid` (`:28`).
        pid: u32,
        /// pi `writerProcessStartIdentity` (`:29`).
        start_identity: Option<ProcessStartIdentity>,
    },
}

impl LeaseWriter {
    /// The lowercase word this writes as `writerState` — pi's own three literals.
    #[must_use]
    pub fn state_word(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Spawning => "spawning",
            Self::Running { .. } => "running",
        }
    }
}

/// pi `SessionLeaseOwner` (`:17-33`) — the `owner.json` a lease directory holds.
///
/// Every field is upstream's, under upstream's camelCase key, so the on-disk record is unchanged.
/// There is deliberately NO `#[derive(Deserialize)]`: the record is read back only through
/// [`parse_owner`], because three of upstream's rules (`pid` must be a positive integer, and the
/// two writer cross-field rules) are REFUSALS that a derive would silently accept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLeaseOwner {
    /// pi `token` (`:19`).
    pub token: LeaseToken,
    /// pi `canonicalSessionFile` (`:20`) — the realpath this lease is held over.
    pub canonical_session_file: PathBuf,
    /// pi `runId` (`:21`) — the REVIVED run holding the lease.
    pub run_id: String,
    /// pi `sourceRunId` (`:22`) — the run being revived FROM.
    pub source_run_id: String,
    /// pi `parentSessionId` (`:23`) — the orchestrator session, when there is one.
    pub parent_session_id: Option<String>,
    /// pi `pid` (`:24`) — the owning process. A positive integer by [`parse_owner`]'s own refusal.
    pub pid: u32,
    /// pi `hostname` (`:25`) — the machine the owning process runs on. Rung 1 of
    /// `demonstrablyStale` (`:175`): a lease held on ANOTHER host is never stale here, because
    /// this host cannot observe that host's pids.
    pub hostname: String,
    /// pi `processStartIdentity` (`:26`).
    pub process_start_identity: Option<ProcessStartIdentity>,
    /// pi `writerState`/`writerPid`/`writerProcessStartIdentity` (`:27-29`), as one sum.
    pub writer: LeaseWriter,
    /// pi `acquiredAt` (`:30`) — the ISO-8601 rendering of [`Self::acquired_at_ms`].
    pub acquired_at: String,
    /// pi `acquiredAtMs` (`:31`).
    pub acquired_at_ms: i64,
    /// pi `updatedAtMs` (`:32`).
    pub updated_at_ms: i64,
}

/// pi `SessionLeaseState` (`:42-45`) — what `inspectSessionLease` answers.
///
/// The three arms are not decorative: `finalizeProcessTerminal` maps `owned` to
/// [`ProcessTerminalReason::CanonicalSessionLeaseActive`](crate::background::process_terminal::ProcessTerminalReason::CanonicalSessionLeaseActive)
/// and `unreadable` to
/// [`ProcessTerminalReason::CanonicalSessionUnavailable`](crate::background::process_terminal::ProcessTerminalReason::CanonicalSessionUnavailable)
/// (`process-terminal.ts:274`), so collapsing them would erase an operator-visible distinction
/// between "a live successor holds this session file" and "something is on disk that this build
/// cannot read".
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionLeaseState {
    /// No lease directory exists (pi `:114`).
    Free {
        /// The realpath the lease would be keyed on.
        canonical_session_file: PathBuf,
        /// Its digest — the lease directory's name.
        canonical_session_id: CanonicalSessionId,
    },
    /// A lease directory exists and its `owner.json` parsed (pi `:117`).
    Owned {
        /// The realpath the lease is keyed on.
        canonical_session_file: PathBuf,
        /// Its digest — the lease directory's name.
        canonical_session_id: CanonicalSessionId,
        /// The parsed owner. Boxed: the record is much wider than the other two arms, and an
        /// un-boxed variant would make every `SessionLeaseState` that size.
        owner: Box<SessionLeaseOwner>,
    },
    /// A lease directory exists but its `owner.json` is missing, unparsable, or refused by
    /// [`parse_owner`] (pi `:118`).
    Unreadable {
        /// The realpath the lease is keyed on.
        canonical_session_file: PathBuf,
        /// Its digest — the lease directory's name.
        canonical_session_id: CanonicalSessionId,
    },
}

impl SessionLeaseState {
    /// `true` only for [`SessionLeaseState::Free`] — pi's `lease.state !== "free"` test, spelled
    /// once so the two readers (`sessionProjection` `:151` and the ladder's lease rung `:273`)
    /// cannot disagree.
    #[must_use]
    pub fn is_free(&self) -> bool {
        matches!(self, Self::Free { .. })
    }
}

/// Reads a JSON number that must be a POSITIVE integer pid — pi `:129-131` (`typeof === "number"
/// && Number.isInteger && > 0`) and `:139` for the writer's own pid.
fn positive_pid(value: &serde_json::Value) -> Option<u32> {
    value
        .as_u64()
        .filter(|pid| *pid > 0)
        .and_then(|pid| u32::try_from(pid).ok())
}

/// pi `parseOwner` (`:121-144`) — the STRICT validator every read of an `owner.json` goes through.
///
/// Returns `None` for any refusal, exactly as upstream does: a malformed owner makes the lease
/// `unreadable`, never an error. `unreadable` is itself a refusal to reclaim ("Refusing to reclaim
/// it without proof that the owner is stale", `:156`), so the failure direction is safe.
///
/// # The refusals, and why each is a refusal rather than a coercion
///
/// * `version !== 1` (`:124`) — a record written by a build this one does not understand.
/// * `pid` not a positive integer (`:129-131`) — a zero or negative pid would be handed to
///   `kill(pid, 0)`, where `0` means "the whole process group" and a negative pid means "a process
///   group". Either would make the liveness probe answer about the wrong thing entirely.
/// * `writerState === "running"` with no `writerPid` (`:141`) — without the pid, rung 4 of
///   `demonstrablyStale` (`:179-180`) can never be satisfied and the lease becomes permanently
///   unreclaimable. One malformed byte would deadlock every future revival of that session file.
/// * a non-`running` `writerState` carrying either writer field (`:142`) — the record contradicts
///   itself, and the contradiction is exactly the shape a partial write produces.
#[must_use]
pub fn parse_owner(value: &serde_json::Value) -> Option<SessionLeaseOwner> {
    let object = value.as_object()?;
    if object.get("version")?.as_u64()? != 1 {
        return None;
    }
    let token = LeaseToken::from_token(object.get("token")?.as_str()?);
    let canonical_session_file = PathBuf::from(object.get("canonicalSessionFile")?.as_str()?);
    let run_id = object.get("runId")?.as_str()?.to_string();
    let source_run_id = object.get("sourceRunId")?.as_str()?.to_string();
    let pid = positive_pid(object.get("pid")?)?;
    let hostname = object.get("hostname")?.as_str()?.to_string();
    let writer_state = object.get("writerState")?.as_str()?;
    if !matches!(writer_state, "none" | "spawning" | "running") {
        return None;
    }
    let acquired_at = object.get("acquiredAt")?.as_str()?.to_string();
    let acquired_at_ms = object.get("acquiredAtMs")?.as_i64()?;
    let updated_at_ms = object.get("updatedAtMs")?.as_i64()?;

    // `:137-140` — every optional is typed when present. `Value::Null` is NOT `undefined`: an
    // explicit null fails the `as_str`/`as_u64` probe and refuses the record, which is upstream's
    // own behaviour (`typeof null !== "string"`).
    let parent_session_id = match object.get("parentSessionId") {
        None => None,
        Some(raw) => Some(raw.as_str()?.to_string()),
    };
    let process_start_identity = match object.get("processStartIdentity") {
        None => None,
        Some(raw) => Some(ProcessStartIdentity::from_token(raw.as_str()?)),
    };
    let writer_pid = match object.get("writerPid") {
        None => None,
        Some(raw) => Some(positive_pid(raw)?),
    };
    let writer_start_identity = match object.get("writerProcessStartIdentity") {
        None => None,
        Some(raw) => Some(ProcessStartIdentity::from_token(raw.as_str()?)),
    };

    // `:141-142` — the two cross-field rules.
    let writer = match writer_state {
        "running" => LeaseWriter::Running {
            pid: writer_pid?,
            start_identity: writer_start_identity,
        },
        _ => {
            if writer_pid.is_some() || writer_start_identity.is_some() {
                return None;
            }
            if writer_state == "spawning" {
                LeaseWriter::Spawning
            } else {
                LeaseWriter::None
            }
        }
    };

    Some(SessionLeaseOwner {
        token,
        canonical_session_file,
        run_id,
        source_run_id,
        parent_session_id,
        pid,
        hostname,
        process_start_identity,
        writer,
        acquired_at,
        acquired_at_ms,
        updated_at_ms,
    })
}

/// pi `SessionLeaseRequest` (`:10-15`) — what a caller asks a lease FOR.
///
/// The four facts that identify a revival: which session file it will write, the new run's id, the
/// run it revives FROM, and the orchestrator session behind it. Everything else on the owner
/// record ([`SessionLeaseOwner`]) is observed by the acquiring process rather than requested.
///
/// [`RunId`] on both id fields, where upstream has bare strings: cyrup already has the newtype and
/// both values ARE run ids, so the two cannot be transposed at a call site. They are written to
/// disk as plain strings, unchanged.
///
/// `Serialize`/`Deserialize` because this value travels from the ORCHESTRATOR to the RUNNER
/// through `runner-config.json`: the orchestrator decides that a launch is a revival, the runner
/// is the process that actually holds the lease for the length of its run.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLeaseRequest {
    /// pi `sessionFile` (`:11`) — resolved to its REALPATH by the acquire, never before.
    pub session_file: PathBuf,
    /// pi `runId` (`:12`) — the REVIVED run that will hold the lease.
    pub run_id: crate::background::RunId,
    /// pi `sourceRunId` (`:13`) — the run being revived from.
    pub source_run_id: crate::background::RunId,
    /// pi `parentSessionId` (`:14`) — the orchestrator session, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<String>,
}

/// What a lease holder may say about its writer — pi `updateWriter`'s parameter type (`:38`):
/// `{ state: "none" | "spawning" } | { state: "running"; pid: number }`.
///
/// # Why this is not [`LeaseWriter`]
///
/// [`LeaseWriter::Running`] carries a `start_identity` as well as a pid, and that field is NOT a
/// caller's to supply: `updateWriter` reads it itself, from the pid, at the moment of the update
/// (`:246`). A caller handed a [`LeaseWriter`] could record an identity belonging to no process,
/// which would make the writer arm of the staleness ladder (`:179-180`) answer about a fiction.
/// The two types are therefore deliberately different: this one is the REQUEST, [`LeaseWriter`] is
/// the RECORD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriterUpdate {
    /// No writer process is dispatched.
    None,
    /// A writer has been dispatched and has not yet reported a pid. **The lease is never stale in
    /// this state** (`:177`).
    Spawning,
    /// A writer process is live under this pid.
    Running {
        /// The writer's pid.
        pid: u32,
    },
}

impl WriterUpdate {
    /// Resolve into the recorded form, reading the start identity from the pid exactly where
    /// upstream reads it (`:246`: `writer.state === "running" ? getIdentity(writer.pid) :
    /// undefined`).
    #[must_use]
    pub fn resolve(
        self,
        start_identity_of: fn(u32) -> Option<ProcessStartIdentity>,
    ) -> LeaseWriter {
        match self {
            Self::None => LeaseWriter::None,
            Self::Spawning => LeaseWriter::Spawning,
            Self::Running { pid } => LeaseWriter::Running {
                pid,
                start_identity: start_identity_of(pid),
            },
        }
    }
}

impl serde::Serialize for SessionLeaseOwner {
    /// pi's object literal at `:214-228` and `:247-259` — the SAME on-disk shape [`parse_owner`]
    /// reads back.
    ///
    /// Hand-written for the same reason [`parse_owner`] is: the record's writer arm is three
    /// fields that upstream spreads conditionally (`...(writer.state === "running" ? { writerPid }
    /// : {})`), and every optional is OMITTED rather than written as `null` — a `null` would fail
    /// `parse_owner`'s own `typeof === "string"` probes and make this process's own lease read as
    /// `unreadable` the moment it tried to reclaim it.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap as _;
        let mut map = serializer.serialize_map(None)?;
        // pi `version: 1` (`:215`) — a literal, never a field a caller chooses.
        map.serialize_entry("version", &1_u32)?;
        map.serialize_entry("token", &self.token)?;
        map.serialize_entry("canonicalSessionFile", &self.canonical_session_file)?;
        map.serialize_entry("runId", &self.run_id)?;
        map.serialize_entry("sourceRunId", &self.source_run_id)?;
        if let Some(parent) = &self.parent_session_id {
            map.serialize_entry("parentSessionId", parent)?;
        }
        map.serialize_entry("pid", &self.pid)?;
        map.serialize_entry("hostname", &self.hostname)?;
        if let Some(identity) = &self.process_start_identity {
            map.serialize_entry("processStartIdentity", identity)?;
        }
        map.serialize_entry("writerState", self.writer.state_word())?;
        if let LeaseWriter::Running {
            pid,
            start_identity,
        } = &self.writer
        {
            map.serialize_entry("writerPid", pid)?;
            if let Some(identity) = start_identity {
                map.serialize_entry("writerProcessStartIdentity", identity)?;
            }
        }
        map.serialize_entry("acquiredAt", &self.acquired_at)?;
        map.serialize_entry("acquiredAtMs", &self.acquired_at_ms)?;
        map.serialize_entry("updatedAtMs", &self.updated_at_ms)?;
        map.end()
    }
}
