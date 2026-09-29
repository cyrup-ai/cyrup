//! `requiredEvidenceForLevel` and level inference: what evidence a level demands, and how an
//! `auto` request resolves to a concrete level (pi `acceptance.ts:55-302`).

use super::types::{
    AcceptanceConfig, AcceptanceEvidenceKind, AcceptanceInput, AcceptanceLevel,
    AcceptanceReviewGate, AcceptanceRole, CriterionInput, GateSeverity, ResolvedAcceptanceConfig,
    ResolvedAcceptanceGate, ReviewSetting, level_rank,
};

// --------------------------------------------------------------------------------------------
// requiredEvidenceForLevel (acceptance.ts:55-67) + level inference (acceptance.ts:69-125)
// --------------------------------------------------------------------------------------------

/// `requiredEvidenceForLevel` (acceptance.ts:55-67).
#[must_use]
pub fn required_evidence_for_level(level: AcceptanceLevel) -> Vec<AcceptanceEvidenceKind> {
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
    /// SUBA-082 — pi `acceptanceRole?: AcceptanceRole` (`acceptance.ts:81` @v0.71.0): the
    /// agent's DECLARED role, and after SUBA-108 the ONLY input [`infer_level`] reads. `None` is
    /// upstream's `undefined` and now takes the final `attested` default (`:118-122`); the
    /// agent-name alternations (`reviewer|oracle|…`, `worker`) that used to key off it were
    /// deleted upstream by `7c98a696` (v0.70.1) and are gone from this port too.
    pub acceptance_role: Option<AcceptanceRole>,
    pub task: Option<String>,
    pub mode: Option<SubagentRunMode>,
    pub is_async: bool,
    pub dynamic: bool,
    pub dynamic_group: bool,
}

/// The return of `inferLevel` (`acceptance.ts:88`), BEFORE `resolveEffectiveAcceptance` merges it
/// with any explicit policy and before `:521-522` clears a `none` level's criteria and evidence.
///
/// SUBA-108 — public because `resolveEffectiveAcceptance` is not the only consumer upstream has:
/// `acceptance.ts:504` binds `inferLevel(input)` and reads its `criteria`/`evidence`/`review` at
/// `:512-517` for the merge. cyrup's lattice
/// ([`crate::exec::acceptance::AcceptanceContract::resolve_effective_for_role`]) performs that same
/// merge against a contract lowered from wire JSON, so it needs the same raw value rather than the
/// already-resolved-and-possibly-cleared [`ResolvedAcceptanceConfig`].
#[derive(Debug, Clone, PartialEq)]
pub struct InferredLevel {
    pub level: AcceptanceLevel,
    pub reasons: Vec<String>,
    pub criteria: Vec<CriterionInput>,
    pub evidence: Vec<AcceptanceEvidenceKind>,
    pub review: Option<ReviewSetting>,
}

/// `inferLevel` (`acceptance.ts:81-122` @v0.71.0).
///
/// SUBA-108 — upstream `7c98a696` ("refactor: remove inferred no-edit completion failures",
/// #2356, v0.70.1) deleted every wording- and name-derived heuristic from this function. At
/// v0.71.0 it reads **only** `input.acceptanceRole`: there is no agent-name alternation, no
/// risky-keyword pattern, no `classifyTaskMutationIntent`, no `taskMayMutate` and no
/// `stripSeverityCompounds` anywhere in `acceptance.ts`. `input.task`, `input.agentName` and
/// `input.mode` remain in the signature and are NEVER READ; this port keeps them on
/// [`AcceptanceResolveInput`] for the same reason, so the two signatures stay aligned.
///
/// Consequence, and upstream's stated intent (CHANGELOG 0.70.1): *"Custom implementation agents
/// must declare `acceptanceRole: writer` to receive writer acceptance defaults."* An agent with
/// no declared role gets the lightweight attestation default regardless of its name or its task
/// wording. cyrup's bundled `resources/agents/worker.md` therefore declares
/// `acceptanceRole: writer`, matching `agents/worker.md` @v0.71.0 (added by the same commit).
///
/// The `read-only` branch returns `none`, i.e. no acceptance prompt at all
/// (`formatAcceptancePrompt` returns `""` for `level === "none"`) and no gate.
#[must_use]
pub fn infer_level(input: &AcceptanceResolveInput) -> InferredLevel {
    // `if (input.acceptanceRole === "writer" && (input.async || input.dynamic ||
    // input.dynamicGroup))` (`acceptance.ts:88-100`).
    if input.acceptance_role == Some(AcceptanceRole::Writer)
        && (input.is_async || input.dynamic || input.dynamic_group)
    {
        let mut reasons = vec![
            if input.is_async {
                "async declared writer"
            } else {
                "dynamic declared writer"
            }
            .to_string(),
        ];
        if input.dynamic || input.dynamic_group {
            reasons.push("dynamic fanout context".to_string());
        }
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
    // `if (input.acceptanceRole === "writer")` (`acceptance.ts:101-108`).
    if input.acceptance_role == Some(AcceptanceRole::Writer) {
        return InferredLevel {
            level: AcceptanceLevel::Checked,
            reasons: vec!["declared writer acceptance role".to_string()],
            criteria: vec![CriterionInput::Text(
                "Implement the requested change without widening scope".to_string(),
            )],
            evidence: required_evidence_for_level(AcceptanceLevel::Checked),
            review: Option::None,
        };
    }
    // `if (input.acceptanceRole === "read-only")` (`acceptance.ts:109-117`).
    if input.acceptance_role == Some(AcceptanceRole::ReadOnly) {
        return InferredLevel {
            level: AcceptanceLevel::None,
            reasons: vec!["declared read-only acceptance role".to_string()],
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
    // `acceptance.ts:118-122`.
    InferredLevel {
        level: AcceptanceLevel::Attested,
        reasons: vec!["default lightweight attestation".to_string()],
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

/// `explicitAcceptanceRequestsPolicy` (`acceptance.ts:243-245` @v0.71.0):
/// `(explicit.level !== undefined && explicit.level !== "auto") || Object.keys(explicit).some((key) => key !== "level")`.
///
/// SUBA-108 — this predicate is what keeps a CALLER-SUPPLIED policy from being thrown away when
/// the agent's declared role infers `none`. Upstream consults it at `acceptance.ts:505` and
/// upgrades the inferred level to `attested`, so criteria/evidence/verify/review the caller asked
/// for are still resolved and still enforced. It became reachable in cyrup the moment
/// [`infer_level`]'s `read-only` branch started returning [`AcceptanceLevel::None`]; before that
/// no inference produced `none` at all.
///
/// CYRUP-DELTA (mechanism) — upstream tests key PRESENCE with `Object.keys`; the Rust side tests
/// `Option::is_some` on the modeled non-`level` fields of [`AcceptanceConfig`], which is how a
/// key's presence is already represented after normalization. The one input TS and Rust could
/// disagree on is an explicit JSON `null` (`{"reason": null}`: a key upstream, `None` here), and
/// that value is rejected earlier by acceptance-input validation, so it cannot reach this
/// function.
///
/// This delta does NOT claim parity on the INPUT SET, and must not be read as doing so. Upstream's
/// `AcceptanceConfig` (`shared/types.ts:1063-1074` @v0.71.0) has NINE keys:
/// `level, report, preserveStagedIndex, criteria, evidence, verify, review, stopRules, reason`.
/// SUBA-108 added `report`, which this predicate now reads. `preserveStagedIndex` is still
/// MISSING: `ACCEPTANCE_CONFIG_KEYS` (`validate_input.rs:71-80`) omits it, so cyrup REFUSES a
/// policy upstream accepts and carries onto its result (`acceptance.ts:497,519`), and this
/// predicate can never see it. That is a separate, pre-existing behavioural divergence with its
/// own fix — named here rather than papered over, and deliberately NOT recorded as a CYRUP-DELTA,
/// because a refusal upstream does not make is a behavioural difference, not a mechanism one.
/// (`ACCEPTANCE_VERIFY_KEYS` is likewise short of upstream's `output`/`schema` at `:56`.)
fn explicit_acceptance_requests_policy(explicit: &AcceptanceConfig) -> bool {
    let level_requests = matches!(explicit.level, Some(level) if level != AcceptanceLevel::Auto);
    let non_level_key_present = explicit.report.is_some()
        || explicit.criteria.is_some()
        || explicit.evidence.is_some()
        || explicit.verify.is_some()
        || explicit.review.is_some()
        || explicit.stop_rules.is_some()
        || explicit.reason.is_some();
    level_requests || non_level_key_present
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

    // `acceptance.ts:505` @v0.71.0: `const inferredLevel = inferred.level === "none" &&
    // explicitAcceptanceRequestsPolicy(explicit) ? "attested" : inferred.level;`
    //
    // SUBA-108 — a declared `read-only` role infers `none`, and `none` ranks below every explicit
    // level, so without this upgrade a caller-supplied policy that names no `level` (say
    // `{ evidence: ["review-findings"] }`) resolves to `None`: no contract block, gate
    // `NotRequired`, the caller's criteria/evidence/verify/review silently discarded. That is a
    // fail-OPEN. Upstream upgrades to `attested` and enforces.
    let inferred_level = if inferred.level == AcceptanceLevel::None
        && explicit_acceptance_requests_policy(&explicit)
    {
        AcceptanceLevel::Attested
    } else {
        inferred.level
    };

    let level = if explicit_acceptance_can_disable(&explicit) {
        AcceptanceLevel::None
    } else if explicit_level == AcceptanceLevel::Auto {
        inferred_level
    } else {
        // MAX(explicit, inferred) by rank.
        let er = level_rank(explicit_level).unwrap_or(0);
        let ir = level_rank(inferred_level).unwrap_or(0);
        if er >= ir {
            explicit_level
        } else {
            inferred_level
        }
    };

    // `acceptance.ts:509` compares against the UPGRADED `inferredLevel`, so an upgraded
    // read-only agent keeps the read-only branch's own evidence list rather than
    // `requiredEvidenceForLevel("attested")`.
    let base_evidence = if level == inferred_level {
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
        // `acceptance.ts:521-522` @v0.71.0: `criteria: level === "none" ? [] : criteria`,
        // `evidence: level === "none" ? [] : evidence`.
        //
        // SUBA-108 — `formatAcceptancePrompt` emits nothing for `none`, so any criterion or
        // evidence kind surviving here is a contract the child is never told about yet the
        // ledger and `acceptance_requires_child_report` still count. Upstream clears both.
        criteria: if level == AcceptanceLevel::None {
            Vec::new()
        } else {
            criteria
        },
        evidence: if level == AcceptanceLevel::None {
            Vec::new()
        } else {
            evidence
        },
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

    /// Everything `inferLevel` decides, as one comparable value — the whole observable surface of
    /// the inference, so an invariance assertion over it cannot miss a field.
    type InferredShape = (
        AcceptanceLevel,
        Vec<String>,
        Vec<ResolvedAcceptanceGate>,
        Vec<AcceptanceEvidenceKind>,
        Option<ReviewSetting>,
    );

    // ---- inferLevel / resolveEffectiveAcceptance (`acceptance.ts:81-122` @v0.71.0) ----

    /// SUBA-108 — the four role-only branches, each with its exact reasons/criteria/evidence
    /// and review gate. `agent_name`, `task` and `mode` are set to values that the pre-v0.70.1
    /// heuristics reacted to, so a regression that reintroduces any of them fails here.
    #[test]
    fn infers_policies_from_the_declared_role_alone() {
        // `writer` + async → checked + REQUIRED reviewer gate (`:88-100`).
        let async_writer = resolve(AcceptanceResolveInput {
            agent_name: "reviewer".into(),
            acceptance_role: Some(AcceptanceRole::Writer),
            task: Some("Review-only. Do not edit.".into()),
            is_async: true,
            ..Default::default()
        });
        assert_eq!(async_writer.level, AcceptanceLevel::Checked);
        assert_eq!(async_writer.inferred_reason, vec!["async declared writer"]);
        assert_eq!(
            async_writer.review,
            Some(ReviewSetting::Gate(AcceptanceReviewGate {
                agent: Some("reviewer".into()),
                focus: None,
                required: Some(true),
            }))
        );
        assert_eq!(
            async_writer.evidence,
            required_evidence_for_level(AcceptanceLevel::Checked)
        );

        // `writer` + dynamic → the same, with the fanout reason appended (`:90`).
        let dynamic_writer = resolve(AcceptanceResolveInput {
            agent_name: "explorer".into(),
            acceptance_role: Some(AcceptanceRole::Writer),
            task: Some("Fix each item".into()),
            mode: Some(SubagentRunMode::Chain),
            dynamic: true,
            ..Default::default()
        });
        assert_eq!(dynamic_writer.level, AcceptanceLevel::Checked);
        assert_eq!(
            dynamic_writer.inferred_reason,
            vec!["dynamic declared writer", "dynamic fanout context"]
        );

        // plain `writer` → checked, ONE criterion, NO review gate (`:101-108`).
        let writer = resolve(AcceptanceResolveInput {
            agent_name: "reviewer".into(),
            acceptance_role: Some(AcceptanceRole::Writer),
            task: Some("Review only; do not edit".into()),
            ..Default::default()
        });
        assert_eq!(writer.level, AcceptanceLevel::Checked);
        assert_eq!(
            writer.inferred_reason,
            vec!["declared writer acceptance role"]
        );
        assert_eq!(writer.review, None);

        // `read-only` → `none` (`:109-117`), NOT `attested`, even on implementation wording.
        let read_only = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            acceptance_role: Some(AcceptanceRole::ReadOnly),
            task: Some("Implement the fix".into()),
            ..Default::default()
        });
        assert_eq!(read_only.level, AcceptanceLevel::None);
        assert_eq!(
            read_only.inferred_reason,
            vec!["declared read-only acceptance role"]
        );
        // `acceptance.ts:521-522`: the `none` level ships NO criteria and NO evidence, even
        // though `inferLevel`'s read-only branch names one criterion and two evidence kinds.
        // The reasons survive (`:520` is `inferredReason: inferred.reasons`), which is how the
        // ledger still records WHY the run was not gated.
        assert!(read_only.criteria.is_empty(), "{:?}", read_only.criteria);
        assert!(read_only.evidence.is_empty(), "{:?}", read_only.evidence);

        // no role → the lightweight default (`:118-122`), whatever the name or task says.
        let default = resolve(AcceptanceResolveInput {
            agent_name: "worker".into(),
            acceptance_role: None,
            task: Some("Run the security migration release".into()),
            ..Default::default()
        });
        assert_eq!(default.level, AcceptanceLevel::Attested);
        assert_eq!(
            default.inferred_reason,
            vec!["default lightweight attestation"]
        );
        assert_eq!(default.review, None);
    }

    /// The property that the v0.71.0 rewrite is FOR: the result depends only on the
    /// role/async/dynamic axes, and is byte-identical across the task-text and agent-name axes.
    /// Before `7c98a696` eleven wording- and name-derived predicates fed this function; this
    /// table fails in many rows at once if any of them comes back.
    #[test]
    fn inference_is_invariant_across_task_text_and_agent_name() {
        for role in [
            Some(AcceptanceRole::Writer),
            Some(AcceptanceRole::ReadOnly),
            None,
        ] {
            for (is_async, dynamic) in [(false, false), (true, false), (false, true)] {
                let mut baseline: Option<InferredShape> = None;
                for task in [
                    "Run the security migration release",
                    "Review only; do not edit",
                    "Implement the fix",
                ] {
                    for agent_name in ["reviewer", "worker"] {
                        let got = resolve(AcceptanceResolveInput {
                            agent_name: agent_name.into(),
                            acceptance_role: role,
                            task: Some(task.into()),
                            is_async,
                            dynamic,
                            ..Default::default()
                        });
                        let shape = (
                            got.level,
                            got.inferred_reason.clone(),
                            got.criteria.clone(),
                            got.evidence.clone(),
                            got.review.clone(),
                        );
                        match &baseline {
                            None => baseline = Some(shape),
                            Some(first) => assert_eq!(
                                *first, shape,
                                "role={role:?} async={is_async} dynamic={dynamic} \
                                 task={task:?} agent={agent_name:?} diverged"
                            ),
                        }
                    }
                }
            }
        }
    }
    // ---- SUBA-108 defect 1: `acceptance.ts:505` — `none` + explicit policy -> `attested` ----

    /// `acceptance.ts:505` @v0.71.0:
    /// `inferred.level === "none" && explicitAcceptanceRequestsPolicy(explicit) ? "attested"`.
    ///
    /// This is the fail-OPEN this row's `read-only -> none` branch made reachable. The caller
    /// hands in a policy that names no `level` at all, so `explicitLevel` is `auto` and the MAX
    /// escalation never runs; without the `:505` upgrade the resolved level is `None`, the
    /// contract block is empty, the gate is `NotRequired` and every criterion, evidence kind,
    /// verify command and review gate the caller asked for is discarded in silence.
    #[test]
    fn a_caller_supplied_policy_upgrades_a_read_only_none_to_attested() {
        let resolved = resolve(AcceptanceResolveInput {
            agent_name: "scout".into(),
            acceptance_role: Some(AcceptanceRole::ReadOnly),
            task: Some("Audit the parser".into()),
            explicit: Some(AcceptanceInput::Config(AcceptanceConfig {
                evidence: Some(vec![AcceptanceEvidenceKind::ReviewFindings]),
                ..AcceptanceConfig::default()
            })),
            ..Default::default()
        });
        assert_eq!(
            resolved.level,
            AcceptanceLevel::Attested,
            "an explicit policy with no `level` must still be enforced on a read-only agent"
        );
        // `:509` compares against the UPGRADED level, so the surviving evidence is the read-only
        // branch's own list plus the caller's, de-duplicated — not
        // `requiredEvidenceForLevel("attested")`.
        assert_eq!(
            resolved.evidence,
            vec![
                AcceptanceEvidenceKind::ReviewFindings,
                AcceptanceEvidenceKind::ResidualRisks
            ]
        );
        assert!(
            !resolved.criteria.is_empty(),
            "the upgraded level is not `none`, so `:521` no longer clears the criteria"
        );
        assert!(resolved.explicit);
    }

    /// The four inputs `explicitAcceptanceRequestsPolicy` (`acceptance.ts:243-245`) distinguishes,
    /// on the one role where the answer changes the resolved level.
    ///
    /// `auto`/absent asks for nothing, so `none` stands. Any non-`auto` level, and any non-`level`
    /// key on its own, is a request for a policy. The `{ level: "none", reason }` case is the
    /// exception that must NOT be upgraded: `explicitAcceptanceCanDisable` (`:239-241`) is checked
    /// first and pins the level at `none` deliberately.
    #[test]
    fn only_a_policy_bearing_explicit_input_upgrades_a_read_only_agent() {
        let cases: Vec<(&str, Option<AcceptanceInput>, AcceptanceLevel)> = vec![
            ("absent", None, AcceptanceLevel::None),
            (
                "level auto",
                Some(AcceptanceInput::Level(AcceptanceLevel::Auto)),
                AcceptanceLevel::None,
            ),
            (
                "empty config",
                Some(AcceptanceInput::Config(AcceptanceConfig::default())),
                AcceptanceLevel::None,
            ),
            (
                "non-level key only",
                Some(AcceptanceInput::Config(AcceptanceConfig {
                    stop_rules: Some(vec!["stop".into()]),
                    ..AcceptanceConfig::default()
                })),
                AcceptanceLevel::Attested,
            ),
            (
                "explicit attested",
                Some(AcceptanceInput::Level(AcceptanceLevel::Attested)),
                AcceptanceLevel::Attested,
            ),
            (
                "explicit checked",
                Some(AcceptanceInput::Level(AcceptanceLevel::Checked)),
                AcceptanceLevel::Checked,
            ),
            (
                "deliberate disable",
                Some(AcceptanceInput::Config(AcceptanceConfig {
                    level: Some(AcceptanceLevel::None),
                    reason: Some("read-only audit".into()),
                    ..AcceptanceConfig::default()
                })),
                AcceptanceLevel::None,
            ),
            (
                "false shorthand",
                Some(AcceptanceInput::Disabled),
                AcceptanceLevel::None,
            ),
        ];
        for (label, explicit, want) in cases {
            let resolved = resolve(AcceptanceResolveInput {
                agent_name: "scout".into(),
                acceptance_role: Some(AcceptanceRole::ReadOnly),
                explicit,
                ..Default::default()
            });
            assert_eq!(resolved.level, want, "case {label}");
            if want == AcceptanceLevel::None {
                assert!(resolved.criteria.is_empty(), "case {label}");
                assert!(resolved.evidence.is_empty(), "case {label}");
            }
        }
    }

    /// The upgrade is scoped to an inferred `none`: a role that already infers a real level is
    /// resolved by the MAX rule alone, so a non-`level` explicit key must not push a writer's
    /// `checked` anywhere (in particular not DOWN to `attested`).
    #[test]
    fn the_none_upgrade_does_not_touch_a_role_that_already_infers_a_level() {
        for (role, want) in [
            (Some(AcceptanceRole::Writer), AcceptanceLevel::Checked),
            (None, AcceptanceLevel::Attested),
        ] {
            let resolved = resolve(AcceptanceResolveInput {
                agent_name: "worker".into(),
                acceptance_role: role,
                explicit: Some(AcceptanceInput::Config(AcceptanceConfig {
                    stop_rules: Some(vec!["stop".into()]),
                    ..AcceptanceConfig::default()
                })),
                ..Default::default()
            });
            assert_eq!(resolved.level, want, "role={role:?}");
        }
    }
}
