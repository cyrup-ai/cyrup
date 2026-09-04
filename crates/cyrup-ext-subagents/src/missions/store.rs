//! The mission record store — a 1:1 port of `pi-subagents/src/missions/store.ts`
//! (507 lines @v0.43.0).
//!
//! Owns four things, and nothing else:
//!
//! 1. **Validation** (`asObject` … `parseMissionRecord`, `store.ts:41-223`). Every value that
//!    crosses the disk boundary — in EITHER direction — is re-validated field by field, with the
//!    upstream error strings reproduced verbatim (they surface to the model as tool errors, so
//!    they are observable behaviour, not diagnostics). `writeMission` deliberately validates on the
//!    WRITE path too (`store.ts:300`), so a malformed record can never be persisted in the first
//!    place.
//! 2. **Placement** (`expandConfiguredPath`/`validateMissionStoreConfig`/
//!    `resolveMissionStoreLocation`/`missionRecordPath`/`indexPath`, `store.ts:225-297`) — where
//!    records and the cross-project pointer index live.
//! 3. **CRUD** (`createMission`/`readMission`/`listMissions`/`updateMission`, `store.ts:334-475`),
//!    including the merge rules that make `addRuns`/`addArtifacts`/`addReceipts` UPSERTS rather
//!    than appends, and the usage-recompute + budget-exhaustion transition.
//! 4. **Retention** (`pruneTerminalMissions`, `store.ts:319-332`) — best-effort, never fatal.
//!
//! # [CYRUP-DELTA] Rebranding
//!
//! The default record directory is `<projectRoot>/.cyrup-subagents/missions`, not upstream's
//! `<projectRoot>/.pi-subagents/missions` (`store.ts:262`) — the same `.pi-subagents` →
//! `.cyrup-subagents` rebrand [`crate::artifacts::project_subagents_dir`] already applies
//! crate-wide, reached through that exact function so there is one owner of the directory name.
//!
//! # [CYRUP-DELTA] `Date.parse` vs. ISO-8601
//!
//! `timestamp()` upstream accepts anything `Date.parse` accepts, which includes a pile of legacy
//! non-ISO formats. [`parse_timestamp`] accepts the ISO-8601 date and date-time forms — which is
//! everything this subsystem itself ever writes (`format_iso8601_millis`) — and rejects the legacy
//! tail. See that function's own doc for the exact grammar.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;
use sha2::Digest as _;

use super::{
    MissionArtifact, MissionArtifactKind, MissionCreateInput, MissionDecision,
    MissionDecisionStatus, MissionError, MissionGoal, MissionGoalStatus, MissionGoalUpdate,
    MissionIndexEntry, MissionListResult, MissionReceipt, MissionReceiptKind,
    MissionReceiptStatus, MissionRecord, MissionResult, MissionRunLink, MissionRunMode,
    MissionStatus, MissionStoreConfig, MissionStoreLocation, MissionTokenBudget, MissionTokenUsage,
    MissionUpdateInput, GlobalMissionIndexRecord, GlobalMissionListResult, MISSION_SCHEMA_VERSION,
    MISSION_STATUSES,
};

/// pi `MISSION_ID_PATTERN` (`store.ts:32`): `^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$`. Implemented as a
/// hand-rolled scan rather than a regex — this crate has no regex dependency, and the pattern is
/// simple enough that a scan is both clearer and faster.
fn matches_mission_id_pattern(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut tail_len = 0usize;
    for c in chars {
        tail_len += 1;
        if tail_len > 127 {
            return false;
        }
        if !(c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return false;
        }
    }
    true
}

/// pi `DEFAULT_TERMINAL_MISSION_RETENTION` (`store.ts:39`).
pub const DEFAULT_TERMINAL_MISSION_RETENTION: u64 = 200;

// =================================================================================================
// Primitive validators (store.ts:41-104)
// =================================================================================================

/// pi `asObject` (`store.ts:41-44`).
fn as_object<'a>(value: &'a Value, label: &str) -> MissionResult<&'a serde_json::Map<String, Value>> {
    value
        .as_object()
        .ok_or_else(|| MissionError::invalid(format!("{label} must be a JSON object")))
}

/// pi `requiredString` (`store.ts:46-49`): a string whose `trim()` is non-empty. Returns the value
/// UNTRIMMED, exactly as upstream does — callers that want a trimmed value trim explicitly.
fn required_string<'a>(value: Option<&'a Value>, label: &str) -> MissionResult<&'a str> {
    match value.and_then(Value::as_str) {
        Some(s) if !s.trim().is_empty() => Ok(s),
        _ => Err(MissionError::invalid(format!("{label} must be a non-empty string"))),
    }
}

/// pi `optionalString` (`store.ts:51-54`): `undefined` (i.e. an absent key) passes through as
/// `None`; anything present is subject to the full [`required_string`] check.
fn optional_string<'a>(value: Option<&'a Value>, label: &str) -> MissionResult<Option<&'a str>> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(_) => required_string(value, label).map(Some),
    }
}

/// pi `timestamp` (`store.ts:56-60`): a non-empty string that `Date.parse` accepts.
///
/// [CYRUP-DELTA] `Date.parse` also accepts a long tail of legacy, implementation-defined formats
/// (`"Mon Jan 01 2024"`, `"1/2/2024"`, …). This accepts the ISO-8601 forms only:
/// `YYYY-MM-DD` optionally followed by `T`/` `, `HH:MM`, optional `:SS`, optional `.fff…`, and an
/// optional `Z` / `±HH:MM` / `±HHMM` offset. Every timestamp this subsystem writes comes from
/// `format_iso8601_millis`, so the narrowing is unreachable from cyrup-written data; it can only
/// reject a hand-edited record pi would have tolerated.
fn parse_timestamp<'a>(value: Option<&'a Value>, label: &str) -> MissionResult<&'a str> {
    let raw = required_string(value, label)?;
    if !is_iso8601_datetime(raw.trim()) {
        return Err(MissionError::invalid(format!("{label} must be an ISO timestamp")));
    }
    Ok(raw)
}

/// The ISO-8601 grammar [`parse_timestamp`] accepts. Total, allocation-free, cannot panic.
fn is_iso8601_datetime(value: &str) -> bool {
    let bytes = value.as_bytes();
    // `YYYY-MM-DD` is the shortest accepted form.
    if bytes.len() < 10 {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| -> bool {
        bytes.get(range).is_some_and(|s| s.iter().all(u8::is_ascii_digit))
    };
    if !digits(0..4) || bytes.get(4) != Some(&b'-') || !digits(5..7) || bytes.get(7) != Some(&b'-')
        || !digits(8..10)
    {
        return false;
    }
    let rest = value.get(10..).unwrap_or_default();
    if rest.is_empty() {
        return true;
    }
    let Some(time) = rest.strip_prefix('T').or_else(|| rest.strip_prefix(' ')) else {
        return false;
    };
    // Split off the trailing offset (`Z`, `+HH:MM`, `-HHMM`) before validating the clock part.
    let (clock, offset) = match time.rfind(['+', '-']) {
        // A leading sign would mean there is no clock part at all.
        Some(idx) if idx > 0 => (time.get(..idx).unwrap_or_default(), time.get(idx..).unwrap_or_default()),
        _ => match time.strip_suffix('Z').or_else(|| time.strip_suffix('z')) {
            Some(clock) => (clock, ""),
            None => (time, ""),
        },
    };
    if !offset.is_empty() {
        let digits_only: String = offset.chars().skip(1).filter(|c| *c != ':').collect();
        if digits_only.len() != 4 || !digits_only.bytes().all(|b| b.is_ascii_digit()) {
            return false;
        }
    }
    let mut parts = clock.split(':');
    let (Some(hh), Some(mm)) = (parts.next(), parts.next()) else {
        return false;
    };
    if hh.len() != 2 || mm.len() != 2 || !hh.bytes().all(|b| b.is_ascii_digit()) || !mm.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    match parts.next() {
        None => parts.next().is_none(),
        Some(seconds) => {
            if parts.next().is_some() {
                return false;
            }
            let (ss, frac) = match seconds.split_once('.') {
                Some((ss, frac)) => (ss, Some(frac)),
                None => (seconds, None),
            };
            if ss.len() != 2 || !ss.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            frac.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
        }
    }
}

/// pi `missionStatus` (`store.ts:62-67`).
fn parse_mission_status(value: Option<&Value>, label: &str) -> MissionResult<MissionStatus> {
    value
        .and_then(Value::as_str)
        .and_then(MissionStatus::from_wire)
        .ok_or_else(|| MissionError::invalid(format!("{label} must be one of {}", mission_status_list())))
}

/// `MISSION_STATUSES.join(", ")` — the exact tail of two upstream error messages
/// (`store.ts:64`, `actions.ts:101`).
pub(super) fn mission_status_list() -> String {
    MISSION_STATUSES.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
}

/// JS `Number.isSafeInteger(value)` for a `serde_json` number: an integral value within
/// ±(2^53 - 1). A fractional or out-of-range JSON number fails, exactly as it does upstream.
fn as_safe_integer(value: Option<&Value>) -> Option<i64> {
    let n = value?.as_i64()?;
    (n.abs() <= 9_007_199_254_740_991).then_some(n)
}

/// pi `positiveTokenCount` (`store.ts:69-72`).
fn positive_token_count(value: Option<&Value>, label: &str) -> MissionResult<u64> {
    match as_safe_integer(value) {
        Some(n) if n >= 1 => Ok(n.unsigned_abs()),
        _ => Err(MissionError::invalid(format!("{label} must be a positive integer"))),
    }
}

/// pi `nonNegativeTokenCount` (`store.ts:74-77`).
fn non_negative_token_count(value: Option<&Value>, label: &str) -> MissionResult<u64> {
    match as_safe_integer(value) {
        Some(n) if n >= 0 => Ok(n.unsigned_abs()),
        _ => Err(MissionError::invalid(format!("{label} must be a non-negative integer"))),
    }
}

/// pi `parseStoredGoal` (`store.ts:79-82`): a LEGACY record wrote its objective into `goal` as a
/// bare string. Such a record has no goal mode — the string is its objective.
struct StoredGoal {
    goal: Option<MissionGoal>,
    legacy_objective: Option<String>,
}

fn parse_stored_goal(value: &Value, label: &str) -> MissionResult<StoredGoal> {
    if value.is_string() {
        let s = required_string(Some(value), label)?;
        return Ok(StoredGoal { goal: None, legacy_objective: Some(s.trim().to_string()) });
    }
    Ok(StoredGoal { goal: Some(parse_goal(value, label)?), legacy_objective: None })
}

/// pi `parseGoal` (`store.ts:84-88`).
fn parse_goal(value: &Value, label: &str) -> MissionResult<MissionGoal> {
    let input = as_object(value, label)?;
    let status = input
        .get("status")
        .and_then(Value::as_str)
        .and_then(MissionGoalStatus::from_wire)
        .ok_or_else(|| MissionError::invalid(format!("{label}.status is invalid")))?;
    Ok(MissionGoal { status })
}

/// pi `parseBudget` (`store.ts:90-93`).
fn parse_budget(value: &Value, label: &str) -> MissionResult<MissionTokenBudget> {
    let input = as_object(value, label)?;
    Ok(MissionTokenBudget {
        tokens: positive_token_count(input.get("tokens"), &format!("{label}.tokens"))?,
    })
}

/// pi `parseUsage` (`store.ts:95-98`).
fn parse_usage(value: &Value, label: &str) -> MissionResult<MissionTokenUsage> {
    let input = as_object(value, label)?;
    Ok(MissionTokenUsage {
        tokens: non_negative_token_count(input.get("tokens"), &format!("{label}.tokens"))?,
    })
}

/// pi `stringArray` (`store.ts:100-104`): every element a non-empty string, TRIMMED, then
/// de-duplicated with `[...new Set(result)]` — which preserves FIRST-occurrence order.
fn parse_string_array(value: &Value, label: &str) -> MissionResult<Vec<String>> {
    let items = value
        .as_array()
        .ok_or_else(|| MissionError::invalid(format!("{label} must be an array of non-empty strings")))?;
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(items.len());
    for (index, item) in items.iter().enumerate() {
        let trimmed = required_string(Some(item), &format!("{label}[{index}]"))?.trim().to_string();
        if seen.insert(trimmed.clone()) {
            out.push(trimmed);
        }
    }
    Ok(out)
}

/// pi `validateMissionId` (`store.ts:106-112`). Exported: `workflow-state.ts:18` and
/// `lifecycle.ts:268` both call it directly.
///
/// # Errors
///
/// [`MissionError::Invalid`] when the value is not a non-empty string, does not match the id
/// pattern, or contains `..`.
pub fn validate_mission_id(value: Option<&Value>, label: &str) -> MissionResult<String> {
    let id = required_string(value, label)?;
    validate_mission_id_str(id, label)
}

/// [`validate_mission_id`] over a value already known to be a string.
///
/// # Errors
///
/// As [`validate_mission_id`].
pub fn validate_mission_id_str(id: &str, label: &str) -> MissionResult<String> {
    if id.trim().is_empty() {
        return Err(MissionError::invalid(format!("{label} must be a non-empty string")));
    }
    if !matches_mission_id_pattern(id) || id.contains("..") {
        return Err(MissionError::invalid(format!(
            "{label} must contain only letters, numbers, '.', '_', or '-' and cannot contain '..'"
        )));
    }
    Ok(id.to_string())
}

// =================================================================================================
// Composite parsers (store.ts:114-223)
// =================================================================================================

/// pi `parseRunLink` (`store.ts:114-133`).
fn parse_run_link(value: &Value, label: &str) -> MissionResult<MissionRunLink> {
    let input = as_object(value, label)?;
    let run_id = required_string(input.get("runId"), &format!("{label}.runId"))?.to_string();
    let raw_mode = required_string(input.get("mode"), &format!("{label}.mode"))?;
    let mode = MissionRunMode::from_wire(raw_mode)
        .ok_or_else(|| MissionError::invalid(format!("{label}.mode is invalid")))?;
    let child_index = match input.get("childIndex") {
        None | Some(Value::Null) => None,
        Some(v) => Some(v.as_u64().ok_or_else(|| {
            MissionError::invalid(format!("{label}.childIndex must be a non-negative integer"))
        })?),
    };
    Ok(MissionRunLink {
        run_id,
        mode,
        async_dir: optional_string(input.get("asyncDir"), &format!("{label}.asyncDir"))?
            .map(str::to_string),
        child_index,
        agent: optional_string(input.get("agent"), &format!("{label}.agent"))?.map(str::to_string),
        status: optional_string(input.get("status"), &format!("{label}.status"))?
            .map(str::to_string),
        started_at: match input.get("startedAt") {
            None | Some(Value::Null) => None,
            v => Some(parse_timestamp(v, &format!("{label}.startedAt"))?.to_string()),
        },
        completed_at: match input.get("completedAt") {
            None | Some(Value::Null) => None,
            v => Some(parse_timestamp(v, &format!("{label}.completedAt"))?.to_string()),
        },
        usage: match input.get("usage") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_usage(v, &format!("{label}.usage"))?),
        },
    })
}

/// pi `parseDecision` (`store.ts:135-150`).
fn parse_decision(value: &Value, label: &str) -> MissionResult<MissionDecision> {
    let input = as_object(value, label)?;
    let status = match input.get("status").and_then(Value::as_str) {
        Some("open") => MissionDecisionStatus::Open,
        Some("resolved") => MissionDecisionStatus::Resolved,
        _ => {
            return Err(MissionError::invalid(format!(
                "{label}.status must be \"open\" or \"resolved\""
            )));
        }
    };
    Ok(MissionDecision {
        id: validate_mission_id(input.get("id"), &format!("{label}.id"))?,
        status,
        title: required_string(input.get("title"), &format!("{label}.title"))?.to_string(),
        created_at: parse_timestamp(input.get("createdAt"), &format!("{label}.createdAt"))?
            .to_string(),
        prompt: optional_string(input.get("prompt"), &format!("{label}.prompt"))?
            .map(str::to_string),
        options: match input.get("options") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_string_array(v, &format!("{label}.options"))?),
        },
        recommendation: optional_string(
            input.get("recommendation"),
            &format!("{label}.recommendation"),
        )?
        .map(str::to_string),
        resolved_at: match input.get("resolvedAt") {
            None | Some(Value::Null) => None,
            v => Some(parse_timestamp(v, &format!("{label}.resolvedAt"))?.to_string()),
        },
        resolution: optional_string(input.get("resolution"), &format!("{label}.resolution"))?
            .map(str::to_string),
    })
}

/// pi `parseArtifact` (`store.ts:152-161`).
fn parse_artifact(value: &Value, label: &str) -> MissionResult<MissionArtifact> {
    let input = as_object(value, label)?;
    let raw_kind = required_string(input.get("kind"), &format!("{label}.kind"))?;
    let kind = MissionArtifactKind::from_wire(raw_kind)
        .ok_or_else(|| MissionError::invalid(format!("{label}.kind is invalid")))?;
    Ok(MissionArtifact {
        kind,
        path: required_string(input.get("path"), &format!("{label}.path"))?.to_string(),
        description: optional_string(input.get("description"), &format!("{label}.description"))?
            .map(str::to_string),
    })
}

/// pi `parseReceipt` (`store.ts:163-183`), including the `new URL(url)` absoluteness check.
fn parse_receipt(value: &Value, label: &str) -> MissionResult<MissionReceipt> {
    let input = as_object(value, label)?;
    let raw_kind = required_string(input.get("kind"), &format!("{label}.kind"))?;
    let raw_status = required_string(input.get("status"), &format!("{label}.status"))?;
    let kind = MissionReceiptKind::from_wire(raw_kind)
        .ok_or_else(|| MissionError::invalid(format!("{label}.kind is invalid")))?;
    let status = MissionReceiptStatus::from_wire(raw_status)
        .ok_or_else(|| MissionError::invalid(format!("{label}.status is invalid")))?;
    let url = required_string(input.get("url"), &format!("{label}.url"))?;
    if !is_absolute_url(url) {
        return Err(MissionError::invalid(format!("{label}.url must be an absolute URL")));
    }
    Ok(MissionReceipt {
        kind,
        status,
        title: required_string(input.get("title"), &format!("{label}.title"))?.to_string(),
        url: url.to_string(),
        created_at: parse_timestamp(input.get("createdAt"), &format!("{label}.createdAt"))?
            .to_string(),
        description: optional_string(input.get("description"), &format!("{label}.description"))?
            .map(str::to_string),
    })
}

/// pi's `new URL(url)` absoluteness test (`store.ts:170-174`, `actions.ts:161-165`), delegated to
/// the `url` crate — which implements the SAME WHATWG URL Standard `new URL()` does, so
/// `"https://x/y"`, `"mailto:a@b"` and `"custom:anything"` parse while `"example.com"`,
/// `"/path"` and `"http://"` do not.
pub(super) fn is_absolute_url(value: &str) -> bool {
    url::Url::parse(value).is_ok()
}

/// pi `parseMissionRecord` (`store.ts:185-223`). Exported: `inspectors/herdr/inspector-runner.ts:24`
/// calls it directly on an arbitrary file.
///
/// # Errors
///
/// [`MissionError::Invalid`] carrying the exact upstream message for the first field that fails.
pub fn parse_mission_record(value: &Value, source: &str) -> MissionResult<MissionRecord> {
    let input = as_object(value, source)?;
    if input.get("schemaVersion").and_then(Value::as_u64) != Some(u64::from(MISSION_SCHEMA_VERSION))
    {
        return Err(MissionError::invalid(format!("{source}.schemaVersion must be 1")));
    }
    let runs = input
        .get("runs")
        .and_then(Value::as_array)
        .ok_or_else(|| MissionError::invalid(format!("{source}.runs must be an array")))?;
    let decisions = input
        .get("decisions")
        .and_then(Value::as_array)
        .ok_or_else(|| MissionError::invalid(format!("{source}.decisions must be an array")))?;
    let artifacts = input
        .get("artifacts")
        .and_then(Value::as_array)
        .ok_or_else(|| MissionError::invalid(format!("{source}.artifacts must be an array")))?;
    let receipts: &[Value] = match input.get("receipts") {
        None | Some(Value::Null) => &[],
        Some(v) => v
            .as_array()
            .ok_or_else(|| MissionError::invalid(format!("{source}.receipts must be an array")))?
            .as_slice(),
    };
    let parsed_goal = match input.get("goal") {
        None | Some(Value::Null) => StoredGoal { goal: None, legacy_objective: None },
        Some(v) => parse_stored_goal(v, &format!("{source}.goal"))?,
    };
    let budget = match input.get("budget") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_budget(v, &format!("{source}.budget"))?),
    };
    let usage = match input.get("usage") {
        None | Some(Value::Null) => None,
        Some(v) => Some(parse_usage(v, &format!("{source}.usage"))?),
    };
    let objective = optional_string(input.get("objective"), &format!("{source}.objective"))?
        .map(|s| s.trim().to_string())
        .or(parsed_goal.legacy_objective)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            MissionError::invalid(format!("{source}.objective must be a non-empty string"))
        })?;
    if parsed_goal.goal.is_some() && budget.is_none() {
        return Err(MissionError::invalid(format!(
            "{source}.budget is required for a goal mission"
        )));
    }
    Ok(MissionRecord {
        schema_version: MISSION_SCHEMA_VERSION,
        id: validate_mission_id(input.get("id"), &format!("{source}.id"))?,
        title: required_string(input.get("title"), &format!("{source}.title"))?.to_string(),
        objective,
        goal: parsed_goal.goal,
        budget,
        usage,
        status: parse_mission_status(input.get("status"), &format!("{source}.status"))?,
        created_at: parse_timestamp(input.get("createdAt"), &format!("{source}.createdAt"))?
            .to_string(),
        updated_at: parse_timestamp(input.get("updatedAt"), &format!("{source}.updatedAt"))?
            .to_string(),
        runs: runs
            .iter()
            .enumerate()
            .map(|(i, item)| parse_run_link(item, &format!("{source}.runs[{i}]")))
            .collect::<MissionResult<Vec<_>>>()?,
        decisions: decisions
            .iter()
            .enumerate()
            .map(|(i, item)| parse_decision(item, &format!("{source}.decisions[{i}]")))
            .collect::<MissionResult<Vec<_>>>()?,
        artifacts: artifacts
            .iter()
            .enumerate()
            .map(|(i, item)| parse_artifact(item, &format!("{source}.artifacts[{i}]")))
            .collect::<MissionResult<Vec<_>>>()?,
        receipts: receipts
            .iter()
            .enumerate()
            .map(|(i, item)| parse_receipt(item, &format!("{source}.receipts[{i}]")))
            .collect::<MissionResult<Vec<_>>>()?,
        cwd: optional_string(input.get("cwd"), &format!("{source}.cwd"))?.map(str::to_string),
        owner_session_id: optional_string(
            input.get("ownerSessionId"),
            &format!("{source}.ownerSessionId"),
        )?
        .map(str::to_string),
        summary: optional_string(input.get("summary"), &format!("{source}.summary"))?
            .map(str::to_string),
        // NOT `optional_string`-gated: `acceptance?: unknown` upstream, carried through verbatim
        // including an explicit `null` (which is a value the caller chose, not an absent key).
        acceptance: input.get("acceptance").cloned(),
        labels: match input.get("labels") {
            None | Some(Value::Null) => None,
            Some(v) => Some(parse_string_array(v, &format!("{source}.labels"))?),
        },
    })
}

// =================================================================================================
// Placement (store.ts:225-297)
// =================================================================================================

/// Lexical path normalization — Node's `path.normalize`/`path.resolve` collapse `.` and `..`
/// WITHOUT touching the filesystem, and `std::path` has no equivalent, so this reproduces it.
fn normalize_lexically(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// pi `expandConfiguredPath` (`store.ts:225-228`): `~/`-relative against `$HOME`, absolute
/// normalized in place, otherwise resolved against `projectRoot`.
fn expand_configured_path(value: &str, project_root: &Path) -> PathBuf {
    let expanded: PathBuf = if let Some(rest) = value.strip_prefix("~/") {
        crate::paths::home_dir().join(rest)
    } else {
        PathBuf::from(value)
    };
    if expanded.is_absolute() {
        normalize_lexically(&expanded)
    } else {
        normalize_lexically(&project_root.join(expanded))
    }
}

/// pi `validateMissionStoreConfig` (`store.ts:230-252`) — the `config.missions` block validator
/// `extension/config.ts:25` runs on every config read.
///
/// # Errors
///
/// [`MissionError::Invalid`] for an unknown key or a wrongly-typed known key.
pub fn validate_mission_store_config(
    value: Option<&Value>,
    label: &str,
) -> MissionResult<Option<MissionStoreConfig>> {
    let Some(value) = value else { return Ok(None) };
    if value.is_null() {
        return Ok(None);
    }
    let input = as_object(value, label)?;
    for key in input.keys() {
        if !matches!(
            key.as_str(),
            "enabled" | "directory" | "globalIndex" | "globalIndexDir" | "retainTerminal"
        ) {
            return Err(MissionError::invalid(format!("{label}.{key} is unknown")));
        }
    }
    let enabled = match input.get("enabled") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => return Err(MissionError::invalid(format!("{label}.enabled must be boolean"))),
    };
    let global_index = match input.get("globalIndex") {
        None => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => {
            return Err(MissionError::invalid(format!("{label}.globalIndex must be boolean")));
        }
    };
    let retain_terminal = match input.get("retainTerminal") {
        None => None,
        Some(v) => match v.as_i64() {
            Some(n) if n >= 1 => Some(n.unsigned_abs()),
            _ => {
                return Err(MissionError::invalid(format!(
                    "{label}.retainTerminal must be a positive integer"
                )));
            }
        },
    };
    let directory = optional_string(input.get("directory"), &format!("{label}.directory"))?;
    let global_index_dir =
        optional_string(input.get("globalIndexDir"), &format!("{label}.globalIndexDir"))?;
    Ok(Some(MissionStoreConfig {
        enabled,
        directory: directory.map(str::to_string),
        global_index,
        global_index_dir: global_index_dir.map(str::to_string),
        retain_terminal,
    }))
}

/// pi `resolveMissionStoreLocation` (`store.ts:254-273`).
///
/// [CYRUP-DELTA] the default record directory is `<projectRoot>/.cyrup-subagents/missions`
/// (upstream: `.pi-subagents/missions`), via [`crate::artifacts::project_subagents_dir`].
///
/// # The `global_index_dir` default is REAL user config — never take it in a test
///
/// With no `config.globalIndexDir` and no `agent_dir_override`, the pointer index lands in
/// [`crate::paths::agent_dir`]`/missions/index` — `~/.cyrup/agent/missions/index` — the same directory that
/// holds `settings.json`, `models-store.json` and `sessions/`. That is faithful to upstream's
/// `path.join(input.agentDir ?? getAgentDir(), "missions", "index")` (`store.ts:265`) and must
/// stay; the pointer index is deliberately cross-project, so it cannot live under a project root.
///
/// A test therefore has to scope it, and upstream's own fixtures show both ways of doing it:
/// `test/unit/mission-goal-driver.test.ts:15` passes `agentDir: path.join(root, "agent")` (this
/// function's `agent_dir_override`), while `test/unit/mission-lifecycle.test.ts:18`'s
/// `projectFixture()` returns `missionConfig: { globalIndexDir: path.join(root, "global-index") }`
/// and threads it through every `prepareMissionLaunch` — because
/// [`super::lifecycle::prepare_mission_launch`] has no override parameter to pass.
/// A tempdir `project_root` alone does NOT isolate this path.
#[must_use]
pub fn resolve_mission_store_location(
    project_root: &Path,
    config: Option<&MissionStoreConfig>,
    agent_dir_override: Option<&Path>,
) -> MissionStoreLocation {
    let project_root = normalize_lexically(&if project_root.is_absolute() {
        project_root.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(project_root)
    });
    let mission_dir = match config.and_then(|c| c.directory.as_deref()) {
        Some(dir) => expand_configured_path(dir, &project_root),
        None => crate::artifacts::project_subagents_dir(&project_root).join("missions"),
    };
    let global_index_dir = match config.and_then(|c| c.global_index_dir.as_deref()) {
        Some(dir) => expand_configured_path(dir, &project_root),
        None => agent_dir_override
            .map_or_else(crate::paths::agent_dir, Path::to_path_buf)
            .join("missions")
            .join("index"),
    };
    MissionStoreLocation {
        project_root,
        mission_dir,
        global_index_dir,
        // pi: `input.config?.globalIndex !== false` — an ABSENT flag means "write it".
        write_global_index: config.and_then(|c| c.global_index) != Some(false),
        retain_terminal: config.and_then(|c| c.retain_terminal),
    }
}

/// pi `missionRecordPath` (`store.ts:275-277`).
///
/// # Errors
///
/// [`MissionError::Invalid`] when `mission_id` fails [`validate_mission_id_str`] — the id is a
/// path component, so this is the traversal guard, not a cosmetic check.
pub fn mission_record_path(
    location: &MissionStoreLocation,
    mission_id: &str,
) -> MissionResult<PathBuf> {
    let id = validate_mission_id_str(mission_id, "missionId")?;
    Ok(location.mission_dir.join(format!("{id}.json")))
}

/// pi `parseIndexEntry` (`store.ts:279-292`).
fn parse_index_entry(value: &Value, source: &str) -> MissionResult<MissionIndexEntry> {
    let input = as_object(value, source)?;
    if input.get("schemaVersion").and_then(Value::as_u64) != Some(u64::from(MISSION_SCHEMA_VERSION))
    {
        return Err(MissionError::invalid(format!("{source}.schemaVersion must be 1")));
    }
    Ok(MissionIndexEntry {
        schema_version: MISSION_SCHEMA_VERSION,
        mission_id: validate_mission_id(input.get("missionId"), &format!("{source}.missionId"))?,
        project_root: required_string(input.get("projectRoot"), &format!("{source}.projectRoot"))?
            .to_string(),
        record_path: required_string(input.get("recordPath"), &format!("{source}.recordPath"))?
            .to_string(),
        title: required_string(input.get("title"), &format!("{source}.title"))?.to_string(),
        status: parse_mission_status(input.get("status"), &format!("{source}.status"))?,
        updated_at: parse_timestamp(input.get("updatedAt"), &format!("{source}.updatedAt"))?
            .to_string(),
        last_run_id: optional_string(input.get("lastRunId"), &format!("{source}.lastRunId"))?
            .map(str::to_string),
    })
}

/// pi `indexPath` (`store.ts:294-297`): `sha256(projectRoot + "\0" + missionId)` hex, so a
/// pointer's filename is stable per (project, mission) without leaking either into the name.
fn index_path(location: &MissionStoreLocation, record: &MissionRecord) -> PathBuf {
    let mut hasher = sha2::Sha256::new();
    hasher.update(location.project_root.to_string_lossy().as_bytes());
    hasher.update([0u8]);
    hasher.update(record.id.as_bytes());
    let key = hasher.finalize();
    let hex: String = key.iter().map(|b| format!("{b:02x}")).collect();
    location.global_index_dir.join(format!("{hex}.json"))
}

// =================================================================================================
// CRUD (store.ts:299-475)
// =================================================================================================

/// pi `writeMission` (`store.ts:299-317`): re-validate, persist, then refresh the pointer index.
fn write_mission(
    location: &MissionStoreLocation,
    record: &MissionRecord,
) -> MissionResult<MissionRecord> {
    let as_value = serde_json::to_value(record)
        .map_err(|e| MissionError::invalid(format!("mission record is not serializable: {e}")))?;
    let validated = parse_mission_record(&as_value, "mission record")?;
    let record_path = mission_record_path(location, &validated.id)?;
    super::write_private_atomic_json(&record_path, &validated)?;
    if location.write_global_index {
        let entry = MissionIndexEntry {
            schema_version: MISSION_SCHEMA_VERSION,
            mission_id: validated.id.clone(),
            project_root: location.project_root.to_string_lossy().into_owned(),
            record_path: record_path.to_string_lossy().into_owned(),
            title: validated.title.clone(),
            status: validated.status,
            updated_at: validated.updated_at.clone(),
            last_run_id: validated.runs.last().map(|run| run.run_id.clone()),
        };
        super::write_private_atomic_json(&index_path(location, &validated), &entry)?;
    }
    Ok(validated)
}

/// pi `pruneTerminalMissions` (`store.ts:319-332`): keep the `max_terminal` most recently updated
/// terminal missions, delete the rest (record, per-mission state dir, pointer). Best-effort — a
/// failure here "must never block a launch", so nothing is propagated.
fn prune_terminal_missions(location: &MissionStoreLocation, max_terminal: u64) {
    let mut terminal: Vec<MissionRecord> = list_missions(location)
        .records
        .into_iter()
        .filter(|record| record.status.is_terminal())
        .collect();
    // `right.updatedAt.localeCompare(left.updatedAt)` — descending, i.e. newest first.
    terminal.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    let keep = usize::try_from(max_terminal).unwrap_or(usize::MAX);
    for record in terminal.into_iter().skip(keep) {
        if let Ok(path) = mission_record_path(location, &record.id) {
            let _ = std::fs::remove_file(&path);
        }
        if let Ok(id) = validate_mission_id_str(&record.id, "missionId") {
            let _ = std::fs::remove_dir_all(location.mission_dir.join(id));
        }
        if location.write_global_index {
            let _ = std::fs::remove_file(index_path(location, &record));
        }
    }
}

/// The effective terminal-retention budget: an explicit per-call override, else the location's
/// own, else [`DEFAULT_TERMINAL_MISSION_RETENTION`] — pi's
/// `retainTerminal = location.retainTerminal ?? DEFAULT_TERMINAL_MISSION_RETENTION` default
/// parameter, called with an explicit `config?.retainTerminal` that may itself be `undefined`
/// (`lifecycle.ts:85`, `actions.ts:336`).
fn effective_retention(location: &MissionStoreLocation, override_value: Option<u64>) -> u64 {
    override_value
        .or(location.retain_terminal)
        .unwrap_or(DEFAULT_TERMINAL_MISSION_RETENTION)
}

/// pi `createMission` (`store.ts:334-359`).
///
/// # Errors
///
/// [`MissionError::Invalid`] when the input fails validation (including "budget is required when
/// goal is true"), or [`MissionError::Io`] when the record cannot be persisted.
pub fn create_mission(
    location: &MissionStoreLocation,
    input: &MissionCreateInput,
    now_ms: i64,
    retain_terminal: Option<u64>,
) -> MissionResult<MissionRecord> {
    let created_at = super::format_iso8601_millis(now_ms);
    let goal_enabled = input.goal == Some(true);
    let record = MissionRecord {
        schema_version: MISSION_SCHEMA_VERSION,
        id: uuid::Uuid::new_v4().hyphenated().to_string(),
        title: required_string(Some(&Value::String(input.title.clone())), "mission.title")?
            .trim()
            .to_string(),
        objective: required_string(
            Some(&Value::String(input.objective.clone())),
            "mission.objective",
        )?
        .trim()
        .to_string(),
        goal: goal_enabled.then_some(MissionGoal { status: MissionGoalStatus::Active }),
        budget: match &input.budget {
            None => None,
            Some(budget) => Some(parse_budget(
                &serde_json::json!({ "tokens": budget.tokens }),
                "mission.budget",
            )?),
        },
        usage: goal_enabled.then_some(MissionTokenUsage { tokens: 0 }),
        status: input.status.unwrap_or(MissionStatus::Planned),
        created_at: created_at.clone(),
        updated_at: created_at,
        runs: Vec::new(),
        decisions: Vec::new(),
        artifacts: Vec::new(),
        receipts: Vec::new(),
        cwd: Some(location.project_root.to_string_lossy().into_owned()),
        owner_session_id: match &input.owner_session_id {
            None => None,
            Some(id) => Some(
                required_string(Some(&Value::String(id.clone())), "mission.ownerSessionId")?
                    .to_string(),
            ),
        },
        summary: None,
        acceptance: None,
        labels: match &input.labels {
            None => None,
            Some(labels) => Some(parse_string_array(
                &Value::Array(labels.iter().cloned().map(Value::String).collect()),
                "mission.labels",
            )?),
        },
    };
    // pi asserts this AFTER building the literal (`store.ts:355`), which matters: a
    // `goal: true`/no-budget input reports the goal/budget mismatch, not a parse error.
    if goal_enabled && input.budget.is_none() {
        return Err(MissionError::invalid(
            "mission.budget is required when mission.goal is true",
        ));
    }
    let created = write_mission(location, &record)?;
    prune_terminal_missions(location, effective_retention(location, retain_terminal));
    Ok(created)
}

/// pi `readMission` (`store.ts:374-388`).
///
/// # Errors
///
/// [`MissionError::NotFound`] when no record file exists (pi's `MissionNotFoundError`, which
/// `lifecycle.ts:299` branches on by identity), [`MissionError::Io`] for any other read failure,
/// and [`MissionError::Invalid`] — wrapped as `Invalid mission file '<path>': <why>` — when the
/// file exists but does not validate.
pub fn read_mission(
    location: &MissionStoreLocation,
    mission_id: &str,
) -> MissionResult<MissionRecord> {
    let file_path = mission_record_path(location, mission_id)?;
    let raw = match std::fs::read_to_string(&file_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(MissionError::NotFound {
                mission_id: mission_id.to_string(),
                mission_dir: location.mission_dir.clone(),
            });
        }
        Err(err) => return Err(MissionError::io(&file_path, err)),
    };
    parse_json(&raw)
        .and_then(|value| parse_mission_record(&value, &file_path.to_string_lossy()))
        .map_err(|e| {
            MissionError::invalid(format!(
                "Invalid mission file '{}': {e}",
                file_path.to_string_lossy()
            ))
        })
}

/// `JSON.parse`, with the failure surfaced as a [`MissionError::Invalid`] so it composes into the
/// same `Invalid mission file '<path>': <why>` wrapper upstream's `try`/`catch` produces.
fn parse_json(raw: &str) -> MissionResult<Value> {
    serde_json::from_str(raw).map_err(|e| MissionError::invalid(e.to_string()))
}

/// pi `listMissions` (`store.ts:390-404`): every `*.json` directly under the mission dir, in
/// filename order, each parsed independently; a corrupt file becomes a WARNING, never an error.
/// The result is re-sorted by `updatedAt` DESCENDING.
#[must_use]
pub fn list_missions(location: &MissionStoreLocation) -> MissionListResult {
    let Ok(dir) = std::fs::read_dir(&location.mission_dir) else {
        return MissionListResult::default();
    };
    let mut names: Vec<PathBuf> = dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    names.sort();
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for file_path in names {
        let display = file_path.to_string_lossy().into_owned();
        match std::fs::read_to_string(&file_path)
            .map_err(|e| MissionError::invalid(e.to_string()))
            .and_then(|raw| parse_json(&raw))
            .and_then(|value| parse_mission_record(&value, &display))
        {
            Ok(record) => records.push(record),
            Err(e) => warnings.push(format!("Skipped corrupt mission '{display}': {e}")),
        }
    }
    records.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    MissionListResult { records, warnings }
}

/// pi `updateMission` (`store.ts:439-547` @v0.64.0; `:406-475` @v0.43.0) — the merge core of the
/// whole subsystem.
///
/// The four `add*` lists are UPSERTS, each with its own identity:
/// * runs on `(runId, childIndex)`, merged field-by-field over the existing link;
/// * artifacts on `(kind, resolved path)`, likewise merged;
/// * receipts on `(kind, url)`, replaced but KEEPING the original `createdAt`;
/// * decisions are always appended as NEW, open decisions with fresh ids — and then
///   `resolve_decision` (SUBA-085) flips exactly one existing open decision to `resolved`.
///
/// Usage is RECOMPUTED from `runs[].usage` unless explicitly supplied, and goal status then
/// transitions to `budget-exhausted` (or back to `active`) against the budget. The mission status
/// is gated on open decisions (`store.ts:521-529` @v0.64.0): see the inline note at the gate.
///
/// # Errors
///
/// [`MissionError::NotFound`] when the record is missing, [`MissionError::Invalid`] for any
/// failed validation (including "budget is required when enabling a goal mission", an unknown
/// or already-resolved decision id), or [`MissionError::Io`] when the merged record cannot be
/// persisted.
pub fn update_mission(
    location: &MissionStoreLocation,
    mission_id: &str,
    update: &MissionUpdateInput,
    now_ms: i64,
    retain_terminal: Option<u64>,
) -> MissionResult<MissionRecord> {
    let current = read_mission(location, mission_id)?;
    let created_at = super::format_iso8601_millis(now_ms);

    // ---- runs: upsert on (runId, childIndex) -------------------------------------------------
    let mut runs = current.runs.clone();
    for candidate in &update.add_runs {
        let run = parse_run_link(
            &serde_json::to_value(candidate)
                .map_err(|e| MissionError::invalid(e.to_string()))?,
            "mission.update.addRuns[]",
        )?;
        match runs
            .iter()
            .position(|item| item.run_id == run.run_id && item.child_index == run.child_index)
        {
            None => runs.push(run),
            Some(index) => {
                if let Some(existing) = runs.get_mut(index) {
                    *existing = merge_run_link(existing, &run);
                }
            }
        }
    }

    // ---- artifacts: upsert on (kind, path.resolve(path)) -------------------------------------
    let mut artifacts = current.artifacts.clone();
    for candidate in &update.add_artifacts {
        let artifact = parse_artifact(
            &serde_json::to_value(candidate)
                .map_err(|e| MissionError::invalid(e.to_string()))?,
            "mission.update.addArtifacts[]",
        )?;
        let resolved = resolve_for_comparison(&artifact.path);
        match artifacts.iter().position(|item| {
            item.kind == artifact.kind && resolve_for_comparison(&item.path) == resolved
        }) {
            None => artifacts.push(artifact),
            Some(index) => {
                if let Some(existing) = artifacts.get_mut(index) {
                    *existing = merge_artifact(existing, &artifact);
                }
            }
        }
    }

    // ---- receipts: upsert on (kind, url), preserving the ORIGINAL createdAt -------------------
    let mut receipts = current.receipts.clone();
    for candidate in &update.add_receipts {
        let receipt = parse_receipt(
            &serde_json::json!({
                "kind": candidate.kind.as_str(),
                "status": candidate.status.as_str(),
                "title": candidate.title,
                "url": candidate.url,
                "createdAt": created_at,
                "description": candidate.description,
            }),
            "mission.update.addReceipts[]",
        )?;
        match receipts
            .iter()
            .position(|item| item.kind == receipt.kind && item.url == receipt.url)
        {
            None => receipts.push(receipt),
            Some(index) => {
                if let Some(existing) = receipts.get_mut(index) {
                    let original_created_at = existing.created_at.clone();
                    *existing = MissionReceipt { created_at: original_created_at, ..receipt };
                }
            }
        }
    }

    // ---- decisions: always appended as fresh, OPEN decisions ----------------------------------
    let mut decisions = current.decisions.clone();
    for decision in &update.add_decisions {
        decisions.push(MissionDecision {
            id: uuid::Uuid::new_v4().hyphenated().to_string(),
            status: MissionDecisionStatus::Open,
            title: required_string(
                Some(&Value::String(decision.title.clone())),
                "mission.update.addDecisions[].title",
            )?
            .to_string(),
            created_at: created_at.clone(),
            // pi guards each of these on TRUTHINESS (`decision.prompt ? … : {}`), so an empty
            // string is dropped rather than validated-and-rejected.
            prompt: match decision.prompt.as_deref().filter(|s| !s.is_empty()) {
                None => None,
                Some(p) => Some(
                    required_string(
                        Some(&Value::String(p.to_string())),
                        "mission.update.addDecisions[].prompt",
                    )?
                    .to_string(),
                ),
            },
            options: match &decision.options {
                None => None,
                Some(options) => Some(parse_string_array(
                    &Value::Array(options.iter().cloned().map(Value::String).collect()),
                    "mission.update.addDecisions[].options",
                )?),
            },
            recommendation: match decision.recommendation.as_deref().filter(|s| !s.is_empty()) {
                None => None,
                Some(r) => Some(
                    required_string(
                        Some(&Value::String(r.to_string())),
                        "mission.update.addDecisions[].recommendation",
                    )?
                    .to_string(),
                ),
            },
            resolved_at: None,
            resolution: None,
        });
    }

    // ---- resolve ONE open decision (SUBA-085, `store.ts:497-508` @v0.64.0) --------------------
    // Runs AFTER the append loop, over the merged list, exactly as upstream does. Both refusals
    // are upstream's verbatim text; the resolution is validated as a non-empty string and stored
    // trimmed, and `resolvedAt` is the same `createdAt` stamp the rest of this update carries.
    if let Some(resolve) = &update.resolve_decision {
        let decision_id =
            validate_mission_id_str(&resolve.id, "mission.update.resolveDecision.id")?;
        let Some(decision) = decisions
            .iter_mut()
            .find(|decision| decision.id == decision_id)
        else {
            return Err(MissionError::invalid(format!(
                "Decision '{decision_id}' was not found in mission '{mission_id}'"
            )));
        };
        if decision.status == MissionDecisionStatus::Resolved {
            return Err(MissionError::invalid(format!(
                "Decision '{decision_id}' is already resolved"
            )));
        }
        let resolution = required_string(
            Some(&Value::String(resolve.resolution.clone())),
            "mission.update.resolveDecision.resolution",
        )?
        .trim()
        .to_string();
        decision.status = MissionDecisionStatus::Resolved;
        decision.resolved_at = Some(created_at.clone());
        decision.resolution = Some(resolution);
    }

    // ---- budget / usage / goal ----------------------------------------------------------------
    let budget = match &update.budget {
        Some(b) => Some(parse_budget(
            &serde_json::json!({ "tokens": b.tokens }),
            "mission.update.budget",
        )?),
        None => current.budget,
    };
    let usage = match &update.usage {
        Some(u) => parse_usage(
            &serde_json::json!({ "tokens": u.tokens }),
            "mission.update.usage",
        )?,
        None => MissionTokenUsage {
            tokens: runs.iter().filter_map(|run| run.usage.map(|u| u.tokens)).sum(),
        },
    };
    let mut goal = match update.goal {
        Some(MissionGoalUpdate::Disable) => None,
        Some(MissionGoalUpdate::Set(g)) => Some(parse_goal(
            &serde_json::json!({ "status": g.status.as_str() }),
            "mission.update.goal",
        )?),
        None => current.goal,
    };
    if goal.is_some() && budget.is_none() {
        return Err(MissionError::invalid(
            "mission.update.budget is required when enabling a goal mission",
        ));
    }
    if let (Some(current_goal), Some(budget)) = (goal, budget) {
        goal = Some(if usage.tokens >= budget.tokens {
            MissionGoal { status: MissionGoalStatus::BudgetExhausted }
        } else if current_goal.status == MissionGoalStatus::BudgetExhausted {
            MissionGoal { status: MissionGoalStatus::Active }
        } else {
            current_goal
        });
    }

    // ---- status: the decision gate (SUBA-085, `store.ts:521-529` @v0.64.0) -------------------
    // Entered at v0.47.1 with `resolveDecision` (`1dec33dd`); v0.43.0 wrote
    // `update.status ?? current.status` and nothing else. An explicit `status` is always the
    // candidate. Otherwise appending a decision to an ACTIVE mission gates it as `needs_decision`,
    // and resolving the LAST open decision of a `needs_decision` mission returns it to `active`
    // (a `planned`/`waiting` mission keeps its lifecycle status either way). Then, whatever the
    // candidate, an `active`/`completed` mission that still has an open decision is held at
    // `needs_decision` — which is also why `mission.close` cannot complete a mission over an
    // unresolved decision.
    let has_open_decisions = decisions
        .iter()
        .any(|decision| decision.status == MissionDecisionStatus::Open);
    let candidate_status = match update.status {
        Some(requested) => requested,
        None if !update.add_decisions.is_empty() && current.status == MissionStatus::Active => {
            MissionStatus::NeedsDecision
        }
        None if update.resolve_decision.is_some()
            && current.status == MissionStatus::NeedsDecision
            && !has_open_decisions =>
        {
            MissionStatus::Active
        }
        None => current.status,
    };
    let status = if has_open_decisions
        && matches!(
            candidate_status,
            MissionStatus::Active | MissionStatus::Completed
        ) {
        MissionStatus::NeedsDecision
    } else {
        candidate_status
    };

    // ---- assemble ------------------------------------------------------------------------------
    // pi spreads `...current` first, so every field not named below is carried over verbatim; note
    // `usage` is written ONLY when a goal is live (`...(goal ? { goal, usage } : {})`), and
    // `delete next.goal` at `:471` is what the `None` below reproduces.
    let next = MissionRecord {
        title: match &update.title {
            None => current.title.clone(),
            Some(t) => required_string(Some(&Value::String(t.clone())), "mission.update.title")?
                .trim()
                .to_string(),
        },
        objective: match &update.objective {
            None => current.objective.clone(),
            Some(o) => {
                required_string(Some(&Value::String(o.clone())), "mission.update.objective")?
                    .trim()
                    .to_string()
            }
        },
        goal,
        budget,
        usage: if goal.is_some() { Some(usage) } else { current.usage },
        status,
        updated_at: created_at,
        runs,
        decisions,
        artifacts,
        receipts,
        summary: match &update.summary {
            None => current.summary.clone(),
            Some(s) => Some(
                required_string(Some(&Value::String(s.clone())), "mission.update.summary")?
                    .to_string(),
            ),
        },
        labels: match &update.labels {
            None => current.labels.clone(),
            Some(labels) => Some(parse_string_array(
                &Value::Array(labels.iter().cloned().map(Value::String).collect()),
                "mission.update.labels",
            )?),
        },
        acceptance: match &update.acceptance {
            None => current.acceptance.clone(),
            Some(a) => Some(a.clone()),
        },
        ..current
    };

    let updated = write_mission(location, &next)?;
    if updated.status.is_terminal() {
        prune_terminal_missions(location, effective_retention(location, retain_terminal));
    }
    Ok(updated)
}

/// pi's `{ ...runs[existingIndex]!, ...run }` (`store.ts:413`): every field the INCOMING link
/// actually carries wins; an absent optional on the incoming link leaves the existing value alone
/// (a JS spread never writes an `undefined`-valued key it does not have).
fn merge_run_link(existing: &MissionRunLink, incoming: &MissionRunLink) -> MissionRunLink {
    MissionRunLink {
        run_id: incoming.run_id.clone(),
        mode: incoming.mode,
        async_dir: incoming.async_dir.clone().or_else(|| existing.async_dir.clone()),
        child_index: incoming.child_index.or(existing.child_index),
        agent: incoming.agent.clone().or_else(|| existing.agent.clone()),
        status: incoming.status.clone().or_else(|| existing.status.clone()),
        started_at: incoming.started_at.clone().or_else(|| existing.started_at.clone()),
        completed_at: incoming.completed_at.clone().or_else(|| existing.completed_at.clone()),
        usage: incoming.usage.or(existing.usage),
    }
}

/// pi's `{ ...artifacts[existingIndex]!, ...artifact }` (`store.ts:420`).
fn merge_artifact(existing: &MissionArtifact, incoming: &MissionArtifact) -> MissionArtifact {
    MissionArtifact {
        kind: incoming.kind,
        path: incoming.path.clone(),
        description: incoming.description.clone().or_else(|| existing.description.clone()),
    }
}

/// Node's `path.resolve(p)` — absolutize against the process cwd, then normalize lexically. Used
/// ONLY for the artifact-dedup comparison (`store.ts:418`), never to rewrite a stored path.
fn resolve_for_comparison(path: &str) -> PathBuf {
    let candidate = PathBuf::from(path);
    if candidate.is_absolute() {
        normalize_lexically(&candidate)
    } else {
        normalize_lexically(&std::env::current_dir().unwrap_or_default().join(candidate))
    }
}

/// pi `listGlobalMissions` (`store.ts:477-507`): read every pointer, verify the record it points
/// at still exists and agrees about its own id, DELETE the pointer when the record is gone, and
/// mark it STALE (without deleting) when it exists but does not validate.
#[must_use]
pub fn list_global_missions(global_index_dir: &Path) -> GlobalMissionListResult {
    let Ok(dir) = std::fs::read_dir(global_index_dir) else {
        return GlobalMissionListResult::default();
    };
    let mut names: Vec<PathBuf> = dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    names.sort();
    let mut entries = Vec::new();
    let mut warnings = Vec::new();
    for file_path in names {
        let display = file_path.to_string_lossy().into_owned();
        let parsed = std::fs::read_to_string(&file_path)
            .map_err(|e| MissionError::invalid(e.to_string()))
            .and_then(|raw| parse_json(&raw))
            .and_then(|value| parse_index_entry(&value, &display));
        let entry = match parsed {
            Ok(entry) => entry,
            Err(e) => {
                warnings
                    .push(format!("Skipped corrupt global mission index entry '{display}': {e}"));
                continue;
            }
        };
        let record_path = PathBuf::from(&entry.record_path);
        let record_raw = match std::fs::read_to_string(&record_path) {
            Ok(raw) => raw,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                match std::fs::remove_file(&file_path) {
                    Ok(()) => warnings.push(format!(
                        "Removed stale global mission pointer '{display}' because '{}' no longer \
                         exists.",
                        entry.record_path
                    )),
                    Err(remove_error) => warnings.push(format!(
                        "Failed to remove stale global mission pointer '{display}': {remove_error}"
                    )),
                }
                continue;
            }
            Err(err) => {
                entries.push(GlobalMissionIndexRecord {
                    entry,
                    stale: true,
                    stale_reason: Some(err.to_string()),
                });
                continue;
            }
        };
        let verdict = parse_json(&record_raw)
            .and_then(|value| parse_mission_record(&value, &entry.record_path))
            .and_then(|record| {
                if record.id == entry.mission_id {
                    Ok(())
                } else {
                    Err(MissionError::invalid(format!(
                        "record id '{}' does not match index id '{}'",
                        record.id, entry.mission_id
                    )))
                }
            });
        match verdict {
            Ok(()) => entries.push(GlobalMissionIndexRecord {
                entry,
                stale: false,
                stale_reason: None,
            }),
            Err(e) => entries.push(GlobalMissionIndexRecord {
                entry,
                stale: true,
                stale_reason: Some(e.to_string()),
            }),
        }
    }
    entries.sort_by(|left, right| right.entry.updated_at.cmp(&left.entry.updated_at));
    GlobalMissionListResult { entries, warnings }
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
    use crate::missions::{MissionDecisionInput, MissionDecisionResolution};

    fn location(root: &Path) -> MissionStoreLocation {
        MissionStoreLocation {
            project_root: root.to_path_buf(),
            mission_dir: root.join("missions"),
            global_index_dir: root.join("index"),
            write_global_index: true,
            retain_terminal: None,
        }
    }

    fn create(loc: &MissionStoreLocation, title: &str) -> MissionRecord {
        create_mission(
            loc,
            &MissionCreateInput {
                title: title.to_string(),
                objective: format!("objective for {title}"),
                ..Default::default()
            },
            0,
            None,
        )
        .unwrap()
    }

    #[test]
    fn mission_id_pattern_rejects_traversal_and_accepts_a_uuid() {
        assert!(validate_mission_id_str(&uuid::Uuid::new_v4().hyphenated().to_string(), "id").is_ok());
        assert!(validate_mission_id_str("a.b_c-d", "id").is_ok());
        let err = validate_mission_id_str("../escape", "missionId").unwrap_err();
        assert_eq!(
            err.to_string(),
            "missionId must contain only letters, numbers, '.', '_', or '-' and cannot contain '..'"
        );
        assert!(validate_mission_id_str("a..b", "id").is_err());
        assert!(validate_mission_id_str(".leading", "id").is_err());
        assert!(validate_mission_id_str(&"a".repeat(129), "id").is_err());
        assert!(validate_mission_id_str(&"a".repeat(128), "id").is_ok());
    }

    #[test]
    fn iso_timestamp_grammar_matches_what_this_subsystem_writes() {
        assert!(is_iso8601_datetime("2026-08-11T13:34:00.000Z"));
        assert!(is_iso8601_datetime("2026-08-11"));
        assert!(is_iso8601_datetime("2026-08-11T13:34:00+02:00"));
        assert!(is_iso8601_datetime("2026-08-11T13:34"));
        assert!(!is_iso8601_datetime("Mon Jan 01 2024"));
        assert!(!is_iso8601_datetime("not a date"));
        assert!(!is_iso8601_datetime("2026-08-11T13"));
    }

    #[test]
    fn create_then_read_round_trips_and_writes_the_global_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "Ship it");
        assert_eq!(record.status, MissionStatus::Planned);
        assert_eq!(record.title, "Ship it");
        assert_eq!(record.cwd.as_deref(), Some(tmp.path().to_string_lossy().as_ref()));

        let read_back = read_mission(&loc, &record.id).unwrap();
        assert_eq!(read_back, record);

        let listed = list_global_missions(&loc.global_index_dir);
        assert_eq!(listed.entries.len(), 1);
        assert!(!listed.entries[0].stale);
        assert_eq!(listed.entries[0].entry.mission_id, record.id);
    }

    #[test]
    fn goal_mission_requires_a_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let err = create_mission(
            &loc,
            &MissionCreateInput {
                title: "g".to_string(),
                objective: "o".to_string(),
                goal: Some(true),
                ..Default::default()
            },
            0,
            None,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "mission.budget is required when mission.goal is true");
    }

    #[test]
    fn read_mission_reports_not_found_distinctly() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let err = read_mission(&loc, "missing-mission").unwrap_err();
        assert!(matches!(err, MissionError::NotFound { .. }), "{err}");
        assert!(err.to_string().starts_with("Mission 'missing-mission' was not found in "));
    }

    #[test]
    fn update_upserts_runs_on_run_id_and_child_index() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        let base = MissionRunLink {
            run_id: "run-1".to_string(),
            mode: MissionRunMode::Single,
            async_dir: None,
            child_index: None,
            agent: Some("scout".to_string()),
            status: Some("running".to_string()),
            started_at: None,
            completed_at: None,
            usage: None,
        };
        let after_first = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput { add_runs: vec![base.clone()], ..Default::default() },
            1000,
            None,
        )
        .unwrap();
        assert_eq!(after_first.runs.len(), 1);

        // Same runId + same childIndex => MERGE, not append; the agent survives the merge because
        // the incoming link omits it.
        let after_second = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    agent: None,
                    status: Some("complete".to_string()),
                    usage: Some(MissionTokenUsage { tokens: 40 }),
                    ..base.clone()
                }],
                ..Default::default()
            },
            2000,
            None,
        )
        .unwrap();
        assert_eq!(after_second.runs.len(), 1);
        assert_eq!(after_second.runs[0].status.as_deref(), Some("complete"));
        assert_eq!(after_second.runs[0].agent.as_deref(), Some("scout"));

        // A different childIndex is a different link.
        let after_third = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink { child_index: Some(1), ..base }],
                ..Default::default()
            },
            3000,
            None,
        )
        .unwrap();
        assert_eq!(after_third.runs.len(), 2);
    }

    #[test]
    fn usage_is_recomputed_from_runs_and_exhausts_the_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create_mission(
            &loc,
            &MissionCreateInput {
                title: "goal".to_string(),
                objective: "o".to_string(),
                goal: Some(true),
                budget: Some(MissionTokenBudget { tokens: 100 }),
                ..Default::default()
            },
            0,
            None,
        )
        .unwrap();
        assert_eq!(record.goal.unwrap().status, MissionGoalStatus::Active);

        let updated = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_runs: vec![MissionRunLink {
                    run_id: "r".to_string(),
                    mode: MissionRunMode::Single,
                    async_dir: None,
                    child_index: None,
                    agent: None,
                    status: None,
                    started_at: None,
                    completed_at: None,
                    usage: Some(MissionTokenUsage { tokens: 120 }),
                }],
                ..Default::default()
            },
            1000,
            None,
        )
        .unwrap();
        assert_eq!(updated.usage.unwrap().tokens, 120);
        assert_eq!(updated.goal.unwrap().status, MissionGoalStatus::BudgetExhausted);
    }

    #[test]
    fn receipt_upsert_keeps_the_original_created_at_and_rejects_a_relative_url() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        let receipt = crate::missions::MissionReceiptInput {
            kind: MissionReceiptKind::PullRequest,
            status: MissionReceiptStatus::Pending,
            title: "PR 1".to_string(),
            url: "https://example.com/pr/1".to_string(),
            description: None,
        };
        let first = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput { add_receipts: vec![receipt.clone()], ..Default::default() },
            1000,
            None,
        )
        .unwrap();
        assert_eq!(first.receipts.len(), 1);
        let created_at = first.receipts[0].created_at.clone();

        let second = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_receipts: vec![crate::missions::MissionReceiptInput {
                    status: MissionReceiptStatus::Succeeded,
                    ..receipt
                }],
                ..Default::default()
            },
            999_000,
            None,
        )
        .unwrap();
        assert_eq!(second.receipts.len(), 1);
        assert_eq!(second.receipts[0].status, MissionReceiptStatus::Succeeded);
        assert_eq!(second.receipts[0].created_at, created_at, "createdAt must be preserved");

        let err = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_receipts: vec![crate::missions::MissionReceiptInput {
                    kind: MissionReceiptKind::Ci,
                    status: MissionReceiptStatus::Pending,
                    title: "t".to_string(),
                    url: "example.com/ci".to_string(),
                    description: None,
                }],
                ..Default::default()
            },
            1000,
            None,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "mission.update.addReceipts[].url must be an absolute URL");
    }

    #[test]
    fn decisions_are_appended_as_fresh_open_decisions() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        let updated = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                add_decisions: vec![MissionDecisionInput {
                    title: "Which database?".to_string(),
                    recommendation: Some("postgres".to_string()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            1000,
            None,
        )
        .unwrap();
        assert_eq!(updated.decisions.len(), 1);
        assert_eq!(updated.decisions[0].status, MissionDecisionStatus::Open);
        assert_eq!(updated.decisions[0].recommendation.as_deref(), Some("postgres"));
        assert!(validate_mission_id_str(&updated.decisions[0].id, "id").is_ok());
    }

    fn open_decision(loc: &MissionStoreLocation, mission_id: &str, title: &str) -> MissionRecord {
        update_mission(
            loc,
            mission_id,
            &MissionUpdateInput {
                add_decisions: vec![MissionDecisionInput {
                    title: title.to_string(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            1000,
            None,
        )
        .unwrap()
    }

    fn resolve(
        loc: &MissionStoreLocation,
        mission_id: &str,
        decision_id: &str,
        text: &str,
    ) -> MissionResult<MissionRecord> {
        update_mission(
            loc,
            mission_id,
            &MissionUpdateInput {
                resolve_decision: Some(MissionDecisionResolution {
                    id: decision_id.to_string(),
                    resolution: text.to_string(),
                }),
                ..Default::default()
            },
            2000,
            None,
        )
    }

    /// SUBA-085 / pi `store.ts:497-508` @v0.64.0: `resolveDecision` flips exactly the named
    /// decision to `resolved`, stamps `resolvedAt` with this update's `createdAt`, stores the
    /// resolution TRIMMED, and leaves every other decision alone. Pre-fix
    /// `MissionUpdateInput` had no such field and `Resolved` was produced only by the parser.
    #[test]
    fn resolve_decision_marks_only_the_named_decision_resolved() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        open_decision(&loc, &record.id, "first");
        let with_two = open_decision(&loc, &record.id, "second");
        let target = with_two.decisions[1].id.clone();
        let resolved = resolve(&loc, &record.id, &target, "  go with B  ").unwrap();
        assert_eq!(resolved.decisions[0].status, MissionDecisionStatus::Open);
        assert_eq!(resolved.decisions[0].resolution, None);
        assert_eq!(
            resolved.decisions[1].status,
            MissionDecisionStatus::Resolved
        );
        assert_eq!(
            resolved.decisions[1].resolution.as_deref(),
            Some("go with B")
        );
        assert_eq!(
            resolved.decisions[1].resolved_at.as_deref(),
            Some(resolved.updated_at.as_str())
        );
        // Read back from disk: the parser's `Resolved` arm is now reached by a real write.
        let reread = read_mission(&loc, &record.id).unwrap();
        assert_eq!(reread.decisions[1].status, MissionDecisionStatus::Resolved);
    }

    /// The store's own refusals (`store.ts:498-501` @v0.64.0), verbatim: a malformed id, an id
    /// that is not on the mission, an already-resolved decision, and an empty resolution.
    #[test]
    fn resolve_decision_refuses_unknown_resolved_and_malformed_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        let with_one = open_decision(&loc, &record.id, "only");
        let target = with_one.decisions[0].id.clone();
        assert_eq!(
            resolve(&loc, &record.id, "a..b", "x")
                .unwrap_err()
                .to_string(),
            "mission.update.resolveDecision.id must contain only letters, numbers, '.', '_', or \
             '-' and cannot contain '..'"
        );
        assert_eq!(
            resolve(&loc, &record.id, "missing", "x")
                .unwrap_err()
                .to_string(),
            format!(
                "Decision 'missing' was not found in mission '{}'",
                record.id
            )
        );
        assert_eq!(
            resolve(&loc, &record.id, &target, "   ")
                .unwrap_err()
                .to_string(),
            "mission.update.resolveDecision.resolution must be a non-empty string"
        );
        // A refused update must not have been persisted.
        assert_eq!(
            read_mission(&loc, &record.id).unwrap().decisions[0].status,
            MissionDecisionStatus::Open
        );
        resolve(&loc, &record.id, &target, "done").unwrap();
        assert_eq!(
            resolve(&loc, &record.id, &target, "again")
                .unwrap_err()
                .to_string(),
            format!("Decision '{target}' is already resolved")
        );
    }

    /// The decision gate (`store.ts:521-529` @v0.64.0, entered at v0.47.1 with `1dec33dd`):
    /// appending a decision to an ACTIVE mission gates it as `needs_decision`; an `active` or
    /// `completed` status requested while a decision is still open is held at `needs_decision`;
    /// resolving the LAST open decision of a `needs_decision` mission returns it to `active`.
    /// Pre-fix the status was `update.status ?? current.status` with no gate at all.
    #[test]
    fn open_decisions_gate_the_mission_status_and_resolving_the_last_one_reopens_it() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        let active = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                status: Some(MissionStatus::Active),
                ..Default::default()
            },
            1,
            None,
        )
        .unwrap();
        assert_eq!(active.status, MissionStatus::Active);
        let gated = open_decision(&loc, &record.id, "first");
        assert_eq!(gated.status, MissionStatus::NeedsDecision);
        let gated = open_decision(&loc, &record.id, "second");
        assert_eq!(gated.status, MissionStatus::NeedsDecision);
        // An explicit `completed` (what `mission.close` sends) is held while a decision is open.
        let held = update_mission(
            &loc,
            &record.id,
            &MissionUpdateInput {
                status: Some(MissionStatus::Completed),
                ..Default::default()
            },
            2,
            None,
        )
        .unwrap();
        assert_eq!(held.status, MissionStatus::NeedsDecision);
        // Resolving one of two leaves the mission gated; resolving the last reopens it.
        let first = gated.decisions[0].id.clone();
        let second = gated.decisions[1].id.clone();
        assert_eq!(
            resolve(&loc, &record.id, &first, "a").unwrap().status,
            MissionStatus::NeedsDecision
        );
        assert_eq!(
            resolve(&loc, &record.id, &second, "b").unwrap().status,
            MissionStatus::Active
        );
    }

    /// The other half of the gate: a `planned` (or `waiting`) mission keeps its lifecycle
    /// status when a decision is added and when it is resolved — only `active` is gated, and
    /// only a `needs_decision` mission is returned to `active` by a resolution.
    #[test]
    fn a_planned_mission_keeps_its_status_across_a_decision_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "m");
        assert_eq!(record.status, MissionStatus::Planned);
        let with_one = open_decision(&loc, &record.id, "later");
        assert_eq!(with_one.status, MissionStatus::Planned);
        let resolved = resolve(&loc, &record.id, &with_one.decisions[0].id, "ok").unwrap();
        assert_eq!(resolved.status, MissionStatus::Planned);
    }

    #[test]
    fn terminal_retention_prunes_the_oldest_records_and_their_pointers() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let mut ids = Vec::new();
        for i in 0..4 {
            let record = create(&loc, &format!("m{i}"));
            let closed = update_mission(
                &loc,
                &record.id,
                &MissionUpdateInput {
                    status: Some(MissionStatus::Completed),
                    ..Default::default()
                },
                1000 + i64::from(i) * 1000,
                // Retain only 2 terminal missions.
                Some(2),
            )
            .unwrap();
            ids.push(closed.id);
        }
        let remaining = list_missions(&loc);
        assert_eq!(remaining.records.len(), 2, "{:?}", remaining.records);
        // The two newest survive.
        assert!(remaining.records.iter().any(|r| r.id == ids[3]));
        assert!(remaining.records.iter().any(|r| r.id == ids[2]));
        let pointers = list_global_missions(&loc.global_index_dir);
        assert_eq!(pointers.entries.len(), 2);
    }

    #[test]
    fn list_missions_warns_about_a_corrupt_neighbour_instead_of_failing() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let good = create(&loc, "good");
        std::fs::write(loc.mission_dir.join("broken.json"), "{ not json").unwrap();
        let listed = list_missions(&loc);
        assert_eq!(listed.records.len(), 1);
        assert_eq!(listed.records[0].id, good.id);
        assert_eq!(listed.warnings.len(), 1);
        assert!(listed.warnings[0].starts_with("Skipped corrupt mission '"), "{:?}", listed.warnings);
    }

    #[test]
    fn a_global_pointer_whose_record_vanished_is_removed_with_a_warning() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "gone");
        std::fs::remove_file(mission_record_path(&loc, &record.id).unwrap()).unwrap();
        let listed = list_global_missions(&loc.global_index_dir);
        assert!(listed.entries.is_empty());
        assert_eq!(listed.warnings.len(), 1);
        assert!(listed.warnings[0].starts_with("Removed stale global mission pointer '"));
        // The pointer file itself is gone, so a second listing is silent.
        assert!(list_global_missions(&loc.global_index_dir).warnings.is_empty());
    }

    #[test]
    fn a_global_index_config_of_false_writes_no_pointer() {
        let tmp = tempfile::tempdir().unwrap();
        let mut loc = location(tmp.path());
        loc.write_global_index = false;
        create(&loc, "m");
        assert!(list_global_missions(&loc.global_index_dir).entries.is_empty());
    }

    #[test]
    fn validate_mission_store_config_rejects_unknown_keys_and_bad_types() {
        assert!(validate_mission_store_config(None, "config.missions").unwrap().is_none());
        let ok = validate_mission_store_config(
            Some(&serde_json::json!({"enabled": false, "retainTerminal": 5})),
            "config.missions",
        )
        .unwrap()
        .unwrap();
        assert_eq!(ok.enabled, Some(false));
        assert_eq!(ok.retain_terminal, Some(5));

        let err = validate_mission_store_config(
            Some(&serde_json::json!({"nope": 1})),
            "config.missions",
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "config.missions.nope is unknown");

        let err = validate_mission_store_config(
            Some(&serde_json::json!({"enabled": "yes"})),
            "config.missions",
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "config.missions.enabled must be boolean");

        let err = validate_mission_store_config(
            Some(&serde_json::json!({"retainTerminal": 0})),
            "config.missions",
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "config.missions.retainTerminal must be a positive integer");
    }

    #[test]
    fn resolve_location_defaults_to_the_rebranded_project_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let resolved = resolve_mission_store_location(tmp.path(), None, Some(Path::new("/agent")));
        assert_eq!(resolved.mission_dir, tmp.path().join(".cyrup-subagents").join("missions"));
        assert_eq!(resolved.global_index_dir, Path::new("/agent").join("missions").join("index"));
        assert!(resolved.write_global_index);
    }

    #[test]
    fn a_configured_directory_expands_tilde_and_relative_forms() {
        let tmp = tempfile::tempdir().unwrap();
        let relative = resolve_mission_store_location(
            tmp.path(),
            Some(&MissionStoreConfig {
                directory: Some("../shared/missions".to_string()),
                ..Default::default()
            }),
            Some(Path::new("/agent")),
        );
        assert_eq!(
            relative.mission_dir,
            normalize_lexically(&tmp.path().join("..").join("shared").join("missions"))
        );
        let absolute = resolve_mission_store_location(
            tmp.path(),
            Some(&MissionStoreConfig {
                directory: Some("/var/missions/./here".to_string()),
                ..Default::default()
            }),
            Some(Path::new("/agent")),
        );
        assert_eq!(absolute.mission_dir, PathBuf::from("/var/missions/here"));
    }

    #[test]
    fn a_legacy_string_goal_supplies_the_objective_and_enables_no_goal_mode() {
        let value = serde_json::json!({
            "schemaVersion": 1,
            "id": "legacy",
            "title": "t",
            "goal": "  ship the thing  ",
            "status": "active",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
            "runs": [], "decisions": [], "artifacts": [],
        });
        let record = parse_mission_record(&value, "legacy").unwrap();
        assert_eq!(record.objective, "ship the thing");
        assert!(record.goal.is_none());
    }

    #[test]
    fn parse_mission_record_reports_the_exact_upstream_messages() {
        let base = serde_json::json!({
            "schemaVersion": 1, "id": "m", "title": "t", "objective": "o", "status": "planned",
            "createdAt": "2026-01-01T00:00:00.000Z", "updatedAt": "2026-01-01T00:00:00.000Z",
            "runs": [], "decisions": [], "artifacts": [],
        });
        let mut bad = base.clone();
        bad["schemaVersion"] = serde_json::json!(2);
        assert_eq!(
            parse_mission_record(&bad, "src").unwrap_err().to_string(),
            "src.schemaVersion must be 1"
        );

        let mut bad = base.clone();
        bad["runs"] = serde_json::json!("nope");
        assert_eq!(
            parse_mission_record(&bad, "src").unwrap_err().to_string(),
            "src.runs must be an array"
        );

        let mut bad = base.clone();
        bad["status"] = serde_json::json!("bogus");
        assert_eq!(
            parse_mission_record(&bad, "src").unwrap_err().to_string(),
            "src.status must be one of planned, active, waiting, needs_decision, completed, \
             failed, cancelled"
        );

        let mut bad = base.clone();
        bad["goal"] = serde_json::json!({"status": "active"});
        assert_eq!(
            parse_mission_record(&bad, "src").unwrap_err().to_string(),
            "src.budget is required for a goal mission"
        );

        let mut bad = base;
        bad["runs"] = serde_json::json!([{"runId": "r", "mode": "telepathy"}]);
        assert_eq!(
            parse_mission_record(&bad, "src").unwrap_err().to_string(),
            "src.runs[0].mode is invalid"
        );
    }

    #[test]
    fn a_written_record_is_owner_readable_only() {
        let tmp = tempfile::tempdir().unwrap();
        let loc = location(tmp.path());
        let record = create(&loc, "private");
        let path = mission_record_path(&loc, &record.id).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "writePrivateAtomicJson writes mode 0600");
        }
        assert!(path.exists());
    }
}
