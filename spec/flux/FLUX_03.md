---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_03 — Port `ask.md` + `split.md` + `aug.md` (planning triad)

## OBJECTIVE

Port the three planning-stage templates —
[`ask.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/ask.md) (clarify +
augment in place),
[`split.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/split.md) (decompose
into `PREFIX_N.md` subtasks),
[`aug.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/aug.md) (deep research +
in-place augmentation) — into `/Users/davidmaple/cyrup.ai/cyrup-flux/prompts/flux/`.

Apply parent spec [§3.3 rules 1–9](../flux.md). These three share the "edit the task file in
place, touch nothing else" constraint family — keep every HARD CONSTRAINT byte-identical.

## SUBTASKS

### SUBTASK 1: `prompts/flux/ask.md`

1. Frontmatter: delete `name: ask`; keep `argument-hint` + `description`.
2. Tool renames (2 sites, both in HARD CONSTRAINTS): `create_file` → `write`,
   `replace_in_file` → `edit`.
3. FLUX-GAP (1 site): STEP 4's "Use `ask_user_question` to ask clarifying questions one at a
   time" — mark with `<!-- FLUX-GAP: ask_user_question -->` and replace with the interim
   plain-text instruction (spec §3.3 rule 5), preserving the one-at-a-time ordering, the
   priority order (Core Behavior → Edge Cases → User Expectations → Scope Boundaries → Success
   Criteria), the 3–8 question range, and the "Other → record verbatim" rule. Keep the
   4-option example block (it documents the option shape the Phase 2 tool restores in FLUX_12).
4. Keep verbatim: the MANDATORY OVERRIDE preamble; STEP 5's `lsd … || find` research block;
   the STEP 6 augmented-file section template (Business Requirements / Implementation Research
   / Implementation Plan / Definition of Done) including its "**Exclude:** unit/functional/
   integration tests, benchmarks, extensive documentation, multiple options" line; the
   "print the full absolute filepath as the VERY LAST LINE" output rule.

### SUBTASK 2: `prompts/flux/split.md`

1. Frontmatter: delete `name: split`; keep `argument-hint` + `description`.
2. Tool rename (1 site, HARD CONSTRAINTS): `create_file` → `write`.
3. No GAP sites (grep confirms zero `ask_user_question` hits).
4. Keep verbatim: the `< 8 char` uppercase prefix rule; the subtask frontmatter
   (`stage: new, status: done`); the body requirements (OBJECTIVE / SUBTASKn / definition of
   done / research notes); §3.3/3.4 "no tests / no benchmarks" statements; STEP 4's
   `status: complete` + `mv` to `done/$SESSION_TS/` (the renderer tolerates `complete` —
   spec §3.4.2); the HARD CONSTRAINT allow-list (write subtasks + move the original only).

### SUBTASK 3: `prompts/flux/aug.md`

1. Frontmatter: delete `name: aug`; keep `argument-hint` + `description`.
2. Tool renames (3 sites): `create_file` → `write` and `replace_in_file` → `edit` (HARD
   CONSTRAINTS); `invoke_agent` → `subagent` (MULTI-TASK MODE) — rewrite as "use the `subagent`
   tool — parallel foreground calls only; NEVER background" (spec §3.3 rule 4).
3. FLUX-GAP (1 site): STEP 1's empty-arg interactive task selection — mark + interim
   plain-text selection instruction ("list the todo files numbered; ask the user to reply with
   numbers or names; wait for the reply").
4. Keep verbatim: the MANDATORY OVERRIDE preamble; the stack-detection bash (with
   `bun … || echo "JavaScript/TypeScript"` fallback); the `./tmp` third-party-clone
   instruction; "REPLACE THE ORIGINAL FILE in place"; the prescriptive-path paragraph
   ("always select the most feature-rich, code-correct option"); the filepath-last-line rule.

### SUBTASK 4: Sweep + behavioral check

```bash
cd /Users/davidmaple/cyrup.ai/cyrup-flux
rg -n 'create_file|replace_in_file|read_file|invoke_agent' prompts/   # expect: no hits
rg -c 'FLUX-GAP: ask_user_question' prompts/flux/ask.md prompts/flux/aug.md  # expect: 1 and 1
```

Behavioral (continue in the `/tmp/flux-scratch` repo from FLUX_02):

- `/flux/ask DARK_MODE` — the agent asks plain-text lettered questions one at a time, then
  rewrites the todo file in place with the augmented sections; frontmatter becomes
  `stage: ask, status: done`.
- `/flux/split DARK_MODE` — produces `DARK_MODE_1.md` … (or its chosen prefix) in `todo/`
  with `stage: new, status: done`, and moves the original to `done/<SESSION_TS>/` with
  `stage: split, status: complete`.
- `/flux/aug 1` — augments each subtask in place; `stack.env` is written on first run
  (`Rust` is not expected here — scratch repo has no manifests, so `software`).

## RESEARCH NOTES

- The multi-task argument grammar (empty → interactive; `all`/`1` → sequential; N>1 → parallel;
  else filename with `$FLUX_BASE/todo/` + `.md` inference) is load-bearing prompt text shared
  by ask/aug/exec/qa — do not "simplify" it (spec §1.4).
- `done/<SESSION_TS>` grouping is what `/flux/status` (FLUX_07) renders; the `SESSION_TS`
  fallback (`date +%Y-%m-%d-%H-%M` when `session.env` is missing) must survive verbatim.
- `subagent` tool semantics for the multi-task rewrite:
  [`../../crates/cyrup-ext-subagents/src/extension.rs`](../../crates/cyrup-ext-subagents/src/extension.rs)
  (foreground parallel fan-out; FLUX_13 aligns the refill loop wording).

## DEFINITION OF DONE

- [ ] `/flux/ask`, `/flux/split`, `/flux/aug` load and expand.
- [ ] Sweeps produce exactly the expected results; 2 new GAP markers in place.
- [ ] Scratch run reproduces every frontmatter transition and the `done/<SESSION_TS>/` move
      listed above; no file outside the task files is touched by ask/aug (their HARD
      CONSTRAINTS hold in practice).

No tests to be written. No benchmarks to be written.
