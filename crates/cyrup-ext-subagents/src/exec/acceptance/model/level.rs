//! `requiredEvidenceForLevel` and level inference: what evidence a level demands, and how an
//! `auto` request resolves to a concrete level (pi `acceptance.ts:55-302`).

use crate::exec::completion_guard::{any_word_boundary, word_boundary_contains};

use super::types::{level_rank, AcceptanceConfig, AcceptanceEvidenceKind, AcceptanceInput, AcceptanceLevel, AcceptanceReviewGate, CriterionInput, GateSeverity, ResolvedAcceptanceConfig, ResolvedAcceptanceGate, ReviewSetting};

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
            vec![ChangedFiles, TestsAdded, CommandsRun, ResidualRisks, NoStagedFiles]
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

/// `inferLevel` (acceptance.ts:77-147 @v0.43.0) — regex-free word-boundary port (the classifier reuses
/// `completion_guard`'s already-tested `word_boundary_contains`, exactly as the enum-lattice
/// `heuristic_default` reuses `expects_implementation_mutation`).
fn infer_level(input: &AcceptanceResolveInput) -> InferredLevel {
    let agent = input.agent_name.to_lowercase();
    let task = input.task.as_deref().unwrap_or("").to_lowercase();
    let mut reasons: Vec<String> = Vec::new();

    // `/\b(?:reviewer|oracle|scout|researcher|analyst)\b/` (`acceptance.ts:99` @ v0.43.0).
    //
    // Both edits to this alternation are VERSION LAG, not a port bug. At the ported baseline it
    // read `reviewer|scout|context-builder|researcher|analyst` (`acceptance.ts:80` @ v0.34.0),
    // which is exactly what this port originally carried — correctly. Upstream `83b9872`
    // ("fix: remove stale bundled roles") then dropped `context-builder` and added `oracle` in
    // the SAME edit; `git log -S` over this alternation returns that one commit and no other.
    // Both halves are applied together here for the same reason they were made together.
    let read_only_agent = any_word_boundary(
        &agent,
        &["reviewer", "oracle", "scout", "researcher", "analyst"],
    );
    // G83 — `const intent = classifyTaskMutationIntent(input.acceptanceRole ? "worker" :
    // input.agentName, input.task ?? "")` (`acceptance.ts:90`). This crate has no
    // `acceptanceRole` input, which is exactly upstream's `acceptanceRole === undefined`
    // branch, so the agent name is passed straight through.
    let intent = crate::exec::task_intent::classify_task_mutation_intent(
        &input.agent_name,
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
    // `const taskMayWrite = readOnlyTask ? false : taskMayMutate(input.task ?? "") ||
    // intent.kind === "implementation" || rolePatchTask` (`acceptance.ts:97`), with
    // `rolePatchTask === false` because no acceptance role is declared (`:93-96`).
    let task_may_write = !read_only_task
        && (crate::exec::task_intent::task_may_mutate(input.task.as_deref().unwrap_or(""))
            || intent == crate::exec::task_intent::TaskMutationIntent::Implementation);
    // `const writeTask = taskMayWrite || (input.acceptanceRole === "writer" && !readOnlyTask)
    // || (input.acceptanceRole === undefined && /\bworker\b/.test(agent) && !readOnlyTask)`
    // (`acceptance.ts:100-102`).
    let write_task =
        task_may_write || (word_boundary_contains(&agent, "worker") && !read_only_task);
    // `const keywordRiskReadOnly = input.acceptanceRole === undefined ? intent.kind ===
    // "read-only" : inferredReadOnly` (`acceptance.ts:105`).
    let keyword_risk_read_only = intent == crate::exec::task_intent::TaskMutationIntent::ReadOnly;
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
    // (!keywordRiskReadOnly && /…/.test(task))` (`acceptance.ts:106-109`);
    // `roleResolvesReadOnly` is `false` with no acceptance role declared (`:102`).
    let risky = (input.is_async && write_task)
        || input.dynamic
        || input.dynamic_group
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
        reasons.push("write-capable worker/task".to_string());
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
        reasons.push(
            if read_only_agent {
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
                "Return concrete findings with file paths and severity when applicable"
                    .to_string(),
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
        if er >= ir { explicit_level } else { inferred.level }
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

}
