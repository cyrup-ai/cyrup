//! The persisted schedule record, its run/history/event siblings, and the one parser every read
//! site goes through.
//!
//! Ports pi `ScheduleRecord`/`ScheduleRunRecord`/`ScheduleTrigger`/`ScheduleTarget`
//! (`runs/background/scheduled-runs.ts:40-77` @ `7fe9dee1`), `validateScheduleId` (`:148-151`),
//! `parseScheduleTarget` (`:283-296`) and `parseSchedule` (`:298-313`).
//!
//! # The wire format is ISO-8601 strings, on purpose
//!
//! `createdAt`, `updatedAt`, `plannedAt`, `anchorAt`, `nextRunAt`, `startedAt` and `completedAt`
//! are upstream's `new Date(value).toISOString()` (`:144-146`) — `Z`-normalised strings, not epoch
//! millis. They stay strings here because the trigger half (SUBA-016 part B) sorts and compares
//! `nextRunAt` LEXICOGRAPHICALLY (`:657`'s `localeCompare`), which is only correct for a
//! `Z`-normalised ISO string of fixed width. Minting goes through
//! [`crate::background::run_status::format_iso8601_millis`] over
//! [`crate::time::now_epoch_millis`], so the crate still has exactly one clock; anything that
//! needs an `i64` back converts at the call site rather than adding a second field.
//!
//! # `quiet` landed with part B
//!
//! `v0.68.0`'s `ScheduleRecord` carries a `quiet?: boolean` that the pinned `7fe9dee1` shape does
//! not. Part A left it out because nothing read it; SUBA-016 part B gives it a consumer — see
//! [`ScheduleRecord::quiet`] — so the field is now present, `Option<bool>` (absent and `false`
//! both mean "notify"), and still forward-tolerant: a record written by an older build has no
//! `quiet` key and parses.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::SCHEDULE_VERSION;

/// pi's verbatim `validateScheduleId` refusal (`scheduled-runs.ts:149`).
pub const SCHEDULE_ID_ERROR: &str =
    "Schedule id must be 1-64 characters and contain only letters, numbers, '.', '_', or '-'.";

// =================================================================================================
// The version, as a type
// =================================================================================================

/// The literal `1` of [`SCHEDULE_VERSION`], as a type.
///
/// The same shape as [`crate::background::result_index::IndexVersion`] and
/// `wait_subscriptions`' `SubscriptionVersion`: a `Deserialize` that accepts exactly one value, so
/// "this record is version 1" is a PARSE outcome rather than a field a reader has to remember to
/// check. A future version-2 record deserializes to `Err` here — never a panic, which the crate's
/// `#![deny(clippy::unwrap_used, …)]` block would forbid anyway.
///
/// What a caller then DOES with that `Err` is a store-API decision, not a serde one: see
/// [`super::store::ScheduleStore::list`] for why listing skips-and-reports where
/// [`super::store::ScheduleStore::get`] propagates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScheduleVersion;

impl serde::Serialize for ScheduleVersion {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u32(SCHEDULE_VERSION)
    }
}

impl<'de> serde::Deserialize<'de> for ScheduleVersion {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = u32::deserialize(deserializer)?;
        if raw == SCHEDULE_VERSION {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported schedule record version {raw} (this build reads version \
                 {SCHEDULE_VERSION})"
            )))
        }
    }
}

// =================================================================================================
// ScheduleId — a parsed path component
// =================================================================================================

/// A schedule's identity, and the name of its own directory under the store root.
///
/// pi `SCHEDULE_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/` (`scheduled-runs.ts:35`), enforced by
/// `validateScheduleId` (`:148-151`) at six separate call sites upstream.
///
/// It is a parsed newtype for the same reason [`crate::identity::ResultFileName`] and
/// [`crate::identity::RunDirName`] are: the value names a DIRECTORY, so
/// `root.join(raw_id)` with an unvalidated `&str` is a traversal. Parsing once, here, is what
/// makes `scheduleDir`'s escape checks (`:258-273`) a belt on top of a structural guarantee
/// rather than the only thing standing between `../evil` and the filesystem.
///
/// The leading-character rule is load-bearing twice over: it rejects `.`, `..` and any dotfile,
/// and it rejects a leading `-`, which would otherwise read as a flag to any tool that ever shells
/// out over a schedule directory listing.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ScheduleId(String);

impl ScheduleId {
    /// The only fallible constructor — pi `SCHEDULE_ID`'s regex, spelled out.
    ///
    /// 1 to 64 characters; the first must be ASCII alphanumeric; every subsequent one must be
    /// ASCII alphanumeric, `.`, `_` or `-`. No trimming: upstream does not trim either, and a
    /// value that needs trimming to be legal is a different identifier from the one on disk.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let mut chars = raw.chars();
        let first = chars.next()?;
        if !first.is_ascii_alphanumeric() {
            return None;
        }
        let mut length = 1usize;
        for character in chars {
            if !(character.is_ascii_alphanumeric()
                || character == '.'
                || character == '_'
                || character == '-')
            {
                return None;
            }
            length += 1;
            if length > 64 {
                return None;
            }
        }
        Some(Self(raw.to_string()))
    }

    /// Borrows the identifier for comparison, display and directory construction.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScheduleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`ScheduleId::parse`] — `identity/`'s discipline: a record on disk was
/// written by some other process, possibly a hand edit, so deserialization is exactly where an
/// id that escapes its own directory would otherwise slip in.
impl<'de> serde::Deserialize<'de> for ScheduleId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| serde::de::Error::custom(SCHEDULE_ID_ERROR))
    }
}

// =================================================================================================
// ScheduleRunId — the OTHER path component
// =================================================================================================

/// A fired run's identity, and the stem of its own `runs/<id>.json` file.
///
/// pi mints it with `randomUUID()` (`:843`) and interpolates it straight into a path
/// (`:373`'s `` path.join(dir, "runs", `${run.id}.json`) ``) with no re-validation on read. cyrup
/// parses it, for the reason [`crate::identity::RunDirName`] exists: the value comes back off
/// disk inside a history record that this process did not necessarily write, and a `../` in it
/// would place a "run record" anywhere the process can write.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ScheduleRunId(String);

impl ScheduleRunId {
    /// pi `randomUUID()` (`:843`). Hyphenated, matching the shape upstream writes, so a directory
    /// written here stays readable by an upstream-shaped reader.
    #[must_use]
    pub fn mint() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }

    /// The only fallible constructor: one non-empty path component, never `.` or `..`, never
    /// containing `/`, `\` or NUL, and whose [`Path::file_name`] is itself.
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        if raw.is_empty() || raw == "." || raw == ".." {
            return None;
        }
        if raw.contains('/') || raw.contains('\\') || raw.contains('\0') {
            return None;
        }
        if Path::new(raw).file_name().and_then(std::ffi::OsStr::to_str) != Some(raw) {
            return None;
        }
        Some(Self(raw.to_string()))
    }

    /// Borrows the identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for ScheduleRunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Deserializes THROUGH [`ScheduleRunId::parse`].
impl<'de> serde::Deserialize<'de> for ScheduleRunId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).ok_or_else(|| {
            serde::de::Error::custom("schedule run id must be a single path component")
        })
    }
}

// =================================================================================================
// Policy fields
// =================================================================================================

/// pi `overlap: "skip"` — the ONLY legal value (`parseSchedule:304` rejects anything else).
///
/// A unit-like serde type rather than a `String` or a one-variant enum, so the single legal value
/// is the type and no consumer can write a `ScheduleRecord` carrying a policy the state machine
/// does not implement. When part B grows a second overlap policy this becomes an enum and every
/// match site fails to compile, which is the point.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScheduleOverlapSkip;

impl ScheduleOverlapSkip {
    /// The wire value.
    pub const VALUE: &'static str = "skip";
}

impl serde::Serialize for ScheduleOverlapSkip {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(Self::VALUE)
    }
}

impl<'de> serde::Deserialize<'de> for ScheduleOverlapSkip {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        if raw == Self::VALUE {
            Ok(Self)
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported overlap policy '{raw}' (this build supports only '{}')",
                Self::VALUE
            )))
        }
    }
}

/// pi `catchUp: "none" | "latest"` (`scheduled-runs.ts:55`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScheduleCatchUp {
    /// A missed fire is simply missed.
    #[default]
    None,
    /// The most recent missed fire is caught up on the next tick (`duePlannedAt`, `:409-413`).
    Latest,
}

/// pi `ScheduleRunState` (`scheduled-runs.ts:40`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScheduleRunState {
    /// Launched and not yet settled.
    Running,
    /// Not launched: the previous run was still active and `overlap` is `skip`.
    Skipped,
    /// The fire time passed with nothing watching.
    Missed,
    /// Launched and settled successfully.
    Completed,
    /// The launch call itself failed.
    FailedLaunch,
    /// The launched run failed.
    FailedRun,
}

/// pi `ScheduleRunRecord["dueReason"]` (`scheduled-runs.ts:71`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScheduleDueReason {
    /// An armed timer fired.
    Timer,
    /// A `schedule.run-due` sweep found it due.
    RunDue,
    /// A `schedule.run` action fired it by hand.
    Manual,
}

// =================================================================================================
// Trigger and target
// =================================================================================================

/// pi `ScheduleTrigger` (`scheduled-runs.ts:42-44`) — an externally tagged `kind` union.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ScheduleTrigger {
    /// Fires once, at `at`.
    #[serde(rename_all = "camelCase")]
    Once {
        /// The requested fire time, ISO-8601.
        at: String,
        /// The pending fire time, absent once it has fired.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        next_run_at: Option<String>,
    },
    /// Fires every `every_ms`, anchored at `anchor_at`.
    #[serde(rename_all = "camelCase")]
    Interval {
        /// The authored interval, e.g. `"6h"` — kept verbatim so a re-render shows what was asked
        /// for rather than a millisecond count.
        every: String,
        /// The authored interval in milliseconds.
        every_ms: i64,
        /// The phase the interval is measured from, ISO-8601.
        anchor_at: String,
        /// The next fire time, ISO-8601. Never absent on an interval trigger.
        next_run_at: String,
    },
}

impl ScheduleTrigger {
    /// The pending fire time, for either arm — pi `schedule.trigger.nextRunAt` (`:390`, `:401`).
    #[must_use]
    pub fn next_run_at(&self) -> Option<&str> {
        match self {
            Self::Once { next_run_at, .. } => next_run_at.as_deref(),
            Self::Interval { next_run_at, .. } => Some(next_run_at),
        }
    }

    /// Set the pending fire time — pi's `schedule.trigger.nextRunAt = nextAfter(...)` (`:842`,
    /// `:872`, `:914`, `:940`) and `runManual`'s `= undefined` (`:701`).
    ///
    /// `None` retires a one-shot, which is exactly what a fired or manually-satisfied `at`
    /// schedule wants. An INTERVAL cannot lose its next run — the type says so (`next_run_at` is
    /// a `String` there, not an `Option`), and `next_after` returns `Some` for every interval
    /// trigger, so the `None` arm is unreachable for one and dropping it preserves the invariant
    /// rather than inventing a second "retired interval" state the parser would then have to
    /// accept.
    pub fn set_next_run_at(&mut self, value: Option<String>) {
        match self {
            Self::Once { next_run_at, .. } => *next_run_at = value,
            Self::Interval { next_run_at, .. } => {
                if let Some(value) = value {
                    *next_run_at = value;
                }
            }
        }
    }
}

/// pi `ScheduleTarget` (`scheduled-runs.ts:45` @ `v0.68.0`) — what a fired run executes.
///
/// # `args` is required, and is carried here on purpose
///
/// The `v0.66.0..v0.68.0` window added a REQUIRED `args: Record<string, unknown>` to this type.
/// Part B owns the behaviour that *reads* `args` at launch; this file owns the on-disk shape, and
/// a field added to a persisted record after records exist is a migration rather than a field. It
/// therefore lands now, always serialized (never `skip_serializing_if`), defaulting to `{}`, so a
/// record written today round-trips byte-identically once part B gives it meaning.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleTarget {
    /// pi `workflowScript` — the script this schedule runs. Trimmed at parse, as upstream does.
    pub workflow_script: String,
    /// pi `args` — an always-present plain JSON object, `{}` when the author supplied none.
    #[serde(default)]
    pub args: serde_json::Map<String, serde_json::Value>,
    /// pi `baseRef?` — a validated Git ref for the worktree the run executes in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
}

// =================================================================================================
// The records
// =================================================================================================

/// pi `ScheduleRecord` (`scheduled-runs.ts:47-62` @ `7fe9dee1`) — the persisted schedule.
///
/// `cwd` is stored even though the store is already cwd-keyed: it is the directory a fired run
/// executes in (`executionParams`, `:456`), and the injected-`root` branch's key is a one-way
/// hash, so the record is the only way back to the path.
///
/// `active_run_id`/`last_run_id` are written only by part B's launch/finish state machine but are
/// fields of THIS record — a round-trip that omitted them would not be testing the format part B
/// writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRecord {
    /// Always [`SCHEDULE_VERSION`].
    pub schema_version: ScheduleVersion,
    /// The schedule's id, and the name of its directory.
    pub id: ScheduleId,
    /// The human-readable name shown in listings.
    pub name: String,
    /// The directory a fired run executes in.
    pub cwd: std::path::PathBuf,
    /// When it fires.
    pub trigger: ScheduleTrigger,
    /// What it runs.
    pub target: ScheduleTarget,
    /// Always [`ScheduleOverlapSkip`].
    pub overlap: ScheduleOverlapSkip,
    /// What happens to a fire that was missed.
    pub catch_up: ScheduleCatchUp,
    /// pi `timeoutMs?` — the fired run's deadline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<i64>,
    /// pi `paused` — required, never optional: an absent value would read as "not paused", which
    /// is the dangerous direction.
    pub paused: bool,
    /// pi `sessionOnly?` — the schedule fires only while its owning session is alive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_only: Option<bool>,
    /// pi `quiet?` (`:59` @v0.68.0) — suppress the completion notice this fire would otherwise
    /// deliver into the live session.
    ///
    /// Recurring schedules only (`schedule.create` refuses `quiet` on a one-shot, `:618`), because
    /// a one-shot the user asked for is exactly the fire they want to hear about.
    ///
    /// **Where the flag actually goes.** Upstream's `quiet` rides `executionParams`'
    /// `scheduleOrigin: { id, name?, quiet? }` (`:461`) onto the fired run, and part B does the
    /// same: `executor::scheduled_runs` stamps [`crate::background::ScheduleOrigin::quiet`] on
    /// the run's status, and
    /// [`crate::background::watch::scheduled_completion_triggers_turn`] reads it back.
    ///
    /// The effect is narrow and deliberate: a quiet fire that SUCCEEDS does not wake a turn. Its
    /// failure still does, and either way the run, its status, its result file, its receipt and
    /// this schedule's own history are written exactly as for a loud fire, and the notice is
    /// still DISPLAYED ([`crate::background::watch::completion_notice_display`]) — quiet means
    /// "do not interrupt me", never "do not tell me".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quiet: Option<bool>,
    /// pi `ownerSessionFile?` — the `sessionManager.getSessionFile()` of the creating session.
    /// Required to be present and non-blank when `session_only` is `Some(true)` (`:311`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_session_file: Option<std::path::PathBuf>,
    /// ISO-8601 creation time.
    pub created_at: String,
    /// ISO-8601 last-write time.
    pub updated_at: String,
    /// The run currently holding `active.lock`, if any (part B writes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_run_id: Option<ScheduleRunId>,
    /// The most recently launched run (part B writes it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<ScheduleRunId>,
}

/// pi `ScheduleRunRecord` (`scheduled-runs.ts:64-76`) — one fire of one schedule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleRunRecord {
    /// Always [`SCHEDULE_VERSION`].
    pub schema_version: ScheduleVersion,
    /// This run's id, and the stem of its `runs/<id>.json` file.
    pub id: ScheduleRunId,
    /// The schedule that fired it.
    pub schedule_id: ScheduleId,
    /// The ISO-8601 time it was DUE, which is not the time it started.
    pub planned_at: String,
    /// What made it due.
    pub due_reason: ScheduleDueReason,
    /// Where it got to.
    pub state: ScheduleRunState,
    /// ISO-8601 launch time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// ISO-8601 settle time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    /// The background run this fire attached to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_id: Option<String>,
    /// That run's directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub async_dir: Option<std::path::PathBuf>,
    /// The failure text, for `failed_launch`/`failed_run`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// pi's `history.json` body — `{ schemaVersion: 1, runs: [...] }` (`:365`, `:372`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleHistory {
    /// Always [`SCHEDULE_VERSION`].
    pub schema_version: ScheduleVersion,
    /// Newest first, de-duplicated by run id, capped at [`super::MAX_HISTORY`].
    pub runs: Vec<ScheduleRunRecord>,
}

/// One line of a schedule's `events.jsonl` — pi `:375`/`:381`.
///
/// `run_id`/`state` are present on the `writeRun` line (`:375`) and absent on the bare
/// `appendEvent` line (`:381`), which is why they are optional rather than two types.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleEvent {
    /// Always [`SCHEDULE_VERSION`].
    pub schema_version: ScheduleVersion,
    /// ISO-8601 append time.
    pub timestamp: String,
    /// The event name, e.g. `schedule.run.started`.
    pub event: String,
    /// The schedule this line belongs to.
    pub schedule_id: ScheduleId,
    /// The run this line belongs to, when the line was written for one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<ScheduleRunId>,
    /// That run's state at append time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<ScheduleRunState>,
}

// =================================================================================================
// The parser
// =================================================================================================

/// `Schedule record '<file>' <tail>` — upstream interpolates the full path into every message.
fn record_error(file: &Path, tail: &str) -> String {
    format!("Schedule record '{}' {tail}", file.display())
}

/// Is this a JSON object (and not an array, and not `null`)?
fn plain_object(value: &serde_json::Value) -> Option<&serde_json::Map<String, serde_json::Value>> {
    value.as_object()
}

fn is_string(value: Option<&serde_json::Value>) -> bool {
    value.is_some_and(serde_json::Value::is_string)
}

/// pi `parseScheduleTarget` (`scheduled-runs.ts:283-296`).
///
/// # Errors
///
/// Upstream's four verbatim sentences, plus the two interpolated ones for an invalid `baseRef`
/// and invalid `args`.
pub fn parse_schedule_target(
    value: &serde_json::Value,
    file: &Path,
) -> Result<ScheduleTarget, String> {
    let Some(target) = plain_object(value) else {
        return Err(record_error(file, "has invalid trigger or target."));
    };
    let workflow_script = target
        .get("workflowScript")
        .and_then(serde_json::Value::as_str);
    if let Some(script) = workflow_script.map(str::trim).filter(|s| !s.is_empty()) {
        // pi `normalizeWorktreeBaseRef(target.baseRef)` (`shared/worktree.ts:473-477`), which this
        // crate already owns as `valid_git_ref` + its verbatim refusal constant. An ABSENT key is
        // `undefined` upstream and means "no baseRef"; a present-but-null or non-string value is
        // a refusal, exactly as `typeof value !== "string"` is upstream.
        let base_ref = match target.get("baseRef") {
            None => None,
            Some(raw) => {
                let accepted = raw
                    .as_str()
                    .filter(|value| crate::workflows::scripted::valid_git_ref(value));
                match accepted {
                    Some(value) => Some(value.to_string()),
                    None => {
                        return Err(record_error(
                            file,
                            &format!(
                                "has an invalid baseRef: {}",
                                crate::workflows::scripted::BASE_REF_VALIDATION_ERROR
                            ),
                        ));
                    }
                }
            }
        };
        // pi `normalizeWorkflowArgs(target.args)` (`workflows/workflow-resources.ts:128-138`),
        // which this crate already owns — one implementation of "what a workflow's args may be",
        // not a second one that drifts from it. `deepFreezeWorkflowArgs` has no Rust analogue: the
        // value is owned by the record and handed out by clone.
        let args = crate::workflows::normalize_workflow_args(target.get("args"))
            .map_err(|reason| record_error(file, &format!("has invalid args: {reason}")))?;
        return Ok(ScheduleTarget {
            workflow_script: script.to_string(),
            args,
            base_ref,
        });
    }
    if target.contains_key("agent") || target.contains_key("task") {
        return Err(record_error(
            file,
            "uses a removed legacy agent target; recreate it with target.workflowScript.",
        ));
    }
    Err(record_error(file, "requires a workflowScript target."))
}

/// pi `parseSchedule` (`scheduled-runs.ts:298-313`) — the one read path for a `schedule.json`.
///
/// The field-by-field validation runs over the raw [`serde_json::Value`] in upstream's exact order
/// so every refusal is upstream's exact sentence; the typed decode then runs over the value that
/// passed. A record that clears every upstream check but still fails the typed decode (an
/// `everyMs` that is a float, a `timeoutMs` that is a string — both of which upstream accepts and
/// then breaks on later) is refused here with a cyrup-specific sentence naming the decode failure,
/// because a typed record is the whole reason part B does not have to re-check these.
///
/// # Errors
///
/// One of upstream's verbatim refusal sentences, or the decode message described above.
pub fn parse_schedule(value: &serde_json::Value, file: &Path) -> Result<ScheduleRecord, String> {
    let Some(record) = plain_object(value) else {
        return Err(record_error(file, "must be a JSON object."));
    };
    // `:301` — the required-field gate, in upstream's order.
    if record
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SCHEDULE_VERSION))
        || !is_string(record.get("id"))
        || !is_string(record.get("name"))
        || !is_string(record.get("cwd"))
        || !is_string(record.get("createdAt"))
        || !is_string(record.get("updatedAt"))
        || !record
            .get("paused")
            .is_some_and(serde_json::Value::is_boolean)
    {
        return Err(record_error(file, "has invalid required fields."));
    }
    // `:302` — `validateScheduleId(record.id)`, whose refusal is its own sentence, not the
    // record's.
    let id = record.get("id").and_then(serde_json::Value::as_str);
    if !id.is_some_and(|raw| ScheduleId::parse(raw).is_some()) {
        return Err(SCHEDULE_ID_ERROR.to_string());
    }
    // `:303`
    let (Some(trigger), Some(target)) = (
        record.get("trigger").and_then(plain_object),
        record.get("target"),
    ) else {
        return Err(record_error(file, "has invalid trigger or target."));
    };
    if !target.is_object() {
        return Err(record_error(file, "has invalid trigger or target."));
    }
    // `:304`
    let overlap_ok = record.get("overlap").and_then(serde_json::Value::as_str)
        == Some(ScheduleOverlapSkip::VALUE);
    let catch_up_ok = matches!(
        record.get("catchUp").and_then(serde_json::Value::as_str),
        Some("none" | "latest")
    );
    if !overlap_ok || !catch_up_ok {
        return Err(record_error(file, "has unsupported policy fields."));
    }
    // `:305-309`
    match trigger.get("kind").and_then(serde_json::Value::as_str) {
        Some("once") => {
            let next_ok = match trigger.get("nextRunAt") {
                None | Some(serde_json::Value::Null) => true,
                Some(value) => value.is_string(),
            };
            if !is_string(trigger.get("at")) || !next_ok {
                return Err(record_error(file, "has an invalid one-shot trigger."));
            }
        }
        Some("interval") => {
            if !is_string(trigger.get("every"))
                // cyrup narrows upstream's `typeof === "number"` to an INTEGER count of
                // milliseconds: a float `everyMs` passes upstream's check and then silently
                // produces fractional-millisecond fire times in `nextAfter` (`:396`).
                || trigger.get("everyMs").and_then(serde_json::Value::as_i64).is_none()
                || !is_string(trigger.get("anchorAt"))
                || !is_string(trigger.get("nextRunAt"))
            {
                return Err(record_error(file, "has an invalid interval trigger."));
            }
        }
        _ => return Err(record_error(file, "has an unsupported trigger.")),
    }
    // `:310`
    let session_only = record.get("sessionOnly");
    if session_only.is_some_and(|value| !value.is_boolean() && !value.is_null()) {
        return Err(record_error(file, "has invalid sessionOnly."));
    }
    // `:311` @`v0.68.0` — `quiet` is type-checked in the parser too, with its OWN sentence, and
    // before the owner-session check. Leaving it out would refuse a hand-edited `quiet: "yes"`
    // with the typed decoder's message instead of upstream's, which is the one thing this parser
    // exists to guarantee it never does. `null` is accepted for the same reason `sessionOnly`
    // accepts it: an explicit null decodes to `None`, which is what an absent key means.
    if record
        .get("quiet")
        .is_some_and(|value| !value.is_boolean() && !value.is_null())
    {
        return Err(record_error(file, "has invalid quiet."));
    }
    // `:312`
    if session_only.and_then(serde_json::Value::as_bool) == Some(true) {
        let owner_ok = record
            .get("ownerSessionFile")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| !value.trim().is_empty());
        if !owner_ok {
            return Err(record_error(
                file,
                "is session-only but has no owner session file.",
            ));
        }
    }
    // `:313` — the target is parsed, not merely type-checked.
    let target = parse_schedule_target(target, file)?;
    let mut decoded: ScheduleRecord = serde_json::from_value(value.clone())
        .map_err(|error| record_error(file, &format!("could not be decoded: {error}")))?;
    decoded.target = target;
    Ok(decoded)
}

/// pi `ScheduleStore.history`'s body check (`scheduled-runs.ts:364-366`).
///
/// # Errors
///
/// `Schedule history '<file>' has invalid fields.`, verbatim, for a body that is not
/// `{ schemaVersion: 1, runs: [...] }`, and the decode message for a `runs` element that is not a
/// run record.
pub fn parse_schedule_history(
    value: &serde_json::Value,
    file: &Path,
) -> Result<Vec<ScheduleRunRecord>, String> {
    let invalid = || format!("Schedule history '{}' has invalid fields.", file.display());
    let Some(body) = plain_object(value) else {
        return Err(invalid());
    };
    if body
        .get("schemaVersion")
        .and_then(serde_json::Value::as_u64)
        != Some(u64::from(SCHEDULE_VERSION))
        || !body.get("runs").is_some_and(serde_json::Value::is_array)
    {
        return Err(invalid());
    }
    let history: ScheduleHistory = serde_json::from_value(value.clone()).map_err(|error| {
        format!(
            "Schedule history '{}' could not be decoded: {error}",
            file.display()
        )
    })?;
    Ok(history.runs)
}

// =================================================================================================
// SUBA-016 part B — the two trigger-input parsers, and the epoch <-> ISO boundary
// =================================================================================================

/// pi `timestamp(value)` (`scheduled-runs.ts:145-147`) — `new Date(value).toISOString()`, through
/// the crate's single clock formatter.
#[must_use]
pub fn schedule_timestamp(epoch_millis: i64) -> String {
    crate::background::run_status::format_iso8601_millis(epoch_millis)
}

/// The inverse of [`schedule_timestamp`] — pi's `Date.parse` over the stamps this module writes.
///
/// Accepts `YYYY-MM-DDTHH:MM[:SS[.mmm]]` followed by `Z` or `±HH:MM`, which is exactly the set
/// [`schedule_timestamp`] produces plus the zoned form
/// [`parse_scheduled_run_time`] accepts from a user. `None` is pi's `!Number.isFinite(parsed)`.
///
/// No `chrono`: this crate has one clock ([`crate::time::now_epoch_millis`]) and one formatter
/// ([`crate::background::run_status::format_iso8601_millis`]), and adding a date-time dependency
/// for one inverse would give the crate two sources of truth for what `2026-02-29` means.
#[must_use]
pub fn parse_schedule_timestamp(value: &str) -> Option<i64> {
    let parts = IsoParts::parse(value.trim())?;
    parts.to_epoch_millis()
}

/// The fields of an ISO-8601 stamp, before validation.
struct IsoParts {
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
    millis: i64,
    /// Minutes EAST of UTC; `0` for `Z`.
    offset_minutes: i64,
    /// Whether the zone's own components were in range — upstream's `offsetHour > 23 ||
    /// offsetMinute > 59` (`:129`), which is part of the CALENDAR condition and not of the SHAPE
    /// one. `+25:00` therefore has to reach [`IsoParts::to_epoch_millis`] and be refused there, or
    /// it takes the "not an ISO timestamp at all" branch and answers with the wrong sentence.
    offset_in_range: bool,
}

impl IsoParts {
    /// pi's ISO regex at `:114` — four-digit year, two-digit month/day, `T`, two-digit
    /// hour/minute, an optional `:SS` with an optional one-to-three-digit fraction, and a
    /// MANDATORY `Z` or `+HH:MM`/`-HH:MM` zone — with the fractional part CAPTURED rather than
    /// discarded: upstream drops it in the regex and recovers it from `Date.parse`, which this
    /// port has no equivalent of.
    fn parse(value: &str) -> Option<Self> {
        let bytes = value.as_bytes();
        fn num(bytes: &[u8], at: usize, len: usize) -> Option<i64> {
            let slice = bytes.get(at..at + len)?;
            if !slice.iter().all(u8::is_ascii_digit) {
                return None;
            }
            std::str::from_utf8(slice).ok()?.parse().ok()
        }
        if bytes.len() < 17 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
            return None;
        }
        if bytes.get(10) != Some(&b'T') || bytes.get(13) != Some(&b':') {
            return None;
        }
        let year = num(bytes, 0, 4)?;
        let month = num(bytes, 5, 2)?;
        let day = num(bytes, 8, 2)?;
        let hour = num(bytes, 11, 2)?;
        let minute = num(bytes, 14, 2)?;
        let mut at = 16;
        let mut second = 0;
        let mut millis = 0;
        if bytes.get(at) == Some(&b':') {
            second = num(bytes, at + 1, 2)?;
            at += 3;
            if bytes.get(at) == Some(&b'.') {
                let mut digits = 0;
                while digits < 3 && bytes.get(at + 1 + digits).is_some_and(u8::is_ascii_digit) {
                    digits += 1;
                }
                if digits == 0 {
                    return None;
                }
                // `.5` is 500 ms, `.05` is 50 ms — pi's `\.\d{1,3}` is a DECIMAL FRACTION, so the
                // digits are left-aligned and padded, never right-aligned.
                let mut value = num(bytes, at + 1, digits)?;
                for _ in digits..3 {
                    value *= 10;
                }
                millis = value;
                at += 1 + digits;
            }
        }
        let (offset_minutes, offset_in_range) = match bytes.get(at) {
            Some(&b'Z') if at + 1 == bytes.len() => (0, true),
            Some(sign @ (&b'+' | &b'-')) if at + 6 == bytes.len() => {
                if bytes.get(at + 3) != Some(&b':') {
                    return None;
                }
                let hours = num(bytes, at + 1, 2)?;
                let minutes = num(bytes, at + 4, 2)?;
                let total = hours * 60 + minutes;
                (
                    if *sign == b'+' { total } else { -total },
                    hours <= 23 && minutes <= 59,
                )
            }
            _ => return None,
        };
        Some(Self {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millis,
            offset_minutes,
            offset_in_range,
        })
    }

    /// pi's calendar check (`:127-129`) plus the conversion — `None` when any component is out of
    /// range, which is upstream's `Invalid at value "..." Use a valid ISO timestamp.` condition.
    fn to_epoch_millis(&self) -> Option<i64> {
        if !self.offset_in_range
            || self.month < 1
            || self.month > 12
            || self.day < 1
            || self.day > days_in_month(self.year, self.month)
            || self.hour > 23
            || self.minute > 59
            || self.second > 59
        {
            return None;
        }
        let days = days_from_civil(self.year, self.month, self.day);
        let seconds = days
            .checked_mul(86_400)?
            .checked_add(self.hour * 3_600 + self.minute * 60 + self.second)?;
        seconds
            .checked_mul(1_000)?
            .checked_add(self.millis)?
            .checked_sub(self.offset_minutes.checked_mul(60_000)?)
    }
}

/// pi `new Date(Date.UTC(year, month, 0)).getUTCDate()` (`:127`) — the real length of the month,
/// leap years included.
fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

/// Howard Hinnant's `days_from_civil` — the exact inverse of the `civil_from_days` that
/// [`crate::background::run_status::format_iso8601_millis`] already uses, so a stamp this module
/// writes and reads back is bit-identical. Integer-only and total.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Splits a trimmed trigger input into its digits and its trailing UNIT character.
///
/// `value.split_at(value.len() - 1)` is the obvious spelling and it **panics**: `len()` is bytes,
/// so `"+10é"` splits inside `é` and takes the process's tool call with it. Both callers are fed
/// unvalidated `schedule.create` input, where a stray non-ASCII character is an ordinary typo and
/// must land on a refusal sentence. Splitting at the last CHAR boundary is total — for an empty
/// string it yields two empty halves, which is what the digits-empty guard below already refuses.
fn split_trailing_unit(value: &str) -> (&str, &str) {
    value
        .char_indices()
        .next_back()
        .map_or(("", ""), |(at, _)| value.split_at(at))
}

/// pi `parseScheduledRunTime` (`scheduled-runs.ts:105-131`) — the `at` trigger.
///
/// Two accepted forms and five refusals, all verbatim:
///
/// * `+<n><s|m|h|d>` with `n >= 1`, relative to `now`;
/// * a ZONE-BEARING ISO stamp — the zone is mandatory, because `2026-03-01T09:00` means a
///   different instant to every reader.
///
/// # Errors
///
/// Upstream's five sentences, byte for byte.
pub fn parse_scheduled_run_time(at: &str, now: i64) -> Result<i64, String> {
    let trimmed = at.trim();
    if let Some(rest) = trimmed.strip_prefix('+') {
        let (digits, unit) = split_trailing_unit(rest);
        let unit_ms = match unit {
            "s" => 1_000_i64,
            "m" => 60_000,
            "h" => 3_600_000,
            "d" => 86_400_000,
            _ => {
                return Err(format!(
                    "Invalid at value \"{at}\". Use a one-shot delay such as \"+10m\" or an ISO timestamp with timezone."
                ));
            }
        };
        if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
            return Err(format!(
                "Invalid at value \"{at}\". Use a one-shot delay such as \"+10m\" or an ISO timestamp with timezone."
            ));
        }
        // pi's `!Number.isSafeInteger(amount) || amount < 1` — a value too large to be an exact
        // JS integer takes the SAME branch as a zero, which `i64::from_str`'s overflow reproduces.
        let Ok(amount) = digits.parse::<i64>() else {
            return Err(format!(
                "Invalid at value \"{at}\". Relative delays must be positive, such as \"+10m\"."
            ));
        };
        if amount < 1 {
            return Err(format!(
                "Invalid at value \"{at}\". Relative delays must be positive, such as \"+10m\"."
            ));
        }
        return amount
            .checked_mul(unit_ms)
            .and_then(|delay| now.checked_add(delay))
            .ok_or_else(|| format!("Invalid at value \"{at}\". Relative delay is too large."));
    }
    let Some(parts) = IsoParts::parse(trimmed) else {
        return Err(format!(
            "Invalid at value \"{at}\". Use a one-shot delay such as \"+10m\" or an ISO timestamp with timezone."
        ));
    };
    let Some(parsed) = parts.to_epoch_millis() else {
        return Err(format!(
            "Invalid at value \"{at}\". Use a valid ISO timestamp."
        ));
    };
    if parsed <= now {
        return Err(format!(
            "Scheduled time {} is in the past.",
            schedule_timestamp(parsed)
        ));
    }
    Ok(parsed)
}

/// pi `parseScheduleInterval` (`scheduled-runs.ts:134-142`) — the `every` trigger.
///
/// `m|h|d|w` only. The smallest legal interval is therefore `1m` = 60 000 ms, which is what
/// [`super::trigger::SCHEDULE_TICK`] is chosen against.
///
/// # Errors
///
/// Upstream's three sentences, byte for byte.
pub fn parse_schedule_interval(every: &str) -> Result<i64, String> {
    let trimmed = every.trim();
    let malformed = || {
        format!(
            "Invalid every value \"{every}\". This first recurring slice supports fixed intervals such as \"30m\", \"6h\", \"2d\", or \"2w\"."
        )
    };
    let (digits, unit) = split_trailing_unit(trimmed);
    let unit_ms = match unit {
        "m" => 60_000_i64,
        "h" => 3_600_000,
        "d" => 86_400_000,
        "w" => 604_800_000,
        _ => return Err(malformed()),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(malformed());
    }
    let Ok(amount) = digits.parse::<i64>() else {
        return Err(format!(
            "Invalid every value \"{every}\". Interval must be positive."
        ));
    };
    if amount < 1 {
        return Err(format!(
            "Invalid every value \"{every}\". Interval must be positive."
        ));
    }
    amount
        .checked_mul(unit_ms)
        .ok_or_else(|| format!("Invalid every value \"{every}\". Interval is too large."))
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use std::path::PathBuf;

    use super::*;

    fn file() -> PathBuf {
        PathBuf::from("/p/.cyrup-subagents/schedules/nightly/schedule.json")
    }

    /// A minimal but complete record, as JSON, with `patch` merged over it.
    fn record_json(patch: serde_json::Value) -> serde_json::Value {
        let mut base = serde_json::json!({
            "schemaVersion": 1,
            "id": "nightly",
            "name": "nightly",
            "cwd": "/p",
            "trigger": { "kind": "once", "at": "2026-09-15T06:00:00.000Z" },
            "target": { "workflowScript": "return 1;", "args": {} },
            "overlap": "skip",
            "catchUp": "none",
            "paused": false,
            "createdAt": "2026-09-15T00:00:00.000Z",
            "updatedAt": "2026-09-15T00:00:00.000Z"
        });
        if let (Some(base_map), Some(patch_map)) = (base.as_object_mut(), patch.as_object()) {
            for (key, value) in patch_map {
                if value.is_null() {
                    base_map.remove(key);
                } else {
                    base_map.insert(key.clone(), value.clone());
                }
            }
        }
        base
    }

    fn refusal(patch: serde_json::Value) -> String {
        parse_schedule(&record_json(patch), &file()).expect_err("this fixture must be refused")
    }

    /// §3.3 — a future `schemaVersion` is an ERROR, never a panic, and the sentence is upstream's
    /// (`scheduled-runs.ts:301`).
    const T: i64 = 1_800_000_000_000;

    /// The two accepted `at` forms, and the five refusals — every sentence byte-verbatim from
    /// `parseScheduledRunTime` (`:105-131`). This is the one piece of SUBA-016 with genuinely
    /// fiddly input handling, so it is pinned input by input.
    #[test]
    fn parse_scheduled_run_time_accepts_two_forms_and_refuses_with_upstreams_sentences() {
        // Form 1 — a relative delay, in each of the four units.
        assert_eq!(parse_scheduled_run_time("+30s", T), Ok(T + 30_000));
        assert_eq!(parse_scheduled_run_time("+10m", T), Ok(T + 600_000));
        assert_eq!(parse_scheduled_run_time("+2h", T), Ok(T + 7_200_000));
        assert_eq!(parse_scheduled_run_time("+1d", T), Ok(T + 86_400_000));
        // Trimmed, as upstream trims.
        assert_eq!(parse_scheduled_run_time("  +10m  ", T), Ok(T + 600_000));

        // Form 2 — a ZONE-BEARING ISO stamp. The zone is mandatory because `2026-03-01T09:00`
        // means a different instant to every reader.
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T09:00Z", T),
            parse_schedule_timestamp("2027-01-15T09:00:00.000Z").ok_or(String::new())
        );
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T09:00:30.250Z", T),
            parse_schedule_timestamp("2027-01-15T09:00:30.250Z").ok_or(String::new())
        );
        // An offset is honoured, not ignored: 12:00+02:00 is 10:00Z, two hours EARLIER than the
        // same wall-clock reading in UTC. An implementation that dropped the zone would land on
        // 12:00Z and fire two hours late.
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T12:00:00+02:00", T),
            parse_schedule_timestamp("2027-01-15T10:00:00.000Z").ok_or(String::new())
        );
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T12:00:00-02:00", T),
            parse_schedule_timestamp("2027-01-15T14:00:00.000Z").ok_or(String::new())
        );

        // Refusal 1 — a relative delay of zero.
        assert_eq!(
            parse_scheduled_run_time("+0m", T),
            Err(
                "Invalid at value \"+0m\". Relative delays must be positive, such as \"+10m\"."
                    .to_string()
            )
        );
        // Refusal 2 — a relative delay too large to be an exact integer.
        assert_eq!(
            parse_scheduled_run_time("+99999999999999999999d", T),
            Err(
                "Invalid at value \"+99999999999999999999d\". Relative delays must be positive, such as \"+10m\"."
                    .to_string()
            )
        );
        // Refusal 3 — neither form. A zone-less stamp lands here, which is the point.
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T09:00", T),
            Err(
                "Invalid at value \"2027-01-15T09:00\". Use a one-shot delay such as \"+10m\" or an ISO timestamp with timezone."
                    .to_string()
            )
        );
        assert_eq!(
            parse_scheduled_run_time("tomorrow", T),
            Err(
                "Invalid at value \"tomorrow\". Use a one-shot delay such as \"+10m\" or an ISO timestamp with timezone."
                    .to_string()
            )
        );
        // …and a MULTI-BYTE trailing unit is a refusal, not a panic. `"+10é"` is four bytes and
        // three characters, so splitting the unit off by byte index lands inside `é` and takes
        // the whole tool call down; this is ordinary `schedule.create` input.
        for value in ["+10é", "+10✓", "é", "+é"] {
            assert_eq!(
                parse_scheduled_run_time(value, T),
                Err(format!(
                    "Invalid at value \"{value}\". Use a one-shot delay such as \"+10m\" or an ISO timestamp with timezone."
                )),
                "a non-ASCII unit must be refused, never a panic"
            );
        }
        // Refusal 4 — the shape parses but the CALENDAR does not. 2027 is not a leap year, so
        // the 29th of February is not a day; a naive days-in-month table would accept it.
        assert_eq!(
            parse_scheduled_run_time("2027-02-29T09:00:00Z", T),
            Err(
                "Invalid at value \"2027-02-29T09:00:00Z\". Use a valid ISO timestamp.".to_string()
            )
        );
        assert_eq!(
            parse_scheduled_run_time("2027-13-01T09:00:00Z", T),
            Err(
                "Invalid at value \"2027-13-01T09:00:00Z\". Use a valid ISO timestamp.".to_string()
            )
        );
        // The ZONE is part of the same calendar condition upstream (`:129`'s `offsetHour > 23 ||
        // offsetMinute > 59`), so an out-of-range offset gets the CALENDAR sentence and not the
        // "that is not a timestamp at all" one. A parser that refused it while matching the shape
        // would answer with the wrong half of upstream's contract.
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T09:00:00+25:00", T),
            Err(
                "Invalid at value \"2027-01-15T09:00:00+25:00\". Use a valid ISO timestamp."
                    .to_string()
            )
        );
        assert_eq!(
            parse_scheduled_run_time("2027-01-15T09:00:00-00:70", T),
            Err(
                "Invalid at value \"2027-01-15T09:00:00-00:70\". Use a valid ISO timestamp."
                    .to_string()
            )
        );
        // …and 2028 IS a leap year, so the same day is accepted. Without this half the row above
        // would pass on a table that refuses every 29 February.
        assert!(parse_scheduled_run_time("2028-02-29T09:00:00Z", T).is_ok());
        // Refusal 5 — a valid stamp in the past.
        assert_eq!(
            parse_scheduled_run_time("2020-01-01T00:00:00Z", T),
            Err("Scheduled time 2020-01-01T00:00:00.000Z is in the past.".to_string())
        );
    }

    /// `parseScheduleInterval` (`:134-142`): `m|h|d|w` only, and its three verbatim refusals.
    #[test]
    fn parse_schedule_interval_accepts_four_units_and_refuses_with_upstreams_sentences() {
        assert_eq!(parse_schedule_interval("30m"), Ok(1_800_000));
        assert_eq!(parse_schedule_interval("6h"), Ok(21_600_000));
        assert_eq!(parse_schedule_interval("2d"), Ok(172_800_000));
        assert_eq!(parse_schedule_interval("2w"), Ok(1_209_600_000));
        // The FLOOR the tick is chosen against.
        assert_eq!(parse_schedule_interval("1m"), Ok(60_000));

        let malformed = |value: &str| {
            format!(
                "Invalid every value \"{value}\". This first recurring slice supports fixed intervals such as \"30m\", \"6h\", \"2d\", or \"2w\"."
            )
        };
        // Seconds are NOT a unit — upstream's `at` accepts `s`, its `every` does not, and that
        // asymmetry is what puts the smallest schedulable interval at one minute.
        assert_eq!(parse_schedule_interval("30s"), Err(malformed("30s")));
        assert_eq!(parse_schedule_interval("day"), Err(malformed("day")));
        assert_eq!(parse_schedule_interval("h"), Err(malformed("h")));
        assert_eq!(parse_schedule_interval(""), Err(malformed("")));
        // A multi-byte trailing unit is a refusal, not a panic — splitting the unit off by BYTE
        // index lands inside the character. `every` is raw `schedule.create` input.
        for value in ["6é", "6✓", "é", "日"] {
            assert_eq!(
                parse_schedule_interval(value),
                Err(malformed(value)),
                "a non-ASCII unit must be refused, never a panic"
            );
        }
        assert_eq!(
            parse_schedule_interval("0h"),
            Err("Invalid every value \"0h\". Interval must be positive.".to_string())
        );
        assert_eq!(
            parse_schedule_interval("99999999999999999999w"),
            Err(
                "Invalid every value \"99999999999999999999w\". Interval must be positive."
                    .to_string()
            )
        );
    }

    /// The epoch <-> ISO boundary round-trips, including the fraction, which is where a
    /// hand-rolled parser usually goes wrong: `.5` is 500 ms, not 5.
    #[test]
    fn the_timestamp_converters_are_exact_inverses() {
        for ms in [
            0_i64,
            1_i64,
            T,
            T + 999,
            -86_400_000,
            1_456_704_000_000, // 2016-02-29, a leap day
        ] {
            let rendered = schedule_timestamp(ms);
            assert_eq!(
                parse_schedule_timestamp(&rendered),
                Some(ms),
                "round trip failed for {ms} rendered as {rendered}"
            );
        }
        assert_eq!(
            parse_schedule_timestamp("2027-01-15T09:00:00.5Z"),
            parse_schedule_timestamp("2027-01-15T09:00:00.500Z"),
            "a one-digit fraction is a DECIMAL, so `.5` is 500ms"
        );
        assert_eq!(
            parse_schedule_timestamp("2027-01-15T09:00:00.05Z"),
            parse_schedule_timestamp("2027-01-15T09:00:00.050Z")
        );
        // A round trip only proves the pair AGREE. These pin each half against epoch values
        // neither of them produced, so a shared off-by-one in the civil-date arithmetic — the
        // exact failure a hand-rolled `days_from_civil`/`civil_from_days` pair invites — cannot
        // cancel out and pass.
        for (stamp, millis) in [
            ("2026-09-15T00:00:00.000Z", 1_789_430_400_000_i64),
            ("2027-01-15T09:00:00.000Z", 1_800_003_600_000),
            ("2016-02-29T12:34:56.789Z", 1_456_749_296_789),
            ("1969-12-31T23:59:59.001Z", -999),
        ] {
            assert_eq!(
                parse_schedule_timestamp(stamp),
                Some(millis),
                "{stamp} must parse to the epoch value a calendar says it is"
            );
            assert_eq!(
                schedule_timestamp(millis),
                stamp,
                "and {millis} must render back as exactly that stamp"
            );
        }
        // The zone is SUBTRACTED, pinned against an absolute value rather than against the
        // module's own `Z` rendering of the same instant.
        assert_eq!(
            parse_schedule_timestamp("2027-01-15T12:00:00+02:00"),
            Some(1_800_007_200_000),
            "12:00 at +02:00 is 10:00Z — two hours EARLIER, not later"
        );
        assert_eq!(parse_schedule_timestamp("not a stamp"), None);
        assert_eq!(
            parse_schedule_timestamp("2027-01-15T09:00:00"),
            None,
            "a zone-less stamp is not parseable, here as at `schedule.create`"
        );
    }

    #[test]
    fn a_future_schedule_version_is_refused_by_the_parser_not_a_panic() {
        assert_eq!(
            refusal(serde_json::json!({ "schemaVersion": 2 })),
            "Schedule record '/p/.cyrup-subagents/schedules/nightly/schedule.json' has invalid \
             required fields."
        );
        // And the version TYPE itself refuses, so no read path anywhere can half-decode one.
        assert!(serde_json::from_value::<ScheduleVersion>(serde_json::json!(2)).is_err());
        assert!(serde_json::from_value::<ScheduleVersion>(serde_json::json!(1)).is_ok());
    }

    /// Every upstream refusal sentence, byte-verbatim, in upstream's own check order.
    #[test]
    fn every_upstream_refusal_sentence_is_byte_verbatim() {
        let prefix = "Schedule record '/p/.cyrup-subagents/schedules/nightly/schedule.json'";
        assert_eq!(
            parse_schedule(&serde_json::json!([1, 2]), &file()).unwrap_err(),
            format!("{prefix} must be a JSON object.")
        );
        for missing in ["id", "name", "cwd", "createdAt", "updatedAt", "paused"] {
            assert_eq!(
                refusal(serde_json::json!({ missing: null })),
                format!("{prefix} has invalid required fields."),
                "a missing {missing} must be an invalid-required-fields refusal"
            );
        }
        assert_eq!(
            refusal(serde_json::json!({ "trigger": "soon" })),
            format!("{prefix} has invalid trigger or target.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "target": ["a"] })),
            format!("{prefix} has invalid trigger or target.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "overlap": "queue" })),
            format!("{prefix} has unsupported policy fields.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "catchUp": "all" })),
            format!("{prefix} has unsupported policy fields.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "trigger": { "kind": "once", "at": 7 } })),
            format!("{prefix} has an invalid one-shot trigger.")
        );
        assert_eq!(
            refusal(serde_json::json!({
                "trigger": { "kind": "interval", "every": "6h", "everyMs": "lots",
                             "anchorAt": "2026-09-15T00:00:00.000Z",
                             "nextRunAt": "2026-09-15T06:00:00.000Z" }
            })),
            format!("{prefix} has an invalid interval trigger.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "trigger": { "kind": "cron", "at": "x" } })),
            format!("{prefix} has an unsupported trigger.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "sessionOnly": "yes" })),
            format!("{prefix} has invalid sessionOnly.")
        );
        // `:311` @`v0.68.0`. This sentence was MISSING from the parser: a hand-edited
        // `quiet: "yes"` reached the typed decoder and came back with its message instead of
        // upstream's, which is the one thing this parser exists to guarantee it never does.
        assert_eq!(
            refusal(serde_json::json!({ "quiet": "yes" })),
            format!("{prefix} has invalid quiet.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "quiet": 1 })),
            format!("{prefix} has invalid quiet.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "sessionOnly": true })),
            format!("{prefix} is session-only but has no owner session file.")
        );
        assert_eq!(
            refusal(serde_json::json!({ "sessionOnly": true, "ownerSessionFile": "   " })),
            format!("{prefix} is session-only but has no owner session file."),
            "a blank owner session file is no owner session file"
        );
        assert_eq!(
            refusal(serde_json::json!({ "target": { "agent": "reviewer", "task": "look" } })),
            format!(
                "{prefix} uses a removed legacy agent target; recreate it with \
                 target.workflowScript."
            )
        );
        assert_eq!(
            refusal(serde_json::json!({ "target": { "workflowScript": "   " } })),
            format!("{prefix} requires a workflowScript target.")
        );
    }

    /// `validateScheduleId`'s refusal is its OWN sentence, not the record's (`:149`, `:302`).
    #[test]
    fn an_id_that_escapes_its_directory_is_refused_at_parse() {
        assert_eq!(
            refusal(serde_json::json!({ "id": "../evil" })),
            SCHEDULE_ID_ERROR
        );
        assert_eq!(
            SCHEDULE_ID_ERROR,
            "Schedule id must be 1-64 characters and contain only letters, numbers, '.', '_', or \
             '-'."
        );
    }

    /// pi `SCHEDULE_ID = /^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$/` (`:35`), spelled out.
    #[test]
    fn schedule_id_parse_enforces_the_upstream_grammar() {
        for ok in ["a", "A9", "nightly.sweep", "a_b-c.d", "0", &"z".repeat(64)] {
            assert!(ScheduleId::parse(ok).is_some(), "{ok} should be accepted");
        }
        for bad in [
            "",
            ".",
            "..",
            "../evil",
            "a/b",
            "a\\b",
            ".hidden",
            "-flag",
            "_lead",
            "a b",
            "schedule\0",
            &"z".repeat(65),
        ] {
            assert!(
                ScheduleId::parse(bad).is_none(),
                "{bad:?} should be refused"
            );
            assert!(
                serde_json::from_value::<ScheduleId>(serde_json::Value::from(bad)).is_err(),
                "{bad:?} must not deserialize either — on-disk data is not trusted",
            );
        }
    }

    /// A run id names `runs/<id>.json`, so it is a single path component or it is nothing.
    #[test]
    fn a_run_id_must_be_a_single_path_component() {
        assert!(ScheduleRunId::parse(ScheduleRunId::mint().as_str()).is_some());
        for bad in ["", ".", "..", "../escape", "a/b", "a\\b", "x\0y"] {
            assert!(
                ScheduleRunId::parse(bad).is_none(),
                "{bad:?} should be refused"
            );
            assert!(serde_json::from_value::<ScheduleRunId>(serde_json::Value::from(bad)).is_err());
        }
    }

    /// `overlap` has exactly one legal value (`parseSchedule:304`), and it is the TYPE.
    #[test]
    fn the_overlap_policy_has_exactly_one_legal_value() {
        assert_eq!(
            serde_json::to_value(ScheduleOverlapSkip).unwrap(),
            serde_json::Value::from("skip")
        );
        assert!(serde_json::from_value::<ScheduleOverlapSkip>(serde_json::json!("skip")).is_ok());
        assert!(serde_json::from_value::<ScheduleOverlapSkip>(serde_json::json!("queue")).is_err());
    }

    /// `baseRef` goes through this crate's existing `valid_git_ref`, with its verbatim refusal
    /// interpolated into upstream's `has an invalid baseRef:` sentence (`:288`).
    #[test]
    fn an_invalid_base_ref_is_refused_and_a_valid_one_survives() {
        let prefix = "Schedule record '/p/.cyrup-subagents/schedules/nightly/schedule.json'";
        assert_eq!(
            refusal(serde_json::json!({
                "target": { "workflowScript": "return 1;", "baseRef": "refs/heads/../evil" }
            })),
            format!(
                "{prefix} has an invalid baseRef: {}",
                crate::workflows::scripted::BASE_REF_VALIDATION_ERROR
            )
        );
        assert_eq!(
            refusal(serde_json::json!({
                "target": { "workflowScript": "return 1;", "baseRef": 7 }
            })),
            format!(
                "{prefix} has an invalid baseRef: {}",
                crate::workflows::scripted::BASE_REF_VALIDATION_ERROR
            ),
            "a non-string baseRef is upstream's `typeof value !== \"string\"` arm"
        );
        let parsed = parse_schedule(
            &record_json(serde_json::json!({
                "target": { "workflowScript": "return 1;", "baseRef": "refs/heads/main" }
            })),
            &file(),
        )
        .expect("a valid baseRef survives");
        assert_eq!(parsed.target.base_ref.as_deref(), Some("refs/heads/main"));
    }

    /// `args` is validated by the SAME normaliser the workflow runtime uses, so a schedule can
    /// never persist args the runtime would then refuse.
    #[test]
    fn invalid_args_are_refused_by_the_shared_workflow_normaliser() {
        let prefix = "Schedule record '/p/.cyrup-subagents/schedules/nightly/schedule.json'";
        assert_eq!(
            refusal(serde_json::json!({
                "target": { "workflowScript": "return 1;", "args": [1, 2] }
            })),
            format!("{prefix} has invalid args: workflow args must be a plain JSON object.")
        );
    }

    /// §3.1 — `args` is REQUIRED on the wire: absent on the way in becomes `{}`, and `{}` is
    /// always written back out. A `skip_serializing_if` here would make a record written today
    /// differ from the one part B expects tomorrow.
    #[test]
    fn args_default_to_an_empty_object_and_always_serialize() {
        let parsed = parse_schedule(
            &record_json(serde_json::json!({ "target": { "workflowScript": "return 1;" } })),
            &file(),
        )
        .expect("an absent args is {}");
        assert!(parsed.target.args.is_empty());

        let written = serde_json::to_value(&parsed).expect("serializes");
        assert_eq!(
            written["target"]["args"],
            serde_json::json!({}),
            "args must be present on the wire even when empty"
        );

        let with_args = parse_schedule(
            &record_json(serde_json::json!({
                "target": { "workflowScript": "return 1;", "args": { "task": "sweep" } }
            })),
            &file(),
        )
        .expect("args survive");
        assert_eq!(
            with_args.target.args.get("task"),
            Some(&serde_json::Value::from("sweep"))
        );
    }

    /// Two different properties that the one row this replaces conflated.
    ///
    /// `quiet` is no longer an unknown field — this build declares it — so asserting forward
    /// tolerance THROUGH it proved nothing about forward tolerance and nothing about `quiet`. It
    /// is now pinned as what it is: a parsed, round-tripping field. Forward tolerance is asserted
    /// separately, with a key no build has ever declared.
    #[test]
    fn quiet_parses_into_its_field_and_a_genuinely_unknown_field_is_tolerated() {
        let parsed = parse_schedule(&record_json(serde_json::json!({ "quiet": true })), &file())
            .expect("a declared quiet parses");
        assert_eq!(parsed.id.as_str(), "nightly");
        assert_eq!(
            parsed.quiet,
            Some(true),
            "quiet is READ, not merely allowed"
        );
        assert_eq!(
            serde_json::to_value(&parsed).expect("serializes")["quiet"],
            serde_json::Value::Bool(true),
            "and it survives the write back out"
        );

        // Absent stays absent — `Option<bool>` with `skip_serializing_if`, so a record written by
        // this build is byte-identical to one written before the field existed.
        let loud = parse_schedule(&record_json(serde_json::json!({})), &file()).expect("parses");
        assert_eq!(loud.quiet, None);
        assert!(
            serde_json::to_value(&loud).expect("serializes")["quiet"].is_null(),
            "an absent quiet is absent on the wire, not `false`"
        );

        // Forward tolerance, asserted against a key this build has never heard of.
        let from_the_future = parse_schedule(
            &record_json(serde_json::json!({ "scheduleOrigin": { "tier": "gold" } })),
            &file(),
        )
        .expect("an unknown field is ignored, not refused");
        assert_eq!(from_the_future.id.as_str(), "nightly");
    }

    /// The history body check (`:364-366`), verbatim.
    #[test]
    fn a_history_body_must_be_versioned_and_an_array() {
        let path = PathBuf::from("/p/.cyrup-subagents/schedules/nightly/history.json");
        assert_eq!(
            parse_schedule_history(
                &serde_json::json!({ "schemaVersion": 2, "runs": [] }),
                &path
            )
            .unwrap_err(),
            "Schedule history '/p/.cyrup-subagents/schedules/nightly/history.json' has invalid \
             fields."
        );
        assert_eq!(
            parse_schedule_history(
                &serde_json::json!({ "schemaVersion": 1, "runs": {} }),
                &path
            )
            .unwrap_err(),
            "Schedule history '/p/.cyrup-subagents/schedules/nightly/history.json' has invalid \
             fields."
        );
        assert_eq!(
            parse_schedule_history(
                &serde_json::json!({ "schemaVersion": 1, "runs": [] }),
                &path
            )
            .expect("an empty history is legal")
            .len(),
            0
        );
    }

    /// The trigger union is externally tagged on `kind`, camelCase on both arms, and
    /// [`ScheduleTrigger::next_run_at`] reads either.
    #[test]
    fn the_trigger_union_round_trips_in_upstreams_wire_shape() {
        let once = ScheduleTrigger::Once {
            at: "2026-09-15T06:00:00.000Z".to_string(),
            next_run_at: Some("2026-09-15T06:00:00.000Z".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&once).unwrap(),
            serde_json::json!({
                "kind": "once",
                "at": "2026-09-15T06:00:00.000Z",
                "nextRunAt": "2026-09-15T06:00:00.000Z"
            })
        );
        assert_eq!(once.next_run_at(), Some("2026-09-15T06:00:00.000Z"));

        let interval = ScheduleTrigger::Interval {
            every: "6h".to_string(),
            every_ms: 21_600_000,
            anchor_at: "2026-09-15T00:00:00.000Z".to_string(),
            next_run_at: "2026-09-15T06:00:00.000Z".to_string(),
        };
        let wire = serde_json::to_value(&interval).unwrap();
        assert_eq!(wire["kind"], "interval");
        assert_eq!(wire["everyMs"], 21_600_000);
        assert_eq!(wire["anchorAt"], "2026-09-15T00:00:00.000Z");
        assert_eq!(
            serde_json::from_value::<ScheduleTrigger>(wire).unwrap(),
            interval
        );

        let pending = ScheduleTrigger::Once {
            at: "2026-09-15T06:00:00.000Z".to_string(),
            next_run_at: None,
        };
        assert_eq!(pending.next_run_at(), None);
        assert_eq!(
            serde_json::to_value(&pending).unwrap(),
            serde_json::json!({ "kind": "once", "at": "2026-09-15T06:00:00.000Z" }),
            "an absent nextRunAt is absent on the wire, as upstream writes it"
        );
    }

    /// The run/event enums use upstream's exact wire spellings — `failed_launch` is snake, and
    /// `run-due` is kebab, in the same file.
    #[test]
    fn the_run_state_and_due_reason_wire_spellings_are_upstreams() {
        assert_eq!(
            serde_json::to_value(ScheduleRunState::FailedLaunch).unwrap(),
            "failed_launch"
        );
        assert_eq!(
            serde_json::to_value(ScheduleRunState::FailedRun).unwrap(),
            "failed_run"
        );
        assert_eq!(
            serde_json::to_value(ScheduleDueReason::RunDue).unwrap(),
            "run-due"
        );
        assert_eq!(
            serde_json::to_value(ScheduleCatchUp::Latest).unwrap(),
            "latest"
        );
    }
}
