//! pi `handleRefinementAction` (`src/agents/agent-refinements.ts:546-624` @v0.68.0) — the
//! `refine` / `refine.show` / `refine.rollback` verbs — plus the write primitives it drives:
//! `serializeRefinementFile` (`:239`), `writeRefinementFile` (`:262`), `metadataFor` (`:277`),
//! `baseMetadata` (`:269`), `hashPrompt` (`:143`), `readExisting` (`:165`) and `resolveOneAgent`
//! (`:538`).
//!
//! # The serializer is DERIVED from the parser, not transcribed from upstream
//!
//! [`super::parse_refinement_file`] was read in full before a byte of [`serialize_refinement_file`]
//! was written, and every literal the serializer emits is the const that parser matches. Its seven
//! hard requirements and where they are met:
//!
//! | # | Parser requirement | What the serializer emits |
//! |---|---|---|
//! | P1 | [`super::METADATA_PREFIX`] must `strip_prefix` at BYTE 0 | the file's first bytes are exactly `<!-- pi-subagents-refinement:v1\n` — no BOM, no leading blank line |
//! | P2 | the first [`super::METADATA_SUFFIX`] (`\n-->\n`) after the prefix ends the capture | the metadata JSON cannot contain that sequence: every newline `to_string_pretty` emits is a pretty-printer break, and every broken line is `{`/`}` at indent 0 or a line indented by at least two spaces — none begins `-->`; a `-->` INSIDE a JSON string is not preceded by a real newline, because a newline in a JSON string is escaped to the two bytes `\` `n` |
//! | P3 | the capture must be non-empty | an object literal is never `""` |
//! | P4 | `revision` must pass `integer_number` | [`integral_number`] renders `1`, not `1.0` |
//! | P5 | `base.source` must be one of `builtin`/`package`/`user`/`project` | see [`RefinementActionError::WouldNotRoundTrip`] — `runtime` is REFUSED rather than written |
//! | P6 | `extract_fence` needs the literal `"\n```<fence>\n"`, so a fence at byte 0 is not matched | both fences are preceded by a blank line, so both have a `\n` in front |
//! | P7 | the snapshots fence body must parse and be an array; an EMPTY body is a hard error | `to_string_pretty(&[])` is `"[]"` — never empty |
//!
//! Two further properties, stated because they are where a tidy-up breaks the format:
//!
//! * `current` is written VERBATIM and untrimmed. `refine` writes
//!   [`super::proposal::guidance_from_proposal`], which has no trailing newline; a serializer that
//!   "tidies" by appending one changes the parsed `current` on the next read and breaks
//!   idempotence.
//! * P6's body terminator is why the validator's A11 (the ` ``` ` refusal) is load-bearing for the
//!   WRITER and not only for the prompt: a `current` containing a fence truncates its own block.
//!   On the `refine` path A11 prevents it; on the `refine.rollback` path `current` comes from a
//!   snapshot `before` that was itself a validated `after` (or `""`), so the chain of custody
//!   holds — EXCEPT for a hand-edited file, where that `before` is attacker-authored. There the
//!   guard is [`write_refinement_file`]'s round trip, and it closes the gap only because it
//!   compares the REPARSED VALUE to the one written. A parses-without-error check would not:
//!   a `before` carrying its own snapshots fence parses fine, just to a truncated `current` and
//!   to the forged array embedded in it.

use std::collections::BTreeSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde_json::Value;
use sha2::{Digest, Sha256};

use super::proposal::{
    PROPOSAL_AGENT, ProposalChildOutcome, RefinementProposalRefusal, guidance_from_proposal,
    proposal_from_child, proposal_schema, proposal_task, validate_refinement_proposal,
};
use super::{
    CURRENT_FENCE, ParsedRefinementFile, REFINEMENT_FORMAT_VERSION, RefinementBase,
    RefinementEvidenceLimits, RefinementMetadata, RefinementSnapshot, SNAPSHOTS_FENCE,
    get_agent_refinement_path, parse_refinement_file,
};
use crate::discovery::types::AgentDefinition;
use crate::discovery::{AgentDiscoveryResult, AgentNameResolution};

/// pi `RefinementAction` (`agent-refinements.ts:19`).
///
/// The three wire literals exist HERE and nowhere else, so an added variant is a compile error at
/// every match rather than a silently-unhandled string.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefinementAction {
    /// pi `"refine"` — collect evidence, launch a proposal child, validate, write.
    Refine,
    /// pi `"refine.show"` — read-only.
    Show,
    /// pi `"refine.rollback"` — undo the last snapshot by APPENDING a new one.
    Rollback,
}

impl RefinementAction {
    /// The wire verb.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Refine => "refine",
            Self::Show => "refine.show",
            Self::Rollback => "refine.rollback",
        }
    }

    /// pi `subagent-executor.ts:6358` — the guard that selects this family.
    #[must_use]
    pub fn from_wire(action: &str) -> Option<Self> {
        match action {
            "refine" => Some(Self::Refine),
            "refine.show" => Some(Self::Show),
            "refine.rollback" => Some(Self::Rollback),
            _ => None,
        }
    }

    /// pi `MUTATING_MANAGEMENT_ACTIONS` (`subagent-executor.ts:213`) carries `refine` and
    /// `refine.rollback` and NOT `refine.show`, which is why the read verb stays reachable from a
    /// child-safe fanout tool (`:6359` consults the set, so `refine.show` falls through the gate).
    ///
    /// An exhaustive match rather than a `contains`, so adding a variant is a compile error rather
    /// than a silently-permitted mutation — the shape
    /// [`crate::extension::tool::lane_actions::LaneAction::is_mutating`] already uses for
    /// `lane.status`.
    #[must_use]
    pub fn is_mutating(self) -> bool {
        match self {
            Self::Refine | Self::Rollback => true,
            Self::Show => false,
        }
    }
}

/// pi `RefinementSnapshot["action"]` (`agent-refinements.ts:74`).
///
/// [`RefinementSnapshot::action`] itself stays a `String`: it is a PARSED field of an existing
/// public type whose parser already constrains it to `refine|rollback`
/// (`super::parse_refinement_file`), and widening it to an enum is a refactor of the read half
/// that this feature does not need. The writer sets it from here so the two literals are not
/// typed twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SnapshotAction {
    Refine,
    Rollback,
}

impl SnapshotAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Refine => "refine",
            Self::Rollback => "rollback",
        }
    }
}

/// pi `result(text, isError)` (`agent-refinements.ts:113-119`) — what a verb answers with when it
/// did not fail.
///
/// The two "nothing happened" outcomes are NOT errors upstream: `:593` (no evidence) and `:599`
/// (zero edits) both call `result(text)` with no second argument, while `:596`/`:598` pass `true`.
/// Collapsing them into [`RefinementActionError`] would make the tool report `is_error: true`
/// where upstream reports a plain answer. The shape is
/// [`crate::discovery::management::ManagementOutcome`]'s, which already establishes exactly this
/// distinction in this crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefinementActionOutcome {
    pub text: String,
    pub is_error: bool,
}

impl RefinementActionOutcome {
    fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
}

/// Every refusal [`handle_refinement_action`] can produce, with pi's own sentence in `Display`.
/// The in-`exec` precedent for this shape is `exec/child_transcript.rs:529`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RefinementActionError {
    /// pi `:548`. The sentence names BOTH surfaces, and interpolates the wire verb TWICE.
    #[error(
        "{action} requires agent. Use /subagents-refine <agent> or subagent({{ action: \"{action}\", agent: \"<agent>\" }})."
    )]
    MissingAgent { action: &'static str },
    /// pi `:551` — `resolveOneAgent`'s own error, unaltered.
    #[error("{0}")]
    AgentResolution(String),
    /// pi `:550`/`:554`'s catch — an unusable agent name, or an existing file that cannot be read
    /// or parsed. Carries `get_agent_refinement_path`'s or `parse_refinement_file`'s own message.
    ///
    /// A MALFORMED existing overlay is an ERROR here, including for `refine.show`, because
    /// `readExisting:168` throws on a parse failure and `:554` catches it. It must not be
    /// "improved" into the friendlier no-overlay sentence: a tampered overlay has to be visible,
    /// precisely because [`super::append_agent_refinement_overlay`] is silent about it by design.
    #[error("{0}")]
    ExistingFile(String),
    /// pi `:578` — `refine.rollback` with no overlay. (`refine.show`'s identical sentence at
    /// `:557` is NOT an error and is returned as ordinary text.)
    #[error("No refinement overlay exists for '{agent}'.")]
    NoOverlay { agent: String },
    /// pi `:580`.
    #[error("No refinement snapshot exists for '{agent}'.")]
    NoSnapshot { agent: String },
    /// pi `:596`.
    #[error("Refinement proposal child failed. No overlay was written.")]
    ProposalChildFailed,
    /// pi `:598` — the validator's sentence with upstream's own suffix.
    #[error("{refusal} No overlay was written.")]
    ProposalRefused { refusal: RefinementProposalRefusal },
    /// pi `:588`/`:617`'s catch.
    #[error("{0}")]
    Write(String),
    /// [CYRUP-DELTA, stricter-than-upstream] — no upstream analogue. Upstream writes
    /// unconditionally and can therefore produce a file its OWN parser refuses: `AgentConfig`'s
    /// `source` is `AgentSource`, a five-member union (`agents/agents.ts:31` @v0.68.0), while
    /// `parseRefinementFile`'s source check accepts only four (`agent-refinements.ts:194`), so
    /// `metadataFor` on a runtime-registered agent writes an unreadable overlay. cyrup has the same fifth variant
    /// ([`crate::discovery::types::AgentSource::Runtime`]), so the same hole exists here.
    ///
    /// The guard is worth having beyond that one case: an unreadable overlay is a SILENT no-op,
    /// because [`super::append_agent_refinement_overlay`] swallows every parse error by design —
    /// so it looks exactly like an absent one at spawn time. Re-parsing costs one pass over a few
    /// KiB and never changes a successful write's bytes.
    #[error(
        "Refusing to write a refinement overlay for '{agent}' that cannot be read back: {reason}"
    )]
    WouldNotRoundTrip { agent: String, reason: String },
}

// =================================================================================================
// The writer
// =================================================================================================

/// JS `JSON.stringify(n)` for a value the parser reads back through `integer_number`: an integral
/// `f64` renders as `1`, not `1.0`.
///
/// Not cosmetic. `serde_json` renders `f64` 1.0 as `"1.0"`; pi's own parse additionally demands
/// `Number.isInteger` (`agent-refinements.ts:194`), which `1.0` SATISFIES after `JSON.parse`, so
/// `1.0` is legal on both sides — but it is not what upstream WRITES, and this file is a format
/// shared with pi. Non-integral or out-of-range values fall back to the float form, which the
/// `evidence.*` fields legitimately allow (`super`'s `number_or` has no integrality check).
fn integral_number(value: f64) -> serde_json::Number {
    if value.is_finite()
        && value.fract() == 0.0
        && value >= -(2f64.powi(53))
        && value <= 2f64.powi(53)
    {
        #[allow(clippy::cast_possible_truncation)]
        let integral = value as i64;
        return serde_json::Number::from(integral);
    }
    serde_json::Number::from_f64(value).unwrap_or_else(|| serde_json::Number::from(0))
}

/// The same rule applied to model-facing TEXT: pi interpolates a JS number, so a revision of 3
/// prints as `3` and never as `3.0`.
fn integral_text(value: f64) -> String {
    integral_number(value).to_string()
}

/// pi `metadataFor`'s own JSON shape (`agent-refinements.ts:278-289`), in upstream's key order.
fn metadata_value(metadata: &RefinementMetadata) -> Value {
    serde_json::json!({
        "agent": metadata.agent,
        "revision": integral_number(metadata.revision),
        "updatedAt": metadata.updated_at,
        "base": {
            "source": metadata.base.source,
            "filePath": metadata.base.file_path,
            "systemPromptSha256": metadata.base.system_prompt_sha256,
        },
        "evidence": {
            "maxItems": integral_number(metadata.evidence.max_items),
            "maxAgeDays": integral_number(metadata.evidence.max_age_days),
            "itemBytes": integral_number(metadata.evidence.item_bytes),
            "totalBytes": integral_number(metadata.evidence.total_bytes),
        },
    })
}

/// pi's snapshot literal (`agent-refinements.ts:604-610` / `:586`), in upstream's key order.
///
/// `proposalAgent` is OMITTED, not `null`, when absent — upstream's conditional spread plus
/// `JSON.stringify`'s drop-`undefined`, and the shape `super::parse_refinement_file` reads back
/// through `text()`.
fn snapshot_value(snapshot: &RefinementSnapshot) -> Value {
    let mut value = serde_json::json!({
        "revision": integral_number(snapshot.revision),
        "at": snapshot.at,
        "action": snapshot.action,
        "before": snapshot.before,
        "after": snapshot.after,
        "evidenceIds": snapshot.evidence_ids,
    });
    if let Some(proposal_agent) = snapshot.proposal_agent.as_ref()
        && let Some(object) = value.as_object_mut()
    {
        object.insert(
            "proposalAgent".to_string(),
            Value::String(proposal_agent.clone()),
        );
    }
    value
}

/// pi `serializeRefinementFile` (`agent-refinements.ts:239-260`) — upstream's own element order,
/// joined with `\n`, ending in a trailing newline (the final `""` element at `:258`).
///
/// See this module's doc for the parser requirement each literal satisfies.
#[must_use]
pub fn serialize_refinement_file(parsed: &ParsedRefinementFile) -> String {
    let metadata = serde_json::to_string_pretty(&metadata_value(&parsed.metadata))
        .unwrap_or_else(|_| "{}".to_string());
    let snapshots: Vec<Value> = parsed.snapshots.iter().map(snapshot_value).collect();
    let snapshots = serde_json::to_string_pretty(&snapshots).unwrap_or_else(|_| "[]".to_string());
    let agent = &parsed.metadata.agent;
    let current = &parsed.current;
    [
        format!("<!-- pi-subagents-refinement:v{REFINEMENT_FORMAT_VERSION}"),
        metadata,
        "-->".to_string(),
        String::new(),
        format!("# Current refinement for `{agent}`"),
        String::new(),
        format!("```{CURRENT_FENCE}"),
        current.clone(),
        "```".to_string(),
        String::new(),
        "# Snapshots".to_string(),
        String::new(),
        format!("```{SNAPSHOTS_FENCE}"),
        snapshots,
        "```".to_string(),
        String::new(),
    ]
    .join("\n")
}

/// pi `writeRefinementFile` (`agent-refinements.ts:262-267`), plus the round-trip guard.
///
/// Upstream is `mkdirSync(dirname)` → write a `pid + uuid` temp file → `renameSync`.
/// [`crate::background::atomic::write_atomic_text`] is that exact shape over this crate's single
/// temp-then-rename implementation.
///
/// The round trip is enforced HERE, not only in a test: a file this crate cannot read back is a
/// silent no-op at spawn time. See [`RefinementActionError::WouldNotRoundTrip`].
///
/// # It is an EQUALITY check, not a parses-without-error check
///
/// "The bytes parse" is strictly weaker than "the bytes mean what was written", and the gap
/// between the two is reachable. `extract_fence` terminates the `current` body at the FIRST
/// `"\n```"` (`agent_refinements.rs:286-290`) and the snapshots lookup scans the WHOLE markdown,
/// so a `current` that itself contains a snapshots fence yields a file that parses cleanly to a
/// DIFFERENT `current` and to the FORGED snapshot array embedded inside it. `refine` cannot reach
/// that state — the validator's A11 refuses ` ``` ` in guidance — but `refine.rollback` sets
/// `current` from a snapshot's `before`, and on a hand-edited overlay that `before` is
/// attacker-authored. That is the exact outcome `proposal.rs`'s A11 note names. Comparing the
/// reparsed value to the written one is what actually closes it; comparing only `is_ok()` did
/// not.
///
/// # Errors
///
/// [`RefinementActionError::WouldNotRoundTrip`] when the serialized bytes do not parse, or parse
/// to a value other than the one written, and [`RefinementActionError::Write`] for any filesystem
/// failure.
pub async fn write_refinement_file(
    path: &Path,
    parsed: &ParsedRefinementFile,
) -> Result<(), RefinementActionError> {
    let serialized = serialize_refinement_file(parsed);
    let label = path.display().to_string();
    let reparsed = parse_refinement_file(&serialized, &label).map_err(|reason| {
        RefinementActionError::WouldNotRoundTrip {
            agent: parsed.metadata.agent.clone(),
            reason,
        }
    })?;
    if &reparsed != parsed {
        return Err(RefinementActionError::WouldNotRoundTrip {
            agent: parsed.metadata.agent.clone(),
            reason: format!(
                "{label} would re-read as a different overlay than the one written (current \
                 block and/or snapshot array changed on re-parse)"
            ),
        });
    }
    crate::background::atomic::write_atomic_text(path, &serialized)
        .await
        .map_err(|err| RefinementActionError::Write(err.to_string()))
}

/// pi `hashPrompt` (`agent-refinements.ts:143-145`) — lowercase hex SHA-256 of the RAW persona
/// body.
///
/// What is NOT hashed: the COMPOSED prompt the child receives. Skills, memory, acceptance and the
/// overlay itself are all folded in later (see this module family's composition-order note), so
/// only [`AgentDefinition::system_prompt_body`] — upstream's `AgentConfig.systemPrompt` — is
/// hashed. Hashing the composed prompt would make `refine.show`'s drift check report `yes` on
/// every run, because the overlay is part of the composition.
///
/// The crate's `sha2` 0.11 hex idiom (`exec/mutation_evidence/repo.rs:190-201`): 0.11's output has
/// no `LowerHex`, so the encoding is an explicit fold.
#[must_use]
pub fn hash_prompt(system_prompt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(system_prompt.as_bytes());
    hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut out, byte| {
            use std::fmt::Write as _;
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// pi `baseMetadata` (`agent-refinements.ts:269-275`) + `metadataFor` (`:277-290`).
///
/// `evidence` is [`RefinementEvidenceLimits::defaults`] — the SAME four constants
/// [`crate::exec::refinement_evidence`] bounds itself by, not a second copy.
fn metadata_for(agent: &AgentDefinition, revision: f64, now_iso: &str) -> RefinementMetadata {
    RefinementMetadata {
        agent: agent.name.clone(),
        revision,
        updated_at: now_iso.to_string(),
        base: RefinementBase {
            // `source_str` returns `"runtime"` for a runtime-registered agent, which
            // `parse_refinement_file` rejects — the round-trip guard turns that into a refusal
            // rather than an unreadable file.
            source: crate::discovery::management::helpers::source_str(agent.source).to_string(),
            file_path: agent.file_path.display().to_string(),
            system_prompt_sha256: hash_prompt(&agent.system_prompt_body),
        },
        evidence: RefinementEvidenceLimits::defaults(),
    }
}

/// pi `readExisting` (`agent-refinements.ts:165-169`).
///
/// `Ok((path, None))` is the MISSING file (upstream's `existsSync` gate at `:167`); a read or
/// parse failure of a file that DOES exist is an `Err` that reaches pi's `:554` catch. `std::fs`
/// and sync, matching the read half's own style.
fn read_existing(
    cwd: &Path,
    agent_name: &str,
) -> Result<(PathBuf, Option<ParsedRefinementFile>), String> {
    let path = get_agent_refinement_path(cwd, agent_name)?;
    if !path.exists() {
        return Ok((path, None));
    }
    let label = path.display().to_string();
    let raw = std::fs::read_to_string(&path).map_err(|err| format!("{label}: {err}"))?;
    let parsed = parse_refinement_file(&raw, &label)?;
    Ok((path, Some(parsed)))
}

/// pi `resolveOneAgent` (`agent-refinements.ts:538-544`) over cyrup's discovery.
///
/// The blocking-diagnostic check runs FIRST, as `canonicalizeAgentName` does
/// (`subagent-executor.ts:2336-2344`) and as this crate's own canonical consumer
/// `extension/executor/resolve.rs:221-243` does: a malformed definition that outranks every
/// resolved candidate refuses the verb with its parse error rather than silently acting on a
/// lower-tier same-named agent.
///
/// [CYRUP-DELTA] the NOT-FOUND sentence is [`crate::error::SubagentError::AgentNotFound`]'s
/// (`agent not found: <name>`), not upstream's `formatUnknownAgentError(name,
/// unknownAgentDiagnosticContext(discovered))`. cyrup has no port of that formatter — no call site
/// in this crate renders it — so this uses the unknown-agent sentence every other launch path in
/// the crate renders, rather than inventing a second one here. The AMBIGUOUS sentence IS
/// upstream's, unaltered ([`AgentNameResolution::Ambiguous`] carries pi's own wording).
fn resolve_one_agent<'a>(
    agents: &'a AgentDiscoveryResult,
    name: &str,
) -> Result<&'a AgentDefinition, String> {
    let resolution = crate::discovery::resolve_agent_name(name, &agents.agents);
    let candidates = crate::discovery::blocking_candidates(name, &agents.agents, &resolution);
    if let Some(diagnostic) = crate::discovery::find_blocking_agent_diagnostic(
        name,
        &candidates,
        &agents.agent_diagnostics,
    ) {
        return Err(crate::error::SubagentError::InvalidAgentConfiguration {
            name: name.to_string(),
            error: diagnostic.error.clone(),
        }
        .to_string());
    }
    match resolution {
        AgentNameResolution::Found(agent) => Ok(agent),
        AgentNameResolution::Ambiguous(message) => Err(message),
        AgentNameResolution::NotFound => {
            Err(crate::error::SubagentError::AgentNotFound(name.to_string()).to_string())
        }
    }
}

// =================================================================================================
// The handler
// =================================================================================================

/// pi `LaunchRefinementProposalChild` (`agent-refinements.ts:100-104`).
///
/// The handler never names an executor; the caller supplies the launch. That indirection is
/// upstream's, and it is what makes every write path in this module testable without a child
/// process.
pub type LaunchProposalChild<'a> = &'a (
        dyn Fn(String, Value) -> Pin<Box<dyn Future<Output = ProposalChildOutcome> + Send + 'a>>
            + Send
            + Sync
            + 'a
    );

/// pi `RefinementActionContext` (`agent-refinements.ts:106-111`).
pub struct RefinementActionContext<'a> {
    pub cwd: &'a Path,
    /// pi `ctx.state` — cyrup's port of `SubagentState`, already built by
    /// `SubagentExecutor::fleet_state` (`extension/executor/status.rs:200`).
    pub state: &'a crate::tui::fleet_state::FleetState,
    /// The already-resolved agents for `cwd`. Passed in rather than discovered inside, so
    /// [`resolve_one_agent`] sees the same `agent_diagnostics` the rest of the crate resolves
    /// against.
    pub agents: &'a AgentDiscoveryResult,
    /// pi's `new Date().toISOString()` at `:581` and `:600`, and `Date.now()` inside
    /// `collectBoundedRefinementEvidence`'s `withinAge`. Injected so a write test can pin the
    /// `updatedAt`/`at` bytes without a clock; production passes
    /// [`crate::time::now_epoch_millis`].
    pub now_ms: i64,
    pub launch_proposal_child: LaunchProposalChild<'a>,
}

/// pi `handleRefinementAction` (`agent-refinements.ts:546-624`).
///
/// The prelude's ORDER is upstream's and is observable: missing-agent (`:548`), then resolve
/// (`:550`), then read-existing (`:554`), and only then the verb switch. `:554` passes
/// `agent.name` — the RESOLVED name, not the requested one — so an alias reaches the same file the
/// canonical name does.
///
/// # Errors
///
/// [`RefinementActionError`], one variant per upstream refusal. The two "nothing to do" outcomes
/// (`:593`, `:599`) are `Ok` with `is_error: false`, exactly as upstream returns them.
pub async fn handle_refinement_action(
    action: RefinementAction,
    requested_agent: Option<&str>,
    ctx: RefinementActionContext<'_>,
) -> Result<RefinementActionOutcome, RefinementActionError> {
    // pi `:547-548` — `params.agent?.trim()`, where a blank string is as absent as a missing one.
    let requested = requested_agent
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or(RefinementActionError::MissingAgent {
            action: action.as_str(),
        })?;
    // pi `:550-551`.
    let agent =
        resolve_one_agent(ctx.agents, requested).map_err(RefinementActionError::AgentResolution)?;
    // pi `:553-554`.
    let (path, existing) =
        read_existing(ctx.cwd, &agent.name).map_err(RefinementActionError::ExistingFile)?;

    match action {
        RefinementAction::Show => Ok(render_show(agent, &path, existing.as_ref())),
        RefinementAction::Rollback => rollback(agent, &path, existing, ctx.now_ms).await,
        RefinementAction::Refine => refine(agent, &path, existing, &ctx).await,
    }
}

/// pi `:556-575` — read-only, and NOT an error when no overlay exists (`:557` has no `true`,
/// unlike `:578`).
fn render_show(
    agent: &AgentDefinition,
    path: &Path,
    existing: Option<&ParsedRefinementFile>,
) -> RefinementActionOutcome {
    let Some(parsed) = existing else {
        return RefinementActionOutcome::ok(format!(
            "No refinement overlay exists for '{}'.",
            agent.name
        ));
    };
    // pi `:558-559` — the drift check, against the RAW persona body.
    let drift =
        if parsed.metadata.base.system_prompt_sha256 == hash_prompt(&agent.system_prompt_body) {
            "no"
        } else {
            "yes"
        };
    // pi `:560` — `slice(-5)` keeps the LAST five in STORED order (oldest of the five first).
    // `.rev().take(5)` alone would silently invert the history, so the second `.rev()` is not
    // decoration.
    let recent: Vec<&RefinementSnapshot> = parsed
        .snapshots
        .iter()
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let history = if recent.is_empty() {
        // pi's `|| "- none"` fires when the JOINED string is empty, i.e. at zero snapshots.
        "- none".to_string()
    } else {
        recent
            .iter()
            .map(|snapshot| {
                let count = snapshot.evidence_ids.len();
                let plural = if count == 1 { "" } else { "s" };
                format!(
                    "- r{} {} at {} ({count} evidence id{plural})",
                    integral_text(snapshot.revision),
                    snapshot.action,
                    snapshot.at
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let current = parsed.current.trim();
    let current = if current.is_empty() {
        "(empty)"
    } else {
        current
    };
    RefinementActionOutcome::ok(
        [
            format!("Refinement overlay for '{}'", agent.name),
            format!("Path: {}", path.display()),
            format!("Revision: {}", integral_text(parsed.metadata.revision)),
            format!("Updated: {}", parsed.metadata.updated_at),
            format!(
                "Base: {} {}",
                parsed.metadata.base.source, parsed.metadata.base.file_path
            ),
            format!("Base prompt changed since overlay: {drift}"),
            String::new(),
            "Current guidance:".to_string(),
            current.to_string(),
            String::new(),
            "Recent history:".to_string(),
            history,
        ]
        .join("\n"),
    )
}

/// pi `:577-590` — APPENDS a rollback snapshot; it never pops.
///
/// Five exact facts, each of which a test pins: the new snapshot's `before` is the FILE's current
/// and its `after` is `latest.before`; `evidenceIds` is COPIED from the snapshot being undone; the
/// rollback snapshot carries NO `proposalAgent` (contrast `:610`); `metadataFor` re-stamps `base`
/// from the LIVE agent, so a rollback also refreshes the drift baseline; and a second consecutive
/// rollback undoes the first, because `snapshots.at(-1)` is now the rollback entry whose `before`
/// is the pre-rollback guidance. Upstream's rollback is an OSCILLATOR, not a stack walk — the
/// single easiest thing here to "improve" into a divergence.
async fn rollback(
    agent: &AgentDefinition,
    path: &Path,
    existing: Option<ParsedRefinementFile>,
    now_ms: i64,
) -> Result<RefinementActionOutcome, RefinementActionError> {
    let parsed = existing.ok_or_else(|| RefinementActionError::NoOverlay {
        agent: agent.name.clone(),
    })?;
    let latest =
        parsed
            .snapshots
            .last()
            .cloned()
            .ok_or_else(|| RefinementActionError::NoSnapshot {
                agent: agent.name.clone(),
            })?;
    let now = crate::time::format_iso8601_millis(now_ms);
    let next_revision = parsed.metadata.revision + 1.0;
    let mut snapshots = parsed.snapshots.clone();
    snapshots.push(RefinementSnapshot {
        revision: next_revision,
        at: now.clone(),
        action: SnapshotAction::Rollback.as_str().to_string(),
        before: parsed.current.clone(),
        after: latest.before.clone(),
        evidence_ids: latest.evidence_ids.clone(),
        proposal_agent: None,
    });
    let next = ParsedRefinementFile {
        metadata: metadata_for(agent, next_revision, &now),
        current: latest.before.clone(),
        snapshots,
    };
    write_refinement_file(path, &next).await?;
    Ok(RefinementActionOutcome::ok(format!(
        "Rolled back refinement overlay for '{}' to revision {}.\nPath: {}",
        agent.name,
        integral_text(next_revision),
        path.display()
    )))
}

/// pi `:592-623`.
async fn refine(
    agent: &AgentDefinition,
    path: &Path,
    existing: Option<ParsedRefinementFile>,
    ctx: &RefinementActionContext<'_>,
) -> Result<RefinementActionOutcome, RefinementActionError> {
    let evidence = crate::exec::refinement_evidence::collect_bounded_refinement_evidence(
        ctx.cwd,
        &agent.name,
        ctx.state,
        ctx.now_ms,
    );
    // pi `:593` — launches NOTHING and writes NOTHING, and is not an error.
    if evidence.is_empty() {
        return Ok(RefinementActionOutcome::ok(format!(
            "No bounded recent evidence was found for '{}'. No proposal child was launched and \
             no overlay was written.",
            agent.name
        )));
    }
    // pi `:594` — an absent file is `current = ""`, so `before` on a first-ever refine is the
    // empty string and not a sentinel.
    let current = existing
        .as_ref()
        .map(|parsed| parsed.current.clone())
        .unwrap_or_default();

    let task = proposal_task(agent, &current, &evidence);
    let child = (ctx.launch_proposal_child)(task, proposal_schema()).await;
    // pi `:596`.
    if child.is_error {
        return Err(RefinementActionError::ProposalChildFailed);
    }
    // pi `:597` — `allowed` is built from the packet THIS parent just collected.
    let allowed: BTreeSet<&str> = evidence.iter().map(|item| item.id.as_str()).collect();
    let proposal = validate_refinement_proposal(&proposal_from_child(&child), &allowed)
        // pi `:598`.
        .map_err(|refusal| RefinementActionError::ProposalRefused { refusal })?;
    // pi `:599` — an ordinary answer, not an error, and no write.
    if proposal.edits.is_empty() {
        return Ok(RefinementActionOutcome::ok(format!(
            "The proposal child returned no edits for '{}'. No overlay was written.",
            agent.name
        )));
    }

    let now = crate::time::format_iso8601_millis(ctx.now_ms);
    // pi `:601` — a fresh file starts at revision 1 (`?? 0` then `+ 1`).
    let next_revision = existing
        .as_ref()
        .map_or(0.0, |parsed| parsed.metadata.revision)
        + 1.0;
    let after = guidance_from_proposal(&proposal);
    // pi `:609` — de-duplicated ACROSS edits, preserving first-seen order (`new Set` over a
    // `flatMap`).
    let mut seen = BTreeSet::new();
    let evidence_ids: Vec<String> = proposal
        .edits
        .iter()
        .flat_map(|edit| edit.evidence_ids.iter())
        .map(|id| id.as_str().to_string())
        .filter(|id| seen.insert(id.clone()))
        .collect();
    let mut snapshots = existing
        .as_ref()
        .map(|parsed| parsed.snapshots.clone())
        .unwrap_or_default();
    snapshots.push(RefinementSnapshot {
        revision: next_revision,
        at: now.clone(),
        action: SnapshotAction::Refine.as_str().to_string(),
        before: current,
        after: after.clone(),
        evidence_ids,
        // pi `:610` — the ONLY consumer of `PROPOSAL_AGENT`. The launch site names the persona
        // separately, so the snapshot records the CONSTANT, not necessarily the agent launched.
        proposal_agent: Some(PROPOSAL_AGENT.to_string()),
    });
    let next = ParsedRefinementFile {
        metadata: metadata_for(agent, next_revision, &now),
        current: after,
        snapshots,
    };
    write_refinement_file(path, &next).await?;
    Ok(RefinementActionOutcome::ok(
        [
            format!(
                "Wrote refinement overlay for '{}' at revision {}.",
                agent.name,
                integral_text(next_revision)
            ),
            format!("Path: {}", path.display()),
            // pi `:621` — the packet the collector returned.
            format!("Evidence items: {}", evidence.len()),
            format!("Edits: {}", proposal.edits.len()),
        ]
        .join("\n"),
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use crate::discovery::management::test_support::sample_agent;
    use crate::discovery::types::AgentSource;

    /// A resolved persona, from the crate's shared discovery fixture — not a hand-rolled struct,
    /// so a new `AgentDefinition` field cannot leave this file compiling against a stale shape.
    fn probe_agent(body: &str) -> AgentDefinition {
        let mut agent = sample_agent(
            AgentSource::Project,
            PathBuf::from("/p/.cyrup/agents/probe.md"),
        );
        agent.name = "probe".to_string();
        agent.local_name = "probe".to_string();
        agent.system_prompt_body = body.to_string();
        agent
    }

    fn metadata(agent: &str, revision: f64) -> RefinementMetadata {
        RefinementMetadata {
            agent: agent.to_string(),
            revision,
            updated_at: "2026-09-19T00:00:00.000Z".to_string(),
            base: RefinementBase {
                source: "project".to_string(),
                file_path: "/p/.cyrup/agents/probe.md".to_string(),
                system_prompt_sha256: "abc123".to_string(),
            },
            evidence: RefinementEvidenceLimits::defaults(),
        }
    }

    fn snapshot(revision: f64, proposal_agent: Option<&str>) -> RefinementSnapshot {
        RefinementSnapshot {
            revision,
            at: "2026-09-19T00:00:00.000Z".to_string(),
            action: "refine".to_string(),
            before: String::new(),
            after: "- new".to_string(),
            evidence_ids: vec!["live:a1".to_string()],
            proposal_agent: proposal_agent.map(str::to_string),
        }
    }

    /// P1 and P4 in one assertion, plus the whole layout: the FORMAT is pinned against a
    /// hand-written literal, not merely against its own round trip.
    #[test]
    fn the_serialized_bytes_are_upstreams_layout() {
        let parsed = ParsedRefinementFile {
            metadata: metadata("probe", 1.0),
            current: "- keep diffs small".to_string(),
            snapshots: vec![snapshot(1.0, Some("reviewer"))],
        };
        let serialized = serialize_refinement_file(&parsed);
        let expected = concat!(
            "<!-- pi-subagents-refinement:v1\n",
            "{\n",
            "  \"agent\": \"probe\",\n",
            "  \"revision\": 1,\n",
            "  \"updatedAt\": \"2026-09-19T00:00:00.000Z\",\n",
            "  \"base\": {\n",
            "    \"source\": \"project\",\n",
            "    \"filePath\": \"/p/.cyrup/agents/probe.md\",\n",
            "    \"systemPromptSha256\": \"abc123\"\n",
            "  },\n",
            "  \"evidence\": {\n",
            "    \"maxItems\": 8,\n",
            "    \"maxAgeDays\": 14,\n",
            "    \"itemBytes\": 2048,\n",
            "    \"totalBytes\": 16384\n",
            "  }\n",
            "}\n",
            "-->\n",
            "\n",
            "# Current refinement for `probe`\n",
            "\n",
            "```pi-subagents-refinement-current\n",
            "- keep diffs small\n",
            "```\n",
            "\n",
            "# Snapshots\n",
            "\n",
            "```pi-subagents-refinement-snapshots-json\n",
            "[\n",
            "  {\n",
            "    \"revision\": 1,\n",
            "    \"at\": \"2026-09-19T00:00:00.000Z\",\n",
            "    \"action\": \"refine\",\n",
            "    \"before\": \"\",\n",
            "    \"after\": \"- new\",\n",
            "    \"evidenceIds\": [\n",
            "      \"live:a1\"\n",
            "    ],\n",
            "    \"proposalAgent\": \"reviewer\"\n",
            "  }\n",
            "]\n",
            "```\n",
        );
        assert_eq!(serialized, expected);
        assert!(
            serialized.starts_with(super::super::METADATA_PREFIX),
            "P1: the metadata comment must be at byte 0"
        );
        assert!(
            serialized.contains("\"revision\": 1") && !serialized.contains("\"revision\": 1.0"),
            "P4: an integral revision renders as `1`, never `1.0` — pi's own parse reads it \
             through `Number.isInteger` and this file is a format shared with pi"
        );
    }

    /// THE round-trip proof: serialize → the EXISTING parser → serialize is a fixed point, over
    /// every shape the writer can produce.
    #[test]
    fn serialize_parse_serialize_is_a_fixed_point() {
        let cases: Vec<ParsedRefinementFile> = vec![
            // Empty current, zero snapshots — the shape a rollback to an empty `before` writes.
            ParsedRefinementFile {
                metadata: metadata("probe", 1.0),
                current: String::new(),
                snapshots: Vec::new(),
            },
            // Multi-line current with no trailing newline — what `guidance_from_proposal`
            // produces for two edits.
            ParsedRefinementFile {
                metadata: metadata("probe", 2.0),
                current: "- one\n- two".to_string(),
                snapshots: vec![snapshot(1.0, Some("reviewer")), {
                    let mut rollback = snapshot(2.0, None);
                    rollback.action = "rollback".to_string();
                    rollback.before = "- new".to_string();
                    rollback.after = String::new();
                    rollback.evidence_ids = Vec::new();
                    rollback
                }],
            },
            // Non-ASCII guidance, and a `current` whose own content looks like markdown.
            ParsedRefinementFile {
                metadata: metadata("probe", 7.0),
                current: "- garde les diffs petits — ✅\n- # not a heading".to_string(),
                snapshots: vec![snapshot(7.0, None)],
            },
        ];
        for case in cases {
            let once = serialize_refinement_file(&case);
            let reparsed = parse_refinement_file(&once, "f")
                .unwrap_or_else(|err| panic!("the writer's own output must parse: {err}"));
            assert_eq!(
                reparsed.current, case.current,
                "`current` round-trips verbatim"
            );
            assert_eq!(reparsed.metadata.revision, case.metadata.revision);
            assert_eq!(reparsed.snapshots.len(), case.snapshots.len());
            assert_eq!(
                reparsed
                    .snapshots
                    .last()
                    .and_then(|s| s.proposal_agent.clone()),
                case.snapshots.last().and_then(|s| s.proposal_agent.clone()),
                "`proposalAgent` is omitted, not nulled, when absent"
            );
            let twice = serialize_refinement_file(&reparsed);
            assert_eq!(
                once, twice,
                "the serializer is a fixed point over its own parse"
            );
        }
    }

    /// [CYRUP-DELTA, stricter-than-upstream] `source: "runtime"` is a real `AgentSource` variant
    /// that `parse_refinement_file` refuses, so upstream's unconditional write would produce a
    /// file its own reader rejects — an overlay that silently never applies. The guard refuses
    /// instead, and NO file appears.
    #[tokio::test]
    async fn a_runtime_source_is_refused_before_any_file_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nested/probe.md");
        let mut parsed = ParsedRefinementFile {
            metadata: metadata("probe", 1.0),
            current: "- fine".to_string(),
            snapshots: Vec::new(),
        };
        parsed.metadata.base.source = "runtime".to_string();
        let err = write_refinement_file(&path, &parsed)
            .await
            .expect_err("must refuse");
        assert!(matches!(
            err,
            RefinementActionError::WouldNotRoundTrip { .. }
        ));
        assert!(!path.exists(), "nothing is written on the refusal path");
    }

    /// The happy write does create the parent and land the bytes, so the refusal above is failing
    /// for the reason claimed and not because the writer never worked.
    #[tokio::test]
    async fn a_valid_write_creates_the_parent_and_round_trips_from_disk() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("nested/deeper/probe.md");
        let parsed = ParsedRefinementFile {
            metadata: metadata("probe", 1.0),
            current: "- fine".to_string(),
            snapshots: vec![snapshot(1.0, Some("reviewer"))],
        };
        write_refinement_file(&path, &parsed).await.expect("writes");
        let raw = std::fs::read_to_string(&path).expect("reads back");
        let reparsed = parse_refinement_file(&raw, "f").expect("parses");
        assert_eq!(reparsed.current, "- fine");
        assert_eq!(reparsed.metadata.revision, 1.0);
    }

    /// The three verbs' wire literals live in exactly one place, and the mutating split is the one
    /// `:6359` consults.
    #[test]
    fn the_action_enum_round_trips_its_wire_verbs_and_names_the_mutators() {
        for verb in [
            RefinementAction::Refine,
            RefinementAction::Show,
            RefinementAction::Rollback,
        ] {
            assert_eq!(RefinementAction::from_wire(verb.as_str()), Some(verb));
        }
        assert_eq!(RefinementAction::from_wire("refine.nope"), None);
        assert!(RefinementAction::Refine.is_mutating());
        assert!(RefinementAction::Rollback.is_mutating());
        assert!(
            !RefinementAction::Show.is_mutating(),
            "pi's MUTATING_MANAGEMENT_ACTIONS does not carry `refine.show`, which is what keeps \
             the read verb reachable from a child-safe fanout tool"
        );
    }

    /// pi `:239`'s `v${REFINEMENT_FORMAT_VERSION}` and the parser's [`METADATA_PREFIX`] are two
    /// spellings of one thing; this is what keeps them from drifting.
    #[test]
    fn the_format_version_and_the_metadata_prefix_agree() {
        assert_eq!(
            format!("<!-- pi-subagents-refinement:v{REFINEMENT_FORMAT_VERSION}\n"),
            super::super::METADATA_PREFIX
        );
    }

    /// pi renders a JS number, so model-facing revisions have no `.0`.
    #[test]
    fn integral_values_render_without_a_decimal_point() {
        assert_eq!(integral_text(3.0), "3");
        assert_eq!(integral_text(0.0), "0");
        assert_eq!(integral_number(2_048.0).to_string(), "2048");
        assert_eq!(
            integral_number(1.5).to_string(),
            "1.5",
            "a non-integral `evidence.*` value keeps its float form, which the parser allows"
        );
    }

    /// pi `:560` — `slice(-5)` keeps the LAST five in STORED order, and `|| "- none"` fires only
    /// at zero snapshots. The pluralisation is singular at exactly one id.
    #[test]
    fn show_renders_the_last_five_snapshots_oldest_first() {
        let mut parsed = ParsedRefinementFile {
            metadata: metadata("probe", 7.0),
            current: "- guidance".to_string(),
            snapshots: (1..=7)
                .map(|revision| {
                    let mut entry = snapshot(f64::from(revision), None);
                    entry.evidence_ids = if revision == 7 {
                        vec!["live:a1".to_string()]
                    } else {
                        vec!["live:a1".to_string(), "live:a2".to_string()]
                    };
                    entry
                })
                .collect(),
        };
        let agent = probe_agent("You are probe.");
        let rendered = render_show(&agent, Path::new("/p/probe.md"), Some(&parsed)).text;
        let history: Vec<&str> = rendered
            .lines()
            .skip_while(|line| *line != "Recent history:")
            .skip(1)
            .collect();
        assert_eq!(history.len(), 5, "pi keeps five");
        assert!(history[0].starts_with("- r3 "), "oldest of the five first");
        assert!(history[4].starts_with("- r7 "));
        assert!(history[0].ends_with("(2 evidence ids)"));
        assert!(
            history[4].ends_with("(1 evidence id)"),
            "singular at exactly one"
        );

        parsed.snapshots.clear();
        let empty = render_show(&agent, Path::new("/p/probe.md"), Some(&parsed)).text;
        assert!(empty.ends_with("Recent history:\n- none"));
    }

    /// pi `:557` — the no-overlay answer is ORDINARY TEXT, not an error. `refine.rollback`'s
    /// identical sentence at `:578` IS an error, and the two must not be collapsed.
    #[test]
    fn show_with_no_overlay_is_not_an_error() {
        let agent = probe_agent("You are probe.");
        let outcome = render_show(&agent, Path::new("/p/probe.md"), None);
        assert!(!outcome.is_error);
        assert_eq!(outcome.text, "No refinement overlay exists for 'probe'.");
        assert_eq!(
            RefinementActionError::NoOverlay {
                agent: "probe".to_string()
            }
            .to_string(),
            "No refinement overlay exists for 'probe'."
        );
    }

    /// pi `:559` — the drift check compares the stored digest against a fresh hash of the RAW
    /// persona body, never of the composed prompt.
    #[test]
    fn show_reports_drift_against_the_raw_persona_body() {
        let mut agent = probe_agent("You are probe.");
        let mut parsed = ParsedRefinementFile {
            metadata: metadata("probe", 1.0),
            current: "- g".to_string(),
            snapshots: Vec::new(),
        };
        assert!(
            render_show(&agent, Path::new("/p"), Some(&parsed))
                .text
                .contains("Base prompt changed since overlay: yes"),
            "the fixture digest is `abc123`, which is not the real hash"
        );
        parsed.metadata.base.system_prompt_sha256 = hash_prompt(&agent.system_prompt_body);
        assert!(
            render_show(&agent, Path::new("/p"), Some(&parsed))
                .text
                .contains("Base prompt changed since overlay: no")
        );
        agent.system_prompt_body = "You are probe, now different.".to_string();
        assert!(
            render_show(&agent, Path::new("/p"), Some(&parsed))
                .text
                .contains("Base prompt changed since overlay: yes")
        );
    }

    /// pi `hashPrompt` is a plain lowercase-hex SHA-256; this pins it against a known vector so a
    /// hasher swap cannot go unnoticed.
    #[test]
    fn hash_prompt_is_lowercase_hex_sha256() {
        assert_eq!(
            hash_prompt(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(hash_prompt("abc").len(), 64);
    }

    /// pi `:548` — the sentence names both surfaces and interpolates the verb TWICE.
    #[test]
    fn the_missing_agent_sentence_names_both_surfaces() {
        assert_eq!(
            RefinementActionError::MissingAgent {
                action: "refine.show"
            }
            .to_string(),
            "refine.show requires agent. Use /subagents-refine <agent> or subagent({ action: \
             \"refine.show\", agent: \"<agent>\" })."
        );
    }

    /// pi `:598` — the validator's sentence with upstream's own suffix. This is the exact text an
    /// integration test pins, so it is pinned here too.
    #[test]
    fn a_refused_proposal_carries_upstreams_no_overlay_suffix() {
        let refusal = RefinementProposalRefusal::EditDisallowedGuidance {
            index: 0,
            cause: super::super::proposal::DisallowedCause::CodeFence,
        };
        assert_eq!(
            RefinementActionError::ProposalRefused { refusal }.to_string(),
            "Refinement proposal edit 0 contains disallowed guidance. No overlay was written."
        );
    }
}
