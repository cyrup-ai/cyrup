# `.flux` — the flux task queue

Working state for the `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa` and `/address-feedback`
commands in `.claude/commands/`. Checked in on purpose: the queue travels with the repo, so it is
visible in review, survives a fresh clone, and is the same for everyone working on the project.

| directory   | holds                                                                 |
| ----------- | --------------------------------------------------------------------- |
| `todo/`     | queued tasks — **stacks**. `/task` adds one, `/address-feedback` moves findings in, `/split` replaces one with its subtasks |
| `done/`     | finished tasks, filed by session timestamp; `/qa` moves them here      |
| `review/`   | code-review findings by severity; `/code-review` writes, `/address-feedback` drains |
| `research/` | created by the boilerplate; **nothing writes here** — see below       |

`research/` is created by every command's `mkdir -p` line and populated by none of them.
`/aug` and `/ask` both edit the task file **in place** and are forbidden from creating files;
`exec.md:169` only says that *if a task names a `research/` path as its deliverable*, the run must
write there and leave source alone. So the directory is a convention available to a task, not an
output any command produces on its own.

Resolved as `<repo root>/.flux` from any subdirectory. Export `FLUX_BASE` to point elsewhere — set
it to a path outside the repo if you would rather keep your queue private.

`session.env` and `stack.env` are per-machine scratch and are not tracked.

## Standing rule: this is a port, so "unused" ≠ "dead"

**Before any task removes a `pub` item, a trait, an enum variant, a hook seam, or a parameter,
check `docs/gap-analysis/` for it:**

```bash
grep -rn '\bNAME\b' docs/gap-analysis/
```

A hit that is not marked `CLOSED` means the item belongs to open parity work. A hit that *is*
marked `CLOSED` usually means the item **is** the closed fix — deleting it re-opens the gap.
Either way: do not remove it. Strike it from the task and record the grep result.

This rule exists because it was learned the expensive way. An audit found two hook seams with no
production callers and a task was filed to delete them as dead code. Both were tracked port work:

- `compaction::CompactionHooks` is the subject of **SESS-040**, an open high-severity item —
  *"Compaction cannot be cancelled from the shipped binary."* `03-cyrup-session.md:493` says the
  defect is *"latent until SESS-040 lands a caller, which is precisely why it must land with it."*
  The missing caller was the work item, not evidence of deadness.
- Its `BeforeTreeDecision` widening was **SESS-034, closed 2026-08-14** — closed by deliberately
  adding the `customInstructions`/`replaceInstructions`/`label` channel the deletion removed.
- `TokenCache::estimate_entry` / `invalidate` / `EstimateKind` are **SESS-020, closed** —
  *"the two projections genuinely differ, so the split key is load-bearing."*

Tests do not catch this. Deleting a seam and the tests that exercise it is self-consistent: the
build stayed clean, clippy stayed at zero and every remaining test passed. The only signal is
`docs/gap-analysis/`.

**When an audit lens asks "what is unreferenced?", the answer in this repo is a list of
candidates for *wiring up*, not for deletion.** The goal is parity with pi.
