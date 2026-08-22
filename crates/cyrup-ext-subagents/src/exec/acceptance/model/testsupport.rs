//! Fixtures shared by more than one `model` submodule's tests.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;
use serde_json::json;
use serde_json::Value;

pub(crate) fn cfg(config: AcceptanceConfig) -> Option<AcceptanceInput> {
    Some(AcceptanceInput::Config(config))
}


pub(crate) fn resolve(input: AcceptanceResolveInput) -> ResolvedAcceptanceConfig {
    resolve_effective_acceptance(&input)
}


pub(crate) fn report_text(overrides: Value, fence: &str) -> String {
    format!(
        "done\n```{fence}\n{}\n```",
        serde_json::to_string(&report_value(overrides)).unwrap()
    )
}


pub(crate) fn temp_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("file.txt"), "hello\n").expect("seed");
    dir
}


// ---- evaluateAcceptance (async, real subprocess / real git) ----

// ---- G78: the reportOptional ladder (`acceptance.ts:1251-1266` @v0.43.0) ----

/// A policy declaring NO criteria and NO evidence — `acceptanceRequiresChildReport` is
/// `false` for it (`acceptance.ts:403-405`), so a `report_optional` caller is one that
/// never showed the child a contract block at all (`:409`).
pub(crate) fn attested_policy_requiring_no_report() -> ResolvedAcceptanceConfig {
    // Built directly rather than through `resolve_effective_acceptance`, which always
    // merges `requiredEvidenceForLevel` and the inferred criteria in
    // (`acceptance.ts:64-75`, applied in `resolveEffectiveAcceptance` at `:344-401`) and so can never yield this shape. Upstream reaches it via
    // an agent contract, whose own resolution supplies neither.
    let acceptance = ResolvedAcceptanceConfig {
        level: AcceptanceLevel::Attested,
        explicit: true,
        inferred_reason: Vec::new(),
        criteria: Vec::new(),
        evidence: Vec::new(),
        verify: Vec::new(),
        review: Option::None,
        stop_rules: Vec::new(),
        reason: Option::None,
    };
    assert!(
        !acceptance_requires_child_report(&acceptance),
        "premise: this policy must require no child report, got {acceptance:?}"
    );
    acceptance
}

pub(crate) fn report_value(overrides: Value) -> Value {
    let mut base = json!({
        "criteriaSatisfied": [{"id": "criterion-1", "status": "satisfied", "evidence": "verified in test"}],
        "changedFiles": ["src/file.ts"],
        "testsAddedOrUpdated": ["test/file.test.ts"],
        "commandsRun": [{"command": "npm test", "result": "passed", "summary": "passed"}],
        "validationOutput": ["tests passed"],
        "residualRisks": [],
        "noStagedFiles": true,
        "notes": "complete"
    });
    if let (Value::Object(b), Value::Object(o)) = (&mut base, overrides) {
        for (k, v) in o {
            b.insert(k, v);
        }
    }
    base
}
