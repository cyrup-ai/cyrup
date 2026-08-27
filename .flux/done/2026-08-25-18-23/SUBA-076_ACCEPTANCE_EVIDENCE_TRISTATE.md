---
stage: qa
status: completed
updated: 2026-08-27 17:30
severity: high
effort: small
subsystem: acceptance / evidence scoring
source: docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md
item: SUBA-076
---

> **Path note.** This task lives in a subdirectory. Every flux command globs a single level (`ls -1 "$FLUX_BASE/todo/"*.md`), so `/exec`, `/aug` and `/qa` will not list it — pass the absolute path explicitly.

# SUBA-076 — Acceptance evidence checks are scored binary where upstream is tri-state, so an honest `changedFiles: []` and an omitted `noStagedFiles` each produce a spurious acceptance REJECTION

**Kind** parity-bug · **Severity** high · **Effort** S · **Confidence** confirmed
**Subsystem** acceptance / evidence scoring
**Window** in-baseline (≤ v0.43.0) for the `changed-files`/`tests-added` tri-state; **v0.47.1..v0.57.0** for the `no-staged-files` skip.

**upstream** — `git show v0.57.0:src/runs/shared/acceptance.ts`. Verified line numbers at that tag:
`checkCriteriaSatisfied` **`:922`**, `reportEvidenceStatus` **`:932`**, `checkNoStagedFiles`
**`:950`**, `runStructuralChecks` **`:961`**.

```js
function reportEvidenceStatus(report, kind): AcceptanceRuntimeCheckStatus {
    switch (kind) {
        case "changed-files":
            if (!isStringArray(report.changedFiles)) return "failed";
            return report.changedFiles.length === 0 ? "not-applicable" : "passed";
        case "tests-added":
            if (!isStringArray(report.testsAddedOrUpdated)) return "failed";
            return report.testsAddedOrUpdated.length === 0 ? "not-applicable" : "passed";
        // the other seven kinds stay binary
    }
}

function runStructuralChecks(acceptance, report, cwd) {
    const checks = [];
    for (const kind of acceptance.evidence) {
        if (kind === "no-staged-files" && report.noStagedFiles === undefined) continue;   // :964
        const status = reportEvidenceStatus(report, kind);
        checks.push({ id: `evidence:${kind}`, status, message: status === "passed"
            ? `${kind} evidence present.`
            : status === "not-applicable"
                ? `${kind} evidence explicitly reported as not applicable.`
                : `${kind} evidence missing from child report.` });
    }
    if (acceptance.evidence.includes("no-staged-files")) checks.push(checkNoStagedFiles(cwd));
    return checks;
}
```

`evaluateAcceptance` rejects on `runtimeChecks.some((check) => check.status === "failed")`
(`:1332`, `:1360`, `:1366`), so `not-applicable` never rejects. The `no-staged-files` `continue` is
absent from v0.47.1 (`bd5664a0 fix: trust parent staged-file acceptance check (#1385)`).

**cyrup** — [`model/checks.rs`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/model/checks.rs):
`report_evidence_present` (`:14-42`) returns a plain `bool`; `run_structural_checks` (`:170-196`)
has no skip clause and maps that bool binary onto `Passed`/`Failed`.
[`model/evaluate.rs`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/model/evaluate.rs)
rejects on `.any(|c| c.status == RuntimeCheckStatus::Failed)` at `:160`, `:208`, `:219`.

---

## Corrections to the item as filed

**(1) The `changedFiles: "oops"` verify case is WRONG for this port — it passes, and correctly so.**
The item asks for "with `changedFiles: "oops"` (not an array) must reject". This port's normalizer
COERCES a bare string into a one-element array before deserialization
([`report/normalize.rs:291-294`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/normalize.rs)):

```rust
            "changedFiles" | "testsAddedOrUpdated" | "validationOutput" | "residualRisks"
            | "reviewFindings" => match field {
                Value::String(_) => Value::Array(vec![field.clone()]),
                _ => field.clone(),
            },
```

so `"oops"` becomes `["oops"]` → non-empty → **passed**. That is deliberate and already covered by a
live test — `live_gate_accepts_an_aliased_child_report`
([`lattice/gate.rs:568`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/lattice/gate.rs))
feeds `"changed_files": "src/file.rs"` and asserts the gate ACCEPTS. The rationale is recorded at
[`report/validate.rs:307-311`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/model/report/validate.rs):
upstream v0.34.0 type-checked the companions, v0.43.0 stopped, and the normalizer now repairs the
shape. **Do not add a check that re-rejects it** — that would regress a passing test.

**(2) Upstream's four arms collapse to three here, and the collapse is total.** `changed_files` is
`Option<Vec<String>>` ([`model/types.rs:316`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/model/types.rs)),
so exactly three states are reachable, and upstream's `!isStringArray(...)` arm is precisely `None`:

| upstream | reachable here as | status |
|---|---|---|
| `isStringArray(undefined) === false` | `None` — key absent | `Failed` |
| `isStringArray("oops") === false` | **unreachable** — normalized to `["oops"]` (correction 1) | — |
| `length === 0` | `Some(vec![])` | `NotApplicable` |
| otherwise | `Some(non-empty)` | `Passed` |

A value that genuinely cannot become a `Vec<String>` — a number, or an array holding a non-string —
fails deserialization and yields `ParsedAcceptanceReport { report: None, error: Some(_) }`, the
malformed-report path, which never reaches `run_structural_checks` at all. So there is no fourth arm
to write, and no way to exercise one.

**(3) The citations on both touched functions are stale.** `checks.rs:14` cites
`reportEvidencePresent (acceptance.ts:632-644)` — upstream RENAMED the function to
`reportEvidenceStatus` and it now sits at `:932`. `run_structural_checks` cites `:950-966`; it is now
`:961`. The module header (`checks.rs:1-2`) cites `:911-966`. Re-point these three while editing
them. (`check_no_staged_files`'s `:939-948` and `check_criteria_satisfied`'s `:911-919` are stale for
the same reason — now `:950` and `:922` — but those functions are not otherwise touched here; leave
them.)

**(4) No existing test needs changing.** The only two tests that use `ChangedFiles` evidence are both
unaffected: `gate.rs:568` passes through the string coercion (correction 1), and the `evaluate.rs`
whitespace test feeds `changedFiles: ["   "]`, which is rejected at PARSE time with
`changedFiles[0]: expected non-empty string` and never reaches the evidence check. This is a purely
additive change.

---

## What already exists — REUSE, do not re-port

| need | already present |
|---|---|
| the third status | `RuntimeCheckStatus::{Passed, Failed, NotApplicable}` (`model/types.rs:339`), `#[serde(rename_all = "kebab-case")]` so it serializes as `"not-applicable"` — upstream's exact wire string |
| `not-applicable` not rejecting | `evaluate.rs` already rejects on `.any(status == Failed)` only; nothing to change there |
| both call sites fixed at once | `run_structural_checks` has exactly two callers — `evaluate.rs:155` (model path) and `lattice/gate.rs:340` (live gate) — so ONE edit fixes both. Do not add a second fix at either call site. |
| the parent-side real check | `check_no_staged_files` (`checks.rs:112`), id `"no-staged-files"` — distinct from the report check's `"evidence:no-staged-files"`, which is what makes the skip observable |
| the seven binary rules | already correct against v0.57.0, including the two that accept an EMPTY array (`residual-risks`, `review-findings` are `isStringArray` with no length test). Preserve them verbatim. |

---

## Required implementation

Both edits are in
[`src/exec/acceptance/model/checks.rs`](../../../crates/cyrup-ext-subagents/src/exec/acceptance/model/checks.rs).

### 1. Replace `report_evidence_present` with `report_evidence_status`

Rename it (it is private — no call sites outside this file) and change the return type. Delete the
`bool` version outright rather than leaving it beside the new one: a leftover
`report_evidence_present(report, ChangedFiles)` would still hand back the old wrong answer to
whoever called it next.

```rust
/// The shared `changed-files` / `tests-added` rule (pi `acceptance.ts:934-941` @v0.57.0).
///
/// An HONEST empty list is not withheld evidence — it is evidence that the question does not apply.
/// A reviewer persona, an oracle, and a genuine no-op task all legitimately change no files, and
/// scoring that `Failed` rejects work upstream accepts.
const fn tri_state_list_evidence(field: Option<&Vec<String>>) -> RuntimeCheckStatus {
    match field {
        // pi's `!isStringArray(...)` arm. An ABSENT key is the only way to reach it here: the
        // normalizer repairs a bare string into a one-element array, and any other non-array shape
        // fails deserialization into the malformed-report path, which never reaches this function.
        Option::None => RuntimeCheckStatus::Failed,
        Some(items) if items.is_empty() => RuntimeCheckStatus::NotApplicable,
        Some(_) => RuntimeCheckStatus::Passed,
    }
}

const fn passed_or_failed(present: bool) -> RuntimeCheckStatus {
    if present { RuntimeCheckStatus::Passed } else { RuntimeCheckStatus::Failed }
}

/// `reportEvidenceStatus` (pi `acceptance.ts:932-949` @v0.57.0 — the function was
/// `reportEvidencePresent`, returning a bool, when this port first followed it at v0.43.0).
///
/// Tri-state for `changed-files`/`tests-added`, binary for the other seven: upstream's own split,
/// not a simplification.
fn report_evidence_status(
    report: &AcceptanceReport,
    kind: AcceptanceEvidenceKind,
) -> RuntimeCheckStatus {
    match kind {
        AcceptanceEvidenceKind::ChangedFiles => {
            tri_state_list_evidence(report.changed_files.as_ref())
        }
        AcceptanceEvidenceKind::TestsAdded => {
            tri_state_list_evidence(report.tests_added_or_updated.as_ref())
        }
        AcceptanceEvidenceKind::CommandsRun => {
            passed_or_failed(report.commands_run.as_ref().is_some_and(|v| !v.is_empty()))
        }
        AcceptanceEvidenceKind::ValidationOutput => {
            passed_or_failed(report.validation_output.as_ref().is_some_and(|v| !v.is_empty()))
        }
        // pi `isStringArray(report.residualRisks)` with NO length test — an empty list passes.
        AcceptanceEvidenceKind::ResidualRisks => passed_or_failed(report.residual_risks.is_some()),
        AcceptanceEvidenceKind::NoStagedFiles => {
            passed_or_failed(report.no_staged_files == Some(true))
        }
        AcceptanceEvidenceKind::DiffSummary => passed_or_failed(
            report.diff_summary.as_deref().is_some_and(|s| !s.trim().is_empty()),
        ),
        // Likewise `isStringArray` only.
        AcceptanceEvidenceKind::ReviewFindings => passed_or_failed(report.review_findings.is_some()),
        AcceptanceEvidenceKind::ManualNotes => passed_or_failed(
            report
                .manual_notes
                .as_deref()
                .or(report.notes.as_deref())
                .is_some_and(|s| !s.trim().is_empty()),
        ),
    }
}
```

`Option::None` (not bare `None`) matches this file's existing style — `report/normalize.rs` spells it
that way too, because `checks.rs` imports a `CriterionStatus::NotApplicable` and the qualified form
keeps the two apart at a glance.

### 2. Add the skip and the third message arm in `run_structural_checks`

```rust
    for kind in evidence {
        // pi `acceptance.ts:964` @v0.57.0: the REPORT-derived no-staged-files check is skipped
        // when the child said nothing about it, leaving the parent's own `git status --short`
        // (pushed below whenever the kind is requested) as the sole authority. Upstream added this
        // in `bd5664a0 fix: trust parent staged-file acceptance check (#1385)`; it is absent at
        // v0.47.1. Without it a child that simply OMITS `noStagedFiles` is failed by the report
        // check even though the real check sitting in the very same list passed.
        if *kind == AcceptanceEvidenceKind::NoStagedFiles && report.no_staged_files.is_none() {
            continue;
        }
        let status = report_evidence_status(report, *kind);
        checks.push(AcceptanceRuntimeCheck {
            id: format!("evidence:{}", kind.as_str()),
            status,
            message: match status {
                RuntimeCheckStatus::Passed => format!("{} evidence present.", kind.as_str()),
                RuntimeCheckStatus::NotApplicable => format!(
                    "{} evidence explicitly reported as not applicable.",
                    kind.as_str()
                ),
                RuntimeCheckStatus::Failed => {
                    format!("{} evidence missing from child report.", kind.as_str())
                }
            },
        });
    }
    if evidence.contains(&AcceptanceEvidenceKind::NoStagedFiles) {
        checks.push(check_no_staged_files(cwd).await);
    }
```

The trailing push is UNCHANGED and must stay unconditional on the kind being requested — that is
what makes the skip safe: staged files are still really checked, by `git status`, in every case where
the policy asked for them.

---

## Definition of done

With `evidence: ["changed-files"]` (and identically for `tests-added`):

1. `changedFiles: []` → ledger ACCEPTS; the `evidence:changed-files` check is `not-applicable`
   carrying `changed-files evidence explicitly reported as not applicable.`
2. `changedFiles` absent → still `failed` with `changed-files evidence missing from child report.`
3. `changedFiles: ["src/a.rs"]` → still `passed` with `changed-files evidence present.`

With `evidence: ["no-staged-files"]` on a clean worktree:

4. `noStagedFiles` omitted → ACCEPTS, and the check list holds exactly one entry for this kind: id
   `no-staged-files` (the real `git status` one), with NO `evidence:no-staged-files`.
5. `noStagedFiles: false` → NOT skipped: both `evidence:no-staged-files` (`failed`) and
   `no-staged-files` appear, and the ledger rejects.

And:

6. The other seven kinds score exactly as they do today — in particular `residual-risks` and
   `review-findings` still PASS on an empty array.
7. `cargo test -p cyrup-ext-subagents`, `cargo clippy -p cyrup-ext-subagents --all-targets` and
   `cargo doc -p cyrup-ext-subagents --no-deps --lib` stay as clean as they are now (2534 passing,
   no new clippy finding, no doc warning). Reverting either edit must break (1) or (4) respectively.

## Notes for whoever executes

- `checks.rs` currently has NO `#[cfg(test)]` module. Whatever coverage this needs will be the
  file's first; the crate convention for such a module is
  `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]` — note that
  `clippy::panic` is NOT in that list and is a hard ERROR in this workspace, so assert with
  `assert!`/`assert_eq!` rather than `panic!`/`unwrap_or_else(|| panic!(...))`.
- `run_structural_checks` is `async` (the git check awaits), so anything driving it directly needs
  `#[tokio::test]`.
