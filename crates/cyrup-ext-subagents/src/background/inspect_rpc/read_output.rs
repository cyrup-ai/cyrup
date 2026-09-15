//! The session-bearing core: how inspect finds a run's delivered text once, and then once more
//! after the payload has been deleted underneath it.
//!
//! This is pi `inspect-rpc.ts:170-290` — `resultOutput`, `readOutputArtifact`,
//! `readSessionBackedOutput` and the `readResultOutput` that sequences them. All THREE session uses
//! live here.

use std::path::{Path, PathBuf};

use crate::background::completion_replay::{
    self, ArchiveSource, CompletionArchiveEntry, ReplayReadFilter,
};
use crate::background::fleet_view::{self, SessionMessageKind};
use crate::background::result_index::{self, errno};
use crate::background::{RunId, StepStatus};
use crate::exec::child_protocol::BoundedByteTail;
use crate::identity::SessionId;

use super::MAX_FINAL_OUTPUT_LENGTH;

/// pi `FAILED_OUTPUT_ARTIFACT_PREFIX` (`inspect-rpc.ts:67`).
///
/// Written by upstream's `formatOutputArtifactContent` (`shared/artifacts.ts:204-215`). cyrup does
/// NOT port that formatter — `artifacts.rs`'s `write_run_artifacts` writes `content.output`
/// verbatim, and `grep -rn "Subagent run failed before producing output" src/` finds nothing else —
/// so for a cyrup-written artifact this prefix never appears and
/// [`read_output_artifact`]'s whole failed-output branch is dead.
///
/// It is ported regardless, and the reason is the standing this crate already gives
/// `completion_replay/archive.rs`'s `summary` rung: the INPUT is a file a FOREIGN build may have
/// written, and a later dead-code sweep must not remove the reader half of a format. Widening
/// cyrup's artifact writer to EMIT the prefix is a separate decision and is out of scope here.
const FAILED_OUTPUT_ARTIFACT_PREFIX: &str =
    "Subagent run failed before producing output.\n\nError:\n";

/// pi's `{ output?: string; errorText?: string }` return (`inspect-rpc.ts:173`), as one type.
///
/// Both absent is a legitimate, common outcome — a child that is still running, or one whose
/// artifacts have aged out — and is NOT an error; see [`read_result_output`]'s own note.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResultOutput {
    /// The child's delivered text.
    pub output: Option<String>,
    /// The child's failure text, when it failed instead.
    pub error_text: Option<String>,
}

impl ResultOutput {
    /// `true` when neither half was resolved — pi's
    /// `result.output === undefined && result.errorText === undefined` (`:398`).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.output.is_none() && self.error_text.is_none()
    }
}

/// Every failure this read path can produce.
///
/// Upstream `throw`s at each of these points and `buildInspectReply`'s outer `try/catch`
/// (`inspect-rpc.ts:424-426`) collapses them all into the one
/// `{ code: "internal", message: "Inspection could not read the async run artifacts." }` reply. So
/// this type's message is for the LOG, never for the caller — which is why it is a single opaque
/// string rather than a code the reply could branch on.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct InspectReadError(String);

impl InspectReadError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl From<std::io::Error> for InspectReadError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

impl From<crate::error::SubagentError> for InspectReadError {
    fn from(error: crate::error::SubagentError) -> Self {
        Self(error.to_string())
    }
}

/// The named arguments of [`read_result_output`] — pi's eight positional parameters (`:233`),
/// four of which are `string`/`string | undefined` and therefore pairwise transposable.
pub struct ResultOutputRequest<'a> {
    /// The per-cwd results directory.
    pub results_dir: &'a Path,
    /// The CURRENT session. Inspect never reads another session's payload: upstream refuses with
    /// `no_active_session` before it can reach here without one (`:327-329`), and the run itself
    /// has already passed the strict session gate (`:354`).
    pub session_id: &'a SessionId,
    /// The run whose payload is wanted.
    pub run_id: &'a RunId,
    /// Which child of that run, if the request named one.
    pub step_index: Option<usize>,
    /// The roots a recorded `sessionFile` may be dereferenced under.
    pub trusted_roots: &'a [PathBuf],
    /// The named child's agent, for the LEGACY archive-entry fallback (`:277`).
    pub step_agent: Option<&'a str>,
    /// Epoch millis, read ONCE by the caller and threaded — the replay filter (`:247`) and the raw
    /// re-verification (`:256`) must not be able to straddle a TTL boundary between two clock
    /// reads.
    pub now: i64,
}

/// pi `readResultOutput` (`inspect-rpc.ts:233-289`) — the run's delivered text, through the
/// payload if it is still there and through the durable replay if it is not.
///
/// # The three session uses
///
/// 1. **`:234`** — the payload is ADDRESSED through
///    [`result_index::result_payload_path_for_session_run`], never by joining the public path.
///    That function's own two rungs are the session index entry (which probes owned → legacy root
///    → staged, and PROMOTES a staged payload on the way) and the staged location; it has no
///    public-root rung, and neither does upstream's `resultPayloadPathForSessionRun`
///    (`result-files.ts:334-338`).
///
///    **[`crate::background::wait_completions`]'s public-path fallback is deliberately NOT copied
///    here.** `collect_wait_completions` falls back to `file.resolve_in(results_dir)` because a
///    `wait` must serve a run with NO session at all. Inspect has no such case — it refuses with
///    `no_active_session` long before this point — and reproducing that fallback would let one
///    session read another's payload straight out of the shared per-cwd root.
/// 2. **`:247`** — the replay record is read with `session_id: Some(..)`, which is an OPTIONAL
///    filter in [`completion_replay::read_completion_replay`] and is supplied here.
/// 3. **`:255`** — the RAW record's own `sessionId` is re-checked. See the raw pre-read below.
///
/// # The `Err` arm of the index read
///
/// `result_payload_path_for_session_run` is `async` and FALLIBLE where upstream's is sync and
/// infallible. The crate's established policy is `wait_completions/collect.rs:71-79`: an
/// access-denied fault retries ONCE against
/// [`result_index::fallback_result_payload_path_for_session_run`] — the staged location alone,
/// addressed directly so an unreadable index DIRECTORY does not block recovery. Its `None` falls
/// through to the replay rung here (NOT to a public path, for the reason above). Any other `Err`
/// is upstream's rethrow.
///
/// # Errors
///
/// [`InspectReadError`] for a read or parse fault, and for the one case upstream raises
/// deliberately: a current-version, unexpired record for this run and session that still failed
/// validation (`:257`).
pub async fn read_result_output(
    request: &ResultOutputRequest<'_>,
) -> Result<ResultOutput, InspectReadError> {
    let ResultOutputRequest {
        results_dir,
        session_id,
        run_id,
        step_index,
        trusted_roots,
        step_agent,
        now,
    } = *request;

    // --- `:234`, the index read -----------------------------------------------------------
    let result_path =
        match result_index::result_payload_path_for_session_run(results_dir, session_id, run_id)
            .await
        {
            Ok(found) => found,
            Err(error) if errno::is_access_denied(&error) => {
                result_index::fallback_result_payload_path_for_session_run(
                    results_dir,
                    session_id,
                    run_id,
                )
                .await
            }
            Err(error) => return Err(error.into()),
        };
    if let Some(result_path) = result_path {
        // pi `:235` — `JSON.parse(readFileSync(...))`, whose throw is the outer catch's.
        let bytes = tokio::fs::read(&result_path).await?;
        let data: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| InspectReadError::new(error.to_string()))?;
        return result_output(&data, step_index);
    }

    // --- `:236-245`, the RAW pre-read ------------------------------------------------------
    //
    // Read the raw record BEFORE `read_completion_replay`, because that function best-effort
    // DELETES invalid and expired records (`completion_replay/store.rs:157`, `:166-167` — the same
    // destructive behaviour upstream's has), so a read taken afterwards cannot distinguish "never
    // existed" from "failed validation".
    //
    // The probe is deliberately UNTYPED. `parse_replay`/`validate_replay_record` are `pub(super)`
    // to `completion_replay` and are not widened for this — the probe wants the UNVALIDATED bytes.
    // `serde_json::from_slice::<CompletionReplayRecord>` cannot serve either: `ReplayVersion`'s
    // `Deserialize` errors on any value but `1`, which is exactly the class the probe must be able
    // to SEE in order to answer `version === 1` at `:254`.
    let replay_path = completion_replay::completion_replay_path(results_dir, run_id);
    let raw_replay: Option<serde_json::Value> = match tokio::fs::read(&replay_path).await {
        Ok(bytes) => serde_json::from_slice(&bytes).ok(),
        // pi `:244` — ENOENT is the only swallowed errno; everything else rethrows.
        Err(error) if errno::is_absent(&error) => None,
        Err(error) => return Err(error.into()),
    };

    let Some(replay) = completion_replay::read_completion_replay(
        results_dir,
        run_id,
        ReplayReadFilter {
            session_id: Some(session_id),
            now: Some(now),
        },
    )
    .await
    else {
        // pi `:249-253`, carried verbatim because it is the whole point of the pre-read:
        // `readCompletionReplay` also returns undefined for absent, expired, foreign, and
        // unknown-version records. A current-version, unexpired record for this run and session
        // that still failed validation is an inspection FAILURE, not a child without output —
        // surface it instead of replying success with no finalOutput.
        if raw_replay.as_ref().is_some_and(|raw| {
            raw.is_object()
                && raw.get("version") == Some(&serde_json::json!(1))
                && raw.get("runId").and_then(serde_json::Value::as_str) == Some(run_id.as_str())
                && raw.get("sessionId").and_then(serde_json::Value::as_str)
                    == Some(session_id.as_str())
                && raw
                    .get("expiresAt")
                    .and_then(serde_json::Value::as_f64)
                    .is_some_and(|expires_at| expires_at > now as f64)
        }) {
            return Err(InspectReadError::new(
                "Completion replay record failed validation.",
            ));
        }
        return Ok(ResultOutput::default());
    };

    // --- `:261-289`, the archive ladder ----------------------------------------------------
    let archive = completion_replay::read_completion_archive(&replay.archive_path).await?;
    let entries: &[CompletionArchiveEntry] = archive
        .as_ref()
        .map_or(&[], |archive| archive.entries.as_slice());

    // pi `:262-268`, verbatim: run-level inspection must not attribute a child's output to the run
    // in a multi-child archive. Current child entries carry `resultIndex` AND `agent`; run-level
    // entries carry neither. Legacy archives (written before `resultIndex` existed) have child
    // entries with `agent` but no `resultIndex`, so child reads fall back to a UNIQUE agent name.
    // Duplicate legacy agent names fail closed. A single-child archive is the one exception: the
    // run's output IS that child's output, including a legacy agent-tagged entry.
    let child_entries = entries
        .iter()
        .filter(|entry| entry.result_index.is_some() || entry.agent.is_some())
        .count();
    let single_child = child_entries == 1;
    let unique_legacy_agent = |agent: Option<&str>| -> bool {
        agent.is_some_and(|agent| {
            entries
                .iter()
                .filter(|entry| {
                    entry.result_index.is_none() && entry.agent.as_deref() == Some(agent)
                })
                .count()
                == 1
        })
    };
    let matches = |entry: &CompletionArchiveEntry| -> bool {
        if let Some(step_index) = step_index {
            if let Some(result_index) = entry.result_index {
                return result_index == step_index;
            }
            return unique_legacy_agent(step_agent) && entry.agent.as_deref() == step_agent;
        }
        if single_child {
            return entry.result_index.is_some() || entry.agent.is_some();
        }
        entry.result_index.is_none() && entry.agent.is_none()
    };
    let pick = |source: ArchiveSource| -> Option<&CompletionArchiveEntry> {
        entries
            .iter()
            .find(|entry| entry.source == source && matches(entry))
    };

    // pi `:282-284`. A matched entry with no `path` falls THROUGH to the next rung rather than
    // ending the ladder — upstream's `artifactPath` is `undefined` in exactly that case.
    if let Some(path) = pick(ArchiveSource::OutputArtifact).and_then(|entry| entry.path.as_deref())
    {
        return read_output_artifact(path).await;
    }
    if let Some(path) = pick(ArchiveSource::Session).and_then(|entry| entry.path.as_deref()) {
        return Ok(read_session_backed_output(path, trusted_roots));
    }
    // pi `:288-289` — `return output ? { output } : {}`: an EMPTY retained text is falsy upstream
    // and must not become `Some("")` here.
    Ok(ResultOutput {
        output: pick(ArchiveSource::ResultTail)
            .and_then(|entry| entry.text.clone())
            .filter(|text| !text.is_empty()),
        error_text: None,
    })
}

/// pi `resultOutput` (`inspect-rpc.ts:173-197`) — project one run's terminal payload onto the
/// `{ output, errorText }` pair, for the whole run or for one child of it.
///
/// # `finalOutput`, not upstream's `output`
///
/// pi reads `data.results[i].output` and `data.summary`. **Neither key exists in a cyrup-written
/// payload**: [`crate::exec::SingleResult`] declares `final_output` → `finalOutput` under
/// `rename_all = "camelCase"`, and [`crate::background::ResultFile`] declares no `summary` at all.
/// `completion_replay/archive.rs:213-231` already met this exact problem and its ruling is binding
/// here — read the key that actually carries a child's delivered text FIRST and keep upstream's
/// spelling as a fallback, which is purely additive. The `summary` rung is likewise ported and
/// likewise dead for a cyrup payload, for the same reason `archive.rs` keeps its own: the input is
/// an untyped value a FOREIGN build may have written.
///
/// # Errors
///
/// Upstream's five shape assertions (`:174-183`, `:192-193`), each with its own sentence.
fn result_output(
    data: &serde_json::Value,
    step_index: Option<usize>,
) -> Result<ResultOutput, InspectReadError> {
    let Some(data) = data.as_object() else {
        return Err(InspectReadError::new(
            "Async result payload must be an object.",
        ));
    };
    let results = match data.get("results") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::Array(results)) => Some(results),
        Some(_) => {
            return Err(InspectReadError::new(
                "Async result payload results must be an array.",
            ));
        }
    };
    let summary = match data.get("summary") {
        None | Some(serde_json::Value::Null) => None,
        Some(serde_json::Value::String(summary)) => Some(summary),
        Some(_) => {
            return Err(InspectReadError::new(
                "Async result payload summary must be a string.",
            ));
        }
    };

    // The child's delivered text under either spelling, with upstream's own type assertion on each.
    let child_output = |child: &serde_json::Map<String, serde_json::Value>,
                        key: &str,
                        message: &str|
     -> Result<Option<String>, InspectReadError> {
        match child.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(InspectReadError::new(message.to_string())),
        }
    };

    if let (Some(step_index), Some(results)) = (step_index, results) {
        // pi `:179-180` — an out-of-range child is an EMPTY result, not an error. `get`, never an
        // index: the workspace denies `clippy::indexing_slicing`, so an indexing port would not
        // even compile clean, and the panic it would introduce is exactly what this arm exists to
        // avoid.
        let Some(step_result) = results.get(step_index) else {
            return Ok(ResultOutput::default());
        };
        let Some(step_result) = step_result.as_object() else {
            return Err(InspectReadError::new(
                "Async result payload step result must be an object.",
            ));
        };
        let output = match child_output(
            step_result,
            "finalOutput",
            "Async result payload step output must be a string.",
        )? {
            Some(text) => Some(text),
            None => child_output(
                step_result,
                "output",
                "Async result payload step output must be a string.",
            )?,
        };
        return Ok(ResultOutput {
            output,
            error_text: child_output(
                step_result,
                "error",
                "Async result payload step error must be a string.",
            )?,
        });
    }

    if let Some(summary) = summary {
        return Ok(ResultOutput {
            output: Some(summary.clone()),
            error_text: None,
        });
    }
    // pi `:190-195` — a single-child run's output IS the run's output.
    if let Some([result]) = results.map(Vec::as_slice) {
        let Some(result) = result.as_object() else {
            return Err(InspectReadError::new(
                "Async result payload result must be an object.",
            ));
        };
        let output = match child_output(
            result,
            "finalOutput",
            "Async result payload output must be a string.",
        )? {
            Some(text) => Some(text),
            None => child_output(
                result,
                "output",
                "Async result payload output must be a string.",
            )?,
        };
        if output.is_some() {
            return Ok(ResultOutput {
                output,
                error_text: None,
            });
        }
    }
    Ok(ResultOutput::default())
}

/// pi `readOutputArtifact` (`inspect-rpc.ts:199-219`) — the tail of a saved output artifact.
///
/// Reads a WINDOW, never the whole file: a long-running child's artifact can be far larger than
/// the [`MAX_FINAL_OUTPUT_LENGTH`] that will survive bounding anyway.
///
/// The failed-output branch (the prefix probe at offset 0, and the `\nMetadata: ` / `\n\nTranscript: `
/// trailer trims) is dead for a cyrup-written artifact — see
/// [`FAILED_OUTPUT_ARTIFACT_PREFIX`] — and is ported regardless.
///
/// # Errors
///
/// Any I/O fault; upstream's `openSync`/`readSync`/`fstatSync` all throw.
async fn read_output_artifact(output_path: &Path) -> Result<ResultOutput, InspectReadError> {
    let mut file = tokio::fs::File::open(output_path).await?;
    let prefix_len = FAILED_OUTPUT_ARTIFACT_PREFIX.len() as u64;
    let failed_output =
        read_window(&mut file, 0, prefix_len).await? == FAILED_OUTPUT_ARTIFACT_PREFIX.as_bytes();

    let size = file.metadata().await?.len();
    let limit = (MAX_FINAL_OUTPUT_LENGTH as u64).saturating_mul(4);
    // pi `:206-209` reads exactly `min(size, limit)` bytes ending at EOF and hands them to
    // `decodeUtf8Tail`, which skips leading UTF-8 continuation bytes.
    // [`BoundedByteTail`] IS this crate's port of that boundary walk
    // (`child_protocol.rs:151-167`) and there must not be a second one — but it only trims when
    // its input EXCEEDS its capacity. So the window is read 3 bytes longer than the capacity (3 is
    // the most continuation bytes a UTF-8 scalar can carry after its lead byte), which makes the
    // ring's own drop-then-advance fire and land on exactly upstream's byte.
    let window_len = size.min(limit.saturating_add(3));
    let window = read_window(&mut file, size.saturating_sub(window_len), window_len).await?;
    let mut tail = BoundedByteTail::new(usize::try_from(limit).unwrap_or(usize::MAX));
    tail.push(&window);
    let mut text = tail.text();

    if !failed_output {
        return Ok(ResultOutput {
            output: Some(text),
            error_text: None,
        });
    }
    // pi `:211-214` — strip the two trailers `formatOutputArtifactContent` appends, but ONLY when
    // each is genuinely the last line (an output whose own text contains `\nMetadata: ` mid-body
    // must not be cut there).
    if let Some(metadata) = text.rfind("\nMetadata: ")
        && text
            .get(metadata.saturating_add(1)..)
            .is_some_and(|rest| !rest.contains('\n'))
        && let Some(head) = text.get(..metadata)
    {
        text = head.to_string();
    }
    if let Some(transcript) = text.rfind("\n\nTranscript: ")
        && text
            .get(transcript.saturating_add(2)..)
            .is_some_and(|rest| !rest.contains('\n'))
        && let Some(head) = text.get(..transcript)
    {
        text = head.to_string();
    }
    Ok(ResultOutput {
        output: None,
        // pi `:215` — the prefix survives the window only when the whole file fit in it.
        error_text: Some(
            text.strip_prefix(FAILED_OUTPUT_ARTIFACT_PREFIX)
                .map_or_else(|| text.clone(), str::to_string),
        ),
    })
}

/// `length` bytes at `start`, or fewer at EOF — pi's `fs.readSync(file, buffer, 0, length, start)`.
async fn read_window(
    file: &mut tokio::fs::File,
    start: u64,
    length: u64,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncSeekExt};

    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut buffer = Vec::new();
    (&mut *file).take(length).read_to_end(&mut buffer).await?;
    Ok(buffer)
}

/// pi `readSessionBackedOutput` (`inspect-rpc.ts:221-231`) — the archive retains the child's
/// session file as its output record, so the final answer is the terminal assistant message's
/// text: ALL text parts of that ONE session record, not just the last part.
///
/// Every byte goes through [`fleet_view::read_session_messages_tail`], which is the crate's single
/// session-file containment gate. A recorded `sessionFile` is data a CHILD wrote, so with no
/// trusted root there is nothing to dereference it against and the read is refused outright
/// (pi `:222`, and the gate's own first refusal).
fn read_session_backed_output(session_path: &Path, trusted_roots: &[PathBuf]) -> ResultOutput {
    if trusted_roots.is_empty() {
        return ResultOutput::default();
    }
    let tail = fleet_view::read_session_messages_tail(
        session_path,
        super::MAX_MESSAGE_LINES,
        trusted_roots,
    );
    for warning in &tail.warnings {
        tracing::debug!(path = %session_path.display(), %warning, "inspect session-backed read");
    }
    let Some(last) =
        tail.messages.iter().rev().find(|message| {
            message.role == "assistant" && message.kind == SessionMessageKind::Text
        })
    else {
        return ResultOutput::default();
    };
    let record_index = last.record_index;
    ResultOutput {
        output: Some(
            tail.messages
                .iter()
                .filter(|message| {
                    message.record_index == record_index && message.kind == SessionMessageKind::Text
                })
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        ),
        error_text: None,
    }
}

/// The agent of the child a request named, for [`ResultOutputRequest::step_agent`].
///
/// A free function rather than an inline `map` at the call site so the `[CYRUP-DELTA]` has one
/// home: upstream reads `step?.agent` where `step` is `node.status.steps?.[node.stepIndex]`
/// (`:370`, `:396`), and [`StepStatus::agent`] is a non-optional `String`, so cyrup's is `Some`
/// whenever the index resolves.
#[must_use]
pub fn step_agent_of(steps: &[StepStatus], step_index: Option<usize>) -> Option<&str> {
    step_index
        .and_then(|index| steps.get(index))
        .map(|step| step.agent.as_str())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::*;
    use crate::background::completion_replay::completion_replay_path;
    use crate::background::result_index::{ResultWrite, write_async_result_file};
    use crate::background::wait_completions::{ReplayPersistence, WaitCompletionStore};
    use crate::background::watch::tests::child_result;
    use crate::background::{ResultFile, RunMode, RunState};
    use crate::identity::ResultFileName;
    use serde_json::json;

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("non-empty")
    }

    /// A payload built from the REAL types, not a hand-written `json!` literal.
    ///
    /// This is load-bearing and not fussiness: a `json!({"results":[{"output":…}]})` fixture would
    /// pass against a reader that only knows upstream's `output` spelling, while every real cyrup
    /// payload — which serializes [`SingleResult::final_output`] as `finalOutput` — would read back
    /// as an empty string. Serializing the real struct is what makes the test able to fail.
    fn payload(run: &RunId, owner: &SessionId, outputs: &[&str]) -> serde_json::Value {
        let file = ResultFile {
            id: run.clone(),
            run_id: run.clone(),
            agent: "agent0".to_string(),
            mode: RunMode::Single,
            state: RunState::Complete,
            success: true,
            cwd: PathBuf::from("/tmp"),
            session_file: None,
            session_id: Some(owner.clone()),
            completion_owner_id: None,
            results: outputs
                .iter()
                .enumerate()
                .map(|(index, output)| child_result(&format!("agent{index}"), Some(output), 0))
                .collect(),
            workflow_children: None,
            workflow_receipt: None,
        };
        let value = serde_json::to_value(&file).expect("serializes");
        assert!(
            value
                .pointer("/results/0/finalOutput")
                .and_then(serde_json::Value::as_str)
                .is_some(),
            "the real payload spells it `finalOutput`, never `output` — that is why this fixture \
             is serialized from the struct: {value}"
        );
        value
    }

    fn request<'a>(
        results_dir: &'a Path,
        session_id: &'a SessionId,
        run_id: &'a RunId,
        step_index: Option<usize>,
    ) -> ResultOutputRequest<'a> {
        ResultOutputRequest {
            results_dir,
            session_id,
            run_id,
            step_index,
            trusted_roots: &[],
            step_agent: None,
            now: 1_000,
        }
    }

    /// SCOPE_12's first definition-of-done clause, pinned at `:234`: the payload is resolved
    /// THROUGH the session index, never by joining the public path.
    ///
    /// The negative half is what makes it real — `write_async_result_file` stages into
    /// `result-pending/<enc(session)>/` and promotes into `result-owned/<enc(session)>/`, so
    /// nothing ever exists at the legacy public path and a public-path join could not have produced
    /// the answer.
    #[tokio::test]
    async fn inspect_resolves_output_through_the_session_index() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run0001");
        let owner = session("s1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &owner,
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(&run, &owner, &["the answer"]),
        )
        .await
        .expect("write");

        assert!(
            !ResultFileName::for_run(&run)
                .resolve_in(tmp.path())
                .exists(),
            "nothing at the legacy public path — the index is the only address"
        );

        let read = read_result_output(&request(tmp.path(), &owner, &run, Some(0)))
            .await
            .expect("reads");
        assert_eq!(read.output.as_deref(), Some("the answer"));
        assert_eq!(read.error_text, None);

        // And the no-child form, which takes the single-result rung (`:190-195`).
        let read = read_result_output(&request(tmp.path(), &owner, &run, None))
            .await
            .expect("reads");
        assert_eq!(read.output.as_deref(), Some("the answer"));
    }

    /// The partition holds at the read boundary: another session's payload is not merely hidden
    /// from the index, it is unreachable — there is no public path for it to leak through either.
    #[tokio::test]
    async fn inspect_of_a_foreign_sessions_run_returns_nothing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run0001");
        let owner = session("s1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &owner,
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(&run, &owner, &["the answer"]),
        )
        .await
        .expect("write");

        let read = read_result_output(&request(tmp.path(), &session("s2"), &run, Some(0)))
            .await
            .expect("no fault — simply nothing addressable");
        assert!(read.is_empty(), "s2 must not read s1's payload: {read:?}");
    }

    /// The SCOPE_4 dependency, at `:247`: after delete-last the payload is gone and the durable
    /// replay record answers instead.
    ///
    /// The store is DROPPED before the read. That is the load-bearing part — with it alive this
    /// passes on the in-process tier alone and proves nothing about the durable one.
    #[tokio::test]
    async fn inspect_falls_back_to_the_replay_record_after_cleanup() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run0001");
        let owner = session("s1");
        {
            let watcher_store = WaitCompletionStore::default();
            watcher_store
                .record(
                    &run,
                    &json!({ "agent": "coder", "results": [{ "finalOutput": "replayed text" }] }),
                    1_000,
                    600_000,
                    Some(&ReplayPersistence {
                        results_dir: tmp.path(),
                        session_id: &owner,
                    }),
                )
                .await;
        }
        assert!(completion_replay_path(tmp.path(), &run).exists());
        assert!(
            !ResultFileName::for_run(&run)
                .resolve_in(tmp.path())
                .exists(),
            "no payload anywhere — the replay rung is the only one left"
        );

        let read = read_result_output(&request(tmp.path(), &owner, &run, Some(0)))
            .await
            .expect("reads");
        assert_eq!(read.output.as_deref(), Some("replayed text"));

        // …and a FOREIGN session still reads nothing: `:247`'s filter is supplied, and it holds.
        let read = read_result_output(&request(tmp.path(), &session("s2"), &run, Some(0)))
            .await
            .expect("reads");
        assert!(read.is_empty(), "{read:?}");
    }

    /// `:255`, and why it is distinct from `:247`.
    ///
    /// `:247`'s filter alone already returns `None` for a record whose `sessionId` differs, so a
    /// test that only asserted emptiness would pass without `:255` existing at all. What `:255`
    /// adds is the ability to tell "no record" from "a record for me that failed validation" — so
    /// this asserts an ERROR, not a silent empty output.
    ///
    /// The record is hand-written with an `archivePath` that does not exist, which is exactly what
    /// `validate_replay_record` rejects, and with the CURRENT session's id so the raw
    /// re-verification's four-field conjunction holds.
    #[tokio::test]
    async fn a_replay_record_with_a_mismatched_session_is_rejected() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run0001");
        let owner = session("s1");
        let replay_path = completion_replay_path(tmp.path(), &run);
        tokio::fs::create_dir_all(replay_path.parent().expect("parent"))
            .await
            .expect("mkdir");
        let record = json!({
            "version": 1,
            "runId": run.as_str(),
            "sessionId": owner.as_str(),
            "completedAt": 900,
            "expiresAt": 600_000,
            "completion": { "runId": run.as_str() },
            // Never written — `validate_replay_record` refuses a record that cannot address its
            // own archive.
            "archivePath": tmp.path().join("output-archives").join("gone.json"),
        });
        tokio::fs::write(&replay_path, serde_json::to_vec(&record).expect("json"))
            .await
            .expect("seed");

        let error = read_result_output(&request(tmp.path(), &owner, &run, Some(0)))
            .await
            .expect_err("a record for me that failed validation is an inspection failure");
        assert_eq!(
            error.to_string(),
            "Completion replay record failed validation."
        );

        // The reader that noticed the invalid record REAPED it (`store.rs:157`) — which is
        // precisely why the raw pre-read has to happen first, and is worth asserting here because
        // it is what makes a second read indistinguishable from "never existed".
        assert!(
            !replay_path.exists(),
            "an invalid record is reaped by the reader that noticed it"
        );

        // …and now the OTHER arm, over a record that is genuinely VALID and simply belongs to
        // someone else: `:247`'s session filter fires, the read is a silent empty one rather than
        // an error, and the record SURVIVES (`store.rs:248-285` — a foreign-session record is not
        // this caller's to reap). That contrast is what `:255` buys and what a test asserting only
        // emptiness would have missed entirely.
        {
            let store = WaitCompletionStore::default();
            store
                .record(
                    &run,
                    &json!({ "results": [{ "finalOutput": "s1 only" }] }),
                    1_000,
                    600_000,
                    Some(&ReplayPersistence {
                        results_dir: tmp.path(),
                        session_id: &owner,
                    }),
                )
                .await;
        }
        let read = read_result_output(&request(tmp.path(), &session("s2"), &run, Some(0)))
            .await
            .expect("no error for a foreign record");
        assert!(read.is_empty(), "{read:?}");
        assert!(
            replay_path.exists(),
            "the foreign-session arm must not reap"
        );
        // And the owner still reads it, so the record really was usable.
        let read = read_result_output(&request(tmp.path(), &owner, &run, Some(0)))
            .await
            .expect("reads");
        assert_eq!(read.output.as_deref(), Some("s1 only"));
    }

    /// The containment gate, reused rather than re-implemented: a `sessionFile` a child recorded
    /// outside every trusted root is refused with the gate's own sentence, and with zero roots the
    /// read is refused outright before any path resolution at all.
    #[test]
    fn a_session_file_outside_the_trusted_roots_is_not_dereferenced() {
        let dir = tempfile::tempdir().expect("tempdir");
        let outside = dir.path().join("secrets.jsonl");
        std::fs::write(
            &outside,
            "{\"role\":\"assistant\",\"content\":\"leak me\"}\n",
        )
        .expect("write");

        // (a) zero roots — pi `:222`'s own short-circuit.
        assert_eq!(
            read_session_backed_output(&outside, &[]),
            ResultOutput::default()
        );
        let tail = fleet_view::read_session_messages_tail(&outside, 10, &[]);
        assert!(tail.messages.is_empty());
        assert!(
            tail.warnings
                .iter()
                .any(|warning| warning.contains("without a trusted root")),
            "{:?}",
            tail.warnings
        );

        // (b) a root that does not contain the file — the gate's second refusal, by its sentence.
        let elsewhere = tempfile::tempdir().expect("tempdir");
        let roots = vec![elsewhere.path().to_path_buf()];
        assert_eq!(
            read_session_backed_output(&outside, &roots),
            ResultOutput::default()
        );
        let tail = fleet_view::read_session_messages_tail(&outside, 10, &roots);
        assert!(tail.messages.is_empty(), "no bytes read: {tail:?}");
        assert!(
            tail.warnings
                .iter()
                .any(|warning| warning.contains(&format!(
                    "Refusing to read session transcript path outside trusted roots: {}",
                    outside.display()
                ))),
            "{:?}",
            tail.warnings
        );

        // (c) inside a trusted root it IS read — otherwise (a) and (b) would pass vacuously.
        let roots = vec![dir.path().to_path_buf()];
        assert_eq!(
            read_session_backed_output(&outside, &roots),
            ResultOutput {
                output: Some("leak me".to_string()),
                error_text: None,
            }
        );
    }

    /// pi `:179-180`: an out-of-range child is an EMPTY result, not an error and certainly not a
    /// panic. Covered on both paths — the payload projector and the archive selector.
    #[tokio::test]
    async fn a_step_index_out_of_range_is_reported_not_panicked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let run = RunId::from_token("run0001");
        let owner = session("s1");
        write_async_result_file(
            &ResultWrite {
                results_dir: tmp.path(),
                session_id: &owner,
                run_id: &run,
                written_at: 1,
                async_dir: None,
                tool_call_id: None,
            },
            &payload(&run, &owner, &["a", "b"]),
        )
        .await
        .expect("write");

        // …exactly one past the end, and far past it.
        for step_index in [2usize, 9_999] {
            let read = read_result_output(&request(tmp.path(), &owner, &run, Some(step_index)))
                .await
                .expect("reads");
            assert!(read.is_empty(), "step {step_index}: {read:?}");
        }
        // The in-range neighbours still answer, so the bound is the only thing being tested.
        let read = read_result_output(&request(tmp.path(), &owner, &run, Some(1)))
            .await
            .expect("reads");
        assert_eq!(read.output.as_deref(), Some("b"));

        // The ARCHIVE path: same run, payload gone, replay record present. The selector matches on
        // `entry.result_index == Some(step_index)` and simply finds nothing.
        let replayed = RunId::from_token("run0002");
        {
            let store = WaitCompletionStore::default();
            store
                .record(
                    &replayed,
                    &json!({ "results": [
                        { "agent": "a0", "finalOutput": "first" },
                        { "agent": "a1", "finalOutput": "second" },
                    ] }),
                    1_000,
                    600_000,
                    Some(&ReplayPersistence {
                        results_dir: tmp.path(),
                        session_id: &owner,
                    }),
                )
                .await;
        }
        let read = read_result_output(&request(tmp.path(), &owner, &replayed, Some(1)))
            .await
            .expect("reads");
        assert_eq!(read.output.as_deref(), Some("second"), "the archive rung");
        for step_index in [2usize, 9_999] {
            let read =
                read_result_output(&request(tmp.path(), &owner, &replayed, Some(step_index)))
                    .await
                    .expect("reads");
            assert!(read.is_empty(), "archive step {step_index}: {read:?}");
        }
    }

    /// The payload projector's own shape assertions and its run-level rungs, including the
    /// `summary` rung that is dead for a cyrup payload and ported anyway.
    #[test]
    fn the_payload_projector_reads_both_output_spellings_and_refuses_bad_shapes() {
        // Both spellings, `finalOutput` winning when both are present.
        let both = json!({ "results": [{ "finalOutput": "mine", "output": "theirs" }] });
        assert_eq!(
            result_output(&both, Some(0)).expect("ok").output.as_deref(),
            Some("mine")
        );
        let foreign = json!({ "results": [{ "output": "theirs" }] });
        assert_eq!(
            result_output(&foreign, Some(0))
                .expect("ok")
                .output
                .as_deref(),
            Some("theirs")
        );
        // `error` rides alongside.
        let failed = json!({ "results": [{ "error": "boom" }] });
        assert_eq!(
            result_output(&failed, Some(0)).expect("ok"),
            ResultOutput {
                output: None,
                error_text: Some("boom".to_string()),
            }
        );
        // The run-level `summary` rung (pi `:189`), ported-but-dead.
        let summary = json!({ "summary": "the whole run", "results": [{}, {}] });
        assert_eq!(
            result_output(&summary, None).expect("ok").output.as_deref(),
            Some("the whole run")
        );
        // A multi-child run with no summary attributes nothing to the run itself.
        let many = json!({ "results": [{ "finalOutput": "a" }, { "finalOutput": "b" }] });
        assert!(result_output(&many, None).expect("ok").is_empty());

        for (bad, message) in [
            (json!([]), "Async result payload must be an object."),
            (
                json!({ "results": 7 }),
                "Async result payload results must be an array.",
            ),
            (
                json!({ "summary": 7 }),
                "Async result payload summary must be a string.",
            ),
            (
                json!({ "results": [7] }),
                "Async result payload step result must be an object.",
            ),
            (
                json!({ "results": [{ "finalOutput": 7 }] }),
                "Async result payload step output must be a string.",
            ),
            (
                json!({ "results": [{ "error": 7 }] }),
                "Async result payload step error must be a string.",
            ),
        ] {
            let error = result_output(&bad, Some(0)).expect_err("rejects");
            assert_eq!(error.to_string(), message);
        }
        let error = result_output(&json!({ "results": [7] }), None).expect_err("rejects");
        assert_eq!(
            error.to_string(),
            "Async result payload result must be an object."
        );
    }

    /// The failed-output artifact reader, dead for a cyrup-written artifact and ported anyway —
    /// the prefix branch plus both trailer trims, and the plain-artifact path beside it.
    #[tokio::test]
    async fn the_output_artifact_reader_handles_both_shapes() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let plain = tmp.path().join("output.log");
        tokio::fs::write(&plain, "just the output\n")
            .await
            .expect("write");
        assert_eq!(
            read_output_artifact(&plain).await.expect("reads"),
            ResultOutput {
                output: Some("just the output\n".to_string()),
                error_text: None,
            }
        );

        let failed = tmp.path().join("failed.log");
        tokio::fs::write(
            &failed,
            format!(
                "{FAILED_OUTPUT_ARTIFACT_PREFIX}the child died\n\nTranscript: /t/x.jsonl\nMetadata: \
                 {{}}"
            ),
        )
        .await
        .expect("write");
        assert_eq!(
            read_output_artifact(&failed).await.expect("reads"),
            ResultOutput {
                output: None,
                error_text: Some("the child died".to_string()),
            },
            "both trailers trimmed and the prefix stripped"
        );

        // A `\nMetadata: ` that is NOT the last line must survive — it is the child's own text.
        let midbody = tmp.path().join("midbody.log");
        tokio::fs::write(
            &midbody,
            format!("{FAILED_OUTPUT_ARTIFACT_PREFIX}see\nMetadata: x\nand more"),
        )
        .await
        .expect("write");
        assert_eq!(
            read_output_artifact(&midbody)
                .await
                .expect("reads")
                .error_text
                .as_deref(),
            Some("see\nMetadata: x\nand more")
        );
    }

    /// The windowed tail really is a TAIL, and it lands on a character boundary — the property
    /// `decodeUtf8Tail` exists for, checked over a file far larger than the window.
    #[tokio::test]
    async fn the_artifact_tail_is_bounded_and_utf8_safe() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("big.log");
        // Three-byte scalars, so the window boundary is overwhelmingly likely to land mid-scalar.
        let body = "あ".repeat(MAX_FINAL_OUTPUT_LENGTH * 2);
        tokio::fs::write(&path, &body).await.expect("write");

        let read = read_output_artifact(&path).await.expect("reads");
        let output = read.output.expect("output");
        assert!(
            !output.contains('\u{fffd}'),
            "no replacement character — the window was trimmed to a boundary"
        );
        assert!(
            output.len() <= MAX_FINAL_OUTPUT_LENGTH * 4,
            "{}",
            output.len()
        );
        assert!(
            body.ends_with(&output) && output.len() > MAX_FINAL_OUTPUT_LENGTH * 4 - 3,
            "it is the TAIL, and it fills the window"
        );
    }
}
