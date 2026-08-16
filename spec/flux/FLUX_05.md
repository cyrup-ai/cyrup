---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_05 — Port git/GitHub templates + `auto-pilot.md` (pipeline A complete)

## OBJECTIVE

Port the remaining seven templates —
[`commit.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/commit.md),
[`create-pr.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/create-pr.md),
[`review.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/review.md),
[`address-feedback.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/address-feedback.md),
[`rebase.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/rebase.md),
[`squash-commits.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/squash-commits.md),
[`auto-pilot.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/auto-pilot.md) —
into `/Users/davidmaple/cyrup.ai/cyrup-flux/prompts/flux/`. This completes the 15-template
Phase 1 set; the package then carries the whole interactive pipeline.

Apply parent spec [§3.3 rules 1–9](../flux.md).

## SUBTASKS

### SUBTASK 1: `prompts/flux/commit.md`

1. Frontmatter: delete `name: commit`; keep `argument-hint: amend` + `description`.
2. FLUX-GAP (2 sites): STEP 2's no-ticket commit-type question (feat/fix/chore/refactor/
   docs/test) and STEP 5's commit confirmation ("Yes, commit now" / "No, let me review first",
   plus the amend variant) — mark each + interim plain-text question.
3. Keep verbatim: the amend-mode flow (`git log -1 --format=%B`, the force-push warning); the
   branch-ticket prefix detection (`[A-Z]+-[0-9]+`); the heredoc inline-message rule ("Never
   use `$DETAILED_MSG` or any shell variable"); "DO NOT push unless the user specifically
   asks"; the HARD CONSTRAINT.

### SUBTASK 2: `prompts/flux/create-pr.md`

1. Frontmatter: delete `name: create-pr`; keep `description` (no `argument-hint` upstream).
2. No renames, no GAP sites (grep confirms).
3. Keep verbatim: the main/master stop; the idempotent existing-PR lookup (`gh pr view --json
   number,url,state,title`); the `COMMIT_COUNT=0` warning stop; the `gh pr create` invocation
   shape with the task-derived body.

### SUBTASK 3: `prompts/flux/review.md`

1. Frontmatter: delete `name: review`; keep `argument-hint` + `description`.
2. Tool renames (3 sites): `invoke_agent` → `subagent` (STEP 6) — including the
   "`run_in_background: false`" phrase, which becomes "foreground calls only; NEVER
   background" (spec §3.3 rule 4); `create_file`/`replace_in_file` → `write`/`edit`.
3. FLUX-GAP (1 site): the PR-posting confirmation — mark + interim plain-text question.
4. Keep verbatim: the PR-number vs no-PR-number branch setup (1a/1b); the merge-base scoping
   rationale callout ("Why `$MERGE_BASE` not `origin/$PARENT_BRANCH` …"); the SCOPE CONSTRAINTS
   (newly introduced issues only; no pre-existing/style/tests flags); the agent-count table
   (1–5 files → 1–3 agents … 50+ → 12–15 max); the severity taxonomy writing
   `review/<severity>/` files; the sub-agent prompt template (with `{{MODULE_NAME}}` etc.).

### SUBTASK 4: `prompts/flux/address-feedback.md`

1. Frontmatter: delete `name: address-feedback`; keep `argument-hint` + `description`.
2. FLUX-GAP (1 site): STEP 1's empty-arg routing question (review folder / zip path /
   something else) — mark + interim plain-text question.
3. Keep verbatim: the zip routing (`test -f "$ARGUMENTS"` → error stop); the review→todo
   conversion rules; the HARD CONSTRAINT allow-list (unzip, move between `review/` and
   `todo/`, frontmatter updates only; no `./src/` changes; no git).

### SUBTASK 5: `prompts/flux/rebase.md` + `prompts/flux/squash-commits.md`

1. Frontmatter: delete the `name:` lines; keep `description`s.
2. FLUX-GAP (5 sites in rebase, 7 in squash-commits): every `ask_user_question` confirmation
   (clean-tree check, proceed confirmations, force-push acknowledgment, etc.) — mark each +
   interim plain-text confirmation. Preserve the exact options and which answer aborts.
3. Keep verbatim: all git command blocks (these are the two heaviest git users — 38 and 27
   `git ` hits); the detached-HEAD gotcha; the commit-list/file-list capture-and-verify
   pattern; the force-push warnings.

### SUBTASK 6: `prompts/flux/auto-pilot.md`

1. Frontmatter: delete `name: auto-pilot`; keep `argument-hint` + `description`.
2. Tool rename (1 site): `read_file` → `read` (STEP 1 file-path branch).
3. Keep verbatim: the Jira-key branch (`get_issue_by_key_or_link` — MCP, spec §3.3 rule 7);
   the 8-step orchestration (2.1–2.8) with `▶`/`✓` announcements; the exec/qa max-3-cycles
   loop; the tests 3-fix-cycles-then-continue rule; the HARD CONSTRAINT ("orchestrates … does
   not implement logic of its own").
4. No GAP sites — auto-pilot delegates questioning to `/flux/ask` (whose GAP marker landed in
   FLUX_03).

### SUBTASK 7: Full-package sweep + end-to-end check

```bash
cd /Users/davidmaple/cyrup.ai/cyrup-flux
rg -n 'create_file|replace_in_file|read_file|invoke_agent' prompts/   # expect: no hits
rg -n 'ask_user_question' prompts/                                     # expect: FLUX-GAP comments only
rg -l 'FLUX-GAP: ask_user_question' prompts/flux/ | wc -l              # expect: 11
rg -o 'FLUX-GAP: ask_user_question' prompts/flux/ | wc -l              # expect: 25
rg -n '//flux' prompts/ | rg -v 'Valid //flux commands'                # expect: no hits
```

(11 files / 25 sites per spec §0.3: new×2, ask, aug, exec, qa, config×3, commit×2, review,
address-feedback, rebase×5, squash-commits×7. The `//flux` grep exempts the literal
"Valid //flux commands" whitelist line, which is byte-identical upstream text — spec §1.4.)

End-to-end (scratch repo, continuing from FLUX_04): run the full pipeline A loop once —
`/flux/new` → `/flux/ask` → `/flux/split` → `/flux/aug` → `/flux/exec` → `/flux/qa` →
`/flux/tests` → `/flux/commit` — per spec [§7 Phase 1](../flux.md). On a feature branch with
`gh` available, optionally exercise `/flux/review` (writes `review/<severity>/`) and
`/flux/create-pr` (idempotent lookup path).

## RESEARCH NOTES

- `review.md`'s `run_in_background: false` is the only place upstream names that parameter;
  the `subagent` tool's foreground/parallel semantics are in
  [`../../crates/cyrup-ext-subagents/src/extension.rs`](../../crates/cyrup-ext-subagents/src/extension.rs).
- `gh` and `git` invocations run through cyrup's `bash` tool unchanged (spec §1.4).
- After this task the package is content-complete for Phase 1: 15 templates + `_docs/` +
  (FLUX_06) the skill.

## DEFINITION OF DONE

- [ ] All 15 templates load under their `/flux/<step>` names with correct
      descriptions/argument-hints.
- [ ] The five sweeps above produce exactly the expected results (0 rename hits; 25 GAP
      markers across 11 files; no stray `//flux` outside the whitelist lines).
- [ ] The pipeline A loop runs end-to-end in the scratch repo with the state transitions of
      spec §7 Phase 1 (task created → per-step frontmatter rewrites → split subtasks + original
      moved → exec done → qa 10/10 move to `done/<SESSION_TS>/` / <10 needs-rework → commit
      after confirmation).

No tests to be written. No benchmarks to be written.
