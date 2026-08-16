---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_12 — FLUX-GAP sweep: restore structured questions at all 25 sites

## OBJECTIVE

Upgrade every `<!-- FLUX-GAP: ask_user_question -->` interim site in the 15 templates to the
real `ask_user_question` tool (FLUX_10), restoring the structured-question wording from the
code-puppy originals (spec [§3.4.4](../flux.md) final step, [§0.3](../flux.md) site
inventory). Edits happen in the CANONICAL home — `crates/cyrup-ext-flux/resources/prompts/flux/`
(FLUX_11) — then re-sync the package.

## SUBTASKS

### SUBTASK 1: The 25-site sweep

```bash
RES=/Users/davidmaple/cyrup.ai/cyrup/crates/cyrup-ext-flux/resources
rg -n 'FLUX-GAP: ask_user_question' "$RES/prompts/flux/"   # expect exactly 25 hits in 11 files
```

Site inventory (spec §0.3 — verify against this list before starting and after finishing):

| File | Sites | What to restore |
|---|---|---|
| `new.md` | 2 | leftover-files choice (Discard / Back up); clarify-if-needed |
| `ask.md` | 1 | STEP 4 one-at-a-time clarifying questions (2–4 options with implication descriptions; priority order; "Other → record verbatim") |
| `aug.md` | 1 | empty-arg interactive task selection (single/multi select) |
| `exec.md` | 1 | empty-arg interactive task selection |
| `qa.md` | 1 | empty-arg interactive task selection |
| `config.md` | 3 | update-yes/no; new-file "Done, verify now / Exit"; TEST_CMD keep-current/new-value (free text — see SUBTASK 2) |
| `commit.md` | 2 | commit-type pick (feat/fix/chore/refactor/docs/test); commit confirmation (+ amend variant) |
| `review.md` | 1 | PR-posting confirmation |
| `address-feedback.md` | 1 | empty-arg routing (review folder / zip path / something else) |
| `rebase.md` | 5 | the five confirmations (exact options + which answer aborts) |
| `squash-commits.md` | 7 | the seven confirmations (exact options + which answer aborts) |

At each site: delete the `<!-- FLUX-GAP: ask_user_question -->` comment and the interim
plain-text paragraph; restore the code-puppy wording with the tool spelled
`ask_user_question` (the originals are in
[`../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/)
— copy their question/header/options text verbatim, including each option's description).
Reference shape (from `ask.md` STEP 4): question text, a `Header`, 2–4 options each with
`Label` + `Description`, one at a time, wait for the answer before the next.

### SUBTASK 2: The two non-select sites

- `config.md` STEP 2b ("New value - use Other below") needs FREE TEXT: the tool's options are
  2–4 choices, so prescribe the two-step the tool supports: first option "Keep current:
  `<value>`", second option "Enter a new value"; when the second is picked, the agent asks for
  the value in plain text and uses the reply. Write exactly that into the template (it mirrors
  code-puppy's "Other" free-text escape without inventing a new tool parameter).
- `ask.md` STEP 4's "If user selects 'Other', record their response verbatim": keep an
  `Other` option at every ask.md question and the same plain-text follow-up rule.

### SUBTASK 3: Re-sync the package + sweep verification

```bash
RES=/Users/davidmaple/cyrup.ai/cyrup/crates/cyrup-ext-flux/resources
PKG=/Users/davidmaple/cyrup.ai/cyrup-flux
rsync -a --delete "$RES/prompts/flux/" "$PKG/prompts/flux/"
cd "$PKG" && git add -A && git commit -m "Restore structured ask_user_question wording (25 sites)"

rg -n 'FLUX-GAP' "$RES/prompts/flux/"                       # expect: no hits
rg -n 'plain text with 2–4 lettered options' "$RES/prompts/flux/"  # expect: no hits (interim wording gone)
rg -c 'ask_user_question' "$RES/prompts/flux/"*.md          # expect: ≥25 across the 11 files
diff -r "$RES/prompts/flux/" "$PKG/prompts/flux/"           # expect: silent
```

### SUBTASK 4: Behavioral check

In the TUI (scratch repo with leftover todo files): `/flux/new another task` — the
leftover-files question now arrives as the structured select dialog (Discard / Back up), not a
plain-text question. `/flux/ask <file>` — clarifying questions arrive one at a time as select
dialogs with option descriptions; picking `Other` yields the plain-text follow-up.
`/flux/config` on an existing `config.env` — the update-yes/no dialog appears.

## RESEARCH NOTES

- The tool's label-projection (`"label — description"` display rows) means option descriptions
  reach the user through the dialog even though `UiKind::Select` carries only strings (spec
  §0.4) — no template wording change is needed to benefit.
- The interim markers were placed by FLUX_02–05 with `rg -c` counts recorded per file; the
  pre/post `rg` counts above are the completeness gate (spec §5.4).

## DEFINITION OF DONE

- [ ] Zero `FLUX-GAP` markers remain; zero interim plain-text paragraphs remain; the 11 files
      carry the restored structured wording with verbatim option text.
- [ ] Package re-synced and byte-identical (`diff -r` silent); package committed.
- [ ] Behavioral check shows the select dialog at the three exercised sites.

No tests to be written. No benchmarks to be written.
