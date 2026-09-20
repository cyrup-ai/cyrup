//! Reading and validating the proof — pi `process-terminal.ts:140-142`, `:161-199` @v0.68.0.
//!
//! `validateProof` is where the whole artifact earns its keep: it refuses a proof that belongs to
//! a DIFFERENT run or a DIFFERENT runner instance. Without that pair of checks, a stale
//! `process-terminal.json` left by an earlier run in a reused directory would be accepted as this
//! run's answer, and "the runner is gone" would be asserted about the wrong process. Both checks
//! are string equality against the values the reader already holds — which is exactly why
//! [`RunnerProcessInstanceId`] is a fresh v4 uuid rather than a reusable pid.

use crate::background::{RunDir, RunId};

use super::error::ProcessTerminalError;
use super::id::RunnerProcessInstanceId;
use super::types::{
    ProcessTerminal, ProcessTerminalBase, ProcessTerminalReason, ProcessTerminalVersion,
};

/// pi's `fallback?: { runId?; runnerProcessInstanceId? }` parameter (`:161`, `:180`, `:190`) — what
/// the READER already believes about the run whose proof it is opening.
///
/// Either half may be absent, and an absent half is not checked at all (upstream's `fallback?.runId
/// &&` guard, `:165`). A reader that holds neither is asking only "is this a well-formed proof?".
#[derive(Clone, Copy, Debug, Default)]
pub struct ProofExpectation<'a> {
    /// The run the reader expects the proof to belong to.
    pub run_id: Option<&'a RunId>,
    /// The runner instance the reader expects the proof to belong to.
    pub runner_process_instance_id: Option<&'a RunnerProcessInstanceId>,
}

impl<'a> ProofExpectation<'a> {
    /// Expect a specific run and runner instance — the form every production reader uses.
    #[must_use]
    pub fn new(run_id: &'a RunId, runner_process_instance_id: &'a RunnerProcessInstanceId) -> Self {
        Self {
            run_id: Some(run_id),
            runner_process_instance_id: Some(runner_process_instance_id),
        }
    }

    /// Expect nothing — a well-formedness check only (upstream's absent `fallback`).
    #[must_use]
    pub fn none() -> Self {
        Self {
            run_id: None,
            runner_process_instance_id: None,
        }
    }
}

/// pi `unknownProof` (`:140-142`) — the shape every refusal on the ladder produces.
#[must_use]
pub fn unknown_proof(
    run_id: RunId,
    runner_process_instance_id: RunnerProcessInstanceId,
    reason: ProcessTerminalReason,
    diagnostic: Option<String>,
) -> ProcessTerminal {
    ProcessTerminal::Unknown {
        base: ProcessTerminalBase::new(run_id, runner_process_instance_id),
        reason,
        // pi `...(diagnostic ? { diagnostic } : {})` — an EMPTY diagnostic is omitted, not written
        // as `""`, so a reader never has to distinguish "no explanation" from "a blank one".
        diagnostic: diagnostic.filter(|text| !text.is_empty()),
    }
}

/// pi `validateProof` (`:161-178`) — every refusal is one of upstream's own sentences.
///
/// Returns the decoded proof on success. Upstream is a TypeScript type predicate that narrows the
/// value in place; the Rust equivalent decodes it, because the arms of [`ProcessTerminal`] are
/// only reachable through the decode.
///
/// # Errors
///
/// All fifteen of the sentences this module and [`super::candidate`] own, as
/// [`ProcessTerminalError`]. None of them reaches a user as an error: every caller folds the
/// message into an `unknown` proof's `diagnostic` (`:186`, `:197`, `:297`), which is the string an
/// operator reads out of `process-terminal.json` when a run will not settle.
pub fn validate_proof(
    raw: &serde_json::Value,
    label: &str,
    fallback: ProofExpectation<'_>,
) -> Result<ProcessTerminal, ProcessTerminalError> {
    let invalid = || ProcessTerminalError::InvalidProof {
        label: label.to_string(),
    };
    // pi `:162` — version, one of the four state words, a non-empty run id and a non-empty
    // instance id, all before anything state-specific is looked at.
    let object = raw.as_object().ok_or_else(invalid)?;
    if object.get("version").and_then(serde_json::Value::as_u64)
        != Some(u64::from(ProcessTerminalVersion::VALUE))
    {
        return Err(invalid());
    }
    let state = object
        .get("state")
        .and_then(serde_json::Value::as_str)
        .filter(|state| matches!(*state, "pending" | "observed" | "unknown" | "not-started"))
        .ok_or_else(invalid)?;
    let run_id = object
        .get("runId")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;
    let instance = object
        .get("runnerProcessInstanceId")
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
        .ok_or_else(invalid)?;

    // pi `:165-166` — the two identity checks the whole artifact exists for.
    if let Some(expected) = fallback.run_id
        && expected.as_str() != run_id
    {
        return Err(ProcessTerminalError::ProofRunMismatch {
            label: label.to_string(),
            actual: run_id.to_string(),
            expected: expected.as_str().to_string(),
        });
    }
    if let Some(expected) = fallback.runner_process_instance_id
        && expected.as_str() != instance
    {
        return Err(ProcessTerminalError::ProofRunnerMismatch {
            label: label.to_string(),
            actual: instance.to_string(),
            expected: expected.as_str().to_string(),
        });
    }

    // pi `:167-169` — `instances`, when present, is an array of valid instance exits of EITHER
    // kind.
    if let Some(instances) = object.get("instances")
        && !instances.is_null()
    {
        let valid = instances.as_array().is_some_and(|entries| {
            entries
                .iter()
                .all(|entry| super::candidate::valid_process_instance(entry, None))
        });
        if !valid {
            return Err(ProcessTerminalError::InvalidInstances {
                label: label.to_string(),
            });
        }
    }

    // pi `:170-175` — the `observed` arm additionally owes an `observedAt`, an `instances` array,
    // and a runner instance inside it whose id MATCHES the proof's own.
    if state == "observed" {
        if !object
            .get("observedAt")
            .is_some_and(|value| value.as_i64().is_some())
        {
            return Err(ProcessTerminalError::ObservedMissingObservedAt {
                label: label.to_string(),
            });
        }
        let Some(instances) = object
            .get("instances")
            .and_then(serde_json::Value::as_array)
        else {
            return Err(ProcessTerminalError::ObservedMissingInstances {
                label: label.to_string(),
            });
        };
        let runner = instances
            .iter()
            .find(|entry| entry.get("kind").and_then(serde_json::Value::as_str) == Some("runner"));
        let matched = runner.is_some_and(|entry| {
            super::candidate::valid_process_instance(
                entry,
                Some(super::candidate::InstanceKind::Runner),
            ) && entry
                .get("processInstanceId")
                .and_then(serde_json::Value::as_str)
                == Some(instance)
        });
        if !matched {
            return Err(ProcessTerminalError::ObservedMissingRunnerInstance {
                label: label.to_string(),
            });
        }
    }

    // pi `:176` — `resumeDisposition`, when present, is one of the three words.
    if let Some(disposition) = object.get("resumeDisposition")
        && !disposition.is_null()
        && !matches!(
            disposition.as_str(),
            Some("resumable" | "non-resumable" | "unavailable")
        )
    {
        return Err(ProcessTerminalError::InvalidResumeDisposition {
            label: label.to_string(),
        });
    }

    serde_json::from_value(raw.clone()).map_err(|_| invalid())
}

/// The run id an unvalidatable proof is attributed to when the reader could not say — pi's
/// `fallback.runId ?? label` with the default `label = "status"` (`:180`, `:186`).
const STATUS_LABEL: &str = "status";

/// The instance id an unvalidatable proof is attributed to — pi's
/// `fallback.runnerProcessInstanceId ?? "unknown"` (`:186`, `:197`), verbatim.
const UNKNOWN_INSTANCE: &str = "unknown";

/// pi `sanitizeProcessTerminal` (`:180-188`) — the OVERLAY reader.
///
/// Never fails and never panics: a value that does not validate becomes an `unknown` proof with
/// reason [`ProcessTerminalReason::ProofWriteFailed`] carrying the refusal sentence as its
/// diagnostic. That degradation is the whole contract — `debug.run` and the async-status
/// projection read `status.processTerminal` on every call, and one corrupt overlay key must not
/// make either of them fail outright.
#[must_use]
pub fn sanitize_process_terminal(
    value: Option<&serde_json::Value>,
    fallback: ProofExpectation<'_>,
    label: &str,
) -> Option<ProcessTerminal> {
    let value = value?;
    if value.is_null() {
        return None;
    }
    Some(match validate_proof(value, label, fallback) {
        Ok(proof) => proof,
        Err(error) => degraded_proof(fallback, label, &error.to_string()),
    })
}

/// pi's `unknownProof(fallback.runId ?? label, fallback.runnerProcessInstanceId ?? "unknown",
/// "proof-write-failed", errorMessage(error))` (`:186`, `:197`) — the one shape both degraded
/// readers produce, spelled once.
fn degraded_proof(
    fallback: ProofExpectation<'_>,
    label: &str,
    diagnostic: &str,
) -> ProcessTerminal {
    unknown_proof(
        fallback
            .run_id
            .cloned()
            .unwrap_or_else(|| RunId::from_token(label)),
        fallback
            .runner_process_instance_id
            .cloned()
            .unwrap_or_else(|| RunnerProcessInstanceId::from_token(UNKNOWN_INSTANCE)),
        ProcessTerminalReason::ProofWriteFailed,
        Some(diagnostic.to_string()),
    )
}

/// pi `readProcessTerminal` (`:190-199`) — the SIDECAR reader.
///
/// `None` means the file is not there at all (upstream's `ENOENT` arm, `:196`), which is a
/// meaningful answer: a run that never reached [`super::initialize_process_terminal`] has no
/// proof, and the capacity release rung must fall through to its pid ladder rather than treat
/// absence as a refusal. Every OTHER failure — unreadable, unparsable, or belonging to another
/// run — degrades to an `unknown` proof, exactly as [`sanitize_process_terminal`] does.
pub async fn read_process_terminal(
    run_dir: &RunDir,
    fallback: ProofExpectation<'_>,
) -> Option<ProcessTerminal> {
    let path = run_dir.process_terminal();
    // pi's label is the async dir itself here (`:193`), and its ENOENT fallback run id is that
    // directory's basename (`:197`).
    let label = run_dir.as_path().display().to_string();
    let raw = match tokio::fs::read(&path).await {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => return Some(degraded_proof(fallback, &label, &error.to_string())),
    };
    let value = match serde_json::from_slice::<serde_json::Value>(&raw) {
        Ok(value) => value,
        Err(error) => return Some(degraded_proof(fallback, &label, &error.to_string())),
    };
    Some(match validate_proof(&value, &label, fallback) {
        Ok(proof) => proof,
        Err(error) => degraded_proof(fallback, &label, &error.to_string()),
    })
}

/// The tolerant field decoder for [`StepStatus::process_terminal`](crate::background::StepStatus)
/// — [`deserialize_overlay`]'s sibling, and DELIBERATELY not the same function.
///
/// # Why a per-step proof must not be run through `validateProof`
///
/// `stepProcessTerminalProof` (`process-terminal.ts:201-222`) writes `instances: records`, where
/// `records` is that STEP's writer exits — and nothing else. A per-step observed proof therefore
/// carries no runner instance at all, which is exactly what `validateProof`'s `:174` guard
/// refuses (*"has no matching runner instance"*). Upstream never hits that contradiction because
/// it never validates a step proof: `run-status.ts:56` sanitizes `status.processTerminal` alone,
/// and its `AsyncStatus` is untyped JSON everywhere else. Routing the step field through
/// [`validate_proof`] would degrade EVERY observed step to `proof-write-failed` — which is the
/// shape of bug this comment exists to stop someone reintroducing by "unifying" the two decoders.
///
/// What is still guaranteed: a value this build cannot decode at all becomes an `unknown` /
/// [`ProcessTerminalReason::ProofWriteFailed`] proof rather than failing the whole `status.json`
/// read.
///
/// # Errors
///
/// Only the ones the outer `Option<Value>` decode itself raises.
pub(crate) fn deserialize_step_overlay<'de, D>(
    deserializer: D,
) -> Result<Option<ProcessTerminal>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <Option<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?;
    let Some(raw) = raw.filter(|value| !value.is_null()) else {
        return Ok(None);
    };
    Ok(Some(match serde_json::from_value::<ProcessTerminal>(raw) {
        Ok(proof) => proof,
        Err(error) => degraded_proof(ProofExpectation::none(), STATUS_LABEL, &error.to_string()),
    }))
}

/// The tolerant field decoder [`RunStatus::process_terminal`](crate::background::RunStatus) uses
/// (`records.rs:425`) — and it alone; [`StepStatus::process_terminal`] has its own,
/// [`deserialize_step_overlay`], for the reason stated there. This is pi's
/// `sanitizeProcessTerminal(status.processTerminal, …)` (`run-status.ts:56`) applied at the moment
/// the status is read, which is the only moment a Rust reader gets.
///
/// # Why this cannot be a plain `Option<ProcessTerminal>`
///
/// A `#[derive(Deserialize)]` on the typed field would make ONE corrupt `processTerminal` key fail
/// the whole `status.json` decode — every status read, every listing, every control verb, for a
/// value that upstream degrades and carries on with. This decoder reads the key as an untyped
/// value first and routes it through [`sanitize_process_terminal`], so a bad overlay becomes an
/// `unknown` / `proof-write-failed` proof and the status still parses.
///
/// # Errors
///
/// Only the ones the outer `Option<Value>` decode itself raises; a decodable-but-invalid proof is
/// never an error.
///
/// [`StepStatus::process_terminal`]: crate::background::StepStatus
pub(crate) fn deserialize_overlay<'de, D>(
    deserializer: D,
) -> Result<Option<ProcessTerminal>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <Option<serde_json::Value> as serde::Deserialize>::deserialize(deserializer)?;
    Ok(sanitize_process_terminal(
        raw.as_ref(),
        ProofExpectation::none(),
        STATUS_LABEL,
    ))
}
