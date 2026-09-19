//! pi `collectBoundedRefinementEvidence` (`src/agents/agent-refinements.ts:349-424` @v0.68.0) and
//! the four bounds that make it safe to put in front of a model — `withinAge` (`:292`),
//! `tailBytes` (`:298`), `pushCapped` (`:304`) and `evidencePacket` (`:337`).
//!
//! # Why this is its own module
//!
//! [`crate::exec::agent_refinements`] is the refinement FILE FORMAT — a parser, an overlay
//! applier and (in its `action` submodule) a serializer, all bound to bytes on disk. This module
//! reads something else entirely: the live fleet projection
//! ([`crate::tui::fleet_state::FleetState`], cyrup's port of pi's `SubagentState`) plus the
//! project's artifact directory. The only thing it shares with the format is the four caps, which
//! are imported from there rather than re-declared, so `metadataFor`'s stamped
//! `evidence` block and this collector's own bounds can never disagree.
//!
//! # What the packet is for
//!
//! The returned items are serialized into the proposal child's task
//! ([`crate::exec::agent_refinements::proposal::proposal_task`]) and their ids become the
//! `allowed` set every proposed edit must cite
//! ([`crate::exec::agent_refinements::proposal::validate_refinement_proposal`]'s A9/A10 arms).
//! That is why the packet is bounded twice — once per item ([`push_capped`]) and once in total
//! ([`evidence_packet`]) — and why the id grammar is load-bearing rather than cosmetic.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::exec::agent_refinements::{
    MAX_AGE_DAYS, MAX_EVIDENCE_ITEMS, MAX_ITEM_BYTES, MAX_PACKET_BYTES, record, text, text_array,
};
use crate::tui::fleet_state::{FleetState, step_status_label};

/// pi `RefinementEvidenceSource` (`agent-refinements.ts:20`).
///
/// `kebab-case` reproduces `"live-state"` / `"artifact-metadata"` / `"artifact-output"` exactly.
/// These strings are model-facing packet content, not a file format, but the proposal prompt is
/// how a child distinguishes a live step from a settled artifact, so they stay byte-identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RefinementEvidenceSource {
    /// pi `"live-state"` — a step of a run this process is tracking in memory (`:362`).
    LiveState,
    /// pi `"artifact-metadata"` — a settled run's `_meta.json` with no `_output.md` sibling.
    ArtifactMetadata,
    /// pi `"artifact-output"` — the same, with the output sibling present and tailed (`:409`).
    ArtifactOutput,
}

/// pi `RefinementEvidenceItem` (`agent-refinements.ts:22-38`).
///
/// # Field order is load-bearing
///
/// [`evidence_packet`] budgets `JSON.stringify(item).length + 1` against `MAX_PACKET_BYTES`
/// (`:341`). Reordering the struct does not change that byte count, but it DOES change the packet
/// the model reads, and the packet is the only thing tying a proposed edit to something that
/// actually happened. The order below is upstream's own object-literal order at `:360-368` (live)
/// and `:405-421` (artifact), which agree on every shared field.
///
/// `Serialize` only: nothing in this crate reads a packet back. The `refine` verb's round-trip
/// proof is on the overlay FILE, in [`crate::exec::agent_refinements::action`].
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RefinementEvidenceItem {
    /// `live:<run_id>:<filtered step index>` or `artifact:<meta basename>`.
    pub id: String,
    pub source: RefinementEvidenceSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,
    /// pi `acceptanceFields(...)` (`:309-324`). Absent on the LIVE half in cyrup — see
    /// [`collect_bounded_refinement_evidence`]'s "What cyrup cannot carry" note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acceptance_status: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub review_findings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub residual_risks: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// pi `controlSignals(...)` (`:326-335`). ALWAYS EMPTY in cyrup, and not because it is
    /// unimplemented: upstream builds each signal from `item.message` / `item.reason`
    /// (`shared/types.ts:383-384`) and cyrup's [`crate::exec::control::ControlEvent`] carries
    /// neither — it has `event_type`/`from`/`to`/`ts`/`run_id` and nothing else. There is no
    /// value to map. Rendering `format!("{event_type:?}")` here would invent packet content
    /// upstream never emits, so the field stays empty (and therefore omitted from the wire) until
    /// `ControlEvent` grows a message.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub control_signals: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tail: Option<String>,
    /// pi `refs` — the on-disk anchors for this item (`[asyncDir]`, or `[meta]`/`[meta, output]`).
    pub refs: Vec<String>,
}

/// pi `withinAge` (`agent-refinements.ts:292-296`).
///
/// Two asymmetries, both upstream's and both easy to "fix" into a divergence:
///
/// * `None` is **IN** (`:293` — `if (!at) return true`), not out. An item with no timestamp is
///   kept.
/// * There is **no lower bound** (`:295`). The comparison is `now - parsed <= MAX_AGE_DAYS…`, so a
///   stamp in the FUTURE yields a negative delta and is kept. A clock-skewed run is evidence.
///
/// The parameter is millis, not an ISO string. Upstream round-trips `isoTime` → `Date.parse`
/// purely because JS has no other carrier — both its call sites (`:356-357`, `:409-410`) compute
/// `at` from a number one line earlier. Taking the number removes an ISO parser from the hot path
/// and cannot drift from the renderer.
fn within_age(at_ms: Option<i64>, now_ms: i64) -> bool {
    let Some(at) = at_ms else { return true };
    now_ms.saturating_sub(at) <= i64::from(MAX_AGE_DAYS) * 24 * 60 * 60 * 1_000
}

/// pi `pushCapped` (`agent-refinements.ts:304-307`): refuse past [`MAX_EVIDENCE_ITEMS`], and cut
/// whatever `output_tail` survives to [`MAX_ITEM_BYTES`].
///
/// [CYRUP-DELTA] `tailBytes` (`:298-302`) slices raw bytes and lets `Buffer.toString("utf-8")`
/// emit U+FFFD for a code point split by the cut;
/// [`crate::exec::child_protocol::utf8_tail`](crate::exec::child_protocol::utf8_tail) instead
/// advances off continuation bytes so the tail starts on a character boundary
/// (`child_protocol.rs`'s `BoundedByteTail::push`). Both cut to at most `max_bytes`; the
/// difference is at most three bytes and one replacement character. `utf8_tail` is this crate's
/// single existing port of upstream's own `trimToUtf8Boundary`, and a second boundary walk here is
/// exactly the duplication that helper exists to prevent. [`evidence_packet`]'s budget is computed
/// from the ACTUAL serialized item, so the size difference is accounted for rather than assumed
/// away. `utf8_tail`'s documented `max_bytes == 0` edge is unreachable here: [`MAX_ITEM_BYTES`] is
/// 2048.
fn push_capped(items: &mut Vec<RefinementEvidenceItem>, mut item: RefinementEvidenceItem) {
    if items.len() >= MAX_EVIDENCE_ITEMS as usize {
        return;
    }
    item.output_tail = item
        .output_tail
        .as_deref()
        .map(|tail| crate::exec::child_protocol::utf8_tail(tail, MAX_ITEM_BYTES as usize).text);
    items.push(item);
}

/// pi `evidencePacket` (`agent-refinements.ts:337-347`): the longest PREFIX of `items` whose
/// serialized JSON array fits in [`MAX_PACKET_BYTES`].
///
/// Three details that a "nicer" implementation loses:
///
/// * the accounting seeds at `2` (the `[]`) and adds `+1` per item (the comma), so it budgets the
///   serialized PACKET rather than the sum of item sizes;
/// * it **breaks**, it does not `continue` (`:342`) — the first oversized item truncates the
///   packet, and a small item after it is NOT back-filled;
/// * `serde_json::to_string` cannot fail for this struct (no non-string map keys, no non-finite
///   floats), and `map_or(0, …)` keeps the function total without an `unwrap`. A `0` could only
///   ever INCLUDE an item, never silently drop one.
fn evidence_packet(items: Vec<RefinementEvidenceItem>) -> Vec<RefinementEvidenceItem> {
    let mut packet = Vec::new();
    let mut bytes = 2usize;
    for item in items {
        let encoded = serde_json::to_string(&item).map_or(0, |json| json.len()) + 1;
        if bytes + encoded > MAX_PACKET_BYTES as usize {
            break;
        }
        bytes += encoded;
        packet.push(item);
    }
    packet
}

/// pi `path.resolve(x)` — this crate's idiom (`background/scheduled_runs/store.rs:170-174`), plus
/// [`crate::spawn::worktree::lexical_normalize`] so `.`/`..` collapse the way Node's
/// `path.resolve` collapses them rather than being left in the string.
fn resolve_for_compare(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    crate::spawn::worktree::lexical_normalize(Path::new(""), &absolute)
}

/// pi `isoTime(number)` (`agent-refinements.ts:134-141`), whose numeric branch is
/// `new Date(ms).toISOString()` — [`crate::time::format_iso8601_millis`], whose own doc names it
/// that.
fn iso_time(ms: i64) -> String {
    crate::time::format_iso8601_millis(ms)
}

/// pi `acceptanceFields(value)` (`agent-refinements.ts:309-324`), applied to a JSON
/// `AcceptanceLedger`.
///
/// Returns `(acceptanceStatus, reviewFindings, residualRisks)`. `reviewFindings` concatenates
/// `childReport.reviewFindings` with `reviewResult.findings[].issue` in upstream's order (`:315`),
/// and both list fields are `Vec` because upstream's own conditional is `length > 0` (`:321-323`)
/// — an absent list and an empty list are indistinguishable on the wire, so there is one
/// representation, not two.
fn acceptance_fields(value: Option<&Value>) -> (Option<String>, Vec<String>, Vec<String>) {
    let Some(ledger) = record(value) else {
        return (None, Vec::new(), Vec::new());
    };
    let child_report = record(ledger.get("childReport"));
    let review_result = record(ledger.get("reviewResult"));
    let mut review_findings = text_array(child_report.and_then(|r| r.get("reviewFindings")));
    if let Some(findings) = review_result
        .and_then(|r| r.get("findings"))
        .and_then(Value::as_array)
    {
        review_findings.extend(
            findings
                .iter()
                .filter_map(|finding| text(record(Some(finding)).and_then(|f| f.get("issue"))))
                .map(str::to_string),
        );
    }
    let residual_risks = text_array(child_report.and_then(|r| r.get("residualRisks")));
    (
        text(ledger.get("status")).map(str::to_string),
        review_findings,
        residual_risks,
    )
}

/// pi `collectBoundedRefinementEvidence` (`agent-refinements.ts:349-424`) — the bounded, recent,
/// this-cwd, this-agent evidence packet the `refine` verb puts in front of a proposal child.
///
/// # Arguments
///
/// `state` is [`FleetState`] rather than a slice because it IS this crate's port of pi's
/// `SubagentState` (its own doc says so), and taking the whole projection keeps the signature
/// honest about the source. **Only [`FleetState::tracked_jobs`] is read** — upstream reads
/// `state.asyncJobs`, the in-memory map, and nothing else; `history_jobs` is cyrup's on-disk scan
/// and has no upstream counterpart here, so the production caller passes `include_history: false`.
///
/// `now_ms` is injected rather than read from [`crate::time::now_epoch_millis`] inside, so the age
/// cut is testable at its exact boundary without sleeping or doctoring file mtimes. Upstream does
/// the same thing through `withinAge(at, now = Date.now())`'s default parameter (`:292`).
///
/// Synchronous and `std::fs`-based for the artifact half, matching upstream's `readFileSync` and
/// this module family's existing sync style (`append_agent_refinement_overlay` is sync on the
/// spawn path). The caller is `async` and awaits the fleet state before calling.
///
/// # The dead arm this does NOT write
///
/// Upstream's run filter (`:353`) is two-clause — `job.agents?.includes(agentName)` OR
/// `job.steps?.some(...)` — and the `matchingSteps.length === 0` branch at `:359-369` exists only
/// for a run whose declared agent list names the agent while its steps have not materialized. In
/// cyrup that case cannot arise, and each premise is grep-verifiable:
///
/// * [`crate::background::RunStatus`] has no `agents` field (`background/records.rs:225-...`);
/// * the detached runner declares EVERY step, with its agent, in its first status write, before
///   any child spawns (`background/runner_main/entry.rs:267-280`);
/// * a tracked job with no status never reaches this function — the fleet builder skips it with
///   `let Some(status) = job.last_status else { continue };`
///   (`extension/executor/status.rs:256-258`).
///
/// So upstream's two clauses collapse to one and `matching_steps` is non-empty by construction
/// whenever the filter passes. [CYRUP-DELTA] writing the job-level `live:<run_id>` arm would be
/// dead code, so it is not written — and the `live:<run_id>` (no-index) id form therefore does not
/// exist in cyrup at all.
///
/// # What cyrup cannot carry, stated rather than silently dropped
///
/// * `acceptanceStatus` / `reviewFindings` / `residualRisks` are absent on LIVE items.
///   [`crate::exec::acceptance::model::types::AcceptanceLedger`] exists with every sub-field
///   upstream reads, but it hangs off [`crate::exec::SingleResult::acceptance`], persisted in the
///   run's terminal `result.json` — not on [`crate::background::StepStatus`]. Upstream's live half
///   does no I/O, and a `result.json` read per tracked job would change this function's cost
///   class. They ARE carried on the artifact half, where the ledger is in the `_meta.json` this
///   crate writes (`artifacts.rs`'s `run_artifact_metadata`).
/// * `controlSignals` is empty on both halves — see [`RefinementEvidenceItem::control_signals`].
#[must_use]
pub fn collect_bounded_refinement_evidence(
    cwd: &Path,
    agent_name: &str,
    state: &FleetState,
    now_ms: i64,
) -> Vec<RefinementEvidenceItem> {
    let mut items: Vec<RefinementEvidenceItem> = Vec::new();
    collect_live_evidence(cwd, agent_name, state, now_ms, &mut items);
    collect_artifact_evidence(cwd, agent_name, now_ms, &mut items);
    evidence_packet(items)
}

/// pi's live half (`agent-refinements.ts:350-387`).
fn collect_live_evidence(
    cwd: &Path,
    agent_name: &str,
    state: &FleetState,
    now_ms: i64,
    items: &mut Vec<RefinementEvidenceItem>,
) {
    let resolved_cwd = resolve_for_compare(cwd);
    // pi `:351-354` — this-cwd, names-this-agent, newest first. `!job.cwd ||` (`:352`) keeps a run
    // whose status carries no cwd: that is the common shape for a status synthesized by
    // reconciliation, which `background/records.rs:269` calls out with its own `None` carve-out.
    let mut jobs: Vec<_> = state
        .tracked_jobs
        .iter()
        .filter(|job| {
            job.status
                .cwd
                .as_ref()
                .is_none_or(|job_cwd| resolve_for_compare(job_cwd) == resolved_cwd)
        })
        .filter(|job| job.status.steps.iter().any(|step| step.agent == agent_name))
        .collect();
    // pi `:354` — `(b.updatedAt ?? 0) - (a.updatedAt ?? 0)`. cyrup's `last_update` is
    // non-optional, so the `?? 0` has no analogue; the sort key is the value itself.
    jobs.sort_by_key(|job| std::cmp::Reverse(job.updated_at()));

    for job in jobs {
        // pi `:356` — `isoTime(job.updatedAt ?? job.startedAt)`. `last_update` is always set in
        // cyrup (`fleet_state.rs`'s `updated_at()`), so the coalesce collapses to the first arm.
        let at_ms = job.updated_at();
        if !within_age(Some(at_ms), now_ms) {
            continue;
        }
        let run_dir = job.dir().display().to_string();
        let run_id = job.status.run_id.as_str().to_string();
        // pi `:371` — `matchingSteps.entries()`, the index within the FILTERED list, NOT the
        // step's flat position. Steps `[other, X, other, X]` are `:0` and `:1`, never `:1`/`:3`.
        for (index, step) in job
            .status
            .steps
            .iter()
            .filter(|step| step.agent == agent_name)
            .enumerate()
        {
            push_capped(
                items,
                RefinementEvidenceItem {
                    id: format!("live:{run_id}:{index}"),
                    source: RefinementEvidenceSource::LiveState,
                    run_id: Some(run_id.clone()),
                    agent: agent_name.to_string(),
                    at: Some(iso_time(at_ms)),
                    status: Some(step_status_label(step.status).to_string()),
                    model: step
                        .model
                        .as_ref()
                        .map(|model| model.as_str().trim().to_string())
                        .filter(|model| !model.is_empty()),
                    thinking: step
                        .telemetry
                        .thinking
                        .as_deref()
                        .map(str::trim)
                        .filter(|thinking| !thinking.is_empty())
                        .map(str::to_string),
                    // Absent by construction on this half — see the fn doc.
                    acceptance_status: None,
                    review_findings: Vec::new(),
                    residual_risks: Vec::new(),
                    // pi `:383` wraps the step error as a one-element array.
                    errors: step
                        .error
                        .as_deref()
                        .map(str::trim)
                        .filter(|error| !error.is_empty())
                        .map(|error| vec![error.to_string()])
                        .unwrap_or_default(),
                    control_signals: Vec::new(),
                    // [CYRUP-DELTA] pi's `text(stepRecord.recentOutput)` (`:384`) CAN NEVER FIRE:
                    // `recentOutput` is typed `string[]` (`shared/types.ts:1945`) and `text()`
                    // requires `typeof value === "string"` (`:126`), so `outputTail` is never set
                    // on a live-state item upstream. (The field is not vestigial overall — the
                    // artifact branch populates it at `:419` and `pushCapped` bounds it at
                    // `:306`; only this one READ is dead.) cyrup's
                    // `StepTelemetry::recent_output` is the same `Vec<String>`
                    // (`background/telemetry.rs:108`), so the lines are JOINED rather than a
                    // TypeScript type slip transcribed into Rust. `push_capped` cuts the result
                    // to MAX_ITEM_BYTES exactly as upstream cuts the artifact branch's tail, so
                    // this adds evidence without widening any cap.
                    output_tail: Some(step.telemetry.recent_output.join("\n"))
                        .filter(|tail| !tail.trim().is_empty()),
                    refs: vec![run_dir.clone()],
                },
            );
        }
    }
}

/// pi's artifact half (`agent-refinements.ts:389-422`).
fn collect_artifact_evidence(
    cwd: &Path,
    agent_name: &str,
    now_ms: i64,
    items: &mut Vec<RefinementEvidenceItem>,
) {
    let artifacts_dir = crate::artifacts::project_artifacts_dir(cwd);
    let Ok(entries) = std::fs::read_dir(&artifacts_dir) else {
        // pi `:390` gates the whole block on `fs.existsSync(artifactsDir)`; an unreadable or
        // absent directory is simply no artifact evidence.
        return;
    };
    // pi `:391-393` — `*_meta.json`, sorted by `statSync(b).mtimeMs - statSync(a).mtimeMs`, i.e.
    // by MTIME and not by the metadata's own `timestamp`. A `BTreeMap` keyed on
    // `(negated mtime, path)` gives that order deterministically, with the path as the tie-break
    // so two files written in the same millisecond do not swap between runs.
    let mut by_mtime: BTreeMap<(i64, PathBuf), PathBuf> = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .is_some_and(|name| name.ends_with("_meta.json"))
        {
            continue;
        }
        let Some(mtime) = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(crate::time::epoch_millis)
        else {
            continue;
        };
        by_mtime.insert((-mtime, path.clone()), path);
    }

    for ((negated_mtime, _), file) in by_mtime {
        // pi `:394` — a SECOND, redundant cap on top of `pushCapped`'s own. Ported because it is
        // what stops the loop reading and tailing files it would then discard.
        if items.len() >= MAX_EVIDENCE_ITEMS as usize {
            break;
        }
        let Ok(raw) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<Value>(&raw) else {
            // pi `:397` — a `try`/`catch` around the parse, then `if (!metadata …) continue`.
            continue;
        };
        let Some(metadata) = record(Some(&parsed)) else {
            continue;
        };
        if text(metadata.get("agent")) != Some(agent_name) {
            continue;
        }
        // pi `:409` — `isoTime(metadata.timestamp ?? stat.mtimeMs)`; `isoTime`'s numeric branch
        // requires a FINITE number, which `as_i64` already enforces.
        let at_ms = metadata
            .get("timestamp")
            .and_then(Value::as_i64)
            .unwrap_or(-negated_mtime);
        if !within_age(Some(at_ms), now_ms) {
            continue;
        }
        // pi `:412` — `file.replace(/_meta\.json$/, "_output.md")`, the sibling pair
        // `crate::artifacts::artifact_paths` mints.
        let output_path = output_sibling(&file);
        let output_tail = std::fs::read_to_string(&output_path).ok().map(|body| {
            crate::exec::child_protocol::utf8_tail(&body, MAX_ITEM_BYTES as usize).text
        });
        let base = file
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .and_then(|name| name.strip_suffix("_meta.json"))
            .unwrap_or_default()
            .to_string();
        let (acceptance_status, review_findings, residual_risks) =
            acceptance_fields(metadata.get("acceptance"));
        push_capped(
            items,
            RefinementEvidenceItem {
                id: format!("artifact:{base}"),
                source: if output_tail.is_some() {
                    RefinementEvidenceSource::ArtifactOutput
                } else {
                    RefinementEvidenceSource::ArtifactMetadata
                },
                run_id: text(metadata.get("runId")).map(str::to_string),
                agent: agent_name.to_string(),
                at: Some(iso_time(at_ms)),
                // pi `:407` — `typeof exitCode === "number" ? (exitCode === 0 ? "completed" :
                // "failed") : undefined`. The two labels are upstream's own and are NOT the
                // `RunState` vocabulary, so they are not routed through `run_state_label`.
                status: metadata
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .map(|code| if code == 0 { "completed" } else { "failed" }.to_string()),
                model: text(metadata.get("model")).map(str::to_string),
                thinking: text(metadata.get("thinking")).map(str::to_string),
                acceptance_status,
                review_findings,
                residual_risks,
                errors: text(metadata.get("error"))
                    .map(|error| vec![error.to_string()])
                    .unwrap_or_default(),
                // pi `:418` reads `metadata.controlEvents`. cyrup's `run_artifact_metadata` does
                // not write that key, and even if it did `ControlEvent` has no `message`/`reason`
                // to build a signal from — see `RefinementEvidenceItem::control_signals`.
                control_signals: Vec::new(),
                refs: if output_tail.is_some() {
                    vec![
                        file.display().to_string(),
                        output_path.display().to_string(),
                    ]
                } else {
                    vec![file.display().to_string()]
                },
                output_tail,
            },
        );
    }
}

/// pi `file.replace(/_meta\.json$/, "_output.md")` (`agent-refinements.ts:412`) — anchored at the
/// END of the file name only, so a directory component containing `_meta.json` is untouched.
fn output_sibling(meta_path: &Path) -> PathBuf {
    let Some(name) = meta_path.file_name().and_then(std::ffi::OsStr::to_str) else {
        return meta_path.to_path_buf();
    };
    match name.strip_suffix("_meta.json") {
        Some(base) => meta_path.with_file_name(format!("{base}_output.md")),
        None => meta_path.to_path_buf(),
    }
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
    use crate::background::{RunId, RunMode, RunPaths, RunState, RunStatus, StepState, StepStatus};
    use crate::tui::fleet_state::AsyncRunView;

    const DAY_MS: i64 = 24 * 60 * 60 * 1_000;

    fn step(agent: &str) -> StepStatus {
        let mut entry = StepStatus::pending(agent.to_string());
        entry.status = StepState::Running;
        entry
    }

    fn view(
        run_id: &str,
        cwd: Option<&Path>,
        last_update: i64,
        steps: Vec<StepStatus>,
    ) -> AsyncRunView {
        let id = RunId::from_token(run_id.to_string());
        let mut status = RunStatus::queued(id.clone(), RunMode::Single, None);
        status.cwd = cwd.map(Path::to_path_buf);
        status.state = RunState::Running;
        status.started_at = last_update;
        status.last_update = last_update;
        status.steps = steps;
        AsyncRunView {
            paths: RunPaths::for_run(
                Path::new("/tmp/does-not-exist/async"),
                Path::new("/tmp/does-not-exist/results"),
                &id,
            ),
            status,
            session_id: None,
            description: None,
            context: None,
            nested_children: Vec::new(),
        }
    }

    fn state(jobs: Vec<AsyncRunView>) -> FleetState {
        FleetState {
            tracked_jobs: jobs,
            ..FleetState::default()
        }
    }

    /// pi `:371` — the index is the position in the FILTERED list, not the step's flat position.
    /// This is the one mapping a reasonable Rust author gets wrong with `enumerate()` over the
    /// unfiltered slice.
    #[test]
    fn live_ids_use_the_filtered_step_index_not_the_flat_one() {
        let cwd = Path::new("/workspace/project");
        let jobs = vec![view(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            Some(cwd),
            1_000,
            vec![
                step("other"),
                step("refineworker"),
                step("other"),
                step("refineworker"),
            ],
        )];
        let items = collect_bounded_refinement_evidence(cwd, "refineworker", &state(jobs), 1_000);
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "live:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:0",
                "live:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa:1"
            ],
            "upstream indexes `matchingSteps.entries()`, so the two ids are :0 and :1 — never \
             :1 and :3"
        );
    }

    /// pi `:352`'s `!job.cwd ||`: a mismatched cwd is excluded and an ABSENT one is INCLUDED.
    /// The absent case is the common shape for a reconciliation-synthesized status
    /// (`background/records.rs:269`'s own `None` carve-out).
    #[test]
    fn the_cwd_filter_excludes_a_mismatch_and_keeps_an_absent_cwd() {
        let cwd = Path::new("/workspace/project");
        let jobs = vec![
            view(
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                Some(Path::new("/workspace/other")),
                2_000,
                vec![step("refineworker")],
            ),
            view(
                "cccccccccccccccccccccccccccccccc",
                None,
                1_000,
                vec![step("refineworker")],
            ),
        ];
        let items = collect_bounded_refinement_evidence(cwd, "refineworker", &state(jobs), 2_000);
        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids, vec!["live:cccccccccccccccccccccccccccccccc:0"]);
    }

    /// pi `:352` again, from the other side: `.`/`..` are collapsed before the comparison, the way
    /// `path.resolve` collapses them, so a job cwd spelled `<cwd>/sub/..` still matches.
    #[test]
    fn the_cwd_filter_normalizes_dot_segments_like_path_resolve() {
        let cwd = Path::new("/workspace/project");
        let jobs = vec![view(
            "dddddddddddddddddddddddddddddddd",
            Some(Path::new("/workspace/project/sub/..")),
            1_000,
            vec![step("refineworker")],
        )];
        let items = collect_bounded_refinement_evidence(cwd, "refineworker", &state(jobs), 1_000);
        assert_eq!(items.len(), 1);
    }

    /// pi `:295` exactly: `<=` at the boundary, nothing beyond it, and NO lower bound.
    #[test]
    fn the_age_cut_is_inclusive_at_fourteen_days_and_has_no_lower_bound() {
        let cwd = Path::new("/workspace/project");
        let last_update = 1_000_000_000_000;
        let jobs = vec![view(
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
            Some(cwd),
            last_update,
            vec![step("refineworker")],
        )];
        let fleet = state(jobs);

        let kept = collect_bounded_refinement_evidence(
            cwd,
            "refineworker",
            &fleet,
            last_update + 14 * DAY_MS,
        );
        assert_eq!(
            kept.len(),
            1,
            "exactly 14 days old is KEPT — pi's comparison is `<=`"
        );

        let dropped = collect_bounded_refinement_evidence(
            cwd,
            "refineworker",
            &fleet,
            last_update + 14 * DAY_MS + 1,
        );
        assert!(
            dropped.is_empty(),
            "one millisecond past the cut is dropped"
        );

        let future = collect_bounded_refinement_evidence(
            cwd,
            "refineworker",
            &fleet,
            last_update - 30 * DAY_MS,
        );
        assert_eq!(
            future.len(),
            1,
            "pi `:295` has no lower bound: a negative delta satisfies `<= 14 days`, so a \
             clock-skewed run in the FUTURE is evidence"
        );
    }

    /// pi `:293` — an item with no timestamp at all is kept.
    #[test]
    fn within_age_keeps_an_absent_timestamp() {
        assert!(
            within_age(None, 0),
            "pi `:293` returns true for a missing `at`"
        );
    }

    /// pi `:305` — `pushCapped` refuses once eight items are held, whatever else is offered.
    #[test]
    fn push_capped_stops_at_max_evidence_items() {
        let cwd = Path::new("/workspace/project");
        let jobs = vec![view(
            "ffffffffffffffffffffffffffffffff",
            Some(cwd),
            1_000,
            (0..9).map(|_| step("refineworker")).collect(),
        )];
        let items = collect_bounded_refinement_evidence(cwd, "refineworker", &state(jobs), 1_000);
        assert_eq!(items.len(), MAX_EVIDENCE_ITEMS as usize);
        assert_eq!(items.len(), 8);
    }

    fn item(id: &str, tail: Option<String>) -> RefinementEvidenceItem {
        RefinementEvidenceItem {
            id: id.to_string(),
            source: RefinementEvidenceSource::LiveState,
            run_id: None,
            agent: "a".to_string(),
            at: None,
            status: None,
            model: None,
            thinking: None,
            acceptance_status: None,
            review_findings: Vec::new(),
            residual_risks: Vec::new(),
            errors: Vec::new(),
            control_signals: Vec::new(),
            output_tail: tail,
            refs: Vec::new(),
        }
    }

    /// pi `:342` BREAKS. The first oversized item truncates the packet, and a small item after it
    /// is NOT back-filled — which is exactly what a `continue` would change.
    #[test]
    fn evidence_packet_breaks_on_the_first_oversized_item_and_does_not_backfill() {
        let huge = item("huge", Some("x".repeat(MAX_PACKET_BYTES as usize)));
        let small = item("small", None);
        let packet = evidence_packet(vec![huge, small]);
        assert!(
            packet.is_empty(),
            "pi `:342` is `break`, not `continue`: a small item AFTER the oversized one is not \
             back-filled"
        );
    }

    /// pi `:339`/`:341` — the budget is the serialized PACKET (`[` + items + commas + `]`), not
    /// the sum of item sizes. A prefix that fits is kept whole.
    #[test]
    fn evidence_packet_keeps_the_prefix_that_fits() {
        let items = (0..5)
            .map(|i| item(&format!("id{i}"), Some("y".repeat(4_000))))
            .collect::<Vec<_>>();
        let packet = evidence_packet(items);
        let encoded = serde_json::to_string(&packet).expect("serializes");
        assert_eq!(
            packet.len(),
            4,
            "five ~4 KiB items do not fit in 16 KiB; the longest PREFIX that does is four"
        );
        let ids: Vec<&str> = packet.iter().map(|row| row.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["id0", "id1", "id2", "id3"],
            "the packet is a PREFIX — the cut is at the first item that does not fit"
        );
        assert!(encoded.len() <= MAX_PACKET_BYTES as usize);
    }

    /// pi `:306` — the tail is the INPUT's tail, cut to `MAX_ITEM_BYTES`, not its head.
    #[test]
    fn push_capped_cuts_the_output_tail_to_max_item_bytes_keeping_the_end() {
        let mut body = "HEAD".to_string();
        body.push_str(&"m".repeat(4 * 1024));
        body.push_str("TAIL");
        let mut items = Vec::new();
        push_capped(&mut items, item("one", Some(body)));
        let tail = items[0].output_tail.as_deref().expect("a tail");
        assert!(tail.len() <= MAX_ITEM_BYTES as usize);
        assert!(tail.ends_with("TAIL"), "the END of the input survives");
        assert!(!tail.starts_with("HEAD"), "the HEAD of the input is cut");
    }

    /// The serialized item is upstream's object shape: camelCase keys, absent optionals OMITTED
    /// rather than `null`, empty lists omitted, and upstream's own key ORDER — which
    /// `evidence_packet` budgets against and the model reads.
    #[test]
    fn an_item_serializes_in_upstreams_key_order_and_omits_absent_fields() {
        let mut row = item("live:r:0", Some("tail".to_string()));
        row.run_id = Some("r".to_string());
        row.at = Some("2026-01-01T00:00:00.000Z".to_string());
        row.status = Some("running".to_string());
        row.refs = vec!["/runs/r".to_string()];
        let json = serde_json::to_string(&row).expect("serializes");
        assert_eq!(
            json,
            "{\"id\":\"live:r:0\",\"source\":\"live-state\",\"runId\":\"r\",\"agent\":\"a\",\
             \"at\":\"2026-01-01T00:00:00.000Z\",\"status\":\"running\",\
             \"outputTail\":\"tail\",\"refs\":[\"/runs/r\"]}"
        );
    }

    /// The PRODUCER half of the `acceptance` key, end to end: [`crate::artifacts::
    /// run_artifact_metadata`]'s own output is what [`acceptance_fields`] reads.
    ///
    /// The sibling test below writes its `_meta.json` by hand, which proves the READER but
    /// assumes the wire shape. That assumption is exactly what rots: rename a field on the ledger
    /// [`crate::exec::SingleResult::acceptance`] actually holds, or change its `serde` casing, and
    /// every production `refine` packet silently loses `acceptanceStatus` on every artifact item
    /// while a hand-authored fixture stays green. Here the fixture is the real producer's output,
    /// so the two halves cannot drift apart without this failing.
    ///
    /// It also pins WHICH ledger that is. `SingleResult.acceptance` is
    /// [`crate::exec::acceptance::AcceptanceLedger`] — the LATTICE ledger (`status`,
    /// `evidenceStatus`, `detail`, `verifyResults`) — not
    /// [`crate::exec::acceptance::model::AcceptanceLedger`], the faithful upstream shape that
    /// carries `childReport`/`reviewResult`. So `acceptanceStatus` has a real producer today and
    /// `reviewFindings`/`residualRisks` do not, and that asymmetry is asserted rather than
    /// assumed: it is the remaining half of the convergence the lattice module's own doc
    /// describes, and when that lands this test is where it shows up.
    #[test]
    fn run_artifact_metadatas_own_output_feeds_acceptance_fields() {
        use crate::exec::acceptance::{AcceptanceLedger, AcceptanceStatus};

        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path();
        let artifacts = crate::artifacts::project_artifacts_dir(cwd);
        std::fs::create_dir_all(&artifacts).expect("mkdir");

        // A real `SingleResult` from a production constructor, not a hand-rolled struct — a new
        // field on that type cannot leave this test compiling against a stale shape.
        let config = crate::exec::testsupport::sample_agent_config("fixture/model", &[]);
        let mut result = crate::exec::pre_spawn_failure(&config, "task", "diagnosis".to_string());
        result.agent = "probe".to_string();
        result.exit_code = 0;
        result.error = None;
        result.acceptance = Some(AcceptanceLedger {
            status: AcceptanceStatus::Verified,
            evidence_status: crate::exec::acceptance::model::AcceptanceEvidenceStatus::Verified,
            detail: Some("every verify command exited 0".to_string()),
            verify_results: Vec::new(),
        });

        let metadata = crate::artifacts::run_artifact_metadata("r1", &result);
        std::fs::write(
            artifacts.join("r1_probe_meta.json"),
            serde_json::to_string(&metadata).expect("metadata serializes"),
        )
        .expect("write meta");

        let items = collect_bounded_refinement_evidence(
            cwd,
            "probe",
            &FleetState::default(),
            crate::time::now_epoch_millis(),
        );
        let item = items
            .iter()
            .find(|i| i.id == "artifact:r1_probe")
            .expect("the producer's own metadata must be collected");
        assert_eq!(
            item.acceptance_status.as_deref(),
            Some("verified"),
            "`acceptance_fields` must read the status the REAL producer wrote; if this is None \
             the key and the reader have drifted and every production packet lost it silently"
        );
        assert_eq!(
            item.review_findings,
            Vec::<String>::new(),
            "the lattice ledger carries no `childReport`/`reviewResult`, so these are empty BY \
             CONSTRUCTION today — when the two ledgers converge this is the assertion to update, \
             and until then no comment may claim these fields have a producer"
        );
        assert_eq!(item.residual_risks, Vec::<String>::new());
    }

    /// pi `:407` / `:409` / `:412` — the artifact half keys off `_meta.json`, dates from the
    /// metadata's own `timestamp`, maps `exitCode` to upstream's two labels, and upgrades the
    /// source to `artifact-output` only when the `_output.md` sibling is present.
    #[test]
    fn the_artifact_half_reads_meta_json_and_pairs_the_output_sibling() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path();
        let artifacts = crate::artifacts::project_artifacts_dir(cwd);
        std::fs::create_dir_all(&artifacts).expect("mkdir");
        std::fs::write(
            artifacts.join("r1_probe_meta.json"),
            "{\"agent\":\"probe\",\"runId\":\"r1\",\"exitCode\":0,\"timestamp\":5000,\
             \"acceptance\":{\"status\":\"accepted\",\
             \"childReport\":{\"reviewFindings\":[\"f1\"],\"residualRisks\":[\"r1\"]}}}",
        )
        .expect("write meta");
        std::fs::write(artifacts.join("r1_probe_output.md"), "the output body").expect("write out");
        std::fs::write(
            artifacts.join("r2_probe_meta.json"),
            "{\"agent\":\"probe\",\"runId\":\"r2\",\"exitCode\":3,\"timestamp\":4000}",
        )
        .expect("write meta 2");
        std::fs::write(
            artifacts.join("r3_other_meta.json"),
            "{\"agent\":\"other\",\"runId\":\"r3\",\"exitCode\":0,\"timestamp\":4500}",
        )
        .expect("write meta 3");

        let items =
            collect_bounded_refinement_evidence(cwd, "probe", &FleetState::default(), 5_000);
        let by_id: std::collections::HashMap<&str, &RefinementEvidenceItem> =
            items.iter().map(|i| (i.id.as_str(), i)).collect();
        assert_eq!(items.len(), 2, "the other agent's metadata is filtered out");

        let first = by_id["artifact:r1_probe"];
        assert_eq!(first.source, RefinementEvidenceSource::ArtifactOutput);
        assert_eq!(first.status.as_deref(), Some("completed"));
        assert_eq!(first.output_tail.as_deref(), Some("the output body"));
        assert_eq!(
            first.refs.len(),
            2,
            "pi `:421` refs both files when the sibling exists"
        );
        assert_eq!(first.acceptance_status.as_deref(), Some("accepted"));
        assert_eq!(first.review_findings, vec!["f1".to_string()]);
        assert_eq!(first.residual_risks, vec!["r1".to_string()]);
        assert_eq!(first.at.as_deref(), Some("1970-01-01T00:00:05.000Z"));

        let second = by_id["artifact:r2_probe"];
        assert_eq!(second.source, RefinementEvidenceSource::ArtifactMetadata);
        assert_eq!(second.status.as_deref(), Some("failed"));
        assert_eq!(second.refs.len(), 1);
    }

    /// pi `:382-383` — a step error is carried as a ONE-element array, and the live telemetry's
    /// `recent_output` lines are joined into the tail (this crate's flagged delta on `:384`).
    #[test]
    fn a_live_item_carries_the_step_error_and_the_joined_recent_output() {
        let cwd = Path::new("/workspace/project");
        let mut only = step("refineworker");
        only.error = Some("  boom  ".to_string());
        only.telemetry.recent_output = vec!["line one".to_string(), "line two".to_string()];
        let jobs = vec![view(
            "11111111111111111111111111111111",
            Some(cwd),
            1_000,
            vec![only],
        )];
        let items = collect_bounded_refinement_evidence(cwd, "refineworker", &state(jobs), 1_000);
        assert_eq!(items[0].errors, vec!["boom".to_string()]);
        assert_eq!(items[0].output_tail.as_deref(), Some("line one\nline two"));
        assert!(items[0].control_signals.is_empty());
        assert!(items[0].acceptance_status.is_none());
    }
}
