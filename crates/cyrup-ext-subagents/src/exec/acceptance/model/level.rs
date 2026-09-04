//! `requiredEvidenceForLevel` and level inference: what evidence a level demands, and how an
//! `auto` request resolves to a concrete level (pi `acceptance.ts:55-302`).

use crate::exec::completion_guard::{any_word_boundary, word_boundary_contains};

use super::types::{
    AcceptanceConfig, AcceptanceEvidenceKind, AcceptanceInput, AcceptanceLevel,
    AcceptanceReviewGate, AcceptanceRole, CriterionInput, GateSeverity, ResolvedAcceptanceConfig,
    ResolvedAcceptanceGate, ReviewSetting, level_rank,
};

// --------------------------------------------------------------------------------------------
// requiredEvidenceForLevel (acceptance.ts:55-67) + level inference (acceptance.ts:69-125)
// --------------------------------------------------------------------------------------------

/// `requiredEvidenceForLevel` (acceptance.ts:55-67).
fn required_evidence_for_level(level: AcceptanceLevel) -> Vec<AcceptanceEvidenceKind> {
    use AcceptanceEvidenceKind::*;
    match level {
        AcceptanceLevel::None | AcceptanceLevel::Auto => Vec::new(),
        AcceptanceLevel::Attested => vec![ManualNotes, ResidualRisks],
        AcceptanceLevel::Checked => {
            vec![
                ChangedFiles,
                TestsAdded,
                CommandsRun,
                ResidualRisks,
                NoStagedFiles,
            ]
        }
        AcceptanceLevel::Verified => vec![
            ChangedFiles,
            TestsAdded,
            CommandsRun,
            ValidationOutput,
            ResidualRisks,
            NoStagedFiles,
        ],
    }
}

/// `SubagentRunMode` (shared/types.ts:231) — carried for parity with pi's `inferLevel` input even
/// though the current heuristic does not branch on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentRunMode {
    Single,
    Parallel,
    Chain,
}

/// Input to [`resolve_effective_acceptance`] / `infer_level` (acceptance.ts:69-76, 265-273).
#[derive(Debug, Clone, Default)]
pub struct AcceptanceResolveInput {
    pub explicit: Option<AcceptanceInput>,
    pub agent_name: String,
    /// SUBA-082 — pi `acceptanceRole?: AcceptanceRole` (`acceptance.ts:79` @v0.57.0, `:81`
    /// @v0.64.0): the agent's DECLARED role. `None` is upstream's `undefined`, the branch on
    /// which the agent-name alternations (`reviewer|oracle|…`, `worker`) are consulted at all.
    pub acceptance_role: Option<AcceptanceRole>,
    pub task: Option<String>,
    pub mode: Option<SubagentRunMode>,
    pub is_async: bool,
    pub dynamic: bool,
    pub dynamic_group: bool,
}

struct InferredLevel {
    level: AcceptanceLevel,
    reasons: Vec<String>,
    criteria: Vec<CriterionInput>,
    evidence: Vec<AcceptanceEvidenceKind>,
    review: Option<ReviewSetting>,
}

/// `\b(?:do not|don't|must not)\s+patch\b` (`acceptance.ts:95` @v0.57.0) over the lowercased
/// task — the negative guard on `rolePatchTask`.
fn forbids_patch(task_lower: &str) -> bool {
    let bytes = task_lower.as_bytes();
    let mut i = 0usize;
    while i <= bytes.len() {
        if !task_lower.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if boundary_before(task_lower, i)
            && let Some(after_phrase) = ["do not", "don't", "must not"].iter().find_map(|phrase| {
                task_lower
                    .get(i..)
                    .filter(|rest| rest.starts_with(phrase))
                    .map(|_| i + phrase.len())
            })
            && let Some(after_ws) = skip_ws1(task_lower, after_phrase)
            && task_lower
                .get(after_ws..)
                .is_some_and(|rest| rest.starts_with("patch"))
            && boundary_after(task_lower, after_ws + "patch".len())
        {
            return true;
        }
        i += 1;
    }
    false
}

/// `\bpatch\s+(?:(?:\.{0,2}[\\/])?(?:[\w.-]+[\\/])+[\w.-]+|[\w.-]+\.[a-z0-9]+\b|(?:the\s+)?parser\b)`
/// (`acceptance.ts:96` @v0.57.0, `:98` @v0.64.0) over the lowercased, severity-compound-stripped
/// task: `patch` followed by a PATH (`src/auth.ts`, `./x/y`, `/etc/hosts`), a FILENAME with an
/// extension (`auth.ts`), or `(the) parser`. This is the one place a bare `patch <object>` reads
/// as mutation intent — `classifyTaskMutationIntent` deliberately requires a recognizable object
/// noun and classifies `Patch src/auth.ts` as `unknown` — and upstream enables it ONLY when a
/// role is declared (`rolePatchTask`'s `input.acceptanceRole !== undefined` guard).
fn has_role_patch_target(text: &str) -> bool {
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i <= bytes.len() {
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        if boundary_before(text, i)
            && text.get(i..).is_some_and(|rest| rest.starts_with("patch"))
            && let Some(after_ws) = skip_ws1(text, i + "patch".len())
            && text.get(after_ws..).is_some_and(role_patch_object_at)
        {
            return true;
        }
        i += 1;
    }
    false
}

/// The three-way object alternation of [`has_role_patch_target`], tested at the start of `rest`.
fn role_patch_object_at(rest: &str) -> bool {
    // `(?:\.{0,2}[\\/])?(?:[\w.-]+[\\/])+[\w.-]+` — the dotted prefixes (`./`, `../`) are
    // consumable by the segment group itself, so only a bare leading slash needs stripping.
    let path_body = rest.strip_prefix(['/', '\\']).unwrap_or(rest);
    let seg1 = path_body
        .char_indices()
        .find(|(_, c)| !is_path_segment_char(*c))
        .map_or(path_body.len(), |(idx, _)| idx);
    if seg1 > 0
        && let Some(after_slash) = path_body
            .get(seg1..)
            .and_then(|tail| tail.strip_prefix(['/', '\\']))
        && after_slash.chars().next().is_some_and(is_path_segment_char)
    {
        return true;
    }
    // `[\w.-]+\.[a-z0-9]+\b` — a `[\w.-]` run containing a dot followed by an `[a-z0-9]+` run
    // that ends at a word boundary. Every dot in the run is a candidate split (backtracking).
    let run_end = rest
        .char_indices()
        .find(|(_, c)| !is_path_segment_char(*c))
        .map_or(rest.len(), |(idx, _)| idx);
    let run = rest.get(..run_end).unwrap_or("");
    for (dot_idx, _) in run.char_indices().filter(|(idx, c)| *c == '.' && *idx > 0) {
        let ext_start = dot_idx + 1;
        let ext_end = run
            .get(ext_start..)
            .map(|tail| {
                tail.char_indices()
                    .find(|(_, c)| !(c.is_ascii_lowercase() || c.is_ascii_digit()))
                    .map_or(run.len(), |(idx, _)| ext_start + idx)
            })
            .unwrap_or(ext_start);
        if ext_end > ext_start && boundary_after(run, ext_end) {
            return true;
        }
    }
    // `(?:the\s+)?parser\b`
    let mut cursor = 0usize;
    if rest.starts_with("the")
        && let Some(after_ws) = skip_ws1(rest, "the".len())
    {
        cursor = after_ws;
    }
    rest.get(cursor..)
        .is_some_and(|tail| tail.starts_with("parser"))
        && boundary_after(rest, cursor + "parser".len())
}

/// `[\w.-]` — JavaScript `\w` (ASCII alphanumerics and `_`) plus `.` and `-`.
fn is_path_segment_char(ch: char) -> bool {
    crate::exec::completion_guard::is_word_char(ch) || ch == '.' || ch == '-'
}

/// `\s+` — the end offset after one-or-more whitespace characters at `i`, else `None`.
fn skip_ws1(text: &str, i: usize) -> Option<usize> {
    let mut end = i;
    for ch in text.get(i..)?.chars() {
        if ch.is_whitespace() {
            end += ch.len_utf8();
        } else {
            break;
        }
    }
    (end > i).then_some(end)
}

/// `\b` immediately before byte offset `i`.
fn boundary_before(text: &str, i: usize) -> bool {
    text.get(..i)
        .and_then(|s| s.chars().next_back())
        .is_none_or(|c| !crate::exec::completion_guard::is_word_char(c))
}

/// `\b` immediately after byte offset `i`, for a pattern whose previous character is a word
/// character.
fn boundary_after(text: &str, i: usize) -> bool {
    text.get(i..)
        .and_then(|s| s.chars().next())
        .is_none_or(|c| !crate::exec::completion_guard::is_word_char(c))
}

/// `inferLevel` (`acceptance.ts:77-147` @v0.43.0; the role-aware prologue at `:77-104` @v0.57.0,
/// `:79-112` @v0.64.0) — regex-free word-boundary port (the classifier reuses
/// `completion_guard`'s already-tested `word_boundary_contains`, exactly as the enum-lattice
/// `heuristic_default` reuses `expects_implementation_mutation`).
///
/// SUBA-082: `input.acceptance_role` is pi's `acceptanceRole`, the PRIMARY classification input
/// (upstream `3c635cc1`, "feat: add per-agent acceptance roles (#481)", in v0.35.0). Every
/// `acceptanceRole === undefined` guard below is the branch this port carried alone before the
/// role landed; each is now spelled out against `role.is_none()` so the two branches read
/// side by side with upstream.
///
/// The body is the one shared by v0.57.0 (the row's tag) and v0.62.0. v0.63.0's `0128385f`
/// ("fix: omit inferred acceptance for read-only reviewers (#1799)") then changed three lines
/// in the SAME function — `readOnlyAgent` feeds `inferredReadOnly` (`:105` @v0.64.0), a new
/// `dynamicResolvesReadOnly` guard on the `dynamic`/`dynamicGroup` escalations (`:107,110-111`),
/// and the read-only branch's level flips from `attested` to `none` (`:137`). That commit is a
/// separate drift from the acceptance-role row (it rewrites what a NAME-classified reviewer
/// gets, role or no role, and interacts with this crate's always-attest lattice mapping in
/// `AcceptanceContract::heuristic_default`) and is deliberately NOT folded in here.
fn infer_level(input: &AcceptanceResolveInput) -> InferredLevel {
    let agent = input.agent_name.to_lowercase();
    let task = input.task.as_deref().unwrap_or("").to_lowercase();
    let role = input.acceptance_role;
    let mut reasons: Vec<String> = Vec::new();

    // `const intent = classifyTaskMutationIntent(input.acceptanceRole ? "worker" :
    // input.agentName, input.task ?? "")` (`acceptance.ts:90` @v0.57.0). Upstream's own comment:
    // "Declared roles replace name heuristics, so use the full writer grammar to detect explicit
    // mutation independently of the actual agent name." With no role declared the agent name is
    // passed straight through, as before.
    let intent = crate::exec::task_intent::classify_task_mutation_intent(
        if role.is_some() {
            "worker"
        } else {
            &input.agent_name
        },
        input.task.as_deref().unwrap_or(""),
    );
    // `const readOnlyTask = intent.kind === "read-only" || (intent.kind === "unknown" &&
    // /\b(?:read[- ]only|review[- ]only|no edits|without edits|inspect|summari[sz]e)\b/.test(task))`
    // (`acceptance.ts:91-92`). The keyword probe is a FALLBACK for `unknown` only, and its
    // `do not edit`/`don't edit` entries moved into the classifier — a bare keyword scan
    // cannot tell `Do not edit files.` (blanket, read-only) from `Do not edit unrelated files;
    // implement the fix.` (scoped constraint on an implementation task), and used to call both
    // read-only.
    let read_only_task = intent == crate::exec::task_intent::TaskMutationIntent::ReadOnly
        || (intent == crate::exec::task_intent::TaskMutationIntent::Unknown
            && any_word_boundary(
                &task,
                &[
                    "read only",
                    "read-only",
                    "review only",
                    "review-only",
                    "no edits",
                    "without edits",
                    "inspect",
                    "summarise",
                    "summarize",
                ],
            ));
    // `const rolePatchTask = input.acceptanceRole !== undefined && intent.kind !== "read-only"
    // && !/\b(?:do not|don't|must not)\s+patch\b/.test(task) && /\bpatch\s+(…)/.test(
    // stripSeverityCompounds(task))` (`acceptance.ts:93-96` @v0.57.0): with a role declared,
    // `Patch src/auth.ts` counts as mutation intent even though the classifier calls it `unknown`.
    let role_patch_task = role.is_some()
        && intent != crate::exec::task_intent::TaskMutationIntent::ReadOnly
        && !forbids_patch(&task)
        && has_role_patch_target(&crate::exec::task_intent::strip_severity_compounds(&task));
    // `const taskMayWrite = readOnlyTask ? false : taskMayMutate(input.task ?? "") ||
    // intent.kind === "implementation" || rolePatchTask` (`acceptance.ts:97`).
    let task_may_write = !read_only_task
        && (crate::exec::task_intent::task_may_mutate(input.task.as_deref().unwrap_or(""))
            || intent == crate::exec::task_intent::TaskMutationIntent::Implementation
            || role_patch_task);
    // `const readOnlyAgent = input.acceptanceRole === "read-only" || (input.acceptanceRole ===
    // undefined && /\b(?:reviewer|oracle|scout|researcher|analyst)\b/.test(agent))`
    // (`acceptance.ts:98-99` @v0.57.0).
    //
    // Both edits to the alternation are VERSION LAG, not a port bug. At the ported baseline it
    // read `reviewer|scout|context-builder|researcher|analyst` (`acceptance.ts:80` @ v0.34.0),
    // which is exactly what this port originally carried — correctly. Upstream `83b9872`
    // ("fix: remove stale bundled roles") then dropped `context-builder` and added `oracle` in
    // the SAME edit; `git log -S` over this alternation returns that one commit and no other.
    // Both halves are applied together here for the same reason they were made together.
    let read_only_agent = role == Some(AcceptanceRole::ReadOnly)
        || (role.is_none()
            && any_word_boundary(
                &agent,
                &["reviewer", "oracle", "scout", "researcher", "analyst"],
            ));
    // `const writeTask = taskMayWrite || (input.acceptanceRole === "writer" && !readOnlyTask)
    // || (input.acceptanceRole === undefined && /\bworker\b/.test(agent) && !readOnlyTask)`
    // (`acceptance.ts:100-102`).
    let write_task = task_may_write
        || (role == Some(AcceptanceRole::Writer) && !read_only_task)
        || (role.is_none() && word_boundary_contains(&agent, "worker") && !read_only_task);
    // `const inferredReadOnly = readOnlyTask || (input.acceptanceRole === "read-only" &&
    // !taskMayWrite)` (`acceptance.ts:103` @v0.57.0; v0.63.0's `#1799` widens this to
    // `(readOnlyAgent || role === "read-only")` — not taken here, see the function doc).
    let inferred_read_only =
        read_only_task || (role == Some(AcceptanceRole::ReadOnly) && !task_may_write);
    // `const roleResolvesReadOnly = input.acceptanceRole !== undefined && inferredReadOnly`
    // (`acceptance.ts:104`): a DECLARED role that resolves read-only cancels the
    // `dynamic`/`dynamicGroup` escalations below.
    let role_resolves_read_only = role.is_some() && inferred_read_only;
    // `const keywordRiskReadOnly = input.acceptanceRole === undefined ? intent.kind ===
    // "read-only" : inferredReadOnly` (`acceptance.ts:105`).
    let keyword_risk_read_only = if role.is_none() {
        intent == crate::exec::task_intent::TaskMutationIntent::ReadOnly
    } else {
        inferred_read_only
    };
    // /\b(?:release|migration|migrate|security|data[- ]loss|destructive|post-review|fix pass)\b/
    let risky_task = any_word_boundary(
        &task,
        &[
            "release",
            "migration",
            "migrate",
            "security",
            "data loss",
            "data-loss",
            "destructive",
            "post-review",
            "fix pass",
        ],
    );
    // `const risky = Boolean(input.async && writeTask) || (Boolean(input.dynamic) &&
    // !roleResolvesReadOnly) || (Boolean(input.dynamicGroup) && !roleResolvesReadOnly) ||
    // (!keywordRiskReadOnly && /…/.test(task))` (`acceptance.ts:106-109` @v0.57.0).
    let risky = (input.is_async && write_task)
        || (input.dynamic && !role_resolves_read_only)
        || (input.dynamic_group && !role_resolves_read_only)
        || (!keyword_risk_read_only && risky_task);

    if risky {
        reasons.push(
            if input.is_async {
                "async write-capable or risky run"
            } else {
                "risky write-capable run"
            }
            .to_string(),
        );
        if input.dynamic || input.dynamic_group {
            reasons.push("dynamic fanout context".to_string());
        }
        // `acceptance.ts:114-120` @v0.43.0 — the risky branch returns `level: "checked"` plus a
        // REQUIRED review gate. Up to v0.34.0 it returned `level: "reviewed"`; v0.43.0 deleted
        // that level entirely (see [`AcceptanceLevel`]), so the "an independent reviewer must
        // sign this off" half of the escalation now lives ONLY in `review`, never in `level`.
        return InferredLevel {
            level: AcceptanceLevel::Checked,
            reasons,
            criteria: vec![
                CriterionInput::Text(
                    "Implement the requested change without widening scope".to_string(),
                ),
                CriterionInput::Text(
                    "Return evidence sufficient for an independent acceptance review".to_string(),
                ),
            ],
            evidence: required_evidence_for_level(AcceptanceLevel::Checked),
            review: Some(ReviewSetting::Gate(AcceptanceReviewGate {
                agent: Some("reviewer".to_string()),
                focus: Option::None,
                required: Some(true),
            })),
        };
    }
    if write_task && !read_only_task {
        // `input.acceptanceRole === "writer" && !taskMayWrite ? "declared writer acceptance
        // role" : "write-capable worker/task"` (`acceptance.ts:124` @v0.57.0, `:126` @v0.64.0).
        reasons.push(
            if role == Some(AcceptanceRole::Writer) && !task_may_write {
                "declared writer acceptance role"
            } else {
                "write-capable worker/task"
            }
            .to_string(),
        );
        return InferredLevel {
            level: AcceptanceLevel::Checked,
            reasons,
            criteria: vec![CriterionInput::Text(
                "Implement the requested change without widening scope".to_string(),
            )],
            evidence: required_evidence_for_level(AcceptanceLevel::Checked),
            review: Option::None,
        };
    }
    if read_only_agent || read_only_task {
        // `input.acceptanceRole === "read-only" && !readOnlyTask ? "declared read-only
        // acceptance role" : readOnlyAgent ? "read-only/reviewer-style agent" : "read-only task
        // wording"` (`acceptance.ts:133` @v0.57.0, `:135` @v0.64.0).
        reasons.push(
            if role == Some(AcceptanceRole::ReadOnly) && !read_only_task {
                "declared read-only acceptance role"
            } else if read_only_agent {
                "read-only/reviewer-style agent"
            } else {
                "read-only task wording"
            }
            .to_string(),
        );
        return InferredLevel {
            level: AcceptanceLevel::Attested,
            reasons,
            criteria: vec![CriterionInput::Text(
                "Return concrete findings with file paths and severity when applicable".to_string(),
            )],
            evidence: vec![
                AcceptanceEvidenceKind::ReviewFindings,
                AcceptanceEvidenceKind::ResidualRisks,
            ],
            review: Option::None,
        };
    }
    reasons.push("default lightweight attestation".to_string());
    InferredLevel {
        level: AcceptanceLevel::Attested,
        reasons,
        criteria: vec![CriterionInput::Text(
            "Return a concise result and residual risks when applicable".to_string(),
        )],
        evidence: vec![
            AcceptanceEvidenceKind::ManualNotes,
            AcceptanceEvidenceKind::ResidualRisks,
        ],
        review: Option::None,
    }
}

// --------------------------------------------------------------------------------------------
// normalizeAcceptanceInput / resolveEffectiveAcceptance (acceptance.ts:127-302)
// --------------------------------------------------------------------------------------------

/// `normalizeAcceptanceInput` (acceptance.ts:149-154).
#[must_use]
pub fn normalize_acceptance_input(input: Option<&AcceptanceInput>) -> AcceptanceConfig {
    match input {
        Option::None | Some(AcceptanceInput::Level(AcceptanceLevel::Auto)) => AcceptanceConfig {
            level: Some(AcceptanceLevel::Auto),
            ..AcceptanceConfig::default()
        },
        Some(AcceptanceInput::Disabled) => AcceptanceConfig {
            level: Some(AcceptanceLevel::None),
            reason: Some("disabled by deprecated false shorthand".to_string()),
            ..AcceptanceConfig::default()
        },
        Some(AcceptanceInput::Level(level)) => AcceptanceConfig {
            level: Some(*level),
            ..AcceptanceConfig::default()
        },
        Some(AcceptanceInput::Config(config)) => config.clone(),
    }
}

/// `explicitAcceptanceCanDisable` (acceptance.ts:167-174).
fn explicit_acceptance_can_disable(explicit: &AcceptanceConfig) -> bool {
    explicit.level == Some(AcceptanceLevel::None)
        && explicit
            .reason
            .as_deref()
            .is_some_and(|reason| !reason.trim().is_empty())
}

/// `normalizeCriteria` (acceptance.ts:330-342).
///
/// Public because [`crate::exec::acceptance::lower_acceptance_input`] resolves an authored `criteria[]` through
/// this exact function on its way onto [`crate::exec::acceptance::AcceptanceContract::criteria`] — the ONE
/// normalization rule (id fallback `criterion-<n>`, evidence inheritance, blank-`must` drop)
/// must not be re-implemented on the live path.
#[must_use]
pub fn normalize_criteria(
    criteria: &[CriterionInput],
    evidence: &[AcceptanceEvidenceKind],
) -> Vec<ResolvedAcceptanceGate> {
    criteria
        .iter()
        .enumerate()
        .map(|(index, criterion)| match criterion {
            CriterionInput::Text(must) => ResolvedAcceptanceGate {
                id: format!("criterion-{}", index + 1),
                must: must.clone(),
                evidence: evidence.to_vec(),
                severity: GateSeverity::Required,
            },
            CriterionInput::Gate(gate) => ResolvedAcceptanceGate {
                id: gate
                    .id
                    .as_deref()
                    .map(str::trim)
                    .filter(|id| !id.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("criterion-{}", index + 1)),
                must: gate.must.clone().unwrap_or_default(),
                evidence: gate.evidence.clone().unwrap_or_else(|| evidence.to_vec()),
                severity: gate.severity.unwrap_or(GateSeverity::Required),
            },
        })
        .filter(|criterion| !criterion.must.trim().is_empty())
        .collect()
}

/// Order-preserving de-duplication of an evidence list (`[...new Set(...)]`,
/// acceptance.ts:283-285). Shared with [`crate::exec::acceptance::lower_acceptance_input`] so a policy declaring
/// the same kind twice produces one prompt line and one runtime check, not two.
#[must_use]
pub fn unique_evidence(items: &[AcceptanceEvidenceKind]) -> Vec<AcceptanceEvidenceKind> {
    let mut seen: Vec<AcceptanceEvidenceKind> = Vec::new();
    for item in items {
        if !seen.contains(item) {
            seen.push(*item);
        }
    }
    seen
}

/// `resolveEffectiveAcceptance` (acceptance.ts:344-401) — including the explicit-vs-inferred MAX
/// escalation and the "inference-escalated-to-reviewed" review-downgrade rule.
#[must_use]
pub fn resolve_effective_acceptance(input: &AcceptanceResolveInput) -> ResolvedAcceptanceConfig {
    let explicit = normalize_acceptance_input(input.explicit.as_ref());
    let inferred = infer_level(input);
    let explicit_level = explicit.level.unwrap_or(AcceptanceLevel::Auto);

    let level = if explicit_acceptance_can_disable(&explicit) {
        AcceptanceLevel::None
    } else if explicit_level == AcceptanceLevel::Auto {
        inferred.level
    } else {
        // MAX(explicit, inferred) by rank.
        let er = level_rank(explicit_level).unwrap_or(0);
        let ir = level_rank(inferred.level).unwrap_or(0);
        if er >= ir {
            explicit_level
        } else {
            inferred.level
        }
    };

    let base_evidence = if level == inferred.level {
        inferred.evidence.clone()
    } else {
        required_evidence_for_level(level)
    };
    let mut combined = base_evidence;
    if let Some(extra) = &explicit.evidence {
        combined.extend(extra.iter().copied());
    }
    let evidence = unique_evidence(&combined);

    let criteria_source: Vec<CriterionInput> = match &explicit.criteria {
        Some(criteria) if !criteria.is_empty() => criteria.clone(),
        _ => inferred.criteria.clone(),
    };
    let criteria = normalize_criteria(&criteria_source, &evidence);

    // `acceptance.ts:389` @v0.43.0: `explicit.review !== undefined ? explicit.review :
    // inferred.review` — and nothing more. v0.34.0 additionally downgraded an inference-
    // escalated `reviewed` gate to `required: false` (`acceptance.ts:288-290` @v0.34.0); that
    // rule existed only because inference could escalate the LEVEL to `reviewed`, which
    // v0.43.0 removed (see [`AcceptanceLevel`]), so the downgrade went with it.
    let review = if explicit.review.is_some() {
        explicit.review.clone()
    } else {
        inferred.review.clone()
    };

    ResolvedAcceptanceConfig {
        level,
        explicit: input.explicit.is_some(),
        inferred_reason: inferred.reasons,
        criteria,
        evidence,
        verify: explicit.verify.clone().unwrap_or_default(),
        review,
        stop_rules: explicit.stop_rules.clone().unwrap_or_default(),
        reason: explicit.reason.clone(),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;
    use crate::exec::acceptance::model::testsupport::resolve;

    // ---- inferLevel / resolveEffectiveAcceptance ----

    #[test]
    fn infers_policies_for_reviewer_writer_async_and_dynamic() {
        assert_eq!(
            resolve(AcceptanceResolveInput {
                agent_name: "reviewer".into(),
                task: Some("Review-only. Do not edit.".into()),
                mode: Some(SubagentRunMode::Single),
                ..Default::default()
            })
            .level,
            AcceptanceLevel::Attested
        );
        assert_eq!(
            resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                task: Some("Implement the fix".into()),
                mode: Some(SubagentRunMode::Single),
                ..Default::default()
            })
            .level,
            AcceptanceLevel::Checked
        );
        // `acceptance.ts:111-121` @v0.43.0 — the risky branch resolves to `checked` (v0.34.0
        // said `reviewed`, a level that no longer exists) and expresses "an independent
        // reviewer must sign this off" through the REQUIRED review gate instead. Both halves
        // are asserted, so a regression that drops the gate cannot hide behind the level.
        let async_write = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Implement the fix".into()),
            is_async: true,
            ..Default::default()
        });
        assert_eq!(async_write.level, AcceptanceLevel::Checked);
        assert_eq!(
            async_write.review,
            Some(ReviewSetting::Gate(AcceptanceReviewGate {
                agent: Some("reviewer".into()),
                focus: None,
                required: Some(true),
            }))
        );
        assert_eq!(
            async_write.evidence,
            required_evidence_for_level(AcceptanceLevel::Checked)
        );
        let dynamic = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            task: Some("Fix each item".into()),
            mode: Some(SubagentRunMode::Chain),
            dynamic: true,
            ..Default::default()
        });
        assert_eq!(dynamic.level, AcceptanceLevel::Checked);
        assert_eq!(
            dynamic.review,
            Some(ReviewSetting::Gate(AcceptanceReviewGate {
                agent: Some("reviewer".into()),
                focus: None,
                required: Some(true),
            }))
        );
    }

    // ---- SUBA-082: the regex-free `rolePatchTask` probes (`acceptance.ts:93-96` @v0.57.0) ----

    /// `/\bpatch\s+(?:(?:\.{0,2}[\\/])?(?:[\w.-]+[\\/])+[\w.-]+|[\w.-]+\.[a-z0-9]+\b|(?:the\s+)?parser\b)/`
    /// over lowercased text — each alternative, plus the objects it must NOT accept.
    #[test]
    fn role_patch_target_matches_paths_filenames_and_the_parser_only() {
        for text in [
            "patch src/auth.ts",
            "please patch ./x/y",
            "patch ../lib/mod.rs and report",
            "patch /etc/hosts",
            "patch a/b",
            "patch auth.ts",
            "patch src.auth.ts",
            "patch the parser",
            "patch parser",
            "patch  the   parser",
        ] {
            assert!(has_role_patch_target(text), "{text:?} must match");
        }
        for text in [
            "patch it",
            "patch x",
            "patch /x",
            "patch x_y.ts_z",
            "patch",
            "dispatch src/auth.ts",
            "patch parsers",
            "patch the parsers",
            "patch .ts",
        ] {
            assert!(!has_role_patch_target(text), "{text:?} must not match");
        }
    }

    /// `/\b(?:do not|don't|must not)\s+patch\b/` — the negative guard.
    #[test]
    fn forbids_patch_matches_the_three_prohibition_phrasings() {
        assert!(forbids_patch("do not patch src/auth.ts"));
        assert!(forbids_patch("review; don't patch anything"));
        assert!(forbids_patch("you must not  patch the parser"));
        assert!(!forbids_patch("do not patchwork"));
        assert!(!forbids_patch("undo not patch"));
        assert!(!forbids_patch("patch the parser"));
    }

    /// The severity-compound strip runs BEFORE the patch probe (`stripSeverityCompounds(task)`):
    /// a "must-patch list" is an adjective, not an instruction to patch a list.
    #[test]
    fn role_patch_task_ignores_severity_compounds_but_not_real_patch_objects() {
        let resolved = |task: &str| {
            resolve(AcceptanceResolveInput {
                agent_name: "explorer".into(),
                acceptance_role: Some(AcceptanceRole::ReadOnly),
                task: Some(task.into()),
                ..Default::default()
            })
        };
        assert_ne!(
            resolved("Triage the must-patch src/auth.ts list").level,
            AcceptanceLevel::Checked,
            "`must-patch` is stripped before the probe sees `patch src/auth.ts`"
        );
        assert_eq!(
            resolved("Patch src/auth.ts").level,
            AcceptanceLevel::Checked
        );
        assert_ne!(
            resolved("Do not patch src/auth.ts; explain the flow").level,
            AcceptanceLevel::Checked,
            "the prohibition guard wins"
        );
    }
}
