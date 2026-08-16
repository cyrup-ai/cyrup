---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_13 — Parallel-exec alignment with `subagent` semantics

## OBJECTIVE

Align the multi-task sections of `aug.md` / `exec.md` / `qa.md` with the `subagent` tool's
actual scheduling, completing Phase 3 (spec [§3.5](../flux.md), [§5.5](../flux.md)). The
flux refill-as-they-finish loop is prompt-driven; this task makes the prompt wording honest
about how the tool really batches. Prompt-text only — no code changes.

## SUBTASKS

### SUBTASK 1: Establish the tool's real semantics (research, then record)

Read the `subagent` tool's parallel execution path in
[`../../crates/cyrup-ext-subagents/src/extension.rs`](../../crates/cyrup-ext-subagents/src/extension.rs)
and [`../../crates/cyrup-ext-subagents/src/exec/`](../../crates/cyrup-ext-subagents/src/exec/):
the PARALLEL mode (`tasks:` array, `count`, `concurrency`), foreground vs `async`, and the
[`wait`](../../crates/cyrup-ext-subagents/src/background/wait.rs) tool's
return-on-first-finish behavior. The question to answer definitively: when the model issues N
parallel foreground `subagent` tool calls in one turn, does the runtime start all N at once,
and can the model issue the (N+1)th call immediately after the first returns (the flux refill
loop), or are calls batched?

### SUBTASK 2: Rewrite the multi-task paragraphs (3 files, canonical crate copy)

Edit `crates/cyrup-ext-flux/resources/prompts/flux/{aug,exec,qa}.md`. Replace the MULTI-TASK
MODE spawning paragraph (currently "Use the `subagent` tool — parallel foreground calls only;
NEVER background … spawn new subagents as initial ones complete") with wording grounded in
SUBTASK 1's finding:

- Prescribe issuing up to N `subagent` calls in ONE parallel block (the runtime fans them out
  concurrently), then — as each result returns — issuing the next call, until every
  `$FLUX_BASE/todo/*.md` task is done. If SUBTASK 1 found the runtime batches all N before any
  result streams back, say so explicitly and prescribe batch-of-N waves instead ("launch N,
  wait for the wave, launch the next N") — the wording must match the tool, not the wish.
- Keep untouched: the foreground-only rule; the per-file frontmatter ordering ("update each
  file as it completes, not all at once"); the dependency-collision guidance ("Avoid
  parallelizing tasks that modify the same file(s) … analyze interdependencies carefully");
  the subagent prompt template (single-task steps + `{{absolute_file_path}}` substitution).

### SUBTASK 3: Re-sync the package

```bash
RES=/Users/davidmaple/cyrup.ai/cyrup/crates/cyrup-ext-flux/resources
PKG=/Users/davidmaple/cyrup.ai/cyrup-flux
rsync -a --delete "$RES/prompts/flux/" "$PKG/prompts/flux/"
cd "$PKG" && git add -A && git commit -m "Align multi-task wording with subagent scheduling"
```

### SUBTASK 4: Behavioral check

In the scratch repo, create two independent todo tasks (touching disjoint files) and run
`/flux/exec 2`: both tasks execute via foreground `subagent` fan-out; each task file's
frontmatter flips to `stage: exec, status: done` as ITS subagent returns (not both at the
end); no background runs are launched. Repeat with `/flux/qa 2` on reworked tasks.

## RESEARCH NOTES

- Crash-resume is free: state lives in the task files; rerunning the step resumes (spec §3.5).
- The power-user path already exists and needs no work: users can orchestrate explicitly with
  the subagents extension's `/parallel` command (one subagent per task file).
- This is the final task — after it, spec [§7 Definition of done](../flux.md) is fully
  satisfied across all three phases.

## DEFINITION OF DONE

- [ ] The three multi-task sections describe the tool's verified scheduling (parallel block +
      refill, or N-waves — whichever SUBTASK 1 proved), with all constraint text intact.
- [ ] Package re-synced, byte-identical, committed.
- [ ] `/flux/exec 2` on the two-task fixture shows concurrent execution with per-file
      completion ordering.

No tests to be written. No benchmarks to be written.
