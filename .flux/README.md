# `.flux` — the flux task queue

Working state for the `/task`, `/ask`, `/split`, `/aug`, `/exec`, `/qa` and `/address-feedback`
commands in `.claude/commands/`. Checked in on purpose: the queue travels with the repo, so it is
visible in review, survives a fresh clone, and is the same for everyone working on the project.

| directory   | holds                                                                 |
| ----------- | --------------------------------------------------------------------- |
| `todo/`     | queued tasks — **stacks**, `/task` only ever adds                      |
| `done/`     | finished tasks, filed by session timestamp; `/qa` moves them here      |
| `review/`   | code-review findings by severity; `/code-review` writes, `/address-feedback` drains |
| `research/` | output path for research-type tasks, written by `/exec`               |

Resolved as `<repo root>/.flux` from any subdirectory. Export `FLUX_BASE` to point elsewhere — set
it to a path outside the repo if you would rather keep your queue private.

`session.env` and `stack.env` are per-machine scratch and are not tracked.
