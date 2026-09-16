//! The on-disk schedule store: where it lives, how a schedule directory is addressed safely, and
//! the read/write/list/history/event/lock primitives part B drives.
//!
//! Ports pi `scheduledRunStorePath` (`scheduled-runs.ts:98-102` @ `7fe9dee1`),
//! `assertScheduleRoot` (`:230-256`), `scheduleDir` (`:258-273`) and the whole `ScheduleStore`
//! class (`:315-383`).
//!
//! # DIVERGENCE — `list` skips and reports where upstream throws
//!
//! Upstream's `parseSchedule` THROWS, `find` propagates it, and `list` is `ids().map(find)` — so a
//! single unreadable record makes EVERY schedule in the project unlistable, and the only thing
//! standing between that and a dead tool surface is the `try/catch` in `handleToolCall`
//! (`:556-558`) that turns it into one error result with no schedules in it.
//!
//! That is the precise failure a schema bump would cause on every existing install: version 2
//! lands, one record is rewritten, and every older build in the project reports "invalid required
//! fields" instead of listing the nine schedules it can still read perfectly well.
//!
//! So the two shapes are split deliberately:
//!
//! * [`ScheduleStore::find`] / [`ScheduleStore::get`] are upstream-faithful — a NAMED, ADDRESSED
//!   record that is corrupt is an error the caller asked for and must see, with upstream's exact
//!   sentence.
//! * [`ScheduleStore::list`] returns `(records, diagnostics)`. A record it cannot read is omitted
//!   from the first vector and described in the second, so the caller can list what it has AND
//!   say what it skipped. Nothing is swallowed and nothing is fatal.
//!
//! Neither shape can panic: the version check is a `Deserialize` that returns `Err`
//! ([`super::schedule::ScheduleVersion`]), not an assertion.
//!
//! # DIVERGENCE — the shared-git-config-root escape hatch is not ported, and cannot fire here
//!
//! `assertScheduleRoot` (`:245-249`) admits a store root whose realpath leaves the project when it
//! lands inside `realpath(<registered-worktree>/<CONFIG_DIR_NAME>)`. That hatch exists for exactly
//! one upstream layout: pi's project subagents dir is `<cwd>/.pi/subagents`
//! (`PROJECT_SUBAGENTS_RELATIVE_DIR`, `shared/artifacts.ts:6`), i.e. it lives INSIDE the config
//! directory, and linked git worktrees routinely share one `.pi` by symlink.
//!
//! cyrup's is [`crate::artifacts::project_subagents_dir`] = `<cwd>/.cyrup-subagents`, a SIBLING of
//! `.cyrup`, not a child of it. `path_within("<cwd>/.cyrup", "<cwd>/.cyrup-subagents/schedules")`
//! is false by construction, so the hatch could only ever admit a root that someone had
//! deliberately symlinked from `.cyrup-subagents` into `.cyrup` — a layout nothing in this
//! workspace creates. Refusing it is both the safe direction and a refusal with a sentence that
//! says what happened, so it is ported as the containment check alone.
//!
//! # `events.jsonl` is `0600`, and this is the seam that makes it so
//!
//! Upstream appends with `fs.appendFileSync(..., { mode: 0o600 })` (`:375`, `:381`).
//! [`crate::jsonl::BoundedJsonlWriter`] — which `run_paths.rs:90-93` makes the mandatory writer for
//! every `.jsonl` this crate appends to — opens with `create(true).append(true)` and sets no mode,
//! so the file lands at `0666 & !umask`. Of the two ways to close that gap (a `mode` option on
//! `create_with_cap`, or a chmod at this call site) this module takes the SECOND: a chmod after
//! the open also repairs a file that a previous build created world-readable, which an
//! `OpenOptions::mode` — which applies only at creation — would not. The shared primitive is left
//! exactly as it is, and there is still only one append path.

use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::identity::SessionId;
use crate::jsonl::BoundedJsonlWriter;

use super::schedule::{
    ScheduleEvent, ScheduleHistory, ScheduleId, ScheduleRecord, ScheduleRunId, ScheduleRunRecord,
    ScheduleVersion, parse_schedule, parse_schedule_history,
};
use super::{MAX_HISTORY, SCHEDULES_SUBDIR};

/// `schedule.json` — the record itself.
pub const SCHEDULE_FILE: &str = "schedule.json";
/// `history.json` — the capped, newest-first run history.
pub const HISTORY_FILE: &str = "history.json";
/// `events.jsonl` — the append-only event log.
pub const EVENTS_FILE: &str = "events.jsonl";
/// `runs/` — one JSON file per fire.
pub const RUNS_SUBDIR: &str = "runs";
/// `active.lock` — the overlap primitive (`:856`).
pub const ACTIVE_LOCK_FILE: &str = "active.lock";

/// The mode every schedule directory is created with — pi's `{ mode: 0o700 }` (`:234`, `:255`,
/// `:269`).
#[cfg(unix)]
const DIR_MODE: u32 = 0o700;
/// The mode every schedule file lands at — pi's `{ mode: 0o600 }` (`:375`, `:381`, `:857`).
#[cfg(unix)]
const FILE_MODE: u32 = 0o600;

// =================================================================================================
// The store path
// =================================================================================================

/// pi `scheduledRunStorePath` (`scheduled-runs.ts:98-102` @ `7fe9dee1`).
///
/// # `_session_id` is accepted and discarded, and that is correct
///
/// Upstream's parameter is underscore-prefixed and unused, and this port keeps both the parameter
/// and the underscore. **A schedule outlives the session that created it** — that is the entire
/// point of scheduling, and a session-keyed store would silently lose every schedule the moment
/// the creating session ended. The parameter stays in the signature so a caller that holds a
/// session hands it over and is answered "yes, and it does not matter", rather than discovering
/// later that it was never threaded. It is typed `Option<&SessionId>` rather than `Option<&str>`
/// because the type is half of what tells the next reader the omission is deliberate.
///
/// This is the counter-example that proves this programme's rule is *scoping*, not
/// *session-keying everything*.
///
/// # Why the default is PROJECT-LOCAL and not a fifth run-scratch sibling
///
/// `background/artifact_roots.rs` already keys four directories per cwd under
/// [`crate::background::temp_root_dir`] — `async`, `results`, `scratch`, `wait-subscriptions` —
/// and adding a fifth would look like consistency. It is not. That root's own doc calls it
/// *"reboot-disposable run scratch"*, and with `CYRUP_HOME` unset (its only state outside tests)
/// it resolves under [`std::env::temp_dir`]. A schedule whose whole purpose is to fire in six
/// hours, tomorrow or next week cannot live in a directory the OS is entitled to clear on reboot
/// or by a tmp reaper.
///
/// Upstream reached the same conclusion first: this is the ONLY store in `pi-subagents` whose
/// default lands in the project tree rather than `TEMP_ROOT_DIR`, while every neighbour in the
/// same file (`ASYNC_DIR`, `RESULTS_DIR`, `wait-subscriptions`, `model-exclusions.json`) is
/// temp-rooted. The asymmetry is the design, not an oversight to be tidied away.
///
/// # The `root` branch
///
/// `Some(root)` is upstream's injected `deps.storeRoot` (`:85`) — the test/sandbox seam. Its leaf
/// is [`project_key`], a one-way hash, which is also why [`ScheduleRecord::cwd`] exists.
#[must_use]
pub fn scheduled_run_store_path(
    cwd: &Path,
    _session_id: Option<&SessionId>,
    root: Option<&Path>,
) -> PathBuf {
    let resolved = resolve_path(cwd);
    match root {
        None => crate::artifacts::project_subagents_dir(&resolved).join(SCHEDULES_SUBDIR),
        Some(root) => root.join(project_key(&resolved)),
    }
}

/// `createHash("sha256").update(path.resolve(cwd)).digest("hex").slice(0, 20)` (`:100`).
///
/// **Twenty hexadecimal CHARACTERS — ten bytes of digest**, not twenty bytes. Truncating the
/// digest instead of the string produces a store a differently-built process cannot find, with no
/// error anywhere.
///
/// [`crate::background::cwd_key`] is deliberately NOT substituted here even though it answers a
/// superficially identical question. It is a [`std::hash::DefaultHasher`] 16-hex-character key,
/// and `DefaultHasher` carries **no stability guarantee across Rust releases** — tolerable for a
/// reboot-disposable scratch key, not tolerable for a directory a schedule must still be found in
/// next month by a binary built from a newer toolchain.
///
/// `to_string_lossy` is the encoding boundary: upstream hashes a JS string, so a non-UTF-8 path is
/// lossy here and two distinct such paths could collide. That is acceptable only because this
/// branch is the injected sandbox seam and never the production default.
#[must_use]
pub fn project_key(resolved_cwd: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(resolved_cwd.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(20);
    for byte in digest.iter().take(10) {
        // Writing to a `String` is infallible; the result is discarded rather than unwrapped.
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// `path.resolve(value)` — this crate's idiom (`discovery/skills.rs:235-236`,
/// `spawn/worktree.rs:255`).
fn resolve_path(value: &Path) -> PathBuf {
    std::path::absolute(value).unwrap_or_else(|_| value.to_path_buf())
}

/// pi `pathWithin` (`:166-169`) over two already-resolved paths: `candidate` is `root` itself or a
/// descendant of it.
fn path_within(root: &Path, candidate: &Path) -> bool {
    candidate == root || candidate.starts_with(root)
}

// =================================================================================================
// Errors
// =================================================================================================

/// Everything [`ScheduleStore`] can refuse.
#[derive(Debug)]
pub enum ScheduleStoreError {
    /// A filesystem failure.
    Io(io::Error),
    /// A record on disk this build cannot read — a future `schemaVersion`, a missing required
    /// field, a target it does not understand. `reason` carries upstream's verbatim sentence.
    Invalid {
        /// The file the record was read from.
        file: PathBuf,
        /// Upstream's refusal sentence.
        reason: String,
    },
    /// pi `ScheduleStore.get`'s `Schedule '<id>' not found.` (`:342`).
    NotFound {
        /// The schedule that was asked for.
        id: ScheduleId,
    },
    /// A containment refusal — `assertScheduleRoot` (`:250`, `:256`) or `scheduleDir` (`:263`,
    /// `:272`). `String` carries upstream's verbatim sentence.
    Refused(String),
}

impl std::fmt::Display for ScheduleStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "{error}"),
            Self::Invalid { reason, .. } | Self::Refused(reason) => f.write_str(reason),
            Self::NotFound { id } => write!(f, "Schedule '{id}' not found."),
        }
    }
}

impl std::error::Error for ScheduleStoreError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ScheduleStoreError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

// =================================================================================================
// The store
// =================================================================================================

/// pi `class ScheduleStore` (`scheduled-runs.ts:315-383`) — one project's schedules.
///
/// Async, unlike upstream's `readFileSync`/`writeFileSync` throughout and unlike this crate's
/// synchronous `missions/` subtree: part B's trigger loop and tool path are both tokio, and
/// wrapping every store touch in `spawn_blocking` is a worse seam than an async store. The two
/// PURE functions beside it ([`scheduled_run_store_path`], [`project_key`]) stay synchronous —
/// they touch nothing.
#[derive(Clone, Debug)]
pub struct ScheduleStore {
    root: PathBuf,
    project_cwd: Option<PathBuf>,
}

impl ScheduleStore {
    /// pi `new ScheduleStore(root, projectCwd)` (`:319-322`).
    ///
    /// `project_cwd` is the containment anchor: with it, every path this store touches must
    /// resolve inside the real project. Upstream passes `undefined` for the injected-`storeRoot`
    /// branch (`:531`), where the root is already a sandbox the caller chose.
    #[must_use]
    pub fn new(root: PathBuf, project_cwd: Option<PathBuf>) -> Self {
        Self {
            root,
            project_cwd: project_cwd.as_deref().map(resolve_path),
        }
    }

    /// The store root this instance addresses.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// pi `assertScheduleRoot` (`:230-256`) — the root resolves inside the real project.
    ///
    /// Walks up to the first ancestor that exists, resolves it, and requires containment; with
    /// `create`, then creates the root `0700` and re-checks the resolved result, because the
    /// `mkdir` itself could have followed a symlink out.
    ///
    /// # Errors
    ///
    /// [`ScheduleStoreError::Refused`] with upstream's `Project schedule root '<root>' resolves
    /// outside the real project.`, or an I/O failure.
    pub async fn ensure_root(&self, create: bool) -> Result<(), ScheduleStoreError> {
        let Some(project_cwd) = self.project_cwd.as_deref() else {
            if create {
                create_private_dir(&self.root).await?;
            }
            return Ok(());
        };
        let project_path = match tokio::fs::canonicalize(project_cwd).await {
            Ok(path) => path,
            Err(error) if !create && error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let refusal = || {
            ScheduleStoreError::Refused(format!(
                "Project schedule root '{}' resolves outside the real project.",
                self.root.display()
            ))
        };

        let mut existing = self.root.clone();
        while tokio::fs::symlink_metadata(&existing).await.is_err() {
            let Some(parent) = existing.parent() else {
                break;
            };
            if parent == existing {
                break;
            }
            existing = parent.to_path_buf();
        }
        let existing_path = tokio::fs::canonicalize(&existing).await?;
        if !path_within(&project_path, &existing_path) {
            return Err(refusal());
        }
        if !create {
            return Ok(());
        }
        create_private_dir(&self.root).await?;
        let created = tokio::fs::canonicalize(&self.root).await?;
        if path_within(&project_path, &created) {
            Ok(())
        } else {
            Err(refusal())
        }
    }

    /// pi `scheduleDir` (`:258-273`) — this schedule's own directory, containment-checked.
    ///
    /// The `lstat` is deliberate: a plain `is_dir()` FOLLOWS a symlink, so a `schedules/<id>`
    /// symlink pointing at `/etc` would read as a perfectly good directory. Upstream rejects the
    /// link itself (`:262-263`), and so does this.
    ///
    /// # Errors
    ///
    /// [`ScheduleStoreError::Refused`] with `Schedule path '<dir>' must be a real directory.` or
    /// `Schedule path '<dir>' escapes the project schedule root.`, whatever
    /// [`ScheduleStore::ensure_root`] raises, or an I/O failure.
    pub async fn directory(
        &self,
        id: &ScheduleId,
        create: bool,
    ) -> Result<PathBuf, ScheduleStoreError> {
        self.ensure_root(create).await?;
        let dir = self.root.join(id.as_str());
        match tokio::fs::symlink_metadata(&dir).await {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_dir() {
                    return Err(ScheduleStoreError::Refused(format!(
                        "Schedule path '{}' must be a real directory.",
                        dir.display()
                    )));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                if !create {
                    return Ok(dir);
                }
                create_private_dir(&dir).await?;
            }
            Err(error) => return Err(error.into()),
        }
        let root_path = tokio::fs::canonicalize(&self.root).await?;
        let dir_path = tokio::fs::canonicalize(&dir).await?;
        if dir_path == root_path.join(id.as_str()) {
            Ok(dir)
        } else {
            Err(ScheduleStoreError::Refused(format!(
                "Schedule path '{}' escapes the project schedule root.",
                dir.display()
            )))
        }
    }

    /// pi `ScheduleStore.ids` (`:328-334`) — every directory under the root whose NAME is a legal
    /// [`ScheduleId`]. Anything else is not a schedule and is not addressable.
    ///
    /// # Errors
    ///
    /// Whatever [`ScheduleStore::ensure_root`] raises, or a `read_dir` failure.
    pub async fn ids(&self) -> Result<Vec<ScheduleId>, ScheduleStoreError> {
        self.ensure_root(false).await?;
        let mut entries = match tokio::fs::read_dir(&self.root).await {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let mut ids = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_dir() {
                continue;
            }
            if let Some(id) = entry.file_name().to_str().and_then(ScheduleId::parse) {
                ids.push(id);
            }
        }
        ids.sort();
        Ok(ids)
    }

    /// pi `ScheduleStore.list` (`:336-338`), with this module's deliberate divergence: a record
    /// that cannot be read is OMITTED and DESCRIBED rather than propagated, so one future-version
    /// record cannot make every other schedule in the project unlistable.
    ///
    /// The diagnostics vector is never silently dropped by this function — it is the caller's to
    /// render, and it is the only thing that distinguishes "you have three schedules" from "you
    /// have three schedules and one I could not read".
    pub async fn list(&self) -> (Vec<ScheduleRecord>, Vec<ScheduleStoreError>) {
        let mut records = Vec::new();
        let mut diagnostics = Vec::new();
        let ids = match self.ids().await {
            Ok(ids) => ids,
            Err(error) => return (records, vec![error]),
        };
        for id in ids {
            match self.find(&id).await {
                Ok(Some(record)) => records.push(record),
                Ok(None) => {}
                Err(error) => diagnostics.push(error),
            }
        }
        (records, diagnostics)
    }

    /// pi `ScheduleStore.find` (`:345-350`) — `Ok(None)` when the schedule no longer exists.
    ///
    /// # Errors
    ///
    /// [`ScheduleStoreError::Invalid`] for a record this build cannot read (upstream's verbatim
    /// sentence), a containment refusal, or an I/O failure.
    pub async fn find(
        &self,
        id: &ScheduleId,
    ) -> Result<Option<ScheduleRecord>, ScheduleStoreError> {
        let file = self.directory(id, false).await?.join(SCHEDULE_FILE);
        let bytes = match tokio::fs::read(&file).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        // pi `readJson` (`:275-281`), whose message is its own shape.
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| ScheduleStoreError::Invalid {
                file: file.clone(),
                reason: format!(
                    "Failed to read schedule record '{}': {error}",
                    file.display()
                ),
            })?;
        parse_schedule(&value, &file)
            .map(Some)
            .map_err(|reason| ScheduleStoreError::Invalid { file, reason })
    }

    /// pi `ScheduleStore.get` (`:340-344`).
    ///
    /// # Errors
    ///
    /// [`ScheduleStoreError::NotFound`] (`Schedule '<id>' not found.`) plus everything
    /// [`ScheduleStore::find`] raises.
    pub async fn get(&self, id: &ScheduleId) -> Result<ScheduleRecord, ScheduleStoreError> {
        self.find(id)
            .await?
            .ok_or_else(|| ScheduleStoreError::NotFound { id: id.clone() })
    }

    /// pi `ScheduleStore.write` (`:352-354`) — `schedule.json`, `0600`, temp-then-rename.
    ///
    /// **This is not where the capability-ceiling gate goes.** See
    /// [`super::ceiling_gate`] for why folding it in here wedges every running schedule.
    ///
    /// # Errors
    ///
    /// A containment refusal or an I/O failure.
    pub async fn write(&self, record: &ScheduleRecord) -> Result<(), ScheduleStoreError> {
        let dir = self.directory(&record.id, true).await?;
        crate::background::atomic::write_private_atomic_json(&dir.join(SCHEDULE_FILE), record)
            .await?;
        Ok(())
    }

    /// pi `ScheduleStore.delete` (`:356-358`) — `rm -rf` of the schedule's own directory.
    ///
    /// # Errors
    ///
    /// A containment refusal or an I/O failure other than "already gone".
    pub async fn delete(&self, id: &ScheduleId) -> Result<(), ScheduleStoreError> {
        let dir = self.directory(id, false).await?;
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// pi `ScheduleStore.history` (`:361-367`) — newest first, `[]` when there is none yet.
    ///
    /// # Errors
    ///
    /// `Schedule history '<file>' has invalid fields.` verbatim, a containment refusal, or an I/O
    /// failure.
    pub async fn history(
        &self,
        id: &ScheduleId,
    ) -> Result<Vec<ScheduleRunRecord>, ScheduleStoreError> {
        let file = self.directory(id, false).await?.join(HISTORY_FILE);
        let bytes = match tokio::fs::read(&file).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error.into()),
        };
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).map_err(|error| ScheduleStoreError::Invalid {
                file: file.clone(),
                reason: format!(
                    "Failed to read schedule history '{}': {error}",
                    file.display()
                ),
            })?;
        parse_schedule_history(&value, &file)
            .map_err(|reason| ScheduleStoreError::Invalid { file, reason })
    }

    /// pi `ScheduleStore.writeRun` (`:369-376`) — the run file, the rewritten capped history, and
    /// one event line, in that order.
    ///
    /// History is newest-first, de-duplicated by run id (a run written twice as it moves
    /// `running` -> `completed` occupies ONE slot, carrying its latest state) and truncated to
    /// [`MAX_HISTORY`].
    ///
    /// # Errors
    ///
    /// A containment refusal, an unreadable existing history, or an I/O failure.
    pub async fn write_run(
        &self,
        schedule: &ScheduleRecord,
        run: &ScheduleRunRecord,
        event: &str,
    ) -> Result<(), ScheduleStoreError> {
        let dir = self.directory(&schedule.id, true).await?;
        let run_file = dir
            .join(RUNS_SUBDIR)
            .join(format!("{}.json", run.id.as_str()));
        crate::background::atomic::write_private_atomic_json(&run_file, run).await?;

        let mut runs = vec![run.clone()];
        runs.extend(
            self.history(&schedule.id)
                .await?
                .into_iter()
                .filter(|item| item.id != run.id),
        );
        runs.truncate(MAX_HISTORY);
        crate::background::atomic::write_private_atomic_json(
            &dir.join(HISTORY_FILE),
            &ScheduleHistory {
                schema_version: ScheduleVersion,
                runs,
            },
        )
        .await?;

        self.append_event_line(
            &dir,
            &ScheduleEvent {
                schema_version: ScheduleVersion,
                timestamp: now_iso8601(),
                event: event.to_string(),
                schedule_id: schedule.id.clone(),
                run_id: Some(run.id.clone()),
                state: Some(run.state),
            },
        )
        .await
    }

    /// pi `ScheduleStore.appendEvent` (`:378-382`) — one event line with no run attached.
    ///
    /// # Errors
    ///
    /// A containment refusal or an I/O failure.
    pub async fn append_event(
        &self,
        schedule: &ScheduleRecord,
        event: &str,
    ) -> Result<(), ScheduleStoreError> {
        let dir = self.directory(&schedule.id, true).await?;
        self.append_event_line(
            &dir,
            &ScheduleEvent {
                schema_version: ScheduleVersion,
                timestamp: now_iso8601(),
                event: event.to_string(),
                schedule_id: schedule.id.clone(),
                run_id: None,
                state: None,
            },
        )
        .await
    }

    /// The single append path (see this module's doc on `0600`).
    async fn append_event_line(
        &self,
        dir: &Path,
        event: &ScheduleEvent,
    ) -> Result<(), ScheduleStoreError> {
        let line = serde_json::to_string(event)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        let path = dir.join(EVENTS_FILE);
        let mut writer = BoundedJsonlWriter::create(&path).await?;
        set_private_file_mode(&path).await?;
        writer.write_line(&line).await?;
        Ok(())
    }

    /// pi's `fs.openSync(lockPath, "wx", 0o600)` (`:857`) — the OVERLAP PRIMITIVE, and only that.
    ///
    /// `Ok(true)` means this caller now holds the lock; `Ok(false)` is upstream's `EEXIST`, i.e.
    /// another run holds it. What to DO about that — skip, steal, or recover a stale claim after
    /// `STALE_LAUNCH_CLAIM_MS` — is part B's policy and is deliberately not decided here.
    ///
    /// # Errors
    ///
    /// A containment refusal or an I/O failure other than "already exists".
    pub async fn acquire_active_lock(
        &self,
        id: &ScheduleId,
        run_id: &ScheduleRunId,
    ) -> Result<bool, ScheduleStoreError> {
        let path = self.directory(id, true).await?.join(ACTIVE_LOCK_FILE);
        let mut options = tokio::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        // `tokio::fs::OpenOptions::mode` is inherent under `cfg(unix)` — no extension trait.
        options.mode(FILE_MODE);
        match options.open(&path).await {
            Ok(mut file) => {
                use tokio::io::AsyncWriteExt as _;
                file.write_all(run_id.as_str().as_bytes()).await?;
                file.flush().await?;
                Ok(true)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(error) => Err(error.into()),
        }
    }

    /// pi's `fs.rmSync(lockPath, { force: true })` (`:777`, `:932`).
    ///
    /// # Errors
    ///
    /// A containment refusal or an I/O failure other than "already gone".
    pub async fn release_active_lock(&self, id: &ScheduleId) -> Result<(), ScheduleStoreError> {
        let path = self.directory(id, false).await?.join(ACTIVE_LOCK_FILE);
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }

    /// The run id currently written into `active.lock`, if the lock is held.
    ///
    /// # Errors
    ///
    /// A containment refusal or an I/O failure other than "not there".
    pub async fn active_lock_holder(
        &self,
        id: &ScheduleId,
    ) -> Result<Option<ScheduleRunId>, ScheduleStoreError> {
        let path = self.directory(id, false).await?.join(ACTIVE_LOCK_FILE);
        match tokio::fs::read_to_string(&path).await {
            Ok(contents) => Ok(ScheduleRunId::parse(contents.trim())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }
}

/// `new Date().toISOString()` (`:375`, `:381`) through the crate's single clock.
fn now_iso8601() -> String {
    crate::background::run_status::format_iso8601_millis(crate::time::now_epoch_millis())
}

/// `fs.mkdirSync(path, { recursive: true, mode: 0o700 })`.
///
/// The chmod is applied AFTER the create rather than through `DirBuilder::mode`, because `mode`
/// is masked by the process umask at creation and a directory that already exists is not
/// re-moded at all — this way `0700` is what is actually on disk either way.
async fn create_private_dir(path: &Path) -> io::Result<()> {
    tokio::fs::create_dir_all(path).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(DIR_MODE)).await?;
    }
    Ok(())
}

/// `{ mode: 0o600 }` on an append target (see this module's doc).
async fn set_private_file_mode(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(FILE_MODE)).await?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use super::super::schedule::ScheduleRunState;
    use super::super::test_fixtures::{full_record, run_record};
    use super::*;

    fn session(raw: &str) -> SessionId {
        SessionId::parse(raw).expect("test session id is non-empty")
    }

    /// A store rooted at the PROJECT-LOCAL default for `project`, containment-anchored to it.
    fn project_store(project: &Path) -> ScheduleStore {
        let root = scheduled_run_store_path(project, Some(&session("s")), None);
        ScheduleStore::new(root, Some(project.to_path_buf()))
    }

    /// SUBTASK1 — two sessions, one cwd, ONE store.
    ///
    /// All three spellings are asserted equal, so an implementation that "only ignores the
    /// session when it is `None`" fails just as loudly as one that threads it into the path.
    #[test]
    fn the_store_path_is_keyed_by_cwd_not_session() {
        let cwd = Path::new("/projects/alpha");
        let a = session("session-a");
        let b = session("/home/u/.cyrup/agent/sessions/b.jsonl");

        let with_a = scheduled_run_store_path(cwd, Some(&a), None);
        let with_b = scheduled_run_store_path(cwd, Some(&b), None);
        let with_none = scheduled_run_store_path(cwd, None, None);
        assert_eq!(with_a, with_b);
        assert_eq!(with_a, with_none);

        let root = Path::new("/sandbox");
        assert_eq!(
            scheduled_run_store_path(cwd, Some(&a), Some(root)),
            scheduled_run_store_path(cwd, Some(&b), Some(root)),
        );
        assert_eq!(
            scheduled_run_store_path(cwd, Some(&a), Some(root)),
            scheduled_run_store_path(cwd, None, Some(root)),
        );

        assert_ne!(
            with_a,
            scheduled_run_store_path(Path::new("/projects/beta"), Some(&a), None),
            "a different cwd IS a different store"
        );
    }

    /// §1.2 — the default store is PROJECT-local and must never drift into the
    /// reboot-disposable run-scratch tree its four cwd-keyed siblings live in.
    #[test]
    fn the_default_store_path_is_project_local_and_never_run_scratch() {
        let project = tempfile::tempdir().expect("real tempdir");
        let resolved = resolve_path(project.path());
        let path = scheduled_run_store_path(project.path(), None, None);

        assert!(
            path.starts_with(crate::artifacts::project_subagents_dir(&resolved)),
            "{path:?} must live under the project's own .cyrup-subagents"
        );
        assert_eq!(
            path.file_name().and_then(std::ffi::OsStr::to_str),
            Some("schedules")
        );
        assert_eq!(
            path,
            crate::artifacts::project_subagents_dir(&resolved).join(SCHEDULES_SUBDIR)
        );

        let scratch = crate::background::temp_root_dir();
        assert!(
            !path.starts_with(&scratch),
            "{path:?} must not be under the reboot-disposable run scratch root {scratch:?}"
        );
        // …and specifically not the "fifth cwd-keyed sibling" shape.
        assert!(!path.starts_with(scratch.join(SCHEDULES_SUBDIR)));
    }

    /// `:100` — the injected-root leaf is TWENTY HEX CHARACTERS of a sha256 over the RESOLVED
    /// cwd, i.e. ten bytes of digest. A 20-BYTE truncation, or a `cwd_key` substitution, produces
    /// a store a differently-built process silently cannot find.
    #[test]
    fn an_explicit_root_keys_by_a_twenty_hex_char_sha256_of_the_resolved_cwd() {
        use std::fmt::Write as _;

        let cwd = Path::new("/projects/alpha");
        let root = Path::new("/sandbox/schedules");
        let path = scheduled_run_store_path(cwd, None, Some(root));
        let leaf = path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .expect("a leaf");

        assert_eq!(
            leaf.len(),
            20,
            "twenty CHARACTERS, not twenty bytes of digest"
        );
        assert!(
            leaf.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );

        let mut hasher = Sha256::new();
        hasher.update(resolve_path(cwd).to_string_lossy().as_bytes());
        let mut full = String::new();
        for byte in hasher.finalize() {
            let _ = write!(full, "{byte:02x}");
        }
        assert_eq!(leaf, &full[..20]);
        assert_eq!(path, root.join(leaf));

        assert_ne!(
            leaf,
            scheduled_run_store_path(Path::new("/projects/beta"), None, Some(root))
                .file_name()
                .and_then(std::ffi::OsStr::to_str)
                .expect("a leaf"),
        );
        // It is NOT `background::cwd_key`, whose DefaultHasher has no cross-release stability
        // guarantee and whose key is sixteen characters.
        assert_ne!(leaf, crate::background::cwd_key(&resolve_path(cwd)));
    }

    /// Every optional field populated, written and read back identical.
    #[tokio::test]
    async fn a_schedule_round_trips_through_the_store() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let record = full_record("nightly.sweep", project.path());

        store.write(&record).await.expect("writes");
        let read_back = store.get(&record.id).await.expect("reads");
        assert_eq!(read_back, record);

        // The wire really is camelCase, and really does carry every optional field.
        let file = store
            .directory(&record.id, false)
            .await
            .expect("directory")
            .join(SCHEDULE_FILE);
        let raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).expect("reads")).expect("json");
        for key in [
            "schemaVersion",
            "catchUp",
            "timeoutMs",
            "sessionOnly",
            "ownerSessionFile",
            "createdAt",
            "updatedAt",
            "activeRunId",
            "lastRunId",
        ] {
            assert!(raw.get(key).is_some(), "{key} must be on the wire");
        }
        assert_eq!(raw["paused"], serde_json::Value::Bool(false));
        assert_eq!(raw["overlap"], "skip");
        assert!(raw["target"].get("args").is_some());
        assert_eq!(raw["target"]["baseRef"], "refs/heads/main");
    }

    /// The reason the store is cwd-keyed: a schedule written under one session is found, byte
    /// for byte, by a store constructed under a different one.
    #[tokio::test]
    async fn the_store_survives_a_session_change() {
        let project = tempfile::tempdir().expect("real tempdir");
        let a = session("session-a");
        let b = session("session-b");

        let store_a = ScheduleStore::new(
            scheduled_run_store_path(project.path(), Some(&a), None),
            Some(project.path().to_path_buf()),
        );
        let record = full_record("survives", project.path());
        store_a.write(&record).await.expect("writes");

        let store_b = ScheduleStore::new(
            scheduled_run_store_path(project.path(), Some(&b), None),
            Some(project.path().to_path_buf()),
        );
        assert_eq!(store_b.root(), store_a.root());
        assert_eq!(store_b.get(&record.id).await.expect("reads"), record);
    }

    /// §3.3 — one future-version record must not make every other schedule unlistable.
    #[tokio::test]
    async fn a_future_schedule_version_is_ignored_not_a_panic() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());

        let good = full_record("good", project.path());
        store.write(&good).await.expect("writes");

        let future_dir = store.root().join("future");
        std::fs::create_dir_all(&future_dir).expect("mkdir");
        let future_file = future_dir.join(SCHEDULE_FILE);
        std::fs::write(
            &future_file,
            serde_json::to_vec(&serde_json::json!({
                "schemaVersion": 2,
                "id": "future",
                "name": "from a newer build",
                "cwd": project.path(),
                "trigger": { "kind": "once", "at": "2027-01-01T00:00:00.000Z" },
                "target": { "workflowScript": "return 1;", "args": {} },
                "overlap": "skip",
                "catchUp": "none",
                "paused": false,
                "createdAt": "2026-09-15T00:00:00.000Z",
                "updatedAt": "2026-09-15T00:00:00.000Z"
            }))
            .expect("json"),
        )
        .expect("writes");

        let future_id = ScheduleId::parse("future").expect("legal id");
        let error = store
            .get(&future_id)
            .await
            .expect_err("addressed: an error");
        match &error {
            ScheduleStoreError::Invalid { file, reason } => {
                assert_eq!(file, &future_file);
                assert_eq!(
                    reason,
                    &format!(
                        "Schedule record '{}' has invalid required fields.",
                        future_file.display()
                    )
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
        assert_eq!(error.to_string(), {
            format!(
                "Schedule record '{}' has invalid required fields.",
                future_file.display()
            )
        });

        let (records, diagnostics) = store.list().await;
        assert_eq!(records.len(), 1, "the readable sibling still lists");
        assert_eq!(records[0].id, good.id);
        assert_eq!(diagnostics.len(), 1, "and the unreadable one is REPORTED");
        assert!(matches!(diagnostics[0], ScheduleStoreError::Invalid { .. }));
        assert!(store.ids().await.expect("ids").len() == 2);
    }

    /// pi `readJson` (`:275-281`) — the arm above the parser: bytes that are not JSON at all.
    ///
    /// The module's whole list/get split is argued over `parse_schedule` refusals, and this is the
    /// other way a record goes bad (a truncated write, a half-synced file). It must reach the same
    /// two shapes — an error for an ADDRESSED record, a diagnostic for a LISTED one — and it must
    /// carry `readJson`'s own message rather than the parser's.
    #[tokio::test]
    async fn bytes_that_are_not_json_are_an_error_not_a_panic() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let good = full_record("good", project.path());
        store.write(&good).await.expect("writes");

        let broken_dir = store.root().join("broken");
        std::fs::create_dir_all(broken_dir.join(RUNS_SUBDIR)).expect("mkdir");
        let broken_file = broken_dir.join(SCHEDULE_FILE);
        std::fs::write(&broken_file, b"{\"schemaVersion\": 1,").expect("writes");
        let broken = ScheduleId::parse("broken").expect("legal id");

        let error = store.get(&broken).await.expect_err("addressed: an error");
        match &error {
            ScheduleStoreError::Invalid { file, reason } => {
                assert_eq!(file, &broken_file);
                assert!(
                    reason.starts_with(&format!(
                        "Failed to read schedule record '{}': ",
                        broken_file.display()
                    )),
                    "readJson's own message, not the parser's: {reason}"
                );
            }
            other => panic!("expected Invalid, got {other:?}"),
        }

        let (records, diagnostics) = store.list().await;
        assert_eq!(records.len(), 1, "the readable sibling still lists");
        assert_eq!(records[0].id, good.id);
        assert_eq!(diagnostics.len(), 1, "and the unreadable one is REPORTED");

        // The same arm on `history.json`, whose own reader is separate.
        let history_file = store.root().join("good").join(HISTORY_FILE);
        std::fs::write(&history_file, b"not json").expect("writes");
        let error = store
            .history(&good.id)
            .await
            .expect_err("a corrupt history is an error, never an empty history");
        assert!(
            error.to_string().starts_with(&format!(
                "Failed to read schedule history '{}': ",
                history_file.display()
            )),
            "{error}"
        );
    }

    /// `:258-273` — an id that would escape the store root cannot be constructed, cannot be
    /// deserialized, and cannot be produced by listing the root.
    #[tokio::test]
    async fn a_schedule_id_that_escapes_the_store_root_is_refused() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let good = full_record("good", project.path());
        store.write(&good).await.expect("writes");

        for bad in [
            "../evil",
            "a/b",
            "",
            ".",
            "..",
            ".hidden",
            "-flag",
            &"z".repeat(65),
        ] {
            assert!(
                ScheduleId::parse(bad).is_none(),
                "{bad:?} must not become a ScheduleId"
            );
        }

        // Directories the id grammar refuses are not addressable through the store either, so a
        // hand-placed `.hidden`/`-flag`/over-long directory is invisible rather than reachable.
        for illegal in [".hidden", "-flag", &"z".repeat(65)] {
            std::fs::create_dir_all(store.root().join(illegal)).expect("mkdir");
        }
        let ids = store.ids().await.expect("ids");
        assert_eq!(ids, vec![good.id.clone()]);
        let (records, diagnostics) = store.list().await;
        assert_eq!(records.len(), 1);
        assert!(diagnostics.is_empty());

        // And nothing was ever created outside the root.
        let parent = store.root().parent().expect("a parent").to_path_buf();
        let stray: Vec<_> = std::fs::read_dir(&parent)
            .expect("read_dir")
            .filter_map(|entry| entry.ok().map(|e| e.file_name()))
            .filter(|name| name != "schedules")
            .collect();
        assert!(stray.is_empty(), "unexpected siblings: {stray:?}");
        assert!(
            !project
                .path()
                .parent()
                .expect("a parent")
                .join("evil")
                .exists()
        );
    }

    /// `:262-263` — the check is an `lstat`, so a SYMLINK named like a schedule is refused
    /// instead of being followed. A plain `is_dir()` would follow it and happily write into
    /// whatever it points at.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlinked_schedule_directory_is_refused() {
        let project = tempfile::tempdir().expect("real tempdir");
        let outside = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        store.ensure_root(true).await.expect("root exists");

        std::os::unix::fs::symlink(outside.path(), store.root().join("linked")).expect("symlink");
        let linked = ScheduleId::parse("linked").expect("legal id");

        let error = store
            .directory(&linked, false)
            .await
            .expect_err("a symlinked schedule directory must be refused");
        assert_eq!(
            error.to_string(),
            format!(
                "Schedule path '{}' must be a real directory.",
                store.root().join("linked").display()
            )
        );
        assert!(store.find(&linked).await.is_err());
        assert!(
            store
                .write(&full_record("linked", project.path()))
                .await
                .is_err()
        );
        // The refusal is what keeps the target untouched.
        assert!(!outside.path().join(SCHEDULE_FILE).exists());

        // A plain FILE where a directory belongs is the same refusal.
        std::fs::write(store.root().join("afile"), b"not a dir").expect("writes");
        let afile = ScheduleId::parse("afile").expect("legal id");
        assert_eq!(
            store
                .directory(&afile, false)
                .await
                .unwrap_err()
                .to_string(),
            format!(
                "Schedule path '{}' must be a real directory.",
                store.root().join("afile").display()
            )
        );
    }

    /// A store root that resolves out of the project is refused with upstream's sentence
    /// (`:250`/`:256`) rather than silently writing outside it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_store_root_that_escapes_the_project_is_refused() {
        let project = tempfile::tempdir().expect("real tempdir");
        let outside = tempfile::tempdir().expect("real tempdir");
        let linked_root = project.path().join("escaping");
        std::os::unix::fs::symlink(outside.path(), &linked_root).expect("symlink");

        let store = ScheduleStore::new(
            linked_root.join("schedules"),
            Some(project.path().to_path_buf()),
        );
        let error = store.ensure_root(true).await.expect_err("must be refused");
        assert_eq!(
            error.to_string(),
            format!(
                "Project schedule root '{}' resolves outside the real project.",
                linked_root.join("schedules").display()
            )
        );
        assert!(!outside.path().join("schedules").exists());

        // With no project anchor (upstream's `projectCwd === undefined`, the injected-root
        // branch) the same root is the caller's own sandbox and is created.
        let unanchored = ScheduleStore::new(linked_root.join("schedules"), None);
        unanchored
            .ensure_root(true)
            .await
            .expect("no anchor, no check");
        assert!(outside.path().join("schedules").is_dir());
    }

    /// `:369-376` — the run file, a newest-first history de-duplicated by run id, and the cap.
    #[tokio::test]
    async fn a_run_record_and_its_history_round_trip() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let schedule = full_record("history", project.path());
        store.write(&schedule).await.expect("writes");

        let first = ScheduleRunId::mint();
        let running = run_record(&schedule.id, first.clone(), ScheduleRunState::Running);
        store
            .write_run(&schedule, &running, "schedule.run.started")
            .await
            .expect("writes run");

        let mut completed = running.clone();
        completed.state = ScheduleRunState::Completed;
        completed.completed_at = Some("2026-09-15T06:10:00.000Z".to_string());
        store
            .write_run(&schedule, &completed, "schedule.run.completed")
            .await
            .expect("rewrites run");

        let history = store.history(&schedule.id).await.expect("history");
        assert_eq!(history.len(), 1, "the same run id occupies ONE slot");
        assert_eq!(history[0], completed);

        let run_file = store
            .directory(&schedule.id, false)
            .await
            .expect("directory")
            .join(RUNS_SUBDIR)
            .join(format!("{first}.json"));
        assert!(run_file.is_file());
        let on_disk: ScheduleRunRecord =
            serde_json::from_slice(&std::fs::read(&run_file).expect("reads")).expect("json");
        assert_eq!(on_disk, completed);

        // A second, distinct run lands NEWEST FIRST.
        let second = run_record(
            &schedule.id,
            ScheduleRunId::mint(),
            ScheduleRunState::Skipped,
        );
        store
            .write_run(&schedule, &second, "schedule.skipped_overlap")
            .await
            .expect("writes run");
        let history = store.history(&schedule.id).await.expect("history");
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].id, second.id);
        assert_eq!(history[1].id, completed.id);

        // …and the cap holds at MAX_HISTORY.
        for _ in 0..MAX_HISTORY {
            let run = run_record(
                &schedule.id,
                ScheduleRunId::mint(),
                ScheduleRunState::Missed,
            );
            store
                .write_run(&schedule, &run, "schedule.missed")
                .await
                .expect("writes run");
        }
        let history = store.history(&schedule.id).await.expect("history");
        assert_eq!(history.len(), MAX_HISTORY);
        assert!(
            history
                .iter()
                .all(|run| run.state == ScheduleRunState::Missed),
            "the oldest entries are the ones dropped"
        );
    }

    /// `:375`/`:381` plus §1.5 — two appends, two parseable lines, and the file is `0600`.
    ///
    /// The mode assertion is what forces the gap closed: [`BoundedJsonlWriter`] opens with no
    /// mode at all, so without this call site's chmod the log lands at `0666 & !umask`.
    #[tokio::test]
    async fn an_event_line_is_appended_privately() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let schedule = full_record("events", project.path());
        store.write(&schedule).await.expect("writes");

        store
            .append_event(&schedule, "schedule.created")
            .await
            .expect("appends");
        let run = run_record(
            &schedule.id,
            ScheduleRunId::mint(),
            ScheduleRunState::Running,
        );
        store
            .write_run(&schedule, &run, "schedule.run.started")
            .await
            .expect("appends");

        let log = store
            .directory(&schedule.id, false)
            .await
            .expect("directory")
            .join(EVENTS_FILE);
        let text = std::fs::read_to_string(&log).expect("reads");
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2, "two appends, two lines");

        let created: ScheduleEvent = serde_json::from_str(lines[0]).expect("line 0 parses");
        assert_eq!(created.event, "schedule.created");
        assert_eq!(created.schedule_id, schedule.id);
        assert_eq!(created.run_id, None);
        assert_eq!(created.state, None);
        assert!(created.timestamp.ends_with('Z'));

        let started: ScheduleEvent = serde_json::from_str(lines[1]).expect("line 1 parses");
        assert_eq!(started.event, "schedule.run.started");
        assert_eq!(started.run_id.as_ref(), Some(&run.id));
        assert_eq!(started.state, Some(ScheduleRunState::Running));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&log)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600, "the event log is owner-only");
            let dir_mode = std::fs::metadata(store.root().join("events"))
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(dir_mode & 0o777, 0o700, "and so is the directory");
        }
    }

    /// `:857` — the primitive only. Policy (skip / steal / stale recovery) stays in part B.
    #[tokio::test]
    async fn an_active_lock_is_exclusive() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let schedule = full_record("locked", project.path());
        store.write(&schedule).await.expect("writes");

        let first = ScheduleRunId::mint();
        let second = ScheduleRunId::mint();
        assert!(
            store
                .acquire_active_lock(&schedule.id, &first)
                .await
                .expect("acquires")
        );
        assert_eq!(
            store.active_lock_holder(&schedule.id).await.expect("reads"),
            Some(first.clone())
        );
        assert!(
            !store
                .acquire_active_lock(&schedule.id, &second)
                .await
                .expect("the second acquire is EEXIST, not an error"),
        );
        assert_eq!(
            store.active_lock_holder(&schedule.id).await.expect("reads"),
            Some(first),
            "a refused acquire must not overwrite the holder"
        );

        store
            .release_active_lock(&schedule.id)
            .await
            .expect("releases");
        assert_eq!(
            store.active_lock_holder(&schedule.id).await.expect("reads"),
            None
        );
        assert!(
            store
                .acquire_active_lock(&schedule.id, &second)
                .await
                .expect("re-acquires")
        );
        assert_eq!(
            store.active_lock_holder(&schedule.id).await.expect("reads"),
            Some(second)
        );
        // Releasing twice is not an error (`fs.rmSync(..., { force: true })`).
        store
            .release_active_lock(&schedule.id)
            .await
            .expect("releases");
        store
            .release_active_lock(&schedule.id)
            .await
            .expect("idempotent");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let third = ScheduleRunId::mint();
            assert!(
                store
                    .acquire_active_lock(&schedule.id, &third)
                    .await
                    .expect("acquires")
            );
            let lock = store.root().join("locked").join(ACTIVE_LOCK_FILE);
            assert_eq!(
                std::fs::metadata(&lock)
                    .expect("metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    /// `find` is `Ok(None)` for a schedule that never existed; `get` is upstream's
    /// `Schedule '<id>' not found.`; `delete` removes the whole directory and is idempotent.
    #[tokio::test]
    async fn find_get_and_delete_behave_as_upstream_does() {
        let project = tempfile::tempdir().expect("real tempdir");
        let store = project_store(project.path());
        let missing = ScheduleId::parse("nope").expect("legal id");

        assert!(store.find(&missing).await.expect("find").is_none());
        assert_eq!(
            store.get(&missing).await.unwrap_err().to_string(),
            "Schedule 'nope' not found."
        );

        let record = full_record("goes-away", project.path());
        store.write(&record).await.expect("writes");
        assert!(store.find(&record.id).await.expect("find").is_some());
        store.delete(&record.id).await.expect("deletes");
        assert!(store.find(&record.id).await.expect("find").is_none());
        assert!(!store.root().join("goes-away").exists());
        store
            .delete(&record.id)
            .await
            .expect("deleting twice is not an error");

        // An empty store lists nothing and does not create its own root just by being read.
        let empty = tempfile::tempdir().expect("real tempdir");
        let empty_store = project_store(empty.path());
        assert!(empty_store.ids().await.expect("ids").is_empty());
        let (records, diagnostics) = empty_store.list().await;
        assert!(records.is_empty() && diagnostics.is_empty());
        assert!(
            !empty_store.root().exists(),
            "a read must not create the root"
        );
    }
}
