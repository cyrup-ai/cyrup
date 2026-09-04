# Flux — the structured development pipeline

Flux is a file-persisted development pipeline: `new → ask → split → aug → exec → qa → tests →
commit → create-pr`. Each stage is a slash command that reads and writes plain Markdown task files
on disk, so a crash or a fresh session never loses pipeline state — the next `/flux/*` command
picks up exactly where the task files left off.

Unlike [subagents](subagents.md), [the permission system](permissions.md) and
[intercom](intercom.md), Flux is not gated behind an environment variable or a config file. It is a
[native extension](overview.md) compiled into the binary and attached unconditionally at startup —
the whole point of moving it into cyrup is that it works with no install step. The one place it
turns itself off is inside a [subagent](subagents.md) child process, so a child spawned to do one
focused task doesn't inherit fifteen pipeline commands and a skill for a workflow it isn't running.

## What you get

- **Fifteen `/flux/*` prompt templates** — one per pipeline stage — contributed to every session's
  command list, plus a bundled skill that explains the pipeline to the model.
- **Three native commands** that render deterministically with no model call:
  `/flux/status`, `/flux/cheatsheet`, `/flux/about`.
- **A `ctrl+alt+f` overlay** — the same status panel as `/flux/status`, drawn with real colour
  inside the TUI instead of plain text. (The chord is `ctrl+alt+f`, not `ctrl+f`: `ctrl+f` is the
  editor's forward-char motion, and an extension shortcut would take it away from you.)
- **A structured `ask_user_question` tool** the pipeline templates use to ask you clarifying
  questions one at a time mid-task, with single- or multi-select options.

The fifteen templates and the skill are compiled into the binary and copied out to
`<agent dir>/flux/resources/` (by default `~/.cyrup/agent/flux/resources/`) the first time a
session starts, and again whenever a new cyrup build ships different content. The copy is
non-destructive: a template you have edited there is kept as `<name>.bak` before the fresh one
lands, and a file of your own that was there first is never touched. Set `CYRUP_FLUX_RESOURCES_DIR`
to point at a tree you maintain yourself instead; if either location is missing at startup you
get a warning naming the path rather than a silent loss of the `/flux/*` commands.

All pipeline state lives under `~/.flux/<flattened-cwd>/`, where `<flattened-cwd>` is your current
working directory with every run of non-alphanumeric characters collapsed to a single `-`. That
layout is byte-identical to the upstream tool Flux was ported from, so a project's task tree is
readable by either.

## The core pipeline

```
new -> ask -> split -> aug -> exec -> qa -> tests -> commit -> create-pr
```

| Step | Command | What happens |
|---|---|---|
| 1 | `/flux/new <description\|Jira ticket>` | Creates a task `.md` file from a description or ticket |
| 2 | `/flux/ask <task-file\|all>` | Asks clarifying questions one at a time, researches the codebase, augments the task file |
| 3 | `/flux/split <task-file>` | Splits a large task into focused subtask files; archives the original |
| 4 | `/flux/aug <task-file\|N\|all>` | Deep research pass — enriches the task file with citations to existing code |
| 5 | `/flux/exec <task-file\|N\|all>` | Implements exactly what the task says — no scope creep |
| 6 | `/flux/qa <task-file\|N\|all>` | Rates the implementation 1–10; a 10/10 archives it, anything else gets refined in place |
| 7 | `/flux/tests` | Runs your test suite, fixes regressions this branch introduced, leaves pre-existing failures alone |
| 8 | `/flux/commit` | Generates a commit message from the diff, asks for confirmation, commits — never pushes |
| 9 | `/flux/create-pr` | Opens a PR from the branch; safe to re-run, shows the existing PR if one is already open |

`aug`, `exec` and `qa` share one argument grammar: a filename processes that one task, a bare
integer `N` fans the work out across `N` parallel subagents over every file in `todo/`, `1` or
`all` processes every task file sequentially, and no argument at all lists `todo/*.md` and asks
which to run. The `N` form needs the `subagent` tool, which only the opt-in
[subagents](subagents.md) extension provides (`CYRUP_SUBAGENTS=1` or a `subagents/config.json`);
Flux itself is always on, so without that opt-in `N` tells you once that subagents are not
available and runs the tasks sequentially instead.

Three more pipelines exist alongside the core one:

- **Self-review** — `review → address-feedback → ask all → exec → qa → tests → commit`, for
  cleaning up your own change before opening a PR.
- **Addressing external feedback** — `address-feedback <zip> → ask all → exec → qa → tests →
  commit`, for a review someone else left on your PR.
- **`/flux/auto-pilot`** — runs the entire core pipeline from one prompt end to end, pausing only
  at `/flux/ask` for clarifying questions.

## First-time setup (optional)

```
/flux/config TEST_CMD="cargo test --workspace --no-fail-fast"
```

This writes `config.env` under the pipeline's state directory with the test command `/flux/tests`
should run. `TEST_CMD` supports chaining (`&&`). Skip this entirely and go straight to `/flux/new`
if you don't plan to use `/flux/tests` yet — every other stage works without it.

## Checking pipeline state

```
/flux/status              # everything: todo, done, review
/flux/status todo         # just the todo section
/flux/status done review  # any combination of todo / done / review
```

Or press `ctrl+alt+f` for the same information as a colour overlay without leaving your current view.

`/flux/cheatsheet` prints the full command table shown above (optionally filtered to one pipeline
with `/flux/cheatsheet A`, `B`, `C` or `D`), and `/flux/about` prints a short description of the
pipeline and where its state lives.

You can also inspect the task tree directly, without any of the above:

```sh
ls ~/.flux/$(printf '%s' "$(pwd -P)" | tr -cs 'a-zA-Z0-9' '-')/todo/
```

## Design principles worth knowing

- **Task files are the source of truth.** `/flux/exec` never modifies a task file beyond its
  frontmatter; only `/flux/qa` edits or archives one.
- **No tests or benchmarks live in a task file.** `/flux/tests` owns tests as a distinct step.
- **`/flux/exec` and `/flux/qa` never run `git` commands** — other agents may be working on the
  same repository concurrently.
- Every stage's response proposes the next command to run, with the right arguments already
  filled in.

## Where to go next

[How extensions work](overview.md) covers the native-extension mechanism Flux uses to ship inside
the binary. [Subagents](subagents.md) is what powers Flux's `N`-way parallel fan-out in `aug`,
`exec` and `qa`.
