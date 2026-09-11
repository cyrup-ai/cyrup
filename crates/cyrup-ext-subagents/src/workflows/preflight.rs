//! The workflow preflight normalizer, advisory-warning builder and formatters (SCOPE_3e
//! SUBTASK1) — pi `workflows/workflow-preflight.ts` (297 LOC), complete.
//!
//! A preflight is caller-supplied plan metadata riding on a tool call, so every constant below is
//! a DoS bound. The ones that must fire **before** the work they bound are
//! [`WORKFLOW_PREFLIGHT_MAX_LANES`] (before the lanes are normalized),
//! [`WORKFLOW_PREFLIGHT_MAX_CLAIMS`] (before the claims are normalized) and
//! [`WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH`] (per string) — upstream already orders them that way.
//! [`WORKFLOW_PREFLIGHT_MAX_DEPTH`] and [`WORKFLOW_PREFLIGHT_MAX_BYTES`] are closed-form and
//! checked last, over the **canonical normalized output** (SCOPE_3e §0.3) — there is no recursive
//! walk to thread a counter through.
//!
//! Consumes 3d's landed types ([`WorkflowKey`], [`crate::workflows::BoundedUtf16`],
//! [`WorkflowPreflightLane`], [`WorkflowPreflightMode`]) and SUBTASK0's
//! [`WorkflowPreflight`]/[`WorkflowPreflightCoverage`]; declares nothing another task owns.

use serde_json::Value;

use super::bounded::BoundedUtf16;
use super::display_text::truncate_display;
use super::key::WorkflowKey;
use super::types::{
    PreflightVersion, WorkflowPreflight, WorkflowPreflightCoverage, WorkflowPreflightLane,
    WorkflowPreflightMode, WorkflowScriptOperation, WorkflowScriptTraceEntry,
};

/// pi `WORKFLOW_PREFLIGHT_VERSION` (`workflow-preflight.ts:3`).
pub const WORKFLOW_PREFLIGHT_VERSION: u32 = 1;
/// pi `WORKFLOW_PREFLIGHT_MAX_LANES` (`:4`) — checked BEFORE the lanes are normalized.
pub const WORKFLOW_PREFLIGHT_MAX_LANES: usize = 64;
/// pi `WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH` (`:5`). UTF-16 code units, not bytes — pi compares
/// `normalized.length` (`:57`); [`normalize_display_string`] measures through
/// [`BoundedUtf16`] so the unit can never be transposed (SCOPE_3e §0.3, §A.4).
pub const WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH: usize = 256;
/// pi `WORKFLOW_PREFLIGHT_MAX_CLAIMS` (`:6`) — checked BEFORE the claims are normalized.
pub const WORKFLOW_PREFLIGHT_MAX_CLAIMS: usize = 16;
/// pi `WORKFLOW_PREFLIGHT_MAX_DEPTH` (`:7`) — evaluated as the closed-form
/// `1 + has_lanes + any_claims`, never a traversal (§0.3).
pub const WORKFLOW_PREFLIGHT_MAX_DEPTH: usize = 3;
/// pi `WORKFLOW_PREFLIGHT_MAX_BYTES` (`:8`). UTF-8 bytes of the CANONICAL normalized output, not
/// the input (`:121`) — normalization only ever shrinks, so measuring the input would reject a
/// large-but-normalizing-small payload upstream accepts.
pub const WORKFLOW_PREFLIGHT_MAX_BYTES: usize = 16 * 1024;
/// pi `WORKFLOW_PREFLIGHT_MAX_WARNINGS` (`:9`) — [`warning_limit`] keeps `MAX - 1` = 15 real
/// warnings and spends the sixteenth slot on the omission line.
pub const WORKFLOW_PREFLIGHT_MAX_WARNINGS: usize = 16;
/// pi `WORKFLOW_PREFLIGHT_PLAN_LABEL_LENGTH` (`:236`) — the eighth constant, module-private
/// upstream too.
const WORKFLOW_PREFLIGHT_PLAN_LABEL_LENGTH: usize = 96;

/// pi `PREFLIGHT_FIELDS` (`:12`).
const PREFLIGHT_FIELDS: [&str; 3] = ["version", "coverage", "lanes"];
/// pi `LANE_FIELDS` (`:13`).
const LANE_FIELDS: [&str; 6] = [
    "key",
    "mode",
    "decision",
    "claims",
    "expectedOutput",
    "independence",
];

/// The trace-row surface the preflight advisory functions read — pi's structural
/// `WorkflowTraceLike` (`workflow-preflight.ts:16-22`), as a trait so
/// [`annotate_workflow_preflight_trace`] stays generic (pi's `<T extends WorkflowTraceLike>`,
/// `:221`) with one body and no `serde_json::Value` hop.
///
/// FIVE accessors, not two — and one mutator, because the annotator writes `warning` back onto
/// the row it returns (`:228`).
pub trait WorkflowTraceLike {
    /// The operation word (`"run"`, `"host"`, …). Implementors backed by an optional field
    /// return `""` for an absent operation — never equal to `"run"`, exactly as upstream's
    /// `undefined !== "run"` comparison behaves.
    fn operation(&self) -> &str;
    /// The workflow key the operation addressed.
    fn key(&self) -> &str;
    /// The display-only phase label, when any.
    fn phase(&self) -> Option<&str>;
    /// The generated `runs.lanes` provenance key, when any.
    fn generated_lane_key(&self) -> Option<&str>;
    /// The row's existing warning, when any.
    fn warning(&self) -> Option<&str>;
    /// Attach a warning to the row (the annotator calls this only when [`Self::warning`] is
    /// `None` — `entry.warning ?? undeclaredWarning(...)`, `:228`).
    fn set_warning(&mut self, warning: String);
}

impl WorkflowTraceLike for WorkflowScriptTraceEntry {
    /// [`WorkflowScriptTraceEntry::operation`] is the [`WorkflowScriptOperation`] **enum**, so
    /// this returns the serde word (`"run"`), which is what the comparisons need.
    fn operation(&self) -> &str {
        match self.operation {
            WorkflowScriptOperation::Run => "run",
            WorkflowScriptOperation::Status => "status",
            WorkflowScriptOperation::Steer => "steer",
            WorkflowScriptOperation::Host => "host",
        }
    }

    fn key(&self) -> &str {
        &self.key
    }

    fn phase(&self) -> Option<&str> {
        self.phase.as_deref()
    }

    fn generated_lane_key(&self) -> Option<&str> {
        self.generated_lane_key.as_deref()
    }

    fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    fn set_warning(&mut self, warning: String) {
        self.warning = Some(warning);
    }
}

/// pi `WorkflowPreflightValidationResult` (`workflow-preflight.ts:24-28`) — the try/catch face's
/// return shape, preserved so the admission path's call shape survives the port.
#[derive(Clone, Debug, PartialEq)]
pub struct WorkflowPreflightValidationResult {
    /// Whether the input validated (an absent input IS ok — upstream's `input === undefined`
    /// pass).
    pub ok: bool,
    /// The canonical preflight, when one was supplied and validated.
    pub preflight: Option<WorkflowPreflight>,
    /// The rejection message, when validation failed.
    pub error: Option<String>,
}

/// pi `assertPlainObject` (`workflow-preflight.ts:36-42`). Three checks upstream; only the first
/// has a Rust analogue, and the other two are unrepresentable — stated rather than silently
/// dropped:
///
/// * `:37` prototype check ⇒ [`Value::Object`] (an array, `null` or scalar is rejected here);
/// * `:38` symbol keys — `serde_json` has no symbol key type;
/// * `:39-41` "enumerable data property" — `serde_json` has no accessor properties.
///
/// Both absent checks are JS-engine defences against a hostile object graph; a decoded
/// [`serde_json::Value`] cannot carry either construct.
fn assert_plain_object<'a>(
    value: &'a Value,
    path: &str,
) -> Result<&'a serde_json::Map<String, Value>, String> {
    value
        .as_object()
        .ok_or_else(|| format!("{path} must be a plain JSON object."))
}

/// pi `assertKnownFields` (`workflow-preflight.ts:44-48`). Key order is insertion order —
/// `Object.keys` upstream, `serde_json`'s `preserve_order` map here — so the first unknown field
/// reported matches.
fn assert_known_fields(
    value: &serde_json::Map<String, Value>,
    allowed: &[&str],
    path: &str,
) -> Result<(), String> {
    for key in value.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(format!("{path} contains unsupported field '{key}'."));
        }
    }
    Ok(())
}

/// pi `normalizeDisplayString` (`workflow-preflight.ts:50-61`) — the exact pipeline, in order:
///
/// 1. reject non-string ⇒ `{path} must be a string.` (an absent value takes the same arm —
///    upstream's `undefined` is not a string);
/// 2. replace every char in `\u{0}..=\u{1f}` and `\u{7f}` with a **space** (not delete);
/// 3. collapse every whitespace run to one space;
/// 4. trim;
/// 5. reject empty ⇒ `{path} must be a non-empty string.`;
/// 6. reject over [`WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH`] **UTF-16 code units** ⇒
///    `{path} exceeds the maximum length of 256.`
///
/// Step 2 before step 3 matters: a control char becomes a space and is then absorbed by the
/// collapse, so `"a\u{1}\u{2}b"` normalizes to `"a b"`, not `"ab"`.
///
/// Step 6 is `BoundedUtf16::<256>::parse(..).is_none()` — this function is a normalizer that
/// **calls** §A.4's one rejecting bounded-string helper as its length predicate, not a tenth copy
/// of it (SCOPE_3e §0.6, SCOPE_3 §3 gate 5). `parse` also rejects blank-after-trim, but step 5
/// has already ruled that out, so the only remaining reason it can fail is the length and the
/// message is unambiguous.
///
/// This is **not** [`super::display_text::sanitize_display_text`]: that one strips ANSI sequences
/// and is the checklist's normalizer. Preflight input arrives through a JSON tool parameter, not
/// a terminal, and upstream deliberately uses the cheaper rule here. Two normalizers, two call
/// sites, no sharing.
fn normalize_display_string(value: Option<&Value>, path: &str) -> Result<String, String> {
    let Some(raw) = value.and_then(Value::as_str) else {
        return Err(format!("{path} must be a string."));
    };
    let mut normalized = String::new();
    let mut pending_space = false;
    for c in raw.chars() {
        let code_point = c as u32;
        let c = if code_point <= 0x1f || code_point == 0x7f {
            ' '
        } else {
            c
        };
        if c.is_whitespace() {
            // The latch composes steps 3 and 4: interior runs collapse to one space, leading and
            // trailing runs never emit.
            if !normalized.is_empty() {
                pending_space = true;
            }
        } else {
            if pending_space {
                normalized.push(' ');
            }
            normalized.push(c);
            pending_space = false;
        }
    }
    if normalized.is_empty() {
        return Err(format!("{path} must be a non-empty string."));
    }
    if BoundedUtf16::<WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH>::parse(&normalized).is_none() {
        return Err(format!(
            "{path} exceeds the maximum length of {WORKFLOW_PREFLIGHT_MAX_STRING_LENGTH}."
        ));
    }
    Ok(normalized)
}

/// pi `normalizeLane` (`workflow-preflight.ts:63-93`).
///
/// The key path is normalize-then-parse (`:67-68`): [`normalize_display_string`] runs FIRST — so
/// `" lane "` becomes `"lane"` and parses, and a key carrying a control character becomes one
/// carrying a space and is rejected by the **grammar**, not by the normalizer. The 256-unit and
/// 128-character limits therefore BOTH apply, in that order, producing two different messages for
/// two different over-long keys (SCOPE_3e §0.14).
///
/// Mapping [`crate::workflows::WorkflowKeyError`] here is §A.3's sixth site, and the *only* place
/// this task mentions the grammar — the pattern itself is never re-declared.
///
/// Upstream's sparse-`claims` check (`:80-82`) is unreachable here: a JSON array has no holes, so
/// `{path}.claims must not contain sparse arrays.` has no port. (Upstream needs it because
/// `Array.prototype.map` *skips* holes rather than throwing on them.)
fn normalize_lane(value: &Value, index: usize) -> Result<WorkflowPreflightLane, String> {
    let path = format!("preflight.lanes[{index}]");
    let object = assert_plain_object(value, &path)?;
    assert_known_fields(object, &LANE_FIELDS, &path)?;

    let key = normalize_display_string(object.get("key"), &format!("{path}.key"))?;
    let key = WorkflowKey::parse(&key).map_err(|_| {
        format!(
            "{path}.key must be 1-128 characters using letters, numbers, '.', '_' or '-', \
             and start with a letter or number."
        )
    })?;

    let mode = match object.get("mode") {
        None => None,
        Some(raw) => {
            let mode = raw.as_str().and_then(|word| match word {
                "mutation" => Some(WorkflowPreflightMode::Mutation),
                "review" => Some(WorkflowPreflightMode::Review),
                "scout" => Some(WorkflowPreflightMode::Scout),
                "gate" => Some(WorkflowPreflightMode::Gate),
                _ => None,
            });
            let Some(mode) = mode else {
                return Err(format!(
                    "{path}.mode must be one of: mutation, review, scout, gate."
                ));
            };
            Some(mode)
        }
    };

    let claims = match object.get("claims") {
        None => None,
        Some(raw) => {
            let Some(items) = raw.as_array() else {
                return Err(format!("{path}.claims must be an array of strings."));
            };
            if items.len() > WORKFLOW_PREFLIGHT_MAX_CLAIMS {
                return Err(format!(
                    "{path}.claims supports at most {WORKFLOW_PREFLIGHT_MAX_CLAIMS} entries."
                ));
            }
            let mut collected = Vec::with_capacity(items.len());
            for (claim_index, claim) in items.iter().enumerate() {
                collected.push(normalize_display_string(
                    Some(claim),
                    &format!("{path}.claims[{claim_index}]"),
                )?);
            }
            Some(collected)
        }
    };

    let decision = match object.get("decision") {
        None => None,
        Some(raw) => Some(normalize_display_string(
            Some(raw),
            &format!("{path}.decision"),
        )?),
    };
    let expected_output = match object.get("expectedOutput") {
        None => None,
        Some(raw) => Some(normalize_display_string(
            Some(raw),
            &format!("{path}.expectedOutput"),
        )?),
    };
    let independence = match object.get("independence") {
        None => None,
        Some(raw) => Some(normalize_display_string(
            Some(raw),
            &format!("{path}.independence"),
        )?),
    };

    Ok(WorkflowPreflightLane {
        key,
        mode,
        decision,
        claims,
        expected_output,
        independence,
    })
}

/// pi `normalizeWorkflowPreflight` (`workflow-preflight.ts:100-124`): validate and canonicalize
/// the explicit preflight input. Intentionally independent of workflow-script parsing — callers
/// may describe dynamic fanout without making the metadata a second execution graph.
///
/// Order is fixed: plain-object ▸ known fields ▸ `version` ▸ `coverage` (defaulting to
/// [`WorkflowPreflightCoverage::Partial`]) ▸ `lanes` is an array ▸ `MAX_LANES` ▸ per-lane
/// normalization ▸ **duplicate keys** (`:116`, §0.14) ▸ `MAX_DEPTH` ▸ `MAX_BYTES`.
///
/// `depth` is `1 + (has lanes) + (any lane has claims)` — a closed-form expression over two
/// booleans (§0.3), so it can never exceed 3 and the check is a documented tautology upstream
/// keeps for shape-drift safety; it is ported as-is. `bytes` is measured over the **normalized**
/// struct's canonical JSON, not the input (see [`WORKFLOW_PREFLIGHT_MAX_BYTES`]).
///
/// Upstream's sparse-`lanes` check (`:110-112`) is unreachable — a JSON array has no holes — so
/// `preflight.lanes must not contain sparse arrays.` has no port (same reasoning as
/// [`normalize_lane`]'s claims).
///
/// # Errors
///
/// Every rejection is upstream's message, verbatim.
pub fn normalize_workflow_preflight(
    input: Option<&Value>,
) -> Result<Option<WorkflowPreflight>, String> {
    let Some(input) = input else {
        return Ok(None);
    };
    let object = assert_plain_object(input, "preflight")?;
    assert_known_fields(object, &PREFLIGHT_FIELDS, "preflight")?;
    if object.get("version").and_then(Value::as_f64) != Some(f64::from(WORKFLOW_PREFLIGHT_VERSION))
    {
        return Err(format!(
            "preflight.version must be {WORKFLOW_PREFLIGHT_VERSION}."
        ));
    }
    let coverage = match object.get("coverage") {
        None => WorkflowPreflightCoverage::Partial,
        Some(raw) => match raw.as_str() {
            Some("complete") => WorkflowPreflightCoverage::Complete,
            Some("partial") => WorkflowPreflightCoverage::Partial,
            _ => return Err("preflight.coverage must be 'complete' or 'partial'.".to_string()),
        },
    };
    let Some(raw_lanes) = object.get("lanes").and_then(Value::as_array) else {
        return Err("preflight.lanes must be an array.".to_string());
    };
    if raw_lanes.len() > WORKFLOW_PREFLIGHT_MAX_LANES {
        return Err(format!(
            "preflight.lanes supports at most {WORKFLOW_PREFLIGHT_MAX_LANES} lanes."
        ));
    }
    let mut lanes = Vec::with_capacity(raw_lanes.len());
    for (index, lane) in raw_lanes.iter().enumerate() {
        lanes.push(normalize_lane(lane, index)?);
    }
    let mut seen_keys = std::collections::HashSet::new();
    for lane in &lanes {
        if !seen_keys.insert(lane.key.as_str()) {
            return Err(format!(
                "preflight.lanes contains duplicate key '{}'.",
                lane.key
            ));
        }
    }
    let normalized = WorkflowPreflight {
        version: PreflightVersion,
        coverage,
        lanes,
    };
    let depth = 1
        + usize::from(!normalized.lanes.is_empty())
        + usize::from(normalized.lanes.iter().any(|lane| lane.claims.is_some()));
    if depth > WORKFLOW_PREFLIGHT_MAX_DEPTH {
        return Err(format!(
            "preflight exceeds the maximum nesting depth of {WORKFLOW_PREFLIGHT_MAX_DEPTH}."
        ));
    }
    let bytes = serde_json::to_string(&normalized)
        .map_err(|error| error.to_string())?
        .len();
    if bytes > WORKFLOW_PREFLIGHT_MAX_BYTES {
        return Err(format!(
            "preflight canonical JSON is {bytes} bytes; maximum is {WORKFLOW_PREFLIGHT_MAX_BYTES}."
        ));
    }
    Ok(Some(normalized))
}

/// pi `validateWorkflowPreflight` (`workflow-preflight.ts:126-133`) — the try/catch face of
/// [`normalize_workflow_preflight`]. Upstream needs a separate function because `normalize`
/// throws; cyrup's already returns a `Result`, so this exists purely to preserve the CALL SHAPE
/// the admission path uses. `ok: true` with no preflight is upstream's `input === undefined`
/// pass. There is deliberately no `strict: bool` variant — one implementation, one thin wrapper
/// (§A.1).
#[must_use]
pub fn validate_workflow_preflight(input: Option<&Value>) -> WorkflowPreflightValidationResult {
    match normalize_workflow_preflight(input) {
        Ok(preflight) => WorkflowPreflightValidationResult {
            ok: true,
            preflight,
            error: None,
        },
        Err(error) => WorkflowPreflightValidationResult {
            ok: false,
            preflight: None,
            error: Some(error),
        },
    }
}

/// pi `warningLimit` (`workflow-preflight.ts:135-139`): at most
/// [`WORKFLOW_PREFLIGHT_MAX_WARNINGS`] lines — when over, the first 15 real warnings plus the
/// omission line, whose count INCLUDES the warning the omission line displaced (`+ 1`).
fn warning_limit(messages: Vec<String>) -> Vec<String> {
    if messages.len() <= WORKFLOW_PREFLIGHT_MAX_WARNINGS {
        return messages;
    }
    let omitted = messages.len() - WORKFLOW_PREFLIGHT_MAX_WARNINGS + 1;
    let mut limited: Vec<String> = messages
        .into_iter()
        .take(WORKFLOW_PREFLIGHT_MAX_WARNINGS - 1)
        .collect();
    limited.push(format!(
        "Preflight advisory: {omitted} additional mismatch warning(s) omitted."
    ));
    limited
}

/// pi `workflowKeyMatchesPreflightLane` (`workflow-preflight.ts:149-151`) — a **three-way**
/// disjunction (§0.14). Treat declared lane keys as plan roots: `lane` is the lane itself and
/// `lane.stage` is a stage by convention, even without generated provenance. The FIRST arm is why
/// an auto-generated `runs.lanes` child counts as covered by the lane that generated it even
/// though its runtime key is unrelated.
///
/// `key` is a raw `&str` (trace keys and phase labels reach this comparison); `lane_key` is a
/// declared lane's parsed [`WorkflowKey`]. The descendant arm goes through
/// [`WorkflowKey::is_descendant_of`] — the grammar's own relation, never a re-derived dotted
/// prefix (§0.7). [CYRUP-DELTA, unrepresentable] a key that fails the grammar cannot be a
/// descendant here, where upstream's raw `startsWith` would still match; such a key is
/// unreachable from real traces (the workflow executor validates every key at each `runs.*`
/// boundary, the same validation that makes [`crate::workflows::build_workflow_chat_progress_rows`]
/// skip non-grammatical rows).
#[must_use]
pub fn workflow_key_matches_preflight_lane(
    key: &str,
    lane_key: &WorkflowKey,
    generated_lane_key: Option<&str>,
) -> bool {
    generated_lane_key == Some(lane_key.as_str())
        || key == lane_key.as_str()
        || WorkflowKey::parse(key).is_ok_and(|parsed| parsed.is_descendant_of(lane_key))
}

/// pi `workflowPreflightLaneForRuntimeKey` (`workflow-preflight.ts:154-174`): "Select the most
/// specific advisory lane without allowing declaration order to override an exact runtime key."
///
/// Three tiers, each a labelled block with an early return — the precedence is the point, so it
/// is never buried in statement order (§A.1):
///
/// 1. **exact** — `lane.key == key` wins outright;
/// 2. **longest matching root** — among lanes the key descends from
///    ([`WorkflowKey::is_descendant_of`], §0.7 — the selection is here, the predicate is the
///    grammar's), the one with the LONGEST key. Longest, not first, so declaration order cannot
///    beat specificity;
/// 3. **preferred keys** in caller order, skipping `None`, first match wins.
///
/// `key` and the preferred keys are `&str`, **not** `&WorkflowKey` (§0.5): the checklist passes
/// phase labels, which have no grammar. A non-grammatical value matches nothing — exactly as
/// upstream's `lanes.find(lane => lane.key === preferredKey)` behaves. (Upstream also skips a
/// falsy — empty — preferred key; an empty `Some("")` here matches no lane for the same reason a
/// phase label doesn't, so only `None` needs an explicit skip.)
///
/// Adapts to 3d's [`crate::workflows::WorkflowPreflightLaneLookup`] seam without changing that
/// type — see this module's tests for the exact closure.
#[must_use]
pub fn workflow_preflight_lane_for_runtime_key<'a>(
    preflight: Option<&'a WorkflowPreflight>,
    key: &str,
    preferred_keys: &[Option<&str>],
) -> Option<&'a WorkflowPreflightLane> {
    let lanes = &preflight?.lanes;

    // Tier 1 — an exact runtime key wins outright (`:161`).
    if let Some(exact) = lanes.iter().find(|lane| lane.key.as_str() == key) {
        return Some(exact);
    }

    // Tier 2 — the LONGEST declared root the key descends from (`:163-167`). The strictly-greater
    // comparison keeps the earlier lane on a tie, as upstream's `>` does (two distinct roots of
    // one key are always nested, hence different lengths, so a tie cannot actually occur).
    let parsed = WorkflowKey::parse(key).ok();
    let mut closest_root: Option<&WorkflowPreflightLane> = None;
    if let Some(parsed) = &parsed {
        for lane in lanes.iter() {
            if parsed.is_descendant_of(&lane.key)
                && closest_root
                    .is_none_or(|closest| lane.key.as_str().len() > closest.key.as_str().len())
            {
                closest_root = Some(lane);
            }
        }
    }
    if let Some(root) = closest_root {
        return Some(root);
    }

    // Tier 3 — the caller's preferred keys, in caller order (`:168-172`).
    for preferred in preferred_keys {
        let Some(preferred) = preferred else {
            continue;
        };
        if let Some(lane) = lanes.iter().find(|lane| lane.key.as_str() == *preferred) {
            return Some(lane);
        }
    }
    None
}

/// pi `declaredLaneCoversWorkflowKey` (`workflow-preflight.ts:176-178`).
fn declared_lane_covers_workflow_key<T: WorkflowTraceLike + ?Sized>(
    entry: &T,
    lane_key: &WorkflowKey,
) -> bool {
    workflow_key_matches_preflight_lane(entry.key(), lane_key, entry.generated_lane_key())
}

/// pi `keyCoveredByDeclaredLane` (`workflow-preflight.ts:180-183`). Upstream iterates a `Set` of
/// declared keys; iterating the lanes directly is the same disjunction (duplicates cannot exist —
/// [`normalize_workflow_preflight`] rejects them — and `any` is order-insensitive).
fn key_covered_by_declared_lane<T: WorkflowTraceLike + ?Sized>(
    entry: &T,
    preflight: &WorkflowPreflight,
) -> bool {
    preflight
        .lanes
        .iter()
        .any(|lane| declared_lane_covers_workflow_key(entry, &lane.key))
}

/// pi `launchedEntries` (`workflow-preflight.ts:185-194`): `operation == "run"` only, **excluding
/// `phase == "auto-resume"`**, deduped by key keeping the FIRST. The auto-resume exclusion is
/// load-bearing: an auto-resumed child is not a fresh launch and must not be warned about twice.
fn launched_entries<T: WorkflowTraceLike>(trace: &[T]) -> Vec<&T> {
    let mut entries: Vec<&T> = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for entry in trace {
        if entry.operation() != "run"
            || entry.phase() == Some("auto-resume")
            || seen.contains(entry.key())
        {
            continue;
        }
        seen.insert(entry.key());
        entries.push(entry);
    }
    entries
}

/// pi `undeclaredWarning` (`workflow-preflight.ts:196-198`).
fn undeclared_warning(key: &str) -> String {
    format!("Preflight advisory: workflow key '{key}' launched without a declared lane.")
}

/// pi `unlaunchedWarning` (`workflow-preflight.ts:200-202`).
fn unlaunched_warning(key: &str) -> String {
    format!("Preflight advisory: declared lane '{key}' was not launched.")
}

/// pi `workflowPreflightWarnings` (`workflow-preflight.ts:205-218`): bounded advisory mismatch
/// warnings — these never reject a child launch.
///
/// Every launched entry not covered by a declared lane warns; declared lanes with no launched
/// entry warn **only when `settled`** (`:214`) — an unsettled run may simply not have reached the
/// lane yet. The whole list goes through [`warning_limit`].
#[must_use]
pub fn workflow_preflight_warnings<T: WorkflowTraceLike>(
    preflight: Option<&WorkflowPreflight>,
    trace: &[T],
    settled: bool,
) -> Vec<String> {
    let Some(preflight) = preflight else {
        return Vec::new();
    };
    let launched = launched_entries(trace);
    let mut messages: Vec<String> = launched
        .iter()
        .filter(|entry| !key_covered_by_declared_lane(**entry, preflight))
        .map(|entry| undeclared_warning(entry.key()))
        .collect();
    if settled {
        for lane in &preflight.lanes {
            if !launched
                .iter()
                .any(|entry| declared_lane_covers_workflow_key(*entry, &lane.key))
            {
                messages.push(unlaunched_warning(lane.key.as_str()));
            }
        }
    }
    warning_limit(messages)
}

/// pi `annotateWorkflowPreflightTrace` (`workflow-preflight.ts:221-230`): attach the first
/// undeclared-key warning to its first trace row for status/debug views.
///
/// Returns a copy of the whole trace even with no preflight (`:222`), warns once per key (the
/// `warned` set), and **preserves an existing warning** (`entry.warning ?? undeclaredWarning`,
/// `:228`). Generic over [`WorkflowTraceLike`] — one body for
/// [`WorkflowScriptTraceEntry`] and the checklist's own trace rows.
#[must_use]
pub fn annotate_workflow_preflight_trace<T: WorkflowTraceLike + Clone>(
    trace: &[T],
    preflight: Option<&WorkflowPreflight>,
) -> Vec<T> {
    let Some(preflight) = preflight else {
        return trace.to_vec();
    };
    let mut warned: std::collections::HashSet<String> = std::collections::HashSet::new();
    trace
        .iter()
        .map(|entry| {
            if entry.operation() != "run"
                || entry.phase() == Some("auto-resume")
                || key_covered_by_declared_lane(entry, preflight)
                || warned.contains(entry.key())
            {
                return entry.clone();
            }
            warned.insert(entry.key().to_string());
            let mut annotated = entry.clone();
            if annotated.warning().is_none() {
                let warning = undeclared_warning(entry.key());
                annotated.set_warning(warning);
            }
            annotated
        })
        .collect()
}

/// pi `laneCell` (`workflow-preflight.ts:232-234`): an absent cell renders as `—` (U+2014).
fn lane_cell(value: Option<&str>) -> &str {
    value.unwrap_or("—")
}

/// The [`WorkflowPreflightMode`] display word — the serde vocabulary, matched exhaustively.
fn mode_word(mode: WorkflowPreflightMode) -> &'static str {
    match mode {
        WorkflowPreflightMode::Mutation => "mutation",
        WorkflowPreflightMode::Review => "review",
        WorkflowPreflightMode::Scout => "scout",
        WorkflowPreflightMode::Gate => "gate",
    }
}

/// `lane`/`lanes` — the plural pi's formatters interpolate.
fn lane_plural(count: usize) -> &'static str {
    if count == 1 { "lane" } else { "lanes" }
}

/// pi `planLaneLabel` (`workflow-preflight.ts:238-242`): the lane's trimmed `decision`, falling
/// back to the key when that trim is **empty** (`||`, not `??` — `:239`), truncated at
/// [`WORKFLOW_PREFLIGHT_PLAN_LABEL_LENGTH`] − 1 = 95 UTF-16 units, `trim_end`ed, then `…`
/// (U+2026).
fn plan_lane_label(lane: &WorkflowPreflightLane) -> String {
    let decision = lane
        .decision
        .as_deref()
        .map(str::trim)
        .filter(|decision| !decision.is_empty());
    let value = decision.unwrap_or_else(|| lane.key.as_str());
    if value.encode_utf16().count() <= WORKFLOW_PREFLIGHT_PLAN_LABEL_LENGTH {
        return value.to_string();
    }
    format!(
        "{}…",
        truncate_display(value, WORKFLOW_PREFLIGHT_PLAN_LABEL_LENGTH - 1).trim_end()
    )
}

/// pi `formatWorkflowPreflight` (`workflow-preflight.ts:245-264`): the detailed bounded table
/// reserved for tool output and expanded/debug views. Returns the lines `join("\n")`ed — a single
/// `String`, unlike the checklist text's `Vec<String>` (§0.13); two different shapes upstream,
/// not unified here. `""` renders nothing — callers concatenate, so an empty string is the
/// "render nothing" signal, never an `Option`.
#[must_use]
pub fn format_workflow_preflight(preflight: Option<&WorkflowPreflight>, indent: &str) -> String {
    let Some(preflight) = preflight else {
        return String::new();
    };
    let count = preflight.lanes.len();
    let mut lines = vec![
        format!(
            "{indent}Preflight: v{} · {} · {count} {}",
            PreflightVersion::VALUE,
            preflight.coverage.as_str(),
            lane_plural(count)
        ),
        format!("{indent}  key | mode | decision | claims | expected output | independence"),
    ];
    for lane in &preflight.lanes {
        // `lane.claims?.join(", ") ?? "—"` — an EMPTY claims list joins to `""`, only an absent
        // one renders the em dash.
        let claims_cell = lane
            .claims
            .as_ref()
            .map_or_else(|| "—".to_string(), |claims| claims.join(", "));
        lines.push(format!(
            "{indent}  {} | {} | {} | {} | {} | {}",
            lane.key,
            lane_cell(lane.mode.map(mode_word)),
            lane_cell(lane.decision.as_deref()),
            claims_cell,
            lane_cell(lane.expected_output.as_deref()),
            lane_cell(lane.independence.as_deref()),
        ));
    }
    lines.join("\n")
}

/// pi `formatWorkflowPreflightSummary` (`workflow-preflight.ts:266-271`) — the only formatter of
/// the five **without** an indent option (`:266`). The `: {keys}` suffix is omitted entirely when
/// there are no lanes.
#[must_use]
pub fn format_workflow_preflight_summary(preflight: Option<&WorkflowPreflight>) -> String {
    let Some(preflight) = preflight else {
        return String::new();
    };
    let count = preflight.lanes.len();
    let keys = preflight
        .lanes
        .iter()
        .take(4)
        .map(|lane| lane.key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let remainder = if count > 4 {
        format!(", +{}", count - 4)
    } else {
        String::new()
    };
    let suffix = if keys.is_empty() {
        String::new()
    } else {
        format!(": {keys}{remainder}")
    };
    format!(
        "preflight · {} · {count} {}{suffix}",
        preflight.coverage.as_str(),
        lane_plural(count)
    )
}

/// pi `formatWorkflowPreflightPlanSummary` (`workflow-preflight.ts:274-282`): the operator-facing
/// one-line plan shown in routine status views. With **exactly one** lane the label is
/// [`plan_lane_label`]; otherwise the first-four-keys form (`:278-280`). With zero lanes `labels`
/// is empty and the ` · ` separator disappears too.
#[must_use]
pub fn format_workflow_preflight_plan_summary(
    preflight: Option<&WorkflowPreflight>,
    indent: &str,
) -> String {
    let Some(preflight) = preflight else {
        return String::new();
    };
    let count = preflight.lanes.len();
    let labels = if count == 1 {
        preflight
            .lanes
            .first()
            .map(plan_lane_label)
            .unwrap_or_default()
    } else {
        let keys = preflight
            .lanes
            .iter()
            .take(4)
            .map(|lane| lane.key.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        let remainder = if count > 4 {
            format!(", +{}", count - 4)
        } else {
            String::new()
        };
        format!("{keys}{remainder}")
    };
    let suffix = if labels.is_empty() {
        String::new()
    } else {
        format!(" · {labels}")
    };
    format!("{indent}Plan: {count} {}{suffix}", lane_plural(count))
}

/// pi `formatWorkflowPreflightWarningSummary` (`workflow-preflight.ts:285-291`): a bounded,
/// non-alarming warning line for routine status views. The default `hint` is
/// `"details available for debug"` (`:288`).
#[must_use]
pub fn format_workflow_preflight_warning_summary(
    warnings: &[String],
    indent: &str,
    hint: Option<&str>,
) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let hint = hint.unwrap_or("details available for debug");
    let count = warnings.len();
    let plural = if count == 1 { "mismatch" } else { "mismatches" };
    format!("{indent}Plan note: {count} preflight {plural} · {hint}.")
}

/// pi `formatWorkflowPreflightWarnings` (`workflow-preflight.ts:293-297`): the full warning list,
/// each line whitespace-collapsed and trimmed (`warning.replace(/\s+/g, " ").trim()`).
#[must_use]
pub fn format_workflow_preflight_warnings(warnings: &[String], indent: &str) -> String {
    if warnings.is_empty() {
        return String::new();
    }
    let mut lines = vec![format!("{indent}Preflight warnings:")];
    for warning in warnings {
        let collapsed = warning.split_whitespace().collect::<Vec<_>>().join(" ");
        lines.push(format!("{indent}  - {collapsed}"));
    }
    lines.join("\n")
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
    use serde_json::json;

    fn preflight_of(lanes: serde_json::Value) -> WorkflowPreflight {
        normalize_workflow_preflight(Some(&json!({ "version": 1, "lanes": lanes })))
            .expect("valid preflight")
            .expect("present")
    }

    fn run_entry(key: &str) -> WorkflowScriptTraceEntry {
        WorkflowScriptTraceEntry {
            operation: WorkflowScriptOperation::Run,
            key: key.to_string(),
            state: crate::workflows::WorkflowScriptTraceState::Started,
            agent: None,
            run_id: None,
            duration_ms: None,
            phase: None,
            label: None,
            error: None,
            generated_lane_key: None,
            lane: None,
            warning: None,
        }
    }

    /// The whole normalize pipeline: `" lane "` normalizes then parses; a control character
    /// becomes a space and fails the GRAMMAR; coverage defaults to `partial`.
    #[test]
    fn normalize_accepts_and_canonicalizes() {
        let preflight = normalize_workflow_preflight(Some(&json!({
            "version": 1,
            "lanes": [{ "key": " lane ", "decision": "a\u{1}\u{2}b", "claims": ["x"] }],
        })))
        .expect("valid")
        .expect("present");
        assert_eq!(preflight.coverage, WorkflowPreflightCoverage::Partial);
        let lane = preflight.lanes.first().expect("one lane");
        assert_eq!(lane.key.as_str(), "lane");
        assert_eq!(
            lane.decision.as_deref(),
            Some("a b"),
            "controls become spaces BEFORE the collapse (step 2 before step 3)"
        );
        assert!(
            normalize_workflow_preflight(None)
                .expect("absent is ok")
                .is_none()
        );
    }

    /// §0.14: the 256-unit and 128-char limits BOTH apply, in normalize-then-parse order, with
    /// two different messages.
    #[test]
    fn overlong_keys_get_two_different_messages() {
        let grammar_message = normalize_workflow_preflight(Some(&json!({
            "version": 1, "lanes": [{ "key": "a".repeat(129) }],
        })))
        .expect_err("129 chars fails the grammar");
        assert!(
            grammar_message.contains("must be 1-128 characters"),
            "{grammar_message}"
        );

        let length_message = normalize_workflow_preflight(Some(&json!({
            "version": 1, "lanes": [{ "key": "a".repeat(257) }],
        })))
        .expect_err("257 chars fails the length rule first");
        assert_eq!(
            length_message,
            "preflight.lanes[0].key exceeds the maximum length of 256."
        );

        let control_message = normalize_workflow_preflight(Some(&json!({
            "version": 1, "lanes": [{ "key": "a\u{1}b" }],
        })))
        .expect_err("a control char becomes a space and fails the grammar");
        assert!(
            control_message.contains("must be 1-128 characters"),
            "{control_message}"
        );
    }

    /// The length rule counts UTF-16 code units via `BoundedUtf16<256>`: 200 astral characters
    /// (400 units) are rejected, 200 ASCII ones are not.
    #[test]
    fn string_length_counts_utf16_units() {
        let astral = normalize_workflow_preflight(Some(&json!({
            "version": 1, "lanes": [{ "key": "k", "decision": "𝄞".repeat(200) }],
        })))
        .expect_err("400 units rejects");
        assert_eq!(
            astral,
            "preflight.lanes[0].decision exceeds the maximum length of 256."
        );
        assert!(
            normalize_workflow_preflight(Some(&json!({
                "version": 1, "lanes": [{ "key": "k", "decision": "a".repeat(200) }],
            })))
            .is_ok(),
            "200 ASCII units fit"
        );
    }

    /// Upstream's rejection messages, verbatim, in upstream's order.
    #[test]
    fn rejections_are_verbatim() {
        let cases: [(serde_json::Value, &str); 8] = [
            (json!([1]), "preflight must be a plain JSON object."),
            (
                json!({ "version": 1, "lanes": [], "extra": 1 }),
                "preflight contains unsupported field 'extra'.",
            ),
            (
                json!({ "version": 2, "lanes": [] }),
                "preflight.version must be 1.",
            ),
            (
                json!({ "version": 1, "coverage": "half", "lanes": [] }),
                "preflight.coverage must be 'complete' or 'partial'.",
            ),
            (json!({ "version": 1 }), "preflight.lanes must be an array."),
            (
                json!({ "version": 1, "lanes": [{ "key": "k", "mode": "audit" }] }),
                "preflight.lanes[0].mode must be one of: mutation, review, scout, gate.",
            ),
            (
                json!({ "version": 1, "lanes": [{ "key": "k", "claims": "x" }] }),
                "preflight.lanes[0].claims must be an array of strings.",
            ),
            (
                json!({ "version": 1, "lanes": [{ "key": "a" }, { "key": "a" }] }),
                "preflight.lanes contains duplicate key 'a'.",
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_workflow_preflight(Some(&input)).expect_err("rejects"),
                expected
            );
        }
        let too_many_lanes: Vec<serde_json::Value> =
            (0..65).map(|i| json!({ "key": format!("k{i}") })).collect();
        assert_eq!(
            normalize_workflow_preflight(Some(&json!({ "version": 1, "lanes": too_many_lanes })))
                .expect_err("rejects"),
            "preflight.lanes supports at most 64 lanes."
        );
        let too_many_claims: Vec<serde_json::Value> =
            (0..17).map(|i| json!(format!("c{i}"))).collect();
        assert_eq!(
            normalize_workflow_preflight(Some(&json!({
                "version": 1, "lanes": [{ "key": "k", "claims": too_many_claims }],
            })))
            .expect_err("rejects"),
            "preflight.lanes[0].claims supports at most 16 entries."
        );
        assert_eq!(
            normalize_workflow_preflight(Some(&json!({
                "version": 1, "lanes": [{ "key": "k", "decision": 5 }],
            })))
            .expect_err("rejects"),
            "preflight.lanes[0].decision must be a string."
        );
        assert_eq!(
            normalize_workflow_preflight(Some(&json!({
                "version": 1, "lanes": [{ "key": "k", "decision": "  " }],
            })))
            .expect_err("rejects"),
            "preflight.lanes[0].decision must be a non-empty string."
        );
    }

    /// The canonical-bytes bound measures the NORMALIZED struct's JSON.
    #[test]
    fn canonical_bytes_bound_fires_over_the_normalized_output() {
        let lanes: Vec<serde_json::Value> = (0..64)
            .map(|i| json!({ "key": format!("lane{i}"), "decision": "d".repeat(256) }))
            .collect();
        let message = normalize_workflow_preflight(Some(&json!({ "version": 1, "lanes": lanes })))
            .expect_err("over 16 KiB of canonical JSON");
        assert!(
            message.starts_with("preflight canonical JSON is ")
                && message.ends_with(" bytes; maximum is 16384."),
            "{message}"
        );
    }

    /// `validate` is the try/catch face of `normalize` — same implementation, shaped result.
    #[test]
    fn validate_preserves_the_call_shape() {
        let ok = validate_workflow_preflight(None);
        assert!(ok.ok && ok.preflight.is_none() && ok.error.is_none());
        let err = validate_workflow_preflight(Some(&json!({ "version": 3, "lanes": [] })));
        assert!(!err.ok && err.preflight.is_none());
        assert_eq!(err.error.as_deref(), Some("preflight.version must be 1."));
    }

    /// The three tiers: exact ▸ LONGEST root ▸ preferred, in that order; a phase label (no
    /// grammar) can still match tier 3 by equality.
    #[test]
    fn lane_lookup_resolves_exact_then_longest_root_then_preferred() {
        let preflight = preflight_of(json!([
            { "key": "lane" },
            { "key": "lane.stage" },
            { "key": "other" },
        ]));
        let exact = workflow_preflight_lane_for_runtime_key(Some(&preflight), "lane.stage", &[])
            .expect("exact");
        assert_eq!(exact.key.as_str(), "lane.stage");

        let root =
            workflow_preflight_lane_for_runtime_key(Some(&preflight), "lane.stage.child", &[])
                .expect("root");
        assert_eq!(
            root.key.as_str(),
            "lane.stage",
            "LONGEST root, not the first declared"
        );

        let preferred = workflow_preflight_lane_for_runtime_key(
            Some(&preflight),
            "unrelated",
            &[None, Some("missing"), Some("other")],
        )
        .expect("preferred");
        assert_eq!(
            preferred.key.as_str(),
            "other",
            "first matching preferred key wins"
        );

        assert!(
            workflow_preflight_lane_for_runtime_key(Some(&preflight), "unrelated", &[]).is_none()
        );
        assert!(workflow_preflight_lane_for_runtime_key(None, "lane", &[]).is_none());
    }

    /// §0.14's three-way disjunction, including the `generatedLaneKey == laneKey` arm.
    #[test]
    fn key_matching_is_a_three_way_disjunction() {
        let lane = WorkflowKey::parse("lane").expect("valid");
        assert!(workflow_key_matches_preflight_lane("lane", &lane, None));
        assert!(workflow_key_matches_preflight_lane(
            "lane.stage",
            &lane,
            None
        ));
        assert!(
            workflow_key_matches_preflight_lane("generated.42", &lane, Some("lane")),
            "an auto-generated child is covered by the lane that generated it"
        );
        assert!(!workflow_key_matches_preflight_lane("lane2", &lane, None));
    }

    /// Warnings: undeclared launches always, unlaunched lanes only when settled, auto-resume
    /// excluded, deduped keeping the first.
    #[test]
    fn warnings_cover_both_directions() {
        let preflight = preflight_of(json!([{ "key": "lane" }]));
        let mut resumed = run_entry("stray");
        resumed.phase = Some("auto-resume".to_string());
        let trace = [
            run_entry("lane.stage"),
            run_entry("stray"),
            run_entry("stray"),
            resumed,
        ];
        let unsettled = workflow_preflight_warnings(Some(&preflight), &trace, false);
        assert_eq!(
            unsettled,
            vec![
                "Preflight advisory: workflow key 'stray' launched without a declared lane."
                    .to_string()
            ]
        );

        let settled =
            workflow_preflight_warnings(Some(&preflight), &[run_entry("unrelated")], true);
        assert_eq!(
            settled,
            vec![
                "Preflight advisory: workflow key 'unrelated' launched without a declared lane."
                    .to_string(),
                "Preflight advisory: declared lane 'lane' was not launched.".to_string(),
            ]
        );
        assert!(
            workflow_preflight_warnings::<WorkflowScriptTraceEntry>(None, &[], true).is_empty()
        );
    }

    /// `warning_limit` keeps 15 and the omission count includes the displaced sixteenth.
    #[test]
    fn warning_limit_keeps_fifteen_plus_the_omission_line() {
        let preflight = preflight_of(json!([]));
        let trace: Vec<WorkflowScriptTraceEntry> =
            (0..20).map(|i| run_entry(&format!("k{i}"))).collect();
        let warnings = workflow_preflight_warnings(Some(&preflight), &trace, false);
        assert_eq!(warnings.len(), 16);
        assert_eq!(
            warnings.last().map(String::as_str),
            Some("Preflight advisory: 5 additional mismatch warning(s) omitted.")
        );
        assert!(
            warnings.first().is_some_and(|w| w.contains("'k0'")),
            "the first 15 real warnings survive"
        );
    }

    /// The annotator: copy without a preflight, warn once per key, never overwrite an existing
    /// warning.
    #[test]
    fn annotate_warns_once_and_preserves_existing_warnings() {
        let preflight = preflight_of(json!([{ "key": "lane" }]));
        let mut pre_warned = run_entry("stray");
        pre_warned.warning = Some("existing".to_string());
        let trace = [pre_warned, run_entry("stray"), run_entry("lane.child")];

        let annotated = annotate_workflow_preflight_trace(&trace, Some(&preflight));
        assert_eq!(
            annotated.first().and_then(|e| e.warning.as_deref()),
            Some("existing"),
            "an existing warning is preserved"
        );
        assert_eq!(
            annotated.get(1).and_then(|e| e.warning.as_deref()),
            None,
            "only the FIRST row per key is annotated"
        );
        assert_eq!(annotated.get(2).and_then(|e| e.warning.as_deref()), None);

        let copied = annotate_workflow_preflight_trace(&trace, None);
        assert_eq!(copied.len(), 3, "a copy comes back even with no preflight");
    }

    /// The five formatters, character-exact: `·`, `—`, `…`, plurals, and the empty-input `""`.
    #[test]
    fn formatters_are_character_exact() {
        let preflight = preflight_of(json!([
            { "key": "lane", "mode": "review", "claims": ["a", "b"] },
        ]));
        assert_eq!(
            format_workflow_preflight(Some(&preflight), "> "),
            "> Preflight: v1 · partial · 1 lane\n\
             >   key | mode | decision | claims | expected output | independence\n\
             >   lane | review | — | a, b | — | —"
        );
        assert_eq!(format_workflow_preflight(None, "> "), "");
        assert_eq!(
            format_workflow_preflight_summary(Some(&preflight)),
            "preflight · partial · 1 lane: lane"
        );
        let empty = preflight_of(json!([]));
        assert_eq!(
            format_workflow_preflight_summary(Some(&empty)),
            "preflight · partial · 0 lanes",
            "no lanes ⇒ no `: keys` suffix"
        );
        assert_eq!(
            format_workflow_preflight_plan_summary(Some(&empty), "  "),
            "  Plan: 0 lanes",
            "no lanes ⇒ the ` · ` separator disappears too"
        );

        let many = preflight_of(json!([
            { "key": "k1" }, { "key": "k2" }, { "key": "k3" }, { "key": "k4" }, { "key": "k5" },
        ]));
        assert_eq!(
            format_workflow_preflight_summary(Some(&many)),
            "preflight · partial · 5 lanes: k1, k2, k3, k4, +1"
        );
        assert_eq!(
            format_workflow_preflight_plan_summary(Some(&many), ""),
            "Plan: 5 lanes · k1, k2, k3, k4, +1"
        );

        let warnings = vec!["a\n  warning".to_string(), "b".to_string()];
        assert_eq!(
            format_workflow_preflight_warning_summary(&warnings, "  ", None),
            "  Plan note: 2 preflight mismatches · details available for debug."
        );
        assert_eq!(
            format_workflow_preflight_warning_summary(&warnings[..1], "", Some("see log")),
            "Plan note: 1 preflight mismatch · see log."
        );
        assert_eq!(format_workflow_preflight_warning_summary(&[], "", None), "");
        assert_eq!(
            format_workflow_preflight_warnings(&warnings, " "),
            " Preflight warnings:\n   - a warning\n   - b"
        );
        assert_eq!(format_workflow_preflight_warnings(&[], ""), "");
    }

    /// `planLaneLabel`: one lane renders its decision; the trim-empty fallback is the key; the
    /// truncation cuts at 95 units, trims, then appends `…`.
    #[test]
    fn plan_lane_label_truncates_and_falls_back() {
        let single = preflight_of(json!([{ "key": "lane", "decision": "Do the thing" }]));
        assert_eq!(
            format_workflow_preflight_plan_summary(Some(&single), ""),
            "Plan: 1 lane · Do the thing"
        );

        // A decision whose 95th unit lands after a space: the trailing space is trimmed before
        // the ellipsis.
        let long_decision = format!("{} tail", "d".repeat(94));
        let long = preflight_of(json!([{ "key": "lane", "decision": long_decision }]));
        assert_eq!(
            format_workflow_preflight_plan_summary(Some(&long), ""),
            format!("Plan: 1 lane · {}…", "d".repeat(94)),
        );
    }

    /// §0.5: the two-argument `WorkflowPreflightLaneLookup` seam adapts with a closure — 3d's
    /// type is consumed, never changed.
    #[test]
    fn adapts_to_the_chat_progress_lookup_seam() {
        let preflight = preflight_of(json!([{ "key": "lane" }]));
        let lookup = |key: &WorkflowKey, generated: Option<&WorkflowKey>| {
            workflow_preflight_lane_for_runtime_key(
                Some(&preflight),
                key.as_str(),
                &[generated.map(WorkflowKey::as_str)],
            )
            .cloned()
        };
        let mut entry = run_entry("lane.child");
        entry.state = crate::workflows::WorkflowScriptTraceState::Completed;
        let rows = crate::workflows::build_workflow_chat_progress_rows(
            &[entry],
            Some(&lookup as crate::workflows::WorkflowPreflightLaneLookup<'_>),
        );
        assert_eq!(
            rows.first()
                .and_then(|row| row.preflight.as_ref())
                .map(|lane| lane.key.as_str()),
            Some("lane")
        );
    }
}
