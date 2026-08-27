# `.flux` — the flux task queue

Working state for the `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa` and `/address-feedback`
commands in `.claude/commands/`. Checked in on purpose: the queue travels with the repo, so it is
visible in review, survives a fresh clone, and is the same for everyone working on the project.

| directory   | holds                                                                 |
| ----------- | --------------------------------------------------------------------- |
| `todo/`     | queued tasks — **stacks**. `/task` adds one, `/address-feedback` moves findings in, `/split` replaces one with its subtasks |
| `backlog/`  | parked tasks — real work, deliberately not in the active queue. **No command reads or writes here**; move a file in or out by hand to change what `/exec all` and `/qa all` will pick up |
| `done/`     | finished tasks, filed by session timestamp; `/qa` moves them here      |
| `review/`   | code-review findings by severity; `/code-review` writes, `/address-feedback` drains |
| `research/` | created by the boilerplate; **nothing writes here** — see below       |

`backlog/` is the queue's pressure valve. `todo/` is what the pipeline acts on — `/exec all`,
`/qa all` and an unargumented `/aug` all enumerate `todo/*.md` — so a queue holding every open
task makes those commands unusable. Parking a task in `backlog/` keeps it checked in and
reviewable without putting it in front of the batch commands. It is an ordinary directory: no
command creates it, drains it, or files anything into it.

`research/` is created by every command's `mkdir -p` line and populated by none of them.
`/aug` and `/ask` both edit the task file **in place** and are forbidden from creating files;
`exec.md:169` only says that *if a task names a `research/` path as its deliverable*, the run must
write there and leave source alone. So the directory is a convention available to a task, not an
output any command produces on its own.

Resolved as `<repo root>/.flux` from any subdirectory. Export `FLUX_BASE` to point elsewhere — set
it to a path outside the repo if you would rather keep your queue private.

`session.env` and `stack.env` are per-machine scratch and are not tracked.
