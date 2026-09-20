//! The candidate — pi `process-terminal.ts:33-61`, `:63-69`, `:75-139` @v0.68.0.
//!
//! The candidate is the runner's DECLARATION: what it will have to prove, written before the
//! proof can be judged. Everything here is a hand-written validator over `serde_json::Value`
//! rather than a `#[derive(Deserialize)]`, for the same reason
//! [`parse_owner`](crate::background::session_lease::parse_owner) is: upstream's checks are
//! REFUSALS with their own sentences (`:79`, `:83`, `:88`, `:91`, `:95`, `:96`, `:97`), and a
//! derive would either accept a shape upstream rejects or fail with a message no operator can
//! match against upstream's.

use std::collections::BTreeMap;

use crate::background::atomic::{write_atomic_json, write_private_atomic_json};
use crate::background::session_lease::LeaseToken;
use crate::background::{RunDir, RunId};

use super::error::ProcessTerminalError;
use super::id::RunnerProcessInstanceId;
use super::types::{
    ProcessInstanceExit, ProcessTerminal, ProcessTerminalBase, ProcessTerminalCandidate,
    ProcessTerminalVersion,
};

/// pi `validProcessInstance` (`:37-57`) — the instance-exit validator, ported EXACTLY, including
/// the inconsistency it contains.
///
/// Takes a RAW [`serde_json::Value`] rather than a decoded [`ProcessInstanceExit`], as upstream
/// does, because two of its rules are about the ABSENCE of a key rather than about a value:
/// `:44` refuses a `runner` record that carries an `attempt` field at all, and `:45` requires a
/// `pi-writer` record to carry one. A decode-then-check port would accept the first (serde
/// ignores unknown fields by default) and could not distinguish the second from a default.
///
/// # `[CYRUP-DELTA]` — the validator accepts two of [`ProcessTreeTerminal`]'s three arms
///
/// `types.ts:660-665` declares an observed `windows-taskkill` process tree, and
/// [`ProcessTreeTerminal::ObservedTaskkill`] ports it because it is the ON-DISK FORMAT and a
/// record written by a Windows build must round-trip. But `process-terminal.ts:46-53` accepts an
/// observed tree only when `mechanism === "posix-process-group"`; a `windows-taskkill` tree falls
/// through to `:54`, fails the `state === "unknown"` test, and is REFUSED. That asymmetry is
/// upstream's, not this port's, and it is reproduced here deliberately so the two builds agree on
/// exactly the same accept set. A reader who "fixes" it introduces a divergence in which cyrup
/// accepts a candidate pi rejects.
///
/// `kind` narrows the accept set to one arm — pi's optional second parameter (`:37`), used as
/// `Some(Runner)` for the runner instance an `observed` proof must carry (`:174`) and as
/// `Some(PiWriter)` for every entry of the candidate's `writers` map (`:59-61`).
#[must_use]
pub fn valid_process_instance(value: &serde_json::Value, kind: Option<InstanceKind>) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    // pi `:39` — a non-empty string id.
    if !object
        .get("processInstanceId")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| !id.is_empty())
    {
        return false;
    }
    // pi `:40` — either the demanded kind, or (with none demanded) one of the two words.
    let record_kind = object.get("kind").and_then(serde_json::Value::as_str);
    match kind {
        Some(InstanceKind::Runner) if record_kind != Some("runner") => return false,
        Some(InstanceKind::PiWriter) if record_kind != Some("pi-writer") => return false,
        None if !matches!(record_kind, Some("runner" | "pi-writer")) => return false,
        _ => {}
    }
    // pi `:41-43` — a finite `closeObservedAt`, and `exitCode`/`signal` each of their one type or
    // explicitly `null`. An ABSENT key is not `null` in upstream's test (`typeof undefined` is
    // neither `"number"` nor `null`), so absence is a refusal for all three.
    if !object
        .get("closeObservedAt")
        .is_some_and(|value| value.as_i64().is_some())
    {
        return false;
    }
    if !object
        .get("exitCode")
        .is_some_and(|value| value.is_null() || value.as_i64().is_some())
    {
        return false;
    }
    if !object
        .get("signal")
        .is_some_and(|value| value.is_null() || value.is_string())
    {
        return false;
    }
    // pi `:44` — a runner record carries NO `attempt`. This is the rule a decode-then-check port
    // silently drops.
    if record_kind == Some("runner") {
        return !object.contains_key("attempt");
    }
    // pi `:45` — a pi-writer carries a non-negative integer `attempt` and an OBJECT `processTree`.
    if !object
        .get("attempt")
        .and_then(serde_json::Value::as_u64)
        .is_some()
    {
        return false;
    }
    let Some(tree) = object
        .get("processTree")
        .and_then(serde_json::Value::as_object)
    else {
        return false;
    };
    match tree.get("state").and_then(serde_json::Value::as_str) {
        // pi `:46-52` — an observed tree is accepted ONLY as a posix process group, with a
        // POSITIVE group id and a finite `verifiedAt`.
        Some("observed") => {
            tree.get("mechanism").and_then(serde_json::Value::as_str) == Some("posix-process-group")
                && tree
                    .get("processGroupId")
                    .and_then(serde_json::Value::as_u64)
                    .is_some_and(|id| id > 0)
                && tree
                    .get("verifiedAt")
                    .is_some_and(|value| value.as_i64().is_some())
        }
        // pi `:54-56` — everything else must be the `unknown` arm with one of the three reasons
        // and an optional string diagnostic. An observed `windows-taskkill` tree reaches this line
        // and fails it; see the delta above.
        Some("unknown") => {
            matches!(
                tree.get("reason").and_then(serde_json::Value::as_str),
                Some("unsupported-platform" | "signal-failed" | "verification-failed")
            ) && tree
                .get("diagnostic")
                .is_none_or(serde_json::Value::is_string)
        }
        _ => false,
    }
}

/// Which arm of [`ProcessInstanceExit`] a validation is narrowed to — pi's
/// `kind?: "runner" | "pi-writer"` parameter (`:37`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceKind {
    /// The runner's own close.
    Runner,
    /// One writer child's close.
    PiWriter,
}

/// pi `readProcessTerminalCandidate` (`:75-112`).
///
/// # Errors
///
/// Every refusal upstream throws, as a [`ProcessTerminalError`] carrying upstream's sentence. A
/// MISSING file is `Ok(None)`, never an error (upstream's `ENOENT` arm, `:109`).
pub async fn read_process_terminal_candidate(
    run_dir: &RunDir,
) -> Result<Option<ProcessTerminalCandidate>, ProcessTerminalError> {
    let path = run_dir.process_terminal_candidate();
    let raw = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ProcessTerminalError::Read {
                path: path.display().to_string(),
                source,
            });
        }
    };
    let dir = run_dir.as_path().display().to_string();
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|source| ProcessTerminalError::Parse {
            path: path.display().to_string(),
            source,
        })?;
    parse_candidate(&value, &dir).map(Some)
}

/// The pure half of [`read_process_terminal_candidate`] — upstream `:78-107` over an already
/// decoded value, so the refusals are provable without a filesystem.
///
/// # Errors
///
/// [`read_process_terminal_candidate`]'s.
pub(super) fn parse_candidate(
    value: &serde_json::Value,
    dir: &str,
) -> Result<ProcessTerminalCandidate, ProcessTerminalError> {
    // pi `:78-80`: version, runId, runnerProcessInstanceId and a `writers` OBJECT are all required
    // before anything else is looked at.
    let invalid = || ProcessTerminalError::InvalidCandidate {
        dir: dir.to_string(),
    };
    let object = value.as_object().ok_or_else(invalid)?;
    if object.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(ProcessTerminalVersion::VALUE))
    {
        return Err(invalid());
    }
    let run_id = object
        .get("runId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    let instance = object
        .get("runnerProcessInstanceId")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(invalid)?;
    let raw_writers = object
        .get("writers")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(invalid)?;

    // pi `:82-85`: every entry must be an ARRAY of valid pi-writer exits, or the whole read fails
    // naming the offending step index.
    let mut writers: BTreeMap<String, Vec<ProcessInstanceExit>> = BTreeMap::new();
    for (index, entries) in raw_writers {
        let bad = || ProcessTerminalError::InvalidWriterRecords {
            index: index.clone(),
        };
        let array = entries.as_array().ok_or_else(bad)?;
        let mut parsed = Vec::with_capacity(array.len());
        for entry in array {
            if !valid_process_instance(entry, Some(InstanceKind::PiWriter)) {
                return Err(bad());
            }
            parsed.push(serde_json::from_value(entry.clone()).map_err(|_| bad())?);
        }
        writers.insert(index.clone(), parsed);
    }

    // pi `:86-94`: `expectedWriters` is optional, but when present every count is a NON-NEGATIVE
    // INTEGER — a float or a negative is a refusal, not a coercion.
    let expected_writers = match object.get("expectedWriters") {
        None | Some(serde_json::Value::Null) => None,
        Some(raw) => {
            let map = raw
                .as_object()
                .ok_or(ProcessTerminalError::InvalidExpectedWriters)?;
            let mut expected = BTreeMap::new();
            for (index, count) in map {
                let value = count.as_u64().filter(|value| *value <= u64::from(u32::MAX));
                let Some(value) = value else {
                    return Err(ProcessTerminalError::InvalidExpectedWriterCount {
                        index: index.clone(),
                    });
                };
                #[allow(clippy::cast_possible_truncation)]
                expected.insert(index.clone(), value as u32);
            }
            Some(expected)
        }
    };

    // pi `:95-97`: three optional scalars, each with its own refusal sentence. pi additionally
    // drops an EMPTY `sessionFile`/`revivalLeaseToken` on the way out (`:104-105`'s `&& raw.x`
    // truthiness test), which is why both are filtered rather than merely typed.
    let session_file = match object.get("sessionFile") {
        None | Some(serde_json::Value::Null) => None,
        Some(raw) => Some(
            raw.as_str()
                .ok_or(ProcessTerminalError::InvalidCandidateSessionFile)?,
        )
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from),
    };
    let revival_lease_token = match object.get("revivalLeaseToken") {
        None | Some(serde_json::Value::Null) => None,
        Some(raw) => Some(
            raw.as_str()
                .ok_or(ProcessTerminalError::InvalidCandidateLeaseToken)?,
        )
        .filter(|value| !value.is_empty())
        .map(LeaseToken::from_token),
    };
    let revival_lease_release_acknowledged = match object.get("revivalLeaseReleaseAcknowledged") {
        None | Some(serde_json::Value::Null) => None,
        Some(raw) => Some(
            raw.as_bool()
                .ok_or(ProcessTerminalError::InvalidLeaseReleaseAcknowledgement)?,
        ),
    };

    Ok(ProcessTerminalCandidate {
        version: ProcessTerminalVersion,
        run_id: RunId::from_token(run_id),
        runner_process_instance_id: RunnerProcessInstanceId::from_token(instance),
        writers,
        expected_writers,
        session_file,
        revival_lease_token,
        revival_lease_release_acknowledged,
    })
}

/// pi `writeProcessTerminalCandidate` (`:114-116`) — a PRIVATE (0600) atomic write.
///
/// The privacy is not incidental: the candidate can carry
/// [`ProcessTerminalCandidate::session_file`], a path into the operator's own session transcripts,
/// and the run directory is under a shared temp root. Upstream uses `writePrivateAtomicJson` here
/// and `writeAtomicJson` for the proof itself for exactly that distinction.
///
/// # Errors
///
/// The atomic write's — a caller that cannot persist the candidate has no proof to offer, which
/// [`super::finalize_process_terminal`] then reports as
/// [`super::ProcessTerminalReason::RunnerCandidateMissing`].
pub async fn write_process_terminal_candidate(
    run_dir: &RunDir,
    candidate: &ProcessTerminalCandidate,
) -> std::io::Result<()> {
    write_private_atomic_json(&run_dir.process_terminal_candidate(), candidate).await
}

/// pi `initializeProcessTerminal` (`:118-132`) — *"Establish ownership before authorizing a runner
/// to start any child session."*
///
/// Writes BOTH artifacts: an empty candidate and a `state: "pending"` proof.
///
/// # The empty candidate is the crash discriminator — do not seed it
///
/// `writers` is `{}` and `expectedWriters` is ABSENT. A runner that dies before it reaches its own
/// candidate write therefore leaves both maps empty, and
/// [`finalize_process_terminal`](super::finalize_process_terminal)'s `:277` rung answers
/// [`ProcessTerminalReason::WriterCloseUnverified`](super::ProcessTerminalReason::WriterCloseUnverified).
/// A runner that finished overwrites this with one entry per step and the ladder can reach
/// `observed`. Seeding this candidate with per-step entries would erase that asymmetry and make a
/// crashed runner indistinguishable from a clean one — which is the exact ambiguity VL-S4 exists
/// to remove.
///
/// # `[CYRUP-DELTA]` — cyrup calls this BEFORE the spawn, upstream after it
///
/// pi calls it at `async-execution.ts:851`, AFTER `spawn`, and can afford to: its runner blocks on
/// `waitForStartupControl(startupProceedPath, launchBarrierToken, "proceed")`
/// (`subagent-runner.ts:5233-5240`) and cannot touch a child session until the parent writes the
/// proceed token at `:868`. cyrup has no such barrier —
/// `grep -rn 'launch_barrier\|startup-proceed\|runner-startup' crates/cyrup-ext-subagents/src`
/// matches no CODE (only this comment and the launch site's, each quoting the grep), and
/// [`spawn_detached_runner_with_command`](crate::background::spawn_detached) returns with the
/// runner already free to run. In cyrup the SPAWN *is* the authorization, so the only placement
/// that satisfies this function's own upstream contract is before it. Nothing here needs the
/// runner's pid, so there is no obstacle.
///
/// # Errors
///
/// Either atomic write's.
pub async fn initialize_process_terminal(
    run_dir: &RunDir,
    run_id: &RunId,
    runner_process_instance_id: &RunnerProcessInstanceId,
) -> std::io::Result<()> {
    write_process_terminal_candidate(
        run_dir,
        &ProcessTerminalCandidate {
            version: ProcessTerminalVersion,
            run_id: run_id.clone(),
            runner_process_instance_id: runner_process_instance_id.clone(),
            writers: BTreeMap::new(),
            expected_writers: None,
            session_file: None,
            revival_lease_token: None,
            revival_lease_release_acknowledged: None,
        },
    )
    .await?;
    write_atomic_json(
        &run_dir.process_terminal(),
        &ProcessTerminal::Pending {
            base: ProcessTerminalBase::new(run_id.clone(), runner_process_instance_id.clone()),
        },
    )
    .await
}

/// pi `markProcessTerminalCandidateLeaseRelease` (`:134-138`) — stamp the revival lease's release
/// acknowledgement onto the candidate the runner already wrote.
///
/// A no-op when there is no candidate, or when the candidate records a DIFFERENT token: the
/// acknowledgement is about one specific lease, and writing it against a successor's token would
/// let [`finalize_process_terminal`](super::finalize_process_terminal) clear the
/// `canonical-session-release-unverified` rung on a lease that was never released.
///
/// Best-effort in upstream and here: a failure leaves the acknowledgement unset, which the ladder
/// reports as `canonical-session-release-unverified` — the safe direction.
pub async fn mark_process_terminal_candidate_lease_release(
    run_dir: &RunDir,
    token: &LeaseToken,
    acknowledged: bool,
) {
    let Ok(Some(mut candidate)) = read_process_terminal_candidate(run_dir).await else {
        return;
    };
    if candidate.revival_lease_token.as_ref() != Some(token) {
        return;
    }
    candidate.revival_lease_release_acknowledged = Some(acknowledged);
    let _ = write_process_terminal_candidate(run_dir, &candidate).await;
}
