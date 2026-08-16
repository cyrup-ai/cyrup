---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_02 — Port `new.md` + `config.md` (state bootstrap templates)

## OBJECTIVE

Port the two templates that create and maintain the `~/.flux/<dir>/` state files —
[`new.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/new.md) (writes
`todo/<TASK>.md` + `session.env`) and
[`config.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/config.md) (writes
`config.env`) — from the vendored code-puppy source into the package at
`/Users/davidmaple/cyrup.ai/cyrup-flux/prompts/flux/`. These two are ported together because
every later template reads the state these two write.

Apply parent spec [§3.3 rules 1–9](../flux.md) exactly. The rules are not repeated here in
full; the per-file deltas below are the complete work list.

## SUBTASKS

### SUBTASK 1: `prompts/flux/new.md`

Copy [`../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/new.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/new.md), then:

1. **Frontmatter**: delete the `name: new` line (cyrup derives the name from the path — spec
   §3.3 rule 1). Keep `argument-hint: task_description | JIRA_ticket_id` and `description:`.
2. **Tool rename** (1 site): in HARD CONSTRAINTS, `create_file` → `write`
   ("The `write` file path MUST use the exact `FLUX_BASE` value …").
3. **FLUX-GAP markers** (2 sites), each replaced per spec §3.3 rule 5 — insert the HTML comment
   `<!-- FLUX-GAP: ask_user_question -->` on the line above, then the interim instruction
   "ask the user one question at a time in plain text with 2–4 lettered options (A/B/C/D, each
   with a one-line implication); wait for the reply before continuing":
   - STEP 1 leftover-files question ("Found existing task files … Discard them / Back them up").
   - STEP 3.3.2 "Clarify if needed" (non-Jira branch).
4. **Keep verbatim**: the Jira pattern `^[A-Z]+-[0-9]+$`; the entire MCP branch including the
   "not configured → stop" path (spec §3.3 rule 7 — cyrup exposes MCP tools the same way,
   spec §0.4); the Jira-markup→Markdown table; the `session.env` write block; the
   `=================` + `$ARGUMENTS` trailer (cyrup really substitutes it — spec §0.2).
5. Do NOT touch the bash snippets (spec §3.3 rule 6) or the "Valid //flux commands" whitelist
   (byte-identical, spec §1.4).

### SUBTASK 2: `prompts/flux/config.md`

Copy [`../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/config.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/config.md), then:

1. **Frontmatter**: delete `name: config`. Keep `description:` (it has no `argument-hint`).
2. **FLUX-GAP markers** (3 sites), same interim-instruction treatment:
   - STEP 1 "Config looks good. Do you want to update any values?" (Yes/No).
   - STEP 2a "Config file created … Ready to continue?" (Done, verify now / Exit).
   - STEP 2b "What command runs your test suite?" (Keep current / New value).
   For 2b preserve the semantics that "New value" means free text — the interim wording for
   that site is: "ask in plain text for the TEST_CMD value, showing the current value as the
   default; an empty reply keeps the current value".
3. **Keep verbatim**: the new-file flow writes `config.env` immediately with the commented
   heredoc (no questions); the overwrite-completely rule; the STEP 4 confirm output format; the
   HARD CONSTRAINT (only `$FLUX_BASE/config.env` may be written).
4. No tool renames in this file (grep confirms zero `create_file`/`replace_in_file`/`read_file`/
   `invoke_agent` hits).

### SUBTASK 3: Sweep + behavioral check

```bash
cd /Users/davidmaple/cyrup.ai/cyrup-flux
rg -n 'create_file|replace_in_file|read_file|invoke_agent' prompts/   # expect: no hits
rg -n 'ask_user_question' prompts/                                     # expect: only inside FLUX-GAP comments
rg -c 'FLUX-GAP: ask_user_question' prompts/flux/new.md prompts/flux/config.md  # expect: 2 and 3
```

Then in a scratch git repo (`mkdir /tmp/flux-scratch && cd /tmp/flux-scratch && git init`):

- `cyrup -p "/flux/new add a dark mode toggle"` — confirm the expanded text reaches the model
  (template expansion runs through the session preflight, spec §0.2) and the agent creates
  `~/.flux/-tmp-flux-scratch/todo/DARK_MODE.md` with `stage: new, status: done` frontmatter and
  writes `session.env` (`SESSION_TS=YYYY-MM-DD-HH-MM`).
- `cyrup -p "/flux/config"` — confirm `config.env` is written with the commented `TEST_CMD=`
  heredoc and the plain-text interim question is asked.

## RESEARCH NOTES

- State model (why these files first): spec [§1.3](../flux.md) and [§3.2](../flux.md) —
  `session.env` is consumed by split/qa's `done/$SESSION_TS/` move; `config.env`'s `TEST_CMD`
  is consumed by `tests.md` (FLUX_04).
- The cwd-flattening rule (`tr -cs 'a-zA-Z0-9' '-'` = runs of non-alphanumerics → one `-`) is
  why the scratch dir maps to `~/.flux/-tmp-flux-scratch` (leading `/` → leading `-`).
- Template expansion + real `$ARGUMENTS` substitution:
  [`../../crates/cyrup-resources/src/prompt.rs`](../../crates/cyrup-resources/src/prompt.rs).

## DEFINITION OF DONE

- [ ] Both templates load as `/flux/new` and `/flux/config` (TUI command list shows them with
      their `description`/`argument-hint`).
- [ ] The three `rg` sweeps above produce exactly the expected results.
- [ ] Scratch-repo run: task file + `session.env` + `config.env` created with the exact
      frontmatter/content shapes above.
- [ ] All 5 GAP sites carry `<!-- FLUX-GAP: ask_user_question -->` markers.

No tests to be written. No benchmarks to be written.
