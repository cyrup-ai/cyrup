//! Shared async-root / results-dir derivation + ensureAccessibleDir-equivalent (C7)
//!
//! C7 root cause: the orchestrator (`extension.rs`) and the detached runner
//! (`crates/cyrup/src/subagent_runner_cmd.rs`) each derived the run's `ResultsDir` independently and
//! arrived at DIFFERENT directories, so every real background run's terminal `ResultFile` write
//! targeted a directory the orchestrator never created (and never watched) — the run appeared to
//! hang forever from the orchestrator's point of view. This section is the single shared source of
//! truth both sides now agree on: the orchestrator derives the two roots here, creates them, and
//! bakes their ABSOLUTE paths into `RunnerConfig` (`runner_main::RunnerConfig::async_root`/
//! `results_dir`); the runner then rebuilds its `RunPaths` from those exact absolute roots rather
//! than re-deriving them from the config-file path's own directory structure. Mirrors pi, where the
//! orchestrator computes `resultPath`/`asyncDir` and passes them verbatim in the runner config
//! (`async-execution.ts:701,966` @v0.34.0) and the runner reads them straight back
//! (`subagent-runner.ts:1316` @v0.34.0) — never re-deriving `RESULTS_DIR`.
//!
//! Split out of `background/mod.rs` behind its private-module facade (same pattern as
//! `runner_main/`): every public item here is re-exported at [`crate::background`], so consumer
//! paths are unchanged.

use std::path::{Path, PathBuf};

/// Path segment, under [`temp_root_dir`], holding one directory per background run (each
/// run's `status.json`, `events.jsonl`, control inbox, logs — everything EXCEPT the terminal
/// [`ResultFile`](crate::background::ResultFile)). pi's `ASYNC_DIR` leaf is `"async-subagent-runs"`
/// (`shared/types.ts:1863` @v0.43.0); cyrup shortens it because the `<cwd_key>` level below it
/// already disambiguates, and `results_dir_for_async_root` pins the two leaves against each other.
const ASYNC_SUBDIR: &str = "async";

/// Path segment, under [`temp_root_dir`], holding the terminal [`ResultFile`](crate::background::ResultFile) for every
/// run (a flat `<run_id>.json` per finished run). A DELIBERATE SIBLING of [`ASYNC_SUBDIR`], never a
/// child of it — "presence in this dir is the authoritative done signal" (R-SA-077) only works if
/// the results dir can be watched independently of the still-being-written run dir. Mirrors pi's
/// `RESULTS_DIR` leaf, `"async-subagent-results"` (`shared/types.ts:1862` @v0.43.0), shortened
/// for the same reason as [`ASYNC_SUBDIR`].
const RESULTS_SUBDIR: &str = "results";

/// Path segment, under [`temp_root_dir`], holding the per-`cwd` directory `exec::run_sync` tees
/// each spawn attempt's raw child stdout into (`attempt-<n>.jsonl`) and parks a run's
/// structured-output capture file in. A third sibling of [`ASYNC_SUBDIR`]/[`RESULTS_SUBDIR`]
/// (and of `crate::artifacts`' `artifacts`/`chain-runs` leaves) under the ONE run-scratch root.
///
/// SUBA-072: this tree used to be `<cwd>/.cyrup-subagent-scratch` — the only run-scratch path in
/// the crate rooted in the PROJECT working tree. pi never writes per-spawn scratch there: every
/// per-spawn file it creates goes under `os.tmpdir()` (`runs/shared/pi-args.ts:787`, `:802`,
/// `:826`, `:841`, `:855` @v0.64.0, `fs.mkdtempSync(path.join(os.tmpdir(), "pi-subagent-"))`),
/// and every persisted run tree hangs off `TEMP_ROOT_DIR` (`shared/types.ts:2689-2695` @v0.64.0).
const SCRATCH_SUBDIR: &str = "scratch";

/// Path segment, under [`temp_root_dir`], holding one `<token>.json` per armed durable wait
/// subscription ([`crate::background::wait_subscriptions`]). A FOURTH sibling of
/// [`ASYNC_SUBDIR`]/[`RESULTS_SUBDIR`]/[`SCRATCH_SUBDIR`], keyed by the same [`cwd_key`].
///
/// pi's own directory is `path.join(path.dirname(ASYNC_DIR), "wait-subscriptions")`
/// (`wait-subscriptions.ts:106`), which resolves to `<TEMP_ROOT_DIR>/wait-subscriptions` because
/// pi's `ASYNC_DIR` is one FLAT, non-cwd-keyed directory (`shared/types.ts:2733`). cyrup's async
/// root carries a `<cwd_key>` level, so the literal `dirname` would resolve to `<scratch>/async` —
/// shared by every cwd and one level too high. This leaf is upstream's *sibling-of-the-async-root*
/// shape re-expressed in cyrup's layering, which `background/wait.rs:87-99` states outright: the
/// cwd partition is the OUTER one, the session filter the inner one.
///
/// Deliberately NOT under [`RESULTS_SUBDIR`]: `spawn_retention_sweep`
/// (`extension/executor/notices.rs:633`) walks the results dir with two reapers, and a live wake
/// registration is not a result.
const SUBSCRIPTIONS_SUBDIR: &str = "wait-subscriptions";

/// Path segment, under [`temp_root_dir`], holding one directory per SESSION's active-async
/// capacity pool ([`crate::background::active_async_capacity`]) — pi `ACTIVE_ASYNC_CAPACITY_DIR`,
/// `path.join(TEMP_ROOT_DIR, "session-active-async-capacity")`
/// (`runs/background/active-async-capacity.ts:10` @v0.66.0, byte-identical at `v0.68.0`).
///
/// # THE ONE ROOT IN THIS CRATE KEYED BY SESSION AND BY NOTHING ELSE
///
/// [`ASYNC_SUBDIR`], [`RESULTS_SUBDIR`], [`SCRATCH_SUBDIR`] and [`SUBSCRIPTIONS_SUBDIR`] are all
/// keyed by [`cwd_key`] — the working directory, and nothing else (see [`RunArtifactRoots`]'s own
/// `# EVERY cyrup instance in a directory resolves these SAME two paths`). This one is keyed by
/// [`crate::identity::SessionId`] instead, and the two consequences are exactly the inverse of
/// that block's:
///
/// * two cyrup instances running in the SAME working directory hold **different** pools, which is
///   the whole point of the cap being per session — one instance's fan-out cannot starve another
///   instance sharing the directory;
/// * the SAME session opened against two different working directories holds **one** pool, so a
///   single session's runs across two projects compete for one cap.
///
/// A reader who assumes this file's cwd-keying convention will place a capacity path wrong, which
/// is why the convention's own doc block carries the reciprocal pointer back here.
const CAPACITY_SUBDIR: &str = "session-active-async-capacity";

/// One segment of a temp-scope id, with every character outside the keep-set — ASCII
/// alphanumerics plus `.`, `_` and `-`, i.e. [`crate::workflows::WorkflowKey`]'s alphabet —
/// collapsed to a single `-` and leading/trailing `-` stripped; an empty result becomes
/// `"unknown"`.
///
/// 1:1 with pi's `sanitizeTempScopeSegment` (`shared/types.ts:1807-1812` @v0.43.0): trim, replace
/// every RUN of characters outside the keep-set with one `-` (the `+` quantifier on upstream's
/// negated character class is why a run collapses to ONE `-` rather than one per character), then
/// strip leading/trailing `-` and fall back to `"unknown"`. A SANITIZER, not the key grammar — it
/// truncates nothing and validates nothing, so it deliberately does not delegate to
/// `WorkflowKey::parse`.
fn sanitize_temp_scope_segment(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_dash = false;
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
            if pending_dash {
                out.push('-');
                pending_dash = false;
            }
            out.push(ch);
        } else {
            // Collapse a run of illegal characters to a single `-`, emitted lazily so a trailing
            // run never lands (matching the `replace(/-+$/g, "")` that follows upstream).
            pending_dash = !out.is_empty();
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

/// The per-user scope segment that keeps two users' subagent scratch trees from colliding inside a
/// world-writable OS temp dir.
///
/// 1:1 with pi's `resolveTempScopeId` (`shared/types.ts:1814-1857` @v0.43.0), in its own precedence
/// order: the real uid (`uid-<n>`) first, then the first non-empty of `USERNAME`/`USER`/`LOGNAME`
/// (`user-<name>`), then the OS user info (`user-<name>`), then `USERPROFILE`/`HOME`
/// (`home-<path>`), then the OS home dir (`home-<path>`), and finally the literal `"shared"`.
///
/// [CYRUP-DELTA] cyrup stops at the uid branch on Unix and at the env branches elsewhere: pi's
/// `os.userInfo()` step exists because Node's `process.getuid` is undefined on Windows, and the
/// stdlib exposes no portable `userInfo` equivalent. Every branch upstream can actually reach on a
/// platform cyrup supports is present, so the resolved value is identical.
fn resolve_temp_scope_id() -> &'static str {
    /// The scope id cannot change within a process (the uid cannot), and [`temp_root_dir`] is on
    /// the path of every run-root derivation — without this each one re-read `/proc/self/status`.
    /// Matches upstream's own once-per-process evaluation: pi's `TEMP_ROOT_DIR` is a module-level
    /// `const`, so `resolveTempScopeId()` runs exactly once per process there too.
    static SCOPE_ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SCOPE_ID.get_or_init(resolve_temp_scope_id_uncached)
}

fn resolve_temp_scope_id_uncached() -> String {
    // pi `if (typeof getuid === "function") return `uid-${getuid()}`` — `process.getuid` is defined
    // on every Unix, so upstream never reaches a later branch there. Read from procfs because this
    // crate is `#![forbid(unsafe_code)]` and cannot call `libc::getuid`.
    if let Some(uid) = real_uid() {
        return format!("uid-{uid}");
    }
    // pi's second branch: the first non-empty of USERNAME/USER/LOGNAME, in that order.
    for key in ["USERNAME", "USER", "LOGNAME"] {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return format!(
                "user-{}",
                sanitize_temp_scope_segment(&value.to_string_lossy())
            );
        }
    }
    // pi's fourth branch (`os.userInfo()`, its third, has no safe stdlib equivalent):
    // `env.USERPROFILE ?? env.HOME`.
    for key in ["USERPROFILE", "HOME"] {
        if let Some(value) = std::env::var_os(key).filter(|v| !v.is_empty()) {
            return format!(
                "home-{}",
                sanitize_temp_scope_segment(&value.to_string_lossy())
            );
        }
    }
    // pi's last resort, verbatim.
    "shared".to_string()
}

/// The calling process's REAL uid (not the effective one), or `None` where it cannot be read
/// without `unsafe`.
///
/// `/proc/self/status`'s `Uid:` line is `Uid:\t<real>\t<effective>\t<saved>\t<fs>`; field 1 is the
/// real uid, which is what `process.getuid()` returns. Returns `None` on a platform with no procfs
/// (macOS, the BSDs, Windows), where [`resolve_temp_scope_id`] falls through to pi's own next
/// branch rather than inventing a constant that would collide across users.
fn real_uid() -> Option<String> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("Uid:"))?;
    let real = line.split_whitespace().nth(1)?;
    if real.bytes().all(|b| b.is_ascii_digit()) {
        Some(real.to_string())
    } else {
        None
    }
}

/// The per-user root every subagent run-artifact directory hangs off:
/// `<os-temp-dir>/cyrup-subagents-<scope>`.
///
/// # This is a scratch root in the OS temp dir, NOT the user's home
///
/// pi puts all four of its run-scratch roots under `os.tmpdir()`:
///
/// ```text
/// TEMP_ROOT_DIR      = path.join(os.tmpdir(), `pi-subagents-${resolveTempScopeId()}`)
/// RESULTS_DIR        = path.join(TEMP_ROOT_DIR, "async-subagent-results")
/// ASYNC_DIR          = path.join(TEMP_ROOT_DIR, "async-subagent-runs")
/// CHAIN_RUNS_DIR     = path.join(TEMP_ROOT_DIR, "chain-runs")
/// TEMP_ARTIFACTS_DIR = path.join(TEMP_ROOT_DIR, "artifacts")
/// ```
///
/// (`shared/types.ts:1862-1866` @v0.43.0; byte-identical at the ported baseline —
/// `shared/types.ts:1097-1101` @v0.33.0 and `:1104-1108` @v0.34.0.)
///
/// This port previously resolved `<CYRUP_HOME|HOME>/.cyrup/subagents` instead, which is where the
/// 59,321-file / 551 MB pile in a developer's real `~/.cyrup/subagents` came from: run scratch that
/// upstream treats as reboot-disposable was being written into permanent user config, per-`cwd`
/// keyed, with nothing ever sweeping it. The two doc citations that justified the old layout were
/// both wrong — `shared/types.ts:958` and `:959` are fields of a run-input interface, not the
/// `RESULTS_DIR`/`ASYNC_DIR` constants.
///
/// `std::env::temp_dir()` is the exact analog of Node's `os.tmpdir()`: both read `TMPDIR` and both
/// fall back to `/tmp`. That is also the ONLY sandbox seam either side has — pi's `DIRS` are
/// module-level constants, so upstream scopes tests by passing explicit `asyncDirRoot`/`resultsDir`
/// options (`async-job-tracker.ts:57`, `async-resume.ts:385`, `fleet-view.ts:326` @v0.43.0) rather
/// than by moving this root.
///
/// # `CYRUP_HOME` is the sandbox seam, and only that
///
/// When `CYRUP_HOME` is set the whole tree relocates to `<CYRUP_HOME>/.cyrup/subagents`. That var
/// is cyrup-original — pi has no `PI_HOME` — so there is no upstream behaviour to diverge from, and
/// its meaning here is the same as in the crate's five other resolvers: "the root every cyrup path
/// resolves against". No production code path in this workspace sets it (`grep -rn CYRUP_HOME
/// crates/*/src` finds only resolvers and docs), so with it unset — its only state outside tests —
/// this function is pi's `TEMP_ROOT_DIR` exactly.
///
/// It earns its place because it is the ONE knob the crate's 19 already-`CYRUP_HOME`-sandboxed
/// integration tests set: honouring it here is what keeps their `TempDir` isolation covering the
/// run-scratch tree instead of letting them pile into the shared real temp root. Upstream's own
/// equivalent is passing explicit `asyncDirRoot`/`resultsDir` options (`async-job-tracker.ts:57`,
/// `async-resume.ts:385`, `fleet-view.ts:326` @v0.43.0) — pi's `DIRS` are module-level constants
/// that cannot be re-scoped by env at all.
///
/// `pub(crate)` so the artifacts/chain-runs housekeeping ([`crate::artifacts`]) can scope its own
/// per-`cwd` roots under the SAME temp root the async/results roots use, rather than re-deriving
/// (and risking drift from) this one resolution.
pub(crate) fn temp_root_dir() -> PathBuf {
    temp_root_dir_from(&|key| std::env::var_os(key), std::env::temp_dir())
}

/// The pure core of [`temp_root_dir`], with the two ambient inputs — the environment and the OS
/// temp dir — passed in, so both branches are provable without mutating process-global state.
/// Follows the crate's existing `native_supervisor::intercom_agent_dir_from` convention.
///
/// # The missing `HOME` rung is deliberate — do not route this through `paths::home_dir`
///
/// `paths::home_dir` falls `CYRUP_HOME` -> `HOME` -> temp. This resolver stops at `CYRUP_HOME`:
/// with it unset the answer is a per-user TEMP directory, never `$HOME`. Sharing that ladder would
/// send this reboot-disposable run scratch into the developer's real home the moment `CYRUP_HOME`
/// is absent — which is its state everywhere except the sandboxed tests. The blank-string filter is
/// load-bearing for the same reason: `CYRUP_HOME=""` must fall through, because
/// `PathBuf::from("").join(".cyrup")` is the RELATIVE path `.cyrup/subagents`, which would root the
/// whole run-scratch tree at the process working directory.
pub(crate) fn temp_root_dir_from(
    env: crate::paths::EnvLookup<'_>,
    os_temp_dir: PathBuf,
) -> PathBuf {
    // The blank filter is the shared one (`cyrup_config::paths` applies the identical rule to every
    // rung of the home ladder): a set-but-blank value is unset, because `PathBuf::from("")` is the
    // RELATIVE empty path and would root this whole tree at the process working directory.
    if let Some(sandbox) = env(cyrup_config::paths::ENV_HOME)
        .filter(|v| !v.to_str().is_some_and(|s| s.trim().is_empty()))
    {
        return PathBuf::from(sandbox).join(".cyrup").join("subagents");
    }
    os_temp_dir.join(format!("cyrup-subagents-{}", resolve_temp_scope_id()))
}

/// A filesystem-safe key derived from `cwd`, so distinct projects' async/result roots never collide
/// under the shared per-user [`temp_root_dir`] tree.
///
/// [CYRUP-DELTA] pi's `ASYNC_DIR`/`RESULTS_DIR` are FLAT — every project's runs share one directory
/// (`shared/types.ts:1863-1864` @v0.43.0), and a run is disambiguated only by its run id. cyrup
/// interposes this `cwd` key so `resume_tracking`'s `read_dir` over the async root cannot re-adopt
/// a run belonging to a different checkout.
///
/// `pub(crate)` for the same reason as [`temp_root_dir`]: [`crate::artifacts`] keys its
/// artifacts/chain-runs roots by the identical `cwd_key` so a project's artifacts sit beside its
/// async/results dirs under one per-`cwd` scope.
pub(crate) fn cwd_key(cwd: &Path) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The two sibling run-artifact roots for one working directory (C7): the `async_root` holding
/// per-run directories and the `results_dir` holding terminal [`ResultFile`](crate::background::ResultFile)s. Both are always
/// keyed by the same `cwd` so a run's directory and its result file are guaranteed to belong to the
/// same project scope.
///
/// # EVERY cyrup instance in a directory resolves these SAME two paths
///
/// The key is [`cwd_key`] — the working directory, and **nothing else**. Running several cyrup
/// instances in one project is ordinary, not an edge case, and they all share these roots byte for
/// byte. Anything written here is therefore visible to all of them, and anything one of them
/// deletes is gone for all of them.
///
/// This is the premise for the whole of [`crate::background::result_index`] and
/// [`crate::background::delivery`]: results are partitioned by session **inside** `results_dir`
/// and consumption is gated on two identities, precisely because the directory itself provides no
/// separation. A doc comment in `watch/results_watcher.rs` once claimed callers could be "scoped
/// to a single-session `ResultsDir`"; there is no such thing, and that claim is what licensed the
/// accept-everything scan that had every instance consuming and deleting every other instance's
/// results.
///
/// A reader who needs "only my runs" must filter by
/// [`crate::background::RunStatus::session_id`], never by directory.
///
/// There is exactly ONE exception in this file: [`active_async_capacity_root_in`] is keyed by
/// session rather than by [`cwd_key`], because the cap it addresses is per session by definition.
/// Everything else here — async, results, scratch, wait-subscriptions — is keyed as described
/// above.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunArtifactRoots {
    /// `<temp_root_dir>/async/<cwd_key>` — passed as `RunPaths::for_run`'s `async_root`.
    pub async_root: PathBuf,
    /// `<temp_root_dir>/results/<cwd_key>` — passed as `RunPaths::for_run`'s `results_dir`.
    pub results_dir: PathBuf,
}

/// THE single derivation of the per-`cwd` async-root and results-dir that both the orchestrator
/// (at spawn time, `extension.rs`) and the runner (transitively, via the absolute paths this
/// function's output is baked into `RunnerConfig` as) agree on — the fix for C7's divergent
/// derivations. Pure path arithmetic; never touches the filesystem (creation is
/// [`ensure_accessible_dir`]'s job).
#[must_use]
pub fn run_artifact_roots(cwd: &Path) -> RunArtifactRoots {
    run_artifact_roots_in(&crate::paths::Roots::from_env(), cwd)
}

/// [`run_artifact_roots`] against already-resolved roots — the same layout, keyed by the same
/// `cwd`, hanging off [`crate::paths::Roots::run_scratch`].
///
/// Takes a resolved value rather than an `Option<&Path>` meaning "or go read the environment": the
/// optional form put this decision in the callee, where it could be answered differently from the
/// same decision made two frames up.
#[must_use]
pub fn run_artifact_roots_in(roots: &crate::paths::Roots, cwd: &Path) -> RunArtifactRoots {
    let scratch = roots.run_scratch();
    let key = cwd_key(cwd);
    RunArtifactRoots {
        async_root: scratch.join(ASYNC_SUBDIR).join(&key),
        results_dir: scratch.join(RESULTS_SUBDIR).join(&key),
    }
}

/// The per-`cwd` directory [`crate::background::wait_subscriptions`] keeps its armed
/// `<token>.json` records in: `<temp_root_dir>/wait-subscriptions/<cwd_key>`.
///
/// The same arithmetic, against the same resolved [`crate::paths::Roots`] and the same
/// [`cwd_key`], as [`run_artifact_roots_in`] — so a subscription armed by one process is found by
/// the next process to open the same working directory. Pure path arithmetic; creation is the
/// caller's job ([`crate::background::wait_subscriptions::WaitSubscriptionManager::arm`] does it
/// once per arm, mirroring pi's `fs.mkdirSync(subscriptionsDir, { recursive: true })`).
#[must_use]
pub fn wait_subscriptions_dir_in(roots: &crate::paths::Roots, cwd: &Path) -> PathBuf {
    roots
        .run_scratch()
        .join(SUBSCRIPTIONS_SUBDIR)
        .join(cwd_key(cwd))
}

/// The per-SESSION root [`crate::background::active_async_capacity`] keeps its slot pools under:
/// `<temp_root_dir>/session-active-async-capacity`. pi `ACTIVE_ASYNC_CAPACITY_DIR`
/// (`active-async-capacity.ts:10` @v0.66.0).
///
/// The same arithmetic, against the same resolved [`crate::paths::Roots`], as
/// [`run_artifact_roots_in`] and [`wait_subscriptions_dir_in`] — **minus the [`cwd_key`] join**.
/// See [`CAPACITY_SUBDIR`] for why that omission is the feature and not an oversight.
///
/// Takes a resolved [`crate::paths::Roots`] rather than re-reading the environment, for
/// [`run_artifact_roots_in`]'s stated reason: "the optional form put this decision in the callee,
/// where it could be answered differently from the same decision made two frames up". Pure path
/// arithmetic; creation is the caller's job.
#[must_use]
pub fn active_async_capacity_root_in(roots: &crate::paths::Roots) -> PathBuf {
    roots.run_scratch().join(CAPACITY_SUBDIR)
}

/// One session's capacity pool: `<capacity root>/<IndexSegment(session)>` — pi `sessionDir`
/// (`active-async-capacity.ts:95-97` @v0.66.0), whose own key is `sha256(sessionId)`.
///
/// # One key, never the alias fan-out
///
/// The segment comes from [`crate::identity::IndexSegment::encode`], never
/// [`crate::identity::IndexSegment::read_aliases`] — the same single-key discipline
/// `terminal_run_index`'s `session_index_dir` applies, and for a sharper reason here: every owner
/// record in the pool re-states its own `ownerSessionId`, and reconciliation re-verifies it
/// against the session it was asked about, so the single hashed key is safe as an address. Fanning
/// out over aliases would let ONE session hold TWO pools, which would defeat the cap entirely.
///
/// [CYRUP-DELTA] pi hashes the session id with a bare `sha256` hex digest and no encoder
/// (`activeAsyncCapacitySessionKey`, `:91-93`). cyrup routes it through the crate's one path-segment
/// encoder instead, so a session id that is already a safe component stays human-readable on disk
/// and a session id that is a full `.jsonl` PATH still collapses to exactly one component. Nothing
/// cross-implementation reads this tree — unlike the terminal-run index, it is scratch state for
/// live admission decisions only — so byte-compatibility with pi's digest buys nothing, while a
/// second hashing scheme in a crate that already has one costs a reader a wrong assumption.
#[must_use]
pub fn active_async_capacity_session_dir(
    roots: &crate::paths::Roots,
    session_id: &crate::identity::SessionId,
) -> PathBuf {
    active_async_capacity_root_in(roots)
        .join(crate::identity::IndexSegment::encode(session_id.as_str()).as_str())
}

/// The per-`cwd` directory `exec::run_sync` writes its per-attempt raw-stdout tee
/// (`attempt-<n>.jsonl`) and structured-output capture file into:
/// `<temp_root_dir>/scratch/<cwd_key>`.
///
/// SUBA-072: resolved from the SAME run-scratch root and keyed by the SAME [`cwd_key`] as the
/// async/results roots ([`run_artifact_roots`]) and the artifacts/chain-runs roots
/// ([`crate::artifacts`]), so a project's every run-scratch tree lives together under one
/// per-`cwd` scope — never under the project's own working tree. With `CYRUP_HOME` unset (its
/// only production state) that is `<os-temp>/cyrup-subagents-<scope>/scratch/<cwd_key>`, pi's
/// `TEMP_ROOT_DIR` shape (`shared/types.ts:2689-2691` @v0.64.0); pi's own per-spawn scratch is
/// likewise `os.tmpdir()`-rooted (`runs/shared/pi-args.ts:787` @v0.64.0).
///
/// `pub` because it is the crate's stated observation channel: the integration tests in
/// `cyrup-it` read the tee back from exactly this path.
#[must_use]
pub fn attempt_scratch_dir(cwd: &Path) -> PathBuf {
    attempt_scratch_dir_in(&crate::paths::Roots::from_env(), cwd)
}

/// [`attempt_scratch_dir`] against already-resolved roots — the pure core, hanging off
/// [`crate::paths::Roots::run_scratch`] exactly as [`run_artifact_roots_in`] does. Pure path
/// arithmetic; creation is the caller's job (`exec::run_sync` does it once per run).
#[must_use]
pub fn attempt_scratch_dir_in(roots: &crate::paths::Roots, cwd: &Path) -> PathBuf {
    roots.run_scratch().join(SCRATCH_SUBDIR).join(cwd_key(cwd))
}

/// Reconstruct the SIBLING results-dir for an `async_root` produced by [`run_artifact_roots`],
/// purely structurally (no `cwd`/env re-read). Given the standard layout
/// `<temp_root_dir>/async/<cwd_key>`, returns `<temp_root_dir>/results/<cwd_key>` —
/// i.e. it swaps the [`ASYNC_SUBDIR`] path segment for [`RESULTS_SUBDIR`] while PRESERVING the
/// `<cwd_key>` leaf, which is exactly what C7's pre-fix `async_root.parent()/results` derivation got
/// wrong (it dropped the `<cwd_key>` and nested `results` UNDER `async` instead of beside it).
///
/// This exists so the runner's config-path-structure fallback (used ONLY on the pre-config-read
/// error path in `crates/cyrup/src/subagent_runner_cmd.rs`, where no authoritative
/// `RunnerConfig::results_dir` has been read yet) still targets the SAME results dir the
/// orchestrator created. For a non-standard `async_root` that does not match the
/// `<...>/async/<key>` shape (e.g. a bare `<base>/async` used by lower-level unit fixtures), it
/// degrades to a `results` sibling of `async_root`'s own parent.
#[must_use]
pub fn results_dir_for_async_root(async_root: &Path) -> PathBuf {
    let parent = async_root.parent();
    let is_standard_layout = parent
        .and_then(Path::file_name)
        .is_some_and(|name| name == std::ffi::OsStr::new(ASYNC_SUBDIR));
    if is_standard_layout
        && let (Some(home), Some(key)) = (parent.and_then(Path::parent), async_root.file_name())
    {
        return home.join(RESULTS_SUBDIR).join(key);
    }
    async_root
        .parent()
        .unwrap_or(async_root)
        .join(RESULTS_SUBDIR)
}

/// `ensureAccessibleDir`-equivalent (pi `extension/index.ts:97-110`): create `dir` (and every
/// missing parent), then verify it is actually a READ+WRITE-accessible directory. On the rare
/// platform edge pi guards against — a directory created shortly after wake-from-sleep on Windows
/// with Azure AD/Entra ID can end up with a broken null DACL that makes it inaccessible to its own
/// creator — the directory is dropped and recreated once before giving up.
///
/// Called on BOTH sides of C7: the orchestrator ensures `async_root`/`results_dir` at spawn time,
/// and the runner ensures `results_dir` again immediately before the terminal [`ResultFile`](crate::background::ResultFile) write
/// (so the authoritative "done" signal always lands even if the orchestrator's own creation was
/// skipped or the dir was since removed).
///
/// # Errors
///
/// Returns the underlying `io::Error` if the directory cannot be created, or a
/// [`std::io::ErrorKind::PermissionDenied`] error if it still fails the read+write accessibility
/// probe after a recreate attempt.
pub async fn ensure_accessible_dir(dir: &Path) -> std::io::Result<()> {
    tokio::fs::create_dir_all(dir).await?;
    if probe_dir_accessible(dir).await {
        return Ok(());
    }
    // Broken-ACL recovery (Windows Azure-AD null-DACL case): drop and recreate once. A cleanup
    // failure is deliberately best-effort — retry the mkdir/probe regardless, mirroring pi.
    let _ = tokio::fs::remove_dir_all(dir).await;
    tokio::fs::create_dir_all(dir).await?;
    if probe_dir_accessible(dir).await {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "directory is not read/write accessible after recreate: {}",
                dir.display()
            ),
        ))
    }
}

/// Probe `dir` for read+write access the way pi's `fs.accessSync(R_OK | W_OK)` does, but portably:
/// confirm it is a listable directory (read) and that a uniquely-named probe file can be created
/// and removed inside it (write). Any failure returns `false`, which drives
/// [`ensure_accessible_dir`]'s recreate-once recovery.
async fn probe_dir_accessible(dir: &Path) -> bool {
    match tokio::fs::metadata(dir).await {
        Ok(meta) if meta.is_dir() => {}
        _ => return false,
    }
    let probe = dir.join(format!(
        ".cyrup-access-probe-{}",
        uuid::Uuid::new_v4().as_simple()
    ));
    match tokio::fs::write(&probe, b"").await {
        Ok(()) => {
            let _ = tokio::fs::remove_file(&probe).await;
            true
        }
        Err(_) => false,
    }
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

    #[test]
    fn run_artifact_roots_places_results_beside_async_keyed_by_the_same_cwd() {
        // The two roots MUST share a subagents-home and a cwd key, differing only in the
        // async/results segment — the exact invariant C7's divergent derivation broke.
        let a = run_artifact_roots(Path::new("/some/project/a"));
        let b = run_artifact_roots(Path::new("/some/project/b"));

        let key = a.async_root.file_name().expect("async root has a key leaf");
        assert_eq!(
            a.results_dir.file_name(),
            Some(key),
            "both roots must be keyed by the same cwd"
        );
        assert_eq!(
            a.async_root.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new("async"))
        );
        assert_eq!(
            a.results_dir.parent().and_then(Path::file_name),
            Some(std::ffi::OsStr::new("results"))
        );
        assert_eq!(
            a.async_root.parent().and_then(Path::parent),
            a.results_dir.parent().and_then(Path::parent),
            "async/ and results/ must be siblings under one shared subagents home"
        );

        // Distinct cwds get distinct keys, so distinct roots — but the same shared home.
        assert_ne!(a.async_root, b.async_root);
        assert_ne!(a.results_dir, b.results_dir);
        assert_eq!(
            a.async_root.parent().and_then(Path::parent),
            b.async_root.parent().and_then(Path::parent),
            "the subagents-home prefix is shared across cwds"
        );
    }

    /// pi `TEMP_ROOT_DIR = path.join(os.tmpdir(), `pi-subagents-${resolveTempScopeId()}`)`
    /// (`shared/types.ts:1862` @v0.43.0, and byte-identical at the ported baseline
    /// `shared/types.ts:1104` @v0.34.0). Every run-scratch root hangs off the OS TEMP dir.
    ///
    /// This port used to resolve `<CYRUP_HOME|HOME>/.cyrup/subagents` instead, and that single
    /// wrong base is what accumulated 59,321 files / 551 MB of synthetic-run residue
    /// (`fleetrun0001`, 21,076 `cwd`-keyed dirs) inside a real developer's `~/.cyrup`. The test
    /// asserts the property that made that possible is gone: the root is under the temp dir, and
    /// nothing this module derives is under the home dir.
    #[test]
    fn temp_root_dir_lives_under_the_os_temp_dir_and_never_under_home() {
        // Production shape: CYRUP_HOME unset -> `<os-temp>/cyrup-subagents-<scope>`, pi's
        // `TEMP_ROOT_DIR`. Proven through the pure core so a stray ambient CYRUP_HOME (this crate's
        // integration tests set one) can never make the assertion vacuous.
        let os_temp = PathBuf::from("/os-temp");
        let root = temp_root_dir_from(&|_| None, os_temp.clone());
        assert!(
            root.starts_with(&os_temp),
            "the subagent scratch root must hang off the OS temp dir (pi TEMP_ROOT_DIR), got {root:?}"
        );
        let leaf = root
            .file_name()
            .and_then(|n| n.to_str())
            .expect("the temp root always has a leaf");
        assert!(
            leaf.starts_with("cyrup-subagents-"),
            "the leaf mirrors pi's `pi-subagents-<scope>` under cyrup's rebrand, got {leaf:?}"
        );
        assert!(
            leaf.len() > "cyrup-subagents-".len(),
            "the per-user scope segment must be non-empty so two users never share a scratch root"
        );

        // THE REGRESSION: with no CYRUP_HOME sandbox, nothing this module derives may land under
        // the real user's `~/.cyrup` — that is where the 59,321-file pile came from.
        assert!(
            !root.to_string_lossy().contains(".cyrup"),
            "production run scratch must never be written into the user's config dir, got {root:?}"
        );
        if std::env::var_os("CYRUP_HOME").is_none()
            && let Some(home) = std::env::var_os("HOME")
        {
            let dot_cyrup = PathBuf::from(home).join(".cyrup");
            let derived = run_artifact_roots(Path::new("/some/project"));
            let scratch = attempt_scratch_dir(Path::new("/some/project"));
            for path in [&derived.async_root, &derived.results_dir, &scratch] {
                assert!(
                    !path.starts_with(&dot_cyrup),
                    "with no CYRUP_HOME sandbox, run scratch must never resolve into the real \
                     user config dir, got {path:?}"
                );
            }
        }

        // Sandbox shape: CYRUP_HOME wins outright and relocates the whole tree.
        let sandbox = temp_root_dir_from(
            &|k| (k == "CYRUP_HOME").then(|| std::ffi::OsString::from("/sandbox")),
            os_temp,
        );
        assert_eq!(sandbox, PathBuf::from("/sandbox/.cyrup/subagents"));
    }

    /// SUBA-072: the per-attempt scratch dir is a THIRD sibling under the one run-scratch root —
    /// `<run_scratch>/scratch/<cwd_key>` — keyed exactly like the async/results roots, and never
    /// anywhere under the project `cwd` it is keyed by. Proven through the pure core against
    /// sandboxed roots so an ambient `CYRUP_HOME` cannot make it vacuous.
    #[test]
    fn attempt_scratch_dir_is_a_cwd_keyed_leaf_of_the_run_scratch_root_never_the_project_tree() {
        let roots = crate::paths::Roots::sandboxed(Path::new("/sandbox"));
        let cwd = Path::new("/some/project");
        let scratch = attempt_scratch_dir_in(&roots, cwd);

        assert_eq!(
            scratch,
            PathBuf::from("/sandbox/.cyrup/subagents")
                .join("scratch")
                .join(cwd_key(cwd)),
            "the scratch dir hangs off Roots::run_scratch under a `scratch` leaf keyed by cwd_key"
        );
        assert!(
            !scratch.starts_with(cwd),
            "the scratch dir must never be under the project working tree, got {scratch:?}"
        );
        let siblings = run_artifact_roots_in(&roots, cwd);
        assert_eq!(
            scratch.parent().and_then(Path::parent),
            siblings.async_root.parent().and_then(Path::parent),
            "scratch/async/results are siblings under the SAME root"
        );
        assert_eq!(
            scratch.file_name(),
            siblings.async_root.file_name(),
            "and share the SAME per-cwd key"
        );
        // Two projects never share a scratch tree.
        assert_ne!(
            scratch,
            attempt_scratch_dir_in(&roots, Path::new("/another/project"))
        );
    }

    /// `sanitizeTempScopeSegment` (`shared/types.ts:1807-1812` @v0.43.0): trim, collapse each run
    /// of characters outside the keep-set to one `-`, strip edge `-`, fall back to `"unknown"`.
    ///
    /// The `+` quantifier is the load-bearing detail — a RUN of illegal characters collapses to one
    /// `-`, not one per character — and so is the `|| "unknown"` fallback, without which an
    /// all-illegal username would produce a bare `cyrup-subagents-` shared by every such user.
    #[test]
    fn sanitize_temp_scope_segment_matches_pis_regex_pipeline() {
        assert_eq!(sanitize_temp_scope_segment("alice"), "alice");
        assert_eq!(sanitize_temp_scope_segment("  alice  "), "alice");
        // A run of illegal chars collapses to a SINGLE dash (the `+` quantifier).
        assert_eq!(sanitize_temp_scope_segment("a///b"), "a-b");
        assert_eq!(sanitize_temp_scope_segment("a/b"), "a-b");
        // `. _ -` survive; everything else does not.
        assert_eq!(sanitize_temp_scope_segment("a.b_c-d"), "a.b_c-d");
        assert_eq!(
            sanitize_temp_scope_segment("/home/d o/m"),
            "home-d-o-m",
            "leading and trailing dashes are stripped after collapsing"
        );
        // Empty after sanitising -> the literal "unknown", never an empty segment.
        assert_eq!(sanitize_temp_scope_segment("///"), "unknown");
        assert_eq!(sanitize_temp_scope_segment(""), "unknown");
        assert_eq!(sanitize_temp_scope_segment("   "), "unknown");
    }

    /// `resolveTempScopeId` (`shared/types.ts:1814-1857` @v0.43.0) prefers `uid-<n>` wherever
    /// `process.getuid` exists, which is every Unix. The scope must be stable within a process (two
    /// calls that disagreed would split a session's runs across two roots) and must never be empty.
    #[test]
    fn resolve_temp_scope_id_is_stable_and_non_empty() {
        let first = resolve_temp_scope_id();
        assert_eq!(
            first,
            resolve_temp_scope_id(),
            "the scope id must be stable"
        );
        assert!(!first.is_empty());
        assert!(
            !first.contains(std::path::MAIN_SEPARATOR),
            "the scope id is ONE path segment; a separator would silently deepen the tree: {first}"
        );
        #[cfg(target_os = "linux")]
        assert!(
            first.starts_with("uid-"),
            "on Linux pi's first branch (`uid-${{getuid()}}`) always wins, got {first}"
        );
    }

    /// `getHistoryPath()` (`runs/shared/run-history.ts:23-25` @v0.43.0):
    /// `path.join(getAgentDir(), "run-history.jsonl")` — the DURABLE agent dir, not the disposable
    /// temp root. This port had it under `<home>/.cyrup/subagents/`, i.e. inside the run-scratch
    /// tree, where a temp sweep would have silently discarded the user's run history.

    #[test]
    fn results_dir_for_async_root_recovers_the_orchestrator_sibling_for_the_standard_layout() {
        // Standard layout: <home>/.cyrup/subagents/async/<key>  ->  .../results/<key>.
        let roots = run_artifact_roots(Path::new("/home/me/project"));
        let recovered = results_dir_for_async_root(&roots.async_root);
        assert_eq!(
            recovered, roots.results_dir,
            "the structural fallback must reconstruct EXACTLY the orchestrator's results dir, \
             preserving the cwd key (the C7 pre-fix derivation dropped it)"
        );
    }

    #[test]
    fn results_dir_for_async_root_never_nests_results_under_async() {
        // C7's specific bug: the old `async_root.parent()/results` for a standard async_root
        // nested `results` UNDER `async`. The fix must NOT.
        let roots = run_artifact_roots(Path::new("/home/me/project"));
        let recovered = results_dir_for_async_root(&roots.async_root);
        assert!(
            !recovered.starts_with(&roots.async_root),
            "results dir must never live underneath the async root: {recovered:?}"
        );
        // Explicitly reject the exact wrong path the pre-fix code produced.
        let wrong = roots.async_root.parent().unwrap().join("results");
        assert_ne!(recovered, wrong.join(roots.async_root.file_name().unwrap()));
    }

    #[test]
    fn results_dir_for_async_root_degrades_for_a_non_standard_async_root() {
        // A bare `<base>/async` (no per-cwd key beneath a subagents home) is not the standard
        // layout; the fallback degrades to a `results` sibling of async_root's parent, matching
        // the fixed-layout fixtures the low-level runner subcommand's own unit tests assume.
        let recovered = results_dir_for_async_root(Path::new("/base/async"));
        assert_eq!(recovered, PathBuf::from("/base/results"));
    }

    #[tokio::test]
    async fn ensure_accessible_dir_creates_a_missing_nested_directory() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let nested = dir.path().join("a").join("b").join("results");
        assert!(!nested.exists());
        ensure_accessible_dir(&nested)
            .await
            .expect("creates the nested dir");
        assert!(
            nested.is_dir(),
            "the full nested path must exist and be a directory"
        );
    }

    #[tokio::test]
    async fn ensure_accessible_dir_is_idempotent_on_an_existing_writable_dir() {
        let dir = tempfile::tempdir().expect("real tempdir");
        let target = dir.path().join("results");
        ensure_accessible_dir(&target)
            .await
            .expect("first call creates");
        // A probe file must NOT be left behind by the accessibility check.
        ensure_accessible_dir(&target)
            .await
            .expect("second call is a no-op");
        let mut leftover_probes = 0usize;
        let mut entries = tokio::fs::read_dir(&target).await.expect("readdir");
        while let Ok(Some(entry)) = entries.next_entry().await {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(".cyrup-access-probe-")
            {
                leftover_probes += 1;
            }
        }
        assert_eq!(
            leftover_probes, 0,
            "the write probe must always be cleaned up"
        );
    }
}
