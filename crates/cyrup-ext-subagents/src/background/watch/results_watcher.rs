//! The orchestrator-side `ResultsDir` scan primitive: the poll/dedup/retry tuning constants, the
//! `(run id, agent, mtime)` dedup and bounded-retry bookkeeping, and [`ResultsWatcher`] itself —
//! the `notify::PollWatcher` install plus the enumerate → parse → dedup → notify pass
//! (R-SA-098/099/102). Split out of `background/watch.rs`; ports pi
//! `runs/background/result-watcher.ts`.
//!
//! # Enumeration is index-driven, not a directory listing
//!
//! `results_dir` is per-**cwd** (`background/artifact_roots.rs:281-284`), so every concurrent
//! cyrup instance in a directory shares it. This module therefore enumerates through
//! [`crate::background::result_index`] — exactly as pi's `indexedResultCandidates`
//! (`result-watcher.ts:634-644`) does, and pointedly **not** by listing `results_dir`, which would
//! surface every other instance's results.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;

use crate::background::delivery::DeliveryReceipt;
use crate::background::result_index::{self, ConsumablePayload};
use crate::background::{ResultFile, RunId};
use crate::error::SubagentError;
use crate::identity::{ResultFileName, SessionId};

/// Target poll-interval fallback for [`ResultsWatcher`], used when native filesystem notification
/// is unavailable or fails (R-SA-098: "a fixed-interval poll fallback... used when native
/// notification is unavailable or fails, e.g. resource-exhaustion errors"). `notify::PollWatcher`
/// is simultaneously both the native-notification-preferring path AND the poll-fallback path (see
/// module docs), so there is no separate branch to maintain.
pub const RESULTS_DIR_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// Bounded TTL for the dedup seen-set (R-SA-099: "a bounded TTL (target ~10 minutes)") — after
/// this long, a given `(run_id, agent, timestamp)` composite key is evicted from the seen-set and
/// would (in the practically-unreachable case the SAME result file somehow reappeared) be treated
/// as new again, rather than the seen-set growing unbounded over a long-lived orchestrator
/// process's lifetime.
pub const DEDUP_TTL: Duration = Duration::from_secs(10 * 60);

/// Upper bound on retry-in-place attempts for a single result file before this watcher gives up
/// re-surfacing it as an ordinary notification and instead reports it as
/// [`CompletionNotification::exhausted`] (R-SA-102: "SHOULD NOT retry indefinitely without any
/// bound"). Chosen generously (well above any plausible number of transient-failure poll cycles a
/// genuinely-recoverable condition would need) since the cost of one extra retry is negligible and
/// the failure mode being guarded against is "spins forever", not "retries slightly too many
/// times".
pub const MAX_PROCESSING_ATTEMPTS: u32 = 20;

/// How many CONSECUTIVE scans an indexed candidate may fail to resolve before its result is
/// declared lost.
///
/// Three ticks is ~1.5s at [`RESULTS_DIR_POLL_INTERVAL`], comfortably longer than any
/// stage → index → promote window, so a payload caught mid-write is never mistaken for a stolen
/// one. The bound exists because the alternative — waiting forever — is what the previous
/// enumeration did: it dropped an unresolvable candidate on every scan, silently, for as long as
/// the process lived.
pub const MISSING_PAYLOAD_GRACE_SCANS: u32 = 3;

// =================================================================================================
// CompletionNotification
// =================================================================================================

/// One notified-but-not-yet-deleted result, surfaced to the caller's own turn/prompt-handling path
/// (R-SA-101: "MUST re-enter the orchestrator's normal turn/prompt-handling path"). This module
/// does not itself know how to inject a message into a live session — that is a later phase's
/// `background/tracker.rs`/`registration/` responsibility (see module docs); this type is the
/// plain, inert payload that hand-off is built from.
#[derive(Debug, Clone, PartialEq)]
pub struct CompletionNotification {
    /// The parsed terminal result.
    pub result: ResultFile,
    /// The on-disk path this result was read from — needed by
    /// [`ResultsWatcher::consume`] to destroy the correct file.
    pub result_path: PathBuf,
    /// `true` if this notification is being surfaced only because [`MAX_PROCESSING_ATTEMPTS`] was
    /// exceeded (R-SA-102's retry bound) — the caller should treat this as a terminal
    /// "give up, log/alert" signal rather than attempting normal turn-re-entry delivery again, since
    /// every prior attempt already failed.
    pub exhausted: bool,
}

// =================================================================================================
// DedupKey / SeenEntry
// =================================================================================================

/// The dedup key R-SA-099 specifies: "a composite of run id/agent/timestamp". `agent` here is the
/// top-level [`ResultFile::agent`]; `timestamp` is the result file's own on-disk mtime (epoch
/// seconds) at the moment it was first observed, which — combined with run id and agent — is
/// stable across repeated polls of the SAME still-undeleted file (a file is never rewritten in
/// place once written, per `runner_main::finish_run`'s single write-and-done contract, so its mtime
/// does not change between polls) while still changing if a run id were ever hypothetically reused
/// for a genuinely new, later result.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct DedupKey {
    run_id: String,
    agent: String,
    mtime_epoch_secs: i64,
}

/// One dedup entry: when it was first inserted, for TTL eviction (R-SA-099).
struct SeenEntry {
    inserted_at: Instant,
}

/// One retry-bookkeeping entry (R-SA-102): how many times processing of this key has been
/// reported as failed via [`ResultsWatcher::record_processing_failure`], and when the FIRST such
/// failure was recorded (for the identical TTL eviction policy as [`SeenEntry`] — a key whose
/// owning result file was never actually deleted but which also stopped being polled for
/// [`DEDUP_TTL`] should not pin memory forever).
struct RetryEntry {
    attempts: u32,
    inserted_at: Instant,
}

/// One missing-payload streak: consecutive scans in which an indexed candidate resolved to
/// `PayloadResolution::Absent`. Same bounded-TTL discipline as the two maps beside it.
struct MissingEntry {
    consecutive: u32,
    inserted_at: Instant,
}

/// Everything one scan turned up, in three piles the drain loop treats differently.
///
/// A single `Vec<CompletionNotification>` cannot express this: "a completion I own", "a completion
/// I may only watch" and "a completion whose result is gone" call for three different actions,
/// and the previous shape — one list, plus predicates consulted afterwards — is what allowed a
/// foreign result to be destroyed and a missing one to be ignored.
#[derive(Debug)]
pub struct ScanOutcome {
    /// Candidates from this instance's own index partition whose payload resolved.
    pub resolved: Vec<ResolvedCandidate>,
    /// Payloads reached through the cross-session bands (tracked ids, mission observer). No
    /// entitlement is carried, so nothing here can be consumed.
    pub observed: Vec<ObservedResult>,
    /// Runs that completed and whose result payload is gone.
    pub missing: Vec<LossReport>,
}

/// One own-partition candidate, read and entitled.
#[derive(Debug)]
pub struct ResolvedCandidate {
    /// The parsed terminal result.
    pub result: ResultFile,
    /// The entitlement to destroy the payload it was read from.
    pub payload: ConsumablePayload,
    /// The run's directory, when the index recorded one.
    pub async_dir: Option<PathBuf>,
    /// `true` if this is being surfaced only because [`MAX_PROCESSING_ATTEMPTS`] was exceeded
    /// (R-SA-102) — a terminal "give up" signal rather than another ordinary delivery attempt.
    pub exhausted: bool,
}

/// A completion seen through a cross-session band: observable, never consumable.
#[derive(Debug)]
pub struct ObservedResult {
    /// The parsed terminal result.
    pub result: ResultFile,
    /// Where it was read from — for diagnostics only; this instance never writes to it.
    pub path: PathBuf,
}

/// What [`ResultsWatcher::inspect_candidate`] decided about one candidate.
enum CandidateOutcome {
    Ready(ResolvedCandidate),
    Missing(LossReport),
    /// Nothing to do THIS scan: already notified, malformed on disk, or a probe fault. Never
    /// "the payload is gone" — that is [`Self::Missing`], which is always reported.
    Skip,
}

/// Read and parse a result payload, returning it with the mtime the dedup key needs.
///
/// `None` for absent, unreadable or malformed — all three leave the file exactly where it is.
async fn read_result_at(path: &std::path::Path) -> Option<(ResultFile, i64)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let mtime_epoch_secs = metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0);
    let bytes = tokio::fs::read(path).await.ok()?;
    match serde_json::from_slice::<ResultFile>(&bytes) {
        Ok(result) => Some((result, mtime_epoch_secs)),
        Err(error) => {
            tracing::warn!(
                path = %path.display(),
                %error,
                "ignoring a malformed result payload; it is left in place, never deleted"
            );
            None
        }
    }
}

/// What to do about a candidate whose payload did not resolve.
///
/// The enumeration this replaces had no such decision to make: an unresolvable candidate was
/// dropped before the watcher saw it. Naming the two outcomes is what makes the silent third one
/// — "say nothing, forever" — unrepresentable.
#[derive(Debug)]
pub enum MissingPayloadVerdict {
    /// Inside the grace window, or the probe itself failed. Say nothing; look again next scan.
    KeepWaiting,
    /// The payload is gone. Announce the loss with whatever the run's own records still hold,
    /// then retire the dangling index entries.
    DeclareLost(LossReport),
}

/// Everything still knowable about a run whose result payload was destroyed before delivery.
///
/// The payload is the only thing that goes missing: `status.json` and `events.jsonl` live in the
/// run's own directory, which no other process touches. That is why a loss can be *reported with
/// content* rather than merely admitted — `steps[].recent_output` carries a tail of the child's
/// actual answer.
#[derive(Debug, Clone)]
pub struct LossReport {
    /// The run whose result was lost.
    pub run_id: RunId,
    /// The agent name(s) the run's status recorded, for the notification header.
    pub agent: String,
    /// The run's own directory, when the index recorded one — the artifact pointer.
    pub async_dir: Option<PathBuf>,
    /// Recovered output tails, one per step that recorded any.
    pub recovered: Vec<RecoveredStep>,
    /// When the run ended (epoch millis), when its status recorded it.
    pub ended_at: Option<i64>,
}

/// One step's recovered output.
#[derive(Debug, Clone)]
pub struct RecoveredStep {
    /// The agent that produced it.
    pub agent: String,
    /// The tail of its output, as `status.json` recorded it.
    pub output: String,
}

// =================================================================================================
// ResultsWatcher
// =================================================================================================

/// The orchestrator-side `ResultsDir` watch state: an in-memory, bounded-TTL seen-set (R-SA-099)
/// guarding against re-notifying for the same result twice, plus a bounded per-key retry-attempt
/// counter (R-SA-102) tracked in a SEPARATE map so "currently suppressed as already-notified" and
/// "how many processing attempts has this key burned" are independent pieces of state — a key can
/// be absent from `seen` (so the next scan will surface it again) while still present in
/// `retry_attempts` (so that next surfacing knows it is not the FIRST attempt and can eventually
/// reach [`MAX_PROCESSING_ATTEMPTS`]). Collapsing these into one map (an earlier version of this
/// type tried that) creates a contradiction: re-inserting into a single seen-map immediately after
/// `record_processing_failure` makes the very next `scan_candidates` see the key as already-present and
/// suppress it again, which defeats retry-in-place entirely. Both maps are shared behind an
/// `Arc<AsyncMutex<..>>` internally so they can be driven from a `notify` callback (which runs on
/// an arbitrary background thread, not necessarily a `tokio` task) as well as read/polled from
/// async code.
pub struct ResultsWatcher {
    results_dir: PathBuf,
    seen: Arc<AsyncMutex<HashMap<DedupKey, SeenEntry>>>,
    retry_attempts: Arc<AsyncMutex<HashMap<DedupKey, RetryEntry>>>,
    /// Consecutive unresolvable-payload scans per run id (see [`MISSING_PAYLOAD_GRACE_SCANS`]).
    /// A third map for the same reason the second one is separate: "already notified", "how many
    /// processing attempts failed" and "how many scans found no payload" are independent facts,
    /// and folding any two of them together makes one of the three unreadable.
    missing_payload: Arc<AsyncMutex<HashMap<String, MissingEntry>>>,
}

impl ResultsWatcher {
    /// Construct a watcher over `results_dir` (does not itself install any filesystem watch — call
    /// [`ResultsWatcher::install`] for that; this constructor is infallible and purely sets up the
    /// in-memory dedup state, so a caller can hold a [`ResultsWatcher`] value before deciding
    /// whether/when to actually attach a live watch).
    #[must_use]
    pub fn new(results_dir: PathBuf) -> Self {
        Self {
            results_dir,
            seen: Arc::new(AsyncMutex::new(HashMap::new())),
            retry_attempts: Arc::new(AsyncMutex::new(HashMap::new())),
            missing_payload: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }

    /// Borrows the directory this watcher is scoped to.
    #[must_use]
    pub fn results_dir(&self) -> &std::path::Path {
        &self.results_dir
    }

    /// Install a real `notify::PollWatcher` over this watcher's `results_dir` (R-SA-098),
    /// forwarding every observed filesystem event as a bare wake-up notification on the returned
    /// channel — mirroring `control::watch_control_inbox`'s identical "notify only, let the
    /// receiver re-scan" pattern, since `notify` does not guarantee events are coalesced 1:1 with
    /// actual result-file arrivals (a burst of writes can coalesce to fewer wake-ups, and a single
    /// write can sometimes surface as more than one event — the receiver is expected to always
    /// re-`scan_candidates` on each wake-up rather than trying to interpret the event
    /// payload itself).
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] if the underlying `notify` watcher cannot be constructed or
    /// attached to `results_dir` (e.g. it does not exist yet — callers are expected to `mkdir` the
    /// results directory before installing a watch on it, mirroring how `AsyncRoot`/`ResultsDir`
    /// are established once by extension initialization).
    pub fn install(
        &self,
    ) -> Result<
        (
            notify::PollWatcher,
            tokio::sync::mpsc::UnboundedReceiver<()>,
        ),
        SubagentError,
    > {
        use notify::Watcher;

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<()>();
        let cfg = notify::Config::default()
            .with_poll_interval(RESULTS_DIR_POLL_INTERVAL)
            .with_compare_contents(false);
        let mut watcher = notify::PollWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    let _ = tx.send(());
                }
            },
            cfg,
        )
        .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

        watcher
            .watch(&self.results_dir, notify::RecursiveMode::NonRecursive)
            .map_err(|e| SubagentError::Spawn(std::io::Error::other(e.to_string())))?;

        Ok((watcher, rx))
    }

    /// Scan this watcher's own partition and return every not-yet-notified
    /// [`CompletionNotification`], deduplicated against the bounded-TTL seen-set.
    ///
    /// # Candidates come from the INDEX, never from a directory listing
    ///
    /// `results_dir` is `<temp_root>/results/<cwd_key>` (`background/artifact_roots.rs:281-284`) —
    /// keyed by **cwd**, never by session. Every concurrent cyrup instance in a directory resolves
    /// the identical path, so listing it returns every instance's results. This function therefore
    /// enumerates through [`crate::background::result_index`], whose candidate sources correspond
    /// 1:1 with pi's `indexedResultCandidates` (`result-watcher.ts:634-644`):
    ///
    /// 1. the current session's partition, plus any claimed predecessor sessions,
    /// 2. run ids this process is actively tracking (supplied by the caller),
    /// 3. the cross-session mission observer band,
    /// 4. explicitly observed run ids (supplied by the caller).
    ///
    /// A candidate's payload may still be **staged** rather than public, so each is resolved
    /// through the index rather than by joining `results_dir` — which is also what promotes a
    /// payload left behind by a runner that died before promoting.
    ///
    /// # This does not decide ownership
    ///
    /// Enumeration narrows to what this instance could plausibly need; it does not decide what to
    /// *do*. That is [`crate::background::delivery::Attribution::classify`]'s job, and it
    /// is deliberately separate: the mission observer band legitimately surfaces other sessions'
    /// results, which must be observed but never delivered or deleted.
    ///
    /// # Errors
    ///
    /// Returns [`SubagentError::Spawn`] only for a genuine I/O fault. A missing index tree, an
    /// unreadable partition and an individual file's parse failure are all non-fatal.
    pub async fn scan_candidates(
        &self,
        sessions: &[SessionId],
        extra_run_ids: &[RunId],
    ) -> Result<ScanOutcome, SubagentError> {
        self.evict_expired().await;

        let mut candidates: Vec<result_index::ResultCandidate> = Vec::new();
        for session_id in sessions {
            for candidate in
                result_index::result_candidates_for_session(&self.results_dir, session_id)
                    .await
                    .map_err(SubagentError::Spawn)?
            {
                if !candidates.iter().any(|c| c.run_id == candidate.run_id) {
                    candidates.push(candidate);
                }
            }
        }

        // Candidate sources #2 and #4 (tracked run ids) and #3 (the cross-session mission observer
        // band) resolve to payloads this instance may only OBSERVE unless the session index also
        // claimed them above, so they are collected as plain file names and read directly.
        let mut observed_files: BTreeSet<ResultFileName> = BTreeSet::new();
        for run_id in extra_run_ids {
            observed_files.insert(ResultFileName::for_run(run_id));
        }
        observed_files.extend(
            result_index::mission_observer_result_candidate_files(&self.results_dir)
                .await
                .map_err(SubagentError::Spawn)?,
        );

        let mut owned = Vec::new();
        let mut missing = Vec::new();
        for candidate in candidates {
            match self.inspect_candidate(candidate).await {
                CandidateOutcome::Ready(entry) => owned.push(entry),
                CandidateOutcome::Missing(report) => missing.push(report),
                CandidateOutcome::Skip => {}
            }
        }

        let mut observed = Vec::new();
        for file in observed_files {
            // These have no session partition to resolve through, so the legacy root path is the
            // only address available; a payload that is not there is simply not a candidate.
            let path = file.resolve_in(&self.results_dir);
            if let Some((result, mtime)) = read_result_at(&path).await {
                let key = DedupKey {
                    run_id: result.run_id.as_str().to_string(),
                    agent: result.agent.clone(),
                    mtime_epoch_secs: mtime,
                };
                if matches!(self.check_and_mark_seen(&key).await, SeenOutcome::AlreadyNotified) {
                    continue;
                }
                observed.push(ObservedResult { result, path });
            }
        }

        Ok(ScanOutcome {
            resolved: owned,
            observed,
            missing,
        })
    }

    /// Resolve one candidate into the outcome the drain loop acts on.
    ///
    /// There is no arm that quietly drops a candidate whose payload is gone: that was the silent
    /// loss. `Skip` covers only the two cases with a live payload that genuinely need no action —
    /// already notified, or malformed on disk (left in place, never destroyed).
    async fn inspect_candidate(
        &self,
        candidate: result_index::ResultCandidate,
    ) -> CandidateOutcome {
        let result_index::ResultCandidate {
            run_id,
            async_dir,
            payload,
            ..
        } = candidate;

        let consumable = match payload {
            result_index::PayloadResolution::Found(consumable) => consumable,
            // A probe fault proves nothing: it is not evidence that a payload is gone, so it must
            // never move a run toward being declared lost.
            result_index::PayloadResolution::Unreadable(error) => {
                tracing::debug!(
                    run_id = %run_id,
                    %error,
                    "could not probe an indexed result payload; retrying on the next scan"
                );
                return CandidateOutcome::Skip;
            }
            result_index::PayloadResolution::Absent => {
                return match self.note_absent_payload(&run_id).await {
                    MissingPayloadVerdict::KeepWaiting => CandidateOutcome::Skip,
                    MissingPayloadVerdict::DeclareLost(mut report) => {
                        if let Some(async_dir) = async_dir.as_deref() {
                            Self::recover_from_status(&mut report, async_dir).await;
                        }
                        CandidateOutcome::Missing(report)
                    }
                };
            }
        };

        // The payload resolved, so any streak this run had accumulated is over.
        self.missing_payload.lock().await.remove(run_id.as_str());

        let Some((result, mtime_epoch_secs)) = read_result_at(consumable.path()).await else {
            // Present but unreadable or malformed: left exactly where it is, never destroyed.
            return CandidateOutcome::Skip;
        };

        let key = DedupKey {
            run_id: result.run_id.as_str().to_string(),
            agent: result.agent.clone(),
            mtime_epoch_secs,
        };
        match self.check_and_mark_seen(&key).await {
            SeenOutcome::AlreadyNotified => CandidateOutcome::Skip,
            SeenOutcome::Exhausted => CandidateOutcome::Ready(ResolvedCandidate {
                result,
                payload: consumable,
                async_dir,
                exhausted: true,
            }),
            SeenOutcome::NewlySeen => CandidateOutcome::Ready(ResolvedCandidate {
                result,
                payload: consumable,
                async_dir,
                exhausted: false,
            }),
        }
    }

    /// Record one scan in which an indexed run's payload was absent, and decide whether that is
    /// now a loss.
    ///
    /// Builds the [`LossReport`] from the run's OWN records, which survive whatever removed the
    /// payload: `status.json` holds the terminal state, the agent names and a tail of each step's
    /// output. Reporting a loss without that content would tell the orchestrator only that
    /// something disappeared; reporting it WITH the content usually hands back the answer.
    async fn note_absent_payload(&self, run_id: &RunId) -> MissingPayloadVerdict {
        let consecutive = {
            let mut missing = self.missing_payload.lock().await;
            let entry = missing.entry(run_id.as_str().to_string()).or_insert(MissingEntry {
                consecutive: 0,
                inserted_at: Instant::now(),
            });
            entry.consecutive = entry.consecutive.saturating_add(1);
            entry.consecutive
        };
        if consecutive < MISSING_PAYLOAD_GRACE_SCANS {
            return MissingPayloadVerdict::KeepWaiting;
        }
        // Declared: stop counting, so the report is produced exactly once per run.
        self.missing_payload.lock().await.remove(run_id.as_str());
        MissingPayloadVerdict::DeclareLost(LossReport {
            run_id: run_id.clone(),
            agent: String::new(),
            async_dir: None,
            recovered: Vec::new(),
            ended_at: None,
        })
    }

    /// Fill a [`LossReport`] from the run's own `status.json`.
    ///
    /// Read through [`crate::background::RunDir::status`], which needs only the run directory —
    /// the results dir is exactly the thing that failed us here. A status that is missing or
    /// unreadable leaves the report as it is: the notification is still sent, because "a run
    /// finished and its result is gone" is worth saying even with nothing to attach to it.
    pub(super) async fn recover_from_status(report: &mut LossReport, async_dir: &std::path::Path) {
        report.async_dir = Some(async_dir.to_path_buf());
        let status_path = async_dir.join("status.json");
        let Ok(bytes) = tokio::fs::read(&status_path).await else {
            return;
        };
        let Ok(status) = serde_json::from_slice::<crate::background::RunStatus>(&bytes) else {
            return;
        };
        report.ended_at = status.ended_at;
        // Same agent-naming rule the terminal result would have carried (pi `agentName`,
        // `subagent-runner.ts:4765-4770`): one step is named for its agent, several are named for
        // the shape that produced them.
        let names: Vec<&str> = status.steps.iter().map(|s| s.agent.as_str()).collect();
        report.agent = match names.as_slice() {
            [] => report.run_id.as_str().to_string(),
            [only] => (*only).to_string(),
            many if status.mode == crate::background::RunMode::Parallel => {
                format!("parallel:{}", many.join("+"))
            }
            many => format!("chain:{}", many.join("->")),
        };
        report.recovered = status
            .steps
            .iter()
            .filter(|step| !step.telemetry.recent_output.is_empty())
            .map(|step| RecoveredStep {
                agent: step.agent.clone(),
                output: step.telemetry.recent_output.join("\n"),
            })
            .collect();
    }


    /// Destroy one DELIVERED result: its payload, then its index entries.
    ///
    /// # Both proofs are required, and both are consumed
    ///
    /// `payload` is the entitlement, minted only by resolving through this instance's own session
    /// partition, so another instance's result cannot be passed in at all. `receipt` is the
    /// evidence that the notification actually reached the session, mintable only by a sink that
    /// delivered it. Taking both BY VALUE means neither can authorise a second destruction.
    ///
    /// This is R-SA-099's delete-last ordering made structural: previously the same rule was
    /// carried by a comment and a `bool`, and the `bool` was produced by a sink that could not
    /// observe delivery at all.
    ///
    /// The payload and the index entries are one operation. Under index-driven enumeration a
    /// leftover entry is not harmless cruft: it is a permanent CANDIDATE that every later scan
    /// re-resolves and — now — eventually reports as a lost result.
    ///
    /// # Errors
    ///
    /// A genuine I/O failure OTHER than the payload already being absent (a concurrent second
    /// watcher, or a prior successful delete): `NotFound` is an achieved goal, not an error.
    pub async fn consume(
        &self,
        payload: ConsumablePayload,
        receipt: DeliveryReceipt,
    ) -> Result<(), SubagentError> {
        debug_assert_eq!(
            payload.run_id(),
            receipt.run_id(),
            "a receipt must authorise the payload it is paired with"
        );
        // Read the mtime BEFORE unlinking: once the file is gone the value the dedup/retry keys
        // were built from is unrecoverable, and the cleanup below would silently miss its entry.
        let mtime_epoch_secs = mtime_epoch_secs_of(payload.path()).await;
        let outcome = match tokio::fs::remove_file(payload.path()).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(SubagentError::Spawn(e)),
        };
        if outcome.is_ok() {
            self.retire(
                payload.session_id(),
                payload.run_id(),
                mtime_epoch_secs.unwrap_or(0),
            )
            .await;
        }
        outcome
    }

    /// Retire a run's index entries and its in-memory bookkeeping.
    ///
    /// Shared by [`Self::consume`] (the payload was delivered and destroyed) and by the lost-result
    /// path (the payload is already gone and the entries would otherwise be re-resolved forever).
    pub(super) async fn retire(
        &self,
        session_id: &SessionId,
        run_id: &RunId,
        mtime_epoch_secs: i64,
    ) {
        // pi `removeDeliveredResult` unlinks the payload AND the indexes
        // (`result-files.ts:185-219`).
        result_index::remove_result_index(&self.results_dir, Some(session_id), run_id, None).await;
        self.missing_payload.lock().await.remove(run_id.as_str());
        // The result can never be re-scanned, so its retry bookkeeping is dead weight. Clearing it
        // here rather than at TTL keeps the map bounded by "currently-failing results" rather than
        // "every result that ever failed once".
        let mut retry = self.retry_attempts.lock().await;
        retry.retain(|key, _| {
            key.run_id != run_id.as_str() || key.mtime_epoch_secs != mtime_epoch_secs
        });
    }

    /// R-SA-102: tell this watcher that downstream processing of an already-returned
    /// [`CompletionNotification`] failed transiently, so the underlying result file should be
    /// retried on the next scan rather than treated as permanently delivered. This un-marks the
    /// result's dedup key (the next `scan_candidates` call will re-surface it, since the
    /// file itself was never deleted) and increments its attempt counter, bounded at
    /// [`MAX_PROCESSING_ATTEMPTS`] — once exceeded, the NEXT scan surfaces it one final time with
    /// [`CompletionNotification::exhausted`] set and stops retrying it thereafter (this method is a
    /// no-op for a key that has already reached the exhausted state, so a caller cannot
    /// accidentally resurrect infinite retries by calling this repeatedly against an exhausted key).
    pub async fn record_processing_failure(&self, candidate: &ResolvedCandidate) {
        let key = DedupKey {
            run_id: candidate.result.run_id.as_str().to_string(),
            agent: candidate.result.agent.clone(),
            mtime_epoch_secs: mtime_epoch_secs_of(candidate.payload.path())
                .await
                .unwrap_or(0),
        };

        // Un-mark from the dedup seen-set: this is the entire "retry-in-place" mechanism — the
        // next `scan_candidates` call will treat this key as unseen again and re-surface
        // it, since the underlying result file itself was never touched. Kept as a SEPARATE map
        // from the attempt counter below (see [`ResultsWatcher`]'s own doc note on why merging
        // them is a bug: re-inserting into a single seen-map here would make the very next scan
        // see the key as already-present and suppress it again).
        self.seen.lock().await.remove(&key);

        let mut retry_attempts = self.retry_attempts.lock().await;
        let attempts = retry_attempts
            .get(&key)
            .map_or(0, |entry| entry.attempts)
            .saturating_add(1);
        retry_attempts.insert(
            key,
            RetryEntry {
                attempts,
                inserted_at: Instant::now(),
            },
        );
        // R-SA-102's bound itself is enforced on the READ side, by `check_and_mark_seen` comparing
        // this stored `attempts` count against `MAX_PROCESSING_ATTEMPTS` the next time this key is
        // scanned — this method's own job is only to record that one more failure happened.
    }

    async fn check_and_mark_seen(&self, key: &DedupKey) -> SeenOutcome {
        let mut seen = self.seen.lock().await;
        if seen.contains_key(key) {
            return SeenOutcome::AlreadyNotified;
        }

        let prior_attempts = self
            .retry_attempts
            .lock()
            .await
            .get(key)
            .map_or(0, |entry| entry.attempts);

        seen.insert(
            key.clone(),
            SeenEntry {
                inserted_at: Instant::now(),
            },
        );

        if prior_attempts >= MAX_PROCESSING_ATTEMPTS {
            SeenOutcome::Exhausted
        } else {
            SeenOutcome::NewlySeen
        }
    }

    /// Evict every seen-set/retry-attempt entry older than [`DEDUP_TTL`] (R-SA-099's bounded-TTL
    /// requirement, extended identically to the retry-attempt map) — called at the top of every
    /// scan so neither map grows without bound across a long-lived orchestrator process's
    /// lifetime.
    async fn evict_expired(&self) {
        let now = Instant::now();
        self.seen
            .lock()
            .await
            .retain(|_, entry| now.duration_since(entry.inserted_at) < DEDUP_TTL);
        self.retry_attempts
            .lock()
            .await
            .retain(|_, entry| now.duration_since(entry.inserted_at) < DEDUP_TTL);
        // Same bound for the missing-payload streaks: a run that stopped being enumerated (its
        // index entries were retired elsewhere) must not pin memory, and a streak that went cold
        // for a whole TTL is no longer describing consecutive scans anyway.
        self.missing_payload
            .lock()
            .await
            .retain(|_, entry| now.duration_since(entry.inserted_at) < DEDUP_TTL);
    }
}

/// Outcome of checking-then-marking a dedup key as seen — distinguishes "brand new" from "already
/// notified, suppress" from "retry-bound exceeded, surface one last time as exhausted".
enum SeenOutcome {
    NewlySeen,
    AlreadyNotified,
    Exhausted,
}

/// Reads a path's on-disk mtime as epoch seconds, or `None` if the file is gone/unreadable (e.g.
/// deleted concurrently between the caller's original scan and its later
/// [`ResultsWatcher::record_processing_failure`] call) — never a panic on a missing file.
async fn mtime_epoch_secs_of(path: &std::path::Path) -> Option<i64> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    let modified = metadata.modified().ok()?;
    let duration = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::indexing_slicing)]

    use super::super::tests::{publish_result, sample_result, temp_results_dir, test_session};
    use super::*;
    use crate::background::RunState;
    use crate::background::atomic::write_atomic_json;

    /// Scan for the session every fixture belongs to, keeping the OWN-partition pile.
    async fn scan_own(watcher: &ResultsWatcher) -> Vec<ResolvedCandidate> {
        watcher
            .scan_candidates(&[test_session()], &[])
            .await
            .expect("scan succeeds")
            .resolved
    }

    /// The promoted payload path for a fixture run in the test session.
    fn owned_path(results_dir: &std::path::Path, run: &str) -> PathBuf {
        crate::background::result_index::owned_payload_path(
            results_dir,
            &test_session(),
            &RunId::from_token(run),
        )
    }

    /// Consume a resolved candidate the way the drain loop does: with a real receipt.
    async fn consume_delivered(watcher: &ResultsWatcher, candidate: ResolvedCandidate) {
        let receipt = match crate::background::delivery::CompletionDelivery::delivered(
            candidate.result.run_id.clone(),
        ) {
            crate::background::delivery::CompletionDelivery::Delivered(receipt) => receipt,
            crate::background::delivery::CompletionDelivery::Deferred => {
                unreachable!("constructed as delivered")
            }
        };
        watcher
            .consume(candidate.payload, receipt)
            .await
            .expect("consume succeeds");
    }

    // ---------------------------------------------------------------------------------------
    // scan: R-SA-098/099 basic detection + dedup
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn scan_finds_a_freshly_written_result_file() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        let result = sample_result("run00001", RunState::Complete, true);
        publish_result(&results_dir, &result).await;

        let watcher = ResultsWatcher::new(results_dir);
        let found = scan_own(&watcher).await;
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].result.run_id, result.run_id);
        assert!(!found[0].exhausted);
    }

    #[tokio::test]
    async fn scan_does_not_renotify_an_already_seen_result() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        let result = sample_result("run00002", RunState::Complete, true);
        publish_result(&results_dir, &result).await;

        let watcher = ResultsWatcher::new(results_dir);
        let first = scan_own(&watcher).await;
        assert_eq!(first.len(), 1);

        // The file is still on disk (not deleted) but a second scan without an intervening
        // `consume` must NOT re-surface it (R-SA-099 dedup).
        let second = scan_own(&watcher).await;
        assert!(
            second.is_empty(),
            "a second scan before deletion must not re-notify for the same result"
        );
    }

    #[tokio::test]
    async fn a_foreign_sessions_result_is_neither_enumerated_nor_deleted() {
        // Replaces `scan_for_session_defers_a_result_that_does_not_belong_to_the_session`, whose
        // name described a `RunId` predicate that no longer exists. The property it was reaching
        // for is now structural: a foreign session's result is not even a CANDIDATE, because
        // enumeration walks this instance's own index partition rather than listing the shared
        // directory.
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00009", RunState::Complete, true);
        publish_result(&results_dir, &result).await;
        let path = owned_path(&results_dir, "run00009");
        assert!(
            path.exists(),
            "precondition: the payload is promoted into its owner's partition"
        );
        assert!(
            !results_dir.join("run00009.json").exists(),
            "and NOT into the shared root, where an index-blind process could take it"
        );

        let watcher = ResultsWatcher::new(results_dir.clone());

        // A DIFFERENT session scans the same directory.
        let other = crate::identity::SessionId::parse("other-session").expect("non-empty");
        let found = watcher
            .scan_candidates(&[other], &[])
            .await
            .expect("scan succeeds");
        assert!(
            found.resolved.is_empty(),
            "another instance's result must not even be enumerated"
        );
        assert!(
            path.exists(),
            "and it must certainly not be deleted — its owner still needs it"
        );

        // The owning session sees exactly the same still-on-disk file.
        let found_own = scan_own(&watcher).await;
        assert_eq!(found_own.len(), 1, "the owner enumerates its own result");
        assert_eq!(found_own[0].result.run_id, result.run_id);
    }

    #[tokio::test]
    async fn an_unindexed_payload_is_invisible_to_enumeration() {
        // The inverse property, and the reason `publish_result` exists: dropping a payload
        // straight into the shared directory (what a pre-change runner did) makes it
        // undiscoverable. It is bounded, self-clearing, and never delivered to the wrong instance.
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00777", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00777.json"), &result)
            .await
            .expect("write an unindexed payload");

        let watcher = ResultsWatcher::new(results_dir.clone());
        assert!(
            scan_own(&watcher).await.is_empty(),
            "an unindexed payload has no index entry, so nothing enumerates it"
        );
        assert!(results_dir.join("run00777.json").exists(), "and nothing deletes it");
    }

    #[tokio::test]
    async fn an_explicitly_tracked_run_id_is_a_candidate_even_without_a_session_partition() {
        // pi's candidate source #2 (`result-watcher.ts:641`, `state.asyncJobs.keys()`): a run this
        // process is actively tracking is a candidate regardless of index state.
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");
        let result = sample_result("run00778", RunState::Complete, true);
        write_atomic_json(&results_dir.join("run00778.json"), &result)
            .await
            .expect("write payload");

        let watcher = ResultsWatcher::new(results_dir);
        let found = watcher
            .scan_candidates(&[], &[RunId::from_token("run00778")])
            .await
            .expect("scan succeeds");
        assert_eq!(
            found.observed.len(),
            1,
            "a tracked run id is always a candidate, and is OBSERVED: it carries no entitlement, \
             so nothing about it can be consumed"
        );
    }

    #[tokio::test]
    async fn consume_removes_the_payload_and_its_index() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        let result = sample_result("run00003", RunState::Complete, true);
        publish_result(&results_dir, &result).await;
        let path = owned_path(&results_dir, "run00003");
        assert!(path.exists(), "precondition");

        let watcher = ResultsWatcher::new(results_dir.clone());
        let mut found = scan_own(&watcher).await;
        consume_delivered(&watcher, found.remove(0)).await;
        assert!(!path.exists(), "the payload is gone");

        // And so are its index entries: a leftover entry is a permanent candidate that later scans
        // would re-resolve and eventually report as a LOST result for a run that was delivered.
        assert!(
            scan_own(&watcher).await.is_empty(),
            "nothing remains to enumerate"
        );
    }

    #[tokio::test]
    async fn malformed_result_file_is_skipped_not_deleted() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        let path = results_dir.join("garbage.json");
        tokio::fs::write(&path, b"not valid json")
            .await
            .expect("write garbage");

        let watcher = ResultsWatcher::new(results_dir);
        let found = scan_own(&watcher).await;
        assert!(found.is_empty());
        assert!(
            path.exists(),
            "a malformed result file must be left in place, never silently deleted"
        );
    }

    #[tokio::test]
    async fn non_json_sibling_files_are_ignored() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        tokio::fs::write(results_dir.join("README.md"), b"not a result")
            .await
            .expect("write sibling");

        let watcher = ResultsWatcher::new(results_dir);
        let found = scan_own(&watcher).await;
        assert!(found.is_empty());
    }

    #[tokio::test]
    async fn scan_over_missing_directory_returns_empty_not_error() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let missing = dir.path().join("does-not-exist-yet");

        let watcher = ResultsWatcher::new(missing);
        let found = scan_own(&watcher).await;
        assert!(found.is_empty());
    }

    // ---------------------------------------------------------------------------------------
    // R-SA-102: bounded retry-in-place on transient processing failure
    // ---------------------------------------------------------------------------------------

    #[tokio::test]
    async fn record_processing_failure_makes_the_result_reappear_on_the_next_scan() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        let result = sample_result("run00010", RunState::Complete, true);
        publish_result(&results_dir, &result).await;

        let watcher = ResultsWatcher::new(results_dir);
        let first = scan_own(&watcher).await;
        assert_eq!(first.len(), 1);

        // Without recording a failure, a second scan sees nothing (normal dedup).
        let second = scan_own(&watcher).await;
        assert!(second.is_empty());

        // Simulate the caller failing to process the notification (retry-in-place, R-SA-102).
        watcher.record_processing_failure(&first[0]).await;

        let third = scan_own(&watcher).await;
        assert_eq!(
            third.len(),
            1,
            "a result whose processing failed must be retried on the next scan"
        );
        assert!(!third[0].exhausted, "still well under the retry bound");
    }

    #[tokio::test]
    async fn processing_failure_bound_eventually_marks_the_result_exhausted() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");
        let result = sample_result("run00011", RunState::Complete, true);
        publish_result(&results_dir, &result).await;

        let watcher = ResultsWatcher::new(results_dir);

        // Drive scan -> record_processing_failure in a loop past the bound; the result must
        // eventually be surfaced with `exhausted: true` and never spin forever.
        let mut last_exhausted = false;
        for _ in 0..(MAX_PROCESSING_ATTEMPTS + 5) {
            let found = scan_own(&watcher).await;
            let Some(notification) = found.into_iter().next() else {
                continue;
            };
            last_exhausted = notification.exhausted;
            watcher.record_processing_failure(&notification).await;
        }

        assert!(
            last_exhausted,
            "R-SA-102: processing that fails indefinitely must eventually be reported exhausted, \
             not retried without bound"
        );
    }

    // ---------------------------------------------------------------------------------------
    // A payload that vanishes is REPORTED, never silently skipped
    // ---------------------------------------------------------------------------------------

    /// The exact on-disk state found on this machine nineteen times: index entries present, the
    /// payload unlinked by something index-blind. The enumeration this replaced dropped such a
    /// candidate on every scan, forever, without a word.
    #[tokio::test]
    async fn a_vanished_payload_is_declared_lost_with_its_recovered_output() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir).await.expect("mkdir");

        // A completed run, with the run directory a real runner leaves behind.
        let async_dir = results_dir.parent().expect("parent").join("async/run00500");
        tokio::fs::create_dir_all(&async_dir).await.expect("mkdir run dir");
        let mut status = crate::background::RunStatus::queued(
            RunId::from_token("run00500"),
            crate::background::RunMode::Single,
            Some(1),
        );
        status.state = crate::background::RunState::Complete;
        status.ended_at = Some(1_700_000_000_000);
        let mut step = crate::background::StepStatus::pending("delegate");
        step.telemetry.recent_output = vec!["the child's".to_string(), "actual answer".to_string()];
        status.steps = vec![step];
        crate::background::atomic::write_atomic_json(&async_dir.join("status.json"), &status)
            .await
            .expect("write status");

        let result = sample_result("run00500", RunState::Complete, true);
        crate::background::result_index::write_async_result_file(
            &crate::background::result_index::ResultWrite {
                results_dir: &results_dir,
                session_id: &test_session(),
                run_id: &result.run_id,
                written_at: 1,
                async_dir: Some(&async_dir),
                tool_call_id: None,
            },
            &result,
        )
        .await
        .expect("publish");

        // The theft: the payload is unlinked, its index left behind.
        tokio::fs::remove_file(owned_path(&results_dir, "run00500"))
            .await
            .expect("unlink the payload");

        let watcher = ResultsWatcher::new(results_dir.clone());
        for scan in 1..MISSING_PAYLOAD_GRACE_SCANS {
            let outcome = watcher
                .scan_candidates(&[test_session()], &[])
                .await
                .expect("scan succeeds");
            assert!(
                outcome.missing.is_empty(),
                "scan {scan} is inside the grace window: a payload mid-write must not be \
                 mistaken for a stolen one"
            );
        }

        let outcome = watcher
            .scan_candidates(&[test_session()], &[])
            .await
            .expect("scan succeeds");
        assert_eq!(outcome.missing.len(), 1, "the loss must be reported");
        let report = &outcome.missing[0];
        assert_eq!(report.run_id, RunId::from_token("run00500"));
        assert_eq!(report.agent, "delegate", "named from the run's own status");
        assert_eq!(report.async_dir.as_deref(), Some(async_dir.as_path()));
        assert_eq!(
            report.recovered.len(),
            1,
            "the child's output survives in status.json and must be handed back"
        );
        assert!(report.recovered[0].output.contains("actual answer"));

        // And the notification carries it, rather than merely admitting the loss.
        let message = super::super::message::format_missing_payload_message(report);
        assert!(message.display, "a loss always needs to be seen");
        assert!(message.trigger_turn);
        assert!(message.content.contains("actual answer"), "{}", message.content);
        assert!(
            message.content.contains(&async_dir.display().to_string()),
            "the artifacts pointer must be present: {}",
            message.content
        );

        // Reported exactly once: the streak is cleared when the verdict is returned.
        let again = watcher
            .scan_candidates(&[test_session()], &[])
            .await
            .expect("scan succeeds");
        assert!(
            again.missing.is_empty(),
            "a declared loss must not be re-announced on every later scan"
        );
    }

    // ---------------------------------------------------------------------------------------
    // install: real notify::PollWatcher against a real tempdir
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_observes_a_real_filesystem_write() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");

        let watcher = ResultsWatcher::new(results_dir.clone());
        let (_native_watcher, mut rx) = watcher.install().expect("watcher installs");

        let result = sample_result("run00008", RunState::Complete, true);
        publish_result(&results_dir, &result).await;

        let notified = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;
        assert!(
            notified.is_ok() && notified.expect("timeout checked above").is_some(),
            "a real filesystem write to ResultsDir must be observed by the watcher"
        );
    }

    /// The load-bearing test this task calls for: a real `notify` watcher against a real tempdir,
    /// writing a real [`ResultFile`], asserting the watcher fires exactly once (dedup) even under a
    /// simulated duplicate-event scenario.
    ///
    /// `notify` (and file systems generally) offer no guarantee of exactly-one-event-per-write —
    /// a single atomic `write_atomic_json` (temp-write + rename) can itself surface as more than
    /// one raw OS-level filesystem event, and a `PollWatcher`'s own poll tick can independently
    /// observe the same still-new file across more than one tick before it is deleted. This test
    /// deliberately drains EVERY wake-up the watcher produces within a bounded settle window
    /// (simulating however many duplicate raw events a real backend might coalesce or fail to
    /// coalesce) and, for EACH wake-up, re-scans `results_dir` — proving that no matter how many
    /// raw notify events a single result-file write generates, [`ResultsWatcher::scan`]'s own
    /// seen-set dedup (R-SA-099) still yields the result as a [`CompletionNotification`] EXACTLY
    /// ONCE across the whole sequence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn watcher_fires_exactly_once_under_duplicate_filesystem_events() {
        let (_dir, results_dir) = temp_results_dir();
        tokio::fs::create_dir_all(&results_dir)
            .await
            .expect("mkdir");

        let watcher = ResultsWatcher::new(results_dir.clone());
        let (_native_watcher, mut rx) = watcher.install().expect("watcher installs");

        let result = sample_result("run00099", RunState::Complete, true);
        publish_result(&results_dir, &result).await;

        // Drain every wake-up the watcher produces within a bounded settle window. A real
        // PollWatcher backend may emit more than one raw event for a single logical write (the
        // temp-write, the rename, and/or more than one poll tick observing the same new file
        // before anything consumes it) — that is exactly the "simulated duplicate-event scenario"
        // this test exercises, using genuine OS/filesystem-driven duplication rather than a
        // hand-rolled fake.
        let mut wake_ups = 0u32;
        let mut total_notifications: Vec<ResolvedCandidate> = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Some(())) => {
                    wake_ups += 1;
                    let found = scan_own(&watcher).await;
                    total_notifications.extend(found);
                }
                Ok(None) => break, // channel closed (watcher dropped)
                Err(_) => break,   // settle-window timeout: no more events arriving
            }
        }

        assert!(
            wake_ups >= 1,
            "the watcher must have observed at least one real filesystem event"
        );
        assert_eq!(
            total_notifications.len(),
            1,
            "exactly one CompletionNotification must be produced across the ENTIRE sequence of \
             (possibly duplicate) wake-ups, proving R-SA-099 dedup absorbs duplicate filesystem \
             events rather than re-notifying per raw event: got {total_notifications:?} across \
             {wake_ups} wake-up(s)"
        );
        assert_eq!(total_notifications[0].result.run_id, result.run_id);

        // One more explicit scan (simulating the shared poller's next scheduled tick, independent
        // of any further filesystem event) must still find nothing new — dedup holds even after
        // the event stream has quieted down, not just across the immediate burst.
        let after = scan_own(&watcher).await;
        assert!(
            after.is_empty(),
            "a scan after the event burst has settled must not re-notify"
        );
    }
}
