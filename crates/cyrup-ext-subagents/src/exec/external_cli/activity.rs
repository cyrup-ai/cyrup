//! SUBA-131 — idle detection for an external-CLI step (pi `c7938169` "fix: track external CLI
//! activity", `subagent-runner.ts` @ad11b7ab).
//!
//! A foreign process has no NDJSON event stream, so the native drive loop's per-event
//! [`ControlMonitor::note_activity`] never fires for it. Upstream credits two other kinds of
//! evidence instead, and this module ports both:
//!
//! * **Stream chunks.** Every non-empty stdout/stderr chunk stamps activity
//!   (`onExternalOutput`, `:925-928`, → `recordExternalStreamActivity`, `:2635-2639`) WITHOUT
//!   re-deriving the state — [`ExternalActivityTracker::on_chunk`].
//! * **Git worktree changes.** A silent CLI that is editing files is working. When control is on,
//!   a baseline fingerprint of the cwd's repository is read BEFORE the spawn
//!   (`prepareExternalActivity`, `:2628-2634`, called at `:869-871`). Once the step is already past
//!   the idle window, the 1s tick re-reads it (throttled to one probe per
//!   [`EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS`], single-flight) instead of raising; a CHANGED
//!   fingerprint is fresh activity, an unchanged one raises (`:3178-3205`) —
//!   [`ExternalActivityTracker::on_tick`] / [`ExternalActivityTracker::on_probe_result`].
//!
//! Upstream counts the external step as one turn (`turnCount: … "external-cli" ? 1 : …`, `:3173`)
//! so the `turnCount === 0` idle exemption never applies; see [`ControlMonitor::idle_due`].
//!
//! `[CYRUP-DELTA]`s:
//! * Upstream's `PROCESS_TREE_UNVERIFIED` outcome is NOT ported. `runSetupCommand` raises it
//!   (`worktree-setup-command.ts:106-131`) whenever termination begins after git itself already
//!   exited: git ended by a signal (`status === null` is never an accepted code), or git exited
//!   but a descendant still holds stdout/stderr when the 30s deadline or an abort fires.
//!   `readGitFingerprint` rethrows it (`subagent-runner.ts:653`); a periodic probe then fails the
//!   step with `PROCESS_TREE_UNVERIFIED: <msg>` (`recordPeriodicGitProbeFailure`, `:2609-2627`),
//!   and the baseline read throws out of `prepareExternalActivity`.
//!   [`crate::spawn::bounded_argv::run_bounded_argv`] reaches the same conditions but reports
//!   them as ordinary outcomes: a signal-ended git is a `None` status (here an unexpected
//!   failure), a held pipe at the deadline is [`BoundedError::DeadlineExceeded`] (unexpected), at
//!   an abort [`BoundedError::Aborted`] (silent). Its cleanup is a fire-and-forget group
//!   `SIGKILL`, which a descendant that called `setsid` escapes. So where upstream fails the
//!   step, cyrup warns once (or stays silent), treats the read as "no fingerprint" and, for a
//!   periodic probe, raises `needs_attention`. Porting it would need a `BoundedError` variant for
//!   "leader reaped, tree unverified".
//! * Upstream shares one in-flight probe per cwd across a run's parallel steps (`gitProbesByCwd`,
//!   `:2597-2603`). Each cyrup step runs through its own `run_sync`, so the probe is single-flight
//!   per STEP only. Efficiency only: the answers are the same.
//! * The once-per-cwd warning drops upstream's `[pi-subagents]` prefix and goes to `tracing`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cyrup_core::CancelToken;

use crate::exec::control::{ControlEvent, ControlMonitor};
use crate::spawn::bounded_argv::{BoundedError, Bounds, run_bounded_argv};

/// pi `EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS` (`subagent-runner.ts:2584` @ad11b7ab).
pub(crate) const EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS: i64 = 2_000;

/// pi `deadlineAt: Date.now() + 30_000` per git command (`subagent-runner.ts:638` @ad11b7ab).
const GIT_PROBE_TIMEOUT: Duration = Duration::from_secs(30);

/// pi `maxBuffer: 16 * 1024 * 1024` (`subagent-runner.ts:638` @ad11b7ab).
const GIT_PROBE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// One fingerprint read: pi `readGitFingerprint`'s `string | undefined` plus the error it would
/// have handed `onError` (`subagent-runner.ts:634-657` @ad11b7ab).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FingerprintRead {
    /// `HEAD` + NUL + the raw `git status --porcelain=v1 -z` bytes; `None` when there is none.
    /// Upstream base64-encodes the status half; only equality is ever used, so the raw bytes are
    /// the same answer.
    pub(crate) fingerprint: Option<Vec<u8>>,
    /// The failure upstream reports through `onError`: `None` for a success, an abort, or the
    /// expected "not a repository / no commit yet" answers (`expectedMissingGitEvidence`,
    /// `:627-632`).
    pub(crate) unexpected_error: Option<String>,
}

enum GitFailure {
    /// Silent: aborted, or the expected missing-evidence answer.
    Silent,
    Unexpected(String),
}

/// pi `expectedMissingGitEvidence` (`subagent-runner.ts:627-632` @ad11b7ab): a numeric exit whose
/// stderr matches `/not a git repository|Needed a single revision/i`.
fn expected_missing_git_evidence(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("not a git repository") || lower.contains("needed a single revision")
}

async fn run_git(cwd: &Path, args: &[&str], bounds: &Bounds) -> Result<Vec<u8>, GitFailure> {
    match run_bounded_argv(OsStr::new("git"), args, cwd, &[], None, bounds).await {
        Ok(output) if output.status == Some(0) => Ok(output.stdout),
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            // A signal-ended git has no numeric `code` upstream, so it is never "expected".
            if output.status.is_some() && expected_missing_git_evidence(&stderr) {
                return Err(GitFailure::Silent);
            }
            Err(GitFailure::Unexpected(if stderr.is_empty() {
                format!(
                    "git exited with {}",
                    output
                        .status
                        .map_or_else(|| "null".to_string(), |code| code.to_string())
                )
            } else {
                stderr
            }))
        }
        Err(BoundedError::Aborted) => Err(GitFailure::Silent),
        Err(error) => Err(GitFailure::Unexpected(error.to_string())),
    }
}

/// pi `readGitFingerprint` (`subagent-runner.ts:634-657` @ad11b7ab): `git rev-parse --verify HEAD`
/// then `git status --porcelain=v1 -z --untracked-files=normal`, each bounded (owned process group,
/// 16 MiB, 30s, cancellable). `deadline` is the run's own deadline — upstream's baseline read runs
/// under `combinedAbortSignal([timeoutSignal, stopSignal])` — and any failure once `cancel` fired
/// or `deadline` passed is silent (`if (!signal.aborted && …) onError(error)`).
///
/// Owned arguments so the future is `'static` and can sit in the run loop's probe slot.
pub(crate) async fn read_git_fingerprint(
    cwd: PathBuf,
    cancel: CancelToken,
    deadline: Option<Instant>,
) -> FingerprintRead {
    let cap = Instant::now() + GIT_PROBE_TIMEOUT;
    let bounds = Bounds {
        cancel: Some(cancel.clone()),
        deadline: Some(deadline.map_or(cap, |deadline| deadline.min(cap))),
        max_bytes: GIT_PROBE_MAX_BYTES,
    };
    let read = async {
        let head = run_git(&cwd, &["rev-parse", "--verify", "HEAD"], &bounds).await?;
        let head = String::from_utf8_lossy(&head).trim().to_string();
        if head.is_empty() {
            return Ok(None);
        }
        let status = run_git(
            &cwd,
            &["status", "--porcelain=v1", "-z", "--untracked-files=normal"],
            &bounds,
        )
        .await?;
        let mut fingerprint = head.into_bytes();
        fingerprint.push(0);
        fingerprint.extend_from_slice(&status);
        Ok(Some(fingerprint))
    };
    match read.await {
        Ok(fingerprint) => FingerprintRead {
            fingerprint,
            unexpected_error: None,
        },
        Err(GitFailure::Unexpected(error))
            if !cancel.is_cancelled() && deadline.is_none_or(|at| Instant::now() < at) =>
        {
            FingerprintRead {
                fingerprint: None,
                unexpected_error: Some(error),
            }
        }
        Err(_) => FingerprintRead::default(),
    }
}

/// pi `ExternalActivityEvidence`'s git half (`subagent-runner.ts:2585-2591` @ad11b7ab).
#[derive(Debug)]
struct GitEvidence {
    cwd: PathBuf,
    baseline: Vec<u8>,
    last_probe_started_at: i64,
    probe_in_flight: bool,
}

/// What one activity tick decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TickAction {
    /// A probe is in flight: evaluate nothing this tick (upstream's `continue`, `:3181`).
    Wait,
    /// Start a fingerprint probe of `cwd`; its result goes to
    /// [`ExternalActivityTracker::on_probe_result`].
    StartProbe {
        /// The step's resolved cwd.
        cwd: PathBuf,
    },
    /// The monitor re-derived the state; `true` when a fresh notice was raised.
    Evaluated(bool),
}

/// One external-CLI step's control state: the [`ControlMonitor`] the native path also uses, plus
/// the git evidence upstream keeps per step.
#[derive(Debug)]
pub(crate) struct ExternalActivityTracker {
    monitor: ControlMonitor,
    git: Option<GitEvidence>,
    /// pi `reportedGitProbeErrors` (`:2595-2600`), scoped to this step's single cwd.
    warned: bool,
    cwd_label: String,
}

impl ExternalActivityTracker {
    /// pi `prepareExternalActivity` (`subagent-runner.ts:2628-2634` @ad11b7ab): with control off,
    /// no git runs at all; otherwise stamp the probe clock BEFORE the baseline read, then await it.
    /// `now` is the epoch-ms clock the tick also uses.
    pub(crate) async fn prepare(
        monitor: ControlMonitor,
        cwd: &Path,
        cancel: &CancelToken,
        deadline: Option<Instant>,
        now: i64,
    ) -> Self {
        // `path.resolve(externalCwd)`.
        let resolved = std::path::absolute(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        let mut tracker = Self {
            monitor,
            git: None,
            warned: false,
            cwd_label: resolved.display().to_string(),
        };
        if !tracker.monitor.enabled() {
            return tracker;
        }
        let read = read_git_fingerprint(resolved.clone(), cancel.clone(), deadline).await;
        if let Some(error) = read.unexpected_error.as_deref() {
            tracker.warn_once(error);
        }
        tracker.git = read.fingerprint.map(|baseline| GitEvidence {
            cwd: resolved,
            baseline,
            last_probe_started_at: now,
            probe_in_flight: false,
        });
        tracker
    }

    /// Whether control tracking is on (and so whether the run loop needs a tick at all).
    pub(crate) fn enabled(&self) -> bool {
        self.monitor.enabled()
    }

    /// pi `onExternalOutput` (`subagent-runner.ts:925-928` @ad11b7ab): a non-empty chunk on either
    /// stream is activity. Stamps only; the tick re-derives.
    pub(crate) fn on_chunk(&mut self, len: usize, now: i64) {
        if len > 0 {
            self.monitor.touch_activity(now);
        }
    }

    /// The external-step half of `updateRunnerActivityState` (`subagent-runner.ts:3178-3220`
    /// @ad11b7ab). Past the idle window with a baseline: wait on an in-flight probe, or start one
    /// when the last started at least [`EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS`] ago. Otherwise —
    /// not idle, no baseline, or probed too recently — the ordinary derivation runs (and raises).
    pub(crate) fn on_tick(&mut self, now: i64) -> TickAction {
        if self.monitor.idle_due(now)
            && let Some(git) = self.git.as_mut()
        {
            if git.probe_in_flight {
                return TickAction::Wait;
            }
            if now - git.last_probe_started_at >= EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS {
                git.last_probe_started_at = now;
                git.probe_in_flight = true;
                return TickAction::StartProbe {
                    cwd: git.cwd.clone(),
                };
            }
        }
        TickAction::Evaluated(self.monitor.update_activity_state(now))
    }

    /// The probe's `.then` (`subagent-runner.ts:3186-3197` @ad11b7ab): a fingerprint that differs
    /// from the baseline becomes the new baseline and is fresh activity; anything else re-derives
    /// WITHOUT probing (`skipExternalProbeIndex`), which raises. Returns `true` when a fresh notice
    /// was raised. The caller drops the result instead once the step has settled (`status !==
    /// "running"`).
    pub(crate) fn on_probe_result(&mut self, now: i64, read: FingerprintRead) -> bool {
        if let Some(git) = self.git.as_mut() {
            git.probe_in_flight = false;
        }
        if let Some(error) = read.unexpected_error.as_deref() {
            self.warn_once(error);
        }
        if let (Some(git), Some(fingerprint)) = (self.git.as_mut(), read.fingerprint)
            && fingerprint != git.baseline
        {
            git.baseline = fingerprint;
            self.monitor.touch_activity(now);
        }
        self.monitor.update_activity_state(now)
    }

    /// pi `reportGitProbeError` (`subagent-runner.ts:2595-2600` @ad11b7ab): once per cwd. `true`
    /// when this call logged.
    fn warn_once(&mut self, error: &str) -> bool {
        if self.warned {
            return false;
        }
        self.warned = true;
        let label = tail_chars(&self.cwd_label, 300);
        let detail: String = error.chars().take(500).collect();
        tracing::warn!(
            "Git activity evidence unavailable for '{label}'; check the cwd and Git installation: {detail}"
        );
        true
    }

    /// The raised events, for [`crate::exec::SingleResult::control_events`].
    pub(crate) fn into_events(self) -> Vec<ControlEvent> {
        self.monitor.into_events()
    }

    #[cfg(test)]
    fn has_baseline(&self) -> bool {
        self.git.is_some()
    }
}

/// `value.slice(-n)`, on chars.
fn tail_chars(value: &str, n: usize) -> &str {
    let count = value.chars().count();
    if count <= n {
        return value;
    }
    value
        .char_indices()
        .nth(count - n)
        .map_or(value, |(at, _)| value.get(at..).unwrap_or(value))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use crate::exec::control::{ControlEventReason, ResolvedControlConfig};
    use crate::registration::ControlEventType;

    const T0: i64 = 1_000_000;
    const THRESHOLD: i64 = 1_500;

    fn monitor(enabled: bool) -> ControlMonitor {
        ControlMonitor::new(
            ResolvedControlConfig {
                enabled,
                needs_attention_after_ms: THRESHOLD,
                ..ResolvedControlConfig::default()
            },
            "run".to_string(),
            "ext".to_string(),
            Some(0),
            None,
            T0,
        )
    }

    fn tracker(git_started_at: Option<i64>) -> ExternalActivityTracker {
        ExternalActivityTracker {
            monitor: monitor(true),
            git: git_started_at.map(|at| GitEvidence {
                cwd: PathBuf::from("/repo"),
                baseline: b"base".to_vec(),
                last_probe_started_at: at,
                probe_in_flight: false,
            }),
            warned: false,
            cwd_label: "/repo".to_string(),
        }
    }

    fn idle_events(tracker: &ExternalActivityTracker) -> usize {
        tracker
            .monitor
            .events()
            .iter()
            .filter(|event| {
                event.event_type == ControlEventType::NeedsAttention
                    && event.reason == Some(ControlEventReason::Idle)
            })
            .count()
    }

    fn read(fingerprint: Option<&[u8]>, error: Option<&str>) -> FingerprintRead {
        FingerprintRead {
            fingerprint: fingerprint.map(<[u8]>::to_vec),
            unexpected_error: error.map(str::to_string),
        }
    }

    /// T3(a) — an empty chunk is not activity, a non-empty one is, and the idle window is a
    /// strict `>` (`subagent-control.ts:111`).
    #[test]
    fn chunks_refresh_activity_and_the_idle_window_is_strict() {
        let mut tracker = tracker(None);
        tracker.on_chunk(0, T0 + 1_000);
        assert!(tracker.monitor.idle_due(T0 + THRESHOLD + 1));
        tracker.on_chunk(5, T0 + 1_000);
        assert!(!tracker.monitor.idle_due(T0 + 1_000 + THRESHOLD));
        assert!(tracker.monitor.idle_due(T0 + 1_000 + THRESHOLD + 1));
        // A chunk stamps only: it never raises by itself.
        assert_eq!(idle_events(&tracker), 0);
        assert_eq!(
            tracker.on_tick(T0 + 1_000 + THRESHOLD),
            TickAction::Evaluated(false)
        );
        assert_eq!(
            tracker.on_tick(T0 + 1_000 + THRESHOLD + 1),
            TickAction::Evaluated(true)
        );
        assert_eq!(idle_events(&tracker), 1);
    }

    /// T3(b) — idle before the first probe interval has elapsed since the baseline started: the
    /// ordinary rule raises, with no probe (upstream fall-through).
    #[test]
    fn an_idle_step_raises_without_probing_inside_the_probe_interval() {
        // The baseline started late, so at the idle boundary < 2000ms have passed since.
        // A literal, not the constant, so a changed interval is caught here.
        let started = T0 + THRESHOLD + 1 - 1_999;
        let mut tracker = tracker(Some(started));
        assert_eq!(
            tracker.on_tick(T0 + THRESHOLD + 1),
            TickAction::Evaluated(true)
        );
        assert_eq!(idle_events(&tracker), 1);
    }

    /// T3(c) — at exactly the interval a probe starts instead of a raise, and while it is in
    /// flight every tick waits without evaluating.
    #[test]
    fn an_idle_step_with_a_baseline_probes_and_waits_on_the_probe() {
        let now = T0 + THRESHOLD + 1;
        let mut tracker = tracker(Some(now - EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS));
        assert_eq!(
            tracker.on_tick(now),
            TickAction::StartProbe {
                cwd: PathBuf::from("/repo")
            }
        );
        assert_eq!(tracker.on_tick(now + 1_000), TickAction::Wait);
        assert_eq!(tracker.on_tick(now + 5_000), TickAction::Wait);
        assert_eq!(idle_events(&tracker), 0);
    }

    /// T3(d) — a changed fingerprint is activity (no raise, idle reset, new baseline); the same
    /// fingerprint, or none, raises.
    #[test]
    fn a_probe_result_credits_only_a_changed_fingerprint() {
        let now = T0 + THRESHOLD + 1;
        let mut changed = tracker(Some(now - EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS));
        assert!(matches!(
            changed.on_tick(now),
            TickAction::StartProbe { .. }
        ));
        assert!(!changed.on_probe_result(now, read(Some(b"new"), None)));
        assert_eq!(idle_events(&changed), 0);
        assert!(!changed.monitor.idle_due(now + THRESHOLD));
        assert_eq!(changed.git.as_ref().unwrap().baseline, b"new".to_vec());
        // The SAME change seen again is not credited twice.
        let later = now + EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS;
        assert!(matches!(
            changed.on_tick(later),
            TickAction::StartProbe { .. }
        ));
        assert!(changed.on_probe_result(later, read(Some(b"new"), None)));
        assert_eq!(idle_events(&changed), 1);

        let mut same = tracker(Some(now - EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS));
        assert!(matches!(same.on_tick(now), TickAction::StartProbe { .. }));
        assert!(same.on_probe_result(now, read(Some(b"base"), None)));
        assert_eq!(idle_events(&same), 1);

        let mut none = tracker(Some(now - EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS));
        assert!(matches!(none.on_tick(now), TickAction::StartProbe { .. }));
        assert!(none.on_probe_result(now, read(None, None)));
        assert_eq!(idle_events(&none), 1);
        // The in-flight flag clears, so the next due tick may probe again.
        assert!(matches!(
            none.on_tick(now + EXTERNAL_GIT_PROBE_MIN_INTERVAL_MS),
            TickAction::StartProbe { .. }
        ));
    }

    /// T3(e)/T7 — control off: no git baseline is read (even in a real repository), no probe is
    /// ever started and nothing is raised.
    #[tokio::test]
    async fn a_disabled_monitor_reads_no_baseline_and_never_raises() {
        let repo = git_repo();
        let mut tracker = ExternalActivityTracker::prepare(
            monitor(false),
            repo.path(),
            &CancelToken::new(),
            None,
            T0,
        )
        .await;
        assert!(!tracker.has_baseline());
        assert_eq!(
            tracker.on_tick(T0 + 100 * THRESHOLD),
            TickAction::Evaluated(false)
        );
        assert!(tracker.into_events().is_empty());
    }

    /// T3(f) — the probe failure warning is reported once per step.
    #[test]
    fn the_probe_failure_warning_fires_once() {
        let mut tracker = tracker(Some(T0));
        assert!(!tracker.warned);
        let _ = tracker.on_tick(T0 + THRESHOLD + 1);
        tracker.on_probe_result(T0 + THRESHOLD + 1, read(None, Some("boom")));
        assert!(tracker.warned);
        tracker.on_probe_result(T0 + THRESHOLD + 2, read(None, Some("boom again")));
        assert!(
            !tracker.warn_once("and again"),
            "a second report must stay silent"
        );
        assert_eq!(tail_chars("abcdef", 3), "def");
        assert_eq!(tail_chars("ab", 3), "ab");
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args([
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(status.status.success(), "git {args:?}: {status:?}");
    }

    fn git_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        std::fs::write(dir.path().join("tracked.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "tracked.txt"]);
        git(dir.path(), &["commit", "-q", "-m", "init"]);
        dir
    }

    async fn fingerprint(dir: &Path) -> FingerprintRead {
        read_git_fingerprint(dir.to_path_buf(), CancelToken::new(), None).await
    }

    /// T4 — the fingerprint moves on every new porcelain answer (untracked file, tracked edit,
    /// commit), stays put when nothing changes, and does NOT move on a second content-only edit of
    /// an already-modified file (porcelain encodes status, not contents — upstream's
    /// "credits a newly observed external Git worktree change once").
    #[tokio::test]
    async fn the_git_fingerprint_tracks_porcelain_status_and_head() {
        let repo = git_repo();
        let dir = repo.path();
        let base = fingerprint(dir).await;
        assert!(base.unexpected_error.is_none());
        let base = base
            .fingerprint
            .expect("a committed repo has a fingerprint");
        assert_eq!(fingerprint(dir).await.fingerprint, Some(base.clone()));

        std::fs::write(dir.join("new.txt"), "x").unwrap();
        let untracked = fingerprint(dir).await.fingerprint.unwrap();
        assert_ne!(untracked, base);

        std::fs::write(dir.join("tracked.txt"), "two\n").unwrap();
        let edited = fingerprint(dir).await.fingerprint.unwrap();
        assert_ne!(edited, untracked);

        std::fs::write(dir.join("tracked.txt"), "three\n").unwrap();
        assert_eq!(fingerprint(dir).await.fingerprint.unwrap(), edited);

        // A commit that leaves the SAME porcelain answer still moves HEAD.
        git(dir, &["commit", "-q", "--allow-empty", "-m", "empty"]);
        let committed = fingerprint(dir).await.fingerprint.unwrap();
        assert_ne!(committed, edited);
    }

    /// T4 — a directory outside any repository is the EXPECTED missing evidence (no warning); a
    /// cwd that does not exist is an unexpected failure.
    #[tokio::test]
    async fn missing_git_evidence_is_silent_but_a_missing_cwd_is_reported() {
        let plain = tempfile::tempdir().unwrap();
        // A tempdir under the system temp root is outside any repository in practice; if it were
        // not, this assertion would say so rather than pass vacuously.
        assert_eq!(fingerprint(plain.path()).await, FingerprintRead::default());

        let missing = plain.path().join("does-not-exist");
        let read = fingerprint(&missing).await;
        assert!(read.fingerprint.is_none());
        assert!(read.unexpected_error.is_some());
    }

    /// T8 — a stop that fired before the baseline read leaves no baseline (the read is aborted,
    /// silently).
    #[tokio::test]
    async fn a_cancelled_baseline_read_is_silent_and_leaves_no_baseline() {
        let repo = git_repo();
        let cancel = CancelToken::new();
        cancel.cancel();
        let tracker =
            ExternalActivityTracker::prepare(monitor(true), repo.path(), &cancel, None, T0).await;
        assert!(!tracker.has_baseline());
        assert!(!tracker.warned);
    }
}
