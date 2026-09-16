//! SUBA-016 part B — the nine `schedule.*` tool actions.
//!
//! Ports pi `SCHEDULED_RUN_ACTIONS` (`runs/background/scheduled-runs.ts:20-30`) and the nine
//! action bodies (`:606-741`) @ `v0.68.0`.
//!
//! # Result shape
//!
//! [`handle_scheduled_run_action`] returns a `{ text, details }` pair rather than a
//! [`cyrup_core::ToolResult`], exactly as [`crate::missions::handle_mission_action`] does and for
//! the same reason recorded there: this module sits under `background/` and must not depend on the
//! tool crate. The `extension/tool/routing.rs` arm lifts the pair into a `ToolResult`.
//!
//! Upstream's `handleToolCall` wraps the whole switch in a `try/catch` that renders EVERY thrown
//! error as `textResult(message, …, isError = true)` (`:558-560`). So a [`ScheduledRunError`] is
//! an ERROR-FLAGGED RESULT at the boundary, never a `ToolError` — a model that asks for a
//! schedule that does not exist should be told so in the same turn, not handed a tool failure.

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::ceiling_gate::{CeilingResolver, schedule_persistence_refusal};
use super::schedule::{
    SCHEDULE_ID_ERROR, ScheduleCatchUp, ScheduleDueReason, ScheduleId, ScheduleOverlapSkip,
    ScheduleRecord, ScheduleRunRecord, ScheduleRunState, ScheduleTarget, ScheduleTrigger,
    ScheduleVersion, parse_schedule_interval, parse_scheduled_run_time, schedule_timestamp,
};
use super::store::{ScheduleStore, ScheduleStoreError};
use super::trigger::{
    ScheduleFireContext, has_pending_schedule_work, launch, schedule_belongs_to_session,
    tick_due_schedules,
};

/// pi `SCHEDULED_RUN_ACTIONS` (`scheduled-runs.ts:20-30`), in upstream's order.
pub const SCHEDULED_RUN_ACTIONS: &[&str] = &[
    "schedule.create",
    "schedule.list",
    "schedule.show",
    "schedule.history",
    "schedule.pause",
    "schedule.resume",
    "schedule.run",
    "schedule.run-due",
    "schedule.delete",
];

/// pi `:544`'s gate reply, verbatim.
pub const SCHEDULED_RUNS_DISABLED: &str =
    "Scheduled runs are disabled by scheduledRuns.enabled=false.";

/// pi `ScheduledRunAction` (`scheduled-runs.ts:39`) — a validated member of
/// [`SCHEDULED_RUN_ACTIONS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScheduledRunAction {
    /// `schedule.create`
    Create,
    /// `schedule.list`
    List,
    /// `schedule.show`
    Show,
    /// `schedule.history`
    History,
    /// `schedule.pause`
    Pause,
    /// `schedule.resume`
    Resume,
    /// `schedule.run`
    Run,
    /// `schedule.run-due`
    RunDue,
    /// `schedule.delete`
    Delete,
}

impl ScheduledRunAction {
    /// The wire spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "schedule.create",
            Self::List => "schedule.list",
            Self::Show => "schedule.show",
            Self::History => "schedule.history",
            Self::Pause => "schedule.pause",
            Self::Resume => "schedule.resume",
            Self::Run => "schedule.run",
            Self::RunDue => "schedule.run-due",
            Self::Delete => "schedule.delete",
        }
    }

    /// pi `isScheduledRunAction` (`scheduled-runs.ts:92-94`): `None` means "not a schedule action,
    /// fall through to the rest of the dispatch table".
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "schedule.create" => Some(Self::Create),
            "schedule.list" => Some(Self::List),
            "schedule.show" => Some(Self::Show),
            "schedule.history" => Some(Self::History),
            "schedule.pause" => Some(Self::Pause),
            "schedule.resume" => Some(Self::Resume),
            "schedule.run" => Some(Self::Run),
            "schedule.run-due" => Some(Self::RunDue),
            "schedule.delete" => Some(Self::Delete),
            _ => None,
        }
    }

    /// Does this action WRITE?
    ///
    /// Six of the nine do. The child-safe fanout tool refuses exactly those six, with the same
    /// sentence `routing.rs` already applies to the mutating `mission.*` verbs — a fanout child
    /// may read what the project has scheduled, and may not schedule anything, resume anything or
    /// fire anything.
    #[must_use]
    pub const fn is_mutating(self) -> bool {
        !matches!(self, Self::List | Self::Show | Self::History)
    }
}

/// Every failure the nine actions produce. Rendered as an error-flagged RESULT (see the module
/// doc), never propagated as a tool error.
#[derive(Debug)]
pub enum ScheduledRunError {
    /// A refusal with upstream's own sentence.
    Refused(String),
    /// A store failure.
    Store(ScheduleStoreError),
}

impl std::fmt::Display for ScheduledRunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(message) => f.write_str(message),
            Self::Store(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ScheduledRunError {}

impl From<ScheduleStoreError> for ScheduledRunError {
    fn from(value: ScheduleStoreError) -> Self {
        Self::Store(value)
    }
}

/// The subset of the `subagent` tool's parameter surface the nine actions read.
///
/// Every field is the RAW tool argument; validation happens inside
/// [`handle_scheduled_run_action`], so a model gets upstream's actionable sentence rather than a
/// deserialization error it cannot act on. `quiet` and `on` are [`Value`] for exactly that reason
/// — `quiet must be a boolean.` is a refusal this surface owns.
#[derive(Clone, Debug, Default)]
pub struct ScheduledRunActionParams {
    /// `id` — the target schedule, or the caller's chosen id at create.
    pub id: Option<String>,
    /// `name` — the schedule's display name; defaults to the target label.
    pub name: Option<String>,
    /// `at` — a one-shot delay (`+10m`) or a zoned ISO timestamp.
    pub at: Option<String>,
    /// `every` — a fixed interval (`30m`/`6h`/`2d`/`2w`).
    pub every: Option<String>,
    /// `sessionOnly` — the schedule fires only for its creating session.
    pub session_only: Option<bool>,
    /// `quiet` — suppress the completion notice. Raw, so a non-boolean is REFUSED with upstream's
    /// sentence rather than dropped.
    pub quiet: Option<Value>,
    /// `on` — the reserved calendar selector. Declared and refused (`:616`).
    pub on: Option<Value>,
    /// `timezone` — the reserved calendar zone. Declared and refused (`:616`).
    pub timezone: Option<String>,
    /// `overlap` — `"skip"` only.
    pub overlap: Option<String>,
    /// `catchUp` — `"none"` or `"latest"`; defaults to `"latest"`.
    pub catch_up: Option<String>,
    /// `baseRef` — the worktree base. See [`BASE_REF_UNSUPPORTED`].
    pub base_ref: Option<String>,
    /// `workflowScript` — the ONLY legal schedule target.
    pub workflow_script: Option<String>,
    /// `args` — the workflow's arguments object.
    pub args: Option<Value>,
    /// `agent` — refused at create (`:441`).
    pub agent: Option<String>,
    /// `task` — refused at create (`:441`).
    pub task: Option<String>,
    /// `tasks` — refused at create (`:440`).
    pub tasks: Option<Vec<Value>>,
    /// `chain` — refused at create (`:440`).
    pub chain: Option<Vec<Value>>,
    /// `context` — `"fork"` is refused (`:443`).
    pub context: Option<String>,
    /// `async` — an explicit `false` is refused (`:444`).
    pub r#async: Option<bool>,
    /// `cwd` — an explicit project root for the schedule.
    pub cwd: Option<String>,
    /// `timeoutMs` — the fired run's deadline.
    pub timeout_ms: Option<u64>,
    /// The five mission parameters — any of them is refused at create (`:615`).
    pub mission_id: Option<String>,
    /// See [`Self::mission_id`].
    pub mission: Option<Value>,
    /// See [`Self::mission_id`].
    pub mission_update: Option<Value>,
    /// See [`Self::mission_id`].
    pub mission_status: Option<String>,
    /// See [`Self::mission_id`].
    pub mission_scope: Option<String>,
    /// pi `validateExecutionAcceptance(params)` (`:446`), already run by the caller.
    ///
    /// Precomputed rather than re-derived: the validator lives in `extension/tool/params.rs` and
    /// walks the WHOLE dispatch (`tasks[i]`, `chain[i]`, `chain[i].parallel[j]`), which this
    /// module cannot see and must not reimplement.
    pub acceptance_errors: Vec<String>,
}

/// `[CYRUP-DELTA]` — the `baseRef` decision, recorded where a caller meets it.
///
/// `ScheduleTarget::base_ref` (`:45` @v0.68.0) reaches upstream's worktree setup. cyrup's
/// workflow launch path threads NO base ref: `route_workflow_mode` never had one, and the
/// extracted [`crate::extension::executor::workflow_launch`] therefore has none either.
///
/// Silently dropping the value would run the schedule against the wrong tree every time it fires
/// — a data-loss-shaped defect that the user would discover only from the diff. So the field is
/// PERSISTED (a record written by a future build round-trips), and a caller who supplies one is
/// REFUSED here, by name, with what to do instead. Threading `crate::spawn::worktree` into the
/// headless launch is the completion of this, and it is a second unbounded piece of work: a
/// workflow script's children each choose their own cwd, so "the workflow's worktree" is a design
/// question, not a parameter.
pub const BASE_REF_UNSUPPORTED: &str = "schedule.create cannot honour baseRef yet: this build runs a scheduled workflow in the \
     project's own tree, so a baseRef would be silently ignored. Omit it, or check out the ref \
     you want before the schedule fires.";

/// Everything the nine actions read that is not a parameter.
pub struct ScheduledRunActionContext<'a> {
    /// The project's schedule store.
    pub store: &'a ScheduleStore,
    /// §SUBTASK2's pinned identity plus §5's launch seam.
    pub fire: &'a ScheduleFireContext,
    /// The session's own cwd — the default project root, and the `sessionOnly` cross-project test.
    pub cwd: &'a Path,
    /// pi `resolveMaxPending(config)` (`:387-390`), already resolved.
    pub max_pending: u32,
    /// pi's `resolveCapabilityCeiling?` dep (`:87`) — SCOPE_15's gate, whose caller this is.
    pub ceiling: CeilingResolver<'a>,
    /// The clock, injected so every test drives the surface without sleeping.
    pub now: i64,
}

/// pi's `textResult` pair (`scheduled-runs.ts:417-424`).
#[derive(Clone, Debug, PartialEq)]
pub struct ScheduledRunActionOutcome {
    /// The single text content part.
    pub text: String,
    /// `{ mode: "management", results: [], schedules: { records?, runs? } }`.
    pub details: Value,
    /// Whether the boundary should flag this result as an error.
    pub is_error: bool,
}

/// pi `publicScheduleRecord` (`scheduled-runs.ts:426-429`) — `ownerSessionFile` STRIPPED.
///
/// The session file is a filesystem path that a model has no business seeing, and it is the one
/// field on the record that names the operator's machine.
fn public_schedule_record(schedule: &ScheduleRecord) -> Value {
    let mut value = serde_json::to_value(schedule).unwrap_or(Value::Null);
    if let Some(map) = value.as_object_mut() {
        map.remove("ownerSessionFile");
    }
    value
}

fn text_result(
    text: impl Into<String>,
    schedules: Option<&[ScheduleRecord]>,
    runs: Option<&[ScheduleRunRecord]>,
    is_error: bool,
) -> ScheduledRunActionOutcome {
    let mut schedules_map = serde_json::Map::new();
    if let Some(schedules) = schedules {
        schedules_map.insert(
            "records".to_string(),
            Value::Array(schedules.iter().map(public_schedule_record).collect()),
        );
    }
    if let Some(runs) = runs {
        schedules_map.insert(
            "runs".to_string(),
            serde_json::to_value(runs).unwrap_or(Value::Array(Vec::new())),
        );
    }
    ScheduledRunActionOutcome {
        text: text.into(),
        details: serde_json::json!({
            "mode": "management",
            "results": [],
            "schedules": Value::Object(schedules_map),
        }),
        is_error,
    }
}

/// pi `targetLabel` (`scheduled-runs.ts:431-434`).
fn target_label(target: &ScheduleTarget) -> String {
    match crate::workflows::scripted::preview_simple_workflow_run(Some(&target.workflow_script))
        .and_then(|preview| preview.agent)
    {
        Some(agent) => format!("workflowScript -> agent {agent}"),
        None => "workflowScript (dynamic)".to_string(),
    }
}

/// pi `sanitizeTarget` (`scheduled-runs.ts:436-451`) — the schedule target is `workflowScript`,
/// and ONLY `workflowScript`.
fn sanitize_target(params: &ScheduledRunActionParams) -> Result<ScheduleTarget, String> {
    if params.tasks.is_some() || params.chain.is_some() {
        return Err(
            "Recurring schedules require workflowScript; legacy tasks and chain inputs are unsupported."
                .to_string(),
        );
    }
    if params.agent.is_some() || params.task.is_some() {
        return Err(
            "schedule.create requires workflowScript. Use workflowScript: \"return runs.run('main', { agent, task })\"."
                .to_string(),
        );
    }
    let Some(script) = params
        .workflow_script
        .as_deref()
        .map(str::trim)
        .filter(|script| !script.is_empty())
    else {
        return Err("schedule.create requires a non-empty workflowScript.".to_string());
    };
    if params.context.as_deref() == Some("fork") {
        return Err("Scheduled runs require fresh context.".to_string());
    }
    if params.r#async == Some(false) {
        return Err("Scheduled runs are always async.".to_string());
    }
    // See `BASE_REF_UNSUPPORTED`. The value is still VALIDATED first, so a caller who supplies a
    // malformed ref learns that before learning the field is unsupported — the same order
    // upstream's `normalizeWorktreeBaseRef` imposes.
    if let Some(base_ref) = params.base_ref.as_deref() {
        if !crate::workflows::scripted::valid_git_ref(base_ref) {
            return Err(crate::workflows::scripted::BASE_REF_VALIDATION_ERROR.to_string());
        }
        return Err(BASE_REF_UNSUPPORTED.to_string());
    }
    if !params.acceptance_errors.is_empty() {
        return Err(params.acceptance_errors.join(" "));
    }
    let args = crate::workflows::normalize_workflow_args(params.args.as_ref())?;
    Ok(ScheduleTarget {
        workflow_script: script.to_string(),
        args,
        base_ref: None,
    })
}

/// pi `this.resolve(params)` (`scheduled-runs.ts:975-979`).
async fn resolve<'a>(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'a>,
) -> Result<ScheduleRecord, ScheduledRunError> {
    let Some(raw) = params
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return Err(ScheduledRunError::Refused(format!(
            "{} requires id.",
            action.as_str()
        )));
    };
    let id = ScheduleId::parse(raw)
        .ok_or_else(|| ScheduledRunError::Refused(SCHEDULE_ID_ERROR.to_string()))?;
    Ok(ctx.store.get(&id).await?)
}

/// pi's `handleToolCall` switch (`scheduled-runs.ts:545-557`).
///
/// # Errors
///
/// A refusal or a store failure. The boundary renders BOTH as an error-flagged result, per
/// upstream's outer `catch` (`:558-560`) — see this module's doc.
pub async fn handle_scheduled_run_action(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    match action {
        ScheduledRunAction::Create => create(params, ctx).await,
        ScheduledRunAction::List => list(ctx).await,
        ScheduledRunAction::Show => show(action, params, ctx).await,
        ScheduledRunAction::History => history(action, params, ctx).await,
        ScheduledRunAction::Pause => pause(action, params, ctx, true).await,
        ScheduledRunAction::Resume => pause(action, params, ctx, false).await,
        ScheduledRunAction::Run => run_manual(action, params, ctx).await,
        ScheduledRunAction::RunDue => run_due(ctx).await,
        ScheduledRunAction::Delete => remove(action, params, ctx).await,
    }
}

/// pi `create` (`scheduled-runs.ts:606-659`).
///
/// **The gate ORDER is load-bearing**: each gate assumes the previous one held, so reordering
/// them changes which sentence a caller gets for a call that is wrong in two ways at once.
async fn create(
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    // 1 — `:608-609`
    let target = match sanitize_target(params) {
        Ok(target) => target,
        Err(message) => return Ok(text_result(message, None, None, true)),
    };
    let at = params
        .at
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let every = params
        .every
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    // 2 — `:612`. `Boolean(at) === Boolean(every)`: NEITHER or BOTH.
    if at.is_some() == every.is_some() {
        return Ok(text_result(
            "schedule.create requires exactly one trigger: at or every.",
            None,
            None,
            true,
        ));
    }
    // 3 — `:613`
    if params
        .overlap
        .as_deref()
        .is_some_and(|value| value != ScheduleOverlapSkip::VALUE)
    {
        return Ok(text_result(
            "This first recurring slice supports overlap='skip' only.",
            None,
            None,
            true,
        ));
    }
    // 4 — `:614`
    if params
        .catch_up
        .as_deref()
        .is_some_and(|value| value != "none" && value != "latest")
    {
        return Ok(text_result(
            "catchUp must be 'none' or 'latest'.",
            None,
            None,
            true,
        ));
    }
    // 5 — `:615`
    if params.mission_id.is_some()
        || params.mission.is_some()
        || params.mission_update.is_some()
        || params.mission_status.is_some()
        || params.mission_scope.is_some()
    {
        return Ok(text_result(
            "Mission attachment is deferred from this first schedule slice.",
            None,
            None,
            true,
        ));
    }
    // 6 — `:616`. The two reserved calendar params are DECLARED in the schema and refused here, so
    // a model that tries a calendar schedule gets this actionable sentence instead of a schema
    // rejection it cannot read.
    if params.on.is_some()
        || params.timezone.is_some()
        || matches!(every, Some("day" | "week" | "month" | "year"))
    {
        return Ok(text_result(
            "Calendar schedules are deferred from this first safe slice. Use a fixed interval such as every:'24h' or every:'7d'.",
            None,
            None,
            true,
        ));
    }
    // 7 — `:617`
    let quiet = match params.quiet.as_ref() {
        None | Some(Value::Null) => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(_) => return Ok(text_result("quiet must be a boolean.", None, None, true)),
    };
    // 8 — `:618`
    if at.is_some() && quiet == Some(true) {
        return Ok(text_result(
            "quiet is only supported for recurring schedules.",
            None,
            None,
            true,
        ));
    }
    let session_only = params.session_only == Some(true);
    let requested_cwd = params
        .cwd
        .as_deref()
        .map(|cwd| std::path::absolute(cwd).unwrap_or_else(|_| PathBuf::from(cwd)));
    // 9 — `:620`
    if session_only
        && let Some(requested) = requested_cwd.as_deref()
        && !same_path(requested, ctx.cwd)
    {
        return Ok(text_result(
            "sessionOnly schedules cannot use an explicit cross-project cwd.",
            None,
            None,
            true,
        ));
    }
    // 10 — `:622`
    let owner_session_file = if session_only {
        let Some(file) = ctx.fire.session.session_file() else {
            return Ok(text_result(
                "sessionOnly schedules require a persisted current session.",
                None,
                None,
                true,
            ));
        };
        Some(file.to_path_buf())
    } else {
        None
    };
    // 11 — `:623-624`. SCOPE_15 owns the predicate; THIS is its caller.
    //
    // The argument is the PINNED session id, and an absent one is NOT an exemption:
    // `ceiling_lookup_key` maps `None` to the literal `"unknown"`, which is exactly the key a
    // headless host registers a ceiling under. An `Option<&str>` pass-through here would let such
    // a host gain the ability to persist schedules that a ceiling-bound one lacks — the asymmetry
    // with `resolve_capability_ceiling(session_id: Option<&str>, …)`, whose `None` means "consult
    // only the inherited ceiling", is deliberate and lives in `ceiling_gate`.
    if let Some(refusal) = schedule_persistence_refusal(ctx.fire.session.session_id(), ctx.ceiling)
    {
        return Ok(text_result(refusal, None, None, true));
    }
    // 12 — `:625-627`. Counted over the WHOLE store, which is why `list` is not filtered either
    // (see `list`'s own note).
    let (existing, diagnostics) = ctx.store.list().await;
    for diagnostic in diagnostics {
        tracing::warn!(%diagnostic, "skipping an unreadable schedule record while counting pending work");
    }
    let pending = existing
        .iter()
        .filter(|item| has_pending_schedule_work(item))
        .count();
    if pending >= ctx.max_pending as usize {
        return Ok(text_result(
            format!("Schedule limit reached ({}).", ctx.max_pending),
            None,
            None,
            true,
        ));
    }
    // 13 — `:628-629`
    let requested_id = params
        .id
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty());
    let raw_id = requested_id.map_or_else(mint_schedule_id, str::to_string);
    let Some(id) = ScheduleId::parse(&raw_id) else {
        return Ok(text_result(SCHEDULE_ID_ERROR, None, None, true));
    };
    if ctx.store.ids().await?.contains(&id) {
        return Ok(text_result(
            format!("Schedule '{id}' already exists."),
            None,
            None,
            true,
        ));
    }

    let now = ctx.now;
    let trigger = if let Some(at) = at {
        let planned = match parse_scheduled_run_time(at, now) {
            Ok(planned) => planned,
            Err(message) => return Ok(text_result(message, None, None, true)),
        };
        ScheduleTrigger::Once {
            at: at.to_string(),
            next_run_at: Some(schedule_timestamp(planned)),
        }
    } else {
        let every = every.unwrap_or_default();
        let every_ms = match parse_schedule_interval(every) {
            Ok(every_ms) => every_ms,
            Err(message) => return Ok(text_result(message, None, None, true)),
        };
        ScheduleTrigger::Interval {
            every: every.to_string(),
            every_ms,
            anchor_at: schedule_timestamp(now),
            next_run_at: schedule_timestamp(now.saturating_add(every_ms)),
        }
    };
    let is_interval = matches!(trigger, ScheduleTrigger::Interval { .. });
    let schedule = ScheduleRecord {
        schema_version: ScheduleVersion,
        id: id.clone(),
        name: params
            .name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map_or_else(|| target_label(&target), str::to_string),
        cwd: requested_cwd.unwrap_or_else(|| ctx.cwd.to_path_buf()),
        trigger,
        target,
        overlap: ScheduleOverlapSkip,
        // `:647` — `latest` is the DEFAULT, not `none`.
        catch_up: match params.catch_up.as_deref() {
            Some("none") => ScheduleCatchUp::None,
            _ => ScheduleCatchUp::Latest,
        },
        timeout_ms: params.timeout_ms.map(|ms| ms as i64),
        paused: false,
        session_only: session_only.then_some(true),
        // `:652` — quiet is stored only for a recurring schedule; gate 8 already refused the
        // combination, so this is the positive half of the same rule.
        quiet: (is_interval && quiet == Some(true)).then_some(true),
        owner_session_file,
        created_at: schedule_timestamp(now),
        updated_at: schedule_timestamp(now),
        active_run_id: None,
        last_run_id: None,
    };
    ctx.store.write(&schedule).await?;
    ctx.store
        .append_event(&schedule, "schedule.created")
        .await?;
    let text = format!(
        "Created schedule {id}.\nName: {}\nTrigger: {}\nSession only: {}\nQuiet: {}\nNext: {}\nTarget: {}",
        schedule.name,
        at.map_or_else(
            || format!("every {}", every.unwrap_or_default()),
            |at| format!("at {at}")
        ),
        yes_no(schedule.session_only == Some(true)),
        yes_no(schedule.quiet == Some(true)),
        schedule.trigger.next_run_at().unwrap_or_default(),
        target_label(&schedule.target),
    );
    Ok(text_result(
        text,
        Some(std::slice::from_ref(&schedule)),
        None,
        false,
    ))
}

/// pi `list` (`scheduled-runs.ts:661-665`).
///
/// **NOT filtered by session, deliberately.** The header is upstream's literal `Project
/// schedules`, and the fourth column is the per-row `session-only`/`project` marker: the session
/// dimension is surfaced as INFORMATION, not as a filter. A filtered list would hide project
/// schedules the same user can still reach with `schedule.show` and `schedule.delete`, and would
/// make `maxPending`'s count — which is over the whole store — unexplainable from the UI.
async fn list(
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let (mut schedules, diagnostics) = ctx.store.list().await;
    for diagnostic in diagnostics {
        tracing::warn!(%diagnostic, "skipping an unreadable schedule record while listing");
    }
    // Upstream's `localeCompare` over `nextRunAt ?? ""`. Correct as a BYTE comparison only because
    // every stamp this module writes is `Z`-normalised and fixed-width (see `schedule.rs`'s own
    // note on the wire format).
    schedules.sort_by(|a, b| {
        a.trigger
            .next_run_at()
            .unwrap_or_default()
            .cmp(b.trigger.next_run_at().unwrap_or_default())
    });
    if schedules.is_empty() {
        return Ok(text_result("No project schedules.", Some(&[]), None, false));
    }
    let mut lines = vec![format!("Project schedules: {}", schedules.len())];
    for item in &schedules {
        lines.push(format!(
            "- {} | {} | {} | {} | {}",
            item.id,
            schedule_state_label(item),
            item.trigger.next_run_at().unwrap_or("no next run"),
            if item.session_only == Some(true) {
                "session-only"
            } else {
                "project"
            },
            item.name,
        ));
    }
    Ok(text_result(lines.join("\n"), Some(&schedules), None, false))
}

/// pi `show` (`scheduled-runs.ts:667-670`).
async fn show(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let schedule = resolve(action, params, ctx).await?;
    let mut lines = vec![
        format!("Schedule: {}", schedule.id),
        format!("Name: {}", schedule.name),
        format!("State: {}", schedule_state_label(&schedule)),
        format!(
            "Session only: {}",
            yes_no(schedule.session_only == Some(true))
        ),
        format!("Quiet: {}", yes_no(schedule.quiet == Some(true))),
        format!("Target: {}", target_label(&schedule.target)),
        format!(
            "CWD: {}",
            crate::exec::tool_call_summary::shorten_path(&schedule.cwd.to_string_lossy())
        ),
        format!("Next: {}", schedule.trigger.next_run_at().unwrap_or("none")),
        format!("Catch up: {}", catch_up_label(schedule.catch_up)),
    ];
    if let Some(active) = schedule.active_run_id.as_ref() {
        lines.push(format!("Active run: {active}"));
    }
    Ok(text_result(
        lines.join("\n"),
        Some(std::slice::from_ref(&schedule)),
        None,
        false,
    ))
}

/// pi `history` (`scheduled-runs.ts:672-676`).
async fn history(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let schedule = resolve(action, params, ctx).await?;
    let runs = ctx.store.history(&schedule.id).await?;
    let text = if runs.is_empty() {
        format!("No runs recorded for schedule {}.", schedule.id)
    } else {
        let mut lines = vec![format!("Schedule history: {}", schedule.id)];
        for run in &runs {
            lines.push(format!(
                "- {} | {} | {}{}",
                run.id,
                run_state_label(run.state),
                run.planned_at,
                run.async_id
                    .as_deref()
                    .map_or_else(String::new, |id| format!(" | async {id}"))
            ));
        }
        lines.join("\n")
    };
    Ok(text_result(
        text,
        Some(std::slice::from_ref(&schedule)),
        Some(&runs),
        false,
    ))
}

/// pi `pause` (`scheduled-runs.ts:678-688`) — one function, a `paused` flag.
///
/// The idempotent reply is NOT an error: asking for a paused schedule to pause is a no-op the
/// caller should be told about, not a failure.
async fn pause(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
    paused: bool,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let mut schedule = resolve(action, params, ctx).await?;
    if schedule.paused == paused {
        return Ok(text_result(
            format!(
                "Schedule {} is already {}.",
                schedule.id,
                if paused { "paused" } else { "active" }
            ),
            Some(std::slice::from_ref(&schedule)),
            None,
            false,
        ));
    }
    schedule.paused = paused;
    schedule.updated_at = schedule_timestamp(ctx.now);
    ctx.store.write(&schedule).await?;
    ctx.store
        .append_event(
            &schedule,
            if paused {
                "schedule.paused"
            } else {
                "schedule.resumed"
            },
        )
        .await?;
    // `:686` — a RESUME re-runs the crash-recovery pass for this one schedule, so a schedule
    // paused across a crash comes back with its claim reconciled rather than wedged.
    if !paused {
        super::trigger::restore_one(ctx.store, ctx.fire, &mut schedule, ctx.now).await?;
    }
    Ok(text_result(
        format!(
            "{} schedule {}.",
            if paused { "Paused" } else { "Resumed" },
            schedule.id
        ),
        Some(std::slice::from_ref(&schedule)),
        None,
        false,
    ))
}

/// pi `runManual` (`scheduled-runs.ts:690-710`) — `advance = false`, so a manual fire does not eat
/// the next scheduled slot.
async fn run_manual(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let mut schedule = resolve(action, params, ctx).await?;
    if !schedule_belongs_to_session(&schedule, &ctx.fire.session) {
        return Ok(text_result(
            format!(
                "Skipped schedule {}: current session is not its owner.",
                schedule.id
            ),
            Some(std::slice::from_ref(&schedule)),
            None,
            false,
        ));
    }
    let quiet = match params.quiet.as_ref() {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => return Ok(text_result("quiet must be a boolean.", None, None, true)),
    };
    let run = launch(
        ctx.store,
        ctx.fire,
        &mut schedule,
        ctx.now,
        ScheduleDueReason::Manual,
        false,
        Some(quiet),
        ctx.now,
    )
    .await?;
    let mut updated = ctx.store.get(&schedule.id).await?;
    if run.state == ScheduleRunState::Running {
        // `:698-704` — a manual fire that really started SATISFIES the pending occurrence.
        let next = match &updated.trigger {
            ScheduleTrigger::Interval { every_ms, .. } => {
                Some(schedule_timestamp(ctx.now.saturating_add(*every_ms)))
            }
            ScheduleTrigger::Once { .. } => None,
        };
        updated.trigger.set_next_run_at(next);
        updated.updated_at = schedule_timestamp(ctx.now);
        ctx.store.write(&updated).await?;
        ctx.store
            .append_event(&updated, "schedule.manual_satisfied")
            .await?;
    }
    let text = format!(
        "Manual schedule run {}: {}{}.",
        run.id,
        run_state_label(run.state),
        run.async_id
            .as_deref()
            .map_or_else(String::new, |id| format!(" (async {id})"))
    );
    let is_error = run.state == ScheduleRunState::FailedLaunch;
    Ok(text_result(
        text,
        Some(std::slice::from_ref(&updated)),
        Some(std::slice::from_ref(&run)),
        is_error,
    ))
}

/// pi `runDue` (`scheduled-runs.ts:712-723`) — the tick, invoked on demand.
async fn run_due(
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let runs = tick_due_schedules(ctx.store, ctx.fire, ctx.now).await?;
    let (schedules, diagnostics) = ctx.store.list().await;
    for diagnostic in diagnostics {
        tracing::warn!(%diagnostic, "skipping an unreadable schedule record after the due scan");
    }
    let text = if runs.is_empty() {
        "No schedules are due.".to_string()
    } else {
        format!("Processed {} due schedule(s).", runs.len())
    };
    Ok(text_result(text, Some(&schedules), Some(&runs), false))
}

/// pi `remove` (`scheduled-runs.ts:725-741`) — with **the active-run guard**.
///
/// A schedule with a live run is refused, because deleting it would orphan the run: the lock file
/// goes with the directory, and nothing would ever settle the record the run is still writing to.
/// It is safe to delete only when the active record is `Running`, carries BOTH `async_id` and
/// `async_dir`, and that directory's `status.json` reports the SAME run id in a terminal state —
/// i.e. the run really is over and only the settle was missed.
async fn remove(
    action: ScheduledRunAction,
    params: &ScheduledRunActionParams,
    ctx: &ScheduledRunActionContext<'_>,
) -> Result<ScheduledRunActionOutcome, ScheduledRunError> {
    let schedule = resolve(action, params, ctx).await?;
    if let Some(active) = schedule.active_run_id.as_ref() {
        let run = ctx
            .store
            .history(&schedule.id)
            .await?
            .into_iter()
            .find(|item| &item.id == active);
        let mut terminal = false;
        if let Some(run) = run
            && run.schedule_id == schedule.id
            && run.state == ScheduleRunState::Running
            && let (Some(async_id), Some(async_dir)) = (run.async_id.as_deref(), run.async_dir)
        {
            let status =
                crate::background::control::read_status_file(&async_dir.join("status.json"))
                    .await
                    .map_err(|error| ScheduledRunError::Refused(error.to_string()))?;
            terminal = status.is_some_and(|status| {
                status.run_id.as_str() == async_id && status.state.is_terminal()
            });
        }
        if !terminal {
            return Ok(text_result(
                format!(
                    "Schedule {} has active run {active}; stop that run before deleting the schedule.",
                    schedule.id
                ),
                Some(std::slice::from_ref(&schedule)),
                None,
                true,
            ));
        }
    }
    ctx.store
        .append_event(&schedule, "schedule.deleted")
        .await?;
    ctx.store.delete(&schedule.id).await?;
    Ok(text_result(
        format!("Deleted schedule {}.", schedule.id),
        None,
        None,
        false,
    ))
}

// =================================================================================================
// Small shared renderers
// =================================================================================================

/// pi's `paused ? "paused" : activeRunId ? "running" : "scheduled"` (`:663`, `:668`).
fn schedule_state_label(schedule: &ScheduleRecord) -> &'static str {
    if schedule.paused {
        "paused"
    } else if schedule.active_run_id.is_some() {
        "running"
    } else {
        "scheduled"
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn catch_up_label(value: ScheduleCatchUp) -> &'static str {
    match value {
        ScheduleCatchUp::None => "none",
        ScheduleCatchUp::Latest => "latest",
    }
}

/// The wire spelling of a run state, as `history`/`run` render it.
fn run_state_label(value: ScheduleRunState) -> &'static str {
    match value {
        ScheduleRunState::Running => "running",
        ScheduleRunState::Skipped => "skipped",
        ScheduleRunState::Missed => "missed",
        ScheduleRunState::Completed => "completed",
        ScheduleRunState::FailedLaunch => "failed_launch",
        ScheduleRunState::FailedRun => "failed_run",
    }
}

/// pi `samePath` (`scheduled-runs.ts:186-188`).
fn same_path(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if cfg!(windows) {
            PathBuf::from(absolute.to_string_lossy().to_lowercase())
        } else {
            absolute
        }
    };
    normalize(left) == normalize(right)
}

/// pi `randomUUID().slice(0, 8)` (`:530`) — the generated schedule id.
///
/// Eight hex characters from the same v4 UUID source [`super::schedule::ScheduleRunId::mint`]
/// uses, so the crate has one entropy source rather than two. Eight is upstream's `slice(0, 8)`
/// and it is checked for collision by gate 13 (`Schedule '<id>' already exists.`), which is what
/// makes the short form safe.
fn mint_schedule_id() -> String {
    uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect()
}
