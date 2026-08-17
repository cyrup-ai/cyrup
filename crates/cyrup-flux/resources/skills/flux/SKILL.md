---
name: flux
description: |
  Run the Flux structured development pipeline (new → ask → split → aug → exec →
  qa → tests → commit → create-pr) whose state lives in ~/.flux/<flattened-cwd>/.
  Use when the user invokes /flux/* commands, asks about flux task files, pipeline
  stages A–D, auto-pilot, or wants to resume pipeline state after a crash.
---

# Flux — structured AI dev pipeline

A structured, file-persisted development pipeline organized around **task files** stored in
`~/.flux/<flattened-dir>/todo/`. Every `/flux/*` command guides you through one stage of the
lifecycle and proposes the next. "Task" and "task-file" mean the same thing: an `.md` file in the
flux todo directory.

**Flux root directory:** `~/.flux/<flattened-dir>/`, where `<flattened-dir>` is the current
working directory with every run of non-alphanumeric characters collapsed to a single `-`
(`tr -cs 'a-zA-Z0-9' '-'` applied to `pwd -P`). All task, review, and config files live under this
root automatically — no manual setup required.

**Optional first-time setup:** `/flux/config` writes `config.env` with the project's test command
(`TEST_CMD`), used only by `/flux/tests`. Skip it and go straight to `/flux/new` if you don't plan
to use `/flux/tests` yet.

## The Core Pipeline (Pipeline A)

```
new -> ask -> split -> aug -> exec -> qa -> tests -> commit -> create-pr
```

| Step | Command | What happens |
| --- | --- | --- |
| 1 | `/flux/new <prompt\|Jira ticket>` | Creates a task `.md` file from a description or Jira ticket |
| 2 | `/flux/ask <task-file\|all>` | Asks clarifying questions one at a time, researches the codebase, augments the task file |
| 3 | `/flux/split <task-file>` | Splits a large task into focused subtask files; moves the original to `done/<SESSION_TS>/` |
| 4 | `/flux/aug <task-file\|N\|all>` | Deep research pass — explores source, finds existing code, enriches the task file with citations |
| 5 | `/flux/exec <task-file\|N\|all>` | Implements exactly what the task says — no scope creep |
| 6 | `/flux/qa <task-file\|N\|all>` | Rates the implementation 1–10; 10/10 moves the file to `done/<SESSION_TS>/`, else refines in place |
| 7 | `/flux/tests` | Runs the test suite; fixes regressions this branch introduced; leaves pre-existing failures alone |
| 8 | `/flux/commit` | Generates a commit message from the diff, asks for confirmation, commits (never pushes automatically) |
| 9 | `/flux/create-pr` | Opens a GitHub PR from the branch; idempotent — shows the existing PR if one already exists |

Other pipelines: **Pipeline B** (self-review: `review → address-feedback → ask all → exec → qa →
tests → commit`), **Pipeline C** (review received on your PR: `address-feedback <zip> → ask all →
exec → qa → tests → commit`), and `/flux/auto-pilot`, which orchestrates the full Pipeline A
end-to-end from one prompt, pausing only at `/flux/ask` for clarifying questions.

## Unified argument grammar

`aug`, `exec`, and `qa` share one argument grammar:

| Argument | Mode | Behavior |
| --- | --- | --- |
| filename (e.g. `NOTIFS`) | Single-task | Process that one task file; path/`.md` inferred |
| `N` (a bare integer > 1) | Multi-task | Spawn `N` parallel subagents over `todo/*.md` |
| `1` or `all` | Sequential | Process every task file one after another, no subagents |
| (empty) | Interactive | List `todo/*.md` and ask which to process |

## Native commands (not templates)

`/flux/status`, `/flux/cheatsheet`, and `/flux/about` are **native commands** built into this
extension, not prompt templates — they render deterministically and never invoke the model. Until
they exist in a given cyrup build, pipeline state can be inspected manually:

```
ls ~/.flux/$(printf '%s' "$(pwd -P)" | tr -cs 'a-zA-Z0-9' '-')/todo/
```

## Reference docs

References are relative to this file's directory (`reference/`), reachable with the `read` tool:

- [`reference/pipeline.md`](reference/pipeline.md) — the full command table and Pipelines A–D
- [`reference/cheatsheet.md`](reference/cheatsheet.md) — quick-reference command cheatsheet
- [`reference/synopsis.md`](reference/synopsis.md) — long-form design synopsis
- [`reference/README.md`](reference/README.md) — the original TL;DR and design principles

## Crash resume + context hygiene

All pipeline state is written to disk under `~/.flux/<flattened-dir>/` at every step, so a crash
or a fresh session loses nothing — the next `/flux/*` invocation picks up exactly where the task
files left off. Because `aug`'s research output persists to the task file rather than to
conversation state, starting a fresh session between pipeline steps is safe and often preferable
to carrying a long transcript forward: the next command reads its input from the task file, not
from prior turns.

## Key design principles

- Task files are the source of truth — `exec` never modifies them beyond frontmatter; only `qa`
  edits or moves them.
- No tests, no benchmarks in task files — `/flux/tests` owns tests, and is a separate step.
- No `git` commands during `exec`/`qa` — other agents may be working concurrently.
- Every step proposes the next step with the right arguments.
