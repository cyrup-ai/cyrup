//! Lowering a raw wire `acceptance` value onto an [`AcceptanceContract`] (SUBA-041 / SUBA-N04).

use super::AcceptanceStatus;
use super::contract::{AcceptanceContract, VerifyCommand};

// ============================================================================================
// Lowering a raw wire `acceptance` value onto an `AcceptanceContract` (SUBA-041 / SUBA-N04)
// ============================================================================================

/// Lower a raw wire `acceptance` value (pi `AcceptanceOverride`, `schemas.ts:80-93`) onto this
/// crate's [`AcceptanceContract`], after running pi's own `validateAcceptanceInput`
/// (`pi-subagents/src/runs/shared/acceptance.ts:164-286` @v0.34.0, applied at
/// `subagent-executor.ts:1418`) so a malformed policy is refused BEFORE any child spawns, with pi's
/// verbatim messages.
///
/// Level mapping (pi `AcceptanceLevel` -> [`AcceptanceStatus`]): `auto` yields `None`, i.e. pi's own
/// "omitted means auto-inferred" — [`crate::exec::run_sync`] then resolves it through
/// [`AcceptanceContract::resolve_effective`] against
/// [`AcceptanceContract::heuristic_default`] (R-SA-023), which is this crate's `inferLevel`. Every
/// other level (and the `false` shorthand, pi's `level: "none"`) becomes an EXPLICIT contract, which
/// is what arms R-SA-033's post-hoc exit-code correction.
///
/// An explicit level is a **floor**, not a replacement ([`AcceptanceContract::explicit_floor`]):
/// upstream takes `max(explicit, inferred)` by rank (`acceptance.ts:277-281`), so the only inputs
/// that can lower or remove the inferred requirement are the two `explicitAcceptanceCanDisable`
/// accepts (`:134-136`) — `false`, and `{ level: "none", reason: <non-blank> }`. A bare `"none"`
/// string is NOT one of them.
///
/// # Why this lives here and not on one call site
///
/// It is the SINGLE lowering every execution surface shares: the SINGLE-mode `acceptance` tool param
/// (`extension.rs::route_single`, pi `subagent-executor.ts:1418`), and every chain/parallel/
/// background STEP's own `acceptance` (`background/runner_main.rs::ExecSingleStepExecutor::
/// run_single`, pi `chain-execution.ts:333,1195` @v0.34.0 — which pass `task.acceptance`/`seqStep.acceptance`
/// into the very same `runSync` call the single path uses). SUBA-N04: the step path used to hard-drop
/// the field to `None`, so a declared contract ran UNVERIFIED; a second parser would have re-opened
/// exactly that drift, so both paths call this one function.
///
/// `criteria`/`evidence`/`review`/`stopRules` are lowered onto the contract too (SUBA-C13), through
/// the [`crate::exec::acceptance::model`] port's own `normalizeCriteria` (`acceptance.ts:330-342`), so
/// [`crate::exec::acceptance::lattice::inject::inject_acceptance_contract`] can TELL the child about them and [`crate::exec::acceptance::lattice::gate::evaluate_acceptance`]'s
/// `Checked` rung can ENFORCE them. Before SUBA-C13 all four were validated here and then dropped on
/// the floor: `{ level: "checked", criteria: [{ id: "c1", must: "add a regression test" }],
/// evidence: ["tests-added", "no-staged-files"] }` armed a gate that could never fire, because the
/// child was never told to report `c1` and nothing ever looked for `testsAddedOrUpdated` or ran
/// `git status`.
///
/// **[CYRUP-DELTA]** `reason` is still consumed only as the `explicitAcceptanceCanDisable` predicate
/// below, never carried onto the contract — it has no injection or enforcement role upstream either.
/// And the evidence set here is the DECLARED one only: pi additionally unions in
/// `requiredEvidenceForLevel(level)` when the resolved level differs from the inferred one
/// (`acceptance.ts:282-286`), which needs the [`crate::exec::acceptance::model`] port's full `inferLevel` tree rather than
/// this crate's enum-lattice [`AcceptanceContract::heuristic_default`], so a bare
/// `{ level: "checked" }` with no `evidence` key still declares no evidence here where pi would
/// require five kinds.
///
/// # Errors
///
/// Returns every `validateAcceptanceInput` message, space-joined, exactly as pi renders them
/// (`subagent-executor.ts:1758-1762`).
pub fn lower_acceptance_input(
    raw: &serde_json::Value,
) -> Result<Option<AcceptanceContract>, String> {
    let errors = crate::exec::acceptance::model::validate_acceptance_input(raw, "acceptance");
    if !errors.is_empty() {
        return Err(errors.join(" "));
    }

    fn level_to_status(level: &str) -> Option<AcceptanceStatus> {
        match level {
            "none" => Some(AcceptanceStatus::NotRequired),
            "attested" => Some(AcceptanceStatus::Attested),
            "checked" => Some(AcceptanceStatus::Checked),
            "verified" => Some(AcceptanceStatus::Verified),
            // `"reviewed"` is deliberately absent: it is not an `AcceptanceLevel` at v0.43.0 and
            // `crate::exec::acceptance::model::validate_acceptance_input` has already rejected it above with
            // `EXPLICIT_REVIEWED_UNAVAILABLE`, so this arm can never be reached from the wire.
            // `"auto"` (and anything `validate_acceptance_input` let through) infers.
            _ => None,
        }
    }

    match raw {
        // pi `acceptance: false` is the `level: "none"` shorthand (`acceptance.ts:149-154`) — and
        // the ONE string-ish form that genuinely disables the gate, because
        // `normalizeAcceptanceInput` supplies the reason `"disabled by deprecated false shorthand"`
        // itself, satisfying `explicitAcceptanceCanDisable` (`:134-136`).
        serde_json::Value::Bool(false) => Ok(Some(AcceptanceContract::explicit(
            AcceptanceStatus::NotRequired,
            Vec::new(),
        ))),
        // A bare level string carries no `reason`, so `explicitAcceptanceCanDisable` is false for
        // it (`acceptance.ts:149-174`): `"none"` here is a FLOOR of `none`, which
        // [`AcceptanceContract::resolve_effective`]'s max then discards in favour of the inferred
        // level — it does not switch the gate off.
        serde_json::Value::String(level) => Ok(level_to_status(level)
            .map(|status| AcceptanceContract::explicit_floor(status, Vec::new()))),
        serde_json::Value::Object(config) => {
            let verify: Vec<VerifyCommand> = config
                .get("verify")
                .and_then(serde_json::Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .enumerate()
                        .filter_map(|(index, item)| lower_verify_command(item, index))
                        .collect()
                })
                .unwrap_or_default();
            let level = config.get("level").and_then(serde_json::Value::as_str);
            // `explicitAcceptanceCanDisable` (`acceptance.ts:167-174`): only an object whose
            // `reason` is a non-blank string may turn the gate off. In practice
            // `validate_acceptance_input` already rejects `{ level: "none" }` with no reason
            // ("acceptance.reason is required when level is none."), so this is belt-and-braces —
            // and for every level above `none` the two constructors are identical anyway.
            let can_disable = config
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| !reason.trim().is_empty());
            let policy = lower_acceptance_policy(config);
            match level.and_then(level_to_status) {
                Some(status) if can_disable => {
                    Ok(Some(policy.apply(AcceptanceContract::explicit(status, verify))))
                }
                Some(status) => Ok(Some(
                    policy.apply(AcceptanceContract::explicit_floor(status, verify)),
                )),
                // `{ verify: [...] }` with no `level` is pi's `level: "auto"` default
                // (`acceptance.ts:149-154` normalizes an absent level to `auto`), so the level is
                // still inferred — but declared `verify[]` commands must not be dropped, so an
                // object carrying any is lowered as an explicit `verified` contract.
                None if !verify.is_empty() => Ok(Some(policy.apply(AcceptanceContract::explicit(
                    AcceptanceStatus::Verified,
                    verify,
                )))),
                // `{ criteria: [...] }` / `{ evidence: [...] }` with no `level` and no `verify[]` is
                // ALSO pi's `auto`: the level is inferred, but `resolveEffectiveAcceptance` still
                // resolves the declared criteria/evidence/review/stopRules and
                // `formatAcceptancePrompt`/`evaluateAcceptance` still consume them
                // (`acceptance.ts:344-401`). Lowering it as a `none` FLOOR reproduces that — the
                // floor is discarded by [`AcceptanceContract::resolve_effective`]'s max in favour of
                // the inferred level, and the policy rides along. Returning `None` here (as this arm
                // did before SUBA-C13) threw the whole policy away.
                None if policy.is_declared() => Ok(Some(
                    policy.apply(AcceptanceContract::explicit_floor(
                        AcceptanceStatus::NotRequired,
                        Vec::new(),
                    )),
                )),
                None => Ok(None),
            }
        }
        // `null`/absent is pi's `undefined`.
        _ => Ok(None),
    }
}

/// Lower one authored `acceptance.verify[i]` object onto a [`VerifyCommand`], carrying **every**
/// key upstream's `ACCEPTANCE_VERIFY_KEYS` admits (`acceptance.ts:52` @v0.43.0) —
/// `id`/`command`/`timeoutMs`/`cwd`/`env`/`allowFailure`. Before SUBA-C12b only `command`
/// survived, so a user who authored `{ id: "lint", command: "npm run lint", allowFailure: true }`
/// passed validation and then had `allowFailure` silently dropped, rejecting the run.
///
/// `command` is the only required-at-lowering key: an entry without it is skipped rather than
/// lowered to an empty shell command. That is unreachable in practice —
/// [`crate::exec::acceptance::model::validate_acceptance_input`] runs first and already rejects a missing/blank `command`
/// (`acceptance.ts:210`) — but keeping the filter makes this function total on arbitrary JSON.
///
/// `id` falls back to `verify[{index}]` for the same defensive reason (upstream requires it,
/// `acceptance.ts:209`); it is used only for diagnostics, never for dispatch.
fn lower_verify_command(item: &serde_json::Value, index: usize) -> Option<VerifyCommand> {
    let command = item.get("command").and_then(serde_json::Value::as_str)?;
    Some(VerifyCommand {
        id: item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map_or_else(|| format!("verify[{index}]"), str::to_string),
        command: command.to_string(),
        timeout_ms: item.get("timeoutMs").and_then(serde_json::Value::as_u64),
        cwd: item
            .get("cwd")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        env: item
            .get("env")
            .and_then(serde_json::Value::as_object)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|(key, value)| {
                        value
                            .as_str()
                            .map(|value| (key.clone(), value.to_string()))
                    })
                    .collect()
            }),
        allow_failure: item.get("allowFailure").and_then(serde_json::Value::as_bool),
    })
}

/// The `criteria`/`evidence`/`review`/`stopRules` half of an authored `acceptance` object, resolved
/// exactly as pi's `resolveEffectiveAcceptance` resolves them (`acceptance.ts:384-395` @v0.43.0):
/// evidence de-duplicated in declaration order, criteria normalized AGAINST that evidence so a
/// criterion declaring none inherits the config-level list.
struct LoweredAcceptancePolicy {
    criteria: Vec<crate::exec::acceptance::model::ResolvedAcceptanceGate>,
    evidence: Vec<crate::exec::acceptance::model::AcceptanceEvidenceKind>,
    review: Option<crate::exec::acceptance::model::ReviewSetting>,
    stop_rules: Vec<String>,
}

impl LoweredAcceptancePolicy {
    /// Whether the authored object declared ANY of the four — the test the no-`level`, no-`verify[]`
    /// arm of [`lower_acceptance_input`] uses to decide between "a real policy at the inferred
    /// level" and pi's own `undefined`.
    fn is_declared(&self) -> bool {
        !self.criteria.is_empty()
            || !self.evidence.is_empty()
            || self.review.is_some()
            || !self.stop_rules.is_empty()
    }

    fn apply(self, contract: AcceptanceContract) -> AcceptanceContract {
        contract.with_policy(self.criteria, self.evidence, self.review, self.stop_rules)
    }
}

/// Lower the four policy keys of an authored `acceptance` object.
///
/// Every arm is deliberately total on arbitrary JSON — [`crate::exec::acceptance::model::validate_acceptance_input`] has
/// already run by the time [`lower_acceptance_input`] calls this and rejected every malformed
/// shape with pi's own message, so a value that does not parse here is unreachable in practice and
/// is skipped rather than defaulted to something a policy author did not write.
fn lower_acceptance_policy(
    config: &serde_json::Map<String, serde_json::Value>,
) -> LoweredAcceptancePolicy {
    // `evidence: AcceptanceEvidenceKind[]` (shared/types.ts:677), de-duplicated by
    // `[...new Set(...)]` (acceptance.ts:283-285).
    let evidence = crate::exec::acceptance::model::unique_evidence(
        &config
            .get("evidence")
            .and_then(serde_json::Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .filter_map(crate::exec::acceptance::model::AcceptanceEvidenceKind::from_wire)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    );

    // `criteria: Array<string | AcceptanceGate>` (shared/types.ts:676).
    let criteria_input: Vec<crate::exec::acceptance::model::CriterionInput> = config
        .get("criteria")
        .and_then(serde_json::Value::as_array)
        .map(|items| items.iter().filter_map(lower_criterion).collect())
        .unwrap_or_default();
    // `normalizeCriteria(criteria, evidence)` (acceptance.ts:296) — the evidence list is the
    // SECOND argument, i.e. a gate that declares no `evidence` of its own inherits the config's.
    let criteria = crate::exec::acceptance::model::normalize_criteria(&criteria_input, &evidence);

    // `review: AcceptanceReviewGate | false` (shared/types.ts:679).
    let review = match config.get("review") {
        Some(serde_json::Value::Bool(flag)) => Some(crate::exec::acceptance::model::ReviewSetting::Disabled(*flag)),
        Some(serde_json::Value::Object(gate)) => {
            Some(crate::exec::acceptance::model::ReviewSetting::Gate(crate::exec::acceptance::model::AcceptanceReviewGate {
                agent: gate
                    .get("agent")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                focus: gate
                    .get("focus")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                required: gate.get("required").and_then(serde_json::Value::as_bool),
            }))
        }
        _ => None,
    };

    // `stopRules: string[]` (shared/types.ts:680).
    let stop_rules = config
        .get("stopRules")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    LoweredAcceptancePolicy {
        criteria,
        evidence,
        review,
        stop_rules,
    }
}

/// Lower one authored `acceptance.criteria[i]` — a bare `must` string, or a full `AcceptanceGate`
/// object (shared/types.ts:652-657). Anything else yields `None` (unreachable past validation).
fn lower_criterion(item: &serde_json::Value) -> Option<crate::exec::acceptance::model::CriterionInput> {
    match item {
        serde_json::Value::String(must) => Some(crate::exec::acceptance::model::CriterionInput::Text(must.clone())),
        serde_json::Value::Object(gate) => {
            Some(crate::exec::acceptance::model::CriterionInput::Gate(crate::exec::acceptance::model::AcceptanceGate {
                id: gate
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                must: gate
                    .get("must")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                evidence: gate.get("evidence").and_then(serde_json::Value::as_array).map(
                    |items| {
                        items
                            .iter()
                            .filter_map(serde_json::Value::as_str)
                            .filter_map(crate::exec::acceptance::model::AcceptanceEvidenceKind::from_wire)
                            .collect()
                    },
                ),
                severity: match gate.get("severity").and_then(serde_json::Value::as_str) {
                    Some("recommended") => Some(crate::exec::acceptance::model::GateSeverity::Recommended),
                    Some("required") => Some(crate::exec::acceptance::model::GateSeverity::Required),
                    _ => None,
                },
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;


    /// `explicitAcceptanceCanDisable` (`acceptance.ts:167-169`) requires a non-blank `reason`, and
    /// a bare `"none"` string carries none. v0.34.0 accepted the string and fell through to
    /// `LEVEL_RANK["none"] >= LEVEL_RANK[inferred]`, leaving the gate armed at the inferred level;
    /// v0.43.0 REFUSES it outright at validation (`acceptance.ts:183`) and says how to write it
    /// properly. Either way the one thing a reasonless `"none"` must never do is disarm the gate —
    /// which is exactly what cyrup did before this pair of fixes — so both halves are asserted:
    /// the string is rejected, and the object form that IS accepted still needs its reason.
    #[test]
    fn a_bare_none_string_cannot_disable_the_gate() {
        let err = lower_acceptance_input(&serde_json::json!("none"))
            .expect_err("a reasonless bare 'none' is rejected at v0.43.0");
        assert_eq!(
            err,
            "acceptance level \"none\" requires a reason; use { level: \"none\", reason: \"...\" }."
        );

        // The object form with no reason is likewise refused, so there is no spelling of a
        // reasonless `none` that reaches the contract at all.
        let object_err = lower_acceptance_input(&serde_json::json!({"level": "none"}))
            .expect_err("`{ level: \"none\" }` with no reason is rejected");
        assert!(
            object_err.contains("acceptance.reason is required when level is none."),
            "{object_err}"
        );

        // And a run with no acceptance param at all still gets the inferred gate, unchanged.
        let effective = AcceptanceContract::resolve_effective(None, "worker", "Implement the fix");
        assert_eq!(effective.required_level, AcceptanceStatus::Checked);
        assert!(!effective.is_no_op(), "the gate must still be evaluated");
    }


    /// Both forms upstream DOES accept as an "off" switch: the `false` shorthand (whose reason
    /// `normalizeAcceptanceInput` supplies itself) and an object `{ level: "none", reason }` with
    /// a non-blank reason.
    #[test]
    fn the_two_disabling_forms_still_turn_the_gate_off() {
        for policy in [
            serde_json::json!(false),
            serde_json::json!({ "level": "none", "reason": "prototype spike, no gate wanted" }),
        ] {
            let lowered = lower_acceptance_input(&policy)
                .expect("a valid policy")
                .expect("a contract");
            assert!(lowered.disables_gate, "must be able to disable: {policy}");

            let effective =
                AcceptanceContract::resolve_effective(Some(lowered), "worker", "Implement the fix");
            assert_eq!(
                effective.required_level,
                AcceptanceStatus::NotRequired,
                "an explicit disable is never raised by the inferred floor: {policy}"
            );
            assert!(effective.is_no_op(), "policy {policy}");
        }
    }


    // ---------------------------------------------------------------------------------------
    // SUBA-C12b regression: the per-command `verify[]` fields upstream's ACCEPTANCE_VERIFY_KEYS
    // admits (`acceptance.ts:52` @v0.43.0 — id/command/timeoutMs/cwd/env/allowFailure) must
    // actually REACH execution and the gate. `validate_verify_input` has always accepted and
    // type-checked all six, but the contract carried only the command string, so five of them
    // were validated and then silently discarded: a `cwd` ran in the wrong directory, an `env`
    // pair was absent, a `timeoutMs` was replaced by a fixed 300 s, and an `allowFailure: true`
    // command still rejected the run (rewriting its exit code to 1 via
    // `apply_post_hoc_correction`) where upstream reports `allowed-failure` and succeeds.
    // ---------------------------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn lower_acceptance_input_carries_every_declared_verify_field() {
        let contract = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [{
                "id": "lint",
                "command": "npm run lint",
                "timeoutMs": 5000,
                "cwd": "packages/api",
                "env": { "CI": "1" },
                "allowFailure": true,
            }],
        }))
        .expect("a well-formed acceptance policy must lower")
        .expect("an explicit contract");

        assert_eq!(contract.verify.len(), 1);
        let declared = &contract.verify[0];
        assert_eq!(declared.id, "lint");
        assert_eq!(declared.command, "npm run lint");
        assert_eq!(declared.timeout_ms, Some(5000), "timeoutMs must survive lowering");
        assert_eq!(
            declared.cwd.as_deref(),
            Some("packages/api"),
            "cwd must survive lowering"
        );
        assert_eq!(
            declared.env.as_ref().and_then(|env| env.get("CI")).map(String::as_str),
            Some("1"),
            "env must survive lowering"
        );
        assert_eq!(
            declared.allow_failure,
            Some(true),
            "allowFailure must survive lowering"
        );
    }


    // ---------------------------------------------------------------------------------------
    // G78 — `reviewed` is not a requestable level, on the LIVE wire-lowering path every
    // execution surface shares (`lower_acceptance_input` -> `crate::exec::acceptance::model::validate_acceptance_input`).
    // ---------------------------------------------------------------------------------------

    #[test]
    fn lowering_rejects_reviewed_as_a_requestable_level_in_both_wire_forms() {
        let bare = lower_acceptance_input(&serde_json::json!("reviewed"))
            .expect_err("a bare `reviewed` level is rejected at v0.43.0");
        assert_eq!(
            bare,
            format!("acceptance {}", crate::exec::acceptance::model::EXPLICIT_REVIEWED_UNAVAILABLE)
        );
        // The message must point the caller at the replacement mechanism, not merely refuse.
        assert!(bare.contains("acceptance.review.required"));

        let object = lower_acceptance_input(&serde_json::json!({"level": "reviewed"}))
            .expect_err("an object-form `reviewed` level is rejected at v0.43.0");
        assert_eq!(
            object,
            format!("acceptance.level {}", crate::exec::acceptance::model::EXPLICIT_REVIEWED_UNAVAILABLE)
        );
    }


    /// The advertise-vs-dispatch invariant: upstream v0.43.0 deliberately KEEPS `"reviewed"` in the
    /// advertised `AcceptanceOverride` enum (`schemas.ts:83-88`, marked `deprecated`) precisely so
    /// this preflight message can explain itself. A schema that stopped advertising it would leave
    /// the model guessing; a dispatch that accepted it would reinstate the deleted level.
    #[test]
    fn reviewed_is_still_advertised_so_the_rejection_can_explain_itself() {
        let rendered = crate::extension::sj_acceptance_override().to_string();
        assert!(
            rendered.contains("\"reviewed\""),
            "the acceptance enum must still advertise `reviewed` so preflight can explain it"
        );
        assert!(lower_acceptance_input(&serde_json::json!("reviewed")).is_err());
    }


    /// The advertise-vs-dispatch invariant, driven over the SCHEMA rather than a hand-written list:
    /// every string value `sj_acceptance_override` offers the model must be one `lower_acceptance_input`
    /// actually accepts — with exactly one upstream-sanctioned exception, `"reviewed"`, which is
    /// advertised in its own `deprecated` branch solely so the refusal can explain itself
    /// (`schemas.ts:83-88` @v0.43.0).
    ///
    /// G78 narrowed the dispatch (bare `"none"` and `"verified"` became hard errors,
    /// `acceptance.ts:183-184`) without narrowing the schema, so the tool advertised two values it
    /// would then refuse. Upstream narrowed both together: `schemas.ts:82` is exactly
    /// `["auto", "attested", "checked"]`.
    #[test]
    fn every_advertised_acceptance_level_is_one_the_dispatch_accepts() {
        let schema = crate::extension::sj_acceptance_override();
        let branches = schema
            .get("anyOf")
            .and_then(serde_json::Value::as_array)
            .expect("AcceptanceOverride is an anyOf");

        let mut advertised: Vec<String> = Vec::new();
        for branch in branches {
            if branch.get("type").and_then(serde_json::Value::as_str) != Some("string") {
                continue;
            }
            let deprecated = branch
                .get("deprecated")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            for value in branch
                .get("enum")
                .and_then(serde_json::Value::as_array)
                .expect("a string branch carries an enum")
            {
                let value = value.as_str().expect("enum entries are strings");
                if deprecated {
                    // The sanctioned exception: advertised, refused, and the refusal EXPLAINS.
                    let err = lower_acceptance_input(&serde_json::json!(value)).expect_err(
                        "a deprecated advertised level must still be refused by the dispatch",
                    );
                    assert!(
                        err.contains(crate::exec::acceptance::model::EXPLICIT_REVIEWED_UNAVAILABLE),
                        "the deprecated `{value}` must be refused with the explanatory text, got {err}"
                    );
                } else {
                    advertised.push(value.to_string());
                }
            }
        }

        assert_eq!(
            advertised,
            ["auto", "attested", "checked"],
            "schemas.ts:82 advertises exactly these three requestable levels"
        );
        for level in &advertised {
            assert!(
                lower_acceptance_input(&serde_json::json!(level)).is_ok(),
                "`{level}` is advertised to the model, so the dispatch must accept it"
            );
        }

        // And the two G78 newly-invalidated values must be gone from the advertised surface
        // entirely — not merely absent from the requestable branch.
        let rendered = schema.to_string();
        for gone in ["\"none\"", "\"verified\""] {
            assert!(
                !rendered.contains(gone),
                "{gone} is rejected by lower_acceptance_input and must not be advertised: {rendered}"
            );
        }
    }


    #[test]
    fn bare_none_and_verified_level_strings_are_rejected() {
        // `AcceptanceInput = Exclude<AcceptanceLevel, "none" | "verified"> | …` (`shared/types.ts:684-685`).
        let none = lower_acceptance_input(&serde_json::json!("none"))
            .expect_err("a bare `none` carries no reason and is rejected");
        assert!(none.contains("requires a reason"), "{none}");
        let verified = lower_acceptance_input(&serde_json::json!("verified"))
            .expect_err("a bare `verified` declares no runtime command and is rejected");
        assert!(
            verified.contains("requires object form with at least one runtime verify command"),
            "{verified}"
        );
        // The three still-requestable bare levels keep working.
        for level in ["auto", "attested", "checked"] {
            assert!(
                lower_acceptance_input(&serde_json::json!(level)).is_ok(),
                "`{level}` must remain requestable"
            );
        }
    }


    #[test]
    fn object_form_verified_without_runtime_commands_is_rejected() {
        let err = lower_acceptance_input(&serde_json::json!({"level": "verified"}))
            .expect_err("`verified` with no verify[] is rejected");
        assert!(
            err.contains("must contain at least one runtime command when level is verified"),
            "{err}"
        );
        let err = lower_acceptance_input(&serde_json::json!({"level": "verified", "verify": []}))
            .expect_err("`verified` with an EMPTY verify[] is rejected too");
        assert!(err.contains("at least one runtime command"), "{err}");
        // A real command is accepted, and the per-command validation still runs alongside.
        let ok = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [{"id": "gate", "command": "true"}],
        }))
        .expect("a verified policy with one command is valid")
        .expect("an explicit contract");
        assert_eq!(ok.required_level, AcceptanceStatus::Verified);
        let bad_item = lower_acceptance_input(&serde_json::json!({
            "level": "verified",
            "verify": [{"command": "true"}],
        }))
        .expect_err("a verify[] entry with no id is still rejected");
        assert!(bad_item.contains("verify[0].id is required."), "{bad_item}");
    }

}
