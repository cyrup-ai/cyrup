---
stage: new
status: done
updated: 2026-08-27 05:30
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

**upstream** — `git show v0.57.0:src/runs/shared/acceptance.ts`. `reportEvidenceStatus` at **`:932`**
returns `AcceptanceRuntimeCheckStatus`, not a boolean: for `"changed-files"` it returns `"failed"`
only when the field is not a string array, and otherwise
`report.changedFiles.length === 0 ? "not-applicable" : "passed"` — identically for `"tests-added"`.
Every other kind is binary. `runStructuralChecks` at **`:961`** opens its loop with
**`:964`** `if (kind === "no-staged-files" && report.noStagedFiles === undefined) continue;` — the
report-derived check is SKIPPED, and only the parent-side real `checkNoStagedFiles(cwd)`
(`git status --short`, pushed unconditionally at **`:976`** when the kind is requested) decides. The
tri-state is recorded as the check status with the message at **`:972`**
``${kind} evidence explicitly reported as not applicable.`` `evaluateAcceptance` rejects on
`runtimeChecks.some((check) => check.status === "failed")` only, so `not-applicable` does **not**
reject. The `no-staged-files` `continue` is absent from `git show v0.47.1:src/runs/shared/acceptance.ts`
(`bd5664a0 fix: trust parent staged-file acceptance check (#1385)`).

**cyrup** — `crates/cyrup-ext-subagents/src/exec/acceptance/model/checks.rs:14-42`
`report_evidence_present` returns a plain `bool`:
`ChangedFiles => report.changed_files.as_ref().is_some_and(|v| !v.is_empty())`,
`TestsAdded => …is_some_and(|v| !v.is_empty())`, `NoStagedFiles => report.no_staged_files == Some(true)`.
`run_structural_checks` (`:170-196`) iterates `for kind in evidence` with **no skip clause** and maps
the bool binary: `status: if present { RuntimeCheckStatus::Passed } else { RuntimeCheckStatus::Failed }`,
message `"{kind} evidence missing from child report."`, then pushes the parent-side
`check_no_staged_files(cwd)` at `:192-194`. `grep -rn 'NotApplicable' --include=*.rs` shows
`RuntimeCheckStatus::NotApplicable` is produced at exactly two sites, `checks.rs:125,132` — both
inside `check_no_staged_files`'s git-unavailable branch — **never** for an evidence check.
`src/exec/acceptance/model/evaluate.rs:160,208,219` reject on
`.any(|c| c.status == RuntimeCheckStatus::Failed)`.

**Impact** — Two spurious rejections on normal paths. **(1)** A child under
`acceptance: {evidence: ["changed-files"]}` that correctly reports `changedFiles: []` — a reviewer, an
oracle, a genuine no-op task — is accepted upstream with `evidence:changed-files = not-applicable` and
**REJECTED** by the port with `evidence:changed-files failed / changed-files evidence missing from
child report`. **(2)** With `evidence: ["no-staged-files"]` and a clean workspace, a child that simply
omits `noStagedFiles` is accepted upstream (the parent's own `git status` passes) and rejected by the
port — even though the port's own `git status` check *in the very same list* passed. In both cases
the ledger flips to `rejected` and the caller is told the child failed acceptance when it did not.
`high` not `critical`: the wrong verdict is loud (an explicit `rejected` status carrying a named
message), it fails closed rather than admitting bad work, and nothing is lost or bypassed.

**Fix** — One function. Change `report_evidence_present` to return `RuntimeCheckStatus`, giving
`ChangedFiles`/`TestsAdded` upstream's three arms (not-a-string-array → `Failed`, empty →
`NotApplicable`, else `Passed`), add the third message arm, and add the
`NoStagedFiles && report.no_staged_files.is_none() → continue` skip at the top of
`run_structural_checks`'s loop.

**Verify** — `evidence: ["changed-files"]` with `changedFiles: []` must accept, with the
`not-applicable` status and pi's message; with `changedFiles: "oops"` (not an array) must reject.
`evidence: ["no-staged-files"]` with `noStagedFiles` omitted and a clean worktree must accept with
exactly one `no-staged-files` check in the list.

**Relation to corpus** — New. Area 09 has no acceptance-scoring row (`SUBA-028` is acceptance
*cancellation*), and this pass confirmed the acceptance tree is otherwise substantially complete —
this is a defect inside ported code, not a missing subsystem. Both halves are one function; file and
fix together.

---

## Scope

In scope: the behaviour described above, in `crates/cyrup-ext-subagents/`.

Out of scope: the other SUBA items in this batch (each has its own file); any refactor beyond what the fix needs; and the ledger corrections in `SUBA-CORPUS-HEALTH.md`.

Full finding, with the complete evidence chain: [`09a-cyrup-ext-subagents-v0.57-drift.md` § SUBA-076](../../../docs/gap-analysis/09a-cyrup-ext-subagents-v0.57-drift.md)

## Approach

Make evidence scoring tri-state (satisfied / unsatisfied / not-applicable) to match upstream, so an
honest empty `changedFiles: []` and an omitted `noStagedFiles` are skipped rather than scored as
failures.

## Acceptance Criteria

- [ ] An honest `changedFiles: []` no longer produces a REJECT
- [ ] An omitted `noStagedFiles` is skipped, not failed
- [ ] Tests cover each of the three states per evidence check
- [ ] `cargo test -p cyrup-ext-subagents` passes
