//! The seven `mission.*` tool actions — a 1:1 port of `pi-subagents/src/missions/actions.ts`
//! (410 lines @v0.43.0; the seventh verb, `mission.resolve-decision`, is `:391-397` @v0.64.0 and
//! entered at v0.47.1 — SUBA-085).
//!
//! [`MISSION_ACTIONS`] is the dispatch vocabulary
//! (`mission.create`/`list`/`show`/`update`/`resolve-decision`/`attach-run`/`close`,
//! `actions.ts:32-40` @v0.64.0), and [`handle_mission_action`] is the single entry point
//! upstream's `runs/foreground/subagent-executor.ts:5723-5732` @v0.64.0 routes all seven through.
//!
//! Three groups of code live here:
//!
//! 1. **Untrusted-input validators** — [`validate_mission_launch`] (`actions.ts:106-132`, also
//!    called from [`super::lifecycle::prepare_mission_launch`]), plus the private
//!    `validate_mission_update`/`validate_artifact`/`validate_receipt` trio. These are STRICTER
//!    than the store's own parsers: they reject unknown keys, which the store's parsers do not,
//!    because their input is a raw model-authored tool argument rather than a file this subsystem
//!    itself wrote.
//! 2. **`refresh_linked_run_status`** (`actions.ts:236-284`) — `mission.show`'s
//!    read-through refresh: every linked run with an `asyncDir` has its `status.json` re-read, and
//!    the mission's own status is re-derived from the resulting set of run states. This is the one
//!    place a mission's status changes without an explicit update.
//! 3. **`format_mission`** (`actions.ts:286-314`) — the human-facing rendering, reproduced
//!    line-for-line (it is what the model reads back).
//!
//! # Result shape
//!
//! Upstream returns an `AgentToolResult<Details>`. cyrup's `ToolResult` carries no `isError` flag
//! (a failed tool call is an `Err(ToolError)`, per the crate-wide convention `extension.rs`
//! documents at `route_management_action`), so this module returns
//! [`MissionResult<MissionActionOutcome>`](super::MissionResult): the `Ok` arm carries upstream's
//! `{ content, details }` pair verbatim, and every upstream `throw` becomes the `Err` arm carrying
//! that throw's exact message. `extension.rs` performs the one-line conversion at the call site.

use std::path::Path;

use serde_json::Value;

use super::store::{
    create_mission, is_absolute_url, list_global_missions, list_missions, mission_record_path,
    mission_status_list, read_mission, resolve_mission_store_location, update_mission,
    validate_mission_id_str,
};
use super::workflow_state::mission_state_path;
use super::{
    MissionArtifact, MissionArtifactKind, MissionCreateInput, MissionDecisionInput,
    MissionDecisionResolution, MissionDecisionStatus, MissionError, MissionGoal, MissionGoalStatus,
    MissionGoalUpdate, MissionReceiptInput, MissionReceiptKind, MissionReceiptStatus,
    MissionRecord, MissionResult, MissionRunLink, MissionRunMode, MissionStatus,
    MissionStoreConfig, MissionStoreLocation, MissionTokenBudget, MissionUpdateInput,
};

/// pi `MISSION_ACTIONS` (`actions.ts:32-40` @v0.64.0) — the exact seven-action vocabulary, in
/// upstream's order. `mission.resolve-decision` (SUBA-085) sits between `update` and
/// `attach-run`, where upstream declares it.
pub const MISSION_ACTIONS: [&str; 7] = [
    "mission.create",
    "mission.list",
    "mission.show",
    "mission.update",
    "mission.resolve-decision",
    "mission.attach-run",
    "mission.close",
];

/// The five mission actions that appear in `subagent-executor.ts:197` @v0.64.0's
/// `MUTATING_MANAGEMENT_ACTIONS` set — the ones a child-safe fanout child may not perform.
/// `mission.list`/`mission.show` are read-only and are NOT in it.
pub const MUTATING_MISSION_ACTIONS: [&str; 5] = [
    "mission.create",
    "mission.update",
    "mission.resolve-decision",
    "mission.attach-run",
    "mission.close",
];

/// pi `MissionAction` (`actions.ts:41`) — a validated member of [`MISSION_ACTIONS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MissionAction {
    /// `mission.create`
    Create,
    /// `mission.list`
    List,
    /// `mission.show`
    Show,
    /// `mission.update`
    Update,
    /// `mission.resolve-decision` (SUBA-085, `actions.ts:391-397` @v0.64.0)
    ResolveDecision,
    /// `mission.attach-run`
    AttachRun,
    /// `mission.close`
    Close,
}

impl MissionAction {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "mission.create",
            Self::List => "mission.list",
            Self::Show => "mission.show",
            Self::Update => "mission.update",
            Self::ResolveDecision => "mission.resolve-decision",
            Self::AttachRun => "mission.attach-run",
            Self::Close => "mission.close",
        }
    }

    /// pi's `(MISSION_ACTIONS as readonly string[]).includes(action)` gate
    /// (`subagent-executor.ts:4397`): `None` means "not a mission action, fall through to the rest
    /// of the dispatch table".
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "mission.create" => Some(Self::Create),
            "mission.list" => Some(Self::List),
            "mission.show" => Some(Self::Show),
            "mission.update" => Some(Self::Update),
            "mission.resolve-decision" => Some(Self::ResolveDecision),
            "mission.attach-run" => Some(Self::AttachRun),
            "mission.close" => Some(Self::Close),
            _ => None,
        }
    }

    /// Membership in [`MUTATING_MISSION_ACTIONS`].
    #[must_use]
    pub const fn is_mutating(self) -> bool {
        !matches!(self, Self::List | Self::Show)
    }
}

/// pi `MissionLaunchInput` (`actions.ts:43-49`) — the validated `mission` tool parameter.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MissionLaunchInput {
    /// From `title`, or its `summary` alias. Trimmed, non-empty.
    pub title: String,
    /// Trimmed, non-empty when present.
    pub objective: Option<String>,
    /// `true` only — upstream refuses any other value (`actions.ts:116`).
    pub goal: bool,
    /// Required when [`Self::goal`] is set.
    pub budget: Option<MissionTokenBudget>,
    /// Trimmed labels.
    pub labels: Option<Vec<String>>,
}

/// pi `MissionActionParams` (`actions.ts:69-82`) — the subset of the `subagent` tool's parameter
/// surface the six mission actions read. Every field is the RAW tool argument; validation happens
/// inside [`handle_mission_action`].
#[derive(Clone, Debug, Default)]
pub struct MissionActionParams {
    /// `missionId` — the target mission for show/update/attach-run/close.
    pub mission_id: Option<String>,
    /// `mission` — the launch/create object.
    pub mission: Option<Value>,
    /// `missionUpdate` — the update object.
    pub mission_update: Option<Value>,
    /// `missionStatus` — the create/close status.
    pub mission_status: Option<String>,
    /// `missionScope` — `"project"` (default) or `"global"` for `mission.list`.
    pub mission_scope: Option<String>,
    /// `id` — the `mission.attach-run` run id, when `runId` is absent; the DECISION id for
    /// `mission.resolve-decision` (`actions.ts:393` @v0.64.0).
    pub id: Option<String>,
    /// `runId` — the `mission.attach-run` run id (preferred over `id`).
    pub run_id: Option<String>,
    /// `dir` — the attached run's async dir.
    pub dir: Option<String>,
    /// `runMode` — the attached run's mode; defaults to `external`.
    pub run_mode: Option<String>,
    /// `runStatus` — the attached run's status string.
    pub run_status: Option<String>,
    /// `agent` — the attached run's agent.
    pub agent: Option<String>,
    /// `summary` — the `mission.close` summary; the RESOLUTION text for
    /// `mission.resolve-decision` (`actions.ts:394-395` @v0.64.0).
    pub summary: Option<String>,
}

/// pi's `MissionActionContext` (`actions.ts:84-89`).
#[derive(Clone, Debug, Default)]
pub struct MissionActionContext {
    /// The resolved execution cwd, used as the project root.
    pub cwd: std::path::PathBuf,
    /// The live session id, stamped onto a created mission as its owner.
    pub current_session_id: Option<String>,
    /// The `config.missions` block.
    pub config: Option<MissionStoreConfig>,
    /// An explicit agent-dir override for the global pointer index.
    pub agent_dir: Option<std::path::PathBuf>,
}

/// The `{ content, details }` pair upstream's `textResult` (`actions.ts:91-93`) builds. See this
/// module's "Result shape" note for why this is not a `cyrup_core::ToolResult`.
#[derive(Clone, Debug, PartialEq)]
pub struct MissionActionOutcome {
    /// The single text content part.
    pub text: String,
    /// The `Details` object — always `{ mode: "management", results: [], … }`.
    pub details: Value,
}

/// pi `requireMissionId` (`actions.ts:95-97`).
fn require_mission_id(params: &MissionActionParams) -> MissionResult<String> {
    match params.mission_id.as_deref() {
        Some(id) => validate_mission_id_str(id, "missionId"),
        None => Err(MissionError::invalid(
            "missionId must be a non-empty string",
        )),
    }
}

/// pi `validateStatus` (`actions.ts:99-104`).
fn validate_status(value: Option<&str>, label: &str) -> MissionResult<MissionStatus> {
    value.and_then(MissionStatus::from_wire).ok_or_else(|| {
        MissionError::invalid(format!("{label} must be one of {}", mission_status_list()))
    })
}

/// pi `validateMissionLaunch` (`actions.ts:106-132`) — the `mission` tool parameter validator.
///
/// Shared with [`super::lifecycle::prepare_mission_launch`] (`lifecycle.ts:75`), which is why it
/// is `pub`.
///
/// # Errors
///
/// [`MissionError::Invalid`] with upstream's exact text for an unknown key, a `title`/`summary`
/// conflict, a missing/blank title, a non-`true` `goal`, a non-positive `budget.tokens`, a
/// `goal: true` with no budget, or a non-string label.
pub fn validate_mission_launch(value: &Value) -> MissionResult<MissionLaunchInput> {
    let input = value
        .as_object()
        .ok_or_else(|| MissionError::invalid("mission must be an object"))?;
    for key in input.keys() {
        if !matches!(
            key.as_str(),
            "title" | "summary" | "objective" | "goal" | "budget" | "labels"
        ) {
            return Err(MissionError::invalid(format!("mission.{key} is unknown")));
        }
    }
    if input.contains_key("title") && input.contains_key("summary") {
        return Err(MissionError::invalid(
            "mission.title and mission.summary cannot both be set",
        ));
    }
    let title = input
        .get("title")
        .or_else(|| input.get("summary"))
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            MissionError::invalid("mission.title or mission.summary must be a non-empty string")
        })?;
    let objective = match input.get("objective") {
        None => None,
        Some(v) => Some(v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            MissionError::invalid("mission.objective must be a non-empty string")
        })?),
    };
    let goal = match input.get("goal") {
        None => false,
        Some(Value::Bool(true)) => true,
        Some(_) => {
            return Err(MissionError::invalid(
                "mission.goal must be true when supplied",
            ));
        }
    };
    let budget = match input.get("budget") {
        None => None,
        Some(v) => Some(MissionTokenBudget {
            tokens: positive_tokens(v, "mission.budget")?,
        }),
    };
    if goal && budget.is_none() {
        return Err(MissionError::invalid(
            "mission.budget is required when mission.goal is true",
        ));
    }
    let labels = match input.get("labels") {
        None => None,
        Some(v) => Some(validate_label_array(v, "mission.labels", true)?),
    };
    Ok(MissionLaunchInput {
        title: title.trim().to_string(),
        objective: objective.map(|o| o.trim().to_string()),
        goal,
        budget,
        labels,
    })
}

/// The `{ tokens: positive safe integer }` shape both `mission.budget` (`actions.ts:117-120`) and
/// `missionUpdate.budget` (`actions.ts:198-203`) demand, with the caller's own label.
fn positive_tokens(value: &Value, label: &str) -> MissionResult<u64> {
    let tokens = value
        .as_object()
        .and_then(|o| o.get("tokens"))
        .and_then(Value::as_i64)
        .filter(|n| *n >= 1 && n.abs() <= 9_007_199_254_740_991);
    tokens
        .map(i64::unsigned_abs)
        .ok_or_else(|| MissionError::invalid(format!("{label}.tokens must be a positive integer")))
}

/// The `labels` array check both `mission.labels` (`actions.ts:122-124`) and
/// `missionUpdate.labels` (`actions.ts:205-208`) apply.
///
/// `trim` is not cosmetic: the LAUNCH validator returns `input.labels.map((label) => label.trim())`
/// (`actions.ts:130`) while the UPDATE validator returns the array UNTRIMMED
/// (`actions.ts:207`, `update.labels = input.labels as string[]`) and lets the store's own
/// `stringArray` trim it a layer down. Both end up trimmed on disk; the difference is observable
/// only in the intermediate value, which is exactly the kind of detail a "close enough" port loses.
fn validate_label_array(value: &Value, label: &str, trim: bool) -> MissionResult<Vec<String>> {
    let items = value.as_array().ok_or_else(|| {
        MissionError::invalid(format!("{label} must contain only non-empty strings"))
    })?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        let s = item
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                MissionError::invalid(format!("{label} must contain only non-empty strings"))
            })?;
        out.push(if trim {
            s.trim().to_string()
        } else {
            s.to_string()
        });
    }
    Ok(out)
}

/// pi `validateArtifact` (`actions.ts:134-146`).
fn validate_artifact(value: &Value, index: usize) -> MissionResult<MissionArtifact> {
    let input = value.as_object().ok_or_else(|| {
        MissionError::invalid(format!(
            "missionUpdate.artifacts[{index}] must be an object"
        ))
    })?;
    let kind = input
        .get("kind")
        .and_then(Value::as_str)
        .and_then(MissionArtifactKind::from_wire)
        .ok_or_else(|| {
            MissionError::invalid(format!("missionUpdate.artifacts[{index}].kind is invalid"))
        })?;
    let path = input
        .get("path")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            MissionError::invalid(format!(
                "missionUpdate.artifacts[{index}].path must be a non-empty string"
            ))
        })?;
    let description = match input.get("description") {
        None => None,
        Some(v) => Some(v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            MissionError::invalid(format!(
                "missionUpdate.artifacts[{index}].description must be a non-empty string"
            ))
        })?),
    };
    Ok(MissionArtifact {
        kind,
        path: path.to_string(),
        description: description.map(str::to_string),
    })
}

/// pi `validateReceipt` (`actions.ts:148-174`). Note this one trims `title`/`url`/`description`,
/// unlike [`validate_artifact`], which does not trim `path`.
fn validate_receipt(value: &Value, index: usize) -> MissionResult<MissionReceiptInput> {
    let input = value.as_object().ok_or_else(|| {
        MissionError::invalid(format!("missionUpdate.receipts[{index}] must be an object"))
    })?;
    for key in input.keys() {
        if !matches!(
            key.as_str(),
            "kind" | "status" | "title" | "url" | "description"
        ) {
            return Err(MissionError::invalid(format!(
                "missionUpdate.receipts[{index}].{key} is unknown"
            )));
        }
    }
    let kind = input
        .get("kind")
        .and_then(Value::as_str)
        .and_then(MissionReceiptKind::from_wire)
        .ok_or_else(|| {
            MissionError::invalid(format!("missionUpdate.receipts[{index}].kind is invalid"))
        })?;
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .and_then(MissionReceiptStatus::from_wire)
        .ok_or_else(|| {
            MissionError::invalid(format!("missionUpdate.receipts[{index}].status is invalid"))
        })?;
    let mut fields = Vec::with_capacity(2);
    for field in ["title", "url"] {
        let s = input
            .get(field)
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                MissionError::invalid(format!(
                    "missionUpdate.receipts[{index}].{field} must be a non-empty string"
                ))
            })?;
        fields.push(s);
    }
    let (title, url) = (
        fields.first().copied().unwrap_or_default(),
        fields.get(1).copied().unwrap_or_default(),
    );
    if !is_absolute_url(url) {
        return Err(MissionError::invalid(format!(
            "missionUpdate.receipts[{index}].url must be an absolute URL"
        )));
    }
    let description = match input.get("description") {
        None => None,
        Some(v) => Some(v.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
            MissionError::invalid(format!(
                "missionUpdate.receipts[{index}].description must be a non-empty string"
            ))
        })?),
    };
    Ok(MissionReceiptInput {
        kind,
        status,
        title: title.trim().to_string(),
        url: url.trim().to_string(),
        description: description.map(|d| d.trim().to_string()),
    })
}

/// pi `validateMissionUpdate` (`actions.ts:176-234`).
fn validate_mission_update(value: Option<&Value>) -> MissionResult<MissionUpdateInput> {
    let value = value.ok_or_else(|| MissionError::invalid("missionUpdate must be an object"))?;
    let input = value
        .as_object()
        .ok_or_else(|| MissionError::invalid("missionUpdate must be an object"))?;
    for key in input.keys() {
        if !matches!(
            key.as_str(),
            "title"
                | "objective"
                | "goal"
                | "budget"
                | "status"
                | "summary"
                | "labels"
                | "artifacts"
                | "receipts"
                | "decisions"
        ) {
            return Err(MissionError::invalid(format!(
                "missionUpdate.{key} is unknown"
            )));
        }
    }
    let mut update = MissionUpdateInput::default();
    for field in ["title", "objective", "summary"] {
        let Some(candidate) = input.get(field) else {
            continue;
        };
        let trimmed = candidate
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or_else(|| {
                MissionError::invalid(format!("missionUpdate.{field} must be a non-empty string"))
            })?
            .trim()
            .to_string();
        match field {
            "title" => update.title = Some(trimmed),
            "objective" => update.objective = Some(trimmed),
            _ => update.summary = Some(trimmed),
        }
    }
    if let Some(goal) = input.get("goal") {
        update.goal = Some(match goal {
            Value::Bool(true) => MissionGoalUpdate::Set(MissionGoal {
                status: MissionGoalStatus::Active,
            }),
            Value::Bool(false) => MissionGoalUpdate::Disable,
            Value::Object(map)
                if map.len() == 1 && matches!(map.get("paused"), Some(Value::Bool(_))) =>
            {
                MissionGoalUpdate::Set(MissionGoal {
                    status: if map.get("paused") == Some(&Value::Bool(true)) {
                        MissionGoalStatus::Paused
                    } else {
                        MissionGoalStatus::Active
                    },
                })
            }
            _ => {
                return Err(MissionError::invalid(
                    "missionUpdate.goal must be boolean or { paused: boolean }",
                ));
            }
        });
    }
    if let Some(budget) = input.get("budget") {
        update.budget = Some(MissionTokenBudget {
            tokens: positive_tokens(budget, "missionUpdate.budget")?,
        });
    }
    if let Some(status) = input.get("status") {
        update.status = Some(validate_status(status.as_str(), "missionUpdate.status")?);
    }
    if let Some(labels) = input.get("labels") {
        update.labels = Some(validate_label_array(labels, "missionUpdate.labels", false)?);
    }
    if let Some(artifacts) = input.get("artifacts") {
        let items = artifacts
            .as_array()
            .ok_or_else(|| MissionError::invalid("missionUpdate.artifacts must be an array"))?;
        update.add_artifacts = items
            .iter()
            .enumerate()
            .map(|(index, item)| validate_artifact(item, index))
            .collect::<MissionResult<Vec<_>>>()?;
    }
    if let Some(receipts) = input.get("receipts") {
        let items = receipts
            .as_array()
            .ok_or_else(|| MissionError::invalid("missionUpdate.receipts must be an array"))?;
        update.add_receipts = items
            .iter()
            .enumerate()
            .map(|(index, item)| validate_receipt(item, index))
            .collect::<MissionResult<Vec<_>>>()?;
    }
    if let Some(decisions) = input.get("decisions") {
        let items = decisions
            .as_array()
            .ok_or_else(|| MissionError::invalid("missionUpdate.decisions must be an array"))?;
        update.add_decisions = items
            .iter()
            .enumerate()
            .map(|(index, item)| validate_decision(item, index))
            .collect::<MissionResult<Vec<_>>>()?;
    }
    if update.is_empty() {
        return Err(MissionError::invalid(
            "missionUpdate must include at least one supported field",
        ));
    }
    Ok(update)
}

/// pi's inline decision validator (`actions.ts:219-230`). Note `prompt`/`recommendation` are
/// guarded on TRUTHINESS-after-trim rather than validated, so an empty string is silently dropped.
fn validate_decision(value: &Value, index: usize) -> MissionResult<MissionDecisionInput> {
    let decision = value.as_object().ok_or_else(|| {
        MissionError::invalid(format!(
            "missionUpdate.decisions[{index}] must be an object"
        ))
    })?;
    let title = decision
        .get("title")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| {
            MissionError::invalid(format!(
                "missionUpdate.decisions[{index}].title must be a non-empty string"
            ))
        })?;
    let options =
        match decision.get("options") {
            None => None,
            Some(v) => {
                let items = v.as_array().ok_or_else(|| {
                MissionError::invalid(format!(
                    "missionUpdate.decisions[{index}].options must contain only non-empty strings"
                ))
            })?;
                let mut out = Vec::with_capacity(items.len());
                for item in items {
                    let s = item.as_str().filter(|s| !s.trim().is_empty()).ok_or_else(|| {
                    MissionError::invalid(format!(
                        "missionUpdate.decisions[{index}].options must contain only non-empty \
                         strings"
                    ))
                })?;
                    out.push(s.to_string());
                }
                Some(out)
            }
        };
    Ok(MissionDecisionInput {
        title: title.trim().to_string(),
        prompt: decision
            .get("prompt")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        options,
        recommendation: decision
            .get("recommendation")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
    })
}

/// pi `refreshLinkedRunStatus` (`actions.ts:236-284`) — `mission.show`'s read-through refresh.
///
/// For every linked run with an `asyncDir`, re-read `<asyncDir>/status.json` and, when its `state`
/// differs from the recorded status, produce an updated run link (stamping `completedAt` if the
/// new state is terminal and none was recorded). The mission's own status is then re-derived from
/// the merged set of run states, with a TERMINAL mission status short-circuiting the whole ladder.
///
/// Every failure to read/parse a linked status becomes a warning; none is fatal.
fn refresh_linked_run_status(
    location: &MissionStoreLocation,
    record: MissionRecord,
) -> MissionResult<(MissionRecord, Vec<String>)> {
    let mut warnings = Vec::new();
    let mut updates: Vec<MissionRunLink> = Vec::new();
    for run in &record.runs {
        let Some(async_dir) = run.async_dir.as_deref() else {
            continue;
        };
        let status_path = Path::new(async_dir).join("status.json");
        if !status_path.exists() {
            continue;
        }
        let display = status_path.to_string_lossy().into_owned();
        let raw = match std::fs::read_to_string(&status_path) {
            Ok(raw) => raw,
            Err(err) => {
                warnings.push(format!(
                    "Failed to read linked run status '{display}': {err}"
                ));
                continue;
            }
        };
        let status: Value = match serde_json::from_str(&raw) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(format!(
                    "Failed to read linked run status '{display}': {err}"
                ));
                continue;
            }
        };
        if !status.is_object() {
            warnings.push(format!("Linked run status '{display}' must be an object"));
            continue;
        }
        let state = status
            .get("state")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty());
        let Some(state) = state else {
            warnings.push(format!("Linked run status '{display}' is missing state"));
            continue;
        };
        if Some(state) == run.status.as_deref() {
            continue;
        }
        let terminal = !matches!(state, "queued" | "running");
        updates.push(MissionRunLink {
            status: Some(state.to_string()),
            completed_at: if terminal && run.completed_at.is_none() {
                Some(super::format_iso8601_millis(crate::time::now_epoch_millis()))
            } else {
                run.completed_at.clone()
            },
            ..run.clone()
        });
    }
    if updates.is_empty() {
        return Ok((record, warnings));
    }
    let merged: Vec<MissionRunLink> = record
        .runs
        .iter()
        .map(|run| {
            updates
                .iter()
                .find(|update| update.run_id == run.run_id && update.child_index == run.child_index)
                .cloned()
                .unwrap_or_else(|| run.clone())
        })
        .collect();
    // `states` keeps the `None`s: upstream's `merged.map((run) => run.status)` yields `undefined`
    // for a status-less run, and the `.every(...)` branch below therefore FAILS on one — a mission
    // with one complete run and one status-less run is NOT "completed". Filtering the `None`s out
    // here would silently flip that case.
    let states: Vec<Option<&str>> = merged.iter().map(|run| run.status.as_deref()).collect();
    let any_live = states
        .iter()
        .any(|s| matches!(*s, Some("queued" | "running" | "active")));
    // pi's ternary ladder (`actions.ts:268-282`), in its own order. Note the FIRST goal branch:
    // a goal mission with no live run is `active`, not `waiting`/`failed` — a goal mission stays
    // active so the turn-end driver keeps raising continuation notices for it.
    let status: MissionStatus = if record.status.is_terminal() {
        record.status
    // Upstream writes these as TWO branches (`record.goal && !anyLive ? "active" : anyLive ?
    // "active" : …`, `actions.ts:270-272`) whose bodies are identical, so they collapse to one
    // disjunction with no behavioural change: a live run means active, and a GOAL mission is
    // active even with nothing live — which is what keeps it eligible for the next turn's
    // continuation notice instead of falling through to `waiting`/`failed` below.
    } else if any_live || record.goal.is_some() {
        MissionStatus::Active
    } else if states.contains(&Some("paused")) {
        MissionStatus::Waiting
    } else if states.contains(&Some("failed")) {
        MissionStatus::Failed
    } else if states
        .iter()
        .any(|s| matches!(*s, Some("stopped" | "rejected" | "cancelled")))
    {
        MissionStatus::Cancelled
    } else if !states.is_empty()
        && states
            .iter()
            .all(|s| matches!(*s, Some("complete" | "completed")))
    {
        MissionStatus::Completed
    } else {
        record.status
    };
    let refreshed = update_mission(
        location,
        &record.id,
        &MissionUpdateInput {
            status: Some(status),
            add_runs: updates,
            ..Default::default()
        },
        crate::time::now_epoch_millis(),
        None,
    )?;
    Ok((refreshed, warnings))
}

/// pi `formatMission` (`actions.ts:286-314`) — the human-facing rendering, line for line.
#[must_use]
pub fn format_mission(record: &MissionRecord) -> String {
    let mut lines = vec![
        format!("Mission: {}", record.id),
        format!("Title: {}", record.title),
        format!("Status: {}", record.status.as_str()),
        format!("Objective: {}", record.objective),
        format!("Updated: {}", record.updated_at),
    ];
    if let (Some(goal), Some(budget)) = (record.goal, record.budget) {
        lines.push(format!("Goal mode: {}", goal.status.as_str()));
        lines.push(format!(
            "Budget: {}/{} tokens",
            record.usage.map_or(0, |u| u.tokens),
            budget.tokens
        ));
    }
    if let Some(summary) = &record.summary {
        lines.push(format!("Summary: {summary}"));
    }
    if let Some(labels) = record.labels.as_ref().filter(|l| !l.is_empty()) {
        lines.push(format!("Labels: {}", labels.join(", ")));
    }
    if !record.runs.is_empty() {
        lines.push("Runs:".to_string());
        for run in &record.runs {
            let status = run
                .status
                .as_ref()
                .map_or_else(String::new, |s| format!(", {s}"));
            let dir = run
                .async_dir
                .as_ref()
                .map_or_else(String::new, |d| format!(" — {d}"));
            lines.push(format!(
                "  {} ({}{status}){dir}",
                run.run_id,
                run.mode.as_str()
            ));
        }
    }
    if !record.decisions.is_empty() {
        lines.push("Decisions:".to_string());
        for decision in &record.decisions {
            // SUBA-085 / `actions.ts:314` @v0.64.0 (entered at v0.47.1; v0.43.0's `:303` had no
            // suffix): a resolved decision renders its resolution after the title, guarded on
            // TRUTHINESS, so an absent resolution adds nothing.
            lines.push(format!(
                "  {}: {} — {}{}",
                decision.id,
                decision.status.as_str(),
                decision.title,
                decision
                    .resolution
                    .as_deref()
                    .filter(|resolution| !resolution.is_empty())
                    .map(|resolution| format!("; resolution: {resolution}"))
                    .unwrap_or_default()
            ));
        }
    }
    if !record.artifacts.is_empty() {
        lines.push("Artifacts:".to_string());
        for artifact in &record.artifacts {
            lines.push(format!("  {}: {}", artifact.kind.as_str(), artifact.path));
        }
    }
    if !record.receipts.is_empty() {
        lines.push("Delivery receipts:".to_string());
        for receipt in &record.receipts {
            lines.push(format!(
                "  {} ({}): {} — {}",
                receipt.kind.as_str(),
                receipt.status.as_str(),
                receipt.title,
                receipt.url
            ));
        }
    }
    lines.join("\n")
}

/// The `{ mode: "management", results: [], missionId, missionPath, mission }` details shape every
/// single-mission action returns (`actions.ts:337`, `:373`, `:395`, `:405`).
fn mission_details(record: &MissionRecord, mission_path: &Path) -> Value {
    serde_json::json!({
        "mode": "management",
        "results": [],
        "missionId": record.id,
        "missionPath": mission_path.to_string_lossy(),
        "mission": record,
    })
}

/// pi `handleMissionAction` (`actions.ts:316-410`) — the single entry point for all six actions.
///
/// # Errors
///
/// [`MissionError::Invalid`] carrying upstream's exact refusal text for any validation failure,
/// [`MissionError::NotFound`] when a targeted mission does not exist, or [`MissionError::Io`] for
/// a persistence failure.
pub fn handle_mission_action(
    action: MissionAction,
    params: &MissionActionParams,
    ctx: &MissionActionContext,
) -> MissionResult<MissionActionOutcome> {
    let location =
        resolve_mission_store_location(&ctx.cwd, ctx.config.as_ref(), ctx.agent_dir.as_deref());
    match action {
        MissionAction::Create => {
            let mission = validate_mission_launch(
                params
                    .mission
                    .as_ref()
                    .ok_or_else(|| MissionError::invalid("mission must be an object"))?,
            )?;
            let record = create_mission(
                &location,
                &MissionCreateInput {
                    title: mission.title.clone(),
                    objective: mission.objective.clone().unwrap_or(mission.title),
                    goal: mission.goal.then_some(true),
                    budget: mission.budget,
                    status: Some(match params.mission_status.as_deref() {
                        Some(raw) => validate_status(Some(raw), "missionStatus")?,
                        None => MissionStatus::Planned,
                    }),
                    labels: mission.labels,
                    owner_session_id: ctx.current_session_id.clone(),
                },
                crate::time::now_epoch_millis(),
                ctx.config.as_ref().and_then(|c| c.retain_terminal),
            )?;
            let path = mission_record_path(&location, &record.id)?;
            Ok(MissionActionOutcome {
                text: format!("Created mission {}: {}", record.id, record.title),
                details: mission_details(&record, &path),
            })
        }
        MissionAction::List => {
            match params.mission_scope.as_deref() {
                None | Some("project") => {
                    let listed = list_missions(&location);
                    let mut lines: Vec<String> = if listed.records.is_empty() {
                        vec!["No project missions.".to_string()]
                    } else {
                        listed
                            .records
                            .iter()
                            .map(|record| {
                                // SUBA-085 / `actions.ts:361-366` @v0.64.0 (entered at v0.47.1):
                                // a record with any decisions carries an open/resolved tally.
                                let open = record
                                    .decisions
                                    .iter()
                                    .filter(|d| d.status == MissionDecisionStatus::Open)
                                    .count();
                                let tally = if record.decisions.is_empty() {
                                    String::new()
                                } else {
                                    format!(
                                        "  decisions: {open} open, {} resolved",
                                        record.decisions.len() - open
                                    )
                                };
                                format!(
                                    "{}  {}  {}  {}{tally}",
                                    record.id,
                                    record.status.as_str(),
                                    record.title,
                                    record.updated_at
                                )
                            })
                            .collect()
                    };
                    if !listed.warnings.is_empty() {
                        lines.push(String::new());
                        lines.extend(listed.warnings.iter().map(|w| format!("Warning: {w}")));
                    }
                    Ok(MissionActionOutcome {
                        text: lines.join("\n"),
                        details: serde_json::json!({
                            "mode": "management",
                            "results": [],
                            "missions": {
                                "records": listed.records,
                                "warnings": listed.warnings,
                            },
                        }),
                    })
                }
                Some("global") => {
                    let listed = list_global_missions(&location.global_index_dir);
                    let mut lines: Vec<String> = if listed.entries.is_empty() {
                        vec!["No indexed missions.".to_string()]
                    } else {
                        listed
                            .entries
                            .iter()
                            .map(|entry| {
                                format!(
                                    "{}  {}{}  {}  {}",
                                    entry.entry.mission_id,
                                    entry.entry.status.as_str(),
                                    if entry.stale { " [stale]" } else { "" },
                                    entry.entry.title,
                                    entry.entry.project_root
                                )
                            })
                            .collect()
                    };
                    if !listed.warnings.is_empty() {
                        lines.push(String::new());
                        lines.extend(listed.warnings.iter().map(|w| format!("Warning: {w}")));
                    }
                    Ok(MissionActionOutcome {
                        text: lines.join("\n"),
                        details: serde_json::json!({
                            "mode": "management",
                            "results": [],
                            "missions": {
                                "globalEntries": listed.entries,
                                "warnings": listed.warnings,
                            },
                        }),
                    })
                }
                Some(_) => Err(MissionError::invalid(
                    "missionScope must be \"project\" or \"global\"",
                )),
            }
        }
        MissionAction::Show => {
            let current = read_mission(&location, &require_mission_id(params)?)?;
            let (record, warnings) = refresh_linked_run_status(&location, current)?;
            let mut lines = vec![
                format_mission(&record),
                format!(
                    "State: {}",
                    mission_state_path(&location, &record.id)?.display()
                ),
            ];
            if !warnings.is_empty() {
                lines.push(String::new());
                lines.extend(warnings.iter().map(|w| format!("Warning: {w}")));
            }
            let path = mission_record_path(&location, &record.id)?;
            let mut details = mission_details(&record, &path);
            if let Some(map) = details.as_object_mut() {
                map.insert(
                    "missions".to_string(),
                    serde_json::json!({ "warnings": warnings }),
                );
            }
            Ok(MissionActionOutcome {
                text: lines.join("\n"),
                details,
            })
        }
        MissionAction::Update => {
            let record = update_mission(
                &location,
                &require_mission_id(params)?,
                &validate_mission_update(params.mission_update.as_ref())?,
                crate::time::now_epoch_millis(),
                None,
            )?;
            let path = mission_record_path(&location, &record.id)?;
            Ok(MissionActionOutcome {
                text: format!(
                    "Updated mission {}.\n\n{}",
                    record.id,
                    format_mission(&record)
                ),
                details: mission_details(&record, &path),
            })
        }
        MissionAction::ResolveDecision => {
            // SUBA-085 / pi `actions.ts:391-397` @v0.64.0, in upstream's check order: mission id,
            // then the decision id through `validateMissionId(params.id, "id")` (so a missing
            // `id` is "id must be a non-empty string" and a malformed one gets the id-pattern
            // text), then the summary guard with upstream's verbatim message. The store then
            // refuses an unknown or already-resolved id rather than silently no-op'ing.
            let mission_id = require_mission_id(params)?;
            let decision_id = match params.id.as_deref() {
                Some(id) => validate_mission_id_str(id, "id")?,
                None => return Err(MissionError::invalid("id must be a non-empty string")),
            };
            let Some(summary) = params
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            else {
                return Err(MissionError::invalid(
                    "mission.resolve-decision requires a non-empty summary",
                ));
            };
            let record = update_mission(
                &location,
                &mission_id,
                &MissionUpdateInput {
                    resolve_decision: Some(MissionDecisionResolution {
                        id: decision_id.clone(),
                        resolution: summary.to_string(),
                    }),
                    ..Default::default()
                },
                crate::time::now_epoch_millis(),
                None,
            )?;
            let path = mission_record_path(&location, &record.id)?;
            Ok(MissionActionOutcome {
                text: format!(
                    "Resolved decision {decision_id} for mission {}.\n\n{}",
                    record.id,
                    format_mission(&record)
                ),
                details: mission_details(&record, &path),
            })
        }
        MissionAction::AttachRun => {
            let mission_id = require_mission_id(params)?;
            let run_id = params
                .run_id
                .as_deref()
                .or(params.id.as_deref())
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(|| MissionError::invalid("mission.attach-run requires runId or id"))?;
            let raw_mode = params.run_mode.as_deref().unwrap_or("external");
            let mode = MissionRunMode::from_wire(raw_mode)
                .ok_or_else(|| MissionError::invalid("runMode is invalid"))?;
            if params.dir.as_deref().is_some_and(|d| d.trim().is_empty()) {
                return Err(MissionError::invalid("dir must be a non-empty string"));
            }
            if params
                .run_status
                .as_deref()
                .is_some_and(|s| s.trim().is_empty())
            {
                return Err(MissionError::invalid(
                    "runStatus must be a non-empty string",
                ));
            }
            let record = update_mission(
                &location,
                &mission_id,
                &MissionUpdateInput {
                    status: Some(MissionStatus::Active),
                    add_runs: vec![MissionRunLink {
                        run_id: run_id.trim().to_string(),
                        mode,
                        async_dir: params.dir.clone(),
                        child_index: None,
                        agent: params.agent.clone(),
                        status: params.run_status.clone(),
                        started_at: Some(super::format_iso8601_millis(
                            crate::time::now_epoch_millis(),
                        )),
                        completed_at: None,
                        usage: None,
                    }],
                    ..Default::default()
                },
                crate::time::now_epoch_millis(),
                None,
            )?;
            let path = mission_record_path(&location, &record.id)?;
            Ok(MissionActionOutcome {
                text: format!("Attached run {run_id} to mission {}.", record.id),
                details: mission_details(&record, &path),
            })
        }
        MissionAction::Close => {
            let mission_id = require_mission_id(params)?;
            let status = match params.mission_status.as_deref().unwrap_or("completed") {
                "completed" => MissionStatus::Completed,
                "failed" => MissionStatus::Failed,
                "cancelled" => MissionStatus::Cancelled,
                _ => {
                    return Err(MissionError::invalid(
                        "mission.close missionStatus must be completed, failed, or cancelled",
                    ));
                }
            };
            if params
                .summary
                .as_deref()
                .is_some_and(|s| s.trim().is_empty())
            {
                return Err(MissionError::invalid("summary must be a non-empty string"));
            }
            let record = update_mission(
                &location,
                &mission_id,
                &MissionUpdateInput {
                    status: Some(status),
                    summary: params.summary.as_deref().map(|s| s.trim().to_string()),
                    ..Default::default()
                },
                crate::time::now_epoch_millis(),
                None,
            )?;
            let path = mission_record_path(&location, &record.id)?;
            Ok(MissionActionOutcome {
                text: format!(
                    "Closed mission {} as {}.",
                    record.id,
                    record.status.as_str()
                ),
                details: mission_details(&record, &path),
            })
        }
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

    fn ctx(root: &Path) -> MissionActionContext {
        MissionActionContext {
            cwd: root.to_path_buf(),
            current_session_id: Some("sess-1".to_string()),
            config: None,
            agent_dir: Some(root.join("agent")),
        }
    }

    fn create(root: &Path, title: &str) -> MissionActionOutcome {
        handle_mission_action(
            MissionAction::Create,
            &MissionActionParams {
                mission: Some(serde_json::json!({ "title": title, "objective": "do the thing" })),
                ..Default::default()
            },
            &ctx(root),
        )
        .unwrap()
    }

    fn mission_id_of(outcome: &MissionActionOutcome) -> String {
        outcome.details["missionId"].as_str().unwrap().to_string()
    }

    /// pi `MISSION_ACTIONS` (`actions.ts:32-40` @v0.64.0), seven verbs in upstream's order. Pre
    /// SUBA-085 the array had six and `mission.resolve-decision` parsed as `None`.
    #[test]
    fn mission_action_vocabulary_matches_upstream_exactly() {
        assert_eq!(
            MISSION_ACTIONS,
            [
                "mission.create",
                "mission.list",
                "mission.show",
                "mission.update",
                "mission.resolve-decision",
                "mission.attach-run",
                "mission.close",
            ]
        );
        assert_eq!(
            MissionAction::from_wire("mission.resolve-decision"),
            Some(MissionAction::ResolveDecision)
        );
        assert!(MissionAction::ResolveDecision.is_mutating());
        for name in MISSION_ACTIONS {
            let action = MissionAction::from_wire(name).unwrap();
            assert_eq!(action.as_str(), name);
            assert_eq!(
                action.is_mutating(),
                MUTATING_MISSION_ACTIONS.contains(&name)
            );
        }
        assert!(MissionAction::from_wire("mission.nope").is_none());
        assert!(MissionAction::from_wire("list").is_none());
    }

    /// Record one decision through `mission.update`, then close it through
    /// `mission.resolve-decision` (pi `actions.ts:391-397` @v0.64.0). The receipt is upstream's
    /// `Resolved decision <id> for mission <id>.` line over the re-rendered mission, and the
    /// record now carries `resolved`, the trimmed resolution, and a `resolvedAt` stamp.
    #[test]
    fn resolve_decision_closes_an_open_decision_and_renders_the_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Deciding"));
        let updated = handle_mission_action(
            MissionAction::Update,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                mission_update: Some(serde_json::json!({
                    "decisions": [{"title": "Which database?", "recommendation": "postgres"}],
                })),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        let decision_id = updated.details["mission"]["decisions"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let resolved = handle_mission_action(
            MissionAction::ResolveDecision,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                id: Some(decision_id.clone()),
                summary: Some("  postgres, with pgvector  ".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(
            resolved.text.starts_with(&format!(
                "Resolved decision {decision_id} for mission {id}.\n\n"
            )),
            "{}",
            resolved.text
        );
        assert!(
            resolved.text.contains(&format!(
                "  {decision_id}: resolved — Which database?; resolution: postgres, with pgvector"
            )),
            "{}",
            resolved.text
        );
        let decision = &resolved.details["mission"]["decisions"][0];
        assert_eq!(decision["status"], "resolved");
        assert_eq!(decision["resolution"], "postgres, with pgvector");
        assert!(decision["resolvedAt"].is_string());
        assert_eq!(resolved.details["missionId"], id);

        // `mission.list` carries the open/resolved tally (`actions.ts:361-366` @v0.64.0).
        let listed = handle_mission_action(
            MissionAction::List,
            &MissionActionParams::default(),
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(
            listed.text.ends_with("  decisions: 0 open, 1 resolved"),
            "{}",
            listed.text
        );
    }

    /// `mission.resolve-decision`'s refusals, each with upstream's verbatim text and in
    /// upstream's check order (`actions.ts:392-394` @v0.64.0, then `store.ts:498-501`): the
    /// mission id first, then the decision id through `validateMissionId(params.id, "id")`, then
    /// the summary guard, then the store's unknown-id and already-resolved refusals. None of
    /// them silently no-op.
    #[test]
    fn resolve_decision_reports_the_exact_upstream_refusals() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Deciding"));
        let resolve = |mission_id: Option<&str>, decision: Option<&str>, summary: Option<&str>| {
            handle_mission_action(
                MissionAction::ResolveDecision,
                &MissionActionParams {
                    mission_id: mission_id.map(str::to_string),
                    id: decision.map(str::to_string),
                    summary: summary.map(str::to_string),
                    ..Default::default()
                },
                &ctx(tmp.path()),
            )
        };
        assert_eq!(
            resolve(None, Some("d1"), Some("s"))
                .unwrap_err()
                .to_string(),
            "missionId must be a non-empty string"
        );
        assert_eq!(
            resolve(Some(&id), None, Some("s")).unwrap_err().to_string(),
            "id must be a non-empty string"
        );
        assert_eq!(
            resolve(Some(&id), Some("../d1"), Some("s"))
                .unwrap_err()
                .to_string(),
            "id must contain only letters, numbers, '.', '_', or '-' and cannot contain '..'"
        );
        assert_eq!(
            resolve(Some(&id), Some("d1"), None)
                .unwrap_err()
                .to_string(),
            "mission.resolve-decision requires a non-empty summary"
        );
        assert_eq!(
            resolve(Some(&id), Some("d1"), Some("   "))
                .unwrap_err()
                .to_string(),
            "mission.resolve-decision requires a non-empty summary"
        );
        assert_eq!(
            resolve(Some(&id), Some("d1"), Some("s"))
                .unwrap_err()
                .to_string(),
            format!("Decision 'd1' was not found in mission '{id}'")
        );

        let updated = handle_mission_action(
            MissionAction::Update,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                mission_update: Some(serde_json::json!({ "decisions": [{"title": "Ship?"}] })),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        let decision_id = updated.details["mission"]["decisions"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        resolve(Some(&id), Some(&decision_id), Some("yes")).unwrap();
        assert_eq!(
            resolve(Some(&id), Some(&decision_id), Some("no"))
                .unwrap_err()
                .to_string(),
            format!("Decision '{decision_id}' is already resolved")
        );
    }

    #[test]
    fn create_stamps_the_owner_session_and_renders_the_receipt() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = create(tmp.path(), "Ship v2");
        assert!(outcome.text.starts_with("Created mission "));
        assert!(outcome.text.ends_with(": Ship v2"));
        assert_eq!(outcome.details["mode"], "management");
        assert_eq!(outcome.details["mission"]["ownerSessionId"], "sess-1");
        assert_eq!(outcome.details["mission"]["status"], "planned");
        assert_eq!(outcome.details["mission"]["objective"], "do the thing");
    }

    #[test]
    fn validate_mission_launch_reports_the_exact_upstream_messages() {
        let cases: Vec<(Value, &str)> = vec![
            (serde_json::json!([]), "mission must be an object"),
            (serde_json::json!({"nope": 1}), "mission.nope is unknown"),
            (
                serde_json::json!({"title": "a", "summary": "b"}),
                "mission.title and mission.summary cannot both be set",
            ),
            (
                serde_json::json!({}),
                "mission.title or mission.summary must be a non-empty string",
            ),
            (
                serde_json::json!({"title": "t", "objective": "  "}),
                "mission.objective must be a non-empty string",
            ),
            (
                serde_json::json!({"title": "t", "goal": false}),
                "mission.goal must be true when supplied",
            ),
            (
                serde_json::json!({"title": "t", "budget": {"tokens": 0}}),
                "mission.budget.tokens must be a positive integer",
            ),
            (
                serde_json::json!({"title": "t", "goal": true}),
                "mission.budget is required when mission.goal is true",
            ),
            (
                serde_json::json!({"title": "t", "labels": ["a", ""]}),
                "mission.labels must contain only non-empty strings",
            ),
        ];
        for (input, expected) in cases {
            let err = validate_mission_launch(&input).unwrap_err();
            assert_eq!(err.to_string(), expected, "input: {input}");
        }
        // `summary` is an accepted ALIAS for `title`.
        let ok = validate_mission_launch(&serde_json::json!({"summary": "  aliased  "})).unwrap();
        assert_eq!(ok.title, "aliased");
        assert!(!ok.goal);
        // `actions.ts:130` trims each label on the LAUNCH path (the UPDATE path does not, and
        // leaves it to the store's `stringArray`).
        let trimmed =
            validate_mission_launch(&serde_json::json!({"title": "t", "labels": ["  a  ", "b"]}))
                .unwrap();
        assert_eq!(
            trimmed.labels.as_deref(),
            Some(["a".to_string(), "b".to_string()].as_slice())
        );
    }

    #[test]
    fn list_renders_project_scope_and_rejects_an_unknown_scope() {
        let tmp = tempfile::tempdir().unwrap();
        let created = create(tmp.path(), "One");
        let id = mission_id_of(&created);
        let listed = handle_mission_action(
            MissionAction::List,
            &MissionActionParams::default(),
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(listed.text.contains(&id), "{}", listed.text);
        assert!(listed.text.contains("planned  One  "), "{}", listed.text);
        assert_eq!(
            listed.details["missions"]["records"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        let global = handle_mission_action(
            MissionAction::List,
            &MissionActionParams {
                mission_scope: Some("global".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(global.text.contains(&id), "{}", global.text);
        assert_eq!(
            global.details["missions"]["globalEntries"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        // The flattened pointer keeps its entry fields at the TOP level.
        assert_eq!(
            global.details["missions"]["globalEntries"][0]["missionId"],
            id.as_str()
        );
        assert_eq!(
            global.details["missions"]["globalEntries"][0]["stale"],
            false
        );

        let err = handle_mission_action(
            MissionAction::List,
            &MissionActionParams {
                mission_scope: Some("galactic".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "missionScope must be \"project\" or \"global\""
        );
    }

    #[test]
    fn an_empty_project_list_says_so() {
        let tmp = tempfile::tempdir().unwrap();
        let listed = handle_mission_action(
            MissionAction::List,
            &MissionActionParams::default(),
            &ctx(tmp.path()),
        )
        .unwrap();
        assert_eq!(listed.text, "No project missions.");
        let global = handle_mission_action(
            MissionAction::List,
            &MissionActionParams {
                mission_scope: Some("global".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert_eq!(global.text, "No indexed missions.");
    }

    #[test]
    fn show_renders_the_full_record_plus_the_state_path() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Showable"));
        let shown = handle_mission_action(
            MissionAction::Show,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(
            shown
                .text
                .starts_with(&format!("Mission: {id}\nTitle: Showable\nStatus: planned"))
        );
        assert!(shown.text.contains(&format!(
                "State: {}",
                tmp.path()
                    .join(".cyrup-subagents")
                    .join("missions")
                    .join(&id)
                    .join("state.json")
                    .display()
            )));
    }

    #[test]
    fn show_refreshes_a_linked_runs_status_from_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Linked"));
        let async_dir = tmp.path().join("async").join("run-1");
        std::fs::create_dir_all(&async_dir).unwrap();
        handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                run_id: Some("run-1".to_string()),
                dir: Some(async_dir.to_string_lossy().into_owned()),
                run_status: Some("running".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        std::fs::write(async_dir.join("status.json"), r#"{"state":"complete"}"#).unwrap();

        let shown = handle_mission_action(
            MissionAction::Show,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(shown.text.contains("Status: completed"), "{}", shown.text);
        assert!(
            shown.text.contains("run-1 (external, complete)"),
            "{}",
            shown.text
        );
        assert_eq!(shown.details["mission"]["runs"][0]["status"], "complete");
        assert!(shown.details["mission"]["runs"][0]["completedAt"].is_string());
    }

    /// A status-less run is `undefined` in upstream's `states` array, and `undefined` fails the
    /// `every(state === "complete" | "completed")` test — so ONE settled run does not close a
    /// mission that also carries an unstatused one. Filtering the `None`s out of the Rust vector
    /// would report `completed` here.
    #[test]
    fn a_status_less_sibling_run_blocks_the_completed_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Mixed"));
        let async_dir = tmp.path().join("async").join("run-done");
        std::fs::create_dir_all(&async_dir).unwrap();
        handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                run_id: Some("run-done".to_string()),
                dir: Some(async_dir.to_string_lossy().into_owned()),
                run_status: Some("running".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        // A second run with NO status at all (no `runStatus`, no async dir to refresh from).
        handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                run_id: Some("run-unknown".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        std::fs::write(async_dir.join("status.json"), r#"{"state":"complete"}"#).unwrap();

        let shown = handle_mission_action(
            MissionAction::Show,
            &MissionActionParams {
                mission_id: Some(id),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        // `mission.attach-run` left the mission ACTIVE, and the refresh must not advance it.
        assert!(shown.text.contains("Status: active"), "{}", shown.text);
        assert!(!shown.text.contains("Status: completed"), "{}", shown.text);
    }

    #[test]
    fn show_warns_but_does_not_fail_on_an_unreadable_linked_status() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Broken"));
        let async_dir = tmp.path().join("async").join("run-2");
        std::fs::create_dir_all(&async_dir).unwrap();
        std::fs::write(async_dir.join("status.json"), "{ not json").unwrap();
        handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                run_id: Some("run-2".to_string()),
                dir: Some(async_dir.to_string_lossy().into_owned()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        let shown = handle_mission_action(
            MissionAction::Show,
            &MissionActionParams {
                mission_id: Some(id),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(
            shown
                .text
                .contains("Warning: Failed to read linked run status '"),
            "{}",
            shown.text
        );
    }

    #[test]
    fn update_validates_every_sub_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Updatable"));
        let bad: Vec<(Value, &str)> = vec![
            (
                serde_json::json!({}),
                "missionUpdate must include at least one supported field",
            ),
            (
                serde_json::json!({"nope": 1}),
                "missionUpdate.nope is unknown",
            ),
            (
                serde_json::json!({"title": " "}),
                "missionUpdate.title must be a non-empty string",
            ),
            (
                serde_json::json!({"goal": {"paused": "yes"}}),
                "missionUpdate.goal must be boolean or { paused: boolean }",
            ),
            (
                serde_json::json!({"budget": {"tokens": -1}}),
                "missionUpdate.budget.tokens must be a positive integer",
            ),
            (
                serde_json::json!({"artifacts": [{"kind": "nope", "path": "p"}]}),
                "missionUpdate.artifacts[0].kind is invalid",
            ),
            (
                serde_json::json!({"receipts": [{"kind": "ci", "status": "pending", "title": "t", "url": "nope"}]}),
                "missionUpdate.receipts[0].url must be an absolute URL",
            ),
            (
                serde_json::json!({"receipts": [{"kind": "ci", "status": "pending", "title": "t", "url": "https://a.b", "extra": 1}]}),
                "missionUpdate.receipts[0].extra is unknown",
            ),
            (
                serde_json::json!({"decisions": [{"title": ""}]}),
                "missionUpdate.decisions[0].title must be a non-empty string",
            ),
        ];
        for (update, expected) in bad {
            let err = handle_mission_action(
                MissionAction::Update,
                &MissionActionParams {
                    mission_id: Some(id.clone()),
                    mission_update: Some(update.clone()),
                    ..Default::default()
                },
                &ctx(tmp.path()),
            )
            .unwrap_err();
            assert_eq!(err.to_string(), expected, "update: {update}");
        }
    }

    #[test]
    fn update_applies_goal_paused_artifacts_and_receipts() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Rich"));
        let updated = handle_mission_action(
            MissionAction::Update,
            &MissionActionParams {
                mission_id: Some(id),
                mission_update: Some(serde_json::json!({
                    "goal": {"paused": true},
                    "budget": {"tokens": 5000},
                    "labels": ["  a  ", "b", "a"],
                    "artifacts": [{"kind": "patch", "path": "/tmp/x.diff", "description": "the patch"}],
                    "receipts": [{"kind": "pull_request", "status": "ready", "title": "PR", "url": "  https://example.com/pr/9  "}],
                    "decisions": [{"title": "Ship?", "options": ["yes", "no"], "recommendation": "yes"}],
                })),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(updated.text.starts_with("Updated mission "));
        assert!(
            updated.text.contains("Goal mode: paused"),
            "{}",
            updated.text
        );
        assert!(
            updated.text.contains("Budget: 0/5000 tokens"),
            "{}",
            updated.text
        );
        assert!(updated.text.contains("Labels: a, b"), "{}", updated.text);
        assert!(
            updated.text.contains("  patch: /tmp/x.diff"),
            "{}",
            updated.text
        );
        assert!(
            updated
                .text
                .contains("  pull_request (ready): PR — https://example.com/pr/9"),
            "{}",
            updated.text
        );
        assert!(updated.text.contains(": open — Ship?"), "{}", updated.text);
        assert_eq!(
            updated.details["mission"]["receipts"][0]["url"],
            "https://example.com/pr/9"
        );
    }

    #[test]
    fn attach_run_requires_a_run_id_and_validates_the_mode() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Attachable"));
        let err = handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "mission.attach-run requires runId or id");

        let err = handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                id: Some("r".to_string()),
                run_mode: Some("telepathy".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "runMode is invalid");

        // `id` is accepted as the fallback for `runId`, and the mission goes ACTIVE.
        let attached = handle_mission_action(
            MissionAction::AttachRun,
            &MissionActionParams {
                mission_id: Some(id),
                id: Some("r-9".to_string()),
                agent: Some("scout".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert!(attached.text.starts_with("Attached run r-9 to mission "));
        assert_eq!(attached.details["mission"]["status"], "active");
        assert_eq!(attached.details["mission"]["runs"][0]["mode"], "external");
        assert_eq!(attached.details["mission"]["runs"][0]["agent"], "scout");
    }

    #[test]
    fn close_defaults_to_completed_and_refuses_a_non_terminal_status() {
        let tmp = tempfile::tempdir().unwrap();
        let id = mission_id_of(&create(tmp.path(), "Closable"));
        let err = handle_mission_action(
            MissionAction::Close,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                mission_status: Some("active".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "mission.close missionStatus must be completed, failed, or cancelled"
        );

        let closed = handle_mission_action(
            MissionAction::Close,
            &MissionActionParams {
                mission_id: Some(id.clone()),
                summary: Some("  shipped  ".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap();
        assert_eq!(closed.text, format!("Closed mission {id} as completed."));
        assert_eq!(closed.details["mission"]["summary"], "shipped");
    }

    #[test]
    fn a_missing_mission_is_reported_as_not_found() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle_mission_action(
            MissionAction::Show,
            &MissionActionParams {
                mission_id: Some("nope".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap_err();
        assert!(err.is_not_found(), "{err}");
    }

    #[test]
    fn a_traversal_mission_id_is_refused_before_any_filesystem_access() {
        let tmp = tempfile::tempdir().unwrap();
        let err = handle_mission_action(
            MissionAction::Show,
            &MissionActionParams {
                mission_id: Some("../../etc/passwd".to_string()),
                ..Default::default()
            },
            &ctx(tmp.path()),
        )
        .unwrap_err();
        assert!(err.to_string().contains("cannot contain '..'"), "{err}");
    }

    #[test]
    fn format_mission_omits_every_empty_section() {
        let tmp = tempfile::tempdir().unwrap();
        let outcome = create(tmp.path(), "Bare");
        let record: MissionRecord =
            serde_json::from_value(outcome.details["mission"].clone()).unwrap();
        let rendered = format_mission(&record);
        assert_eq!(rendered.lines().count(), 5, "{rendered}");
        assert!(!rendered.contains("Runs:"));
        assert!(!rendered.contains("Decisions:"));
        assert!(!rendered.contains("Artifacts:"));
        assert!(!rendered.contains("Delivery receipts:"));
        assert!(!rendered.contains("Goal mode:"));
    }
}
