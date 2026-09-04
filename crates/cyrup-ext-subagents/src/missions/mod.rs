//! `missions` — the durable mission subsystem, a 1:1 port of `pi-subagents/src/missions/`
//! (6 files, 1659 lines @v0.43.0).
//!
//! | cyrup module | upstream file | lines |
//! |---|---|---|
//! | [`types`] | `pi-subagents/src/missions/types.ts` | 157 |
//! | [`store`] | `pi-subagents/src/missions/store.ts` | 507 |
//! | [`workflow_state`] | `pi-subagents/src/missions/workflow-state.ts` | 77 |
//! | [`lifecycle`] | `pi-subagents/src/missions/lifecycle.ts` | 346 |
//! | [`actions`] | `pi-subagents/src/missions/actions.ts` | 410 |
//! | [`goal_driver`] | `pi-subagents/src/missions/goal-driver.ts` | 162 |
//!
//! **`pi-subagents/src/missions/types.ts` is NOT `pi-subagents/src/shared/types.ts`** (nor
//! `src/watchdog/types.ts` — v0.43.0 has exactly those three). They are unrelated files with
//! overlapping names; every citation in this subtree names the full path for that reason. Nothing
//! in `missions/` re-exports anything from `shared/types.ts` except the `Details`/`SubagentRunMode`
//! shapes `lifecycle.ts` reads off a tool result — and in cyrup those arrive as an opaque
//! [`serde_json::Value`] (see [`lifecycle::LaunchOutcome`]).
//!
//! # What the subsystem does
//!
//! A **mission** is a durable objective that outlives any one subagent run. Three flows own it:
//!
//! 1. **Launch binding** ([`lifecycle`]). A `subagent` tool call carrying `mission`/`missionId` —
//!    or, when missions are enabled, ANY execution call with a task — resolves or creates a
//!    mission BEFORE the run starts, and folds the settled run back onto it afterwards (run link,
//!    artifacts, usage, summary, derived mission status). A background run additionally gets a
//!    `mission.json` binding file written into its async dir so the completion path can reconcile
//!    it later, in a different process.
//! 2. **Explicit actions** ([`actions`]). Six `mission.*` tool actions —
//!    `create`/`list`/`show`/`update`/`attach-run`/`close`.
//! 3. **Goal continuation** ([`goal_driver`]). At every turn end, a GOAL mission (one with a token
//!    budget) that is not terminal, has no live run, and has budget left raises a
//!    `needs_attention` control notice naming its next ready action.
//!
//! # Mechanism notes for this port
//!
//! * **Everything here is synchronous.** Upstream's mission store is synchronous `node:fs`, and
//!   the interlock between `createMission` → `writeMission` → `pruneTerminalMissions` →
//!   `listMissions` is much clearer kept that way. The operations are small local-filesystem
//!   reads/writes on the orchestrator's own machine, on the same call path the crate already does
//!   synchronous FS work on (`prompt_runtime.rs`, `spawn/nested_events.rs`).
//! * **`writePrivateAtomicJson` is [`write_private_atomic_json`]**, which delegates its
//!   temp-then-rename to [`crate::background::atomic`] — the crate's designated single owner of
//!   that primitive — rather than growing a second copy.
//! * **No subprocess mechanism is touched.** Nothing in this subtree spawns, signals, or waits on
//!   a child; it only records what the run mechanism already did.

pub mod actions;
pub mod goal_driver;
pub mod lifecycle;
pub mod store;
pub mod types;
pub mod workflow_state;

pub use actions::{
    MISSION_ACTIONS, MissionAction, MissionActionContext, MissionActionOutcome,
    MissionActionParams, MissionLaunchInput, handle_mission_action, validate_mission_launch,
};
pub use goal_driver::{GoalContinuationNotice, RetainedChild, collect_goal_continuation_notices};
pub use lifecycle::{
    LaunchOutcome, MISSION_BINDING_FILE, MissionLaunchBinding, MissionLaunchParams,
    attach_mission_to_launch_result, prepare_mission_launch, read_mission_binding,
    sync_mission_from_async_completion,
};
pub use store::{
    DEFAULT_TERMINAL_MISSION_RETENTION, create_mission, list_global_missions, list_missions,
    mission_record_path, parse_mission_record, read_mission, resolve_mission_store_location,
    update_mission, validate_mission_id, validate_mission_id_str, validate_mission_store_config,
};
pub use types::*;
pub use workflow_state::{
    MISSION_STATE_MAX_BYTES, MissionWorkflowState, create_mission_workflow_state,
    mission_state_path,
};

use std::path::{Path, PathBuf};

/// The mission subsystem's error taxonomy.
///
/// Upstream throws plain `Error`s everywhere except one case: `MissionNotFoundError`
/// (`store.ts:361-372`), which `lifecycle.ts:299` branches on by `instanceof` to decide whether a
/// missing record is a skip-with-a-breadcrumb or a genuine failure. That distinction is the only
/// reason this is an enum rather than a newtype over `String`.
#[derive(Debug, thiserror::Error)]
pub enum MissionError {
    /// pi `MissionNotFoundError` (`store.ts:361-372`), message reproduced verbatim.
    #[error("Mission '{mission_id}' was not found in {}", mission_dir.display())]
    NotFound {
        /// The id that was looked up.
        mission_id: String,
        /// The directory it was looked up in.
        mission_dir: PathBuf,
    },
    /// Every `throw new Error(...)` in the subsystem. The `Display` is the BARE message with no
    /// added prefix: these strings are reproduced from upstream character-for-character and reach
    /// the model as tool-error text, so prefixing them would corrupt observable behaviour (the
    /// same convention [`crate::error::SubagentError::Management`] already follows).
    #[error("{0}")]
    Invalid(String),
    /// A filesystem failure that upstream would have let propagate as a raw Node error.
    #[error("{path}: {source}")]
    Io {
        /// The path being operated on.
        path: String,
        /// The underlying failure.
        source: std::io::Error,
    },
}

impl MissionError {
    /// Build an [`MissionError::Invalid`] from anything string-like.
    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::Invalid(message.into())
    }

    /// Build an [`MissionError::Io`] tagged with the path it happened on.
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_string_lossy().into_owned(),
            source,
        }
    }

    /// `true` for [`Self::NotFound`] — pi's `error instanceof MissionNotFoundError`
    /// (`lifecycle.ts:299`).
    #[must_use]
    pub const fn is_not_found(&self) -> bool {
        matches!(self, Self::NotFound { .. })
    }
}

/// The subsystem's `Result` alias.
pub type MissionResult<T> = Result<T, MissionError>;

/// pi `writePrivateAtomicJson` (`shared/atomic-json.ts:62`,
/// `createAtomicJsonWriter({ mode: 0o600 })`): create the parent directory, write pretty JSON to a
/// uniquely-named temp file in that same directory, `rename` it over the target, and remove the
/// temp file on any failure path.
///
/// The temp-then-rename half is [`crate::background::atomic::write_private_atomic_json_blocking`],
/// the crate's single owner of that primitive; this function exists only to name the upstream
/// symbol it ports and to keep the mission modules free of a `crate::background` import.
///
/// # Errors
///
/// [`MissionError::Io`] if the directory cannot be created, the temp file cannot be written, or
/// the rename does not land within its retry budget.
pub(crate) fn write_private_atomic_json<T: serde::Serialize>(
    path: &Path,
    value: &T,
) -> MissionResult<()> {
    crate::background::atomic::write_private_atomic_json_blocking(path, value)
        .map_err(|err| MissionError::io(path, err))
}

/// `new Date(ms).toISOString()` — reused from [`crate::background::run_status`] rather than
/// reimplemented, so the whole crate renders one ISO-8601 spelling.
pub(crate) fn format_iso8601_millis(ms: i64) -> String {
    crate::background::run_status::format_iso8601_millis(ms)
}
