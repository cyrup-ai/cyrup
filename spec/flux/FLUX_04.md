---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_04 — Port `exec.md` + `qa.md` + `tests.md` (execution triad)

## OBJECTIVE

Port the three execution-stage templates —
[`exec.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/exec.md) (implement
exactly what the task says),
[`qa.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/qa.md) (10/10 gate with
the `done/$SESSION_TS/` move),
[`tests.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/tests.md) (regression
classification via merge-base worktree) — into
`/Users/davidmaple/cyrup.ai/cyrup-flux/prompts/flux/`.

Apply parent spec [§3.3 rules 1–9](../flux.md). These three carry flux's strictest constraint
blocks (no-git, scope guard, review-only, git allow-list) — every one stays byte-identical.

## SUBTASKS

### SUBTASK 1: `prompts/flux/exec.md`

1. Frontmatter: delete `name: exec`; keep `argument-hint` + `description`.
2. Tool renames (3 sites): `create_file`/`replace_in_file` → `write`/`edit` (HARD CONSTRAINTS);
   `invoke_agent` → `subagent` (MULTI-TASK MODE) with the "parallel foreground calls only;
   NEVER background" rewrite (spec §3.3 rule 4). Apply the same `subagent` wording inside the
   "Subagent prompt template" paragraph.
3. FLUX-GAP (1 site): STEP 1's empty-arg interactive selection (`Use ask_user_question to let
   user select task(s)`) — mark + interim plain-text selection (numbered list; reply with
   numbers or names; multiple selections processed sequentially).
4. Keep verbatim: the MANDATORY OVERRIDE preamble; the pure-integer guard paragraph
   (`grep -qE '^[0-9]+$'`, "CMPAN_5 … is NOT a number — treat it as a filename"); SINGLE-TASK
   MODE steps 2–6 including the two exact output sentences ("I've completed and verified 100%
   …" / "I'm ready for a full and detailed QA review …"); the **no-git** rule ("YOU WILL BE
   IMMEDIATELY FIRED …"); the scope guard (research deliverables must not touch `./src/`); the
   per-file frontmatter update ordering in MULTI-TASK MODE ("Update each file as it completes,
   not all at once").

### SUBTASK 2: `prompts/flux/qa.md`

1. Frontmatter: delete `name: qa`; keep `argument-hint` + `description`.
2. Tool rename (1 site): `invoke_agent` → `subagent` (MULTI-TASK MODE), same foreground-only
   rewrite.
3. FLUX-GAP (1 site): STEP 1 empty-arg interactive selection — same treatment as exec.
4. Keep verbatim: the MANDATORY OVERRIDE preamble; BOTH "⚠️ CRITICAL: `stage` MUST be the
   literal string `qa` — NEVER `done`" warnings (spec §1.4 — the renderer groups by directory,
   not stage); the 10/10 move block (`SESSION_TS` grep + fallback + `mkdir -p` + `mv`); the
   <10 needs-rework body-rewrite rule ("remove every item that is complete … focus on
   outstanding items"); the review-only HARD CONSTRAINT; the no-git rule; the two-option
   "Propose next steps" block (PIPELINE A `/flux/tests` vs PIPELINE B `/flux/review`).

### SUBTASK 3: `prompts/flux/tests.md`

1. Frontmatter: delete `name: tests`; keep `argument-hint` + `description`.
2. No tool renames, no GAP sites (grep confirms).
3. Keep verbatim: the `TEST_CMD` missing → "Run /flux/config" stop; the merge-base baseline
   worktree flow with its failure fallback ("skip baseline and assume all failures are
   regressions — note this clearly"); the Regression/Pre-existing/Unknown classification table;
   the max-3 fix cycles; the HARD CONSTRAINT git allow-list (`git merge-base`, `git worktree`,
   `git remote` ONLY — no commit/push/stash/checkout/branch); "MUST NOT add new test cases or
   expand test scope".

### SUBTASK 4: Sweep + behavioral check

```bash
cd /Users/davidmaple/cyrup.ai/cyrup-flux
rg -n 'create_file|replace_in_file|read_file|invoke_agent' prompts/   # expect: no hits
rg -c 'FLUX-GAP: ask_user_question' prompts/flux/exec.md prompts/flux/qa.md  # expect: 1 and 1
```

Behavioral (scratch repo from FLUX_02/03, which now has augmented subtasks in `todo/`):

- `/flux/exec 1` — sequential execution of all subtasks; each file's frontmatter transitions
  `stage: exec, status: in-progress` → `status: done`; task bodies are NOT modified.
- `/flux/qa 1` — on a 10/10 verdict the task file moves to `done/<SESSION_TS>/` with
  `stage: qa, status: completed`; on <10 the file stays in `todo/` with
  `stage: qa, status: needs-rework` and a body reduced to outstanding items.
- `/flux/tests` — with `TEST_CMD` set in `config.env` (FLUX_02), the suite runs and the
  classification summary prints; with `TEST_CMD` empty, it stops with the
  "Run /flux/config" error.

## RESEARCH NOTES

- The `done/<SESSION_TS>/` move is the state transition the Phase 2 status renderer groups by
  (spec §3.4.2, `collect_done` + `format_timestamp`) — the `mv` block must stay exact.
- exec/qa's parallel refill loop ("spawn new subagents as initial ones complete") is
  deliberately left as-is here; FLUX_13 aligns it with the `subagent` tool's real scheduling
  (spec §3.5).
- `tests.md`'s worktree baseline is the only flux prompt allowed `git worktree`; the allow-list
  wording is the safety boundary — do not paraphrase it.

## DEFINITION OF DONE

- [ ] `/flux/exec`, `/flux/qa`, `/flux/tests` load and expand.
- [ ] Sweeps produce exactly the expected results; 2 new GAP markers in place.
- [ ] Scratch run reproduces: exec frontmatter transitions with untouched task bodies; the
      10/10 `done/<SESSION_TS>/` move with `stage: qa, status: completed`; the <10
      `needs-rework` path; the `TEST_CMD` stop-when-empty behavior.

No tests to be written. No benchmarks to be written.
